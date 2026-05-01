use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;

pub struct Spool {
    dir: PathBuf,
}

impl Spool {
    pub fn new(dir: PathBuf) -> std::io::Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    pub fn append(&self, kind: &str, body: &serde_json::Value) -> std::io::Result<()> {
        let line = serde_json::to_string(&serde_json::json!({
            "kind": kind,
            "body": body,
            "ts": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        }))
        .unwrap();
        let path = self.active_file()?;
        let mut f = OpenOptions::new().create(true).append(true).open(path)?;
        f.write_all(line.as_bytes())?;
        f.write_all(b"\n")?;
        Ok(())
    }

    /// Iterates over spooled records in append order. Returns (file_path, JSON line).
    pub fn drain(&self) -> std::io::Result<Vec<(PathBuf, String)>> {
        let mut out = Vec::new();
        let mut entries: Vec<_> = fs::read_dir(&self.dir)?
            .flatten()
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("jsonl"))
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let path = e.path();
            let body = fs::read_to_string(&path)?;
            for line in body.lines() {
                if !line.is_empty() {
                    out.push((path.clone(), line.to_string()));
                }
            }
        }
        Ok(out)
    }

    pub fn remove(&self, file: &PathBuf) {
        let _ = fs::remove_file(file);
    }

    fn active_file(&self) -> std::io::Result<PathBuf> {
        let day = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let mut i = 0u32;
        loop {
            let name = if i == 0 {
                format!("{day}.jsonl")
            } else {
                format!("{day}-{i}.jsonl")
            };
            let path = self.dir.join(&name);
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if size < MAX_FILE_BYTES {
                return Ok(path);
            }
            i += 1;
        }
    }
}
