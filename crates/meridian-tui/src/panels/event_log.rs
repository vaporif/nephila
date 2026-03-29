use std::collections::VecDeque;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::Line,
    widgets::{Block, Borders, Paragraph, StatefulWidget, Widget},
};

use crate::layout::focused_border_style;

const DEFAULT_MAX_LINES: usize = 1000;

#[derive(Debug)]
pub struct EventLogState {
    pub entries: VecDeque<String>,
    pub max_lines: usize,
    pub scroll_offset: Option<usize>,
}

impl Default for EventLogState {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            max_lines: DEFAULT_MAX_LINES,
            scroll_offset: None,
        }
    }
}

impl EventLogState {
    pub fn push(&mut self, entry: String) {
        if self.entries.len() >= self.max_lines {
            self.entries.pop_front();
            if let Some(offset) = &mut self.scroll_offset {
                *offset = offset.saturating_sub(1);
            }
        }
        self.entries.push_back(entry);
    }

    /// `None` = auto-scroll to bottom, `Some(n)` = pinned at index n.
    pub fn scroll_up(&mut self, amount: usize) {
        let current = self
            .scroll_offset
            .unwrap_or(self.entries.len().saturating_sub(1));
        self.scroll_offset = Some(current.saturating_sub(amount));
    }

    pub fn scroll_down(&mut self, amount: usize) {
        if let Some(offset) = &mut self.scroll_offset {
            *offset += amount;
            if *offset + 1 >= self.entries.len() {
                self.scroll_offset = None;
            }
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = None;
    }
}

pub struct EventLogWidget {
    pub focused: bool,
}

impl StatefulWidget for EventLogWidget {
    type State = EventLogState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let block = Block::default()
            .title(" Event Log ")
            .borders(Borders::ALL)
            .border_style(focused_border_style(self.focused));

        let inner = block.inner(area);
        block.render(area, buf);

        let visible_height = inner.height as usize;
        let total = state.entries.len();

        let start = match state.scroll_offset {
            Some(offset) => offset.min(total.saturating_sub(1)),
            None => total.saturating_sub(visible_height),
        };

        let lines: Vec<Line<'_>> = state
            .entries
            .iter()
            .skip(start)
            .take(visible_height)
            .map(|e| Line::raw(e.as_str()))
            .collect();

        Paragraph::new(lines).render(inner, buf);
    }
}
