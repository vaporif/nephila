pub mod goals;
pub mod input;
pub mod layout;
pub mod modal;
pub mod panels;
pub mod tui_command;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use meridian_core::event::BusEvent;
use meridian_core::id::AgentId;
use ratatui::{DefaultTerminal, Frame};
use tokio::sync::{broadcast, mpsc};

use crate::goals::{GoalObjective, ObjectiveItem};
use crate::input::FocusPanel;
use crate::layout::AppLayout;
use crate::modal::Modal;
use crate::panels::agent_tree::{AgentTreeState, AgentTreeWidget, TreePanelState};
use crate::panels::event_log::{EventLogState, EventLogWidget};
use crate::panels::hotkey_bar::{HotkeyBarWidget, HotkeyContext};
use crate::panels::objective_tree::{ObjectiveTreeState, ObjectiveTreeWidget};
use crate::tui_command::TuiCommand;

pub struct App {
    event_rx: broadcast::Receiver<BusEvent>,
    cmd_tx: mpsc::Sender<TuiCommand>,
    focus: FocusPanel,
    running: bool,
    modal: Modal,
    needs_clear: bool,
    pending_rollback_agent: Option<AgentId>,
    pending_hitl: HashMap<AgentId, (String, Vec<String>)>,
    goals_dir: PathBuf,
    goals: Vec<GoalObjective>,
    objective_tree: ObjectiveTreeState,
    agent_tree: AgentTreeState,
    event_log: EventLogState,
}

impl App {
    pub fn new(
        event_rx: broadcast::Receiver<BusEvent>,
        cmd_tx: mpsc::Sender<TuiCommand>,
        working_dir: PathBuf,
    ) -> Self {
        let goals_dir = working_dir.join("goals");
        let mut goals = goals::scan_goals_dir(&goals_dir).unwrap_or_default();
        goals::reconcile_with_mapping(&goals_dir, &mut goals);

        let mut objective_tree = ObjectiveTreeState::default();
        objective_tree.load_goals(&goals);

        Self {
            event_rx,
            cmd_tx,
            focus: FocusPanel::default(),
            running: true,
            modal: Modal::default(),
            needs_clear: false,
            pending_rollback_agent: None,
            pending_hitl: HashMap::new(),
            goals_dir,
            goals,
            objective_tree,
            agent_tree: TreePanelState::default(),
            event_log: EventLogState::default(),
        }
    }

    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        while self.running {
            if self.needs_clear {
                self.needs_clear = false;
                terminal.clear()?;
            }
            terminal.draw(|frame| self.draw(frame))?;

            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
            {
                self.handle_key(key).await;
            }

            loop {
                match self.event_rx.try_recv() {
                    Ok(event) => self.handle_bus_event(event),
                    Err(broadcast::error::TryRecvError::Lagged(n)) => {
                        self.event_log
                            .push(format!("Warning: missed {n} events"));
                    }
                    Err(_) => break,
                }
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
            AgentTreeWidget {
                focused: self.focus == FocusPanel::AgentTree,
            },
            layout.agent_tree,
            &mut self.agent_tree,
        );

        frame.render_stateful_widget(
            EventLogWidget {
                focused: self.focus == FocusPanel::EventLog,
            },
            layout.event_log,
            &mut self.event_log,
        );

        let ctx = self.compute_hotkey_context();
        let hitl_hint = self.hitl_hint_text();
        let hotkey_widget = HotkeyBarWidget {
            context: ctx,
            hitl_hint,
        };
        frame.render_widget(hotkey_widget, layout.hotkey_bar);

        self.modal.render(frame.area(), frame.buffer_mut());
    }

    fn compute_hotkey_context(&self) -> HotkeyContext {
        match self.focus {
            FocusPanel::EventLog => HotkeyContext::EventLogFocused,
            FocusPanel::ObjectiveTree => {
                if let Some(item) = self.objective_tree.panel.selected() {
                    if item.data.agent_id().is_some() {
                        HotkeyContext::ObjectiveSelectedWithAgent
                    } else {
                        HotkeyContext::ObjectiveSelectedNoAgent
                    }
                } else {
                    HotkeyContext::ObjectivesPanelNoSelection
                }
            }
            FocusPanel::AgentTree => {
                if let Some(item) = self.agent_tree.selected() {
                    if item.data.hitl_pending {
                        HotkeyContext::AgentSelectedHitlPending
                    } else {
                        HotkeyContext::AgentSelected
                    }
                } else {
                    HotkeyContext::Nothing
                }
            }
        }
    }

