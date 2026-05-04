//! Blob spillover store for oversized tool-result payloads.
//!
//! `prepare_blob` is a pure helper — no I/O. The actual `INSERT OR IGNORE INTO
//! blobs` runs inside the writer-thread transaction that appends the
//! `ToolResult` event row, keeping the blob and the referencing event
//! atomic. There is no standalone `BlobStore::put` for this reason.
//!
//! `BlobReader` is the read-only handle consumed by `SessionPane` and replay
//! paths to fetch spilled payloads on demand. The default impl uses the
//! read-only connection pool from step 7.

use crate::read_pool::ReadPool;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct PreparedBlob {
    pub hash: String,
    pub original_len: u64,
    pub snippet: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct BlobRef {
    pub hash: String,
    pub original_len: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    #[error("storage error: {0}")]
    Storage(String),
    #[error("not found")]
    NotFound,
}

pub trait BlobReader: Send + Sync {
    fn get(
        &self,
        hash: &str,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, BlobError>> + Send;
}

#[derive(Clone)]
pub struct SqliteBlobReader {
    pool: Arc<ReadPool>,
}

impl SqliteBlobReader {
    #[must_use]
    pub fn new(pool: Arc<ReadPool>) -> Self {
        Self { pool }
    }
}

impl BlobReader for SqliteBlobReader {
    async fn get(&self, hash: &str) -> Result<Vec<u8>, BlobError> {
        let pool = self.pool.clone();
        let hash = hash.to_owned();
        tokio::task::spawn_blocking(move || -> Result<Vec<u8>, BlobError> {
            let conn = pool
                .acquire()
                .map_err(|e| BlobError::Storage(e.to_string()))?;
            let result: Result<Vec<u8>, rusqlite::Error> = conn.query_row(
                "SELECT payload FROM blobs WHERE hash = ?1",
                rusqlite::params![&hash],
                |row| row.get::<_, Vec<u8>>(0),
            );
            pool.release(conn);
            match result {
                Ok(bytes) => Ok(bytes),
                Err(rusqlite::Error::QueryReturnedNoRows) => Err(BlobError::NotFound),
                Err(e) => Err(BlobError::Storage(e.to_string())),
            }
        })
        .await
        .map_err(|e| BlobError::Storage(format!("join: {e}")))?
    }
}

/// Hashes the bytes, computes a UTF-8-safe snippet, returns the parts the
/// connector embeds in the `Spilled` payload variant.
#[must_use]
pub fn prepare_blob(bytes: &[u8]) -> PreparedBlob {
    let hash = blake3::hash(bytes).to_hex().to_string();
    let original_len = bytes.len() as u64;
    let cap = bytes.len().min(8 * 1024);
    // Walk back from `cap` to a UTF-8 boundary so the snippet is decodable.
    let snippet_end = (0..=cap)
        .rev()
        .find(|&i| std::str::from_utf8(&bytes[..i]).is_ok())
        .unwrap_or(0);
    let snippet = String::from_utf8_lossy(&bytes[..snippet_end]).into_owned();
    PreparedBlob {
        hash,
        original_len,
        snippet,
        payload: bytes.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_blob_hashes_and_clips_snippet() {
        let bytes = b"hello world".repeat(2000); // ~22000 bytes, well over 8 KiB
        let p = prepare_blob(&bytes);
        assert_eq!(p.original_len, bytes.len() as u64);
        assert_eq!(p.payload.len(), bytes.len());
        assert!(p.snippet.len() <= 8 * 1024);
        assert!(p.hash.len() == 64); // blake3 hex
    }

    #[test]
    fn prepare_blob_snippet_is_utf8_safe_at_boundary() {
        // Construct a vector where the byte at offset 8192 is mid-codepoint.
        let mut bytes = Vec::with_capacity(8200);
        // Fill with ascii then a 4-byte codepoint straddling 8192.
        bytes.extend(std::iter::repeat_n(b'a', 8190));
        bytes.extend_from_slice("𝄞".as_bytes()); // 4 bytes; offsets 8190..=8193
        bytes.extend(std::iter::repeat_n(b'b', 5));
        let p = prepare_blob(&bytes);
        // Decoding the snippet must succeed; the safe-walk-back trims at the
        // codepoint boundary.
        assert!(p.snippet.is_ascii() || p.snippet.contains('𝄞') || p.snippet.ends_with('a'));
        assert_eq!(p.original_len, bytes.len() as u64);
    }

    #[test]
    fn prepare_blob_idempotent_hash() {
        let p1 = prepare_blob(b"data");
        let p2 = prepare_blob(b"data");
        assert_eq!(p1.hash, p2.hash);
    }
}
