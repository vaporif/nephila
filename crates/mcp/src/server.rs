use std::collections::HashMap;
use std::sync::Arc;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::ErrorData;
use rmcp::ServerHandler;
use tokio::sync::{RwLock, broadcast};

use meridian_core::config::MeridianConfig;
use meridian_core::event::BusEvent;
use meridian_core::id::AgentId;
use meridian_core::store::{AgentStore, CheckpointStore, EventStore, HitlStore, MemoryStore, ObjectiveStore};

use crate::state::HitlRequest;
use crate::tools::checkpoint::{GetSessionCheckpointTool, SerializeAndPersistTool};
use crate::tools::hitl::RequestHumanInputTool;
use crate::tools::lifecycle::{GetDirectiveTool, ReportTokenEstimateTool, RequestContextResetTool};
use crate::tools::memory::{SearchGraphTool, StoreMemoryTool};
use crate::tools::objective::{GetObjectiveTreeTool, UpdateObjectiveTool};

pub struct MeridianMcpServer<S> {
    tool_router: ToolRouter<Self>,
    pub store: Arc<S>,
    pub event_tx: broadcast::Sender<BusEvent>,
    pub hitl_requests: Arc<RwLock<HashMap<AgentId, HitlRequest>>>,
    pub config: MeridianConfig,
}

impl<S> MeridianMcpServer<S>
where
    S: AgentStore + CheckpointStore + MemoryStore + ObjectiveStore + EventStore + HitlStore + Send + Sync + 'static,
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
    }

    pub fn new(
        store: Arc<S>,
        event_tx: broadcast::Sender<BusEvent>,
        config: MeridianConfig,
    ) -> Self {
        Self {
            tool_router: Self::build_tool_router(),
            store,
            event_tx,
            hitl_requests: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }
}

impl<S> ServerHandler for MeridianMcpServer<S>
where
    S: AgentStore + CheckpointStore + MemoryStore + ObjectiveStore + EventStore + HitlStore + Send + Sync + 'static,
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
        let tcc =
            rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
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
