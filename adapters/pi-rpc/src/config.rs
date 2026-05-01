use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(version, about = "Run `pi --mode rpc` and forward every event to Hifz.")]
pub struct Config {
    /// Path to the `pi` binary.
    #[arg(long, env = "PI_BIN", default_value = "pi")]
    pub pi_bin: String,

    /// Hifz database path (SurrealKV). Schema migration runs automatically on
    /// first open. Note: SurrealKV is single-writer — if `hifz serve` is
    /// running on this same path, it will refuse the lock.
    #[arg(
        long,
        env = "HIFZ_DB_PATH",
        default_value_os_t = default_db_path()
    )]
    pub db_path: std::path::PathBuf,

    /// Optional one-shot prompt sent on startup. If unset, run interactively (forward stdin to Pi).
    #[arg(long)]
    pub prompt: Option<String>,

    /// Project / cwd. Defaults to the harness's cwd.
    #[arg(long)]
    pub project: Option<String>,

    /// `extension_ui_request` reply policy: deny | terminal | allow.
    #[arg(long, default_value = "deny")]
    pub ui_mode: String,

    /// Capture per-token streaming deltas (massive volume; debug only).
    #[arg(long)]
    pub verbose_deltas: bool,

    /// Spool directory for events that fail to POST.
    #[arg(
        long,
        env = "HIFZ_SPOOL",
        default_value_os_t = default_spool()
    )]
    pub spool_dir: std::path::PathBuf,
}

fn default_spool() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join(".hifz/spool/pi-rpc")
}

fn default_db_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join(".hifz/data")
}
