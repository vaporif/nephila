mod orchestrator;
mod session_registry;

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

    // Slice 4 (Task 6 step 4): cross-process lockfile. Held for the lifetime
    // of `main`; second nephila against the same workdir errors out with the
    // lock acquisition failure surfaced in stderr.
    let workdir = std::env::current_dir().wrap_err("read current dir for workdir lock")?;
    let _workdir_lock = nephila_store::lockfile::WorkdirLock::acquire(&workdir)
        .map_err(|e| color_eyre::eyre::eyre!("another nephila is already running here: {e}"))?;

    let ferrex_config = build_ferrex_config(&config)?;
    let memory_service = ferrex_core::MemoryService::from_config(ferrex_config)
        .await
        .map_err(|e| color_eyre::eyre::eyre!("ferrex init: {e}"))?;

    let sqlite_store = Arc::new(
        nephila_store::SqliteStore::open(
            &config.nephila.sqlite_path,
            memory_service.embedder().dimension(),
        )
        .wrap_err("failed to open database")?,
    );

    let ferrex_store = Arc::new(
        nephila_store::FerrexStore::new(
            memory_service,
            (*sqlite_store).clone(),
            config.nephila.l2_collection.clone(),
            "nephila",
        )
        .await
        .map_err(|e| color_eyre::eyre::eyre!("ferrex store init: {e}"))?,
    );

    let (event_tx, event_rx) = broadcast::channel::<BusEvent>(1024);
    let (cmd_tx, cmd_rx) = mpsc::channel::<OrchestratorCommand>(256);

    let hitl_requests: Arc<RwLock<HashMap<AgentId, HitlRequest>>> =
        Arc::new(RwLock::new(HashMap::new()));

    let cancellation_token = CancellationToken::new();

    let (mcp_handle, bound_addr) = nephila_mcp::http::serve(
        sqlite_store.clone(),
        ferrex_store.clone(),
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
        let store = sqlite_store.clone();
        let event_tx = event_tx.clone();
        let hitl_requests = hitl_requests.clone();
        let max_agent_depth = config.supervision.max_agent_depth;
        let connector_config = config.connector.clone();
        let mcp_endpoint = format!("http://{bound_addr}/mcp");
        let cmd_tx_clone = cmd_tx.clone();
        tokio::spawn(async move {
            match orchestrator::Orchestrator::load(
                store,
                event_tx,
                hitl_requests,
                max_agent_depth,
                connector_config,
                mcp_endpoint,
                cmd_tx_clone,
            )
            .await
            {
                Ok(mut orch) => orch.run(cmd_rx).await,
                Err(e) => tracing::error!(%e, "failed to load orchestrator"),
            }
        })
    };

    let lifecycle_handle = {
        let event_rx = event_tx.subscribe();
        let cmd_tx = cmd_tx.clone();
        let store = sqlite_store.clone();
        let lifecycle_config = config.lifecycle;
        let supervision_config = config.supervision.clone();
        tokio::spawn(async move {
            let mut supervisor = nephila_lifecycle::LifecycleSupervisor::new(
                event_rx,
                cmd_tx,
                store,
                lifecycle_config,
                supervision_config,
            );
            supervisor.run().await;
        })
    };

    // Slice 4: stand up the new session-event-driven supervisor alongside the
    // legacy `BusEvent`-driven one. The registry now owns sessions, watches
    // for crashes via `subscribe_after`, and respawns through
    // `ClaudeCodeSession::resume`.
    let blob_reader = Arc::new(nephila_store::blob::SqliteBlobReader::new(
        sqlite_store.read_pool(),
    ));
    let registry_defaults = session_registry::RegistryDefaults {
        claude_binary: PathBuf::from(&config.connector.claude_binary),
        mcp_endpoint: format!("http://{bound_addr}/mcp"),
        permission_mode: "bypassPermissions".into(),
    };
    let session_registry = Arc::new(session_registry::SessionRegistry::new(
        sqlite_store.clone(),
        blob_reader,
        registry_defaults,
    ));
    // Resume any agents in an active phase from a previous orchestrator run.
    if let Err(e) = session_registry.on_startup().await {
        tracing::warn!(%e, "SessionRegistry::on_startup failed");
    }
    // Wire the connector's crash-fallback channel into `on_crash`.
    let _crash_fallback_handle = session_registry
        .clone()
        .start_crash_fallback_listener()
        .await;
    let session_supervisor = Arc::new(tokio::sync::Mutex::new(
        nephila_lifecycle::SessionSupervisor::new(sqlite_store.clone(), config.supervision.clone()),
    ));
    let session_supervisor_handle = {
        let registry = session_registry.clone();
        let _supervisor = session_supervisor.clone();
        tokio::spawn(async move {
            let mut new_agents_rx = registry.subscribe_session_started();
            loop {
                match new_agents_rx.recv().await {
                    Ok(agent_id) => {
                        // DEFERRED to slice 4 (Task 6 step 5a): wire JoinSet +
                        // run_per_session. The agent_id → session_id lookup
                        // requires the AgentSessionAssigned event, which slice 4
                        // introduces. For now the SessionSupervisor is parked —
                        // the legacy LifecycleSupervisor handles all session
                        // traffic until the slice-4 cutover.
                        tracing::debug!(%agent_id, "SessionSupervisor wiring deferred to slice 4");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(%n, "SessionRegistry lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
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
    lifecycle_handle.abort();
    session_supervisor_handle.abort();
    let _ = mcp_handle.await;

    Ok(())
}

const FERREX_BASELINE: &str = include_str!("../../config/ferrex.toml");

fn build_ferrex_config(config: &NephilaConfig) -> Result<ferrex_core::FerrexConfig> {
    let ferrex_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".ferrex");

    let config_path = config
        .nephila
        .ferrex_config_path
        .clone()
        .unwrap_or_else(|| ferrex_dir.join("ferrex.toml"));

    let loaded = ferrex_core::load_or_init(&config_path, FERREX_BASELINE)
        .map_err(|e| color_eyre::eyre::eyre!("ferrex config: {e}"))?;

    Ok(ferrex_core::FerrexConfig {
        qdrant_url: None,
        qdrant_bin: "qdrant".into(),
        qdrant_port: 6334,
        model_tier: ferrex_core::ModelTier::default(),
        reranker_tier: ferrex_core::RerankerTier::default(),
        namespace: "nephila".into(),
        db_path: ferrex_dir.join("ferrex.db"),
        config_path: Some(config_path),
        deduplication: loaded.deduplication,
        conflict: loaded.conflict,
        predicates: loaded.predicates,
        reconciliation: loaded.reconciliation,
        staleness: loaded.staleness,
        reader_pool_size: loaded.reader_pool_size,
        cache: loaded.cache,
    })
}
