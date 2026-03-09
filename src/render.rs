use std::io::Write;

use crossterm::{
    cursor::{MoveTo, Show},
    queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{Clear, ClearType},
};

use crate::{
    copy_mode::Selection,
    ipc::{PaneRender, RenderSnapshot},
    pane::Rect,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub width: u16,
    pub height: u16,
}

pub fn render_session<W: Write>(
    out: &mut W,
    session: &str,
    snapshot: &RenderSnapshot,
    status_message: Option<&str>,
    selection: Option<PaneSelection>,
    size: TerminalSize,
) -> std::io::Result<()> {
    queue!(out, Clear(ClearType::All), MoveTo(0, 0))?;

    for pane in &snapshot.panes {
        render_pane(out, pane)?;
    }
    if let Some(selection) = selection {
        render_selection_overlay(out, snapshot, selection)?;
    }
    render_statusline(out, session, snapshot, status_message, size)?;
    render_cursor(out, snapshot)?;
    out.flush()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneSelection {
    pub pane_id: u64,
    pub selection: Selection,
}

fn render_pane<W: Write>(out: &mut W, pane: &PaneRender) -> std::io::Result<()> {
    let border = if pane.focused { '#' } else { '|' };
    let horizontal = if pane.focused { '=' } else { '-' };
    draw_box(out, pane.rect, border, horizontal, &pane.title)?;
    let content = pane.rect.content();
    for (offset, row) in pane.rows_formatted.iter().enumerate() {
        if offset as u16 >= content.height {
            break;
        }
        queue!(out, MoveTo(content.x, content.y + offset as u16))?;
        out.write_all(row.as_bytes())?;
    }
    Ok(())
}

fn draw_box<W: Write>(
    out: &mut W,
    rect: Rect,
    border: char,
    horizontal: char,
    title: &str,
) -> std::io::Result<()> {
    if rect.width < 2 || rect.height < 2 {
        return Ok(());
    }

    let top = format!(
        "{border}{}{border}",
        fit_center(title, rect.width.saturating_sub(2), horizontal)
    );
    let bottom = format!("{border}{}{border}", horizontal.to_string().repeat(rect.width.saturating_sub(2) as usize));
    queue!(out, MoveTo(rect.x, rect.y), Print(top))?;
    for row in rect.y.saturating_add(1)..rect.y.saturating_add(rect.height.saturating_sub(1)) {
        queue!(
            out,
            MoveTo(rect.x, row),
            Print(border),
            MoveTo(rect.x + rect.width.saturating_sub(1), row),
            Print(border)
        )?;
    }
    queue!(
        out,
        MoveTo(rect.x, rect.y + rect.height.saturating_sub(1)),
        Print(bottom)
    )?;
    Ok(())
}

fn render_statusline<W: Write>(
    out: &mut W,
    session: &str,
    snapshot: &RenderSnapshot,
    status_message: Option<&str>,
    size: TerminalSize,
) -> std::io::Result<()> {
    let left = format!(" admux | {session} | ");
    let windows = snapshot
        .windows
        .iter()
        .map(|window| {
            if window.active {
                format!("[{}:{}]", window.index, window.name)
            } else {
                format!(" {}:{} ", window.index, window.name)
            }
        })
        .collect::<Vec<_>>()
        .join("");
    let right = match status_message {
        Some(message) => format!(" {message} "),
        None => format!(" pane {} | Ctrl-b d ", snapshot.active_pane_id),
    };
    let status = fit_status_segments(&left, &windows, &right, size.width);
    queue!(
        out,
        MoveTo(0, size.height.saturating_sub(1)),
        SetAttribute(Attribute::Reverse),
        Print(status),
        SetAttribute(Attribute::Reset)
    )?;
    Ok(())
}

fn render_cursor<W: Write>(out: &mut W, snapshot: &RenderSnapshot) -> std::io::Result<()> {
    if let Some(pane) = snapshot.panes.iter().find(|pane| pane.focused)
        && let Some(cursor) = &pane.cursor
    {
        let content = pane.rect.content();
        queue!(
            out,
            Show,
            MoveTo(content.x + cursor.col, content.y + cursor.row)
        )?;
    }
    Ok(())
}

fn render_selection_overlay<W: Write>(
    out: &mut W,
    snapshot: &RenderSnapshot,
    selection: PaneSelection,
) -> std::io::Result<()> {
    let Some(pane) = snapshot.panes.iter().find(|pane| pane.pane_id == selection.pane_id) else {
        return Ok(());
    };
    let content = pane.rect.content();
    let selection = selection.selection.normalized();
    for row in selection.start_row..=selection.end_row {
        if row >= content.height {
            break;
        }
        let line = pane.rows_plain.get(row as usize);
        let start_col = if row == selection.start_row {
            selection.start_col
        } else {
            0
        };
        let end_col = if row == selection.end_row {
            selection.end_col
        } else {
            content.width.saturating_sub(1)
        };
        for col in start_col..=end_col.min(content.width.saturating_sub(1)) {
            let ch = line
                .and_then(|line| line.chars().nth(col as usize))
                .unwrap_or(' ');
            queue!(
                out,
                MoveTo(content.x + col, content.y + row),
                SetAttribute(Attribute::Reverse),
                Print(ch)
            )?;
        }
    }
    queue!(out, SetAttribute(Attribute::Reset))?;
    Ok(())
}

fn fit_status_segments(left: &str, center: &str, right: &str, width: u16) -> String {
    let width = width as usize;
    if width == 0 {
        return String::new();
    }
    let mut status = String::new();
    status.push_str(left);
    let remaining = width.saturating_sub(status.chars().count() + right.chars().count());
    status.push_str(&center.chars().take(remaining).collect::<String>());
    let used = status.chars().count() + right.chars().count();
    if used < width {
        status.push_str(&" ".repeat(width - used));
    }
    status.push_str(right);
    status.chars().take(width).collect()
}

fn fit_center(title: &str, width: u16, fill: char) -> String {
    let title = title.chars().take(width as usize).collect::<String>();
    if title.chars().count() >= width as usize {
        return title;
    }
    let padding = width as usize - title.chars().count();
    let left = padding / 2;
    let right = padding - left;
    format!(
        "{}{}{}",
        fill.to_string().repeat(left),
        title,
        fill.to_string().repeat(right)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{PaneCursor, PaneRender, RenderSnapshot};
    use crate::window::WindowSummary;

    fn sample_snapshot() -> RenderSnapshot {
        RenderSnapshot {
            windows: vec![WindowSummary {
                id: 1,
                index: 0,
                name: "shell".into(),
                active: true,
            }],
            panes: vec![PaneRender {
                pane_id: 1,
                title: "shell".into(),
                rect: Rect {
                    x: 0,
                    y: 0,
                    width: 20,
                    height: 5,
                },
                focused: true,
                rows_plain: vec!["hello".into()],
                rows_formatted: vec!["\u{1b}[31mhello\u{1b}[0m".into()],
                cursor: Some(PaneCursor { row: 0, col: 2 }),
            }],
            active_window_id: 1,
            active_pane_id: 1,
        }
    }

    #[test]
    fn render_includes_status_and_preview() {
        let mut buf = Vec::new();
        render_session(
            &mut buf,
            "work",
            &sample_snapshot(),
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
        assert!(rendered.contains("copied 5 chars"));
        assert!(rendered.contains("\u{1b}[?25h"));
    }

    #[test]
    fn render_highlights_selected_text() {
        let mut buf = Vec::new();
        render_session(
            &mut buf,
            "work",
            &sample_snapshot(),
            None,
            Some(PaneSelection {
                pane_id: 1,
                selection: Selection::new(0, 1, 0, 3),
            }),
            TerminalSize {
                width: 20,
                height: 6,
            },
        )
        .expect("render session");
        let rendered = String::from_utf8_lossy(&buf);

        assert!(rendered.contains("\u{1b}[7m"));
        assert!(rendered.contains("\u{1b}[2;3H"));
    }

    #[test]
    fn status_line_pads_to_terminal_width() {
        let status = fit_status_segments("left", "center", "right", 20);
        assert_eq!(status.chars().count(), 20);
    }
}
