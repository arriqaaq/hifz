use anyhow::Result;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use std::sync::Mutex;

const BATCH_SIZE: usize = 64;

pub struct Embedder {
    model: Mutex<TextEmbedding>,
    dim: usize,
}

impl Embedder {
    pub fn new() -> Result<Self> {
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::AllMiniLML6V2).with_show_download_progress(true),
        )?;
        Ok(Self {
            model: Mutex::new(model),
            dim: 384,
        })
    }

    pub fn dimension(&self) -> usize {
        self.dim
    }

    /// Embed passages (for indexing). No prefix needed for MiniLM.
    pub fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut model = self.model.lock().unwrap();
        let embeddings = model.embed(texts, None)?;
        Ok(embeddings)
    }

    /// Embed a single query (for searching). No prefix needed for MiniLM.
    pub fn embed_single(&self, text: &str) -> Result<Vec<f32>> {
        let mut model = self.model.lock().unwrap();
        let mut embeddings = model.embed(vec![text], None)?;
        Ok(embeddings.remove(0))
    }

    /// Embed passages in batches.
    pub fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut all = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(BATCH_SIZE) {
            let refs: Vec<&str> = chunk.iter().map(|s| s.as_str()).collect();
            let batch = self.embed(&refs)?;
            all.extend(batch);
        }
        Ok(all)
    }
}
