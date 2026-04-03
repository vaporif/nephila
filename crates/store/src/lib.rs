pub mod agent;
pub mod checkpoint;
pub mod domain_event;
pub mod event;
pub mod interrupt;
pub mod memory;
pub mod objective;
pub mod schema;
pub mod search_provider;
pub mod tracing_store;
pub(crate) mod util;
pub mod writer;

#[cfg(test)]
pub(crate) mod test_util;

use writer::WriterHandle;

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
}

impl From<StoreError> for meridian_core::MeridianError {
    fn from(e: StoreError) -> Self {
        meridian_core::MeridianError::Storage(e.to_string())
    }
}

#[derive(Clone)]
pub struct SqliteStore {
    writer: WriterHandle,
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
        Ok(Self { writer })
    }

    pub fn open_in_memory(embedding_dim: usize) -> Result<Self, StoreError> {
        schema::register_sqlite_vec();
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        schema::init_db(&conn)?;
        schema::init_vec_tables(&conn, embedding_dim)?;
        let writer = WriterHandle::new(conn);
        Ok(Self { writer })
    }
}
