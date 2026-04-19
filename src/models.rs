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
    pub concepts: Vec<String>,
    pub files: Vec<String>,
    pub importance: i64,
    pub confidence: Option<f64>,
    pub embedding: Option<Vec<f32>>,
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

// --- Memory (consolidated long-term) ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct Hifz {
    pub id: Option<surrealdb::types::RecordId>,
    pub project: String,
    pub mem_type: String,
    pub title: String,
    pub content: String,
    pub concepts: Vec<String>,
    pub files: Vec<String>,
    pub keywords: Vec<String>,
    pub tags: Vec<String>,
    pub context: Option<String>,
    pub session_ids: Vec<surrealdb::types::RecordId>,
    pub strength: f64,
    pub access_count: i64,
    pub last_accessed_at: String,
    pub embedding: Option<Vec<f32>>,
    pub version: i64,
    pub parent_id: Option<surrealdb::types::RecordId>,
    pub supersedes: Option<Vec<surrealdb::types::RecordId>>,
    pub is_latest: bool,
    pub forget_after: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

// --- Entity (Phase 4) ---

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

// --- Episode (Phase 4) ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct Episode {
    pub id: Option<surrealdb::types::RecordId>,
    pub session_id: surrealdb::types::RecordId,
    pub project: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub prompt: String,
    pub outcome: String,
    pub observation_ids: Vec<surrealdb::types::RecordId>,
    pub lesson: Option<String>,
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

// --- Session Summary ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct SessionSummary {
    pub id: Option<surrealdb::types::RecordId>,
    pub session_id: Option<surrealdb::types::RecordId>,
    pub project: String,
    pub created_at: String,
    pub title: String,
    pub narrative: String,
    pub key_decisions: Vec<String>,
    pub files_modified: Vec<String>,
    pub concepts: Vec<String>,
    pub observation_count: i64,
}

// --- Consolidation Tiers ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct SemanticHifz {
    pub id: Option<surrealdb::types::RecordId>,
    pub fact: String,
    pub confidence: f64,
    pub source_sessions: Vec<surrealdb::types::RecordId>,
    pub access_count: i64,
    pub strength: f64,
    pub last_accessed_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct ProceduralHifz {
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
    pub top_concepts: Vec<ConceptFreq>,
    pub top_files: Vec<FileFreq>,
    pub session_count: i64,
    pub total_observations: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptFreq {
    pub concept: String,
    pub frequency: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileFreq {
    pub file: String,
    pub frequency: i64,
}

// --- Hook payload from Claude Code ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookPayload {
    #[serde(rename = "hookType")]
    pub hook_type: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub project: String,
    pub cwd: String,
    pub timestamp: String,
    pub data: serde_json::Value,
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
    "other",
];

pub const HOOK_TYPES: &[&str] = &[
    "session_start",
    "prompt_submit",
    "pre_tool_use",
    "post_tool_use",
    "post_tool_failure",
    "pre_compact",
    "post_compact",
    "subagent_start",
    "subagent_stop",
    "notification",
    "task_completed",
    "stop",
    "session_end",
];
