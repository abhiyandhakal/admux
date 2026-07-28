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
                if config::key_event_matches(&self.keymap.prefix, event) {
                    // Match tmux's prefix-prefix convention: the second prefix
                    // key is delivered to the foreground application.
                    key_to_bytes(event)
                } else if let Some(action) = self.resolve(&self.keymap.leader, event) {
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
            control_character_bytes(ch, event.modifiers)
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
        KeyCode::Left => csi_cursor_key_bytes('D', event.modifiers),
        KeyCode::Right => csi_cursor_key_bytes('C', event.modifiers),
        KeyCode::Up => csi_cursor_key_bytes('A', event.modifiers),
        KeyCode::Down => csi_cursor_key_bytes('B', event.modifiers),
        KeyCode::Home => csi_cursor_key_bytes('H', event.modifiers),
        KeyCode::End => csi_cursor_key_bytes('F', event.modifiers),
        KeyCode::Insert => csi_tilde_key_bytes(2, event.modifiers),
        KeyCode::Delete => csi_tilde_key_bytes(3, event.modifiers),
        KeyCode::PageUp => csi_tilde_key_bytes(5, event.modifiers),
        KeyCode::PageDown => csi_tilde_key_bytes(6, event.modifiers),
        KeyCode::F(number) => function_key_bytes(number, event.modifiers),
        _ => InputAction::Noop,
    }
}

fn control_character_bytes(ch: char, modifiers: KeyModifiers) -> InputAction {
    let ascii = ch.to_ascii_lowercase() as u8;
    let byte = match ascii {
        b'a'..=b'z' => ascii - b'a' + 1,
        b'@' | b' ' => 0x00,
        b'[' => 0x1b,
        b'\\' => 0x1c,
        b']' => 0x1d,
        b'^' => 0x1e,
        b'_' => 0x1f,
        b'?' => 0x7f,
        _ => return InputAction::Noop,
    };
    let mut bytes = Vec::with_capacity(2);
    if modifiers.contains(KeyModifiers::ALT) {
        bytes.push(0x1b);
    }
    bytes.push(byte);
    InputAction::SendBytes(bytes)
}

fn function_key_bytes(number: u8, modifiers: KeyModifiers) -> InputAction {
    let modifier = xterm_modifier_parameter(modifiers);
    let sequence = match (number, modifier) {
        (1, None) => "\x1bOP".to_string(),
        (2, None) => "\x1bOQ".to_string(),
        (3, None) => "\x1bOR".to_string(),
        (4, None) => "\x1bOS".to_string(),
        (1..=4, Some(modifier)) => format!("\x1b[1;{modifier}{}", (b'P' + number - 1) as char),
        (5, None) => "\x1b[15~".to_string(),
        (6, None) => "\x1b[17~".to_string(),
        (7, None) => "\x1b[18~".to_string(),
        (8, None) => "\x1b[19~".to_string(),
        (9, None) => "\x1b[20~".to_string(),
        (10, None) => "\x1b[21~".to_string(),
        (11, None) => "\x1b[23~".to_string(),
        (12, None) => "\x1b[24~".to_string(),
        (5..=12, Some(modifier)) => {
            let code = [15, 17, 18, 19, 20, 21, 23, 24][(number - 5) as usize];
            format!("\x1b[{code};{modifier}~")
        }
        _ => return InputAction::Noop,
    };
    InputAction::SendBytes(sequence.into_bytes())
}

fn csi_cursor_key_bytes(final_byte: char, modifiers: KeyModifiers) -> InputAction {
    let sequence = match xterm_modifier_parameter(modifiers) {
        Some(modifier) => format!("\x1b[1;{modifier}{final_byte}"),
        None => format!("\x1b[{final_byte}"),
    };
    InputAction::SendBytes(sequence.into_bytes())
}

fn csi_tilde_key_bytes(code: u8, modifiers: KeyModifiers) -> InputAction {
    let sequence = match xterm_modifier_parameter(modifiers) {
        Some(modifier) => format!("\x1b[{code};{modifier}~"),
        None => format!("\x1b[{code}~"),
    };
    InputAction::SendBytes(sequence.into_bytes())
}

fn xterm_modifier_parameter(modifiers: KeyModifiers) -> Option<u8> {
    let shift = modifiers.contains(KeyModifiers::SHIFT) as u8;
    let alt = modifiers.contains(KeyModifiers::ALT) as u8;
    let control = modifiers.contains(KeyModifiers::CONTROL) as u8;
    let value = 1 + shift + 2 * alt + 4 * control;
    (value != 1).then_some(value)
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
    fn pressing_the_prefix_twice_forwards_it_to_the_pane() {
        let mut state = configured_state("");
        let prefix = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL);
        assert_eq!(state.handle_key(prefix), InputAction::Noop);
        assert_eq!(state.handle_key(prefix), InputAction::SendBytes(vec![0x02]));
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
    fn forwards_extended_terminal_keys_and_modifiers() {
        let mut state = configured_state("");
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            InputAction::SendBytes(b"\x1b[15~".to_vec())
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::F(12), KeyModifiers::ALT)),
            InputAction::SendBytes(b"\x1b[24;3~".to_vec())
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::CONTROL)),
            InputAction::SendBytes(b"\x1b[6;5~".to_vec())
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Insert, KeyModifiers::NONE)),
            InputAction::SendBytes(b"\x1b[2~".to_vec())
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::ALT)),
            InputAction::SendBytes(b"\x1b[1;3D".to_vec())
        );
    }

    #[test]
    fn forwards_control_punctuation() {
        let mut state = configured_state("");
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('['), KeyModifiers::CONTROL)),
            InputAction::SendBytes(vec![0x1b])
        );
        assert_eq!(
            state.handle_key(KeyEvent::new(
                KeyCode::Char('?'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            )),
            InputAction::SendBytes(vec![0x1b, 0x7f])
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
