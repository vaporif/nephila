use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use meridian_core::embedding::EmbeddingProvider;
use meridian_core::memory::Embedding;
use meridian_core::{MeridianError, Result};
use std::sync::{Arc, Mutex};

pub struct FastEmbedder {
    model: Arc<Mutex<TextEmbedding>>,
    dimension: usize,
}

impl FastEmbedder {
    pub fn new(model_name: &str) -> Result<Self> {
        let embedding_model: EmbeddingModel =
            model_name.parse().map_err(MeridianError::Embedding)?;

        let model_info = TextEmbedding::get_model_info(&embedding_model)
            .map_err(|e| MeridianError::Embedding(e.to_string()))?;
        let dimension = model_info.dim;

        let model = TextEmbedding::try_new(InitOptions::new(embedding_model))
            .map_err(|e| MeridianError::Embedding(e.to_string()))?;

        Ok(Self {
            model: Arc::new(Mutex::new(model)),
            dimension,
        })
    }

    pub fn with_default() -> Result<Self> {
        Self::new("Xenova/bge-small-en-v1.5")
    }
}

impl EmbeddingProvider for FastEmbedder {
    async fn embed(&self, text: &str) -> Result<Embedding> {
        let model = self.model.clone();
        let text = text.to_owned();
        let mut results = tokio::task::spawn_blocking(move || {
            let model = model.lock().expect("embedding mutex poisoned");
            model
                .embed(vec![text], None)
                .map_err(|e| MeridianError::Embedding(e.to_string()))
        })
        .await
        .map_err(|e| MeridianError::Embedding(e.to_string()))??;

        results
            .pop()
            .ok_or_else(|| MeridianError::Embedding("empty embedding result".into()))
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>> {
        let model = self.model.clone();
        let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        tokio::task::spawn_blocking(move || {
            let model = model.lock().expect("embedding mutex poisoned");
            model
                .embed(texts, None)
                .map_err(|e| MeridianError::Embedding(e.to_string()))
        })
        .await
        .map_err(|e| MeridianError::Embedding(e.to_string()))?
    }

    fn dimension(&self) -> usize {
        self.dimension
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meridian_core::embedding::EmbeddingProvider;

    #[test]
    fn test_dimension() {
        let embedder = FastEmbedder::with_default().unwrap();
        assert_eq!(embedder.dimension(), 384);
    }

    #[tokio::test]
    async fn test_embed_returns_correct_dimension() {
        let embedder = FastEmbedder::with_default().unwrap();
        let result = embedder.embed("hello world").await.unwrap();
        assert_eq!(result.len(), 384);
    }

    #[tokio::test]
    async fn test_embed_batch() {
        let embedder = FastEmbedder::with_default().unwrap();
        let results = embedder.embed_batch(&["hello", "world"]).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].len(), 384);
    }

    #[tokio::test]
    async fn test_similar_texts_have_high_cosine() {
        let embedder = FastEmbedder::with_default().unwrap();
        let a = embedder
            .embed("authentication with JWT tokens")
            .await
            .unwrap();
        let b = embedder.embed("JWT token based auth").await.unwrap();
        let c = embedder.embed("cooking pasta recipes").await.unwrap();

        let sim_ab = cosine_similarity(&a, &b);
        let sim_ac = cosine_similarity(&a, &c);
        assert!(
            sim_ab > sim_ac,
            "similar texts should have higher cosine similarity"
        );
    }

    fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (norm_a * norm_b)
    }
}
