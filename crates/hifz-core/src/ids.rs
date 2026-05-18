//! RecordId / string helpers, extracted from `hifz`'s `lib.rs` so the
//! shared crates can use them. `rid_to_string` is now `pub` (was
//! `pub(crate)` in hifz) — re-exported at the hifz crate root so
//! `crate::rid_to_string` still resolves.

/// Canonical `table:key` string for a SurrealDB [`RecordId`].
///
/// `RecordId`'s `Debug`/`Display` renders as
/// `RecordId { table: Table("memory"), key: String("...") }`, which is
/// useless to API consumers (they can't feed it back to delete/evolve/link).
/// This produces the canonical `memory:<key>` form instead.
pub fn rid_to_string(rid: &surrealdb::types::RecordId) -> String {
    use surrealdb::types::RecordIdKey;
    let key = match &rid.key {
        RecordIdKey::String(s) => s.clone(),
        RecordIdKey::Number(n) => n.to_string(),
        RecordIdKey::Uuid(u) => u.to_string(),
        other => format!("{other:?}"),
    };
    format!("{}:{key}", rid.table)
}

/// Truncate a string at the largest char boundary `<= max_bytes`.
///
/// Plain `&s[..max_bytes]` panics when `max_bytes` lands inside a multi-byte
/// UTF-8 codepoint (e.g. shell-prompt glyphs `✗`, `➜`, box-drawing `─`).
/// Use this anywhere you'd otherwise byte-slice user-supplied text.
pub fn truncate_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}
