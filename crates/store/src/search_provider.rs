use crate::SqliteStore;
use crate::util::f32_slice_to_bytes;
use nephila_eventsourcing::search::{
    SearchEntry, SearchError, SearchFilter, SearchMetadata, SearchProvider, VectorSearchResult,
};

impl SearchProvider for SqliteStore {
    async fn index(
        &self,
        id: &str,
        embedding: Vec<f32>,
        metadata: SearchMetadata,
    ) -> Result<(), SearchError> {
        let id = id.to_string();
        let emb_bytes = f32_slice_to_bytes(&embedding);
        let metadata_json =
            serde_json::to_string(&metadata).map_err(|e| SearchError::Storage(e.to_string()))?;

        self.writer
            .execute(move |conn| {
                let tx = conn.unchecked_transaction()?;

                tx.execute(
                    "INSERT OR REPLACE INTO search_entries (id, metadata) VALUES (?1, ?2)",
                    rusqlite::params![id, metadata_json],
                )?;

                let rowid: i64 = tx.query_row(
                    "SELECT rowid FROM search_entries WHERE id = ?1",
                    rusqlite::params![id],
                    |row| row.get(0),
                )?;

                tx.execute(
                    "DELETE FROM vec_search_entries WHERE rowid = ?1",
                    rusqlite::params![rowid],
                )?;
                tx.execute(
                    "INSERT INTO vec_search_entries (rowid, embedding) VALUES (?1, ?2)",
                    rusqlite::params![rowid, &emb_bytes],
                )?;

                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|e| SearchError::Storage(e.to_string()))
    }

    async fn remove(&self, id: &str) -> Result<(), SearchError> {
        let id = id.to_string();
        self.writer
            .execute(move |conn| {
                let tx = conn.unchecked_transaction()?;

                match tx.query_row(
                    "SELECT rowid FROM search_entries WHERE id = ?1",
                    rusqlite::params![id],
                    |row| row.get::<_, i64>(0),
                ) {
                    Ok(rowid) => {
                        tx.execute(
                            "DELETE FROM vec_search_entries WHERE rowid = ?1",
                            rusqlite::params![rowid],
                        )?;
                        tx.execute(
                            "DELETE FROM search_entries WHERE id = ?1",
                            rusqlite::params![id],
                        )?;
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => {}
                    Err(e) => return Err(e),
                }

                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|e| SearchError::Storage(e.to_string()))
    }

    async fn search(
        &self,
        query_embedding: Vec<f32>,
        limit: usize,
        filter: Option<SearchFilter>,
    ) -> Result<Vec<VectorSearchResult>, SearchError> {
        let query_bytes = f32_slice_to_bytes(&query_embedding);
        self.writer
            .execute(move |conn| {
                let mut vec_stmt = conn.prepare(
                    "SELECT rowid, distance FROM vec_search_entries WHERE embedding MATCH ?1 AND k = ?2",
                )?;
                let vec_rows: Vec<(i64, f64)> = vec_stmt
                    .query_map(rusqlite::params![query_bytes, limit as i64], |row| {
                        Ok((row.get(0)?, row.get(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;

                let mut results = Vec::new();
                let mut meta_stmt =
                    conn.prepare("SELECT id, metadata FROM search_entries WHERE rowid = ?1")?;

                for (rowid, distance) in vec_rows {
                    match meta_stmt.query_row(rusqlite::params![rowid], |row| {
                        let id: String = row.get(0)?;
                        let metadata_json: String = row.get(1)?;
                        Ok((id, metadata_json))
                    }) {
                        Ok((id, metadata_json)) => {
                            let metadata: SearchMetadata =
                                serde_json::from_str(&metadata_json).map_err(|e| {
                                    rusqlite::Error::FromSqlConversionFailure(
                                        0,
                                        rusqlite::types::Type::Text,
                                        Box::new(e),
                                    )
                                })?;

                            if let Some(ref f) = filter {
                                if let Some(ref filter_tags) = f.tags
                                    && !filter_tags.iter().any(|t| metadata.tags.contains(t))
                                {
                                    continue;
                                }
                                if let Some(ref filter_agg) = f.aggregate_id
                                    && metadata.aggregate_id.as_ref() != Some(filter_agg)
                                {
                                    continue;
                                }
                            }

                            results.push(VectorSearchResult {
                                id,
                                score: (1.0 - distance) as f32,
                                metadata,
                            });
                        }
                        Err(rusqlite::Error::QueryReturnedNoRows) => continue,
                        Err(e) => return Err(e),
                    }
                }
                Ok(results)
            })
            .await
            .map_err(|e| SearchError::Storage(e.to_string()))
    }

    async fn rebuild(&self, entries: Vec<SearchEntry>) -> Result<(), SearchError> {
        let prepared: Vec<(String, Vec<u8>, String)> = entries
            .into_iter()
            .map(|e| {
                let bytes = f32_slice_to_bytes(&e.embedding);
                let meta = serde_json::to_string(&e.metadata)
                    .map_err(|err| SearchError::Storage(err.to_string()));
                meta.map(|m| (e.id, bytes, m))
            })
            .collect::<Result<Vec<_>, _>>()?;

        self.writer
            .execute(move |conn| {
                let tx = conn.unchecked_transaction()?;

                tx.execute("DELETE FROM vec_search_entries", [])?;
                tx.execute("DELETE FROM search_entries", [])?;

                for (id, emb_bytes, metadata_json) in &prepared {
                    tx.execute(
                        "INSERT INTO search_entries (id, metadata) VALUES (?1, ?2)",
                        rusqlite::params![id, metadata_json],
                    )?;
                    let rowid = tx.last_insert_rowid();
                    tx.execute(
                        "INSERT INTO vec_search_entries (rowid, embedding) VALUES (?1, ?2)",
                        rusqlite::params![rowid, emb_bytes],
                    )?;
                }

                tx.commit()?;
                Ok(())
            })
            .await
            .map_err(|e| SearchError::Storage(e.to_string()))
    }
}
