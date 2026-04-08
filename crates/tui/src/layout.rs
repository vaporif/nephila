use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::BorderType;

pub fn focused_border_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

pub fn focused_border_type(focused: bool) -> BorderType {
    if focused {
        BorderType::Thick
    } else {
        BorderType::Plain
    }
}

pub struct AppLayout {
    pub objective_tree: Rect,
    pub agent_tree: Rect,
    pub event_log: Rect,
    pub hotkey_bar: Rect,
}

impl AppLayout {
    pub fn compute_with_focus(area: Rect, event_log_focused: bool) -> Self {
        let vertical = if event_log_focused {
            Layout::vertical([
                Constraint::Length(5),
                Constraint::Min(10),
                Constraint::Length(2),
            ])
            .split(area)
        } else {
            Layout::vertical([
                Constraint::Min(10),
                Constraint::Length(15),
                Constraint::Length(2),
            ])
            .split(area)
        };

        let top_area = vertical[0];
        let event_log = vertical[1];
        let hotkey_bar = vertical[2];

        let horizontal =
            Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
                .split(top_area);

        let objective_tree = horizontal[0];
        let agent_tree = horizontal[1];

        Self {
            objective_tree,
            agent_tree,
            event_log,
            hotkey_bar,
        }
    }
}
