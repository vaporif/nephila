pub mod commands;
pub mod input;
pub mod layout;
pub mod panels;

use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use meridian_core::event::BusEvent;
use ratatui::{
    widgets::{Block, Borders, Paragraph},
    DefaultTerminal, Frame,
};
use tokio::sync::broadcast;

use crate::input::FocusPanel;
use crate::layout::AppLayout;
use crate::panels::agent_status::{AgentStatusState, AgentStatusWidget};
use crate::panels::event_log::{EventLogState, EventLogWidget};
use crate::panels::objective_tree::{ObjectiveTreeState, ObjectiveTreeWidget};
use crate::panels::tasks::{TasksState, TasksWidget};

pub struct App {
    event_rx: broadcast::Receiver<BusEvent>,
    focus: FocusPanel,
    command_input: String,
    running: bool,
    objective_tree: ObjectiveTreeState,
    agent_status: AgentStatusState,
    tasks: TasksState,
    event_log: EventLogState,
}

impl App {
    pub fn new(event_rx: broadcast::Receiver<BusEvent>) -> Self {
        Self {
            event_rx,
            focus: FocusPanel::default(),
            command_input: String::new(),
            running: true,
            objective_tree: ObjectiveTreeState::default(),
            agent_status: AgentStatusState::default(),
            tasks: TasksState::default(),
            event_log: EventLogState::default(),
        }
    }

    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        while self.running {
            terminal.draw(|frame| self.draw(frame))?;

            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
            {
                self.handle_key(key);
            }

            while let Ok(bus_event) = self.event_rx.try_recv() {
                self.handle_bus_event(bus_event);
            }
        }
        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let layout = AppLayout::compute(frame.area());

        frame.render_stateful_widget(
            ObjectiveTreeWidget {
                focused: self.focus == FocusPanel::ObjectiveTree,
            },
            layout.objective_tree,
            &mut self.objective_tree,
        );

        frame.render_stateful_widget(
            AgentStatusWidget {
                focused: self.focus == FocusPanel::AgentStatus,
            },
            layout.agent_status,
            &mut self.agent_status,
        );

        frame.render_stateful_widget(
            TasksWidget {
                focused: self.focus == FocusPanel::Tasks,
            },
            layout.tasks,
            &mut self.tasks,
        );

        frame.render_stateful_widget(
            EventLogWidget {
                focused: self.focus == FocusPanel::EventLog,
            },
            layout.event_log,
            &mut self.event_log,
        );

        let cmd_focused = self.focus == FocusPanel::CommandBar;
        let cmd_block = Block::default()
            .title(" Command ")
            .borders(Borders::ALL)
            .border_style(layout::focused_border_style(cmd_focused));
        let cmd_text = format!("> {}", self.command_input);
        let cmd_widget = Paragraph::new(cmd_text).block(cmd_block);
        frame.render_widget(cmd_widget, layout.command_bar);
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.running = false;
            return;
        }

        if key.code == KeyCode::Tab {
            self.focus = self.focus.next();
            return;
        }
        if key.code == KeyCode::BackTab {
            self.focus = self.focus.prev();
            return;
        }

        match self.focus {
            FocusPanel::CommandBar => self.handle_command_key(key),
            FocusPanel::EventLog => self.handle_event_log_key(key),
            _ => {}
        }
    }

    fn handle_command_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(c) => {
                self.command_input.push(c);
            }
            KeyCode::Backspace => {
                self.command_input.pop();
            }
            KeyCode::Enter => {
                let input = std::mem::take(&mut self.command_input);
                self.execute_command(&input);
            }
            KeyCode::Esc => {
                self.command_input.clear();
            }
            _ => {}
        }
    }

    fn handle_event_log_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.event_log.scroll_up(1),
            KeyCode::Down => self.event_log.scroll_down(1),
            KeyCode::Home => {
                self.event_log.scroll_offset = Some(0);
            }
            KeyCode::End => self.event_log.scroll_to_bottom(),
            _ => {}
        }
    }

    fn execute_command(&mut self, input: &str) {
        match commands::parse(input) {
            Ok(cmd) => match cmd {
                commands::Command::Quit => {
                    self.running = false;
                }
                commands::Command::Help => {
                    self.event_log.push(
                        "Commands: spawn, kill, pause, resume, rollback, respond, inject, tree, filter, help, quit"
                            .into(),
                    );
                }
                commands::Command::Tree => {
                    self.event_log.push("Refreshing objective tree...".into());
                }
                other => {
                    self.event_log
                        .push(format!("Command accepted: {other:?}"));
                }
            },
            Err(e) => {
                self.event_log.push(format!("Error: {e}"));
            }
        }
    }

    fn handle_bus_event(&mut self, event: BusEvent) {
        match &event {
            BusEvent::Mcp(mcp) => {
                self.event_log
                    .push(format!("[{}] {:?}", mcp.agent_id, mcp.event_type));
            }
            BusEvent::AgentStateChanged {
                agent_id,
                old_state: _,
                new_state,
            } => {
                self.agent_status
                    .update_agent_state(agent_id, *new_state);
                self.event_log
                    .push(format!("[{agent_id}] state -> {new_state}"));
            }
            BusEvent::TokenReport {
                agent_id,
                used,
                remaining,
            } => {
                self.agent_status
                    .update_tokens(agent_id, *used, *remaining);
            }
            BusEvent::HitlRequested {
                agent_id,
                question,
                options,
            } => {
                self.event_log.push(format!(
                    "[{agent_id}] HITL: {question} (options: {})",
                    options.join(", ")
                ));
            }
            BusEvent::HitlResponded {
                agent_id,
                response,
            } => {
                self.event_log
                    .push(format!("[{agent_id}] HITL response: {response}"));
            }
            BusEvent::Shutdown => {
                self.event_log.push("Shutdown signal received".into());
                self.running = false;
            }
        }
    }
}
