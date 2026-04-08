#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPanel {
    #[default]
    ObjectiveTree,
    AgentTree,
    EventLog,
}

impl FocusPanel {
    pub fn next(self) -> Self {
        match self {
            Self::ObjectiveTree => Self::AgentTree,
            Self::AgentTree => Self::EventLog,
            Self::EventLog => Self::ObjectiveTree,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::ObjectiveTree => Self::EventLog,
            Self::AgentTree => Self::ObjectiveTree,
            Self::EventLog => Self::AgentTree,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_cycle_forward() {
        let start = FocusPanel::ObjectiveTree;
        assert_eq!(start.next(), FocusPanel::AgentTree);
        assert_eq!(start.next().next(), FocusPanel::EventLog);
        assert_eq!(start.next().next().next(), FocusPanel::ObjectiveTree);
    }

    #[test]
    fn focus_cycle_backward() {
        let start = FocusPanel::ObjectiveTree;
        assert_eq!(start.prev(), FocusPanel::EventLog);
        assert_eq!(start.prev().prev(), FocusPanel::AgentTree);
        assert_eq!(start.prev().prev().prev(), FocusPanel::ObjectiveTree);
    }

    #[test]
    fn default_is_objective_tree() {
        assert_eq!(FocusPanel::default(), FocusPanel::ObjectiveTree);
    }
}
