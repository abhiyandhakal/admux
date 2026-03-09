#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyModeState {
    Inactive,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub start_row: u16,
    pub start_col: u16,
    pub end_row: u16,
    pub end_col: u16,
}

impl Selection {
    pub fn new(start_row: u16, start_col: u16, end_row: u16, end_col: u16) -> Self {
        Self {
            start_row,
            start_col,
            end_row,
            end_col,
        }
    }

    pub fn normalized(self) -> Self {
        if (self.start_row, self.start_col) <= (self.end_row, self.end_col) {
            self
        } else {
            Self {
                start_row: self.end_row,
                start_col: self.end_col,
                end_row: self.start_row,
                end_col: self.start_col,
            }
        }
    }
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

    #[test]
    fn normalizes_reverse_selection() {
        assert_eq!(
            Selection::new(4, 8, 1, 2).normalized(),
            Selection::new(1, 2, 4, 8)
        );
    }
}
