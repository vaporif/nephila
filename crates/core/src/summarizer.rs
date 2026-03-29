use crate::checkpoint::{L0State, L2Chunk};
use crate::error::Result;
use crate::event::McpEvent;
use crate::objective::ObjectiveTree;

/// Fallback checkpoint generation when Claude doesn't provide layer content.
pub trait Summarizer: Send + Sync {
    fn generate_l0(
        &self,
        mcp_log: &[McpEvent],
        objectives: &ObjectiveTree,
    ) -> impl std::future::Future<Output = Result<L0State>> + Send;

    fn generate_l1(
        &self,
        mcp_log: &[McpEvent],
        objectives: &ObjectiveTree,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    fn generate_l2(
        &self,
        mcp_log: &[McpEvent],
    ) -> impl std::future::Future<Output = Result<Vec<L2Chunk>>> + Send;
}
