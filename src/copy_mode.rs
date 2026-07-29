#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub start_row: u16,
    pub start_col: u16,
    pub end_row: u16,
    pub end_col: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryPosition {
    pub from_bottom: u32,
    pub col: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistorySelection {
    pub start: HistoryPosition,
    pub end: HistoryPosition,
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
    cursor_from_bottom: u32,
    viewport_scrollback: u32,
    viewport_rows: u16,
    viewport_initialized: bool,
    anchor: Option<HistoryPosition>,
}

impl CopyMode {
    pub fn new(pane_id: u64, cursor_row: u16, cursor_col: u16) -> Self {
        Self {
            pane_id,
            cursor_row,
            cursor_col,
            cursor_from_bottom: 0,
            viewport_scrollback: 0,
            viewport_rows: 0,
            viewport_initialized: false,
            anchor: None,
        }
    }

    pub fn clamp_to(&mut self, rows: usize, cols: usize) {
        self.sync_viewport(rows, cols, 0);
    }

    pub fn sync_viewport(&mut self, rows: usize, cols: usize, scrollback: u32) {
        let max_row = terminal_index(rows);
        let max_col = terminal_index(cols);
        self.cursor_row = self.cursor_row.min(max_row);
        self.cursor_col = self.cursor_col.min(max_col);
        if rows == 0 {
            self.cursor_row = 0;
        }
        if cols == 0 {
            self.cursor_col = 0;
        }
        let viewport_rows = u16::try_from(rows).unwrap_or(u16::MAX).max(1);
        if self.viewport_initialized {
            self.cursor_from_bottom = if scrollback >= self.viewport_scrollback {
                self.cursor_from_bottom
                    .saturating_add(scrollback - self.viewport_scrollback)
            } else {
                self.cursor_from_bottom
                    .saturating_sub(self.viewport_scrollback - scrollback)
            };
        } else {
            self.cursor_from_bottom = scrollback.saturating_add(
                u32::from(viewport_rows.saturating_sub(1).saturating_sub(self.cursor_row)),
            );
            self.viewport_initialized = true;
        }
        self.viewport_scrollback = scrollback;
        self.viewport_rows = viewport_rows;
    }

    pub fn move_left(&mut self) {
        self.cursor_col = self.cursor_col.saturating_sub(1);
    }

    pub fn move_right(&mut self, cols: usize) {
        self.cursor_col = self.cursor_col.saturating_add(1).min(terminal_index(cols));
    }

    pub fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_from_bottom = self.cursor_from_bottom.saturating_add(1);
        }
    }

    pub fn move_down(&mut self, rows: usize) {
        let next = self.cursor_row.saturating_add(1).min(terminal_index(rows));
        if next != self.cursor_row {
            self.cursor_row = next;
            self.cursor_from_bottom = self.cursor_from_bottom.saturating_sub(1);
        }
    }

    pub fn move_line_start(&mut self) {
        self.cursor_col = 0;
    }

    pub fn move_line_end(&mut self, line: &str) {
        self.cursor_col = terminal_index(UnicodeWidthStr::width(line.trim_end()));
    }

    pub fn move_top(&mut self) {
        self.cursor_row = 0;
        self.cursor_from_bottom = self.viewport_scrollback.saturating_add(u32::from(
            self.viewport_rows.saturating_sub(1),
        ));
    }

    pub fn move_bottom(&mut self, rows: usize) {
        self.cursor_row = terminal_index(rows);
        self.cursor_from_bottom = self.viewport_scrollback;
    }

    pub fn start_selection(&mut self) {
        if self.anchor.is_none() {
            self.anchor = Some(self.cursor_history_position());
        } else {
            self.anchor = None;
        }
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    pub fn selection(&self) -> Option<Selection> {
        self.history_selection()
            .and_then(|selection| self.visible_selection(selection))
    }

    pub fn cursor_selection(&self) -> Selection {
        Selection::new(
            self.cursor_row,
            self.cursor_col,
            self.cursor_row,
            self.cursor_col,
        )
    }

    pub fn history_selection(&self) -> Option<HistorySelection> {
        self.anchor.map(|anchor| normalize_history_selection(anchor, self.cursor_history_position()))
    }

    pub fn cursor_history_selection(&self) -> HistorySelection {
        let cursor = self.cursor_history_position();
        HistorySelection {
            start: cursor,
            end: cursor,
        }
    }

    fn cursor_history_position(&self) -> HistoryPosition {
        HistoryPosition {
            from_bottom: self.cursor_from_bottom,
            col: self.cursor_col,
        }
    }

    fn visible_selection(&self, selection: HistorySelection) -> Option<Selection> {
        let oldest_visible = self
            .viewport_scrollback
            .saturating_add(u32::from(self.viewport_rows.saturating_sub(1)));
        let newest_visible = self.viewport_scrollback;
        if selection.start.from_bottom < newest_visible || selection.end.from_bottom > oldest_visible {
            return None;
        }

        let start = if selection.start.from_bottom > oldest_visible {
            (0, 0)
        } else {
            (
                history_row_to_viewport(
                    selection.start.from_bottom,
                    self.viewport_scrollback,
                    self.viewport_rows,
                ),
                selection.start.col,
            )
        };
        let end = if selection.end.from_bottom < newest_visible {
            (self.viewport_rows.saturating_sub(1), u16::MAX)
        } else {
            (
                history_row_to_viewport(
                    selection.end.from_bottom,
                    self.viewport_scrollback,
                    self.viewport_rows,
                ),
                selection.end.col,
            )
        };
        Some(Selection::new(start.0, start.1, end.0, end.1))
    }
}

