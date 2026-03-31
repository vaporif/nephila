use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("index error: {0}")]
    Index(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMetadata {
    pub content: String,
    pub tags: Vec<String>,
    pub aggregate_id: Option<String>,
    pub extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchFilter {
    pub tags: Option<Vec<String>>,
    pub aggregate_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorSearchResult {
    pub id: String,
    pub score: f32,
    pub metadata: SearchMetadata,
}

#[derive(Debug, Clone)]
pub struct SearchEntry {
    pub id: String,
    pub embedding: Vec<f32>,
    pub metadata: SearchMetadata,
}

pub trait SearchProvider: Send + Sync {
    fn index(
        &self,
        id: &str,
        embedding: Vec<f32>,
        metadata: SearchMetadata,
    ) -> impl std::future::Future<Output = Result<(), SearchError>> + Send;

    fn remove(&self, id: &str)
    -> impl std::future::Future<Output = Result<(), SearchError>> + Send;

    fn search(
        &self,
        query_embedding: Vec<f32>,
        limit: usize,
        filter: Option<SearchFilter>,
    ) -> impl std::future::Future<Output = Result<Vec<VectorSearchResult>, SearchError>> + Send;

    fn rebuild(
        &self,
        entries: Vec<SearchEntry>,
    ) -> impl std::future::Future<Output = Result<(), SearchError>> + Send;
}
