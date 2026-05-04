pub mod agent;
pub mod blob;
pub mod checkpoint;
pub mod domain_event;
pub mod event;
pub mod ferrex_store;
pub mod interrupt;
pub mod lockfile;
pub mod metrics;
pub mod objective;
pub mod read_pool;
pub mod resilient_subscribe;
pub mod retention;
pub mod schema;
pub mod search_provider;
pub mod subscribe;
pub mod tracing_store;
pub(crate) mod util;
pub mod writer;

pub use ferrex_store::FerrexStore;

#[cfg(test)]
pub(crate) mod test_util;

use crate::read_pool::ReadPool;
use dashmap::DashMap;
use std::sync::Arc;
use writer::WriterHandle;

const SNAPSHOT_INTERVAL: u64 = 1000;

/// Per-aggregate snapshot-task lock. Prevents thundering-herd snapshotting
/// when multiple consumers `subscribe_after` against the same aggregate
/// in rapid succession.
type SnapshotLocks = Arc<DashMap<(String, String), ()>>;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("writer thread closed")]
    WriterClosed,

    #[error("not found: {0}")]
    NotFound(String),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("read pool: {0}")]
    ReadPool(#[from] crate::read_pool::ReadPoolError),
}

impl From<StoreError> for nephila_core::NephilaError {
    fn from(e: StoreError) -> Self {
        nephila_core::NephilaError::Storage(e.to_string())
    }
}

const READ_POOL_SIZE: usize = 4;

#[derive(Clone)]
pub struct SqliteStore {
    pub(crate) writer: WriterHandle,
    pub(crate) read_pool: Arc<ReadPool>,
    pub(crate) snapshot_locks: SnapshotLocks,
}

impl SqliteStore {
    /// Accessor for the read-only connection pool. Used by `SqliteBlobReader`
    /// and other read paths that bypass the writer thread.
    #[must_use]
    pub fn read_pool(&self) -> Arc<ReadPool> {
        self.read_pool.clone()
    }
}

impl SqliteStore {
    pub fn open(path: &std::path::Path, embedding_dim: usize) -> Result<Self, StoreError> {
        schema::register_sqlite_vec();
        let conn = rusqlite::Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        schema::init_db(&conn)?;
        schema::init_vec_tables(&conn, embedding_dim)?;
        let writer = WriterHandle::new(conn);
        let read_pool = Arc::new(read_pool::ReadPool::open_file(path, READ_POOL_SIZE)?);
        Ok(Self {
            writer,
            read_pool,
            snapshot_locks: Arc::new(DashMap::new()),
        })
    }

    pub fn open_in_memory(embedding_dim: usize) -> Result<Self, StoreError> {
        // Use a unique shared in-memory URI so the read pool can attach via
        // `cache=shared`. This is the only way to share an in-memory db across
        // connections in rusqlite.
        let share_name = format!("nephila-mem-{}", uuid::Uuid::new_v4());
        let uri = format!("file:{share_name}?mode=memory&cache=shared");
        schema::register_sqlite_vec();
        let conn = rusqlite::Connection::open_with_flags(
            &uri,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
                | rusqlite::OpenFlags::SQLITE_OPEN_URI
                | rusqlite::OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // Read-uncommitted on writer so shared-cache readers don't block on
        // table locks. This is in-memory-only — file-backed stores use WAL.
        conn.pragma_update(None, "read_uncommitted", "ON")?;
        schema::init_db(&conn)?;
        schema::init_vec_tables(&conn, embedding_dim)?;
        let writer = WriterHandle::new(conn);
        let read_pool = Arc::new(
            read_pool::ReadPool::open_in_memory_shared(&share_name, READ_POOL_SIZE)
                .map_err(|e| StoreError::NotFound(format!("read pool: {e}")))?,
        );
        Ok(Self {
            writer,
            read_pool,
            snapshot_locks: Arc::new(DashMap::new()),
        })
    }
}
