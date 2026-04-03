use meridian_core::agent::AgentPhase;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
    GetSessionCheckpoint,
    SearchGraph,
    GetObjectiveTree,
    GetDirective,
    ReportTokenEstimate,
    StoreMemory,
    UpdateObjective,
    RequestHumanInput,
    SerializeAndPersist,
    RequestContextReset,
    SpawnAgent,
    GetAgentStatus,
    GetEventLog,
}

impl ToolName {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::GetSessionCheckpoint => "get_session_checkpoint",
            Self::SearchGraph => "search_graph",
            Self::GetObjectiveTree => "get_objective_tree",
            Self::GetDirective => "get_directive",
            Self::ReportTokenEstimate => "report_token_estimate",
            Self::StoreMemory => "store_memory",
            Self::UpdateObjective => "update_objective",
            Self::RequestHumanInput => "request_human_input",
            Self::SerializeAndPersist => "serialize_and_persist",
            Self::RequestContextReset => "request_context_reset",
            Self::SpawnAgent => "spawn_agent",
            Self::GetAgentStatus => "get_agent_status",
            Self::GetEventLog => "get_event_log",
        }
    }
}

pub fn phase_tools(phase: AgentPhase) -> Vec<ToolName> {
    match phase {
        AgentPhase::Starting => vec![
            ToolName::GetSessionCheckpoint,
            ToolName::SearchGraph,
            ToolName::GetObjectiveTree,
            ToolName::GetDirective,
        ],
        AgentPhase::Active => vec![
            ToolName::ReportTokenEstimate,
            ToolName::SearchGraph,
            ToolName::StoreMemory,
            ToolName::GetObjectiveTree,
            ToolName::UpdateObjective,
            ToolName::RequestHumanInput,
            ToolName::GetDirective,
            ToolName::SpawnAgent,
            ToolName::GetAgentStatus,
            ToolName::GetEventLog,
        ],
        AgentPhase::Suspending => vec![
            ToolName::SerializeAndPersist,
            ToolName::StoreMemory,
            ToolName::RequestContextReset,
            ToolName::GetDirective,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starting_phase_has_expected_tools() {
        let tools = phase_tools(AgentPhase::Starting);
        assert!(tools.contains(&ToolName::GetSessionCheckpoint));
        assert!(tools.contains(&ToolName::SearchGraph));
        assert!(tools.contains(&ToolName::GetObjectiveTree));
        assert!(tools.contains(&ToolName::GetDirective));
        assert_eq!(tools.len(), 4);

        assert!(!tools.contains(&ToolName::ReportTokenEstimate));
        assert!(!tools.contains(&ToolName::StoreMemory));
        assert!(!tools.contains(&ToolName::SerializeAndPersist));
    }

    #[test]
    fn active_phase_has_expected_tools() {
        let tools = phase_tools(AgentPhase::Active);
        assert!(tools.contains(&ToolName::ReportTokenEstimate));
        assert!(tools.contains(&ToolName::SearchGraph));
        assert!(tools.contains(&ToolName::StoreMemory));
        assert!(tools.contains(&ToolName::GetObjectiveTree));
        assert!(tools.contains(&ToolName::UpdateObjective));
        assert!(tools.contains(&ToolName::RequestHumanInput));
        assert!(tools.contains(&ToolName::GetDirective));
        assert!(tools.contains(&ToolName::SpawnAgent));
        assert!(tools.contains(&ToolName::GetAgentStatus));
        assert!(tools.contains(&ToolName::GetEventLog));
        assert_eq!(tools.len(), 10);

        assert!(!tools.contains(&ToolName::GetSessionCheckpoint));
        assert!(!tools.contains(&ToolName::SerializeAndPersist));
        assert!(!tools.contains(&ToolName::RequestContextReset));
    }

    #[test]
    fn suspending_phase_has_expected_tools() {
        let tools = phase_tools(AgentPhase::Suspending);
        assert!(tools.contains(&ToolName::SerializeAndPersist));
        assert!(tools.contains(&ToolName::StoreMemory));
        assert!(tools.contains(&ToolName::RequestContextReset));
        assert!(tools.contains(&ToolName::GetDirective));
        assert_eq!(tools.len(), 4);

        assert!(!tools.contains(&ToolName::GetSessionCheckpoint));
        assert!(!tools.contains(&ToolName::ReportTokenEstimate));
        assert!(!tools.contains(&ToolName::UpdateObjective));
    }

    #[test]
    fn get_directive_available_in_all_phases() {
        for phase in [
            AgentPhase::Starting,
            AgentPhase::Active,
            AgentPhase::Suspending,
        ] {
            let tools = phase_tools(phase);
            assert!(
                tools.contains(&ToolName::GetDirective),
                "GetDirective must be available in {phase:?}"
            );
        }
    }

    #[test]
    fn tool_name_as_str_round_trips() {
        let all = [
            ToolName::GetSessionCheckpoint,
            ToolName::SearchGraph,
            ToolName::GetObjectiveTree,
            ToolName::GetDirective,
            ToolName::ReportTokenEstimate,
            ToolName::StoreMemory,
            ToolName::UpdateObjective,
            ToolName::RequestHumanInput,
            ToolName::SerializeAndPersist,
            ToolName::RequestContextReset,
            ToolName::SpawnAgent,
            ToolName::GetAgentStatus,
            ToolName::GetEventLog,
        ];
        for name in &all {
            let s = name.as_str();
            assert!(!s.is_empty(), "{name:?} must have a non-empty wire name");
        }
        assert_eq!(ToolName::SpawnAgent.as_str(), "spawn_agent");
        assert_eq!(ToolName::GetAgentStatus.as_str(), "get_agent_status");
        assert_eq!(ToolName::GetEventLog.as_str(), "get_event_log");
    }
}
