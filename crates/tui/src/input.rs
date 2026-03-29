#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPanel {
    ObjectiveTree,
    AgentStatus,
    Tasks,
    EventLog,
    #[default]
    CommandBar,
}

impl FocusPanel {
    pub fn next(self) -> Self {
        match self {
            Self::ObjectiveTree => Self::AgentStatus,
            Self::AgentStatus => Self::Tasks,
            Self::Tasks => Self::EventLog,
            Self::EventLog => Self::CommandBar,
            Self::CommandBar => Self::ObjectiveTree,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::ObjectiveTree => Self::CommandBar,
            Self::AgentStatus => Self::ObjectiveTree,
            Self::Tasks => Self::AgentStatus,
            Self::EventLog => Self::Tasks,
            Self::CommandBar => Self::EventLog,
        }
    }
}
