use crate::checkpoint::{ChannelEntry, L2Chunk};
use crate::error::Result;
use crate::event::McpEvent;
use crate::objective::ObjectiveTree;
use std::collections::BTreeMap;

/// Fallback checkpoint generation when the agent doesn't provide channel content.
pub trait Summarizer: Send + Sync {
    fn generate_channels(
        &self,
        mcp_log: &[McpEvent],
        objectives: &ObjectiveTree,
    ) -> impl std::future::Future<Output = Result<BTreeMap<String, ChannelEntry>>> + Send;

    fn generate_l2(
        &self,
        mcp_log: &[McpEvent],
    ) -> impl std::future::Future<Output = Result<Vec<L2Chunk>>> + Send;
}
