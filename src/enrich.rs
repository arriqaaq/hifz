//! A-MEM-style insert pipeline for memories.
//!
//! Replaces the deterministic-only `remember::save` flow with an
//! enrich-and-link loop modeled on A-MEM (Xu et al., 2502.12110), but with
//! typed edges (PROV-O / SKOS / IBIS, see `models::EdgeRelation`).
//!
//! ## The pipeline
//!
//! 1. **Caller input**: `title`, `content`, `category` (typed), `keywords`,
//!    `files`, `tags`, optional `content_long`, optional explicit lifecycle
//!    targets (`closes_memory_id`, `supersedes_memory_id`), `session_id`.
//! 2. **Deterministic extraction**: regex file-path detection from
//!    `content`/`content_long` merged with caller-supplied `files`. Existing
//!    `entities::extract` handles symbols/error-codes.
//! 3. **LLM enrichment + link judgment** (single Ollama call, gated by the
//!    salience threshold and `Hifz.llm_evolve`): given the new memory and
//!    its top-K kNN neighbors, the model returns enriched metadata
//!    (`keywords`, `tags`, `context_summary`), proposed typed edges
//!    (`relation`, `reason`, `score`), and proposed neighbor rewrites.
//!    Falls back to deterministic-only when LLM is unavailable.
//! 4. **Embedding** via `Embedder` (fastembed).
//! 5. **Persist**: write the memory row with all enriched fields.
//! 6. **Link generation**: deterministic co-occurrence edges via
//!    `link::generate_links` PLUS LLM-proposed typed edges from step 3.
//! 7. **Provenance edges**: `generated_by` to the open run, `derived_from`
//!    to recalled memories.
//! 8. **Lifecycle edges**: explicit `closes` and `supersedes`.
//! 9. **Async neighbor evolution**: spawned after persist; applies the LLM's
//!    proposed `context_summary`/`tags` rewrites with append-only
//!    `evolution_history` audit entries. Hard-capped at K mutations.
//!
//! ## Cost control
//!
//! - Observations are NEVER LLM-enriched (volume too high).
//! - Single Ollama call per insert is the budget.
//! - Salience gate: memories with `content.len() < SALIENCE_MIN_CHARS` skip
//!   LLM enrichment entirely.
//! - Neighbor evolution runs as a background task — the new memory is
//!   durable before the rewrite resolves.

use std::collections::HashSet;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use surrealdb::Surreal;
use surrealdb::types::{RecordId, SurrealValue};

use crate::chunk;
use crate::db::Db;
use crate::embed::Embedder;
use crate::link;
use crate::models::Category;
use crate::ollama::OllamaClient;

/// Top-K embedding-nearest neighbors considered for LLM link judgment +
/// bounded evolution. Mirrors A-MEM's default and caps the per-insert cost.
const ENRICH_KNN_K: usize = 10;

/// Memories with `content` shorter than this skip LLM enrichment entirely.
/// They run the deterministic path only — LLM rationale on a one-line note
/// is not worth the round-trip.
const SALIENCE_MIN_CHARS: usize = 80;

/// Hard cap on neighbor mutations applied per insert. Matches A-MEM's bound.
const MAX_NEIGHBOR_MUTATIONS: usize = 10;

/// Regex for file-path-like tokens in free text. Greedy-but-safe: requires
/// a dot and a known extension so we don't grab arbitrary identifiers.
static FILE_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?xi)
        \b
        (?: [A-Za-z0-9_\-./]+ )           # path body (letters, digits, _, -, ., /)
        \.
        (?: rs|py|ts|tsx|js|jsx|go|md|toml|json|yaml|yml|sql|sh|html|css|svelte|mjs|cjs|c|cpp|h|hpp|java|rb )
        \b
        ",
    )
    .expect("file path regex")
});

// ---------------------------------------------------------------------------
// LLM payload shapes
// ---------------------------------------------------------------------------

/// Wire shape returned by the single combined enrich+link LLM call.
#[derive(Debug, Default, Deserialize)]
struct LlmEnrichOutput {
    #[serde(default)]
    enrich: Option<EnrichMetadata>,
    #[serde(default)]
    links: Vec<LlmLinkProposal>,
    #[serde(default)]
    neighbour_updates: Vec<LlmNeighborUpdate>,
}

