// SPDX-License-Identifier: Apache-2.0
//! Independent semantic memory-state renderer for hifz.
//!
//! Pattern: hifz's mutation paths emit a `Vec<Change>`; `compute` turns that
//! into one structured [`MemoryDelta`] (computed once); sinks render that
//! same value to terminal text, JSON, or — via the serialized form — the
//! web UI. No surrealdb / kernel / hifz types leak in here (record ids are
//! plain strings), so this crate stays a pure presentation layer.
//!
//! Clean-room: the design pattern is shared with another project but no code,
//! comments, or type definitions were copied; names and logic are original.

pub mod compute;
pub mod model;
pub mod replay;
pub mod sink_json;
pub mod sink_text;
pub mod theme;

pub use compute::{delta_from_changes, view_of};
pub use model::{
    Change, ChangeOp, Cite, DeltaLine, Glyph, MemoryDelta, MemoryView, Span, SpanStyle, Tone,
};
pub use replay::SessionEvent;
