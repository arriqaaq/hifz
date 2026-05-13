use serde::{Deserialize, Serialize};
use surrealdb::types::SurrealValue;

// --- Session ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct Session {
    pub id: Option<surrealdb::types::RecordId>,
    pub project: String,
    pub cwd: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub status: String,
    pub observation_count: i64,
    pub name: Option<String>,
    pub model: Option<String>,
    pub tags: Option<Vec<String>>,
}

// --- Observation (compressed) ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct Observation {
    pub id: Option<surrealdb::types::RecordId>,
    pub session_id: Option<surrealdb::types::RecordId>,
    pub timestamp: String,
    pub obs_type: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub facts: Vec<String>,
    pub facts_text: Option<String>,
    pub narrative: String,
    pub keywords: Vec<String>,
    pub files: Vec<String>,
    pub importance: i64,
    pub confidence: Option<f64>,
    pub embedding: Option<Vec<f32>>,
    pub metadata: Option<serde_json::Value>,
}

// --- Raw observation from hooks (before compression) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawObservation {
    pub hook_type: String,
    pub session_id: String,
    pub project: String,
    pub cwd: String,
    pub timestamp: String,
    pub data: serde_json::Value,
}

// --- Memory (long-term, A-mem aligned) ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct Memory {
    pub id: Option<surrealdb::types::RecordId>,
    pub project: String,
    /// Typed category (see `Category` enum). Stored as snake_case string for
    /// SurrealDB schemafullness; convert via `Category::from_str` at boundaries.
    pub category: String,
    pub title: String,
    /// Short retrieval-friendly form (≤ ~2KB). Always populated and embedded.
    pub content: String,
    /// Caller-supplied domain terms. NOT extracted from content (Phase 2 LLM
    /// enrichment may add to this list when available).
    pub keywords: Vec<String>,
    /// Caller-supplied file paths the memory references. Phase 2 deterministic
    /// extraction adds paths regex-detected in `content` / `content_long`.
    pub files: Vec<String>,
    /// LLM-generated coarse buckets (e.g. `auth`, `migration`). Distinct from
    /// `keywords` which are domain terms. Empty when no LLM enrichment ran.
    pub tags: Vec<String>,
    /// Legacy free-form context line. Deprecated in favor of `context_summary`;
    /// retained for backward-compat with rows authored before Phase 2.
    pub context: Option<String>,
    /// LLM-generated one-paragraph contextual placement (A-MEM's `Xᵢ`).
    /// Null when no LLM enrichment ran.
    pub context_summary: Option<String>,
    /// Append-only audit log of LLM rewrites applied during bounded evolution.
    pub evolution_history: Vec<EvolutionEntry>,
    /// Long-form markdown body for artifact categories (Plan, Design,
    /// CodeReview, ShipReport, ContextSlice). When set, `content` carries a
    /// short summary derived from this. Phase 4 chunks this for retrieval.
    pub content_long: Option<String>,
    pub strength: f64,
    pub retrieval_count: i64,
    pub last_accessed_at: String,
    pub embedding: Option<Vec<f32>>,
    pub version: i64,
    pub parent_id: Option<surrealdb::types::RecordId>,
    pub supersedes: Option<Vec<surrealdb::types::RecordId>>,
    pub is_latest: bool,
    pub pinned: bool,
    pub forget_after: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Single entry in a memory's evolution history. Captures one LLM rewrite
/// pass — what fields changed, the model's rationale, when.
#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct EvolutionEntry {
    pub timestamp: String,
    /// Field name that was rewritten (e.g. "context_summary", "tags").
    pub field: String,
    /// Prior value (stringified) so the rewrite is auditable.
    pub previous: Option<String>,
    /// LLM's one-line justification for the rewrite.
    pub reason: String,
    /// ID of the memory whose insert triggered this evolution, if any.
    pub triggered_by: Option<String>,
}