#[derive(Debug, Default, Deserialize)]
struct EnrichMetadata {
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    context_summary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LlmLinkProposal {
    /// Stringified neighbor id ("memory:abc"). Resolved against the
    /// neighbor candidate set; foreign IDs are dropped.
    neighbor_id: String,
    /// Typed relation string. Validated against `EdgeRelation::from_str`;
    /// unknown values dropped.
    relation: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default = "default_score")]
    score: f64,
}

#[derive(Debug, Deserialize)]
struct LlmNeighborUpdate {
    neighbor_id: String,
    #[serde(default)]
    new_context_summary: Option<String>,
    #[serde(default)]
    new_tags: Option<Vec<String>>,
    #[serde(default)]
    reason: Option<String>,
}

fn default_score() -> f64 {
    0.6
}

// ---------------------------------------------------------------------------
// Title derivation
// ---------------------------------------------------------------------------

/// Strip leading markdown heading / blockquote / bullet / ordered-list markers
/// from a line so a derived headline reads cleanly (`## Foo` → `Foo`,
/// `1. Bar` → `Bar`). Returns the slice after the markers (may be empty).
fn strip_md_markers(line: &str) -> &str {
    let s = line.trim_start_matches(['#', '>', '-', '*', '+', ' ', '\t']);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i > 0 && i < bytes.len() && (bytes[i] == b'.' || bytes[i] == b')') {
        s[i + 1..].trim_start()
    } else {
        s
    }
}

