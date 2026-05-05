//! Read-only connection pool for backfill queries.
//!
//! Hand-rolled `Vec<Connection>` behind a `Mutex` (per the plan, simpler than
//! pulling in `r2d2_sqlite`). Backfill from `subscribe_after` and blob reads
//! flow through here so they don't compete with the writer thread for the
//! single writer connection.

use rusqlite::{Connection, OpenFlags};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum ReadPoolError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("pool exhausted")]
    Exhausted,
}

/// A small fixed-size pool of read-only SQLite connections.
///
/// `acquire()` returns the next available `Connection`; `release()` returns it.
/// We do not block on exhaustion — callers receive `ReadPoolError::Exhausted`
/// and can retry. This matches the spec's "consumer is responsible for
/// backpressure" stance.
pub struct ReadPool {
    conns: Mutex<Vec<Connection>>,
}

impl ReadPool {
    pub fn open_file(path: &std::path::Path, size: usize) -> Result<Self, ReadPoolError> {
        let mut conns = Vec::with_capacity(size);
        for _ in 0..size {
            let conn = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
            )?;
            crate::schema::apply_tuning_pragmas(&conn)?;
            conns.push(conn);
        }
        Ok(Self {
            conns: Mutex::new(conns),
        })
    }

    /// Build an in-memory pool by attaching to a shared name. Used in tests
    /// where the writer also runs in-memory; we cannot share a `:memory:`
    /// across connections without `cache=shared`. The caller passes the same
    /// `share_name` they used when opening the writer connection.
    pub fn open_in_memory_shared(share_name: &str, size: usize) -> Result<Self, ReadPoolError> {
        let uri = format!("file:{share_name}?mode=memory&cache=shared");
        let mut conns = Vec::with_capacity(size);
        for _ in 0..size {
            let conn = Connection::open_with_flags(
                PathBuf::from(&uri),
                OpenFlags::SQLITE_OPEN_READ_ONLY
                    | OpenFlags::SQLITE_OPEN_FULL_MUTEX
                    | OpenFlags::SQLITE_OPEN_URI,
            )?;
            conn.pragma_update(None, "read_uncommitted", "ON")?;
            conns.push(conn);
        }
        Ok(Self {
            conns: Mutex::new(conns),
        })
    }

    pub fn acquire(&self) -> Result<Connection, ReadPoolError> {
        let mut guard = self.conns.lock().expect("read pool mutex poisoned");
        guard.pop().ok_or(ReadPoolError::Exhausted)
    }

    pub fn release(&self, conn: Connection) {
        let mut guard = self.conns.lock().expect("read pool mutex poisoned");
        guard.push(conn);
    }

    /// Acquire a connection wrapped in an RAII guard that returns the
    /// connection to the pool on drop — including drops triggered by
    /// `panic!` inside `spawn_blocking`. Use this from any read path that
    /// runs inside `spawn_blocking` or other panic-prone scopes; without
    /// it, a panicking closure permanently leaks the connection and
    /// eventually drains the pool to `PoolExhausted`.
    pub fn acquire_guarded(self: &Arc<Self>) -> Result<PooledConn, ReadPoolError> {
        let conn = self.acquire()?;
        Ok(PooledConn {
            pool: self.clone(),
            conn: Some(conn),
        })
    }

    #[must_use]
    pub fn available(&self) -> usize {
        self.conns.lock().map(|g| g.len()).unwrap_or_default()
    }
}

/// RAII wrapper around a pooled `Connection`. Returns the connection to the
/// pool on `Drop` — including drops triggered by panics. Deref(Mut) gives
/// callers transparent access to the underlying `Connection`.
pub struct PooledConn {
    pool: Arc<ReadPool>,
    conn: Option<Connection>,
}

impl PooledConn {
    /// Borrow the underlying connection. Panics if the guard has already
    /// been consumed by `into_inner` (it never has internally — this is
    /// defensive against misuse).
    #[must_use]
    pub fn as_conn(&self) -> &Connection {
        self.conn
            .as_ref()
            .expect("PooledConn used after into_inner")
    }
}

impl std::ops::Deref for PooledConn {
    type Target = Connection;
    fn deref(&self) -> &Self::Target {
        self.as_conn()
    }
}

impl Drop for PooledConn {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.pool.release(conn);
        }
    }
}

impl std::fmt::Debug for ReadPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadPool")
            .field("available", &self.available())
            .finish()
    }
}