// --- Entity ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct Entity {
    pub id: Option<surrealdb::types::RecordId>,
    pub kind: String, // file | symbol | concept | error
    pub name: String,
    pub project: String,
    pub first_seen: String,
    pub last_seen: String,
    pub count: i64,
}

// --- Run ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct Run {
    pub id: Option<surrealdb::types::RecordId>,
    pub session_id: surrealdb::types::RecordId,
    pub project: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub prompt: String,
    pub prompts: Option<Vec<String>>,
    pub outcome: String,
    pub observation_ids: Vec<surrealdb::types::RecordId>,
    pub lesson: Option<String>,
    pub recalled_ids: Vec<surrealdb::types::RecordId>,
}

// --- Core memory (per-project always-on block) ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct CoreMemory {
    pub id: Option<surrealdb::types::RecordId>,
    pub project: String,
    pub identity: Option<String>,
    pub goals: Vec<String>,
    pub invariants: Vec<String>,
    pub watchlist: Vec<String>,
    pub updated_at: String,
}

// --- Consolidation Tiers ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct SemanticMemory {
    pub id: Option<surrealdb::types::RecordId>,
    pub fact: String,
    pub confidence: f64,
    pub source_sessions: Vec<surrealdb::types::RecordId>,
    pub retrieval_count: i64,
    pub strength: f64,
    pub last_accessed_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct ProceduralMemory {
    pub id: Option<surrealdb::types::RecordId>,
    pub name: String,
    pub steps: Vec<String>,
    pub trigger_condition: String,
    pub frequency: i64,
    pub strength: f64,
    pub source_sessions: Vec<surrealdb::types::RecordId>,
    pub created_at: String,
    pub updated_at: String,
}

// --- Search Results ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct SearchResult {
    pub id: Option<surrealdb::types::RecordId>,
    pub session_id: Option<surrealdb::types::RecordId>,
    pub title: String,
    pub obs_type: String,
    pub narrative: String,
    pub timestamp: String,
    pub importance: i64,
    pub score: Option<f64>,
    /// Optional because rows from BM25/text-search paths don't carry the
    /// graph-expansion flag — only KNN-expanded rows set it.
    #[serde(default)]
    pub is_neighbor: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct RrfResult {
    pub id: Option<surrealdb::types::RecordId>,
    pub rrf_score: Option<f64>,
}

// --- Health ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub status: String,
    pub version: String,
    pub sessions: i64,
    pub observations: i64,
    pub memories: i64,
    pub uptime_seconds: u64,
}

