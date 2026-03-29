use clap::Parser;
use color_eyre::eyre::{Result, WrapErr};
use meridian_core::config::MeridianConfig;
use meridian_core::event::BusEvent;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Parser)]
#[command(name = "meridian", about = "Agent orchestration with persistent context")]
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

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

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
    let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel::<meridian_tui::tui_command::TuiCommand>(256);

    let _mcp_server = meridian_mcp::server::MeridianMcpServer::new(
        store.clone(),
        event_tx.clone(),
        config.clone(),
    );

    tracing::info!("Meridian initialized, starting TUI...");

    let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut terminal = ratatui::init();
    let mut app = meridian_tui::App::new(event_rx, cmd_tx, working_dir, tui_log);
    let tui_result = app.run(&mut terminal).await;
    ratatui::restore();

    let _ = event_tx.send(BusEvent::Shutdown);

    tui_result?;
    Ok(())
}
