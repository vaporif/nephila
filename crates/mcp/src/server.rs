use std::collections::HashMap;
use std::sync::Arc;

use rmcp::ErrorData;
use rmcp::ServerHandler;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Implementation, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use tokio::sync::{RwLock, broadcast, mpsc};

use meridian_core::command::OrchestratorCommand;
use meridian_core::config::MeridianConfig;
use meridian_core::embedding::EmbeddingProvider;
use meridian_core::error::MeridianError;
use meridian_core::event::BusEvent;
use meridian_core::id::AgentId;
use meridian_core::store::{
    AgentStore, CheckpointStore, InterruptStore, McpEventLog, MemoryStore, ObjectiveStore,
};

use crate::state::HitlRequest;
use crate::tools::agent::{GetAgentStatusTool, GetEventLogTool, SpawnAgentTool};
use crate::tools::checkpoint::{GetSessionCheckpointTool, SerializeAndPersistTool};
use crate::tools::fork::ForkAgentTool;
use crate::tools::hitl::RequestHumanInputTool;
use crate::tools::lifecycle::{GetDirectiveTool, ReportTokenEstimateTool, RequestContextResetTool};
use crate::tools::memory::{SearchGraphTool, StoreMemoryTool};
use crate::tools::objective::{GetObjectiveTreeTool, UpdateObjectiveTool};

pub fn meridian_err(e: MeridianError) -> rmcp::ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

pub fn parse_agent_id(s: &str) -> Result<AgentId, rmcp::ErrorData> {
    s.parse::<uuid::Uuid>()
        .map(AgentId)
        .map_err(|e| ErrorData::invalid_params(format!("invalid agent_id: {e}"), None))
}

pub fn parse_objective_id(s: &str) -> Result<meridian_core::id::ObjectiveId, rmcp::ErrorData> {
    s.parse::<uuid::Uuid>()
        .map(meridian_core::id::ObjectiveId)
        .map_err(|e| ErrorData::invalid_params(format!("invalid objective_id: {e}"), None))
}

pub struct MeridianMcpServer<S, E> {
    tool_router: ToolRouter<Self>,
    pub store: Arc<S>,
    pub embedder: Arc<E>,
    pub event_tx: broadcast::Sender<BusEvent>,
    pub cmd_tx: mpsc::Sender<OrchestratorCommand>,
    pub hitl_requests: Arc<RwLock<HashMap<AgentId, HitlRequest>>>,
    pub config: MeridianConfig,
}

impl<S, E> MeridianMcpServer<S, E>
where
    S: AgentStore
        + CheckpointStore
        + MemoryStore
        + ObjectiveStore
        + McpEventLog
        + InterruptStore
        + Send
        + Sync
        + 'static,
    E: EmbeddingProvider + 'static,
{
    fn build_tool_router() -> ToolRouter<Self> {
        ToolRouter::new()
            .with_async_tool::<GetSessionCheckpointTool>()
            .with_async_tool::<SerializeAndPersistTool>()
            .with_async_tool::<ReportTokenEstimateTool>()
            .with_async_tool::<GetDirectiveTool>()
            .with_async_tool::<RequestContextResetTool>()
            .with_async_tool::<SearchGraphTool>()
            .with_async_tool::<StoreMemoryTool>()
            .with_async_tool::<GetObjectiveTreeTool>()
            .with_async_tool::<UpdateObjectiveTool>()
            .with_async_tool::<RequestHumanInputTool>()
            .with_async_tool::<SpawnAgentTool>()
            .with_async_tool::<GetAgentStatusTool>()
            .with_async_tool::<GetEventLogTool>()
            .with_async_tool::<ForkAgentTool>()
    }

    pub fn new(
        store: Arc<S>,
        embedder: Arc<E>,
        event_tx: broadcast::Sender<BusEvent>,
        cmd_tx: mpsc::Sender<OrchestratorCommand>,
        hitl_requests: Arc<RwLock<HashMap<AgentId, HitlRequest>>>,
        config: MeridianConfig,
    ) -> Self {
        Self {
            tool_router: Self::build_tool_router(),
            store,
            embedder,
            event_tx,
            cmd_tx,
            hitl_requests,
            config,
        }
    }
}

impl<S, E> ServerHandler for MeridianMcpServer<S, E>
where
    S: AgentStore
        + CheckpointStore
        + MemoryStore
        + ObjectiveStore
        + McpEventLog
        + InterruptStore
        + Send
        + Sync
        + 'static,
    E: EmbeddingProvider + 'static,
{
    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .build();

        ServerInfo::new(capabilities)
            .with_server_info(Implementation::new(
                "meridian-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Meridian lifecycle-aware MCP server. Tools are filtered by agent phase.",
            )
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: self.tool_router.list_all(),
            meta: None,
            next_cursor: None,
        })
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        self.tool_router.get(name).cloned()
    }
}
