#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyModeState {
    Inactive,
    Active,
}

pub fn search_forward(buffer: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    buffer.find(needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_copy_mode_search_match() {
        assert_eq!(search_forward("hello pane output", "pane"), Some(6));
    }
}
