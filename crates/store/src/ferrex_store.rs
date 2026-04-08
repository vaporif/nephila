use ferrex_core::{
    ForgetRequest, ForgetResponse, MemoryService, RecallRequest, RecallResult, ReflectRequest,
    ReflectResponse, StoreRequest, StoreResponse,
};
use ferrex_store::{BatchPoint, ScoredPayload};
use nephila_core::NephilaError;
use nephila_core::checkpoint::{CheckpointNode, L2Chunk, L2SearchResult};
use nephila_core::error::Result;
use nephila_core::id::{AgentId, CheckpointId, EntryId};
use nephila_core::store::{CheckpointStore, MemoryStore};
use qdrant_client::qdrant::{Condition, Filter};
use uuid::Uuid;

use crate::SqliteStore;

const L2_RERANK_POOL: usize = 20;

pub struct FerrexStore {
    service: MemoryService,
    sqlite: SqliteStore,
    l2_collection: String,
}

impl FerrexStore {
    pub async fn new(
        service: MemoryService,
        sqlite: SqliteStore,
        l2_collection: String,
        memory_namespace: &str,
    ) -> Result<Self> {
        if l2_collection == memory_namespace {
            return Err(NephilaError::Storage(format!(
                "L2 collection '{l2_collection}' must differ from memory namespace '{memory_namespace}'"
            )));
        }

        service
            .vector_store()
            .ensure_collection(&l2_collection)
            .await
            .map_err(|e| NephilaError::Storage(format!("failed to create L2 collection: {e}")))?;

        Ok(Self {
            service,
            sqlite,
            l2_collection,
        })
    }

    pub fn service(&self) -> &MemoryService {
        &self.service
    }

    pub fn sqlite(&self) -> &SqliteStore {
        &self.sqlite
    }
}

impl MemoryStore for FerrexStore {
    async fn store(&self, request: StoreRequest) -> Result<StoreResponse> {
        self.service
            .store(request)
            .await
            .map_err(|e| NephilaError::Storage(e.to_string()))
    }

    async fn recall(&self, request: RecallRequest) -> Result<Vec<RecallResult>> {
        self.service
            .recall(request)
            .await
            .map_err(|e| NephilaError::Storage(e.to_string()))
    }

    async fn forget(&self, request: ForgetRequest) -> Result<ForgetResponse> {
        self.service
            .forget(request)
            .await
            .map_err(|e| NephilaError::Storage(e.to_string()))
    }

    async fn reflect(&self, request: ReflectRequest) -> Result<ReflectResponse> {
        self.service
            .reflect(request)
            .await
            .map_err(|e| NephilaError::Storage(e.to_string()))
    }
}

impl CheckpointStore for FerrexStore {
    async fn save(&self, node: &CheckpointNode, l2_chunks: &[L2Chunk]) -> Result<()> {
        self.sqlite.save_checkpoint_metadata(node).await?;

        if !l2_chunks.is_empty() {
            let texts: Vec<&str> = l2_chunks.iter().map(|c| c.content.as_str()).collect();
            let embeddings = self
                .service
                .embedder()
                .embed_batch(&texts)
                .await
                .map_err(|e| NephilaError::Storage(format!("L2 embedding failed: {e}")))?;

            let mut payload_data = Vec::with_capacity(l2_chunks.len());
            for (chunk, embedding) in l2_chunks.iter().zip(embeddings.into_iter()) {
                let payload = qdrant_client::Payload::try_from(serde_json::json!({
                    "agent_id": node.agent_id.0.to_string(),
                    "checkpoint_id": node.id.0.to_string(),
                    "namespace": &node.l2_namespace,
                    "content": &chunk.content,
                    "tags": &chunk.tags,
                }))
                .map_err(|e| NephilaError::Storage(format!("payload build failed: {e}")))?;

                payload_data.push(BatchPoint {
                    id: chunk.id.0,
                    vector: embedding,
                    content_text: &chunk.content,
                    payload,
                });
            }

            self.service
                .vector_store()
                .upsert_batch_hybrid(&self.l2_collection, payload_data)
                .await
                .map_err(|e| NephilaError::Storage(format!("L2 upsert failed: {e}")))?;
        }

        Ok(())
    }

