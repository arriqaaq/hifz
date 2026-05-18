// SPDX-License-Identifier: Apache-2.0
//! Terminal sink: `MemoryDelta` → a string. Hand-rolled SGR (no deps).
//! Used by the CLI and the MCP text content block.

use crate::model::{Cite, MemoryDelta, MemoryView, Span, SpanStyle};
use crate::theme;

/// Rendering options.
#[derive(Debug, Clone, Copy)]
pub struct TextOpts {
    /// Emit ANSI SGR colour. When false, output is plain UTF-8 text.
    pub colour: bool,
}

impl Default for TextOpts {
    fn default() -> Self {
        TextOpts { colour: true }
    }
}

/// Render a diff. Each line is `  <glyph> <spans…>`.
pub fn render(delta: &MemoryDelta, opts: &TextOpts) -> String {
    let mut out = String::new();
    for line in &delta.lines {
        out.push_str("  ");
        out.push_str(line.glyph.unicode());
        out.push(' ');
        write_spans(&mut out, &line.spans, opts);
        out.push('\n');
    }
    out
}

/// Render an inspect view: header line, then indented rows.
pub fn render_view(view: &MemoryView, opts: &TextOpts) -> String {
    let mut out = String::new();
    write_spans(&mut out, &view.header, opts);
    out.push('\n');
    for line in &view.rows {
        out.push_str("  ");
        out.push_str(line.glyph.unicode());
        out.push(' ');
        write_spans(&mut out, &line.spans, opts);
        out.push('\n');
    }
    out
}

fn write_spans(out: &mut String, spans: &[Span], opts: &TextOpts) {
    for (i, sp) in spans.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut text = sp.text.clone();
        if let Some(c) = &sp.cite {
            text.push_str(&format!(" [{}]", cite_label(c)));
        }
        if opts.colour {
            out.push_str(&open_sgr(&sp.style));
            out.push_str(&text);
            out.push_str("\x1b[0m");
        } else {
            out.push_str(&text);
        }
    }
}

fn cite_label(c: &Cite) -> String {
    match c {
        Cite::Memory { id } => id.clone(),
        Cite::Edge { relation, target } => format!("{relation}:{target}"),
        Cite::Run { id } => id.clone(),
    }
}

fn open_sgr(style: &SpanStyle) -> String {
    let mut codes: Vec<String> = Vec::new();
    if style.bold {
        codes.push("1".into());
    }
    if style.dim {
        codes.push("2".into());
    }
    if style.strike {
        codes.push("9".into());
    }
    let (r, g, b) = theme::rgb(style.tone);
    codes.push(format!("38;2;{r};{g};{b}"));
    format!("\x1b[{}m", codes.join(";"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::delta_from_changes;
    use crate::model::Change;

    fn sample() -> MemoryDelta {
        delta_from_changes(&[
            Change::Created {
                id: "memory:a".into(),
                title: "Auth uses JWT".into(),
                category: "decision".into(),
            },
            Change::Superseded {
                old_id: "memory:a".into(),
                new_id: "memory:b".into(),
            },
        ])
    }

    #[test]
    fn plain_mode_has_no_escapes_and_expected_glyphs() {
        let s = render(&sample(), &TextOpts { colour: false });
        assert!(!s.contains('\x1b'), "plain mode must not emit SGR");
        assert!(s.contains("+ "), "created glyph");
        assert!(s.contains("⊘ "), "superseded glyph");
        assert!(s.contains("Auth uses JWT"));
        assert!(s.contains("[memory:a]"), "inline cite rendered");
        assert_eq!(s.lines().count(), 2);
    }

    #[test]
    fn colour_mode_wraps_and_resets_sgr() {
        let s = render(&sample(), &TextOpts { colour: true });
        assert!(s.contains("\x1b[38;2;"), "24-bit colour opened");
        assert!(s.contains("\x1b[0m"), "SGR reset");
    }
}