    fn hitl_hint_text(&self) -> Option<String> {
        for item in &self.agent_tree.items {
            if item.data.hitl_pending {
                return Some(format!(
                    "> {}: Enter to respond to HITL question",
                    item.data.id
                ));
            }
        }
        None
    }

    async fn handle_key(&mut self, key: KeyEvent) {
        if self.modal.is_open() {
            self.handle_modal_key(key).await;
            return;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.running = false;
            return;
        }

        match key.code {
            KeyCode::Tab => {
                self.focus = self.focus.next();
                return;
            }
            KeyCode::BackTab => {
                self.focus = self.focus.prev();
                return;
            }
            KeyCode::Char('q') => {
                self.running = false;
                return;
            }
            KeyCode::Char('n') => {
                self.handle_new_objective().await;
                return;
            }
            KeyCode::Char('?') => {
                self.modal = Modal::Help;
                return;
            }
            _ => {}
        }

        match self.focus {
            FocusPanel::ObjectiveTree => self.handle_objective_key(key).await,
            FocusPanel::AgentTree => self.handle_agent_tree_key(key).await,
            FocusPanel::EventLog => self.handle_event_log_key(key),
        }
    }

    async fn handle_modal_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.modal = Modal::None;
            }
            KeyCode::Up => self.modal.move_up(),
            KeyCode::Down => self.modal.move_down(),
            KeyCode::Enter => {
                self.confirm_modal().await;
            }
            _ => {}
        }
    }

    async fn confirm_modal(&mut self) {
        let modal = std::mem::take(&mut self.modal);
        match modal {
            Modal::HitlResponse {
                agent_id,
                options,
                selected,
                ..
            } => {
                if let Some(response) = options.get(selected) {
                    if self
                        .cmd_tx
                        .send(TuiCommand::HitlRespond {
                            agent_id,
                            response: response.clone(),
                        })
                        .await
                        .is_err()
                    {
                        self.event_log.push("Command channel closed".into());
                        return;
                    }
                    self.pending_hitl.remove(&agent_id);
                    if let Some(agent) = self.find_agent_mut(&agent_id) {
                        agent.hitl_pending = false;
                    }
                }
            }
            Modal::RollbackPicker {
                agent_id,
                versions,
                selected,
            } => {
                if let Some(cs) = versions.get(selected)
                    && self
                        .send_command(TuiCommand::Rollback {
                            agent_id,
                            version: cs.version,
                        })
                        .await
                {
                    self.event_log.push(format!(
                        "[{agent_id}] Rollback to {} requested",
                        cs.version
                    ));
                }
            }
            Modal::FilePicker { files, selected } => {
                if let Some(path) = files.get(selected)
                    && let Some(goal) = self.goals.iter().find(|g| g.file_path == *path)
                {
                    if let Some(oid) = goal.id {
                        self.send_command(TuiCommand::Spawn {
                            objective_id: oid,
                            content: goal.content.clone(),
                            dir: self
                                .goals_dir
                                .parent()
                                .unwrap_or(Path::new("."))
                                .to_path_buf(),
                        })
                        .await;
                    } else {
                        self.event_log
                            .push("Objective not yet registered — try again shortly".into());
                    }
                }
            }
            Modal::Help | Modal::None => {}
        }
    }

    async fn handle_new_objective(&mut self) {
        match goals::create_template_file(&self.goals_dir) {
            Ok(path) => {
                let _ = crossterm::terminal::disable_raw_mode();
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::terminal::LeaveAlternateScreen
                );

                let edit_result =
                    tokio::task::spawn_blocking(move || goals::open_in_editor(&path)).await;

                crossterm::terminal::enable_raw_mode().ok();
                crossterm::execute!(
                    std::io::stdout(),
                    crossterm::terminal::EnterAlternateScreen
                )
                .ok();
                self.needs_clear = true;

                match edit_result {
                    Ok(Ok(true)) => {
                        self.event_log.push("New objective created".into());
                    }
                    _ => {
                        self.event_log.push("Editor closed without saving".into());
                    }
                }

                self.goals = goals::scan_goals_dir(&self.goals_dir).unwrap_or_default();
                goals::reconcile_with_mapping(&self.goals_dir, &mut self.goals);
                self.objective_tree.load_goals(&self.goals);
            }
            Err(e) => {
                self.event_log
                    .push(format!("Failed to create objective: {e}"));
            }
        }
    }

    async fn handle_objective_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.objective_tree.panel.move_up(),
            KeyCode::Down => self.objective_tree.panel.move_down(),
            KeyCode::Left => {
                self.objective_tree.collapse();
                self.objective_tree.load_goals(&self.goals);
            }
            KeyCode::Right => {
                self.objective_tree.expand();
                self.objective_tree.load_goals(&self.goals);
            }
            KeyCode::Char('s') => {
                self.handle_spawn().await;
            }
            KeyCode::Char('k') => {
                self.handle_kill_from_objective().await;
            }
            KeyCode::Char('p') => {
                self.handle_pause_from_objective().await;
            }
            KeyCode::Char('r') => {
                self.handle_rollback_from_objective().await;
            }
            KeyCode::Enter => {
                self.handle_hitl_from_objective();
            }
            _ => {}
        }
    }

    async fn handle_spawn(&mut self) {
        let selected_data = self.objective_tree.panel.selected().map(|item| {
            (
                item.data.objective_id(),
                item.data.agent_id(),
                match &item.data {
                    ObjectiveItem::Root(g) => g.content.clone(),
                    ObjectiveItem::Sub(s) => s.description.clone(),
                },
            )
        });

        match selected_data {
            Some((oid, agent, content)) => {
                if agent.is_some() {
                    self.event_log
                        .push("Objective already has an active agent".into());
                    return;
                }

                match oid {
                    Some(id) => {
                        self.send_command(TuiCommand::Spawn {
                            objective_id: id,
                            content,
                            dir: self
                                .goals_dir
                                .parent()
                                .unwrap_or(Path::new("."))
                                .to_path_buf(),
                        })
                        .await;
                    }
                    None => {
                        self.event_log
                            .push("Objective not yet registered — try again shortly".into());
                    }
                }
            }
            None => {
                let files: Vec<PathBuf> =
                    self.goals.iter().map(|g| g.file_path.clone()).collect();
                if files.is_empty() {
                    self.event_log
                        .push("No goal files found — press N to create one".into());
                } else {
                    self.modal = Modal::FilePicker { files, selected: 0 };
                }
            }
        }
    }

    async fn handle_kill_from_objective(&mut self) {
        let aid = self
            .objective_tree
            .panel
            .selected()
            .and_then(|item| item.data.agent_id());
        if let Some(aid) = aid {
            self.send_command(TuiCommand::Kill { agent_id: aid }).await;
        }
    }

    async fn handle_pause_from_objective(&mut self) {
        let aid = self
            .objective_tree
            .panel
            .selected()
            .and_then(|item| item.data.agent_id());
        if let Some(aid) = aid {
            let is_paused = self
                .agent_tree
                .items
                .iter()
                .find(|i| i.data.id == aid)
                .map(|i| i.data.state == meridian_core::agent::AgentState::Paused)
                .unwrap_or(false);

            let cmd = if is_paused {
                TuiCommand::Resume { agent_id: aid }
            } else {
                TuiCommand::Pause { agent_id: aid }
            };
            self.send_command(cmd).await;
        }
    }

    async fn handle_rollback_from_objective(&mut self) {
        let aid = self
            .objective_tree
            .panel
            .selected()
            .and_then(|item| item.data.agent_id());
        if let Some(aid) = aid {
            self.initiate_rollback(aid).await;
        }
    }

    async fn initiate_rollback(&mut self, aid: AgentId) {
        if self.pending_rollback_agent.is_some() {
            return;
        }
        self.pending_rollback_agent = Some(aid);
        if self
            .cmd_tx
            .send(TuiCommand::ListCheckpoints { agent_id: aid })
            .await
            .is_err()
        {
            self.pending_rollback_agent = None;
            self.event_log.push("Command channel closed".into());
        }
    }

    fn handle_hitl_from_objective(&mut self) {
        let aid = self
            .objective_tree
            .panel
            .selected()
            .and_then(|item| item.data.agent_id());
        if let Some(aid) = aid {
            let is_hitl = self
                .agent_tree
                .items
                .iter()
                .any(|i| i.data.id == aid && i.data.hitl_pending);
            if is_hitl {
                self.try_open_hitl_modal(aid);
            }
        }
    }

    async fn handle_agent_tree_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up => self.agent_tree.move_up(),
            KeyCode::Down => self.agent_tree.move_down(),
            KeyCode::Left | KeyCode::Right => {}
            KeyCode::Char('k') => {
                let aid = self.agent_tree.selected().map(|item| item.data.id);
                if let Some(aid) = aid {
                    self.send_command(TuiCommand::Kill { agent_id: aid }).await;
                }
            }
            KeyCode::Char('p') => {
                let selected = self.agent_tree.selected().map(|item| {
                    (
                        item.data.id,
                        item.data.state == meridian_core::agent::AgentState::Paused,
                    )
                });
                if let Some((aid, is_paused)) = selected {
                    let cmd = if is_paused {
                        TuiCommand::Resume { agent_id: aid }
                    } else {
                        TuiCommand::Pause { agent_id: aid }
                    };
                    self.send_command(cmd).await;
                }
            }
            KeyCode::Char('r') => {
                let aid = self.agent_tree.selected().map(|item| item.data.id);
                if let Some(aid) = aid {
                    self.initiate_rollback(aid).await;
                }
            }
            KeyCode::Enter => {
                let hitl_data = self
                    .agent_tree
                    .selected()
                    .and_then(|item| {
                        if item.data.hitl_pending {
                            Some(item.data.id)
                        } else {
                            None
                        }
                    });
                if let Some(aid) = hitl_data {
                    self.try_open_hitl_modal(aid);
                }
            }
            _ => {}
        }
    }

    async fn send_command(&mut self, cmd: TuiCommand) -> bool {
        if self.cmd_tx.send(cmd).await.is_err() {
            self.event_log.push("Command channel closed".into());
            return false;
        }
        true
    }

    fn find_agent_mut(&mut self, agent_id: &AgentId) -> Option<&mut crate::panels::agent_tree::AgentTreeNode> {
        self.agent_tree
            .items
            .iter_mut()
            .find(|i| i.data.id == *agent_id)
            .map(|i| &mut i.data)
    }

    fn try_open_hitl_modal(&mut self, agent_id: AgentId) {
        if let Some((question, options)) = self.pending_hitl.get(&agent_id) {
            self.modal = Modal::HitlResponse {
                agent_id,
                question: question.clone(),
                options: options.clone(),
                selected: 0,
            };
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
                if let Some(agent) = self.find_agent_mut(agent_id) {
                    agent.state = *new_state;
                }
                self.event_log
                    .push(format!("[{agent_id}] state -> {new_state}"));
            }
            BusEvent::TokenReport {
                agent_id,
                used,
                remaining,
            } => {
                if let Some(agent) = self.find_agent_mut(agent_id) {
                    agent.tokens_used = Some(*used);
                    agent.tokens_remaining = Some(*remaining);
                }
            }
            BusEvent::HitlRequested {
                agent_id,
                question,
                options,
            } => {
                if let Some(agent) = self.find_agent_mut(agent_id) {
                    agent.hitl_pending = true;
                }
                self.pending_hitl
                    .insert(*agent_id, (question.clone(), options.clone()));
                self.event_log.push(format!(
                    "[{agent_id}] ! HITL: {question} ({})",
                    options.join(", ")
                ));
            }
            BusEvent::HitlResponded {
                agent_id,
                response,
            } => {
                self.pending_hitl.remove(agent_id);
                if let Some(agent) = self.find_agent_mut(agent_id) {
                    agent.hitl_pending = false;
                }
                self.event_log
                    .push(format!("[{agent_id}] HITL response: {response}"));
            }
            BusEvent::CheckpointList {
                agent_id,
                versions,
            } => {
                if self.pending_rollback_agent == Some(*agent_id) {
                    self.pending_rollback_agent = None;
                    if versions.is_empty() {
                        self.event_log
                            .push(format!("[{agent_id}] No checkpoints available"));
                    } else {
                        self.modal = Modal::RollbackPicker {
                            agent_id: *agent_id,
                            versions: versions.clone(),
                            selected: 0,
                        };
                    }
                }
            }
            BusEvent::Shutdown => {
                self.event_log.push("Shutdown signal received".into());
                self.running = false;
            }
        }
    }
}