    async fn get(&self, id: CheckpointId) -> Result<Option<CheckpointNode>> {
        self.sqlite.get_checkpoint(id).await
    }

    async fn get_latest(&self, agent_id: AgentId) -> Result<Option<CheckpointNode>> {
        self.sqlite.get_latest_checkpoint(agent_id).await
    }

    async fn get_children(&self, id: CheckpointId) -> Result<Vec<CheckpointNode>> {
        self.sqlite.get_checkpoint_children(id).await
    }

    async fn get_ancestry(&self, id: CheckpointId) -> Result<Vec<CheckpointNode>> {
        self.sqlite.get_checkpoint_ancestry(id).await
    }

    async fn list_branches(&self, agent_id: AgentId) -> Result<Vec<CheckpointNode>> {
        self.sqlite.list_checkpoint_branches(agent_id).await
    }

    async fn search_l2(
        &self,
        agent_id: AgentId,
        namespace: Option<&str>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<L2SearchResult>> {
        let embedding = self
            .service
            .embedder()
            .embed(query)
            .await
            .map_err(|e| NephilaError::Storage(format!("L2 query embedding failed: {e}")))?;

        let mut conditions = vec![Condition::matches("agent_id", agent_id.0.to_string())];
        if let Some(ns) = namespace {
            conditions.push(Condition::matches("namespace", ns.to_owned()));
        }
        let filter = Filter::must(conditions);

        let fetch_limit = (limit * 3).max(L2_RERANK_POOL);
        let raw_results = self
            .service
            .vector_store()
            .search_with_payload(
                &self.l2_collection,
                embedding,
                query,
                fetch_limit,
                Some(filter),
            )
            .await
            .map_err(|e| NephilaError::Storage(format!("L2 search failed: {e}")))?;

        if raw_results.is_empty() {
            return Ok(vec![]);
        }

        Ok(scored_payloads_to_l2_results(&raw_results, limit))
    }

    async fn search_l2_global(
        &self,
        namespace: Option<&str>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<L2SearchResult>> {
        let embedding = self
            .service
            .embedder()
            .embed(query)
            .await
            .map_err(|e| NephilaError::Storage(format!("L2 query embedding failed: {e}")))?;

        let filter =
            namespace.map(|ns| Filter::must(vec![Condition::matches("namespace", ns.to_owned())]));

        let fetch_limit = (limit * 3).max(L2_RERANK_POOL);
        let raw_results = self
            .service
            .vector_store()
            .search_with_payload(&self.l2_collection, embedding, query, fetch_limit, filter)
            .await
            .map_err(|e| NephilaError::Storage(format!("L2 search failed: {e}")))?;

        Ok(scored_payloads_to_l2_results(&raw_results, limit))
    }
}

fn scored_payloads_to_l2_results(results: &[ScoredPayload], limit: usize) -> Vec<L2SearchResult> {
    results
        .iter()
        .take(limit)
        .map(|r| {
            let json: serde_json::Value = r.payload.clone().into();
            let entry_id = r.id.parse::<Uuid>().unwrap_or_default();
            let content = json["content"].as_str().unwrap_or_default().to_string();
            let agent_str = json["agent_id"].as_str().unwrap_or_default();
            let agent_id = agent_str
                .parse::<Uuid>()
                .map(AgentId)
                .unwrap_or(AgentId(Uuid::nil()));
            let ns = json["namespace"].as_str().unwrap_or("general").to_string();
            let tags = json["tags"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            L2SearchResult {
                chunk: L2Chunk {
                    id: EntryId(entry_id),
                    content,
                    tags,
                },
                agent_id,
                namespace: ns,
                score: r.score,
            }
        })
        .collect()
}
