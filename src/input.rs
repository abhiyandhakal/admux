use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    config::{self, Action, ResolvedKeyConfig},
    ipc::NavigationDirection,
    layout::SplitAxis,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Leader,
    CopyMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    Noop,
    Detach,
    SendBytes(Vec<u8>),
    SplitPane(SplitAxis),
    SelectWindowIndex(u8),
    NewWindow,
    NextWindow,
    PrevWindow,
    FocusPane(NavigationDirection),
    ResizePane(NavigationDirection, u16),
    KillPane,
    PasteTopBuffer,
    ListBuffers,
    DeleteTopBuffer,
    ChooseBuffer,
    OpenPrompt,
    OpenSessions,
    OpenHelp,
    EnterCopyMode,
    ExitCopyMode,
    CopyMove(NavigationDirection),
    CopyLineStart,
    CopyLineEnd,
    CopyTop,
    CopyBottom,
    CopyPageUp,
    CopyPageDown,
    CopyStartSelection,
    CopyYank,
    ReloadConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputState {
    pub mode: InputMode,
    keymap: ResolvedKeyConfig,
    resize_step: u16,
}

impl Default for InputState {
    fn default() -> Self {
        Self::new(ResolvedKeyConfig::default(), 50)
    }
}

impl InputState {
    pub fn new(keymap: ResolvedKeyConfig, resize_step: u16) -> Self {
        Self {
            mode: InputMode::Normal,
            keymap,
            resize_step,
        }
    }

    pub fn replace_config(&mut self, keymap: ResolvedKeyConfig, resize_step: u16) {
        self.keymap = keymap;
        self.resize_step = resize_step;
        self.mode = InputMode::Normal;
    }

    pub fn handle_key(&mut self, event: KeyEvent) -> InputAction {
        match self.mode {
            InputMode::Normal => {
                if config::key_event_matches(&self.keymap.prefix, event) {
                    self.mode = InputMode::Leader;
                    InputAction::Noop
                } else if let Some(action) = self.resolve(&self.keymap.normal, event) {
                    self.map_action(action)
                } else {
                    key_to_bytes(event)
                }
            }
            InputMode::Leader => {
                self.mode = InputMode::Normal;
                if let Some(action) = self.resolve(&self.keymap.leader, event) {
                    match action {
                        Action::EnterCopyMode => {
                            self.mode = InputMode::CopyMode;
                            InputAction::EnterCopyMode
                        }
                        other => self.map_action(other),
                    }
                } else {
                    InputAction::Noop
                }
            }
            InputMode::CopyMode => {
                if let Some(action) = self.resolve(&self.keymap.copy_mode, event) {
                    match action {
                        Action::ExitCopyMode | Action::CopyYank => {
                            self.mode = InputMode::Normal;
                            self.map_action(action)
                        }
                        other => self.map_action(other),
                    }
                } else {
                    match event.code {
                        KeyCode::Esc | KeyCode::Char('q') => {
                            self.mode = InputMode::Normal;
                            InputAction::ExitCopyMode
                        }
                        _ => InputAction::Noop,
                    }
                }
            }
        }
    }

    fn resolve(&self, table: &[(config::KeyPattern, Action)], event: KeyEvent) -> Option<Action> {
        table.iter().find_map(|(pattern, action)| {
            config::key_event_matches(pattern, event).then_some(*action)
        })
    }

    fn map_action(&self, action: Action) -> InputAction {
        match action {
            Action::Detach => InputAction::Detach,
            Action::SplitVertical => InputAction::SplitPane(SplitAxis::Vertical),
            Action::SplitHorizontal => InputAction::SplitPane(SplitAxis::Horizontal),
            Action::OpenPrompt => InputAction::OpenPrompt,
            Action::OpenSessions => InputAction::OpenSessions,
            Action::OpenHelp => InputAction::OpenHelp,
            Action::NewWindow => InputAction::NewWindow,
            Action::NextWindow => InputAction::NextWindow,
            Action::PrevWindow => InputAction::PrevWindow,
            Action::SelectWindowIndex(index) => InputAction::SelectWindowIndex(index),
            Action::FocusLeft => InputAction::FocusPane(NavigationDirection::Left),
            Action::FocusDown => InputAction::FocusPane(NavigationDirection::Down),
            Action::FocusUp => InputAction::FocusPane(NavigationDirection::Up),
            Action::FocusRight => InputAction::FocusPane(NavigationDirection::Right),
            Action::ResizeLeft => {
                InputAction::ResizePane(NavigationDirection::Left, self.resize_step)
            }
            Action::ResizeDown => {
                InputAction::ResizePane(NavigationDirection::Down, self.resize_step)
            }
            Action::ResizeUp => InputAction::ResizePane(NavigationDirection::Up, self.resize_step),
            Action::ResizeRight => {
                InputAction::ResizePane(NavigationDirection::Right, self.resize_step)
            }
            Action::KillPane => InputAction::KillPane,
            Action::PasteTopBuffer => InputAction::PasteTopBuffer,
            Action::ListBuffers => InputAction::ListBuffers,
            Action::DeleteTopBuffer => InputAction::DeleteTopBuffer,
            Action::ChooseBuffer => InputAction::ChooseBuffer,
            Action::EnterCopyMode => InputAction::EnterCopyMode,
            Action::ExitCopyMode => InputAction::ExitCopyMode,
            Action::CopyMoveLeft => InputAction::CopyMove(NavigationDirection::Left),
            Action::CopyMoveDown => InputAction::CopyMove(NavigationDirection::Down),
            Action::CopyMoveUp => InputAction::CopyMove(NavigationDirection::Up),
            Action::CopyMoveRight => InputAction::CopyMove(NavigationDirection::Right),
            Action::CopyLineStart => InputAction::CopyLineStart,
            Action::CopyLineEnd => InputAction::CopyLineEnd,
            Action::CopyTop => InputAction::CopyTop,
            Action::CopyBottom => InputAction::CopyBottom,
            Action::CopyPageUp => InputAction::CopyPageUp,
            Action::CopyPageDown => InputAction::CopyPageDown,
            Action::CopyStartSelection => InputAction::CopyStartSelection,
            Action::CopyYank => InputAction::CopyYank,
            Action::ReloadConfig => InputAction::ReloadConfig,
        }
    }
}

