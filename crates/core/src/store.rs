use crate::agent::{Agent, AgentState};
use crate::checkpoint::{CheckpointNode, L2Chunk, L2SearchResult};
use crate::directive::Directive;
use crate::error::Result;
use crate::event::McpEvent;
use crate::id::*;
use crate::interrupt::InterruptRequest;
use crate::memory::{Embedding, LifecycleState, Link, MemoryEntry, SearchResult};
use crate::objective::{NewObjective, ObjectiveNode, ObjectiveStatus, ObjectiveTree};
use chrono::{DateTime, Utc};

pub trait AgentStore: Send + Sync {
    fn register(&self, agent: Agent) -> impl std::future::Future<Output = Result<()>> + Send;

    fn get(&self, id: AgentId) -> impl std::future::Future<Output = Result<Option<Agent>>> + Send;

    fn list(&self) -> impl std::future::Future<Output = Result<Vec<Agent>>> + Send;

    fn save(&self, agent: &Agent) -> impl std::future::Future<Output = Result<()>> + Send;

    fn update_state(
        &self,
        id: AgentId,
        state: AgentState,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn set_directive(
        &self,
        id: AgentId,
        directive: Directive,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn get_directive(
        &self,
        id: AgentId,
    ) -> impl std::future::Future<Output = Result<Directive>> + Send;

    fn set_injected_message(
        &self,
        id: AgentId,
        message: Option<String>,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn set_checkpoint_id(
        &self,
        id: AgentId,
        checkpoint_id: CheckpointId,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn set_restore_checkpoint(
        &self,
        id: AgentId,
        checkpoint_id: Option<CheckpointId>,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
}

pub trait CheckpointStore: Send + Sync {
    fn save(
        &self,
        node: &CheckpointNode,
        l2_chunks: &[L2Chunk],
        l2_embeddings: &[Embedding],
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn get(
        &self,
        id: CheckpointId,
    ) -> impl std::future::Future<Output = Result<Option<CheckpointNode>>> + Send;

    fn get_latest(
        &self,
        agent_id: AgentId,
    ) -> impl std::future::Future<Output = Result<Option<CheckpointNode>>> + Send;

    fn get_children(
        &self,
        id: CheckpointId,
    ) -> impl std::future::Future<Output = Result<Vec<CheckpointNode>>> + Send;

    fn get_ancestry(
        &self,
        id: CheckpointId,
    ) -> impl std::future::Future<Output = Result<Vec<CheckpointNode>>> + Send;

    fn list_branches(
        &self,
        agent_id: AgentId,
    ) -> impl std::future::Future<Output = Result<Vec<CheckpointNode>>> + Send;

    fn search_l2(
        &self,
        agent_id: AgentId,
        namespace: Option<&str>,
        embedding: &[f32],
        limit: usize,
    ) -> impl std::future::Future<Output = Result<Vec<L2SearchResult>>> + Send;

    fn search_l2_global(
        &self,
        namespace: Option<&str>,
        embedding: &[f32],
        limit: usize,
    ) -> impl std::future::Future<Output = Result<Vec<L2SearchResult>>> + Send;
}

pub trait MemoryStore: Send + Sync {
    fn store(
        &self,
        entry: MemoryEntry,
    ) -> impl std::future::Future<Output = Result<EntryId>> + Send;

    fn get(
        &self,
        id: EntryId,
    ) -> impl std::future::Future<Output = Result<Option<MemoryEntry>>> + Send;

    fn search(
        &self,
        query: &Embedding,
        limit: usize,
    ) -> impl std::future::Future<Output = Result<Vec<SearchResult>>> + Send;

    fn find_similar(
        &self,
        embedding: &Embedding,
        threshold: f32,
    ) -> impl std::future::Future<Output = Result<Vec<(EntryId, f32)>>> + Send;

    fn update_links(
        &self,
        id: EntryId,
        links: Vec<Link>,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn get_linked(
        &self,
        id: EntryId,
        depth: usize,
    ) -> impl std::future::Future<Output = Result<Vec<MemoryEntry>>> + Send;

    fn transition_state(
        &self,
        id: EntryId,
        new_state: LifecycleState,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn increment_access(&self, id: EntryId)
    -> impl std::future::Future<Output = Result<()>> + Send;
}

pub trait ObjectiveStore: Send + Sync {
    fn create(
        &self,
        objective: NewObjective,
    ) -> impl std::future::Future<Output = Result<ObjectiveId>> + Send;

    fn update_status(
        &self,
        id: ObjectiveId,
        status: ObjectiveStatus,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn get_node(
        &self,
        id: ObjectiveId,
    ) -> impl std::future::Future<Output = Result<Option<ObjectiveNode>>> + Send;

    fn get_tree(
        &self,
        root_id: ObjectiveId,
    ) -> impl std::future::Future<Output = Result<ObjectiveTree>> + Send;

    fn assign_agent(
        &self,
        objective_id: ObjectiveId,
        agent_id: AgentId,
    ) -> impl std::future::Future<Output = Result<()>> + Send;
}

pub trait McpEventLog: Send + Sync {
    fn append(&self, event: McpEvent) -> impl std::future::Future<Output = Result<()>> + Send;

    fn get_events(
        &self,
        agent_id: AgentId,
        since: Option<DateTime<Utc>>,
        limit: usize,
    ) -> impl std::future::Future<Output = Result<Vec<McpEvent>>> + Send;

    fn get_tool_calls(
        &self,
        agent_id: AgentId,
    ) -> impl std::future::Future<Output = Result<Vec<McpEvent>>> + Send;
}

pub trait InterruptStore: Send + Sync {
    fn save(
        &self,
        request: &InterruptRequest,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn get_pending(
        &self,
        agent_id: AgentId,
    ) -> impl std::future::Future<Output = Result<Option<InterruptRequest>>> + Send;

    fn resolve(
        &self,
        id: InterruptId,
        response: serde_json::Value,
    ) -> impl std::future::Future<Output = Result<()>> + Send;

    fn expire(&self, id: InterruptId) -> impl std::future::Future<Output = Result<()>> + Send;

    fn list_pending(
        &self,
    ) -> impl std::future::Future<Output = Result<Vec<InterruptRequest>>> + Send;
}
