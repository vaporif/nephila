use std::borrow::Cow;

use rmcp::ErrorData;
use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::server::{MeridianMcpServer, meridian_err, parse_agent_id};
use meridian_core::embedding::EmbeddingProvider;
use meridian_core::id::EntryId;
use meridian_core::memory::{LifecycleState, Link, MemoryEntry};
use meridian_core::store::{
    AgentStore, CheckpointStore, HitlStore, McpEventLog, MemoryStore, ObjectiveStore,
};

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct SearchGraphParams {
    /// Natural-language query text to search the memory graph.
    pub query: String,
    /// Maximum number of results to return.
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    5
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SearchGraphOutput {
    pub results: Vec<SearchGraphResult>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct SearchGraphResult {
    pub entry_id: String,
    pub content: String,
    pub score: f32,
    pub tags: Vec<String>,
}

pub struct SearchGraphTool;

impl ToolBase for SearchGraphTool {
    type Parameter = SearchGraphParams;
    type Output = SearchGraphOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "search_graph".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some("Find memories related to a natural-language query.".into())
    }
}

impl<S, E> AsyncTool<MeridianMcpServer<S, E>> for SearchGraphTool
where
    S: AgentStore
        + CheckpointStore
        + MemoryStore
        + ObjectiveStore
        + McpEventLog
        + HitlStore
        + Send
        + Sync
        + 'static,
    E: EmbeddingProvider + 'static,
{
    async fn invoke(
        service: &MeridianMcpServer<S, E>,
        params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        let embedding = service
            .embedder
            .embed(&params.query)
            .await
            .map_err(|e| ErrorData::internal_error(format!("embedding failed: {e}"), None))?;

        let search_results = service
            .store
            .search(&embedding, params.limit)
            .await
            .map_err(meridian_err)?;

        let mut results = Vec::with_capacity(search_results.len());
        for sr in search_results {
            if let Err(e) = service.store.increment_access(sr.entry.id).await {
                tracing::warn!(entry_id = %sr.entry.id, %e, "failed to increment access count");
            }
            results.push(SearchGraphResult {
                entry_id: sr.entry.id.0.to_string(),
                content: sr.entry.content,
                score: sr.score,
                tags: sr.entry.tags,
            });
        }

        Ok(SearchGraphOutput { results })
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct StoreMemoryParams {
    /// The agent ID storing the memory.
    pub agent_id: String,
    /// The text content to memorize.
    pub content: String,
    /// Optional tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Importance score (0.0 to 1.0).
    #[serde(default = "default_importance")]
    pub importance: f32,
}

fn default_importance() -> f32 {
    0.5
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct StoreMemoryOutput {
    pub entry_id: String,
    pub stored: bool,
}

pub struct StoreMemoryTool;

impl ToolBase for StoreMemoryTool {
    type Parameter = StoreMemoryParams;
    type Output = StoreMemoryOutput;
    type Error = ErrorData;

    fn name() -> Cow<'static, str> {
        "store_memory".into()
    }

    fn description() -> Option<Cow<'static, str>> {
        Some("Store a new memory entry, embedding it and linking it to related entries.".into())
    }
}

impl<S, E> AsyncTool<MeridianMcpServer<S, E>> for StoreMemoryTool
where
    S: AgentStore
        + CheckpointStore
        + MemoryStore
        + ObjectiveStore
        + McpEventLog
        + HitlStore
        + Send
        + Sync
        + 'static,
    E: EmbeddingProvider + 'static,
{
    async fn invoke(
        service: &MeridianMcpServer<S, E>,
        params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        let agent_id = parse_agent_id(&params.agent_id)?;

        let embedding = service
            .embedder
            .embed(&params.content)
            .await
            .map_err(|e| ErrorData::internal_error(format!("embedding failed: {e}"), None))?;

        let entry = MemoryEntry {
            id: EntryId::new(),
            agent_id,
            content: params.content,
            embedding: embedding.clone(),
            tags: params.tags,
            lifecycle_state: LifecycleState::Active,
            importance: params.importance.clamp(0.0, 1.0),
            access_count: 0,
            created_at: chrono::Utc::now(),
        };

        let entry_id = service.store.store(entry).await.map_err(meridian_err)?;

        let similar = service
            .store
            .find_similar(&embedding, 0.8)
            .await
            .map_err(meridian_err)?;
        let links: Vec<Link> = similar
            .into_iter()
            .filter(|(id, _)| *id != entry_id)
            .map(|(target_id, similarity_score)| Link {
                target_id,
                similarity_score,
            })
            .collect();
        if !links.is_empty() {
            service
                .store
                .update_links(entry_id, links)
                .await
                .map_err(meridian_err)?;
        }

        Ok(StoreMemoryOutput {
            entry_id: entry_id.0.to_string(),
            stored: true,
        })
    }
}
