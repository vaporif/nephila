use meridian_core::id::AgentId;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::Line,
    widgets::{Block, Borders, Paragraph, StatefulWidget, Widget},
};

use crate::layout::focused_border_style;

#[derive(Debug, Clone)]
pub(crate) struct TaskEntry {
    pub(crate) agent_id: AgentId,
    pub(crate) description: String,
}

#[derive(Debug, Default)]
pub struct TasksState {
    pub(crate) entries: Vec<TaskEntry>,
    pub(crate) scroll_offset: usize,
}

pub struct TasksWidget {
    pub focused: bool,
}

impl StatefulWidget for TasksWidget {
    type State = TasksState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let block = Block::default()
            .title(" Tasks ")
            .borders(Borders::ALL)
            .border_style(focused_border_style(self.focused));

        let inner = block.inner(area);
        block.render(area, buf);

        let lines: Vec<Line<'_>> = state
            .entries
            .iter()
            .skip(state.scroll_offset)
            .map(|entry| Line::raw(format!("[{}] {}", entry.agent_id, entry.description)))
            .collect();

        Paragraph::new(lines).render(inner, buf);
    }
}
