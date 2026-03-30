mod service;

use clap::Parser;
use color_eyre::eyre::{Result, WrapErr};
use meridian_core::agent::AgentCommand;
use meridian_core::config::MeridianConfig;
use meridian_core::event::BusEvent;
use meridian_store::SqliteStore;
use meridian_tui::tui_command::TuiCommand;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
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
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let tui_log = meridian_tui::tui_tracing::TuiLogBuffer::new();
    let tui_layer = meridian_tui::tui_tracing::TuiTracingLayer::new(tui_log.clone());

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "meridian=debug".into()),
        )
        .with(tui_layer)
        .init();

    let cli = Cli::parse();

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
    let (cmd_tx, cmd_rx) = mpsc::channel::<TuiCommand>(256);

    let _mcp_server = meridian_mcp::server::MeridianMcpServer::new(
        store.clone(),
        event_tx.clone(),
        config.clone(),
    );

    tracing::info!("Meridian initialized, starting TUI...");

    let cmd_handle = tokio::spawn(run_command_handler(
        cmd_rx,
        event_tx.clone(),
        store.clone(),
        config.clone(),
    ));

    let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut terminal = ratatui::init();
    let mut app = meridian_tui::App::new(event_rx, cmd_tx, working_dir, tui_log);
    let tui_result = app.run(&mut terminal).await;
    ratatui::restore();

    let _ = event_tx.send(BusEvent::Shutdown);
    cmd_handle.abort();

    tui_result?;
    Ok(())
}

async fn run_command_handler(
    mut cmd_rx: mpsc::Receiver<TuiCommand>,
    event_tx: broadcast::Sender<BusEvent>,
    store: Arc<SqliteStore>,
    _config: MeridianConfig,
) {
    let mut service = match service::AgentService::load(store, event_tx).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(%e, "failed to load agent service");
            return;
        }
    };

    while let Some(cmd) = cmd_rx.recv().await {
        tracing::debug!(?cmd, "command handler received");

        let result = match cmd {
            TuiCommand::Spawn {
                objective_id,
                content,
                dir,
            } => service
                .spawn(objective_id, content, dir, None)
                .await
                .map(|_| ()),
            TuiCommand::Kill { agent_id } => service.dispatch(agent_id, AgentCommand::Kill).await,
            TuiCommand::Pause { agent_id } => service.dispatch(agent_id, AgentCommand::Pause).await,
            TuiCommand::Resume { agent_id } => {
                service.dispatch(agent_id, AgentCommand::Resume).await
            }
            TuiCommand::Rollback { agent_id, version } => {
                service
                    .dispatch(agent_id, AgentCommand::Rollback { version })
                    .await
            }
            TuiCommand::ListCheckpoints { agent_id } => {
                service.list_checkpoints(agent_id).await;
                Ok(())
            }
            TuiCommand::HitlRespond { agent_id, response } => {
                service.hitl_respond(agent_id, response);
                Ok(())
            }
        };

        if let Err(e) = result {
            tracing::error!(%e, "command handler error");
        }
    }

    tracing::debug!("command handler exiting");
}
