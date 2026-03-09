use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputState {
    pub mode: InputMode,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            mode: InputMode::Normal,
        }
    }
}

impl InputState {
    pub fn handle_key(&mut self, event: KeyEvent) -> InputAction {
        match self.mode {
            InputMode::Normal => {
                if event.code == KeyCode::Char('b')
                    && event.modifiers.contains(KeyModifiers::CONTROL)
                {
                    self.mode = InputMode::Leader;
                    InputAction::Noop
                } else {
                    key_to_bytes(event)
                }
            }
            InputMode::Leader => {
                self.mode = InputMode::Normal;
                match event.code {
                    KeyCode::Char('d') => InputAction::Detach,
                    _ => InputAction::Noop,
                }
            }
            InputMode::CopyMode => InputAction::Noop,
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
        KeyCode::Char(ch) => InputAction::SendBytes(ch.to_string().into_bytes()),
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

    #[test]
    fn ctrl_b_then_d_detaches() {
        let mut state = InputState::default();
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
        let mut state = InputState::default();
        assert_eq!(
            state.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL)),
            InputAction::SendBytes(vec![0x0c])
        );
    }
}
