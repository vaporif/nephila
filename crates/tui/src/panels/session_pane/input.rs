//! Input box state and key routing for the `SessionPane`.
//!
//! Hand-rolled multi-line buffer with vim-flavored Normal/Input modes. We
//! avoid `tui-textarea` because the `0.7` release on crates.io pins
//! `ratatui = "0.29"`, which conflicts with the workspace's `ratatui = "0.30"`
//! at the trait level (Widget impls hang off different `Buffer` types). The
//! `tui-textarea-2 = "0.11"` fork resolves the version but is a non-canonical
//! crate name; per Task 4 step 1's explicit fallback, we hand-roll the buffer.
//!
//! Keymap:
//!   Normal: `i` → Input, `q` → ClosePane, `j`/`k` → scroll, Esc → ReturnToGlobal
//!   Input:  Esc → Normal, Ctrl+Enter → Submit, Enter → newline, Backspace → erase,
//!           arrow keys → cursor move, printable → insert at cursor

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PaneMode {
    Normal,
    Input,
}

#[derive(Debug)]
pub enum InputAction {
    None,
    Submit(String),
    ClosePane,
    ReturnToGlobal,
    ScrollUp(u16),
    ScrollDown(u16),
}

#[derive(Debug)]
pub struct InputState {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
    mode: PaneMode,
}

impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}

impl InputState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
            mode: PaneMode::Normal,
        }
    }

    #[must_use]
    pub const fn mode(&self) -> PaneMode {
        self.mode
    }

    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    #[must_use]
    pub const fn cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    fn reset(&mut self) {
        self.lines = vec![String::new()];
        self.cursor_row = 0;
        self.cursor_col = 0;
    }

    fn collect_text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> InputAction {
        match self.mode {
            PaneMode::Normal => self.handle_normal(key),
            PaneMode::Input => self.handle_input(key),
        }
    }

    fn handle_normal(&mut self, key: KeyEvent) -> InputAction {
        match key.code {
            KeyCode::Char('i') => {
                self.mode = PaneMode::Input;
                InputAction::None
            }
            KeyCode::Char('q') => InputAction::ClosePane,
            KeyCode::Char('j') => InputAction::ScrollDown(1),
            KeyCode::Char('k') => InputAction::ScrollUp(1),
            KeyCode::Esc => InputAction::ReturnToGlobal,
            _ => InputAction::None,
        }
    }

    fn handle_input(&mut self, key: KeyEvent) -> InputAction {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                self.mode = PaneMode::Normal;
                InputAction::None
            }
            (KeyCode::Enter, mods) if mods.contains(KeyModifiers::CONTROL) => {
                let text = self.collect_text();
                self.reset();
                self.mode = PaneMode::Normal;
                InputAction::Submit(text)
            }
            (KeyCode::Enter, _) => {
                self.insert_newline();
                InputAction::None
            }
            (KeyCode::Backspace, _) => {
                self.backspace();
                InputAction::None
            }
            (KeyCode::Left, _) => {
                self.move_left();
                InputAction::None
            }
            (KeyCode::Right, _) => {
                self.move_right();
                InputAction::None
            }
            (KeyCode::Up, _) => {
                self.move_up();
                InputAction::None
            }
            (KeyCode::Down, _) => {
                self.move_down();
                InputAction::None
            }
            (KeyCode::Home, _) => {
                self.cursor_col = 0;
                InputAction::None
            }
            (KeyCode::End, _) => {
                self.cursor_col = self.current_line_len();
                InputAction::None
            }
            (KeyCode::Char(c), mods) if !mods.contains(KeyModifiers::CONTROL) => {
                self.insert_char(c);
                InputAction::None
            }
            _ => InputAction::None,
        }
    }

    fn current_line_len(&self) -> usize {
        self.lines
            .get(self.cursor_row)
            .map_or(0, |l| l.chars().count())
    }

    fn insert_char(&mut self, c: char) {
        let row = self.cursor_row;
        if let Some(line) = self.lines.get_mut(row) {
            let byte_idx = char_idx_to_byte(line, self.cursor_col);
            line.insert(byte_idx, c);
            self.cursor_col += 1;
        }
    }

    fn insert_newline(&mut self) {
        let row = self.cursor_row;
        let col = self.cursor_col;
        let split_at = self.lines.get(row).map_or(0, |l| char_idx_to_byte(l, col));
        let tail = self
            .lines
            .get_mut(row)
            .map(|l| l.split_off(split_at))
            .unwrap_or_default();
        self.lines.insert(row + 1, tail);
        self.cursor_row += 1;
        self.cursor_col = 0;
    }

    fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let row = self.cursor_row;
            if let Some(line) = self.lines.get_mut(row) {
                let new_col = self.cursor_col - 1;
                let byte_start = char_idx_to_byte(line, new_col);
                let byte_end = char_idx_to_byte(line, self.cursor_col);
                line.replace_range(byte_start..byte_end, "");
                self.cursor_col = new_col;
            }
        } else if self.cursor_row > 0 {
            let line = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            let prev_len = self
                .lines
                .get(self.cursor_row)
                .map_or(0, |l| l.chars().count());
            if let Some(prev) = self.lines.get_mut(self.cursor_row) {
                prev.push_str(&line);
            }
            self.cursor_col = prev_len;
        }
    }

    fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.current_line_len();
        }
    }

    fn move_right(&mut self) {
        let line_len = self.current_line_len();
        if self.cursor_col < line_len {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.cursor_col.min(self.current_line_len());
        }
    }

    fn move_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = self.cursor_col.min(self.current_line_len());
        }
    }
}

