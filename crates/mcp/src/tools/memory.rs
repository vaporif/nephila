use std::borrow::Cow;

use rmcp::ErrorData;
use rmcp::handler::server::router::tool::{AsyncTool, ToolBase};
use rmcp::schemars;
use serde::{Deserialize, Serialize};

use crate::server::{NephilaMcpServer, nephila_err, parse_agent_id};
use nephila_core::store::MemoryStore;

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
    pub memory_type: String,
    pub staleness: String,
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

impl AsyncTool<NephilaMcpServer> for SearchGraphTool {
    async fn invoke(
        service: &NephilaMcpServer,
        params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        let request = ferrex_core::RecallRequest {
            query: params.query,
            types: None,
            entities: None,
            namespace: None,
            limit: Some(params.limit),
            include_stale: Some(true),
            include_invalidated: Some(false),
            time_range: None,
            validate_ids: None,
            explain: false,
        };

        let results = service.ferrex.recall(request).await.map_err(nephila_err)?;

        let output_results = results
            .into_iter()
            .map(|r| SearchGraphResult {
                entry_id: r.memory.id.clone(),
                content: r.memory.searchable_text(),
                score: r.relevance_score,
                tags: r.memory.entities.clone(),
                memory_type: r.memory.memory_type.to_string(),
                staleness: format!("{:?}", r.freshness_label).to_lowercase(),
            })
            .collect();

        Ok(SearchGraphOutput {
            results: output_results,
        })
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct StoreMemoryParams {
    /// The agent ID storing the memory.
    pub agent_id: String,
    /// The text content to memorize (for episodic/procedural).
    pub content: Option<String>,
    /// Optional tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Entity names to link to this memory.
    #[serde(default)]
    pub entities: Vec<String>,
    /// Memory type: episodic, semantic, or procedural.
    pub memory_type: Option<String>,
    /// Subject (for semantic triples).
    pub subject: Option<String>,
    /// Predicate (for semantic triples).
    pub predicate: Option<String>,
    /// Object (for semantic triples).
    pub object: Option<String>,
    /// Confidence score (0.0 to 1.0).
    #[serde(default = "default_confidence")]
    pub confidence: f64,
}

fn default_confidence() -> f64 {
    1.0
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct StoreMemoryOutput {
    pub entry_id: String,
    pub stored: bool,
    pub memory_type: Option<String>,
    pub superseded: Vec<String>,
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
        Some("Store a new memory entry with typed memory support.".into())
    }
}

impl AsyncTool<NephilaMcpServer> for StoreMemoryTool {
    async fn invoke(
        service: &NephilaMcpServer,
        params: Self::Parameter,
    ) -> Result<Self::Output, Self::Error> {
        let agent_id = parse_agent_id(&params.agent_id)?;

        let memory_type = params
            .memory_type
            .as_deref()
            .map(|t| {
                t.parse::<ferrex_core::MemoryType>()
                    .map_err(|e| ErrorData::invalid_params(e, None))
            })
            .transpose()?;

        let request = ferrex_core::StoreRequest {
            content: params.content,
            memory_type,
            subject: params.subject,
            predicate: params.predicate,
            object: params.object,
            confidence: Some(params.confidence),
            source: Some(format!("agent:{agent_id}")),
            context: None,
            entities: params.entities,
            namespace: Some(agent_id.0.to_string()),
            supersedes: None,
        };

        let response = service.ferrex.store(request).await.map_err(nephila_err)?;

        Ok(StoreMemoryOutput {
            entry_id: response.id,
            stored: true,
            memory_type: Some(response.memory_type),
            superseded: response.superseded,
        })
    }
}