/// Derive a non-empty display headline from `content` when the caller omits
/// `title`. Mirrors the universal note-app pattern (first line = title; Apple
/// Notes / Joplin / basic-memory). `title` in hifz is descriptive only
/// (memories are keyed by RecordId), so a derived headline is fully adequate.
///
/// Rules: first non-blank line, stripped of leading markdown markers; cut at
/// the first sentence terminator (`.!?` followed by whitespace/EOL) when that
/// shortens it; truncated to ≤80 chars on a UTF-8 char boundary with an
/// ellipsis. If `content` is entirely blank the title is `"<category>
/// <YYYY-MM-DD>"` so the stored title is *never* empty.
pub fn derive_title(content: &str, category: &str) -> String {
    const MAX: usize = 80;

    let Some(line) = content.lines().map(str::trim).find(|l| !l.is_empty()) else {
        let date = chrono::Utc::now().format("%Y-%m-%d");
        return format!("{category} {date}");
    };

    let stripped = strip_md_markers(line);
    let base = if stripped.is_empty() { line } else { stripped };

    // Sentence end = `.!?` followed by whitespace or end-of-line only, so
    // version strings ("v1.2.3") and decimals aren't split mid-token.
    let term = base.char_indices().find(|&(i, c)| {
        matches!(c, '.' | '!' | '?')
            && base[i + c.len_utf8()..]
                .chars()
                .next()
                .map(char::is_whitespace)
                .unwrap_or(true)
    });
    let sentence = match term {
        Some((i, c)) => &base[..i + c.len_utf8()],
        None => base,
    };
    let sentence = sentence.trim();

    if sentence.chars().count() <= MAX {
        return sentence.to_string();
    }
    let truncated: String = sentence.chars().take(MAX).collect();
    format!("{}…", truncated.trim_end())
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// One enriched memory insert.
///
/// `enable_llm` is the runtime gate: even when the dependency Ollama exists,
/// the caller can disable LLM enrichment per-call (e.g. for batch ingest).
/// When `false` or `ollama=None` or content fails the salience gate, only
/// the deterministic pipeline runs.
#[allow(clippy::too_many_arguments)]
pub async fn save_enriched(
    db: &Surreal<Db>,
    embedder: &Embedder,
    ollama: Option<&OllamaClient>,
    enable_llm: bool,
    project: &str,
    title: &str,
    content: &str,
    category: Category,
    mut keywords: Vec<String>,
    mut files: Vec<String>,
    mut tags: Vec<String>,
    content_long: Option<String>,
    closes_memory_id: Option<&str>,
    supersedes_memory_id: Option<&str>,
    session_id: Option<&str>,
) -> Result<String> {
    let now = chrono::Utc::now().to_rfc3339();

    // ---- Step 2: deterministic file-path extraction ----------------------
    extract_files_into(content, &mut files);
    if let Some(long) = content_long.as_deref() {
        extract_files_into(long, &mut files);
    }
    dedup_in_place(&mut files);
    dedup_in_place(&mut keywords);
    dedup_in_place(&mut tags);

    // ---- Step 3: LLM enrichment + link judgment (if salient + available) -
    let llm_enrich_eligible = enable_llm
        && ollama.is_some()
        && content.len() >= SALIENCE_MIN_CHARS
        && category != Category::Observation;

    // We need kNN candidates BEFORE the LLM call. Embed first against
    // caller-only fields; the LLM may augment, but cost control says one
    // call per insert. The slight quality loss from not re-embedding with
    // `context_summary` is accepted (the content body is the dominant signal).
    let provisional_embed =
        embedder.embed_single(&build_embed_text(title, content, &keywords, &files, None))?;

    let neighbors = if llm_enrich_eligible {
        fetch_knn_neighbors(db, project, &provisional_embed)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let llm_output = if llm_enrich_eligible {
        match call_llm_enrich(
            ollama.unwrap(),
            title,
            content,
            category,
            &keywords,
            &files,
            &neighbors,
        )
        .await
        {
            Ok(out) => out,
            Err(e) => {
                tracing::warn!("LLM enrich failed; falling back to deterministic: {e}");
                LlmEnrichOutput::default()
            }
        }
    } else {
        LlmEnrichOutput::default()
    };

    let mut context_summary: Option<String> = None;
    if let Some(meta) = llm_output.enrich {
        // Merge LLM-suggested keywords/tags onto caller-supplied; LLM extends,
        // never replaces, so caller intent stays authoritative.
        for k in meta.keywords {
            let lc = k.trim().to_lowercase();
            if !lc.is_empty() && !keywords.iter().any(|x| x.to_lowercase() == lc) {
                keywords.push(lc);
            }
        }
        for t in meta.tags {
            let lc = t.trim().to_lowercase();
            if !lc.is_empty() && !tags.iter().any(|x| x.to_lowercase() == lc) {
                tags.push(lc);
            }
        }
        context_summary = meta.context_summary.filter(|s| !s.trim().is_empty());
    }

    // Default-tag for non-Note categories so retrieval grouping has *something*
    // to work with even when the LLM is off.
    if tags.is_empty() && category != Category::Note {
        tags.push(category.as_str().to_string());
    }

    // ---- Step 4: re-embed if we got new fields ---------------------------
    let final_embedding = if context_summary.is_some() || keywords.len() > files.len()
    // cheap heuristic: if anything changed, re-embed
    {
        embedder.embed_single(&build_embed_text(
            title,
            content,
            &keywords,
            &files,
            context_summary.as_deref(),
        ))?
    } else {
        provisional_embed
    };

    // ---- Step 5: persist memory row --------------------------------------
    #[derive(Debug, SurrealValue)]
    struct Created {
        id: Option<RecordId>,
    }

    let mut response = db
        .query(
            "CREATE memory SET \
             project = $project, \
             category = $category, \
             title = $title, \
             content = $content, \
             content_long = $content_long, \
             keywords = $keywords, \
             files = $files, \
             tags = $tags, \
             context_summary = $context_summary, \
             evolution_history = [], \
             strength = 1.0, \
             retrieval_count = 0, \
             last_accessed_at = $now, \
             embedding = $embedding, \
             version = 1, \
             is_latest = true, \
             created_at = $now, \
             updated_at = $now \
             RETURN id",
        )
        .bind(("project", project.to_string()))
        .bind(("category", category.as_str().to_string()))
        .bind(("title", title.to_string()))
        .bind(("content", content.to_string()))
        .bind(("content_long", content_long.clone()))
        .bind(("keywords", keywords.clone()))
        .bind(("files", files.clone()))
        .bind(("tags", tags.clone()))
        .bind(("context_summary", context_summary.clone()))
        .bind(("embedding", final_embedding.clone()))
        .bind(("now", now.clone()))
        .await?;
    response = response.check()?;
    let created: Vec<Created> = response.take(0).unwrap_or_default();
    let Some(new_id) = created.into_iter().next().and_then(|c| c.id) else {
        anyhow::bail!("memory insert returned no id");
    };

    // ---- Step 6: link generation -----------------------------------------
    // Long-form artifact categories own intentional links (Plans link via
    // `supersedes` to prior Plans, etc.); skip the noisy kNN co-occurrence
    // pass for them.
    if !category.is_long_form()
        && let Err(e) =
            link::generate_links(db, &new_id, project, &final_embedding, &keywords, &files).await
    {
        tracing::warn!("co-occurrence link generation failed for {new_id:?}: {e}");
    }

    // Long-form artifacts: split content_long into chunks for retrieval.
    // Each chunk gets its own embedding + a `part_of` edge to this memory.
    if let Some(long) = content_long.as_deref() {
        let chunks = chunk::split(long);
        if !chunks.is_empty() {
            match chunk::persist_chunks(db, embedder, &new_id, project, &chunks).await {
                Ok(n) => tracing::debug!("persisted {n} chunks for memory {new_id:?}"),
                Err(e) => tracing::warn!("chunk persist failed for {new_id:?}: {e}"),
            }
        }
    }

    // Shared-entity links between memories. The `via='entity'` tag is a plain
    // string label — no dedicated `entity` table is queried; similarity is
    // computed from shared keywords + files on the memory rows themselves.
    if let Err(e) = link_by_shared_entities(db, &new_id, project, &keywords, &files).await {
        tracing::warn!("entity-link pass failed for {new_id:?}: {e}");
    }

    // LLM-proposed typed edges (synchronous — they're the immediate value).
    apply_llm_links(db, &new_id, &llm_output.links, &neighbors).await;

    // ---- Step 6.5: code cross-linking (M3+) ------------------------------
    // Auto-extract `path:line[-line]` and qualified-symbol patterns from the
    // memory text and create `references` / `references_symbol` edges to
    // already-indexed code chunks/symbols. Failure is logged + swallowed so
    // a code-link bug never blocks memory persistence.
    #[cfg(feature = "code")]
    {
        let long_text = content_long.as_deref().unwrap_or("");
        let texts: [&str; 3] = [title, content, long_text];
        if let Err(e) = crate::code::link::auto_link_memory(db, &new_id, project, &texts).await {
            tracing::warn!("code auto-link failed for {new_id:?}: {e}");
        }
    }

    // ---- Step 7: provenance edges ----------------------------------------
    if let Some(sid) = session_id
        && let Ok(Some(run_id)) = crate::run::find_open(db, sid).await
    {
        let _ = link::upsert_edge(
            db,
            &new_id,
            &run_id,
            "generated_by",
            "system",
            1.0,
            Some("memory created during this run"),
        )
        .await;
        if let Ok(recalled) = crate::run::get_recalled_ids(db, &run_id).await {
            for rid in &recalled {
                let _ = link::upsert_edge(
                    db,
                    &new_id,
                    rid,
                    "derived_from",
                    "system",
                    0.8,
                    Some("authored after recalling this memory"),
                )
                .await;
            }
        }
    }

    // ---- Step 8: lifecycle edges (explicit) ------------------------------
    if let Some(target_id_str) = closes_memory_id
        && let Some(target) = resolve_memory_id(db, target_id_str).await
    {
        let _ = link::upsert_edge(
            db,
            &new_id,
            &target,
            "closes",
            "system",
            1.0,
            Some("explicit `closes_memory_id` from caller"),
        )
        .await;
    }
    if let Some(target_id_str) = supersedes_memory_id
        && let Some(target) = resolve_memory_id(db, target_id_str).await
    {
        let _ = link::upsert_edge(
            db,
            &new_id,
            &target,
            "supersedes",
            "system",
            1.0,
            Some("explicit `supersedes_memory_id` from caller"),
        )
        .await;
        // Mark the old memory non-latest so retrieval skips it by default.
        let _ = db
            .query("UPDATE type::record($id) SET is_latest = false, updated_at = $now")
            .bind(("id", target.clone()))
            .bind(("now", now.clone()))
            .await;
    }

    // ---- Step 9: async bounded neighbor evolution ------------------------
    // Fire-and-forget; the new memory is durable. The rewrites are
    // append-only via `evolution_history` so concurrent reads see consistent
    // state (worst case: a stale `context_summary` for a few hundred ms).
    if !llm_output.neighbour_updates.is_empty() {
        let db = db.clone();
        let new_id_str = crate::rid_to_string(&new_id);
        let updates = llm_output.neighbour_updates;
        let neighbors_snapshot = neighbors.clone();
        tokio::spawn(async move {
            apply_neighbor_evolutions(&db, &new_id_str, &updates, &neighbors_snapshot).await;
        });
    }

    Ok(crate::rid_to_string(&new_id))
}

// ---------------------------------------------------------------------------
// Step 2 helpers
// ---------------------------------------------------------------------------

fn extract_files_into(text: &str, out: &mut Vec<String>) {
    for m in FILE_PATH_RE.find_iter(text) {
        let path = m
            .as_str()
            .trim_matches(|c: char| matches!(c, ',' | ';' | ')' | '('));
        if !path.is_empty() {
            out.push(path.to_string());
        }
    }
}

fn dedup_in_place(v: &mut Vec<String>) {
    let mut seen: HashSet<String> = HashSet::new();
    v.retain(|x| {
        let k = x.to_lowercase();
        if seen.contains(&k) {
            false
        } else {
            seen.insert(k);
            true
        }
    });
}

// ---------------------------------------------------------------------------
// Embedding text builder (extends the legacy `remember::build_embed_text`)
// ---------------------------------------------------------------------------

fn build_embed_text(
    title: &str,
    content: &str,
    keywords: &[String],
    files: &[String],
    context_summary: Option<&str>,
) -> String {
    let mut s = String::with_capacity(title.len() + content.len() + 256);
    s.push_str(title);
    s.push('\n');
    s.push_str(content);
    if let Some(ctx) = context_summary {
        s.push_str("\ncontext: ");
        s.push_str(ctx);
    }
    if !keywords.is_empty() {
        s.push_str("\nkeywords: ");
        s.push_str(&keywords.join(", "));
    }
    if !files.is_empty() {
        s.push_str("\nfiles: ");
        s.push_str(&files.join(", "));
    }
    s
}

// ---------------------------------------------------------------------------
// Step 3: kNN candidate fetch + LLM call
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, SurrealValue)]
struct NeighborRow {
    id: Option<RecordId>,
    title: Option<String>,
    content: Option<String>,
    keywords: Option<Vec<String>>,
    tags: Option<Vec<String>>,
    context_summary: Option<String>,
}

