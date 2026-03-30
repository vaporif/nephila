use meridian_core::checkpoint::CheckpointSummary;
use meridian_core::id::AgentId;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub enum Modal {
    #[default]
    None,
    HitlResponse {
        agent_id: AgentId,
        question: String,
        options: Vec<String>,
        selected: usize,
    },
    RollbackPicker {
        agent_id: AgentId,
        versions: Vec<CheckpointSummary>,
        selected: usize,
    },
    FilePicker {
        files: Vec<PathBuf>,
        selected: usize,
    },
    View {
        title: String,
        content: String,
    },
    ConfirmDelete {
        path: PathBuf,
        title: String,
    },
    Help,
}

impl Modal {
    pub fn is_open(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn selected_index(&self) -> Option<usize> {
        match self {
            Self::HitlResponse { selected, .. } => Some(*selected),
            Self::RollbackPicker { selected, .. } => Some(*selected),
            Self::FilePicker { selected, .. } => Some(*selected),
            _ => None,
        }
    }

    fn item_count(&self) -> usize {
        match self {
            Self::HitlResponse { options, .. } => options.len(),
            Self::RollbackPicker { versions, .. } => versions.len(),
            Self::FilePicker { files, .. } => files.len(),
            _ => 0,
        }
    }

    pub fn move_up(&mut self) {
        match self {
            Self::HitlResponse { selected, .. }
            | Self::RollbackPicker { selected, .. }
            | Self::FilePicker { selected, .. } => {
                *selected = selected.saturating_sub(1);
            }
            _ => {}
        }
    }

    pub fn move_down(&mut self) {
        let count = self.item_count();
        match self {
            Self::HitlResponse { selected, .. }
            | Self::RollbackPicker { selected, .. }
            | Self::FilePicker { selected, .. } => {
                if *selected + 1 < count {
                    *selected += 1;
                }
            }
            _ => {}
        }
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        if !self.is_open() {
            return;
        }
        let popup_area = centered_rect(60, 50, area);
        Clear.render(popup_area, buf);

        match self {
            Self::HitlResponse {
                agent_id,
                question,
                options,
                selected,
            } => {
                let block = Block::default()
                    .title(format!(" Agent {} asks: ", agent_id))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow));
                let inner = block.inner(popup_area);
                block.render(popup_area, buf);

                let mut lines = vec![Line::from(question.as_str()), Line::from("")];
                for (i, opt) in options.iter().enumerate() {
                    let prefix = if i == *selected { " > " } else { "   " };
                    let style = if i == *selected {
                        Style::default()
                            .add_modifier(Modifier::BOLD)
                            .fg(Color::Cyan)
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(Span::styled(format!("{prefix}{}", opt), style)));
                }
                lines.push(Line::from(""));
                lines.push(Line::styled(
                    " [Enter: Select]  [Esc: Dismiss]",
                    Style::default().fg(Color::DarkGray),
                ));
                Paragraph::new(lines).render(inner, buf);
            }
            Self::RollbackPicker {
                agent_id,
                versions,
                selected,
            } => {
                let block = Block::default()
                    .title(format!(" Rollback {} ", agent_id))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Magenta));
                let inner = block.inner(popup_area);
                block.render(popup_area, buf);

