// SPDX-License-Identifier: Apache-2.0
//! Single source of colour truth. `rgb` drives the ANSI sink; `tokens`
//! is exported as JSON so the web UI uses the identical palette (no drift
//! between terminal and browser).

use crate::model::Tone;

/// 24-bit colour for a semantic tone (dark-background palette).
pub fn rgb(t: Tone) -> (u8, u8, u8) {
    match t {
        Tone::Plain => (0xc9, 0xd1, 0xd9),
        Tone::Added => (0x7e, 0xe7, 0x8a),
        Tone::Revised => (0x79, 0xc0, 0xff),
        Tone::Removed => (0xff, 0x7b, 0x72),
        Tone::Linked => (0xd2, 0xa8, 0xff),
        Tone::Conflict => (0xf8, 0x51, 0x49),
        Tone::Muted => (0x8b, 0x94, 0x9e),
        Tone::Cite => (0x58, 0xa6, 0xff),
    }
}

fn hex(t: Tone) -> String {
    let (r, g, b) = rgb(t);
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// Tone-name → hex map, consumed by the web UI as `tokens.json`. Keys match
/// `Tone`'s serde (snake_case) representation.
pub fn tokens() -> serde_json::Value {
    serde_json::json!({
        "plain": hex(Tone::Plain),
        "added": hex(Tone::Added),
        "revised": hex(Tone::Revised),
        "removed": hex(Tone::Removed),
        "linked": hex(Tone::Linked),
        "conflict": hex(Tone::Conflict),
        "muted": hex(Tone::Muted),
        "cite": hex(Tone::Cite),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_cover_every_tone_as_hex() {
        let t = tokens();
        for k in [
            "plain", "added", "revised", "removed", "linked", "conflict", "muted", "cite",
        ] {
            let v = t.get(k).and_then(|v| v.as_str()).unwrap();
            assert!(v.starts_with('#') && v.len() == 7, "{k} = {v}");
        }
    }
}
