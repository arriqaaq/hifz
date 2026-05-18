// SPDX-License-Identifier: Apache-2.0
//! Structured, surface-agnostic model for the memory-state diff.
//!
//! [`Change`] is the input contract (what a mutation did); everything else
//! is the rendered structure sinks consume. All record ids are plain
//! `String`s so this crate needs no DB types.

use serde::{Deserialize, Serialize};

/// One concrete change to memory state. hifz's mutation paths
/// (save / supersede / evolve / link / forget) emit a `Vec<Change>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Change {
    /// A new memory row was created.
    Created {
        id: String,
        title: String,
        category: String,
    },
    /// `old_id` was superseded by `new_id` (its `is_latest` was flipped).
    Superseded { old_id: String, new_id: String },
    /// The just-written memory's own metadata was revised in place.
    SelfRevised {
        id: String,
        field: String,
        previous: Option<String>,
    },
    /// A neighbour memory's metadata was revised during evolution.
    NeighbourRevised {
        id: String,
        field: String,
        previous: Option<String>,
        reason: String,
    },
    /// A typed edge `from -> to` was written.
    Linked {
        from: String,
        to: String,
        relation: String,
        score: f64,
        reason: Option<String>,
        via: String,
    },
    /// A memory was deleted.
    Forgotten { id: String },
}

/// The semantic operation a rendered line represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeOp {
    Created,
    Revised,
    Superseded,
    Linked,
    NeighbourRevised,
    Forgotten,
    Conflict,
}

/// Visual glyph for an operation. Glyphs are a conventional, hand-written
/// mapping (not copyrightable).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Glyph {
    Plus,
    Tilde,
    Slashed,
    Arrow,
    Recycle,
    Cross,
    Bang,
}

impl Glyph {
    /// The Unicode glyph string.
    pub fn unicode(self) -> &'static str {
        match self {
            Glyph::Plus => "+",
            Glyph::Tilde => "~",
            Glyph::Slashed => "⊘",
            Glyph::Arrow => "→",
            Glyph::Recycle => "↻",
            Glyph::Cross => "×",
            Glyph::Bang => "!",
        }
    }
}

impl ChangeOp {
    /// The glyph that always represents this op (one visual language).
    pub fn glyph(self) -> Glyph {
        match self {
            ChangeOp::Created => Glyph::Plus,
            ChangeOp::Revised => Glyph::Tilde,
            ChangeOp::Superseded => Glyph::Slashed,
            ChangeOp::Linked => Glyph::Arrow,
            ChangeOp::NeighbourRevised => Glyph::Recycle,
            ChangeOp::Forgotten => Glyph::Cross,
            ChangeOp::Conflict => Glyph::Bang,
        }
    }
}

/// Semantic colour token. `theme::rgb` resolves it to a concrete colour;
/// the web UI resolves the same token names from `theme::tokens()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    #[default]
    Plain,
    Added,
    Revised,
    Removed,
    Linked,
    Conflict,
    Muted,
    Cite,
}

/// Styling for a run of text.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SpanStyle {
    #[serde(default)]
    pub tone: Tone,
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub dim: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub strike: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Inline provenance pointer (hifz domain).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cite {
    Memory { id: String },
    Edge { relation: String, target: String },
    Run { id: String },
}

/// A styled run of text with optional inline provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub text: String,
    #[serde(default)]
    pub style: SpanStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cite: Option<Cite>,
}

impl Span {
    pub fn new(text: impl Into<String>, tone: Tone) -> Self {
        Span {
            text: text.into(),
            style: SpanStyle {
                tone,
                ..Default::default()
            },
            cite: None,
        }
    }

    pub fn plain(text: impl Into<String>) -> Self {
        Span::new(text, Tone::Plain)
    }

    pub fn muted(text: impl Into<String>) -> Self {
        Span::new(text, Tone::Muted)
    }

    pub fn styled(mut self, f: impl FnOnce(&mut SpanStyle)) -> Self {
        f(&mut self.style);
        self
    }

    pub fn with_cite(mut self, cite: Cite) -> Self {
        self.cite = Some(cite);
        self
    }
}

/// One rendered line: a glyph + a semantic op + styled spans.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeltaLine {
    pub op: ChangeOp,
    pub glyph: Glyph,
    pub spans: Vec<Span>,
}

impl DeltaLine {
    pub fn new(op: ChangeOp, spans: Vec<Span>) -> Self {
        DeltaLine {
            op,
            glyph: op.glyph(),
            spans,
        }
    }
}

/// The computed-once memory-state diff. Serialized form is what every
/// non-terminal surface (REST/MCP/web/replay) consumes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MemoryDelta {
    pub lines: Vec<DeltaLine>,
}

/// An inspect view of one memory: a header plus lineage/links/evolution rows.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MemoryView {
    pub header: Vec<Span>,
    pub rows: Vec<DeltaLine>,
}
