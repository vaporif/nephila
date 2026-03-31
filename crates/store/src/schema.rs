use rusqlite::Connection;
use std::sync::Once;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS agents (
    id TEXT PRIMARY KEY,
    state TEXT NOT NULL DEFAULT 'starting',
    directive TEXT NOT NULL DEFAULT 'continue',
    directory TEXT NOT NULL,
    objective_id TEXT NOT NULL,
    checkpoint_version INTEGER,
    spawned_by TEXT,
    injected_message TEXT,
    session_id TEXT,
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
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent_id TEXT NOT NULL REFERENCES agents(id),
    version INTEGER NOT NULL,
    layer TEXT NOT NULL,
    content TEXT NOT NULL,
    embedding BLOB,
    created_at TEXT NOT NULL,
    UNIQUE(agent_id, version, layer)
);

CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id),
    content TEXT NOT NULL,
    embedding BLOB NOT NULL,
    tags TEXT NOT NULL DEFAULT '[]',
    lifecycle_state TEXT NOT NULL DEFAULT 'generated',
    importance REAL NOT NULL DEFAULT 0.5,
    access_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS memory_links (
    source_id TEXT NOT NULL REFERENCES memories(id),
    target_id TEXT NOT NULL REFERENCES memories(id),
    similarity_score REAL NOT NULL,
    PRIMARY KEY (source_id, target_id)
);

CREATE TABLE IF NOT EXISTS events (
    id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agents(id),
    event_type TEXT NOT NULL,
    timestamp TEXT NOT NULL,
    content TEXT NOT NULL DEFAULT '{}',
    objective_id TEXT
);

CREATE TABLE IF NOT EXISTS hitl_tracking (
    agent_id TEXT NOT NULL REFERENCES agents(id),
    question_hash INTEGER NOT NULL,
    ask_count INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (agent_id, question_hash)
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

CREATE INDEX IF NOT EXISTS idx_checkpoints_agent_version ON checkpoints(agent_id, version DESC);
CREATE INDEX IF NOT EXISTS idx_memories_agent ON memories(agent_id);
CREATE INDEX IF NOT EXISTS idx_memories_lifecycle ON memories(lifecycle_state);
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
    Ok(())
}

pub fn init_vec_tables(conn: &Connection, dimension: usize) -> Result<(), rusqlite::Error> {
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_memories USING vec0(embedding float[{dimension}]);
         CREATE VIRTUAL TABLE IF NOT EXISTS vec_search_entries USING vec0(embedding float[{dimension}]);"
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
        assert!(tables.contains(&"memories".to_string()));
        assert!(tables.contains(&"memory_links".to_string()));
        assert!(tables.contains(&"events".to_string()));
        assert!(tables.contains(&"hitl_tracking".to_string()));
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
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='vec_memories'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}
