use meridian_core::agent::AgentState;
use meridian_core::id::AgentId;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Row, StatefulWidget, Table, Widget},
};

use crate::layout::focused_border_style;

fn state_color(state: &AgentState) -> Color {
    match state {
        AgentState::Active => Color::Green,
        AgentState::Starting | AgentState::Restoring => Color::Yellow,
        AgentState::Draining | AgentState::Paused => Color::Cyan,
        AgentState::Exited | AgentState::Completed => Color::Gray,
        AgentState::Failed => Color::Red,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AgentRow {
    pub(crate) id: AgentId,
    pub(crate) state: AgentState,
    pub(crate) tokens_used: u64,
    pub(crate) tokens_remaining: u64,
    pub(crate) objective: String,
    pub(crate) checkpoint_version: Option<u32>,
}

#[derive(Debug, Default)]
pub struct AgentStatusState {
    pub(crate) agents: Vec<AgentRow>,
}

impl AgentStatusState {
    pub fn update_agent_state(&mut self, agent_id: &AgentId, new_state: AgentState) {
        if let Some(row) = self.agents.iter_mut().find(|r| r.id == *agent_id) {
            row.state = new_state;
        }
    }

    pub fn update_tokens(&mut self, agent_id: &AgentId, used: u64, remaining: u64) {
        if let Some(row) = self.agents.iter_mut().find(|r| r.id == *agent_id) {
            row.tokens_used = used;
            row.tokens_remaining = remaining;
        }
    }
}

pub struct AgentStatusWidget {
    pub focused: bool,
}

impl StatefulWidget for AgentStatusWidget {
    type State = AgentStatusState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let block = Block::default()
            .title(" Agents ")
            .borders(Borders::ALL)
            .border_style(focused_border_style(self.focused));

        let header = Row::new(["ID", "State", "Tokens", "Objective", "Checkpoint"])
            .style(Style::default().add_modifier(Modifier::BOLD))
            .bottom_margin(1);

        let rows: Vec<Row<'_>> = state
            .agents
            .iter()
            .map(|agent| {
                let color = state_color(&agent.state);
                let ckpt = agent
                    .checkpoint_version
                    .map(|v| format!("v{v}"))
                    .unwrap_or_default();
                Row::new([
                    agent.id.to_string(),
                    agent.state.to_string(),
                    format!("{}/{}", agent.tokens_used, agent.tokens_remaining),
                    agent.objective.clone(),
                    ckpt,
                ])
                .style(Style::default().fg(color))
            })
            .collect();

        let widths = [
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(15),
            Constraint::Min(20),
            Constraint::Length(10),
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .block(block)
            .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

        Widget::render(table, area, buf);
    }
}
