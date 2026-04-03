use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use meridian_core::command::OrchestratorCommand;
use meridian_core::config::MeridianConfig;
use meridian_core::embedding::EmbeddingProvider;
use meridian_core::event::BusEvent;
use meridian_core::id::AgentId;
use meridian_core::store::{
    AgentStore, CheckpointStore, InterruptStore, McpEventLog, MemoryStore, ObjectiveStore,
};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager,
    tower::{StreamableHttpServerConfig, StreamableHttpService},
};

use crate::server::MeridianMcpServer;
use crate::state::HitlRequest;

pub async fn serve<S, E>(
    store: Arc<S>,
    embedder: Arc<E>,
    event_tx: broadcast::Sender<BusEvent>,
    cmd_tx: mpsc::Sender<OrchestratorCommand>,
    hitl_requests: Arc<RwLock<HashMap<AgentId, HitlRequest>>>,
    config: MeridianConfig,
    cancellation_token: CancellationToken,
) -> std::io::Result<(JoinHandle<()>, SocketAddr)>
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
    let ip: IpAddr = config
        .mcp
        .host
        .parse()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let bind_addr = SocketAddr::new(ip, config.mcp.port);

    let http_config =
        StreamableHttpServerConfig::default().with_cancellation_token(cancellation_token.clone());

    let session_manager = Arc::new(LocalSessionManager::default());

    let service = StreamableHttpService::new(
        move || {
            Ok(MeridianMcpServer::new(
                store.clone(),
                embedder.clone(),
                event_tx.clone(),
                cmd_tx.clone(),
                hitl_requests.clone(),
                config.clone(),
            ))
        },
        session_manager,
        http_config,
    );

    let router = Router::new().nest_service("/mcp", service);

    let listener = TcpListener::bind(bind_addr).await?;
    let bound_addr = listener.local_addr()?;

    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router)
            .with_graceful_shutdown(cancellation_token.cancelled_owned())
            .await
        {
            tracing::error!(%e, "MCP HTTP server error");
        }
    });

    Ok((handle, bound_addr))
}