async fn fetch_knn_neighbors(
    db: &Surreal<Db>,
    project: &str,
    embedding: &[f32],
) -> Result<Vec<NeighborRow>> {
    let sql = format!(
        "SELECT id, title, content, keywords, tags, context_summary \
         FROM memory \
         WHERE is_latest = true \
           AND (project = $project OR project = 'global') \
           AND embedding <|{ENRICH_KNN_K},100|> $vec"
    );
    let mut resp = db
        .query(&sql)
        .bind(("project", project.to_string()))
        .bind(("vec", embedding.to_vec()))
        .await?;
    let rows: Vec<NeighborRow> = resp.take(0).unwrap_or_default();
    Ok(rows)
}

async fn call_llm_enrich(
    ollama: &OllamaClient,
    title: &str,
    content: &str,
    category: Category,
    keywords: &[String],
    files: &[String],
    neighbors: &[NeighborRow],
) -> Result<LlmEnrichOutput> {
    let prompt = build_enrich_prompt(title, content, category, keywords, files, neighbors);
    let raw = ollama.complete(SYSTEM_PROMPT, &prompt).await?;
    parse_enrich_json(&raw).context("parse LLM enrich JSON")
}

fn build_enrich_prompt(
    title: &str,
    content: &str,
    category: Category,
    keywords: &[String],
    files: &[String],
    neighbors: &[NeighborRow],
) -> String {
    let mut s = String::with_capacity(content.len() + 2048);
    s.push_str("NEW MEMORY:\n");
    s.push_str(&format!("  title: {title}\n"));
    s.push_str(&format!("  category: {}\n", category.as_str()));
    s.push_str(&format!("  content: {content}\n"));
    s.push_str(&format!("  caller keywords: {keywords:?}\n"));
    s.push_str(&format!("  caller files: {files:?}\n\n"));

    s.push_str("NEIGHBOURS (top-K embedding-nearest existing memories):\n");
    if neighbors.is_empty() {
        s.push_str("  (none — this is the first memory in this project)\n");
    } else {
        for n in neighbors {
            s.push_str(&format!(
                "- id: {:?}\n  title: {}\n  content: {}\n  keywords: {:?}\n  tags: {:?}\n  context_summary: {:?}\n\n",
                n.id.as_ref().map(|r| format!("{r:?}")).unwrap_or_default(),
                n.title.as_deref().unwrap_or(""),
                n.content.as_deref().unwrap_or(""),
                n.keywords.clone().unwrap_or_default(),
                n.tags.clone().unwrap_or_default(),
                n.context_summary.as_deref().unwrap_or(""),
            ));
        }
    }
    s.push_str(
        "\nRespond with strict JSON now. Only reference neighbour ids from the NEIGHBOURS list above.",
    );
    s
}

