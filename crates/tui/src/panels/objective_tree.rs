use meridian_core::objective::{ObjectiveNode, ObjectiveStatus};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, StatefulWidget, Widget},
};

use crate::layout::focused_border_style;

fn status_icon(status: &ObjectiveStatus) -> &'static str {
    match status {
        ObjectiveStatus::Done => "[x]",
        ObjectiveStatus::InProgress => "[~]",
        ObjectiveStatus::Pending => "[ ]",
        ObjectiveStatus::Blocked => "[!]",
    }
}

fn status_color(status: &ObjectiveStatus) -> Color {
    match status {
        ObjectiveStatus::Done => Color::Green,
        ObjectiveStatus::InProgress => Color::Yellow,
        ObjectiveStatus::Pending => Color::Gray,
        ObjectiveStatus::Blocked => Color::Red,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct FlatNode {
    pub(crate) description: String,
    pub(crate) status: ObjectiveStatus,
    pub(crate) indent: usize,
}

#[derive(Debug, Default)]
pub struct ObjectiveTreeState {
    pub(crate) nodes: Vec<FlatNode>,
    pub(crate) scroll_offset: usize,
}

impl ObjectiveTreeState {
    pub fn load_tree(&mut self, root: &ObjectiveNode) {
        self.nodes.clear();
        self.flatten(root, 0);
    }

    fn flatten(&mut self, node: &ObjectiveNode, depth: usize) {
        self.nodes.push(FlatNode {
            description: node.description.clone(),
            status: node.status,
            indent: depth,
        });
        for child in &node.children {
            self.flatten(child, depth + 1);
        }
    }
}

pub struct ObjectiveTreeWidget {
    pub focused: bool,
}

impl StatefulWidget for ObjectiveTreeWidget {
    type State = ObjectiveTreeState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let block = Block::default()
            .title(" Objectives ")
            .borders(Borders::ALL)
            .border_style(focused_border_style(self.focused));

        let inner = block.inner(area);
        block.render(area, buf);

        let lines: Vec<Line<'_>> = state
            .nodes
            .iter()
            .skip(state.scroll_offset)
            .map(|node| {
                let indent = "  ".repeat(node.indent);
                let icon = status_icon(&node.status);
                let color = status_color(&node.status);
                Line::from(vec![
                    Span::raw(indent),
                    Span::styled(icon, Style::default().fg(color)),
                    Span::raw(" "),
                    Span::raw(&node.description),
                ])
            })
            .collect();

        Paragraph::new(lines).render(inner, buf);
    }
}