fn normalize_history_selection(
    first: HistoryPosition,
    second: HistoryPosition,
) -> HistorySelection {
    if (first.from_bottom, std::cmp::Reverse(first.col))
        >= (second.from_bottom, std::cmp::Reverse(second.col))
    {
        HistorySelection {
            start: first,
            end: second,
        }
    } else {
        HistorySelection {
            start: second,
            end: first,
        }
    }
}

fn history_row_to_viewport(from_bottom: u32, scrollback: u32, rows: u16) -> u16 {
    u16::try_from(
        u32::from(rows.saturating_sub(1))
            .saturating_sub(from_bottom.saturating_sub(scrollback)),
    )
    .unwrap_or(0)
}

fn terminal_index(length: usize) -> u16 {
    u16::try_from(length.saturating_sub(1)).unwrap_or(u16::MAX)
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
        mode.sync_viewport(5, 6, 0);
        mode.start_selection();
        mode.move_down(5);
        mode.move_right(6);

        assert_eq!(mode.selection(), Some(Selection::new(1, 2, 2, 3)));
    }

    #[test]
    fn copy_selection_keeps_history_positions_across_page_scrolling() {
        let mut mode = CopyMode::new(1, 1, 2);
        mode.sync_viewport(4, 8, 0);
        mode.start_selection();

        mode.sync_viewport(4, 8, 2);
        mode.move_down(4);

        assert_eq!(
            mode.history_selection(),
            Some(HistorySelection {
                start: HistoryPosition {
                    from_bottom: 3,
                    col: 2,
                },
                end: HistoryPosition {
                    from_bottom: 2,
                    col: 2,
                },
            })
        );
        assert_eq!(mode.selection(), Some(Selection::new(2, 2, 3, 2)));
    }

    #[test]
    fn line_end_stops_at_content_not_pane_width() {
        let mut mode = CopyMode::new(1, 0, 0);

        mode.move_line_end("hello     ");

        assert_eq!(mode.cursor_col, 4);
    }

    #[test]
    fn line_end_uses_terminal_cell_width() {
        let mut mode = CopyMode::new(1, 0, 0);

        mode.move_line_end("a界  ");

        assert_eq!(mode.cursor_col, 2);
    }

    #[test]
    fn copy_mode_movement_saturates_at_terminal_coordinate_limits() {
        let mut mode = CopyMode::new(1, u16::MAX, u16::MAX);
        mode.move_right(usize::MAX);
        mode.move_down(usize::MAX);
        assert_eq!((mode.cursor_row, mode.cursor_col), (u16::MAX, u16::MAX));
    }
}
use unicode_width::UnicodeWidthStr;
