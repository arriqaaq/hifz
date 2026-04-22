/// Detect a git commit from a Claude Code Bash PostToolUse payload.
///
/// Parses `tool_input.command` for `git commit` (excluding --dry-run),
/// then parses `tool_output` for the success pattern `[branch sha] message`.

#[derive(Debug, Clone)]
pub struct DetectedCommit {
    pub sha: String,
    pub message: String,
    pub branch: String,
}

/// Try to detect a successful git commit from a Bash hook payload.
/// Returns None if the command is not a commit or the output doesn't show success.
pub fn detect_commit(data: &serde_json::Value) -> Option<DetectedCommit> {
    let tool_input = data.get("tool_input").or_else(|| data.get("toolInput"))?;
    let command = tool_input
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if !is_git_commit_command(command) {
        return None;
    }

    let raw_output = data
        .get("tool_output")
        .or_else(|| data.get("toolOutput"))
        .unwrap_or(&serde_json::Value::Null);

    // tool_response can be a plain string or a JSON object like {"stdout": "..."}
    let output_str = if let Some(s) = raw_output.as_str() {
        // Could be a JSON string containing {"stdout": ...}
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
            parsed
                .get("stdout")
                .and_then(|v| v.as_str())
                .unwrap_or(s)
                .to_string()
        } else {
            s.to_string()
        }
    } else if let Some(stdout) = raw_output.get("stdout").and_then(|v| v.as_str()) {
        stdout.to_string()
    } else {
        String::new()
    };

    parse_commit_output(&output_str)
}

fn is_git_commit_command(command: &str) -> bool {
    // Split compound commands on && ; || and check each sub-command
    for sub in command.split(&['&', ';'][..]) {
        let trimmed = sub.trim().trim_start_matches('&').trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();

        let Some(git_idx) = parts.iter().position(|&p| p == "git") else {
            continue;
        };
        let Some(commit_idx) = parts[git_idx + 1..]
            .iter()
            .position(|&p| !p.starts_with('-') || p == "--")
            .map(|i| i + git_idx + 1)
        else {
            continue;
        };

        if parts.get(commit_idx) != Some(&"commit") {
            continue;
        }

        if parts.iter().any(|&p| p == "--dry-run" || p == "-n") {
            continue;
        }

        return true;
    }
    false
}

/// Parse git commit output for the success pattern.
/// Git outputs: `[branch_name short_sha] commit message`
/// e.g. `[main abc1234] fix: resolve race condition`
fn parse_commit_output(output: &str) -> Option<DetectedCommit> {
    // Look for the pattern [branch sha] in the output
    // Git may output other text before/after, so scan line by line
    for line in output.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('[') {
            continue;
        }

        let close_bracket = trimmed.find(']')?;
        let inside = &trimmed[1..close_bracket];
        let rest = trimmed[close_bracket + 1..].trim();

        // Inside bracket: "branch sha" or "branch (root-commit) sha"
        let parts: Vec<&str> = inside.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        let branch = parts[0].to_string();
        // The SHA is always the last token inside the brackets
        let sha = parts[parts.len() - 1].to_string();

        // Validate SHA looks like a hex string (at least 7 chars)
        if sha.len() < 7 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }

        return Some(DetectedCommit {
            sha,
            message: rest.to_string(),
            branch,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_commit() {
        let output =
            "[main abc1234f] fix: resolve race condition\n 2 files changed, 10 insertions(+)";
        let result = parse_commit_output(output).unwrap();
        assert_eq!(result.branch, "main");
        assert_eq!(result.sha, "abc1234f");
        assert_eq!(result.message, "fix: resolve race condition");
    }

    #[test]
    fn parse_root_commit() {
        let output = "[main (root-commit) abc1234f] initial commit";
        let result = parse_commit_output(output).unwrap();
        assert_eq!(result.branch, "main");
        assert_eq!(result.sha, "abc1234f");
        assert_eq!(result.message, "initial commit");
    }

    #[test]
    fn parse_with_preceding_text() {
        let output =
            "Some warning text\n[feature/auth a1b2c3d] add JWT middleware\n 3 files changed";
        let result = parse_commit_output(output).unwrap();
        assert_eq!(result.branch, "feature/auth");
        assert_eq!(result.sha, "a1b2c3d");
    }

    #[test]
    fn no_commit_output() {
        let output = "nothing to commit, working tree clean";
        assert!(parse_commit_output(output).is_none());
    }

    #[test]
    fn detect_dry_run() {
        assert!(!is_git_commit_command("git commit --dry-run -m 'test'"));
        assert!(!is_git_commit_command("git commit -n -m 'test'"));
    }

    #[test]
    fn detect_real_commit() {
        assert!(is_git_commit_command("git commit -m 'fix bug'"));
        assert!(is_git_commit_command("git commit -am 'fix bug'"));
        assert!(is_git_commit_command("git commit"));
    }

    #[test]
    fn detect_non_commit() {
        assert!(!is_git_commit_command("git status"));
        assert!(!is_git_commit_command("git log --oneline"));
    }

    #[test]
    fn detect_compound_command() {
        assert!(is_git_commit_command("git add . && git commit -m 'fix'"));
        assert!(is_git_commit_command(
            "git add -A && git commit -am 'update'"
        ));
        assert!(is_git_commit_command(
            "git add file.rs; git commit -m 'done'"
        ));
    }
}