fn key_to_bytes(event: KeyEvent) -> InputAction {
    match event.code {
        KeyCode::Char(ch) if event.modifiers.contains(KeyModifiers::CONTROL) => {
            let ascii = ch.to_ascii_lowercase() as u8;
            if ascii.is_ascii_lowercase() {
                InputAction::SendBytes(vec![ascii - b'a' + 1])
            } else {
                InputAction::Noop
            }
        }
        KeyCode::Char(ch) if event.modifiers.contains(KeyModifiers::ALT) => {
            let mut bytes = vec![0x1b];
            bytes.extend_from_slice(ch.to_string().as_bytes());
            InputAction::SendBytes(bytes)
        }
        KeyCode::Char(ch) => InputAction::SendBytes(ch.to_string().into_bytes()),
        KeyCode::Esc => InputAction::SendBytes(vec![0x1b]),
        KeyCode::Enter => InputAction::SendBytes(vec![b'\r']),
        KeyCode::Tab => InputAction::SendBytes(vec![b'\t']),
        KeyCode::Backspace => InputAction::SendBytes(vec![0x7f]),
        KeyCode::Left => InputAction::SendBytes(b"\x1b[D".to_vec()),
        KeyCode::Right => InputAction::SendBytes(b"\x1b[C".to_vec()),
        KeyCode::Up => InputAction::SendBytes(b"\x1b[A".to_vec()),
        KeyCode::Down => InputAction::SendBytes(b"\x1b[B".to_vec()),
        KeyCode::Home => InputAction::SendBytes(b"\x1b[H".to_vec()),
        KeyCode::End => InputAction::SendBytes(b"\x1b[F".to_vec()),
        KeyCode::Delete => InputAction::SendBytes(b"\x1b[3~".to_vec()),
        _ => InputAction::Noop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn configured_state(toml: &str) -> InputState {
        let config = Config::from_toml(toml).expect("config");
        let resolved = config.resolve().expect("resolved");
        InputState::new(resolved.keys, resolved.behavior.resize_step)
    }

    #[test]
    fn ctrl_b_then_d_detaches() {
        let mut state = configured_state("");
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            InputAction::Noop
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)),
            InputAction::Detach
        );
    }

    #[test]
    fn ctrl_l_is_forwarded_as_form_feed() {
        let mut state = configured_state("");
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)),
            InputAction::SendBytes(vec![0x0c])
        );
    }

    #[test]
    fn escape_is_forwarded_to_the_pane() {
        let mut state = configured_state("");
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            InputAction::SendBytes(vec![0x1b])
        );
    }

    #[test]
    fn alt_key_is_forwarded_as_escape_sequence() {
        let mut state = configured_state("");
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::ALT)),
            InputAction::SendBytes(vec![0x1b, b'j'])
        );
    }

    #[test]
    fn leader_split_triggers_split_action() {
        let mut state = configured_state("");
        let _ = state.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('%'), KeyModifiers::NONE)),
            InputAction::SplitPane(SplitAxis::Vertical)
        );
    }

    #[test]
    fn leader_digit_selects_window_index() {
        let mut state = configured_state("");
        let _ = state.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE)),
            InputAction::SelectWindowIndex(3)
        );
    }

    #[test]
    fn leader_question_opens_help() {
        let mut state = configured_state("");
        let _ = state.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT)),
            InputAction::OpenHelp
        );
    }

    #[test]
    fn leader_bracket_pastes_top_buffer() {
        let mut state = configured_state("");
        let _ = state.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE)),
            InputAction::PasteTopBuffer
        );
    }

    #[test]
    fn leader_left_bracket_enters_copy_mode() {
        let mut state = configured_state("");
        let _ = state.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE)),
            InputAction::EnterCopyMode
        );
        assert_eq!(state.mode, InputMode::CopyMode);
    }

    #[test]
    fn copy_mode_yank_exits_back_to_normal() {
        let mut state = configured_state("");
        state.mode = InputMode::CopyMode;
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE)),
            InputAction::CopyYank
        );
        assert_eq!(state.mode, InputMode::Normal);
    }

    #[test]
    fn custom_prefix_is_respected() {
        let mut state = configured_state(
            r#"
                [keys]
                leader = "Ctrl-a"
            "#,
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL)),
            InputAction::SendBytes(vec![2])
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)),
            InputAction::Noop
        );
    }

    #[test]
    fn custom_leader_binding_triggers_action() {
        let mut state = configured_state(
            r#"
                [keys.leader]
                new_window = "w"
            "#,
        );
        let _ = state.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE)),
            InputAction::NewWindow
        );
    }

    #[test]
    fn custom_copy_mode_binding_triggers_action() {
        let mut state = configured_state(
            r#"
                [keys.copy_mode]
                copy_yank = "Enter"
            "#,
        );
        state.mode = InputMode::CopyMode;
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            InputAction::CopyYank
        );
        assert_eq!(state.mode, InputMode::Normal);
    }

    #[test]
    fn configured_resize_step_is_used() {
        let mut state = configured_state(
            r#"
                [behavior]
                resize_step = 7
            "#,
        );
        let _ = state.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT)),
            InputAction::ResizePane(NavigationDirection::Right, 7)
        );
    }
}
