//! Rust-side memory scoring.
//!
//! SurrealDB has neither `math::exp` nor `time::diff`, so the Mem0-style
//! recency/access reinforcement formula runs here, against columns we SELECT
//! from the database.
//!
//! score = strength * exp(-age_days / HALF_LIFE) * (1 + ACCESS_COEF * min(access, ACCESS_CAP))

use chrono::{DateTime, Utc};

/// Ebbinghaus half-life for recency decay (days).
pub const HALF_LIFE_DAYS: f64 = 30.0;
/// Per-retrieval access boost coefficient (Mem0 default).
pub const ACCESS_COEF: f64 = 0.1;
/// Cap on access reinforcement so hot items don't dominate forever.
pub const ACCESS_CAP: i64 = 20;

/// Compute the recency × access multiplier for a memory row.
pub fn recency_access_boost(created_at: &str, retrieval_count: i64) -> f64 {
    let age_days = age_days_since(created_at);
    let recency = (-age_days / HALF_LIFE_DAYS).exp();
    let access = 1.0 + ACCESS_COEF * (retrieval_count.clamp(0, ACCESS_CAP) as f64);
    recency * access
}

/// Compose a final ranking score from a base score (e.g. strength or RRF)
/// and the recency/access boost.
pub fn final_score(base: f64, created_at: &str, retrieval_count: i64) -> f64 {
    base * recency_access_boost(created_at, retrieval_count)
}

fn age_days_since(created_at: &str) -> f64 {
    let Ok(parsed) = DateTime::parse_from_rfc3339(created_at) else {
        return 0.0;
    };
    let age = Utc::now().signed_duration_since(parsed.with_timezone(&Utc));
    age.num_seconds().max(0) as f64 / 86_400.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recency_halves_roughly_at_one_half_life() {
        let long_ago = (Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        let boost = recency_access_boost(&long_ago, 0);
        // e^-1 ≈ 0.3679 — check loosely
        assert!((boost - 0.3679).abs() < 0.01, "got {boost}");
    }

    #[test]
    fn access_boost_caps() {
        let now = Utc::now().to_rfc3339();
        let capped = recency_access_boost(&now, 100);
        // 1 + 0.1 * 20 = 3.0
        assert!((capped - 3.0).abs() < 0.001, "got {capped}");
    }

    #[test]
    fn fresh_and_unused_is_one() {
        let now = Utc::now().to_rfc3339();
        let boost = recency_access_boost(&now, 0);
        assert!((boost - 1.0).abs() < 0.001, "got {boost}");
    }

    #[test]
    fn parse_failure_is_zero_age() {
        let boost = recency_access_boost("not-a-date", 0);
        // If parse fails we pretend age=0 → boost=1.0. Better than zeroing out the row.
        assert!((boost - 1.0).abs() < 0.001);
    }
}
