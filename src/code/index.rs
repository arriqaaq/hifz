//! Code indexing orchestrator.
//!
//! Walks a repo (gitignore-honest), chunks each supported file, embeds the
//! chunks, extracts named symbols, and persists `code_file`, `code_chunk`,
//! and `code_symbol` rows. Idempotent via `(mtime_ns, content_hash)`
//! short-circuiting.
//!
//! M2: delete-then-create per file. M3 layers `re_anchor_references` in front
//! of the delete so memory→chunk edges survive edits.

use std::path::Path;
use std::time::SystemTime;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::code::lang::Language;
use crate::code::splitter::{CodeSplitter, RawChunk};
use crate::code::symbols::{RawSymbol, extract_symbols};
use crate::code::walker::{WalkOpts, WalkedFile, walk};
use crate::db::Db;
use crate::embed::Embedder;
use crate::link;

#[derive(Debug, Clone)]
pub struct IndexOpts {
    pub follow_symlinks: bool,
    pub max_file_bytes: u64,
    /// Bound on inflight `index_file` calls. fastembed holds a Mutex so this
    /// caps how many files queue up reading + chunking before embedding.
    pub concurrent_files: usize,
}

impl Default for IndexOpts {
    fn default() -> Self {
        Self {
            follow_symlinks: false,
            max_file_bytes: 2 * 1024 * 1024,
            concurrent_files: 8,
        }
    }
}

#[derive(Debug, Default, serde::Serialize)]
pub struct IndexReport {
    pub indexed: usize,
    pub skipped_unchanged: usize,
    pub chunks: usize,
    pub symbols: usize,
    pub errors: usize,
}

#[derive(Debug)]
pub enum IndexFileOutcome {
    Skipped,
    Indexed { chunks: usize, symbols: usize },
}

#[derive(Debug, SurrealValue)]
struct CodeFileRow {
    id: Option<RecordId>,
    mtime_ns: Option<i64>,
    content_hash: Option<String>,
}

#[derive(Debug, SurrealValue)]
struct CreatedId {
    id: Option<RecordId>,
}

pub async fn index_repo(
    db: &Surreal<Db>,
    embedder: &Embedder,
    project: &str,
    root: &Path,
    opts: &IndexOpts,
) -> Result<IndexReport> {
    let walk_opts = WalkOpts {
        follow_symlinks: opts.follow_symlinks,
        max_file_bytes: opts.max_file_bytes,
        include_hidden: false,
    };
    let files = walk(root, &walk_opts).context("walk failed")?;
    tracing::info!(
        "code::index_repo: project={} root={} candidates={}",
        project,
        root.display(),
        files.len()
    );

    let mut report = IndexReport::default();

    for f in &files {
        match index_walked(db, embedder, project, f).await {
            Ok(IndexFileOutcome::Skipped) => report.skipped_unchanged += 1,
            Ok(IndexFileOutcome::Indexed { chunks, symbols }) => {
                report.indexed += 1;
                report.chunks += chunks;
                report.symbols += symbols;
            }
            Err(e) => {
                tracing::warn!("index_file failed for {}: {e}", f.rel);
                report.errors += 1;
            }
        }
        // Yield to let memory_search/memory_save interleave (G2).
        tokio::task::yield_now().await;
    }

    Ok(report)
}

