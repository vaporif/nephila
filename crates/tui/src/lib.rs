pub mod goals;
pub mod input;
pub mod layout;
pub mod modal;
pub mod panels;
pub mod tui_command;
pub mod tui_tracing;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use nephila_core::event::BusEvent;
use nephila_core::id::AgentId;
use ratatui::{DefaultTerminal, Frame};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc};

use crate::goals::{GoalObjective, ObjectiveItem};
use crate::input::FocusPanel;
use crate::layout::{AppLayout, AppLayoutWithSession};
use crate::modal::Modal;
use crate::panels::agent_tree::{
    AgentActivityUpdate, AgentTreeState, AgentTreeWidget, TreePanelState,
};
use crate::panels::event_log::{EventLogState, EventLogWidget};
use crate::panels::hotkey_bar::{HotkeyBarWidget, HotkeyContext};
use crate::panels::objective_tree::{ObjectiveTreeState, ObjectiveTreeWidget};
use crate::panels::session_pane::SessionPane;
use crate::panels::session_pane::pump::HitlRequest;
use crate::tui_command::TuiCommand;
use crate::tui_tracing::TuiLogBuffer;

#[derive(Debug, Error)]
pub enum TuiCommandError {
    #[error("command channel closed")]
    ChannelClosed,
}

/// Bound on the activity-update channel. Pumps publish a glyph per
/// `SessionEvent`; the App drains the receiver before each render. Drops on
/// full are acceptable per spec — only the latest glyph matters.
pub const ACTIVITY_CHANNEL_BOUND: usize = 16;

pub struct App {
    event_rx: broadcast::Receiver<BusEvent>,
    cmd_tx: mpsc::Sender<TuiCommand>,
    claude_binary: String,
    focus: FocusPanel,
    running: bool,
    modal: Modal,
    needs_clear: bool,
    show_debug: bool,
    debug_scroll: usize,
    debug_log: TuiLogBuffer,
    pending_hitl: HashMap<AgentId, (String, Vec<String>)>,
    goals_dir: PathBuf,
    goals: Vec<GoalObjective>,
    objective_tree: ObjectiveTreeState,
    agent_tree: AgentTreeState,
    event_log: EventLogState,
    /// When `Some`, the embedded `SessionPane` is the focused panel and the
    /// layout switches to the three-column variant.
    session_focus: Option<AgentId>,
    /// The embedded session panel. Today there is at most one; future work
    /// may swap this for a per-agent pane index keyed by `AgentId`.
    session_pane: SessionPane,
    /// Receiver drained before each render. The App owns the `Sender` half
    /// (cloned to each pump task on `SessionStarted`); the `SessionRegistry`
    /// is expected to take ownership of the sender in a later iteration.
    activity_rx: mpsc::Receiver<AgentActivityUpdate>,
    activity_tx: mpsc::Sender<AgentActivityUpdate>,
    /// HITL request channel: pumps push when they observe a
    /// `CheckpointReached(Hitl)`; the tick loop drains and opens the modal.
    hitl_rx: mpsc::Receiver<HitlRequest>,
    hitl_tx: mpsc::Sender<HitlRequest>,
}

impl App {
    pub fn new(
        event_rx: broadcast::Receiver<BusEvent>,
        cmd_tx: mpsc::Sender<TuiCommand>,
        working_dir: PathBuf,
        debug_log: TuiLogBuffer,
        claude_binary: String,
    ) -> Self {
        let goals_dir = working_dir.join("goals");
        let mut goals = goals::scan_goals_dir(&goals_dir).unwrap_or_default();
        goals::reconcile_with_mapping(&goals_dir, &mut goals);

        let mut objective_tree = ObjectiveTreeState::default();
        objective_tree.load_goals(&goals);

        let (activity_tx, activity_rx) = mpsc::channel(ACTIVITY_CHANNEL_BOUND);
        let (hitl_tx, hitl_rx) = mpsc::channel(ACTIVITY_CHANNEL_BOUND);
        Self {
            event_rx,
            cmd_tx,
            claude_binary,
            focus: FocusPanel::default(),
            running: true,
            modal: Modal::default(),
            needs_clear: false,
            show_debug: false,
            debug_scroll: 0,
            debug_log,
            pending_hitl: HashMap::new(),
            goals_dir,
            goals,
            objective_tree,
            agent_tree: TreePanelState::default(),
            event_log: EventLogState::default(),
            session_focus: None,
            session_pane: SessionPane::new(),
            activity_rx,
            activity_tx,
            hitl_rx,
            hitl_tx,
        }
    }

