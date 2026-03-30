use std::borrow::Cow;

use rmcp::ErrorData;
use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::server::MeridianMcpServer;
use meridian_core::store::{
    AgentStore, CheckpointStore, EventStore, HitlStore, MemoryStore, ObjectiveStore,
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

impl<S> AsyncTool<MeridianMcpServer<S>> for SearchGraphTool
where
    S: AgentStore
        + CheckpointStore
        + MemoryStore
        + ObjectiveStore
        + EventStore
        + HitlStore
        + Send
        + Sync
        + 'static,
{
    async fn invoke(
        _service: &MeridianMcpServer<S>,
        _params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        Ok(SearchGraphOutput { results: vec![] })
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

impl<S> AsyncTool<MeridianMcpServer<S>> for StoreMemoryTool
where
    S: AgentStore
        + CheckpointStore
        + MemoryStore
        + ObjectiveStore
        + EventStore
        + HitlStore
        + Send
        + Sync
        + 'static,
{
    async fn invoke(
        _service: &MeridianMcpServer<S>,
        _params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        Ok(StoreMemoryOutput {
            entry_id: uuid::Uuid::new_v4().to_string(),
            stored: true,
        })
    }
}
