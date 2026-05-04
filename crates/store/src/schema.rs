use rusqlite::Connection;
use std::sync::Once;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY,
    state TEXT NOT NULL DEFAULT 'starting',
    directive TEXT NOT NULL DEFAULT 'continue',
    directory TEXT NOT NULL,
    objective_id TEXT NOT NULL,
    checkpoint_id TEXT REFERENCES checkpoints(id),
    restore_checkpoint_id TEXT REFERENCES checkpoints(id),
    spawned_by TEXT,
    spawn_origin_type TEXT NOT NULL DEFAULT 'operator',
    source_checkpoint_id TEXT,
    injected_message TEXT,
    session_id TEXT,
    last_config_snapshot TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS objectives (
    id TEXT PRIMARY KEY,
    parent_id TEXT REFERENCES objectives(id),
    agent_id TEXT REFERENCES agents(id),
    description TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS checkpoints (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id),
    parent_id TEXT REFERENCES checkpoints(id),
    branch_label TEXT,
    channels TEXT NOT NULL,
    l2_namespace TEXT NOT NULL DEFAULT 'general',
    interrupt TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_checkpoints_agent ON checkpoints(agent_id);
CREATE INDEX IF NOT EXISTS idx_checkpoints_parent ON checkpoints(parent_id);
CREATE INDEX IF NOT EXISTS idx_checkpoints_created ON checkpoints(agent_id, created_at DESC);

CREATE TABLE IF NOT EXISTS interrupt_requests (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL,
    checkpoint_id TEXT NOT NULL REFERENCES checkpoints(id),
    interrupt_type TEXT NOT NULL,
    payload TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    response TEXT,
    question_hash TEXT,
    ask_count INTEGER DEFAULT 0,
    created_at TEXT NOT NULL,
    resolved_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_interrupts_pending ON interrupt_requests(status) WHERE status = 'pending';
CREATE INDEX IF NOT EXISTS idx_interrupts_agent ON interrupt_requests(agent_id);

CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id),
    event_type TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '{}',
    objective_id TEXT
);

CREATE TABLE IF NOT EXISTS domain_events (
    id TEXT PRIMARY KEY,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    payload TEXT NOT NULL,
    trace_id TEXT NOT NULL,
    outcome TEXT,
    timestamp TEXT NOT NULL,
    context_snapshot TEXT,
    metadata TEXT,
    UNIQUE(aggregate_type, aggregate_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_domain_events_aggregate ON domain_events(aggregate_type, aggregate_id, sequence);
CREATE INDEX IF NOT EXISTS idx_domain_events_trace_id ON domain_events(trace_id);
CREATE INDEX IF NOT EXISTS idx_domain_events_timestamp ON domain_events(timestamp);

CREATE TABLE IF NOT EXISTS aggregate_snapshots (
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    state TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    PRIMARY KEY (aggregate_type, aggregate_id, sequence)
);

CREATE TABLE IF NOT EXISTS blobs (
    hash TEXT PRIMARY KEY,
    payload BLOB NOT NULL,
    original_len INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS spans (
    span_id TEXT PRIMARY KEY,
    trace_id TEXT NOT NULL,
    parent_span_id TEXT,
    name TEXT NOT NULL,
    level TEXT NOT NULL,
    target TEXT NOT NULL,
    start_time TEXT NOT NULL,
    end_time TEXT,
    duration_us INTEGER,
    attributes TEXT,
    events TEXT,
    status TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_spans_trace_id ON spans(trace_id);
CREATE INDEX IF NOT EXISTS idx_spans_parent ON spans(parent_span_id);
CREATE INDEX IF NOT EXISTS idx_spans_start_time ON spans(start_time);

CREATE TABLE IF NOT EXISTS search_entries (
    id TEXT PRIMARY KEY,
    metadata TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_events_agent_timestamp ON events(agent_id, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_objectives_parent ON objectives(parent_id);
"#;

static SQLITE_VEC_REGISTERED: Once = Once::new();

pub fn register_sqlite_vec() {
    SQLITE_VEC_REGISTERED.call_once(|| unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            unsafe extern "C" fn(
                *mut rusqlite::ffi::sqlite3,
                *mut *mut i8,
                *const rusqlite::ffi::sqlite3_api_routines,
            ) -> i32,
        >(
            sqlite_vec::sqlite3_vec_init as *const ()
        )));
    });
}

pub fn init_db(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(SCHEMA)?;
    apply_post_create_migrations(conn)?;
    Ok(())
}

/// Idempotent migrations applied after `CREATE TABLE IF NOT EXISTS`.
///
/// New columns added in later slices land here so existing databases pick
/// them up without requiring a full rebuild. SQLite's `ALTER TABLE ... ADD
/// COLUMN` errors with `duplicate column name` if the column already exists;
/// we swallow that specific error so the migration is idempotent.
fn apply_post_create_migrations(conn: &Connection) -> Result<(), rusqlite::Error> {
    // Slice 4: agents.last_config_snapshot — JSON-encoded `AgentConfigSnapshot`,
    // populated by `AgentEvent::AgentConfigSnapshotted` reducer projections.
    add_column_if_missing(conn, "agents", "last_config_snapshot", "TEXT")?;
    Ok(())
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    column_type: &str,
) -> Result<(), rusqlite::Error> {
    let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {column_type}");
    match conn.execute(&sql, []) {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(_, Some(msg)))
            if msg.contains("duplicate column name") =>
        {
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub fn init_vec_tables(conn: &Connection, dimension: usize) -> Result<(), rusqlite::Error> {
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_search_entries USING vec0(embedding float[{dimension}]);"
    ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_exist_after_init() {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();

        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        assert!(tables.contains(&"agents".to_string()));
        assert!(tables.contains(&"objectives".to_string()));
        assert!(tables.contains(&"checkpoints".to_string()));
        assert!(tables.contains(&"interrupt_requests".to_string()));
        assert!(tables.contains(&"events".to_string()));
        assert!(tables.contains(&"search_entries".to_string()));
        assert!(tables.contains(&"domain_events".to_string()));
        assert!(tables.contains(&"aggregate_snapshots".to_string()));
        assert!(tables.contains(&"spans".to_string()));
    }

    #[test]
    fn vec_tables_exist_after_init() {
        register_sqlite_vec();
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        init_vec_tables(&conn, 384).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='vec_search_entries'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