const SYSTEM_PROMPT: &str = r#"You are a memory curator for a coding agent's long-term knowledge base.
Given a NEW memory and its top-K embedding-nearest NEIGHBOURS, output strict
JSON (no prose, no code fences) with three sections:

{
  "enrich": {
    "keywords": ["..."],          // 3-8 salient technical terms (lowercase, no names/dates)
    "tags": ["..."],              // 2-5 coarse buckets (e.g. "auth", "migration", "performance")
    "context_summary": "..."      // 2-4 sentences placing this memory in broader work
  },
  "links": [
    {
      "neighbor_id": "memory:abc",
      "relation": "related|broader|narrower|same_as|elaborates|contradicts|supports|responds_to|supersedes|closes|tests",
      "reason": "one sentence justification",
      "score": 0.0..1.0
    }
  ],
  "neighbour_updates": [
    {
      "neighbor_id": "memory:xyz",
      "new_context_summary": "...",   // optional rewrite when the new memory makes the old context stale
      "new_tags": ["..."],            // optional additional tags
      "reason": "why this update"
    }
  ]
}

Guidelines:
- Only assert a typed `links` relation when you can articulate why in one sentence.
  If unsure, omit the link — the deterministic co-occurrence edges already exist.
- Pick the most specific honest relation. "related" is the weakest fallback.
- Never reference a neighbour id that did not appear in NEIGHBOURS.
- Cap neighbour_updates at 5 — only include genuinely stale neighbours.
- Keywords/tags are lowercase. Tags are coarse buckets, not specific terms."#;