fn char_idx_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map_or_else(|| s.len(), |(b, _)| b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_mod(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn normal_i_enters_input_mode() {
        let mut s = InputState::new();
        let action = s.handle_key(key(KeyCode::Char('i')));
        assert!(matches!(action, InputAction::None));
        assert_eq!(s.mode(), PaneMode::Input);
    }

    #[test]
    fn normal_q_closes_pane() {
        let mut s = InputState::new();
        assert!(matches!(
            s.handle_key(key(KeyCode::Char('q'))),
            InputAction::ClosePane
        ));
    }

    #[test]
    fn normal_j_k_scroll() {
        let mut s = InputState::new();
        assert!(matches!(
            s.handle_key(key(KeyCode::Char('j'))),
            InputAction::ScrollDown(1)
        ));
        assert!(matches!(
            s.handle_key(key(KeyCode::Char('k'))),
            InputAction::ScrollUp(1)
        ));
    }

    #[test]
    fn normal_esc_returns_to_global() {
        let mut s = InputState::new();
        assert!(matches!(
            s.handle_key(key(KeyCode::Esc)),
            InputAction::ReturnToGlobal
        ));
    }

    #[test]
    fn input_esc_returns_to_normal() {
        let mut s = InputState::new();
        s.handle_key(key(KeyCode::Char('i')));
        assert_eq!(s.mode(), PaneMode::Input);
        assert!(matches!(s.handle_key(key(KeyCode::Esc)), InputAction::None));
        assert_eq!(s.mode(), PaneMode::Normal);
    }

    #[test]
    fn input_ctrl_enter_submits_and_resets() {
        let mut s = InputState::new();
        s.handle_key(key(KeyCode::Char('i')));
        for c in "hi".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        let action = s.handle_key(key_mod(KeyCode::Enter, KeyModifiers::CONTROL));
        match action {
            InputAction::Submit(text) => assert_eq!(text, "hi"),
            other => panic!("expected Submit, got {other:?}"),
        }
        assert_eq!(s.mode(), PaneMode::Normal);
        assert_eq!(s.lines(), &[""]);
    }

    #[test]
    fn input_enter_inserts_newline() {
        let mut s = InputState::new();
        s.handle_key(key(KeyCode::Char('i')));
        for c in "ab".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        s.handle_key(key(KeyCode::Enter));
        for c in "cd".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(s.lines(), &["ab", "cd"]);
        assert_eq!(s.cursor(), (1, 2));
    }

    #[test]
    fn input_backspace_removes_char() {
        let mut s = InputState::new();
        s.handle_key(key(KeyCode::Char('i')));
        for c in "ab".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        s.handle_key(key(KeyCode::Backspace));
        assert_eq!(s.lines(), &["a"]);
        assert_eq!(s.cursor(), (0, 1));
    }

    #[test]
    fn input_backspace_at_line_start_joins_lines() {
        let mut s = InputState::new();
        s.handle_key(key(KeyCode::Char('i')));
        s.handle_key(key(KeyCode::Char('a')));
        s.handle_key(key(KeyCode::Enter));
        s.handle_key(key(KeyCode::Char('b')));
        s.handle_key(key(KeyCode::Home));
        s.handle_key(key(KeyCode::Backspace));
        assert_eq!(s.lines(), &["ab"]);
        assert_eq!(s.cursor(), (0, 1));
    }

    #[test]
    fn input_arrows_move_cursor() {
        let mut s = InputState::new();
        s.handle_key(key(KeyCode::Char('i')));
        for c in "ab".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        s.handle_key(key(KeyCode::Left));
        assert_eq!(s.cursor(), (0, 1));
        s.handle_key(key(KeyCode::Home));
        assert_eq!(s.cursor(), (0, 0));
        s.handle_key(key(KeyCode::End));
        assert_eq!(s.cursor(), (0, 2));
    }

    #[test]
    fn input_handles_unicode_chars() {
        let mut s = InputState::new();
        s.handle_key(key(KeyCode::Char('i')));
        for c in "héllo".chars() {
            s.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(s.lines(), &["héllo"]);
        let action = s.handle_key(key_mod(KeyCode::Enter, KeyModifiers::CONTROL));
        match action {
            InputAction::Submit(t) => assert_eq!(t, "héllo"),
            other => panic!("expected Submit, got {other:?}"),
        }
    }
}