    /// Get a fresh `Sender<HitlRequest>` for a per-agent pump.
    #[must_use]
    pub fn hitl_sender(&self) -> mpsc::Sender<HitlRequest> {
        self.hitl_tx.clone()
    }

    /// Get a fresh `Sender<AgentActivityUpdate>` to hand to a per-agent pump.
    /// Cloned per pump; drops on a full channel are acceptable.
    #[must_use]
    pub fn activity_sender(&self) -> mpsc::Sender<AgentActivityUpdate> {
        self.activity_tx.clone()
    }

    /// Open the existing HITL modal for the given agent. Used by the pump
    /// when a `CheckpointReached(Hitl{question, options})` event arrives.
    pub fn open_hitl_modal(&mut self, agent_id: AgentId, question: String, options: Vec<String>) {
        if let Some(agent) = self.find_agent_mut(&agent_id) {
            agent.hitl_pending = true;
        }
        self.pending_hitl
            .insert(agent_id, (question.clone(), options.clone()));
        self.modal = Modal::HitlResponse {
            agent_id,
            question,
            options,
            selected: 0,
        };
    }

    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        let mut term_events = EventStream::new();
        while self.running {
            if self.needs_clear {
                self.needs_clear = false;
                terminal.clear()?;
            }
            terminal.draw(|frame| self.draw(frame))?;

            tokio::select! {
                biased;

                maybe_term = term_events.next() => match maybe_term {
                    Some(Ok(Event::Key(key))) => self.handle_key(key).await,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        self.event_log.push(format!("Terminal event error: {e}"));
                    }
                    None => self.running = false,
                },
                Some(update) = self.activity_rx.recv() => {
                    if let Some(agent) = self.find_agent_mut(&update.agent_id) {
                        agent.last_session_event = Some(update.last_event_kind);
                        agent.activity_glyph = Some(update.glyph);
                    }
                }
                Some(req) = self.hitl_rx.recv() => self.handle_hitl_request(req),
                bus = self.event_rx.recv() => match bus {
                    Ok(event) => self.handle_bus_event(event),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        self.event_log.push(format!("Warning: missed {n} events"));
                    }
                    Err(broadcast::error::RecvError::Closed) => self.running = false,
                },
            }
        }
        Ok(())
    }

    fn handle_hitl_request(&mut self, req: HitlRequest) {
        self.pending_hitl
            .insert(req.agent_id, (req.question.clone(), req.options.clone()));
        if let Some(agent) = self.find_agent_mut(&req.agent_id) {
            agent.hitl_pending = true;
        }
        if !self.modal.is_open() {
            self.modal = Modal::HitlResponse {
                agent_id: req.agent_id,
                question: req.question,
                options: req.options,
                selected: 0,
            };
        }
    }

    fn draw(&mut self, frame: &mut Frame) {
        if self.session_focus.is_some() {
            self.draw_with_session(frame);
        } else {
            self.draw_overview(frame);
        }
        self.modal.render(frame.area(), frame.buffer_mut());
        if self.show_debug {
            self.render_debug_overlay(frame);
        }
    }

    fn draw_overview(&mut self, frame: &mut Frame) {
        let layout =
            AppLayout::compute_with_focus(frame.area(), self.focus == FocusPanel::EventLog);

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
            focus: self.focus,
        };
        frame.render_widget(hotkey_widget, layout.hotkey_bar);
    }

    fn draw_with_session(&mut self, frame: &mut Frame) {
        let layout = AppLayoutWithSession::compute(frame.area());

        frame.render_stateful_widget(
            AgentTreeWidget {
                focused: self.focus == FocusPanel::AgentTree,
            },
            layout.agent_tree,
            &mut self.agent_tree,
        );

        // The session pane is the primary panel in this layout; treat it as
        // focused unless the operator has Tabbed away to AgentTree or EventLog.
        self.session_pane.focused =
            !matches!(self.focus, FocusPanel::AgentTree | FocusPanel::EventLog);
        frame.render_widget(&self.session_pane, layout.session_pane);

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
            focus: self.focus,
        };
        frame.render_widget(hotkey_widget, layout.hotkey_bar);
    }

    fn render_debug_overlay(&self, frame: &mut Frame) {
        use ratatui::style::{Color, Style};
        use ratatui::text::Line;
        use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};

        let area = frame.area();
        let height = (area.height / 2).max(5);
        let overlay = ratatui::layout::Rect {
            x: 0,
            y: area.height.saturating_sub(height),
            width: area.width,
            height,
        };

        Clear.render(overlay, frame.buffer_mut());

        let block = Block::default()
            .title(" Debug Log (D:close) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red));
        let inner = block.inner(overlay);
        block.render(overlay, frame.buffer_mut());

        let all_lines = self.debug_log.lines();
        let visible_height = inner.height as usize;
        let total = all_lines.len();
        let start = if self.debug_scroll > 0 {
            total
                .saturating_sub(visible_height)
                .saturating_sub(self.debug_scroll)
        } else {
            total.saturating_sub(visible_height)
        };

        let lines: Vec<Line<'_>> = all_lines
            .into_iter()
            .skip(start)
            .take(visible_height)
            .map(|l| Line::styled(l, Style::default().fg(Color::DarkGray)))
            .collect();

        Paragraph::new(lines).render(inner, frame.buffer_mut());
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
        if self.show_debug {
            match key.code {
                KeyCode::Char('D') | KeyCode::Esc => {
                    self.show_debug = false;
                    self.debug_scroll = 0;
                    return;
                }
                KeyCode::Up => {
                    self.debug_scroll = self.debug_scroll.saturating_add(1);
                    return;
                }
                KeyCode::Down => {
                    self.debug_scroll = self.debug_scroll.saturating_sub(1);
                    return;
                }
                _ => return,
            }
        }

        if self.modal.is_open() {
            self.handle_modal_key(key).await;
            return;
        }

        // Global Ctrl+C is the only override that survives session focus.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.running = false;
            return;
        }

        // Session-focused: route every other key through the pane's
        // input/normal mode FSM. Global hotkeys like `q`, `Tab`, `n` are
        // suppressed here — `q` in Normal mode closes the pane, `i` opens
        // the editor, etc.
        if self.session_focus.is_some() {
            self.handle_session_pane_key(key);
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
            KeyCode::Char('D') => {
                self.show_debug = true;
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

    fn handle_session_pane_key(&mut self, key: KeyEvent) {
        use crate::panels::session_pane::input::InputAction;
        let action = self.session_pane.input.handle_key(key);
        match action {
            InputAction::None => {}
            InputAction::Submit(text) => {
                if !text.is_empty() {
                    self.session_pane.submit_text(text);
                }
            }
            InputAction::ClosePane | InputAction::ReturnToGlobal => {
                self.session_focus = None;
            }
            InputAction::ScrollUp(_) | InputAction::ScrollDown(_) => {
                // Scroll is a no-op for now; deferred per spec.
            }
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
            Modal::FilePicker { files, selected } => {
                if let Some(path) = files.get(selected)
                    && let Some(goal) = self.goals.iter().find(|g| g.file_path == *path)
                {
                    if let Some(oid) = goal.id {
                        let _ = self
                            .send_command(TuiCommand::Spawn {
                                objective_id: oid,
                                content: goal.content.clone(),
                                dir: self
                                    .goals_dir
                                    .parent()
                                    .unwrap_or(Path::new("."))
                                    .to_path_buf(),
                                restore_checkpoint_id: None,
                            })
                            .await;
                    } else {
                        self.event_log
                            .push("Objective not yet registered — try again shortly".into());
                    }
                }
            }
            Modal::ConfirmDelete { path, title } => match tokio::fs::remove_file(&path).await {
                Ok(()) => {
                    self.event_log.push(format!("Deleted \"{title}\""));
                    self.goals = goals::scan_goals_dir(&self.goals_dir).unwrap_or_default();
                    goals::reconcile_with_mapping(&self.goals_dir, &mut self.goals);
                    self.objective_tree.load_goals(&self.goals);
                }
                Err(e) => {
                    self.event_log.push(format!("Failed to delete: {e}"));
                }
            },
            Modal::View { .. } | Modal::Help | Modal::None => {}
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
                crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)
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
            KeyCode::Char('e') => {
                self.handle_edit_objective().await;
            }
            KeyCode::Char('v') => {
                self.handle_view_objective();
            }
            KeyCode::Char('x') | KeyCode::Delete => {
                self.handle_delete_objective();
            }
            KeyCode::Enter => {
                self.handle_hitl_from_objective();
            }
            _ => {}
        }
    }

    fn selected_goal_file(&self) -> Option<PathBuf> {
        self.objective_tree
            .panel
            .selected()
            .and_then(|item| match &item.data {
                ObjectiveItem::Root(g) => Some(g.file_path.clone()),
                ObjectiveItem::Sub(_) => None,
            })
    }

    async fn handle_edit_objective(&mut self) {
        let Some(path) = self.selected_goal_file() else {
            self.event_log
                .push("Select a root objective to edit".into());
            return;
        };

        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);

        let _ = tokio::task::spawn_blocking(move || goals::open_in_editor(&path)).await;

        crossterm::terminal::enable_raw_mode().ok();
        crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen).ok();
        self.needs_clear = true;

        self.goals = goals::scan_goals_dir(&self.goals_dir).unwrap_or_default();
        goals::reconcile_with_mapping(&self.goals_dir, &mut self.goals);
        self.objective_tree.load_goals(&self.goals);
        self.event_log.push("Objective reloaded".into());
    }

    fn handle_view_objective(&mut self) {
        let Some(item) = self.objective_tree.panel.selected() else {
            return;
        };
        let (title, content) = match &item.data {
            ObjectiveItem::Root(g) => (g.title.clone(), g.content.clone()),
            ObjectiveItem::Sub(s) => (s.description.clone(), s.description.clone()),
        };
        self.modal = Modal::View { title, content };
    }

    fn handle_delete_objective(&mut self) {
        let Some(item) = self.objective_tree.panel.selected() else {
            return;
        };
        match &item.data {
            ObjectiveItem::Root(g) => {
                if g.agent_id.is_some() {
                    self.event_log
                        .push("Cannot delete objective with active agent".into());
                    return;
                }
                self.modal = Modal::ConfirmDelete {
                    path: g.file_path.clone(),
                    title: g.title.clone(),
                };
            }
            ObjectiveItem::Sub(_) => {
                self.event_log
                    .push("Cannot delete sub-objectives directly".into());
            }
        }
    }

    async fn handle_spawn(&mut self) {
        tracing::debug!(
            cursor = self.objective_tree.panel.cursor,
            items = self.objective_tree.panel.items.len(),
            "handle_spawn called"
        );

        let selected_data = self.objective_tree.panel.selected().map(|item| {
            (
                item.data.objective_id(),
                item.data.agent_id(),
                item.data.title().to_string(),
                match &item.data {
                    ObjectiveItem::Root(g) => g.content.clone(),
                    ObjectiveItem::Sub(s) => s.description.clone(),
                },
            )
        });

        match selected_data {
            Some((oid, agent, title, content)) => {
                tracing::debug!(?oid, ?agent, %title, "selected objective");

                if agent.is_some() {
                    self.event_log
                        .push("Objective already has an active agent".into());
                    return;
                }

                match oid {
                    Some(id) => {
                        tracing::debug!(?id, "sending Spawn command");
                        let _ = self
                            .send_command(TuiCommand::Spawn {
                                objective_id: id,
                                content,
                                dir: self
                                    .goals_dir
                                    .parent()
                                    .unwrap_or(Path::new("."))
                                    .to_path_buf(),
                                restore_checkpoint_id: None,
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
                let files: Vec<PathBuf> = self.goals.iter().map(|g| g.file_path.clone()).collect();
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
            let _ = self.send_command(TuiCommand::Kill { agent_id: aid }).await;
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
                .map(|i| i.data.state == nephila_core::agent::AgentState::Paused)
                .unwrap_or(false);

            let cmd = if is_paused {
                TuiCommand::Resume { agent_id: aid }
            } else {
                TuiCommand::Pause { agent_id: aid }
            };
            let _ = self.send_command(cmd).await;
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
                    let _ = self.send_command(TuiCommand::Kill { agent_id: aid }).await;
                }
            }
            KeyCode::Char('p') => {
                let selected = self.agent_tree.selected().map(|item| {
                    (
                        item.data.id,
                        item.data.state == nephila_core::agent::AgentState::Paused,
                    )
                });
                if let Some((aid, is_paused)) = selected {
                    let cmd = if is_paused {
                        TuiCommand::Resume { agent_id: aid }
                    } else {
                        TuiCommand::Pause { agent_id: aid }
                    };
                    let _ = self.send_command(cmd).await;
                }
            }
            KeyCode::Enter => {
                if let Some(item) = self.agent_tree.selected() {
                    if item.data.hitl_pending {
                        self.try_open_hitl_modal(item.data.id);
                    } else if item.data.session_id.is_some() {
                        // Enter focuses the embedded session pane. The legacy
                        // TTY-handoff `attach_agent_session` lives on hotkey
                        // `a` until the parity matrix closes.
                        self.session_focus = Some(item.data.id);
                    } else {
                        self.event_log
                            .push("Agent has no session to attach to".into());
                    }
                }
            }
            KeyCode::Char('a') => {
                if let Some(item) = self.agent_tree.selected()
                    && let (Some(sid), Some(dir)) = (&item.data.session_id, &item.data.directory)
                {
                    let aid = item.data.id;
                    let first = !item.data.has_session;
                    self.attach_agent_session(aid, sid.clone(), dir.clone(), first)
                        .await;
                }
            }
            _ => {}
        }
    }

    async fn send_command(&mut self, cmd: TuiCommand) -> Result<(), TuiCommandError> {
        tracing::debug!(?cmd, "sending command");
        if self.cmd_tx.send(cmd).await.is_err() {
            tracing::warn!("command channel closed");
            self.event_log.push("Command channel closed".into());
            return Err(TuiCommandError::ChannelClosed);
        }
        Ok(())
    }

    fn find_agent_mut(
        &mut self,
        agent_id: &AgentId,
    ) -> Option<&mut crate::panels::agent_tree::AgentTreeNode> {
        self.agent_tree
            .items
            .iter_mut()
            .find(|i| i.data.id == *agent_id)
            .map(|i| &mut i.data)
    }

    async fn attach_agent_session(
        &mut self,
        agent_id: AgentId,
        session_id: String,
        directory: PathBuf,
        first_time: bool,
    ) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);

        let mut cmd = tokio::process::Command::new(&self.claude_binary);
        if first_time {
            cmd.arg("--session-id").arg(&session_id);
        } else {
            cmd.arg("--resume").arg(&session_id);
        }
        cmd.arg("--permission-mode")
            .arg("bypassPermissions")
            .arg("--settings")
            .arg(r#"{"skipDangerousModePermissionPrompt": true}"#)
            .current_dir(&directory);

        let result = cmd.status().await;

        crossterm::terminal::enable_raw_mode().ok();
        crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen).ok();
        self.needs_clear = true;

        // Mark session as created after first attach
        if first_time && let Some(agent) = self.find_agent_mut(&agent_id) {
            agent.has_session = true;
        }

        match result {
            Ok(status) => {
                self.event_log
                    .push(format!("Claude session exited: {status}"));
            }
            Err(e) => {
                self.event_log.push(format!("Failed to attach: {e}"));
            }
        }
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
                } else {
                    let label = self
                        .goals
                        .iter()
                        .find(|g| g.agent_id == Some(*agent_id))
                        .map(|g| g.title.clone())
                        .unwrap_or_else(|| agent_id.to_string());
                    self.agent_tree
                        .items
                        .push(crate::panels::agent_tree::FlatTreeItem {
                            data: crate::panels::agent_tree::AgentTreeNode {
                                id: *agent_id,
                                state: *new_state,
                                objective_label: label,
                                tokens_used: None,
                                tokens_remaining: None,
                                checkpoint_id: None,
                                hitl_pending: false,
                                session_id: None,
                                directory: None,
                                has_session: false,
                                last_session_event: None,
                                activity_glyph: None,
                            },
                            depth: 0,
                            is_expanded: true,
                            has_children: false,
                        });
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
            BusEvent::HitlResponded { agent_id, response } => {
                self.pending_hitl.remove(agent_id);
                if let Some(agent) = self.find_agent_mut(agent_id) {
                    agent.hitl_pending = false;
                }
                self.event_log
                    .push(format!("[{agent_id}] HITL response: {response}"));
            }
            BusEvent::AgentSessionReady {
                agent_id,
                session_id,
                directory,
            } => {
                if let Some(agent) = self.find_agent_mut(agent_id) {
                    agent.session_id = Some(session_id.clone());
                    agent.directory = Some(directory.clone());
                }
                self.event_log
                    .push(format!("[{agent_id}] session ready — Enter to attach"));
            }
            BusEvent::CheckpointSaved {
                agent_id,
                checkpoint_id,
            } => {
                // The MCP handler no longer emits CheckpointSaved; the
                // connector reader emits SessionEvent::CheckpointReached
                // instead. A legacy emission reaching here means something
                // is producing it that shouldn't be — log and surface to
                // operator.
                tracing::warn!(
                    target: "nephila_tui::bus",
                    %agent_id,
                    %checkpoint_id,
                    "legacy CheckpointSaved bus event observed — should be unreachable",
                );
                self.event_log
                    .push(format!("[{agent_id}] checkpoint saved: {checkpoint_id}"));
            }
            BusEvent::ObjectiveUpdated {
                objective_id,
                status,
            } => {
                self.event_log
                    .push(format!("[{objective_id}] objective -> {status}"));
            }
            BusEvent::Shutdown => {
                self.event_log.push("Shutdown signal received".into());
                self.running = false;
            }
        }
    }
}
