//! Generic, model-name-agnostic usage patterns.
//!
//! Every pattern derives its thresholds from data — none of them hardcodes
//! a vendor or model name. The 10 generators are direct ports of
//! claude-spend's heuristics with cost math stripped (token math only).
//!
//! Skip on this port:
//!   - day-of-week (cross-time slice we don't surface)
//!   - project-dominance (cross-project comparison we don't surface)

use crate::models::UsagePattern;
use crate::usage::aggregate::{UsageCallRow, sum_calls};

/// Patterns scoped to a single session.
pub fn for_session(calls: &[UsageCallRow]) -> Vec<UsagePattern> {
    if calls.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    push_some(&mut out, context_growth_session(calls));
    push_some(&mut out, heavy_context_first_call(calls));
    push_some(&mut out, tool_heavy_session(calls));
    push_some(&mut out, smart_clear_inflection(calls));
    out
}

/// Patterns scoped to a whole project (set of sessions).
pub fn for_project(calls: &[UsageCallRow]) -> Vec<UsagePattern> {
    if calls.is_empty() {
        return Vec::new();
    }
    let sessions = group_by_session(calls);
    let mut out = Vec::new();
    push_some(&mut out, vague_prompts(calls));
    push_some(&mut out, context_growth_project(&sessions));
    push_some(&mut out, marathon_sessions(&sessions));
    push_some(&mut out, input_heavy(calls));
    push_some(&mut out, expensive_model_for_short(&sessions));
    push_some(&mut out, tool_heavy_project(&sessions));
    push_some(&mut out, per_msg_efficiency(&sessions));
    push_some(&mut out, heavy_context_project(&sessions));
    push_some(&mut out, cache_share(calls));
    push_some(&mut out, smart_clear_inflection_project(&sessions));
    out
}

// === 1. vague-prompts ===

fn vague_prompts(calls: &[UsageCallRow]) -> Option<UsagePattern> {
    // Group consecutive calls by shared prompt and sum tokens per prompt.
    let mut prompts: Vec<(String, i64)> = Vec::new();
    let mut cur: Option<(String, i64)> = None;
    for c in calls {
        match c.prompt.as_ref() {
            Some(p) if cur.as_ref().map(|(s, _)| s) != Some(p) => {
                if let Some(row) = cur.take() {
                    prompts.push(row);
                }
                cur = Some((p.clone(), 0));
            }
            _ => {}
        }
        if let Some((_, t)) = cur.as_mut() {
            *t += c.total_tokens;
        }
    }
    if let Some(row) = cur {
        prompts.push(row);
    }
    let short_expensive: Vec<&(String, i64)> = prompts
        .iter()
        .filter(|(p, t)| p.trim().chars().count() < 30 && *t > 100_000)
        .collect();
    if short_expensive.is_empty() {
        return None;
    }
    let wasted: i64 = short_expensive.iter().map(|(_, t)| *t).sum();
    Some(UsagePattern {
        id: "vague-prompts".into(),
        kind: "warning".into(),
        title: "Short, vague messages cost the most".into(),
        body: format!(
            "{} short messages each burned over 100K tokens — together {}. Vague prompts force the model to re-scan the conversation and search files to figure out what you wanted.",
            short_expensive.len(),
            fmt_tokens(wasted)
        ),
        action: Some(
            "Lead with the specific file or function. \"Fix the bug in src/auth.rs:42\" triggers fewer tool calls than \"fix the login bug.\""
                .into(),
        ),
    })
}

// === 2. context-growth (project-level: how many sessions exhibit it) ===

