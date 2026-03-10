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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyMode {
    pub pane_id: u64,
    pub cursor_row: u16,
    pub cursor_col: u16,
    anchor: Option<(u16, u16)>,
}

impl CopyMode {
    pub fn new(pane_id: u64, cursor_row: u16, cursor_col: u16) -> Self {
        Self {
            pane_id,
            cursor_row,
            cursor_col,
            anchor: None,
        }
    }

    pub fn clamp_to(&mut self, rows: usize, cols: usize) {
        let max_row = rows.saturating_sub(1) as u16;
        let max_col = cols.saturating_sub(1) as u16;
        self.cursor_row = self.cursor_row.min(max_row);
        self.cursor_col = self.cursor_col.min(max_col);
        if rows == 0 {
            self.cursor_row = 0;
        }
        if cols == 0 {
            self.cursor_col = 0;
        }
        self.update_selection();
    }

    pub fn move_left(&mut self) {
        self.cursor_col = self.cursor_col.saturating_sub(1);
        self.update_selection();
    }

    pub fn move_right(&mut self, cols: usize) {
        let max_col = cols.saturating_sub(1) as u16;
        self.cursor_col = (self.cursor_col + 1).min(max_col);
        self.update_selection();
    }

    pub fn move_up(&mut self) {
        self.cursor_row = self.cursor_row.saturating_sub(1);
        self.update_selection();
    }

    pub fn move_down(&mut self, rows: usize) {
        let max_row = rows.saturating_sub(1) as u16;
        self.cursor_row = (self.cursor_row + 1).min(max_row);
        self.update_selection();
    }

    pub fn move_line_start(&mut self) {
        self.cursor_col = 0;
        self.update_selection();
    }

    pub fn move_line_end(&mut self, cols: usize) {
        self.cursor_col = cols.saturating_sub(1) as u16;
        self.update_selection();
    }

    pub fn move_top(&mut self) {
        self.cursor_row = 0;
        self.update_selection();
    }

    pub fn move_bottom(&mut self, rows: usize) {
        self.cursor_row = rows.saturating_sub(1) as u16;
        self.update_selection();
    }

    pub fn start_selection(&mut self) {
        if self.anchor.is_none() {
            self.anchor = Some((self.cursor_row, self.cursor_col));
        } else {
            self.anchor = None;
        }
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    pub fn selection(&self) -> Option<Selection> {
        self.anchor.map(|(row, col)| {
            Selection::new(row, col, self.cursor_row, self.cursor_col).normalized()
        })
    }

    pub fn cursor_selection(&self) -> Selection {
        Selection::new(
            self.cursor_row,
            self.cursor_col,
            self.cursor_row,
            self.cursor_col,
        )
    }

    fn update_selection(&mut self) {}
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

    #[test]
    fn copy_mode_clamps_and_moves_cursor() {
        let mut mode = CopyMode::new(1, 10, 10);
        mode.clamp_to(4, 5);
        assert_eq!((mode.cursor_row, mode.cursor_col), (3, 4));

        mode.move_left();
        mode.move_up();
        assert_eq!((mode.cursor_row, mode.cursor_col), (2, 3));

        mode.move_top();
        mode.move_line_start();
        assert_eq!((mode.cursor_row, mode.cursor_col), (0, 0));
    }

    #[test]
    fn copy_mode_tracks_selection_from_anchor() {
        let mut mode = CopyMode::new(1, 1, 2);
        mode.start_selection();
        mode.move_down(5);
        mode.move_right(6);

        assert_eq!(mode.selection(), Some(Selection::new(1, 2, 2, 3)));
    }
}
