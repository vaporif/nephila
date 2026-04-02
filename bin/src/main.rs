mod orchestrator;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use color_eyre::eyre::{Result, WrapErr};
use meridian_core::command::OrchestratorCommand;
use meridian_core::config::MeridianConfig;
use meridian_core::event::BusEvent;
use meridian_core::id::AgentId;
use meridian_mcp::state::HitlRequest;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Parser)]
#[command(
    name = "meridian",
    about = "Agent orchestration with persistent context"
)]
struct Cli {
    /// Config file path
    #[arg(short, long, default_value = "meridian.toml")]
    config: PathBuf,

    /// Override SQLite database path
    #[arg(long)]
    db: Option<PathBuf>,

    /// Run without TUI (daemon mode)
    #[arg(long)]
    headless: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();

    let tui_log = if cli.headless {
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "meridian=debug".into()),
            )
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .init();
        None
    } else {
        let tui_log = meridian_tui::tui_tracing::TuiLogBuffer::new();
        let tui_layer = meridian_tui::tui_tracing::TuiTracingLayer::new(tui_log.clone());
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "meridian=debug".into()),
            )
            .with(tui_layer)
            .init();
        Some(tui_log)
    };

    let config_str = std::fs::read_to_string(&cli.config).unwrap_or_else(|_| {
        tracing::warn!("Config file not found, using defaults");
        String::new()
    });
    let mut config: MeridianConfig = if config_str.is_empty() {
        MeridianConfig::default()
    } else {
        toml::from_str(&config_str).wrap_err("failed to parse config")?
    };

    if let Some(db) = cli.db {
        config.meridian.sqlite_path = db;
    }

    let embedder = Arc::new(
        meridian_embedding::FastEmbedder::new(&config.meridian.embedding_model)
            .wrap_err("failed to initialize embedding model")?,
    );

    let store = Arc::new(
        meridian_store::SqliteStore::open(
            &config.meridian.sqlite_path,
            meridian_core::embedding::EmbeddingProvider::dimension(embedder.as_ref()),
        )
        .wrap_err("failed to open database")?,
    );

    let (event_tx, event_rx) = broadcast::channel::<BusEvent>(1024);
    let (cmd_tx, cmd_rx) = mpsc::channel::<OrchestratorCommand>(256);

    let hitl_requests: Arc<RwLock<HashMap<AgentId, HitlRequest>>> =
        Arc::new(RwLock::new(HashMap::new()));

    let cancellation_token = CancellationToken::new();

    let (mcp_handle, bound_addr) = meridian_mcp::http::serve(
        store.clone(),
        embedder.clone(),
        event_tx.clone(),
        cmd_tx.clone(),
        hitl_requests.clone(),
        config.clone(),
        cancellation_token.clone(),
    )
    .await
    .wrap_err("failed to start MCP HTTP server")?;

    tracing::info!(%bound_addr, "MCP server listening");

    let orch_store = store.clone();
    let orch_event_tx = event_tx.clone();
    let orch_hitl = hitl_requests.clone();
    let max_agent_depth = config.supervision.max_agent_depth;
    let connector_config = config.connector.clone();
    let cmd_handle = tokio::spawn(async move {
        match orchestrator::Orchestrator::load(
            orch_store,
            orch_event_tx,
            orch_hitl,
            max_agent_depth,
            connector_config,
        )
        .await
        {
            Ok(mut orch) => orch.run(cmd_rx).await,
            Err(e) => tracing::error!(%e, "failed to load orchestrator"),
        }
    });

    if cli.headless {
        tracing::info!("Running in headless mode, press Ctrl+C to stop");
        tokio::signal::ctrl_c()
            .await
            .wrap_err("failed to listen for Ctrl+C")?;
    } else {
        let tui_log = tui_log.expect("tui_log must be Some in TUI mode");
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut terminal = ratatui::init();
        let mut app = meridian_tui::App::new(
            event_rx,
            cmd_tx,
            working_dir,
            tui_log,
            config.connector.claude_binary.clone(),
        );
        let tui_result = app.run(&mut terminal).await;
        ratatui::restore();
        tui_result?;
    }

    tracing::info!("Shutting down...");
    let _ = event_tx.send(BusEvent::Shutdown);
    cancellation_token.cancel();
    cmd_handle.abort();
    let _ = mcp_handle.await;

    Ok(())
}