fn context_growth_project(sessions: &[Vec<UsageCallRow>]) -> Option<UsagePattern> {
    let long: Vec<&Vec<UsageCallRow>> = sessions.iter().filter(|s| s.len() > 50).collect();
    if long.is_empty() {
        return None;
    }
    let grown: Vec<(f64, &Vec<UsageCallRow>)> = long
        .iter()
        .filter_map(|s| {
            let first = avg_total(s.iter().take(5));
            let last = avg_total(s.iter().rev().take(5));
            if first > 0.0 && last / first > 2.0 {
                Some((last / first, *s))
            } else {
                None
            }
        })
        .collect();
    if grown.is_empty() {
        return None;
    }
    let avg_ratio: f64 = grown.iter().map(|(r, _)| r).sum::<f64>() / grown.len() as f64;
    Some(UsagePattern {
        id: "context-growth".into(),
        kind: "warning".into(),
        title: "Long conversations get more expensive per turn".into(),
        body: format!(
            "{} sessions show the last 5 turns averaging {:.1}× the tokens of the first 5. Each turn re-reads the whole history, so cost compounds.",
            grown.len(),
            avg_ratio
        ),
        action: Some(
            "Start a fresh conversation when switching tasks. Paste a one-paragraph summary to seed the new one."
                .into(),
        ),
    })
}

// Session-level variant: just one session.
fn context_growth_session(calls: &[UsageCallRow]) -> Option<UsagePattern> {
    if calls.len() < 50 {
        return None;
    }
    let first = avg_total(calls.iter().take(5));
    let last = avg_total(calls.iter().rev().take(5));
    if first <= 0.0 || last / first <= 2.0 {
        return None;
    }
    let ratio = last / first;
    Some(UsagePattern {
        id: "context-growth".into(),
        kind: "warning".into(),
        title: "This conversation is growing expensive".into(),
        body: format!(
            "Last 5 turns average {:.1}× the tokens of the first 5. The model is re-reading more history every turn.",
            ratio
        ),
        action: Some("Consider /clear and a brief summary to start fresh.".into()),
    })
}

// === 3. marathon-sessions ===

fn marathon_sessions(sessions: &[Vec<UsageCallRow>]) -> Option<UsagePattern> {
    let long: Vec<&Vec<UsageCallRow>> = sessions.iter().filter(|s| s.len() > 200).collect();
    if long.len() < 2 {
        return None;
    }
    let long_tokens: i64 = long
        .iter()
        .map(|s| s.iter().map(|c| c.total_tokens).sum::<i64>())
        .sum();
    let all_tokens: i64 = sessions
        .iter()
        .flat_map(|s| s.iter())
        .map(|c| c.total_tokens)
        .sum();
    let pct = if all_tokens > 0 {
        (long_tokens as f64 / all_tokens as f64) * 100.0
    } else {
        0.0
    };
    Some(UsagePattern {
        id: "marathon-sessions".into(),
        kind: "info".into(),
        title: format!(
            "{} marathon sessions used {:.0}% of project tokens",
            long.len(),
            pct
        ),
        body: format!(
            "Sessions with >200 calls dominate this project's usage: {} of total {}.",
            fmt_tokens(long_tokens),
            fmt_tokens(all_tokens)
        ),
        action: None,
    })
}

// === 4. input-heavy ===

fn input_heavy(calls: &[UsageCallRow]) -> Option<UsagePattern> {
    let totals = sum_calls(calls);
    if totals.total <= 0 {
        return None;
    }
    let out_pct = (totals.output as f64 / totals.total as f64) * 100.0;
    if out_pct >= 2.0 {
        return None;
    }
    Some(UsagePattern {
        id: "input-heavy".into(),
        kind: "info".into(),
        title: format!("{:.1}% of tokens are the model writing", out_pct),
        body: format!(
            "Of {} tokens, only {} are the model's responses; the rest is conversation history and tool output being re-read.",
            fmt_tokens(totals.total),
            fmt_tokens(totals.output)
        ),
        action: Some("Shorter conversations save more than shorter answers.".into()),
    })
}

// === 5. expensive-model-for-short-sessions ===

