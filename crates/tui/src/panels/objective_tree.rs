use meridian_core::objective::ObjectiveStatus;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, StatefulWidget, Widget},
};

use crate::goals::{GoalObjective, GoalSubObjective, ObjectiveItem};
use crate::layout::focused_border_style;
use crate::panels::agent_tree::{FlatTreeItem, TreePanelState};

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

#[derive(Debug, Default)]
pub struct ObjectiveTreeState {
    pub panel: TreePanelState<ObjectiveItem>,
    expanded: Vec<bool>,
}

impl ObjectiveTreeState {
    pub fn load_goals(&mut self, goals: &[GoalObjective]) {
        self.expanded.resize(goals.len(), true);
        let flat = flatten_objectives_with_expanded(goals, &self.expanded);
        self.panel.items = flat;
        self.panel.clamp_cursor();
    }

    pub fn collapse(&mut self) {
        if let Some(item) = self.panel.items.get(self.panel.cursor)
            && item.depth == 0
            && item.has_children
            && item.is_expanded
        {
            let root_count = self.panel.items[..self.panel.cursor]
                .iter()
                .filter(|i| i.depth == 0)
                .count();
            if root_count < self.expanded.len() {
                self.expanded[root_count] = false;
            }
        }
    }

    pub fn expand(&mut self) {
        if let Some(item) = self.panel.items.get(self.panel.cursor)
            && item.depth == 0
            && item.has_children
            && !item.is_expanded
        {
            let root_count = self.panel.items[..self.panel.cursor]
                .iter()
                .filter(|i| i.depth == 0)
                .count();
            if root_count < self.expanded.len() {
                self.expanded[root_count] = true;
            }
        }
    }
}

#[cfg(test)]
fn flatten_objectives(goals: &[GoalObjective]) -> Vec<FlatTreeItem<ObjectiveItem>> {
    let expanded = vec![true; goals.len()];
    flatten_objectives_with_expanded(goals, &expanded)
}

fn flatten_objectives_with_expanded(
    goals: &[GoalObjective],
    expanded: &[bool],
) -> Vec<FlatTreeItem<ObjectiveItem>> {
    let mut out = Vec::new();
    for (i, goal) in goals.iter().enumerate() {
        let is_expanded = expanded.get(i).copied().unwrap_or(true);
        out.push(FlatTreeItem {
            data: ObjectiveItem::Root(goal.clone()),
            depth: 0,
            is_expanded,
            has_children: !goal.children.is_empty(),
        });
        if is_expanded {
            flatten_sub_objectives(&goal.children, 1, &mut out);
        }
    }
    out
}

fn flatten_sub_objectives(
    children: &[GoalSubObjective],
    depth: usize,
    out: &mut Vec<FlatTreeItem<ObjectiveItem>>,
) {
    for child in children {
        out.push(FlatTreeItem {
            data: ObjectiveItem::Sub(child.clone()),
            depth,
            is_expanded: true,
            has_children: !child.children.is_empty(),
        });
        flatten_sub_objectives(&child.children, depth + 1, out);
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

        let visible_height = inner.height as usize;

        if state.panel.cursor < state.panel.scroll_offset {
            state.panel.scroll_offset = state.panel.cursor;
        } else if state.panel.cursor >= state.panel.scroll_offset + visible_height {
            state.panel.scroll_offset = state.panel.cursor + 1 - visible_height;
        }

        if state.panel.items.is_empty() {
            let placeholder = Line::styled(
                "  No objectives — press N to create one",
                Style::default().fg(Color::DarkGray),
            );
            Paragraph::new(vec![placeholder]).render(inner, buf);
            return;
        }

        let lines: Vec<Line<'_>> = state
            .panel
            .items
            .iter()
            .enumerate()
            .skip(state.panel.scroll_offset)
            .take(visible_height)
            .map(|(i, item)| {
                let indent = "  ".repeat(item.depth);
                let status = item.data.status();
                let icon = status_icon(&status);
                let color = status_color(&status);
                let title = item.data.title();

                let arrow = if item.has_children {
                    if item.is_expanded {
                        "v "
                    } else {
                        "> "
                    }
                } else {
                    "  "
                };

                let mut spans = vec![
                    Span::raw(format!("{indent}{arrow}")),
                    Span::styled(icon, Style::default().fg(color)),
                    Span::raw(" "),
                    Span::raw(title.to_string()),
                ];

                if let Some(aid) = item.data.agent_id() {
                    spans.push(Span::styled(
                        format!("  ({aid})"),
                        Style::default().fg(Color::DarkGray),
                    ));
                }

                let mut line = Line::from(spans);
                if i == state.panel.cursor {
                    line = line.style(Style::default().add_modifier(Modifier::REVERSED));
                }
                line
            })
            .collect();

        Paragraph::new(lines).render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meridian_core::id::ObjectiveId;
    use meridian_core::objective::ObjectiveStatus;
    use std::path::PathBuf;

    fn make_root(title: &str, status: ObjectiveStatus) -> GoalObjective {
        GoalObjective {
            id: None,
            file_path: PathBuf::from(format!("goals/{title}.md")),
            title: title.into(),
            content: String::new(),
            status,
            agent_id: None,
            children: Vec::new(),
        }
    }

    #[test]
    fn flatten_objectives_produces_correct_items() {
        let goals = vec![make_root("alpha", ObjectiveStatus::Pending), {
            let mut g = make_root("beta", ObjectiveStatus::InProgress);
            g.children.push(GoalSubObjective {
                id: ObjectiveId::new(),
                description: "sub-1".into(),
                status: ObjectiveStatus::Done,
                children: vec![],
            });
            g
        }];

        let flat = flatten_objectives(&goals);
        assert_eq!(flat.len(), 3);
        assert_eq!(flat[0].depth, 0);
        assert_eq!(flat[1].depth, 0);
        assert_eq!(flat[2].depth, 1);
    }

    #[test]
    fn panel_state_cursor_navigation() {
        let goals = vec![
            make_root("a", ObjectiveStatus::Pending),
            make_root("b", ObjectiveStatus::Pending),
            make_root("c", ObjectiveStatus::Pending),
        ];
        let mut state = ObjectiveTreeState::default();
        state.load_goals(&goals);

        assert_eq!(state.panel.cursor, 0);
        state.panel.move_down();
        assert_eq!(state.panel.cursor, 1);
        state.panel.move_down();
        assert_eq!(state.panel.cursor, 2);
        state.panel.move_down();
        assert_eq!(state.panel.cursor, 2);
        state.panel.move_up();
        assert_eq!(state.panel.cursor, 1);
    }
}
