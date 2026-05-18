//! Document ingestion: PDF + markdown/txt → `atlas_node{kind:document}`
//! + `atlas_chunk` rows (chunked via hifz-core's splitter, embedded via
//! hifz-core's local embedder). Deterministic node id keyed on
//! `(project, rel_path)` so re-ingest UPSERTs in place.

use std::path::{Path, PathBuf};

use anyhow::Result;
use kernel::embed::Embedder;
use sha2::{Digest, Sha256};

use crate::store::Store;

#[derive(Debug, Default, serde::Serialize)]
pub struct IngestReport {
    pub documents: usize,
    pub chunks: usize,
    pub skipped: usize,
}

const DOC_EXTS: &[&str] = &["pdf", "md", "markdown", "mdx", "txt", "rst", "text"];

fn node_key(project: &str, rel: &str) -> String {
    let mut h = Sha256::new();
    h.update(project.as_bytes());
    h.update([0u8]);
    h.update(rel.as_bytes());
    hex::encode(&h.finalize()[..16])
}

/// Extract plain text. PDF via pure-Rust `pdf-extract`, wrapped in
/// `catch_unwind` (it can panic on malformed PDFs — we skip those, never
/// crash). Markdown/txt read directly.
fn extract_text(path: &Path, ext: &str) -> Option<String> {
    if ext == "pdf" {
        let p = path.to_path_buf();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            pdf_extract::extract_text(&p)
        }));
        match r {
            Ok(Ok(t)) if !t.trim().is_empty() => Some(t),
            _ => None,
        }
    } else {
        std::fs::read_to_string(path)
            .ok()
            .filter(|s| !s.trim().is_empty())
    }
}

fn collect(root: &Path, out: &mut Vec<PathBuf>) {
    if root.is_file() {
        out.push(root.to_path_buf());
        return;
    }
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') {
            continue; // skip dotfiles/dirs
        }
        if p.is_dir() {
            collect(&p, out);
        } else if p
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| DOC_EXTS.contains(&x.to_ascii_lowercase().as_str()))
            .unwrap_or(false)
        {
            out.push(p);
        }
    }
}

pub async fn ingest_path(store: &Store, embedder: &Embedder, root: &Path) -> Result<IngestReport> {
    let mut files = Vec::new();
    collect(root, &mut files);
    let mut report = IngestReport::default();
    let now = chrono::Utc::now().to_rfc3339();

    for path in files {
        let ext = path
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !DOC_EXTS.contains(&ext.as_str()) {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let rel = if rel.is_empty() {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        } else {
            rel
        };

        let Some(text) = extract_text(&path, &ext) else {
            report.skipped += 1;
            continue;
        };
        let label = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&rel)
            .to_string();
        let summary: String = text.chars().take(280).collect();
        let nid = format!("atlas_node:{}", node_key(&store.project, &rel));

        // Node embedding = label + summary (captures topic for clustering).
        let node_emb = embedder.embed_single(&format!("{label}\n{summary}")).ok();

        store
            .db
            .query(
                "UPSERT type::record($id) SET project=$p, kind='document', \
                 label=$label, path=$rel, summary=$summary, embedding=$emb, \
                 cluster=-1, created_at=$now",
            )
            .bind(("id", nid.clone()))
            .bind(("p", store.project.clone()))
            .bind(("label", label))
            .bind(("rel", rel.clone()))
            .bind(("summary", summary))
            .bind(("emb", node_emb))
            .bind(("now", now.clone()))
            .await?
            .check()?;

        // Re-ingest idempotency: replace this node's chunks.
        let _ = store
            .db
            .query("DELETE atlas_chunk WHERE node = type::record($id)")
            .bind(("id", nid.clone()))
            .await;

        for ch in kernel::text::split(&text) {
            let emb = embedder.embed_single(&ch.content).ok();
            store
                .db
                .query(
                    "CREATE atlas_chunk SET node=type::record($id), project=$p, \
                     chunk_index=$idx, content=$c, embedding=$emb, created_at=$now",
                )
                .bind(("id", nid.clone()))
                .bind(("p", store.project.clone()))
                .bind(("idx", ch.index as i64))
                .bind(("c", ch.content.clone()))
                .bind(("emb", emb))
                .bind(("now", now.clone()))
                .await?
                .check()?;
            report.chunks += 1;
        }
        report.documents += 1;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ingest_md_txt_and_skip_bad_pdf() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("README.md"),
            "# Title\n\nThis is the auth flow doc. "
                .repeat(1)
                .to_string()
                + &"More content about tokens and sessions. ".repeat(120),
        )
        .unwrap();
        std::fs::write(dir.path().join("notes.txt"), "short note about jwt").unwrap();
        // A .pdf that is not a real PDF → extract must skip, never panic.
        std::fs::write(dir.path().join("broken.pdf"), b"%PDF-1.4 not really").unwrap();

        let db = kernel::db::connect_mem().await.unwrap();
        crate::store::init_atlas_schema(&db, 384).await.unwrap();
        let store = Store::new(db, "demo");
        let emb = Embedder::new().unwrap();

        let rep = ingest_path(&store, &emb, dir.path()).await.unwrap();
        assert_eq!(rep.documents, 2, "md + txt ingested");
        assert!(rep.chunks >= 2, "README split into multiple chunks");
        assert!(rep.skipped >= 1, "broken pdf skipped, not crashed");

        // Re-ingest is idempotent (UPSERT node, replace chunks).
        let rep2 = ingest_path(&store, &emb, dir.path()).await.unwrap();
        assert_eq!(rep2.documents, 2);
        use surrealdb::types::SurrealValue;
        #[derive(Debug, SurrealValue)]
        struct C {
            c: Option<i64>,
        }
        let mut r = store
            .db
            .query("SELECT count() AS c FROM atlas_node WHERE kind='document' GROUP ALL")
            .await
            .unwrap();
        let rows: Vec<C> = r.take(0).unwrap_or_default();
        assert_eq!(
            rows.into_iter().next().and_then(|x| x.c),
            Some(2),
            "no node duplication on re-ingest"
        );
    }
}
