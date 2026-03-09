use crossterm::{
    cursor::MoveTo,
    queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{Clear, ClearType},
};
use std::io::Write;

use crate::copy_mode::Selection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub width: u16,
    pub height: u16,
}

pub fn render_session<W: Write>(
    out: &mut W,
    session: &str,
    preview: &str,
    formatted_preview: &str,
    formatted_cursor: &str,
    status_message: Option<&str>,
    selection: Option<Selection>,
    size: TerminalSize,
) -> std::io::Result<()> {
    queue!(out, Clear(ClearType::All), MoveTo(0, 0))?;
    out.write_all(formatted_preview.as_bytes())?;
    if let Some(selection) = selection {
        render_selection_overlay(out, preview, selection, size)?;
    }

    let status = match status_message {
        Some(message) => format!(" admux | session: {session} | {message} "),
        None => format!(" admux | session: {session} | Ctrl-b d detach "),
    };
    let status_row = size.height.saturating_sub(1);
    queue!(
        out,
        MoveTo(0, status_row),
        SetAttribute(Attribute::Reverse),
        Print(truncate(&status, size.width)),
        SetAttribute(Attribute::Reset)
    )?;
    out.write_all(formatted_cursor.as_bytes())?;
    out.flush()
}

fn truncate(value: &str, width: u16) -> String {
    value.chars().take(width as usize).collect()
}

fn render_selection_overlay<W: Write>(
    out: &mut W,
    preview: &str,
    selection: Selection,
    size: TerminalSize,
) -> std::io::Result<()> {
    let selection = selection.normalized();
    let max_row = size.height.saturating_sub(1);
    let lines: Vec<Vec<char>> = preview.lines().map(|line| line.chars().collect()).collect();

    for row in selection.start_row..=selection.end_row {
        if row >= max_row {
            break;
        }

        let line = lines.get(row as usize);
        let start_col = if row == selection.start_row {
            selection.start_col
        } else {
            0
        };
        let end_col = if row == selection.end_row {
            selection.end_col
        } else {
            size.width.saturating_sub(1)
        };

        for col in start_col..=end_col.min(size.width.saturating_sub(1)) {
            let ch = line
                .and_then(|chars| chars.get(col as usize))
                .copied()
                .unwrap_or(' ');
            queue!(
                out,
                MoveTo(col, row),
                SetAttribute(Attribute::Reverse),
                Print(ch)
            )?;
        }
    }

    queue!(out, SetAttribute(Attribute::Reset))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_includes_status_and_preview() {
        let mut buf = Vec::new();
        render_session(
            &mut buf,
            "work",
            "hello\nworld",
            "\u{1b}[31mhello\u{1b}[0m\nworld",
            "\u{1b}[2;3H",
            Some("copied 5 chars"),
            None,
            TerminalSize {
                width: 40,
                height: 6,
            },
        )
        .expect("render session");
        let rendered = String::from_utf8_lossy(&buf);

        assert!(rendered.contains("hello"));
        assert!(rendered.contains("\u{1b}[31m"));
        assert!(rendered.contains("\u{1b}[2;3H"));
        assert!(rendered.contains("copied 5 chars"));
    }

    #[test]
    fn render_highlights_selected_text() {
        let mut buf = Vec::new();
        render_session(
            &mut buf,
            "work",
            "hello\nworld",
            "hello\nworld",
            "\u{1b}[1;1H",
            None,
            Some(Selection::new(0, 1, 0, 3)),
            TerminalSize {
                width: 20,
                height: 4,
            },
        )
        .expect("render session");
        let rendered = String::from_utf8_lossy(&buf);

        assert!(rendered.contains("\u{1b}[7m"));
        assert!(rendered.contains("\u{1b}[1;2H"));
        assert!(rendered.contains("\u{1b}[1;4H"));
    }
}