fn parse_enrich_json(raw: &str) -> Result<LlmEnrichOutput> {
    // Models often wrap JSON in prose; extract the first {...} block.
    let (start, end) = match (raw.find('{'), raw.rfind('}')) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => return Err(anyhow::anyhow!("no JSON object in LLM output")),
    };
    let slice = &raw[start..=end];
    serde_json::from_str::<LlmEnrichOutput>(slice).context("parse enrich JSON")
}

// ---------------------------------------------------------------------------
// Step 6 helper — entity-shared linking (lifted from old remember.rs)
// ---------------------------------------------------------------------------

async fn link_by_shared_entities(
    db: &Surreal<Db>,
    self_id: &RecordId,
    project: &str,
    keywords: &[String],
    files: &[String],
) -> Result<()> {
    if keywords.is_empty() && files.is_empty() {
        return Ok(());
    }

    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
        keywords: Option<Vec<String>>,
        files: Option<Vec<String>>,
    }

    let mut resp = db
        .query(
            "SELECT id, keywords, files FROM memory \
             WHERE is_latest = true \
               AND id != $self \
               AND (project = $project OR project = 'global')",
        )
        .bind(("self", self_id.clone()))
        .bind(("project", project.to_string()))
        .await?;
    let rows: Vec<Row> = resp.take(0).unwrap_or_default();

    let self_set: HashSet<&str> = keywords
        .iter()
        .chain(files.iter())
        .map(String::as_str)
        .collect();

    for r in rows {
        let Some(other_id) = r.id else {
            continue;
        };
        let other_set: HashSet<&str> = r
            .keywords
            .iter()
            .flatten()
            .chain(r.files.iter().flatten())
            .map(String::as_str)
            .collect();
        let shared = self_set.intersection(&other_set).count();
        if shared == 0 {
            continue;
        }
        let total = self_set.union(&other_set).count().max(1);
        let score = shared as f64 / total as f64;
        let reason = format!("shares {shared} of {total} entities (kw+files)");
        link::upsert_edge(
            db,
            self_id,
            &other_id,
            "mentions",
            "entity",
            score,
            Some(&reason),
        )
        .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 6b — apply LLM-proposed typed edges
// ---------------------------------------------------------------------------

async fn apply_llm_links(
    db: &Surreal<Db>,
    new_id: &RecordId,
    proposals: &[LlmLinkProposal],
    neighbors: &[NeighborRow],
) {
    use crate::models::EdgeRelation;

    // Build a set of valid neighbor id strings so we drop any LLM-hallucinated ids.
    let valid: HashSet<String> = neighbors
        .iter()
        .filter_map(|n| n.id.as_ref().map(|r| format!("{r:?}")))
        .collect();

    for p in proposals {
        if !valid.contains(&p.neighbor_id) {
            tracing::debug!(
                "LLM proposed link to unknown neighbor id {} — dropping",
                p.neighbor_id
            );
            continue;
        }
        // Only accept relations that map to a known typed variant; reject `Other`
        // proposals so the LLM can't smuggle in a fake relation.
        let rel = EdgeRelation::from_str(&p.relation);
        if matches!(rel, EdgeRelation::Other) {
            tracing::debug!("LLM proposed unknown relation {:?} — dropping", p.relation);
            continue;
        }
        let Some(neighbor_rid) = resolve_memory_id(db, &p.neighbor_id).await else {
            continue;
        };
        let score = p.score.clamp(0.0, 1.0);
        let reason = p
            .reason
            .clone()
            .unwrap_or_else(|| format!("LLM-proposed {} (no rationale)", p.relation));
        let _ = link::upsert_edge(
            db,
            new_id,
            &neighbor_rid,
            rel.as_str(),
            "llm",
            score,
            Some(&reason),
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Step 9 — bounded async neighbor evolution
// ---------------------------------------------------------------------------

async fn apply_neighbor_evolutions(
    db: &Surreal<Db>,
    triggered_by_id: &str,
    updates: &[LlmNeighborUpdate],
    neighbors: &[NeighborRow],
) {
    let valid_with_existing: std::collections::HashMap<String, NeighborRow> = neighbors
        .iter()
        .filter_map(|n| n.id.as_ref().map(|r| (format!("{r:?}"), n.clone())))
        .collect();

    let mut applied = 0usize;
    for upd in updates {
        if applied >= MAX_NEIGHBOR_MUTATIONS {
            tracing::info!("neighbor evolution capped at {MAX_NEIGHBOR_MUTATIONS}");
            break;
        }
        let Some(prior) = valid_with_existing.get(&upd.neighbor_id) else {
            continue;
        };
        let Some(rid) = resolve_memory_id(db, &upd.neighbor_id).await else {
            continue;
        };
        let now = chrono::Utc::now().to_rfc3339();
        let reason = upd
            .reason
            .clone()
            .unwrap_or_else(|| "LLM neighbor refinement".to_string());

        // Apply rewrites field-by-field, building one evolution_history entry
        // per touched field with the prior value preserved.
        if let Some(new_ctx) = upd
            .new_context_summary
            .clone()
            .filter(|s| !s.trim().is_empty())
        {
            let entry = serde_json::json!({
                "timestamp": now,
                "field": "context_summary",
                "previous": prior.context_summary.clone(),
                "reason": reason,
                "triggered_by": triggered_by_id,
            });
            let res = db
                .query(
                    "UPDATE type::record($id) SET context_summary = $ctx, \
                     evolution_history = array::concat(evolution_history ?? [], [$entry]), \
                     updated_at = $now",
                )
                .bind(("id", rid.clone()))
                .bind(("ctx", new_ctx))
                .bind(("entry", entry))
                .bind(("now", now.clone()))
                .await;
            if let Err(e) = res {
                tracing::warn!("neighbor context_summary rewrite failed for {rid:?}: {e}");
                continue;
            }
            applied += 1;
        }

        if let Some(new_tags) = upd.new_tags.clone() {
            let entry = serde_json::json!({
                "timestamp": now,
                "field": "tags",
                "previous": prior.tags.clone(),
                "reason": reason,
                "triggered_by": triggered_by_id,
            });
            let res = db
                .query(
                    "UPDATE type::record($id) SET \
                     tags = array::distinct(array::concat(tags ?? [], $tags)), \
                     evolution_history = array::concat(evolution_history ?? [], [$entry]), \
                     updated_at = $now",
                )
                .bind(("id", rid.clone()))
                .bind(("tags", new_tags))
                .bind(("entry", entry))
                .bind(("now", now.clone()))
                .await;
            if let Err(e) = res {
                tracing::warn!("neighbor tags rewrite failed for {rid:?}: {e}");
                continue;
            }
            applied += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Shared helper — resolve "memory:abc" / "memory:⟨ulid⟩" string to RecordId
// ---------------------------------------------------------------------------

async fn resolve_memory_id(db: &Surreal<Db>, id_str: &str) -> Option<RecordId> {
    let normalized = if id_str.starts_with("memory:") {
        id_str.to_string()
    } else {
        format!("memory:{id_str}")
    };
    #[derive(Debug, SurrealValue)]
    struct Row {
        id: Option<RecordId>,
    }
    let mut resp = db
        .query("SELECT id FROM type::record($id)")
        .bind(("id", normalized))
        .await
        .ok()?;
    let rows: Vec<Row> = resp.take(0).ok()?;
    rows.into_iter().next().and_then(|r| r.id)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rust_paths_from_text() {
        let text = "see the bug in src/auth.rs and tests/auth_test.rs";
        let mut out = Vec::new();
        extract_files_into(text, &mut out);
        assert!(out.iter().any(|p| p == "src/auth.rs"));
        assert!(out.iter().any(|p| p == "tests/auth_test.rs"));
    }

    #[test]
    fn extracts_typescript_and_markdown_paths() {
        let text = "edit website/src/lib/api.ts; doc in docs/ontology.md";
        let mut out = Vec::new();
        extract_files_into(text, &mut out);
        assert!(out.iter().any(|p| p == "website/src/lib/api.ts"));
        assert!(out.iter().any(|p| p == "docs/ontology.md"));
    }

    #[test]
    fn dedup_keeps_first_case_insensitive() {
        let mut v = vec![
            "Auth".to_string(),
            "auth".to_string(),
            "JWT".to_string(),
            "jwt".to_string(),
        ];
        dedup_in_place(&mut v);
        assert_eq!(v, vec!["Auth".to_string(), "JWT".to_string()]);
    }

    #[test]
    fn parse_enrich_handles_prose_wrapping() {
        let raw = r#"Sure, here you go:
        {"enrich":{"keywords":["jwt"],"tags":["auth"],"context_summary":"x"},"links":[],"neighbour_updates":[]}
        That's my response."#;
        let out = parse_enrich_json(raw).unwrap();
        assert_eq!(out.enrich.unwrap().keywords, vec!["jwt".to_string()]);
    }

    #[test]
    fn parse_enrich_rejects_no_object() {
        let raw = "I refuse to comply";
        assert!(parse_enrich_json(raw).is_err());
    }

    #[test]
    fn derive_title_uses_first_nonblank_line() {
        assert_eq!(
            derive_title("\n\n  Fixed the auth race  \nmore detail here", "fix"),
            "Fixed the auth race"
        );
    }

    #[test]
    fn derive_title_strips_markdown_markers() {
        assert_eq!(
            derive_title("## Plan: rework cache", "plan"),
            "Plan: rework cache"
        );
        assert_eq!(derive_title("- a bullet point", "note"), "a bullet point");
        assert_eq!(
            derive_title("1. first step of the plan", "plan"),
            "first step of the plan"
        );
        assert_eq!(derive_title("> quoted insight", "note"), "quoted insight");
    }

    #[test]
    fn derive_title_cuts_at_sentence_but_not_versions() {
        assert_eq!(
            derive_title("The bug was a deadlock. It only repro'd under load.", "bug"),
            "The bug was a deadlock."
        );
        // version/decimal must NOT split (terminator not followed by space)
        assert_eq!(
            derive_title("v1.2.3 released to prod", "note"),
            "v1.2.3 released to prod"
        );
    }

    #[test]
    fn derive_title_truncates_on_char_boundary_with_ellipsis() {
        let long = "x".repeat(200);
        let t = derive_title(&long, "note");
        assert_eq!(t.chars().count(), 81); // 80 + ellipsis
        assert!(t.ends_with('…'));
        // multibyte content must not panic and must stay on a char boundary
        let multi = "é".repeat(200);
        let mt = derive_title(&multi, "note");
        assert!(mt.ends_with('…'));
        assert_eq!(mt.chars().filter(|&c| c == 'é').count(), 80);
    }

    #[test]
    fn derive_title_blank_content_falls_back_never_empty() {
        let t = derive_title("   \n\t  \n", "lesson");
        assert!(t.starts_with("lesson "), "got {t:?}");
        assert!(!t.trim().is_empty());
    }
}