async fn index_walked(
    db: &Surreal<Db>,
    embedder: &Embedder,
    project: &str,
    f: &WalkedFile,
) -> Result<IndexFileOutcome> {
    // -- Step 1: cheap mtime check ----------------------------------------
    let mut resp = db
        .query("SELECT id, mtime_ns, content_hash FROM code_file WHERE project = $p AND path = $r LIMIT 1")
        .bind(("p", project.to_string()))
        .bind(("r", f.rel.clone()))
        .await?;
    let existing: Vec<CodeFileRow> = resp.take(0).unwrap_or_default();
    let existing = existing.into_iter().next();

    if let Some(ref e) = existing {
        if let Some(stored_mtime) = e.mtime_ns {
            if (stored_mtime as i128) == f.mtime_ns {
                return Ok(IndexFileOutcome::Skipped);
            }
        }
    }

    // -- Step 2: content hash check ---------------------------------------
    let bytes = match std::fs::read(&f.abs) {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!("read failed for {}: {e}", f.rel);
            return Ok(IndexFileOutcome::Skipped);
        }
    };
    let content_hash = sha256_hex(&bytes);

    if let Some(ref e) = existing {
        if e.content_hash.as_deref() == Some(content_hash.as_str()) {
            // Same hash, just touch mtime so future cheap checks skip too.
            if let Some(ref id) = e.id {
                let _ = db
                    .query("UPDATE type::record($id) SET mtime_ns = $m")
                    .bind(("id", id.clone()))
                    .bind(("m", f.mtime_ns as i64))
                    .await;
            }
            return Ok(IndexFileOutcome::Skipped);
        }
    }

    // -- Step 3: chunk + extract symbols ----------------------------------
    let source = match std::str::from_utf8(&bytes) {
        Ok(s) => s.to_string(),
        Err(_) => {
            tracing::debug!("non-utf8 file skipped: {}", f.rel);
            return Ok(IndexFileOutcome::Skipped);
        }
    };
    let lang = Language::from_path(&f.abs).unwrap_or(Language::Plain);
    let splitter = CodeSplitter::for_lang(lang);
    let chunks: Vec<RawChunk> = splitter.split(lang, &source)?;
    if chunks.is_empty() {
        return Ok(IndexFileOutcome::Skipped);
    }
    let symbols: Vec<RawSymbol> = extract_symbols(lang, &source).unwrap_or_default();

    // -- Step 4: embed ----------------------------------------------------
    let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
    let embeddings = embedder.embed_batch(&texts).context("embed_batch failed")?;
    if embeddings.len() != chunks.len() {
        anyhow::bail!(
            "embedder returned {} vectors for {} chunks",
            embeddings.len(),
            chunks.len()
        );
    }

    let now = chrono::Utc::now().to_rfc3339();

    // -- Step 5: upsert code_file -----------------------------------------
    let file_id = match existing.as_ref().and_then(|e| e.id.clone()) {
        Some(id) => {
            db.query(
                "UPDATE type::record($id) SET \
                 abs_path = $abs, language = $lang, size_bytes = $size, mtime_ns = $mtime, \
                 content_hash = $hash, chunk_count = $cc, deleted_at = NONE, indexed_at = $now",
            )
            .bind(("id", id.clone()))
            .bind(("abs", f.abs.to_string_lossy().to_string()))
            .bind(("lang", lang.as_str().to_string()))
            .bind(("size", f.size_bytes as i64))
            .bind(("mtime", f.mtime_ns as i64))
            .bind(("hash", content_hash.clone()))
            .bind(("cc", chunks.len() as i64))
            .bind(("now", now.clone()))
            .await?
            .check()?;
            id
        }
        None => {
            let mut resp = db
                .query(
                    "CREATE code_file SET project = $p, path = $rel, abs_path = $abs, \
                     language = $lang, size_bytes = $size, mtime_ns = $mtime, \
                     content_hash = $hash, chunk_count = $cc, indexed_at = $now RETURN id",
                )
                .bind(("p", project.to_string()))
                .bind(("rel", f.rel.clone()))
                .bind(("abs", f.abs.to_string_lossy().to_string()))
                .bind(("lang", lang.as_str().to_string()))
                .bind(("size", f.size_bytes as i64))
                .bind(("mtime", f.mtime_ns as i64))
                .bind(("hash", content_hash.clone()))
                .bind(("cc", chunks.len() as i64))
                .bind(("now", now.clone()))
                .await?;
            let created: Vec<CreatedId> = resp.take(0).unwrap_or_default();
            created
                .into_iter()
                .next()
                .and_then(|c| c.id)
                .context("CREATE code_file returned no id")?
        }
    };

    // -- Step 6a: snapshot inbound references for re-anchoring (G6) -------
    let archived_chunks = crate::code::link::snapshot_references(db, &file_id)
        .await
        .unwrap_or_default();
    let archived_symbols = crate::code::link::snapshot_symbol_references(db, &file_id)
        .await
        .unwrap_or_default();

    // -- Step 6b: wipe old chunks/symbols/structural edges ----------------
    let _ = db
        .query(
            "DELETE edge WHERE relation = 'part_of' AND \
             (in IN (SELECT VALUE id FROM code_chunk WHERE file = $fid) \
              OR in IN (SELECT VALUE id FROM code_symbol WHERE file = $fid))",
        )
        .bind(("fid", file_id.clone()))
        .await;
    let _ = db
        .query("DELETE code_chunk WHERE file = $fid")
        .bind(("fid", file_id.clone()))
        .await;
    let _ = db
        .query("DELETE code_symbol WHERE file = $fid")
        .bind(("fid", file_id.clone()))
        .await;

    // -- Step 7: write new chunks -----------------------------------------
    let mut chunk_ids: Vec<RecordId> = Vec::with_capacity(chunks.len());
    for (idx, (c, emb)) in chunks.iter().zip(embeddings.into_iter()).enumerate() {
        let chunk_hash = sha256_hex(c.content.as_bytes());
        let mut resp = db
            .query(
                "CREATE code_chunk SET file = $fid, project = $p, path = $path, \
                 language = $lang, chunk_index = $idx, content = $content, \
                 start_line = $sl, end_line = $el, start_byte = $sb, end_byte = $eb, \
                 content_hash = $ch, embedding = $emb, symbols = [], \
                 created_at = $now RETURN id",
            )
            .bind(("fid", file_id.clone()))
            .bind(("p", project.to_string()))
            .bind(("path", f.rel.clone()))
            .bind(("lang", lang.as_str().to_string()))
            .bind(("idx", idx as i64))
            .bind(("content", c.content.clone()))
            .bind(("sl", c.start_line as i64))
            .bind(("el", c.end_line as i64))
            .bind(("sb", c.start_byte as i64))
            .bind(("eb", c.end_byte as i64))
            .bind(("ch", chunk_hash))
            .bind(("emb", emb))
            .bind(("now", now.clone()))
            .await?;
        let created: Vec<CreatedId> = resp.take(0).unwrap_or_default();
        let Some(cid) = created.into_iter().next().and_then(|c| c.id) else {
            continue;
        };
        let _ = link::upsert_edge(
            db,
            &cid,
            &file_id,
            "part_of",
            "system",
            1.0,
            Some("code_chunk part of code_file"),
        )
        .await;
        chunk_ids.push(cid);
    }

    // -- Step 8: write symbols (find primary_chunk by line overlap) -------
    let mut symbol_count = 0usize;
    for s in &symbols {
        // Pick the chunk whose line range best contains the symbol.
        let primary = chunks
            .iter()
            .enumerate()
            .find(|(_, c)| c.start_line <= s.start_line && c.end_line >= s.start_line);
        let primary_chunk_id = primary.and_then(|(i, _)| chunk_ids.get(i).cloned());

        let mut resp = db
            .query(
                "CREATE code_symbol SET project = $p, name = $name, qualified = $qual, \
                 kind = $kind, language = $lang, file = $fid, path = $path, \
                 primary_chunk = $primary, start_line = $sl, end_line = $el, \
                 created_at = $now RETURN id",
            )
            .bind(("p", project.to_string()))
            .bind(("name", s.name.clone()))
            .bind(("qual", s.qualified.clone()))
            .bind(("kind", s.kind.clone()))
            .bind(("lang", lang.as_str().to_string()))
            .bind(("fid", file_id.clone()))
            .bind(("path", f.rel.clone()))
            .bind(("primary", primary_chunk_id.clone()))
            .bind(("sl", s.start_line as i64))
            .bind(("el", s.end_line as i64))
            .bind(("now", now.clone()))
            .await?;
        let created: Vec<CreatedId> = resp.take(0).unwrap_or_default();
        let Some(sid) = created.into_iter().next().and_then(|c| c.id) else {
            continue;
        };
        let _ = link::upsert_edge(
            db,
            &sid,
            &file_id,
            "part_of",
            "system",
            1.0,
            Some("code_symbol part of code_file"),
        )
        .await;
        symbol_count += 1;
    }

    // -- Step 9: re-anchor archived references to new chunks/symbols (G6)
    if !archived_chunks.is_empty() {
        let _ = crate::code::link::re_anchor_references(db, project, &archived_chunks).await;
    }
    if !archived_symbols.is_empty() {
        let _ = crate::code::link::re_anchor_symbol_references(
            db,
            project,
            &file_id,
            &archived_symbols,
        )
        .await;
    }

    Ok(IndexFileOutcome::Indexed {
        chunks: chunk_ids.len(),
        symbols: symbol_count,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Convert a system mtime to nanos-since-epoch (i128). Used by tests + the
/// CLI subcommand; production walker computes this inline.
#[allow(dead_code)]
pub fn mtime_nanos(t: SystemTime) -> i128 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as i128)
        .unwrap_or(0)
}
