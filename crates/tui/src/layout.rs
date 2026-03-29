use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};

pub fn focused_border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    }
}

pub struct AppLayout {
    pub objective_tree: Rect,
    pub agent_status: Rect,
    pub tasks: Rect,
    pub event_log: Rect,
    pub command_bar: Rect,
}

impl AppLayout {
    pub fn compute(area: Rect) -> Self {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(10),
                Constraint::Length(15),
                Constraint::Length(3),
            ])
            .split(area);

        let main_area = vertical[0];
        let event_log = vertical[1];
        let command_bar = vertical[2];

        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(main_area);

        let objective_tree = horizontal[0];

        let right_side = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(horizontal[1]);

        let agent_status = right_side[0];
        let tasks = right_side[1];

        Self {
            objective_tree,
            agent_status,
            tasks,
            event_log,
            command_bar,
        }
    }
}
