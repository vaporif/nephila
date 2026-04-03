mod orchestrator;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use color_eyre::eyre::{Result, WrapErr};
use nephila_core::command::OrchestratorCommand;
use nephila_core::config::NephilaConfig;
use nephila_core::event::BusEvent;
use nephila_core::id::AgentId;
use nephila_mcp::state::HitlRequest;
use tokio::sync::{RwLock, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[derive(Parser)]
#[command(
    name = "nephila",
    about = "Agent orchestration with persistent context"
)]
struct Cli {
    /// Config file path
    #[arg(short, long, default_value = "nephila.toml")]
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

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "nephila=debug".into());

    let tui_log = if cli.headless {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
            .init();
        None
    } else {
        let tui_log = nephila_tui::tui_tracing::TuiLogBuffer::new();
        let tui_layer = nephila_tui::tui_tracing::TuiTracingLayer::new(tui_log.clone());
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tui_layer)
            .init();
        Some(tui_log)
    };

    let config_str = std::fs::read_to_string(&cli.config).unwrap_or_else(|_| {
        tracing::warn!("Config file not found, using defaults");
        String::new()
    });
    let mut config: NephilaConfig = if config_str.is_empty() {
        NephilaConfig::default()
    } else {
        toml::from_str(&config_str).wrap_err("failed to parse config")?
    };

    if let Some(db) = cli.db {
        config.nephila.sqlite_path = db;
    }

    let embedder = Arc::new(
        nephila_embedding::FastEmbedder::new(&config.nephila.embedding_model)
            .wrap_err("failed to initialize embedding model")?,
    );

    let store = Arc::new(
        nephila_store::SqliteStore::open(
            &config.nephila.sqlite_path,
            nephila_core::embedding::EmbeddingProvider::dimension(embedder.as_ref()),
        )
        .wrap_err("failed to open database")?,
    );

    let (event_tx, event_rx) = broadcast::channel::<BusEvent>(1024);
    let (cmd_tx, cmd_rx) = mpsc::channel::<OrchestratorCommand>(256);

    let hitl_requests: Arc<RwLock<HashMap<AgentId, HitlRequest>>> =
        Arc::new(RwLock::new(HashMap::new()));

    let cancellation_token = CancellationToken::new();

    let (mcp_handle, bound_addr) = nephila_mcp::http::serve(
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

    let cmd_handle = {
        let store = store.clone();
        let event_tx = event_tx.clone();
        let hitl_requests = hitl_requests.clone();
        let max_agent_depth = config.supervision.max_agent_depth;
        let connector_config = config.connector.clone();
        tokio::spawn(async move {
            match orchestrator::Orchestrator::load(
                store,
                event_tx,
                hitl_requests,
                max_agent_depth,
                connector_config,
            )
            .await
            {
                Ok(mut orch) => orch.run(cmd_rx).await,
                Err(e) => tracing::error!(%e, "failed to load orchestrator"),
            }
        })
    };

    if cli.headless {
        tracing::info!("Running in headless mode, press Ctrl+C to stop");
        tokio::signal::ctrl_c()
            .await
            .wrap_err("failed to listen for Ctrl+C")?;
    } else {
        let tui_log = tui_log.expect("tui_log must be Some in TUI mode");
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut terminal = ratatui::init();
        let mut app = nephila_tui::App::new(
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