fn expensive_model_for_short(sessions: &[Vec<UsageCallRow>]) -> Option<UsagePattern> {
    // Find the model with the highest avg-tokens-per-call across all sessions.
    use std::collections::HashMap;
    let mut per_model_total: HashMap<String, (i64, i64)> = HashMap::new();
    for s in sessions {
        for c in s {
            let e = per_model_total.entry(c.model.clone()).or_insert((0, 0));
            e.0 += c.total_tokens;
            e.1 += 1;
        }
    }
    let priciest = per_model_total
        .iter()
        .filter(|(_, (_, n))| *n > 0)
        .max_by(|a, b| {
            let av = a.1.0 as f64 / a.1.1 as f64;
            let bv = b.1.0 as f64 / b.1.1 as f64;
            av.partial_cmp(&bv).unwrap_or(std::cmp::Ordering::Equal)
        });
    let priciest = priciest?;
    let priciest_name = priciest.0.clone();
    let short: Vec<&Vec<UsageCallRow>> = sessions
        .iter()
        .filter(|s| {
            s.len() < 10
                && s.iter().map(|c| c.total_tokens).sum::<i64>() < 200_000
                && s.iter().any(|c| c.model == priciest_name)
        })
        .collect();
    if short.len() < 3 {
        return None;
    }
    let wasted: i64 = short
        .iter()
        .flat_map(|s| s.iter())
        .map(|c| c.total_tokens)
        .sum();
    Some(UsagePattern {
        id: "expensive-model-for-short-sessions".into(),
        kind: "warning".into(),
        title: format!(
            "Heavyweight model ({}) used on short sessions",
            priciest_name
        ),
        body: format!(
            "{} sessions had fewer than 10 calls and under 200K tokens but ran on {} ({} tokens total). A lighter model would likely have produced similar results.",
            short.len(),
            priciest_name,
            fmt_tokens(wasted)
        ),
        action: Some("Switch to a lighter model for quick tasks.".into()),
    })
}

// === 6. tool-heavy ===

fn tool_heavy_project(sessions: &[Vec<UsageCallRow>]) -> Option<UsagePattern> {
    let heavy: Vec<&Vec<UsageCallRow>> = sessions.iter().filter(|s| is_tool_heavy(s)).collect();
    if heavy.len() < 3 {
        return None;
    }
    let total: i64 = heavy
        .iter()
        .flat_map(|s| s.iter())
        .map(|c| c.total_tokens)
        .sum();
    Some(UsagePattern {
        id: "tool-heavy".into(),
        kind: "info".into(),
        title: format!("{} sessions were tool-call heavy", heavy.len()),
        body: format!(
            "More than 3 tool calls per user message. Each tool call re-reads the full history; these sessions used {} tokens total.",
            fmt_tokens(total)
        ),
        action: Some("Point to specific files or line ranges to cut tool-call rounds.".into()),
    })
}

fn tool_heavy_session(calls: &[UsageCallRow]) -> Option<UsagePattern> {
    if !is_tool_heavy(calls) {
        return None;
    }
    let user_msgs = calls.iter().filter(|c| c.prompt.is_some()).count();
    let tool_calls = calls.len().saturating_sub(user_msgs);
    if user_msgs == 0 {
        return None;
    }
    let ratio = tool_calls as f64 / user_msgs as f64;
    Some(UsagePattern {
        id: "tool-heavy".into(),
        kind: "info".into(),
        title: "Tool-heavy conversation".into(),
        body: format!(
            "About {:.0} tool calls per user message. Each tool round-trip re-reads the conversation, compounding cost.",
            ratio
        ),
        action: Some("Point Claude at specific files / lines to reduce tool rounds.".into()),
    })
}

fn is_tool_heavy(calls: &[UsageCallRow]) -> bool {
    let user = calls.iter().filter(|c| c.prompt.is_some()).count();
    let tool = calls.len().saturating_sub(user);
    user > 0 && tool > user * 3
}

// === 7. per-message efficiency ===