                let mut lines = vec![Line::from("Select checkpoint version:"), Line::from("")];
                for (i, cs) in versions.iter().enumerate() {
                    let prefix = if i == *selected { " > " } else { "   " };
                    let style = if i == *selected {
                        Style::default()
                            .add_modifier(Modifier::BOLD)
                            .fg(Color::Cyan)
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(Span::styled(
                        format!("{prefix}{} — {}", cs.version, cs.summary),
                        style,
                    )));
                }
                lines.push(Line::from(""));
                lines.push(Line::styled(
                    " [Enter: Select]  [Esc: Cancel]",
                    Style::default().fg(Color::DarkGray),
                ));
                Paragraph::new(lines).render(inner, buf);
            }
            Self::FilePicker { files, selected } => {
                let block = Block::default()
                    .title(" Select Goal File ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Green));
                let inner = block.inner(popup_area);
                block.render(popup_area, buf);

                let mut lines = Vec::new();
                for (i, path) in files.iter().enumerate() {
                    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("???");
                    let prefix = if i == *selected { " > " } else { "   " };
                    let style = if i == *selected {
                        Style::default()
                            .add_modifier(Modifier::BOLD)
                            .fg(Color::Cyan)
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(Span::styled(format!("{prefix}{name}"), style)));
                }
                lines.push(Line::from(""));
                lines.push(Line::styled(
                    " [Enter: Select]  [Esc: Cancel]",
                    Style::default().fg(Color::DarkGray),
                ));
                Paragraph::new(lines).render(inner, buf);
            }
            Self::View { title, content } => {
                let block = Block::default()
                    .title(format!(" {title} "))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Blue));
                let inner = block.inner(popup_area);
                block.render(popup_area, buf);

                let mut lines: Vec<Line<'_>> =
                    content.lines().map(|l| Line::raw(l.to_string())).collect();
                lines.push(Line::from(""));
                lines.push(Line::styled(
                    " [Esc: Close]",
                    Style::default().fg(Color::DarkGray),
                ));
                Paragraph::new(lines).render(inner, buf);
            }
            Self::ConfirmDelete { title, .. } => {
                let block = Block::default()
                    .title(" Confirm Delete ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Red));
                let inner = block.inner(popup_area);
                block.render(popup_area, buf);

                let lines = vec![
                    Line::from(""),
                    Line::from(format!("  Delete \"{title}\"?")),
                    Line::from(""),
                    Line::styled(
                        " [Enter: Delete]  [Esc: Cancel]",
                        Style::default().fg(Color::DarkGray),
                    ),
                ];
                Paragraph::new(lines).render(inner, buf);
            }
            Self::Help => {
                let block = Block::default()
                    .title(" Help ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::White));
                let inner = block.inner(popup_area);
                block.render(popup_area, buf);

                let lines = vec![
                    Line::from("  Tab / Shift+Tab  Cycle focus"),
                    Line::from("  Up / Down        Navigate items"),
                    Line::from("  Left / Right     Collapse / expand tree"),
                    Line::from("  s                Spawn agent on objective"),
                    Line::from("  e                Edit objective in $EDITOR"),
                    Line::from("  v                View objective content"),
                    Line::from("  x / Del          Delete objective"),
                    Line::from("  k                Kill agent"),
                    Line::from("  p                Pause / resume agent"),
                    Line::from("  r                Rollback to checkpoint"),
                    Line::from("  n                New objective"),
                    Line::from("  D                Toggle debug log"),
                    Line::from("  Enter            Attach to agent / HITL respond"),
                    Line::from("  ?                Toggle help"),
                    Line::from("  q / Ctrl+C       Quit"),
                    Line::from(""),
                    Line::styled(" [Esc: Close]", Style::default().fg(Color::DarkGray)),
                ];
                Paragraph::new(lines).render(inner, buf);
            }
            Self::None => {}
        }
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use meridian_core::checkpoint::CheckpointSummary;
    use meridian_core::id::{AgentId, CheckpointVersion};

    #[test]
    fn modal_is_open() {
        assert!(!Modal::None.is_open());
        assert!(Modal::Help.is_open());
        assert!(
            Modal::HitlResponse {
                agent_id: AgentId::new(),
                question: "q".into(),
                options: vec!["a".into()],
                selected: 0,
            }
            .is_open()
        );
    }

    #[test]
    fn hitl_modal_navigate() {
        let mut modal = Modal::HitlResponse {
            agent_id: AgentId::new(),
            question: "pick".into(),
            options: vec!["a".into(), "b".into(), "c".into()],
            selected: 0,
        };
        modal.move_down();
        assert_eq!(modal.selected_index(), Some(1));
        modal.move_down();
        assert_eq!(modal.selected_index(), Some(2));
        modal.move_down();
        assert_eq!(modal.selected_index(), Some(2));
        modal.move_up();
        assert_eq!(modal.selected_index(), Some(1));
    }

    #[test]
    fn rollback_picker_navigate() {
        let mut modal = Modal::RollbackPicker {
            agent_id: AgentId::new(),
            versions: vec![
                CheckpointSummary {
                    version: CheckpointVersion(1),
                    timestamp: Utc::now(),
                    summary: "v1".into(),
                },
                CheckpointSummary {
                    version: CheckpointVersion(2),
                    timestamp: Utc::now(),
                    summary: "v2".into(),
                },
            ],
            selected: 0,
        };
        modal.move_down();
        assert_eq!(modal.selected_index(), Some(1));
    }

    #[test]
    fn file_picker_navigate() {
        let mut modal = Modal::FilePicker {
            files: vec![PathBuf::from("a.md"), PathBuf::from("b.md")],
            selected: 0,
        };
        modal.move_down();
        assert_eq!(modal.selected_index(), Some(1));
        modal.move_up();
        assert_eq!(modal.selected_index(), Some(0));
    }
}
