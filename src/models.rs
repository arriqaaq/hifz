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
    pub category: String,
    pub title: String,
    pub content: String,
    pub keywords: Vec<String>,
    pub files: Vec<String>,
    pub tags: Vec<String>,
    pub context: Option<String>,
    pub strength: f64,
    pub retrieval_count: i64,
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

// --- Commit (git commit tracking) ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct Commit {
    pub id: Option<surrealdb::types::RecordId>,
    pub sha: String,
    pub message: String,
    pub author: String,
    pub branch: String,
    pub project: String,
    pub files_changed: Vec<String>,
    pub insertions: Option<i64>,
    pub deletions: Option<i64>,
    pub is_amend: bool,
    pub session_id: Option<surrealdb::types::RecordId>,
    pub run_id: Option<surrealdb::types::RecordId>,
    pub plan_id: Option<surrealdb::types::RecordId>,
    pub timestamp: String,
    pub created_at: String,
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
    pub commit_id: Option<surrealdb::types::RecordId>,
    pub plan_id: Option<surrealdb::types::RecordId>,
}

// --- Plan ---

#[derive(Debug, Clone, Serialize, Deserialize, SurrealValue)]
pub struct Plan {
    pub id: Option<surrealdb::types::RecordId>,
    pub file_path: String,
    pub title: String,
    pub content: String,
    pub status: String,
    pub project: String,
    pub keywords: Vec<String>,
    pub files: Vec<String>,
    pub session_id: Option<surrealdb::types::RecordId>,
    pub commit_id: Option<surrealdb::types::RecordId>,
    pub created_at: String,
    pub completed_at: Option<String>,
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
    pub keywords: Vec<String>,
    pub observation_count: i64,
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
    #[serde(default)]
    pub is_neighbor: bool,
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
    GitCommit,
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
            "git_commit" => Self::GitCommit,
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
    "other",
];