fn per_msg_efficiency(sessions: &[Vec<UsageCallRow>]) -> Option<UsagePattern> {
    let short: Vec<&Vec<UsageCallRow>> = sessions
        .iter()
        .filter(|s| s.len() >= 3 && s.len() <= 15)
        .collect();
    let long: Vec<&Vec<UsageCallRow>> = sessions.iter().filter(|s| s.len() > 80).collect();
    if short.len() < 3 || long.len() < 2 {
        return None;
    }
    let short_avg = short
        .iter()
        .map(|s| {
            let total: i64 = s.iter().map(|c| c.total_tokens).sum();
            total as f64 / s.len() as f64
        })
        .sum::<f64>()
        / short.len() as f64;
    let long_avg = long
        .iter()
        .map(|s| {
            let total: i64 = s.iter().map(|c| c.total_tokens).sum();
            total as f64 / s.len() as f64
        })
        .sum::<f64>()
        / long.len() as f64;
    if short_avg <= 0.0 {
        return None;
    }
    let ratio = long_avg / short_avg;
    if ratio < 2.0 {
        return None;
    }
    Some(UsagePattern {
        id: "per-msg-efficiency".into(),
        kind: "warning".into(),
        title: format!("Each turn costs {:.1}× more in long conversations", ratio),
        body: format!(
            "Short conversations average ~{} tokens/turn; long ones average ~{}. Same task, very different cost — the model re-reads more history each time.",
            fmt_tokens(short_avg as i64),
            fmt_tokens(long_avg as i64)
        ),
        action: Some(
            "Start fresh conversations more often; this is the single biggest lever.".into(),
        ),
    })
}

// === 8. heavy-context (first turn input > 50K) ===

fn heavy_context_project(sessions: &[Vec<UsageCallRow>]) -> Option<UsagePattern> {
    let heavy: Vec<&Vec<UsageCallRow>> = sessions
        .iter()
        .filter(|s| s.first().map(|c| c.input_tokens).unwrap_or(0) > 50_000)
        .collect();
    if heavy.len() < 5 {
        return None;
    }
    let avg = heavy
        .iter()
        .map(|s| s.first().map(|c| c.input_tokens).unwrap_or(0))
        .sum::<i64>()
        / heavy.len() as i64;
    let total: i64 = heavy
        .iter()
        .map(|s| s.first().map(|c| c.input_tokens).unwrap_or(0))
        .sum();
    Some(UsagePattern {
        id: "heavy-context".into(),
        kind: "info".into(),
        title: format!("{} sessions started with heavy context", heavy.len()),
        body: format!(
            "Average starting input was {}; total {} tokens of context loaded before the first user message. CLAUDE.md and system context get re-read every turn.",
            fmt_tokens(avg),
            fmt_tokens(total)
        ),
        action: Some(
            "Trim CLAUDE.md to what you actually need — savings compound across every turn.".into(),
        ),
    })
}

fn heavy_context_first_call(calls: &[UsageCallRow]) -> Option<UsagePattern> {
    let first = calls.first()?;
    if first.input_tokens <= 50_000 {
        return None;
    }
    Some(UsagePattern {
        id: "heavy-context".into(),
        kind: "info".into(),
        title: "Heavy starting context".into(),
        body: format!(
            "The first call already loaded {} input tokens of context. This gets re-read every subsequent turn.",
            fmt_tokens(first.input_tokens)
        ),
        action: Some("Trim CLAUDE.md or system prompt; savings compound across turns.".into()),
    })
}

// === 9. cache-share (any breakdown key, report share of total input) ===

fn cache_share(calls: &[UsageCallRow]) -> Option<UsagePattern> {
    let t = sum_calls(calls);
    if t.cache_read <= 0 {
        return None;
    }
    let denom = t.input + t.cache_read + t.cache_creation;
    if denom <= 0 {
        return None;
    }
    let hit_rate = (t.cache_read as f64 / denom as f64) * 100.0;
    Some(UsagePattern {
        id: "cache-share".into(),
        kind: "info".into(),
        title: format!("Cache hit rate: {:.1}%", hit_rate),
        body: format!(
            "{} of {} input tokens were served from cache. Cache reuse is highest in long, focused conversations with stable context.",
            fmt_tokens(t.cache_read),
            fmt_tokens(denom)
        ),
        action: None,
    })
}

// === 10. smart-clear inflection ===

