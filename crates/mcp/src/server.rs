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

use nephila_core::command::OrchestratorCommand;
use nephila_core::config::NephilaConfig;
use nephila_core::error::NephilaError;
use nephila_core::event::BusEvent;
use nephila_core::id::AgentId;
use nephila_store::{FerrexStore, SqliteStore};

use crate::state::HitlRequest;
use crate::tools::agent::{GetAgentStatusTool, GetEventLogTool, SpawnAgentTool};
use crate::tools::checkpoint::{GetSessionCheckpointTool, SerializeAndPersistTool};
use crate::tools::fork::ForkAgentTool;
use crate::tools::hitl::RequestHumanInputTool;
use crate::tools::lifecycle::{GetDirectiveTool, ReportTokenEstimateTool, RequestContextResetTool};
use crate::tools::memory::{SearchGraphTool, StoreMemoryTool};
use crate::tools::objective::{GetObjectiveTreeTool, UpdateObjectiveTool};

pub fn nephila_err(e: NephilaError) -> rmcp::ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

pub fn parse_agent_id(s: &str) -> Result<AgentId, rmcp::ErrorData> {
    s.parse::<uuid::Uuid>()
        .map(AgentId)
        .map_err(|e| ErrorData::invalid_params(format!("invalid agent_id: {e}"), None))
}

pub fn parse_objective_id(s: &str) -> Result<nephila_core::id::ObjectiveId, rmcp::ErrorData> {
    s.parse::<uuid::Uuid>()
        .map(nephila_core::id::ObjectiveId)
        .map_err(|e| ErrorData::invalid_params(format!("invalid objective_id: {e}"), None))
}

pub struct NephilaMcpServer {
    tool_router: ToolRouter<Self>,
    pub sqlite: Arc<SqliteStore>,
    pub ferrex: Arc<FerrexStore>,
    pub event_tx: broadcast::Sender<BusEvent>,
    pub cmd_tx: mpsc::Sender<OrchestratorCommand>,
    pub hitl_requests: Arc<RwLock<HashMap<AgentId, HitlRequest>>>,
    pub config: NephilaConfig,
}

impl NephilaMcpServer {
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
        sqlite: Arc<SqliteStore>,
        ferrex: Arc<FerrexStore>,
        event_tx: broadcast::Sender<BusEvent>,
        cmd_tx: mpsc::Sender<OrchestratorCommand>,
        hitl_requests: Arc<RwLock<HashMap<AgentId, HitlRequest>>>,
        config: NephilaConfig,
    ) -> Self {
        Self {
            tool_router: Self::build_tool_router(),
            sqlite,
            ferrex,
            event_tx,
            cmd_tx,
            hitl_requests,
            config,
        }
    }
}

impl ServerHandler for NephilaMcpServer {
    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .build();

        ServerInfo::new(capabilities)
            .with_server_info(Implementation::new(
                "nephila-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Nephila lifecycle-aware MCP server. Tools are filtered by agent phase.",
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