// --- Project Profile ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectDigest {
    pub project: String,
    pub updated_at: String,
    pub top_keywords: Vec<KeywordFreq>,
    pub top_files: Vec<FileFreq>,
    pub session_count: i64,
    pub total_observations: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeywordFreq {
    pub keyword: String,
    pub frequency: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileFreq {
    pub file: String,
    pub frequency: i64,
}

// --- Generic event ledger (producer-agnostic) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventRequest {
    pub source: String,
    pub event_type: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub sequence: Option<i64>,
    pub timestamp: String,
    #[serde(default)]
    pub parent_event_id: Option<String>,
    pub payload_hash: String,
    #[serde(default)]
    pub payload: Option<serde_json::Value>,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

// --- Request types for library / REST API ---
//
// These mirror the on-the-wire JSON shape used by the REST handlers, exposed
// here so library users can construct them directly. `Default` is provided so
// callers can use `EventsListReq { source: Some("foo".into()), ..Default::default() }`.

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventsListReq {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub event_type: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionStartReq {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub project: String,
    pub cwd: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ObservationsReq {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub obs_type: Option<String>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub min_importance: Option<i64>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoriesReq {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    /// Phase 5: filter by `created_at >= since` (RFC3339 timestamp).
    /// Powers "decisions in the last 30 days" style queries.
    #[serde(default)]
    pub since: Option<String>,
    /// Phase 5: open-only filter — drop memories that have an incoming
    /// `closes` edge. Useful for `?category=bug&open=true`.
    #[serde(default)]
    pub open: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchReq {
    pub query: String,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default, rename = "sessionId", alias = "session_id")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RememberReq {
    pub title: String,
    pub content: String,
    /// Typed category string (`Category::as_str()`). Defaults to `Note`.
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub keywords: Option<Vec<String>>,
    #[serde(default)]
    pub files: Option<Vec<String>>,
    /// Caller-supplied tags (LLM enrichment may add more when available).
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Long-form markdown body. When set, `content` is treated as a summary
    /// and `content_long` carries the full artifact.
    #[serde(default)]
    pub content_long: Option<String>,
    /// Explicit lifecycle: this memory closes/resolves the named one. Writes
    /// a `closes` edge.
    #[serde(default)]
    pub closes_memory_id: Option<String>,
    /// Explicit lifecycle: this memory replaces the named one. Writes a
    /// `supersedes` edge AND sets the old memory's `is_latest=false`.
    #[serde(default)]
    pub supersedes_memory_id: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default, rename = "sessionId", alias = "session_id")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunsReq {
    pub query: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CoreEditReq {
    pub project: String,
    pub field: String,
    pub op: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TraceReq {
    pub id: String,
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(default)]
    pub relations: Option<Vec<String>>,
    #[serde(default)]
    pub max_hops: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanActivateReq {
    pub project: String,
    #[serde(default)]
    pub plan_id: Option<String>,
    #[serde(default, rename = "sessionId", alias = "session_id")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlansListReq {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommitsReq {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub sha: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExportReq {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub obs_type: Option<String>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub min_importance: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TimelineReq {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextReq {
    pub project: String,
    #[serde(default)]
    pub token_budget: Option<usize>,
    #[serde(default)]
    pub query: Option<String>,
}

// --- Code-indexing DTOs (M2+) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeIndexReq {
    pub project: String,
    pub root: String,
    #[serde(default)]
    pub follow_symlinks: Option<bool>,
    #[serde(default)]
    pub max_file_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSearchReq {
    pub query: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    /// Substring match against `code_chunk.path`. Glob-style wildcards are
    /// not supported in v1 — use plain substring.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub group_by_file: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeLinkReq {
    pub memory_id: String,
    #[serde(default)]
    pub project: Option<String>,
    pub file: String,
    pub start_line: usize,
    #[serde(default)]
    pub end_line: Option<usize>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeLinkSymReq {
    pub memory_id: String,
    #[serde(default)]
    pub project: Option<String>,
    pub name: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeGcReq {
    pub project: String,
    pub root: String,
    #[serde(default)]
    pub dry_run: Option<bool>,
    #[serde(default)]
    pub force_decay: Option<bool>,
}

// --- Hook payload from agent harness ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookPayload {
    #[serde(rename = "hookType")]
    pub hook_type: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub project: String,
    pub cwd: String,
    pub timestamp: String,
    #[serde(default)]
    pub source: Option<String>,
    /// Adapter may pre-set obs_type (e.g. "commit_made"). Overrides inference.
    #[serde(rename = "obs_type", default)]
    pub obs_type: Option<String>,
    pub data: serde_json::Value,
}

// --- Canonical event vocabulary ---

#[derive(Debug, Clone, PartialEq)]
pub enum HifzEvent {
    SessionStart,
    PromptSubmit,
    ToolStart,
    ToolComplete,
    ToolFailed,
    PreCompact,
    PostCompact,
    SubagentStart,
    SubagentStop,
    Notification,
    TaskCompleted,
    SessionStop,
    SessionEnd,
    Unknown(String),
}

impl From<&str> for HifzEvent {
    fn from(s: &str) -> Self {
        match s {
            "UserPromptSubmit" | "prompt_submit" => Self::PromptSubmit,
            "PreToolUse" | "pre_tool_use" | "tool_start" => Self::ToolStart,
            "PostToolUse" | "post_tool_use" | "tool_complete" => Self::ToolComplete,
            "PostToolUseFailure" | "PostToolFailure" | "post_tool_failure" | "tool_failed" => {
                Self::ToolFailed
            }
            "SessionStart" | "session_start" => Self::SessionStart,
            "Stop" | "stop" | "session_stop" => Self::SessionStop,
            "TaskCompleted" | "task_completed" => Self::TaskCompleted,
            "SessionEnd" | "session_end" => Self::SessionEnd,
            "PreCompact" | "pre_compact" => Self::PreCompact,
            "PostCompact" | "post_compact" => Self::PostCompact,
            "SubagentStart" | "subagent_start" => Self::SubagentStart,
            "SubagentStop" | "subagent_stop" => Self::SubagentStop,
            "Notification" | "notification" => Self::Notification,
            other => Self::Unknown(other.to_string()),
        }
    }
}

// --- Observation types ---

pub const OBS_TYPES: &[&str] = &[
    "file_read",
    "file_write",
    "file_edit",
    "command_run",
    "search",
    "web_fetch",
    "conversation",
    "error",
    "decision",
    "discovery",
    "subagent",
    "notification",
    "task",
    "compaction_summary",
    "commit_made",
    "plan_activated",
    "session_summary",
    "other",
];

// --- Knowledge Graph Edge Types ---
//
// Canonical typed relation vocabulary, grouped by the source of the edge.
// Grouping makes the deterministic vs. LLM split visible in the type itself.
// See `docs/ontology.md` (Phase 2) and `link::is_allowed_relation` for
// per-relation type-pair constraints.

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeRelation {
    // -- Co-occurrence (deterministic, write-time, signal-typed) --
    /// Two memories share file paths.
    CoOccursFiles,
    /// Two memories share caller-supplied keywords.
    CoOccursKeywords,
    /// Two memories' embeddings are within cosine threshold.
    CoOccursEmbedding,
    /// Memory references a shared entity (file/symbol/concept/error).
    Mentions,

    // -- Provenance (deterministic, system-set; PROV-O grounded) --
    /// Memory was generated during a run.
    GeneratedBy,
    /// Memory was used as input to a run (surfaced via search).
    InformedBy,
    /// Memory was derived from one or more recalled memories.
    DerivedFrom,
    /// Memory authored by an agent / model.
    AttributedTo,
    /// Structural containment (chunk in memory, run in session).
    PartOf,
    /// Temporal sequence within a session (run after run).
    Follows,

    // -- Conceptual (LLM-set or strong heuristic; SKOS grounded) --
    /// A is more general than B.
    Broader,
    /// A is more specific than B.
    Narrower,
    /// Symmetric conceptual link (LLM can't decompose into broader/narrower).
    Related,
    /// Explicit dedup pointer (SKOS exactMatch semantics; does NOT merge identities).
    SameAs,

    // -- Argumentative (LLM-set; IBIS grounded) --
    /// A supports B's claim.
    Supports,
    /// A conflicts with B.
    Contradicts,
    /// A adds detail to B.
    Elaborates,
    /// A is a response to a question/issue raised in B.
    RespondsTo,

    // -- Lifecycle (deterministic when explicit, LLM otherwise) --
    /// A replaces B (also reflected on B as `is_latest=false`).
    Supersedes,
    /// A closes/resolves B (e.g., a fix memory closes a bug memory).
    Closes,

    // -- Code-domain (deterministic, write-time) --
    /// An observation touched a file referenced by a memory.
    TouchesFile,
    /// A `commit_made` observation is the commit for a task/bug memory.
    CommitsFor,
    /// A memory describes tests for code another memory describes.
    Tests,
    /// A memory references a precise location (line range) in source code,
    /// stored as a `code_chunk` row. Edge `metadata` records the original
    /// `(ref_path, ref_start, ref_end)` so re-anchoring on file change can
    /// remap the edge to a new chunk that overlaps the same line range.
    References,
    /// A memory references a named symbol (function/struct/class/...), stored
    /// as a `code_symbol` row. Survives chunk re-splitting and reformatting.
    ReferencesSymbol,

    /// Catch-all for forward/backward compat. Validation accepts.
    #[serde(other)]
    Other,
}

impl EdgeRelation {
    pub fn as_str(&self) -> &str {
        match self {
            Self::CoOccursFiles => "co_occurs_files",
            Self::CoOccursKeywords => "co_occurs_keywords",
            Self::CoOccursEmbedding => "co_occurs_embedding",
            Self::Mentions => "mentions",
            Self::GeneratedBy => "generated_by",
            Self::InformedBy => "informed_by",
            Self::DerivedFrom => "derived_from",
            Self::AttributedTo => "attributed_to",
            Self::PartOf => "part_of",
            Self::Follows => "follows",
            Self::Broader => "broader",
            Self::Narrower => "narrower",
            Self::Related => "related",
            Self::SameAs => "same_as",
            Self::Supports => "supports",
            Self::Contradicts => "contradicts",
            Self::Elaborates => "elaborates",
            Self::RespondsTo => "responds_to",
            Self::Supersedes => "supersedes",
            Self::Closes => "closes",
            Self::TouchesFile => "touches_file",
            Self::CommitsFor => "commits_for",
            Self::Tests => "tests",
            Self::References => "references",
            Self::ReferencesSymbol => "references_symbol",
            Self::Other => "other",
        }
    }

    /// Parse a relation string into the typed enum. Unknown strings map to `Other`.
    pub fn from_str(s: &str) -> Self {
        match s {
            "co_occurs_files" => Self::CoOccursFiles,
            "co_occurs_keywords" => Self::CoOccursKeywords,
            "co_occurs_embedding" => Self::CoOccursEmbedding,
            "mentions" => Self::Mentions,
            "generated_by" => Self::GeneratedBy,
            "informed_by" => Self::InformedBy,
            "derived_from" => Self::DerivedFrom,
            "attributed_to" => Self::AttributedTo,
            "part_of" => Self::PartOf,
            "follows" => Self::Follows,
            "broader" => Self::Broader,
            "narrower" => Self::Narrower,
            "related" => Self::Related,
            "same_as" => Self::SameAs,
            "supports" => Self::Supports,
            "contradicts" => Self::Contradicts,
            "elaborates" => Self::Elaborates,
            "responds_to" => Self::RespondsTo,
            "supersedes" => Self::Supersedes,
            "closes" => Self::Closes,
            "touches_file" => Self::TouchesFile,
            "commits_for" => Self::CommitsFor,
            "tests" => Self::Tests,
            "references" => Self::References,
            "references_symbol" => Self::ReferencesSymbol,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeVia {
    System,
    Embedding,
    Keyword,
    File,
    Entity,
    Llm,
    Cluster,
    #[serde(other)]
    Other,
}

impl EdgeVia {
    pub fn as_str(&self) -> &str {
        match self {
            Self::System => "system",
            Self::Embedding => "embedding",
            Self::Keyword => "keyword",
            Self::File => "file",
            Self::Entity => "entity",
            Self::Llm => "llm",
            Self::Cluster => "cluster",
            Self::Other => "other",
        }
    }
}

// --- Memory categories (typed) ---
//
// Stored on `Memory.category` as snake_case strings. Use `Category::from_str`
// at API boundaries; use `Category::is_long_form` to decide whether the row
// owns a `content_long` body and gets chunked retrieval (Phase 4).

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Ephemeral hook record. Rare in the memory table — most observations
    /// stay in the `observation` table. Reserved for the few cases where an
    /// observation gets promoted.
    Observation,
    /// A generalized learning extracted across one or more sessions.
    Lesson,
    /// An architectural or scoping choice with stated reasoning.
    Decision,
    /// An open problem; closed by a `Fix` memory linked via `closes`.
    Bug,
    /// A fix for a `Bug` memory; should carry a `closes` edge.
    Fix,
    /// A non-obvious behavior that surprised the agent.
    Gotcha,
    /// A project rule or pattern (naming, structure, etc.).
    Convention,
    /// A recurring failure pattern surfaced during review or kata.
    FailurePattern,
    /// Long-form: a plan document.
    Plan,
    /// Long-form: a design document.
    Design,
    /// Long-form: a code-review report.
    CodeReview,
    /// Long-form: a ship/release verdict.
    ShipReport,
    /// Long-form: a project-context slice (architecture, conventions, etc.).
    ContextSlice,
    /// Catch-all for ad-hoc memories.
    Note,
}

impl Category {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Lesson => "lesson",
            Self::Decision => "decision",
            Self::Bug => "bug",
            Self::Fix => "fix",
            Self::Gotcha => "gotcha",
            Self::Convention => "convention",
            Self::FailurePattern => "failure_pattern",
            Self::Plan => "plan",
            Self::Design => "design",
            Self::CodeReview => "code_review",
            Self::ShipReport => "ship_report",
            Self::ContextSlice => "context_slice",
            Self::Note => "note",
        }
    }

    /// Parse a string into the typed category. Unknown values map to `Note`
    /// (lossy by design — agents should not crash on a typo'd category).
    pub fn from_str(s: &str) -> Self {
        match s {
            "observation" => Self::Observation,
            "lesson" => Self::Lesson,
            "decision" => Self::Decision,
            "bug" => Self::Bug,
            "fix" => Self::Fix,
            "gotcha" => Self::Gotcha,
            "convention" => Self::Convention,
            "failure_pattern" => Self::FailurePattern,
            "plan" => Self::Plan,
            "design" => Self::Design,
            "code_review" => Self::CodeReview,
            "ship_report" => Self::ShipReport,
            "context_slice" => Self::ContextSlice,
            _ => Self::Note,
        }
    }

    /// True for categories whose `content_long` body is the canonical artifact.
    /// Long-form rows skip the deterministic co-occurrence link pass during
    /// insert and earn chunked retrieval (Phase 4).
    pub fn is_long_form(&self) -> bool {
        matches!(
            self,
            Self::Plan | Self::Design | Self::CodeReview | Self::ShipReport | Self::ContextSlice
        )
    }
}

// --- Record kinds (for edge type-pair constraints) ---
//
// Mirrors the table names in `db::SCHEMA`. Used by `link::is_allowed_relation`
// to reject illegal `(from_kind, relation, to_kind)` triples.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordKind {
    Memory,
    Observation,
    Run,
    Session,
    Entity,
    SemanticMemory,
    ProceduralMemory,
    /// Long-form artifact chunk (parent of a `Memory` row).
    MemoryChunk,
    /// Indexed source file (root of the code dimension).
    CodeFile,
    /// One chunk of a `CodeFile`.
    CodeChunk,
    /// Named symbol (function/struct/etc) extracted from a `CodeFile`.
    CodeSymbol,
    Other,
}

impl RecordKind {
    /// Map a SurrealDB table name to the typed kind.
    pub fn from_table(table: &str) -> Self {
        match table {
            "memory" => Self::Memory,
            "observation" => Self::Observation,
            "run" => Self::Run,
            "session" => Self::Session,
            "entity" => Self::Entity,
            "semantic_memory" => Self::SemanticMemory,
            "procedural_memory" => Self::ProceduralMemory,
            "memory_chunk" => Self::MemoryChunk,
            "code_file" => Self::CodeFile,
            "code_chunk" => Self::CodeChunk,
            "code_symbol" => Self::CodeSymbol,
            _ => Self::Other,
        }
    }
}
