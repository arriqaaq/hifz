// SPDX-License-Identifier: Apache-2.0
//! Compute-once: map `Change` records into the structured `MemoryDelta`,
//! and build a `MemoryView` for the inspect surface. Pure; no I/O.

use crate::model::{Change, ChangeOp, Cite, DeltaLine, MemoryView, Span, Tone};

/// Turn the mutation's `Change` list into one structured diff.
pub fn delta_from_changes(changes: &[Change]) -> crate::model::MemoryDelta {
    crate::model::MemoryDelta {
        lines: changes.iter().map(line_for).collect(),
    }
}

/// Build an inspect view: a header for `title`/`category`/`id` plus one row
/// per lineage/link/evolution `Change` the caller assembled (no DB here).
pub fn view_of(title: &str, category: &str, id: &str, history: &[Change]) -> MemoryView {
    let header = vec![
        Span::new(title.to_string(), Tone::Added).styled(|s| s.bold = true),
        Span::muted(format!("({category})")),
        Span::new(id.to_string(), Tone::Cite).with_cite(Cite::Memory { id: id.to_string() }),
    ];
    MemoryView {
        header,
        rows: history.iter().map(line_for).collect(),
    }
}

fn opt(prev: &Option<String>) -> String {
    match prev {
        Some(p) if !p.is_empty() => p.clone(),
        _ => "∅".to_string(),
    }
}

fn line_for(c: &Change) -> DeltaLine {
    match c {
        Change::Created {
            id,
            title,
            category,
        } => DeltaLine::new(
            ChangeOp::Created,
            vec![
                Span::muted("memory"),
                Span::new(title.clone(), Tone::Added)
                    .styled(|s| s.bold = true)
                    .with_cite(Cite::Memory { id: id.clone() }),
                Span::muted(format!("({category})")),
            ],
        ),
        Change::Superseded { old_id, new_id } => DeltaLine::new(
            ChangeOp::Superseded,
            vec![
                Span::new(old_id.clone(), Tone::Removed).styled(|s| s.strike = true),
                Span::muted("→"),
                Span::new(new_id.clone(), Tone::Added)
                    .with_cite(Cite::Memory { id: new_id.clone() }),
            ],
        ),
        Change::SelfRevised {
            id,
            field,
            previous,
        } => DeltaLine::new(
            ChangeOp::Revised,
            vec![
                Span::new(field.clone(), Tone::Revised),
                Span::muted("was"),
                Span::new(opt(previous), Tone::Muted)
                    .styled(|s| s.dim = true)
                    .with_cite(Cite::Memory { id: id.clone() }),
            ],
        ),
        Change::NeighbourRevised {
            id,
            field,
            previous,
            reason,
        } => DeltaLine::new(
            ChangeOp::NeighbourRevised,
            vec![
                Span::new(field.clone(), Tone::Revised),
                Span::muted(format!("({reason})")),
                Span::new(opt(previous), Tone::Muted)
                    .styled(|s| s.dim = true)
                    .with_cite(Cite::Memory { id: id.clone() }),
            ],
        ),
        Change::Linked {
            from,
            to,
            relation,
            score,
            reason,
            via,
        } => {
            let mut spans = vec![
                Span::plain(from.clone()),
                Span::new(format!("-[{relation}]→"), Tone::Linked),
                Span::plain(to.clone()).with_cite(Cite::Edge {
                    relation: relation.clone(),
                    target: to.clone(),
                }),
                Span::muted(format!("({score:.2}, via {via})")),
            ];
            if let Some(r) = reason {
                spans.push(Span::muted(format!("— {r}")).styled(|s| s.dim = true));
            }
            DeltaLine::new(ChangeOp::Linked, spans)
        }
        Change::Forgotten { id } => DeltaLine::new(
            ChangeOp::Forgotten,
            vec![
                Span::new(id.clone(), Tone::Removed)
                    .styled(|s| s.strike = true)
                    .with_cite(Cite::Memory { id: id.clone() }),
            ],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Glyph;

    #[test]
    fn maps_every_variant_to_its_op_and_glyph() {
        let changes = vec![
            Change::Created {
                id: "memory:a".into(),
                title: "Auth uses JWT".into(),
                category: "decision".into(),
            },
            Change::Superseded {
                old_id: "memory:a".into(),
                new_id: "memory:b".into(),
            },
            Change::SelfRevised {
                id: "memory:b".into(),
                field: "keywords".into(),
                previous: Some("jwt".into()),
            },
            Change::NeighbourRevised {
                id: "memory:c".into(),
                field: "tags".into(),
                previous: None,
                reason: "LLM evolve".into(),
            },
            Change::Linked {
                from: "memory:c".into(),
                to: "memory:b".into(),
                relation: "related".into(),
                score: 0.5,
                reason: None,
                via: "llm".into(),
            },
            Change::Forgotten {
                id: "memory:z".into(),
            },
        ];
        let d = delta_from_changes(&changes);
        let ops: Vec<ChangeOp> = d.lines.iter().map(|l| l.op).collect();
        assert_eq!(
            ops,
            vec![
                ChangeOp::Created,
                ChangeOp::Superseded,
                ChangeOp::Revised,
                ChangeOp::NeighbourRevised,
                ChangeOp::Linked,
                ChangeOp::Forgotten,
            ]
        );
        // Glyph is derived from op — one visual language.
        assert_eq!(d.lines[0].glyph, Glyph::Plus);
        assert_eq!(d.lines[1].glyph, Glyph::Slashed);
        assert_eq!(d.lines[5].glyph, Glyph::Cross);
        // Forgotten previous renders as ∅ placeholder, not an empty line.
        let nr = &d.lines[3];
        assert!(nr.spans.iter().any(|s| s.text == "∅"));
    }

    #[test]
    fn view_has_header_and_rows() {
        let v = view_of(
            "Auth uses JWT",
            "decision",
            "memory:a",
            &[Change::Superseded {
                old_id: "memory:x".into(),
                new_id: "memory:a".into(),
            }],
        );
        assert_eq!(v.header.len(), 3);
        assert_eq!(v.rows.len(), 1);
        assert_eq!(v.rows[0].op, ChangeOp::Superseded);
    }
}
