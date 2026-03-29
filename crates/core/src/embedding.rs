use crate::error::Result;
use crate::memory::Embedding;

pub trait EmbeddingProvider: Send + Sync {
    fn embed(&self, text: &str) -> impl std::future::Future<Output = Result<Embedding>> + Send;

    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> impl std::future::Future<Output = Result<Vec<Embedding>>> + Send;

    fn dimension(&self) -> usize;
}
