use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

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
        HotkeyContext::ObjectiveSelectedNoAgent => "S:Spawn agent on this objective",
        HotkeyContext::ObjectiveSelectedWithAgent => "K:Kill  P:Pause  R:Rollback  Enter:HITL",
        HotkeyContext::AgentSelected => "K:Kill  P:Pause  R:Rollback",
        HotkeyContext::AgentSelectedHitlPending => {
            "Enter:Respond to question  K:Kill  P:Pause  R:Rollback"
        }
        HotkeyContext::EventLogFocused => "Up/Down:Scroll  Home/End:Jump",
    }
}

pub struct HotkeyBarWidget {
    pub context: HotkeyContext,
    pub hitl_hint: Option<String>,
}

impl Widget for HotkeyBarWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 {
            return;
        }

        let key_style = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let sep_style = Style::default().fg(Color::DarkGray);

        let line1 = Line::from(vec![
            Span::styled("Tab", key_style),
            Span::styled(":Focus  ", sep_style),
            Span::styled("N", key_style),
            Span::styled(":New  ", sep_style),
            Span::styled("?", key_style),
            Span::styled(":Help  ", sep_style),
            Span::styled("Q", key_style),
            Span::styled(":Quit", sep_style),
        ]);

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
