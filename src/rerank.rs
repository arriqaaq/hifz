//! In-process cross-encoder rerankers (fastembed / ONNX).
//!
//! Wraps [`fastembed::TextRerank`]. Used as an alternative to the LLM
//! rerank path in [`crate::llm_rerank`] — public cross-encoders are
//! cheaper per call (~100–500ms CPU) but tend to underperform on
//! domain-technical text because their training distributions (MS-MARCO +
//! web QA) don't cover code/config terminology. Phase 10 bench: `bge-base`
//! dropped Recall@5 from 0.944 → 0.900 on the hifz corpus. Jina variants
//! are plausible alternatives worth measuring before concluding.
//!
//! Contract: on any error the original results should be returned by the
//! caller. See [`crate::search::apply_rerank`] for the integration point.

use std::sync::Mutex;

use anyhow::Result;
use fastembed::{RerankInitOptions, RerankerModel, TextRerank};

/// Which reranker model to load. Each downloads separate ONNX + tokenizer
/// files (fastembed caches under `~/.fastembed/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RerankerChoice {
    /// `BAAI/bge-reranker-base` — ~1GB. English/Chinese. Baseline.
    BgeBase,
    /// `rozgo/bge-reranker-v2-m3` — ~2.3GB. Multilingual, larger, newer.
    BgeV2M3,
    /// `jinaai/jina-reranker-v1-turbo-en` — ~100MB. English, fast.
    JinaV1Turbo,
    /// `jinaai/jina-reranker-v2-base-multilingual` — ~600MB. Multilingual.
    JinaV2Multilingual,
}

impl RerankerChoice {
    fn model(self) -> RerankerModel {
        match self {
            RerankerChoice::BgeBase => RerankerModel::BGERerankerBase,
            RerankerChoice::BgeV2M3 => RerankerModel::BGERerankerV2M3,
            RerankerChoice::JinaV1Turbo => RerankerModel::JINARerankerV1TurboEn,
            RerankerChoice::JinaV2Multilingual => RerankerModel::JINARerankerV2BaseMultiligual,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RerankerChoice::BgeBase => "bge-base",
            RerankerChoice::BgeV2M3 => "bge-v2-m3",
            RerankerChoice::JinaV1Turbo => "jina-v1-turbo",
            RerankerChoice::JinaV2Multilingual => "jina-v2-multilingual",
        }
    }

    /// Parse a user-supplied tag (matching `as_str` output).
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "bge-base" => Some(RerankerChoice::BgeBase),
            "bge-v2-m3" => Some(RerankerChoice::BgeV2M3),
            "jina-v1-turbo" => Some(RerankerChoice::JinaV1Turbo),
            "jina-v2-multilingual" => Some(RerankerChoice::JinaV2Multilingual),
            _ => None,
        }
    }
}

pub struct Reranker {
    /// fastembed's `rerank` takes `&mut self`; wrap for shared use.
    inner: Mutex<TextRerank>,
    choice: RerankerChoice,
}

impl Reranker {
    /// Load the model (downloads on first use; cached subsequently).
    pub fn new(choice: RerankerChoice) -> Result<Self> {
        let options = RerankInitOptions::new(choice.model()).with_show_download_progress(true);
        let inner = TextRerank::try_new(options)?;
        Ok(Self {
            inner: Mutex::new(inner),
            choice,
        })
    }

    pub fn choice(&self) -> RerankerChoice {
        self.choice
    }

    /// Rerank `documents` against `query`. Returns `(original_index, score)`
    /// per document. Higher score = more relevant; absolute scale depends
    /// on model.
    pub fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<(usize, f64)>> {
        if documents.is_empty() {
            return Ok(vec![]);
        }
        // Bind `S = &str` across both `query` and `documents`.
        let docs: Vec<&str> = documents.iter().map(String::as_str).collect();
        let mut guard = self.inner.lock().expect("reranker mutex poisoned");
        let results = guard.rerank(query, docs.as_slice(), false, None)?;
        Ok(results
            .into_iter()
            .map(|r| (r.index, r.score as f64))
            .collect())
    }
}