fn smart_clear_inflection(calls: &[UsageCallRow]) -> Option<UsagePattern> {
    if calls.len() < 10 {
        return None;
    }
    let baseline: f64 = calls
        .iter()
        .take(5)
        .map(|c| c.total_tokens as f64)
        .sum::<f64>()
        / 5.0;
    if baseline <= 0.0 {
        return None;
    }
    for i in 2..calls.len() {
        let window = (calls[i].total_tokens + calls[i - 1].total_tokens + calls[i - 2].total_tokens)
            as f64
            / 3.0;
        if window > baseline * 2.0 {
            let later_avg: f64 = calls
                .iter()
                .skip(i - 1)
                .map(|c| c.total_tokens as f64)
                .sum::<f64>()
                / (calls.len() - (i - 1)) as f64;
            let mult = later_avg / baseline;
            return Some(UsagePattern {
                id: "smart-clear".into(),
                kind: "warning".into(),
                title: format!("Cost inflected around call {}", i - 1),
                body: format!(
                    "After call {}, average tokens-per-call rose to {:.1}× baseline. A /clear (with a brief handoff note) would have reset the growth.",
                    i - 1,
                    mult
                ),
                action: Some(
                    "Try /clear around this point next time; paste a short summary to seed the next session."
                        .into(),
                ),
            });
        }
    }
    None
}

fn smart_clear_inflection_project(sessions: &[Vec<UsageCallRow>]) -> Option<UsagePattern> {
    // Median inflection point across sessions where one exists.
    let mut inflections: Vec<usize> = Vec::new();
    let mut multipliers: Vec<f64> = Vec::new();
    for s in sessions.iter().filter(|s| s.len() >= 10) {
        let baseline: f64 = s.iter().take(5).map(|c| c.total_tokens as f64).sum::<f64>() / 5.0;
        if baseline <= 0.0 {
            continue;
        }
        for i in 2..s.len() {
            let window =
                (s[i].total_tokens + s[i - 1].total_tokens + s[i - 2].total_tokens) as f64 / 3.0;
            if window > baseline * 2.0 {
                let later: f64 = s
                    .iter()
                    .skip(i - 1)
                    .map(|c| c.total_tokens as f64)
                    .sum::<f64>()
                    / (s.len() - (i - 1)) as f64;
                inflections.push(i - 1);
                multipliers.push(later / baseline);
                break;
            }
        }
    }
    if inflections.len() < 2 {
        return None;
    }
    inflections.sort();
    let median = inflections[inflections.len() / 2];
    let avg_mult: f64 = multipliers.iter().sum::<f64>() / multipliers.len() as f64;
    Some(UsagePattern {
        id: "smart-clear".into(),
        kind: "warning".into(),
        title: format!("Most sessions inflect around call {}", median),
        body: format!(
            "Across {} sessions, cost-per-call jumps to {:.1}× baseline around the {}th call. /clear at that point would reset the growth.",
            inflections.len(),
            avg_mult,
            median
        ),
        action: Some(format!(
            "Try /clear after ~{} calls when the context feels heavy.",
            median
        )),
    })
}

// === helpers ===

fn group_by_session(calls: &[UsageCallRow]) -> Vec<Vec<UsageCallRow>> {
    use std::collections::HashMap;
    let mut by: HashMap<String, Vec<UsageCallRow>> = HashMap::new();
    for c in calls {
        by.entry(c.session_id.clone()).or_default().push(c.clone());
    }
    for s in by.values_mut() {
        s.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    }
    by.into_values().collect()
}

fn avg_total<'a>(iter: impl Iterator<Item = &'a UsageCallRow>) -> f64 {
    let mut n = 0usize;
    let mut s = 0i64;
    for c in iter {
        n += 1;
        s += c.total_tokens;
    }
    if n == 0 { 0.0 } else { s as f64 / n as f64 }
}

fn push_some(out: &mut Vec<UsagePattern>, opt: Option<UsagePattern>) {
    if let Some(p) = opt {
        out.push(p);
    }
}

fn fmt_tokens(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{}K", n / 1_000)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
