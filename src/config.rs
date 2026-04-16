use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// hifz configuration loaded from ~/.hifz/.env and environment.
#[derive(Debug, Clone)]
pub struct Config {
    pub db_path: String,
    pub rest_port: u16,
    pub viewer_port: u16,
    pub ollama_url: Option<String>,
    pub ollama_model: String,
    pub auto_compress: bool,
    pub consolidation_enabled: bool,
    pub token_budget: usize,
    pub max_obs_per_session: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            db_path: default_db_path(),
            rest_port: 3111,
            viewer_port: 3113,
            ollama_url: None,
            ollama_model: "qwen2.5:7b".to_string(),
            auto_compress: false,
            consolidation_enabled: true,
            token_budget: 2000,
            max_obs_per_session: 500,
        }
    }
}

fn default_db_path() -> String {
    dirs_data_path()
        .join("db_data")
        .to_string_lossy()
        .to_string()
}

fn dirs_data_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".hifz")
}

/// Load .env file from ~/.hifz/.env if it exists.
fn load_env_file() -> HashMap<String, String> {
    let path = dirs_data_path().join(".env");
    load_env_from_path(&path)
}

fn load_env_from_path(path: &Path) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vars,
    };
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(eq_idx) = trimmed.find('=') {
            let key = trimmed[..eq_idx].trim().to_string();
            let mut val = trimmed[eq_idx + 1..].trim().to_string();
            // Strip surrounding quotes
            if (val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\''))
            {
                val = val[1..val.len() - 1].to_string();
            }
            vars.insert(key, val);
        }
    }
    vars
}

/// Get an environment variable, checking ~/.hifz/.env first, then process env.
pub fn get_env_var(key: &str) -> Option<String> {
    let file_env = load_env_file();
    file_env
        .get(key)
        .cloned()
        .or_else(|| std::env::var(key).ok())
}

fn parse_bool(val: &str) -> bool {
    matches!(val.to_lowercase().as_str(), "true" | "1" | "yes")
}

fn parse_usize(val: &str, default: usize) -> usize {
    val.parse().unwrap_or(default)
}

/// Load config from environment + ~/.hifz/.env.
pub fn load_config() -> Config {
    let mut cfg = Config::default();
    let file_env = load_env_file();

    // Helper to get from file env or process env
    let get = |key: &str| -> Option<String> {
        file_env
            .get(key)
            .cloned()
            .or_else(|| std::env::var(key).ok())
    };

    if let Some(v) = get("HIFZ_PORT") {
        cfg.rest_port = v.parse().unwrap_or(3111);
    }
    cfg.viewer_port = cfg.rest_port + 2;
    cfg.ollama_url = get("OLLAMA_URL");
    if let Some(v) = get("OLLAMA_MODEL") {
        cfg.ollama_model = v;
    }
    if let Some(v) = get("HIFZ_AUTO_COMPRESS") {
        cfg.auto_compress = parse_bool(&v);
    }
    if let Some(v) = get("CONSOLIDATION_ENABLED") {
        cfg.consolidation_enabled = parse_bool(&v);
    }
    if let Some(v) = get("TOKEN_BUDGET") {
        cfg.token_budget = parse_usize(&v, 2000);
    }
    if let Some(v) = get("MAX_OBS_PER_SESSION") {
        cfg.max_obs_per_session = parse_usize(&v, 500);
    }

    cfg
}

/// dirs crate alternative — just use home_dir
mod dirs {
    use std::path::PathBuf;

    pub fn home_dir() -> Option<PathBuf> {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}
