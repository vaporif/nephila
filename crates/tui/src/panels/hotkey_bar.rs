use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use crate::input::FocusPanel;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyContext {
    Nothing,
    ObjectivesPanelNoSelection,
    ObjectiveSelectedNoAgent,
    ObjectiveSelectedWithAgent,
    AgentSelected,
    AgentSelectedHitlPending,
    EventLogFocused,
}

pub fn context_line(ctx: &HotkeyContext) -> &'static str {
    match ctx {
        HotkeyContext::Nothing => "",
        HotkeyContext::ObjectivesPanelNoSelection => "S:Spawn (file picker)",
        HotkeyContext::ObjectiveSelectedNoAgent => "S:Spawn  E:Edit  V:View  X:Delete",
        HotkeyContext::ObjectiveSelectedWithAgent => "K:Kill  P:Pause  Enter:HITL  E:Edit  V:View",
        HotkeyContext::AgentSelected => "Enter:Attach  K:Kill  P:Pause",
        HotkeyContext::AgentSelectedHitlPending => "Enter:Respond to question  K:Kill  P:Pause",
        HotkeyContext::EventLogFocused => "Up/Down:Scroll  Home/End:Jump",
    }
}

pub struct HotkeyBarWidget {
    pub context: HotkeyContext,
    pub hitl_hint: Option<String>,
    pub focus: FocusPanel,
}

impl Widget for HotkeyBarWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 {
            return;
        }

        let active_style = Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let inactive_style = Style::default().fg(Color::DarkGray);
        let sep_style = Style::default().fg(Color::DarkGray);

        let tab = |label: &'static str, panel: FocusPanel| -> Span<'static> {
            if self.focus == panel {
                Span::styled(format!(" {label} "), active_style)
            } else {
                Span::styled(format!(" {label} "), inactive_style)
            }
        };

        let spans = vec![
            tab("Objectives", FocusPanel::ObjectiveTree),
            Span::styled(" | ", sep_style),
            tab("Agents", FocusPanel::AgentTree),
            Span::styled(" | ", sep_style),
            tab("Event Log", FocusPanel::EventLog),
            Span::styled("    ", sep_style),
            Span::styled(
                "N",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":New ", sep_style),
            Span::styled(
                "?",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":Help ", sep_style),
            Span::styled(
                "Q",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":Quit", sep_style),
        ];

        let line1 = Line::from(spans);

        let ctx_text = self
            .hitl_hint
            .as_deref()
            .unwrap_or_else(|| context_line(&self.context));

        let line2 = if ctx_text.is_empty() {
            Line::from("")
        } else {
            Line::styled(format!(" {ctx_text}"), Style::default().fg(Color::Yellow))
        };

        Paragraph::new(vec![line1, line2]).render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_hint_changes_by_context() {
        let none = HotkeyContext::Nothing;
        let obj_no_agent = HotkeyContext::ObjectiveSelectedNoAgent;
        let hitl = HotkeyContext::AgentSelectedHitlPending;
        let log = HotkeyContext::EventLogFocused;

        assert!(context_line(&none).is_empty());
        assert!(context_line(&obj_no_agent).contains("S:Spawn"));
        assert!(context_line(&hitl).contains("Enter:Respond"));
        assert!(context_line(&log).contains("Up/Down:Scroll"));
    }
}
