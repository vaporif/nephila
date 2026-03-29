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
        }
    }
}

pub fn phase_tools(phase: AgentPhase) -> Vec<ToolName> {
    match phase {
        AgentPhase::Restoring => vec![
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
        ],
        AgentPhase::Draining => vec![
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
    fn restoring_phase_has_expected_tools() {
        let tools = phase_tools(AgentPhase::Restoring);
        assert!(tools.contains(&ToolName::GetSessionCheckpoint));
        assert!(tools.contains(&ToolName::SearchGraph));
        assert!(tools.contains(&ToolName::GetObjectiveTree));
        assert!(tools.contains(&ToolName::GetDirective));
        assert_eq!(tools.len(), 4);

        // excluded from this phase
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
        assert_eq!(tools.len(), 7);

        // excluded from this phase
        assert!(!tools.contains(&ToolName::GetSessionCheckpoint));
        assert!(!tools.contains(&ToolName::SerializeAndPersist));
        assert!(!tools.contains(&ToolName::RequestContextReset));
    }

    #[test]
    fn draining_phase_has_expected_tools() {
        let tools = phase_tools(AgentPhase::Draining);
        assert!(tools.contains(&ToolName::SerializeAndPersist));
        assert!(tools.contains(&ToolName::StoreMemory));
        assert!(tools.contains(&ToolName::RequestContextReset));
        assert!(tools.contains(&ToolName::GetDirective));
        assert_eq!(tools.len(), 4);

        // excluded from this phase
        assert!(!tools.contains(&ToolName::GetSessionCheckpoint));
        assert!(!tools.contains(&ToolName::ReportTokenEstimate));
        assert!(!tools.contains(&ToolName::UpdateObjective));
    }

    #[test]
    fn get_directive_available_in_all_phases() {
        for phase in [
            AgentPhase::Restoring,
            AgentPhase::Active,
            AgentPhase::Draining,
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
        ];
        for name in &all {
            let s = name.as_str();
            assert!(!s.is_empty(), "{name:?} must have a non-empty wire name");
        }
    }
}
