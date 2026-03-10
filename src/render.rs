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

const CONNECT_UP: u8 = 0b0001;
const CONNECT_DOWN: u8 = 0b0010;
const CONNECT_LEFT: u8 = 0b0100;
const CONNECT_RIGHT: u8 = 0b1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneSelection {
    pub pane_id: u64,
    pub selection: Selection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomBar<'a> {
    Status {
        message: Option<&'a str>,
    },
    Prompt {
        buffer: &'a str,
        completions: &'a [String],
        selected: usize,
        cursor: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeLine {
    pub depth: usize,
    pub label: String,
    pub selected: bool,
    pub expanded: bool,
    pub has_children: bool,
}

pub fn render_session<W: Write>(
    out: &mut W,
    session: &str,
    snapshot: &RenderSnapshot,
    bottom_bar: BottomBar<'_>,
    selection: Option<PaneSelection>,
    size: TerminalSize,
) -> std::io::Result<()> {
    queue!(out, Clear(ClearType::All), MoveTo(0, 0))?;

    for pane in &snapshot.panes {
        render_pane(out, pane)?;
    }
    render_split_separators(out, snapshot)?;
    if let Some(selection) = selection {
        render_selection_overlay(out, snapshot, selection)?;
    }
    let prompt_cursor = render_bottom_bar(out, session, snapshot, bottom_bar, size)?;
    if let Some(col) = prompt_cursor {
        queue!(out, Show, MoveTo(col, size.height.saturating_sub(1)))?;
    } else {
        render_cursor(out, snapshot)?;
    }
    out.flush()
}

pub fn render_choose_tree<W: Write>(
    out: &mut W,
    session: &str,
    current_snapshot: &RenderSnapshot,
    lines: &[TreeLine],
    preview_title: &str,
    preview_snapshot: &RenderSnapshot,
    bottom_message: &str,
    size: TerminalSize,
) -> std::io::Result<()> {
    queue!(out, Clear(ClearType::All), MoveTo(0, 0))?;

    let body_height = size.height.saturating_sub(1);
    let list_height = body_height.min((lines.len() as u16).saturating_add(1).min(8));

    for (index, line) in lines.iter().enumerate() {
        if index as u16 >= list_height {
            break;
        }
        let row = index as u16;
        let prefix = if line.has_children && line.depth == 0 {
            if line.expanded { "-" } else { "+" }
        } else {
            " "
        };
        let content = format!("{}{}{}", "  ".repeat(line.depth), prefix, line.label);
        queue!(out, MoveTo(0, row))?;
        if line.selected {
            queue!(
                out,
                SetAttribute(Attribute::Reverse),
                Print(fit_width(&content, size.width)),
                SetAttribute(Attribute::Reset)
            )?;
        } else {
            queue!(out, Print(fit_width(&content, size.width)))?;
        }
    }

    queue!(
        out,
        MoveTo(0, list_height),
        Print(fit_width(
            &format!(
                " {preview_title} {}",
                "-".repeat(size.width.saturating_sub(preview_title.len() as u16 + 2) as usize)
            ),
            size.width,
        ))
    )?;
    let preview_area = Rect {
        x: 0,
        y: list_height.saturating_add(1),
        width: size.width,
        height: body_height.saturating_sub(list_height.saturating_add(1)),
    };
    if preview_area.height > 0 {
        render_preview_snapshot(out, preview_snapshot, preview_area)?;
    }

    render_bottom_bar(
        out,
        session,
        current_snapshot,
        BottomBar::Status {
            message: Some(bottom_message),
        },
        size,
    )?;
    out.flush()
}

pub fn render_help_overlay<W: Write>(
    out: &mut W,
    session: &str,
    snapshot: &RenderSnapshot,
    lines: &[String],
    size: TerminalSize,
) -> std::io::Result<()> {
    queue!(out, Clear(ClearType::All), MoveTo(0, 0))?;
    let body_height = size.height.saturating_sub(1);

    for (index, line) in lines.iter().enumerate() {
        if index as u16 >= body_height {
            break;
        }
        queue!(
            out,
            MoveTo(0, index as u16),
            Print(fit_width(line, size.width))
        )?;
    }

    render_bottom_bar(
        out,
        session,
        snapshot,
        BottomBar::Status {
            message: Some("help | q cancel"),
        },
        size,
    )?;
    out.flush()
}

fn render_pane<W: Write>(out: &mut W, pane: &PaneRender) -> std::io::Result<()> {
    for (offset, row) in pane.rows_formatted.iter().enumerate() {
        if offset as u16 >= pane.rect.height {
            break;
        }
        queue!(out, MoveTo(pane.rect.x, pane.rect.y + offset as u16))?;
        out.write_all(row.as_bytes())?;
    }
    Ok(())
}

fn render_split_separators<W: Write>(
    out: &mut W,
    snapshot: &RenderSnapshot,
) -> std::io::Result<()> {
    use std::collections::BTreeMap;

    let panes = &snapshot.panes;
    let mut grid = BTreeMap::<(u16, u16), u8>::new();

    for left in panes {
        for right in panes {
            if left.pane_id == right.pane_id {
                continue;
            }
            if left.rect.x + left.rect.width + 1 == right.rect.x {
                let start = left.rect.y.max(right.rect.y);
                let end = (left.rect.y + left.rect.height).min(right.rect.y + right.rect.height);
                let x = left.rect.x + left.rect.width;
                for row in start..end {
                    let entry = grid.entry((x, row)).or_insert(0);
                    if row > start {
                        *entry |= CONNECT_UP;
                    }
                    if row + 1 < end {
                        *entry |= CONNECT_DOWN;
                    }
                }
            }
            if left.rect.y + left.rect.height + 1 == right.rect.y {
                let start = left.rect.x.max(right.rect.x);
                let end = (left.rect.x + left.rect.width).min(right.rect.x + right.rect.width);
                let y = left.rect.y + left.rect.height;
                for col in start..end {
                    let entry = grid.entry((col, y)).or_insert(0);
                    if col > start {
                        *entry |= CONNECT_LEFT;
                    }
                    if col + 1 < end {
                        *entry |= CONNECT_RIGHT;
                    }
                }
                if start > 0 && grid.contains_key(&(start - 1, y)) {
                    *grid.entry((start - 1, y)).or_insert(0) |= CONNECT_RIGHT;
                    *grid.entry((start, y)).or_insert(0) |= CONNECT_LEFT;
                }
                if grid.contains_key(&(end, y)) {
                    *grid.entry((end, y)).or_insert(0) |= CONNECT_LEFT;
                    *grid.entry((end - 1, y)).or_insert(0) |= CONNECT_RIGHT;
                }
            }
        }
    }

    for ((x, y), mask) in grid {
        queue!(out, MoveTo(x, y), Print(connection_glyph(mask)))?;
    }
    Ok(())
}

fn connection_glyph(mask: u8) -> char {
    match mask {
        m if m == (CONNECT_UP | CONNECT_DOWN) => '│',
        m if m == (CONNECT_LEFT | CONNECT_RIGHT) => '─',
        m if m == (CONNECT_UP | CONNECT_DOWN | CONNECT_RIGHT) => '├',
        m if m == (CONNECT_UP | CONNECT_DOWN | CONNECT_LEFT) => '┤',
        m if m == (CONNECT_LEFT | CONNECT_RIGHT | CONNECT_DOWN) => '┬',
        m if m == (CONNECT_LEFT | CONNECT_RIGHT | CONNECT_UP) => '┴',
        m if m == (CONNECT_UP | CONNECT_DOWN | CONNECT_LEFT | CONNECT_RIGHT) => '┼',
        m if m == (CONNECT_DOWN | CONNECT_RIGHT) => '┌',
        m if m == (CONNECT_DOWN | CONNECT_LEFT) => '┐',
        m if m == (CONNECT_UP | CONNECT_RIGHT) => '└',
        m if m == (CONNECT_UP | CONNECT_LEFT) => '┘',
        m if m & (CONNECT_UP | CONNECT_DOWN) != 0 => '│',
        m if m & (CONNECT_LEFT | CONNECT_RIGHT) != 0 => '─',
        _ => ' ',
    }
}

fn render_preview_snapshot<W: Write>(
    out: &mut W,
    snapshot: &RenderSnapshot,
    area: Rect,
) -> std::io::Result<()> {
    if snapshot.panes.is_empty() || area.width < 8 || area.height < 4 {
        return Ok(());
    }

    let source_width = snapshot
        .panes
        .iter()
        .map(|pane| pane.rect.right())
        .max()
        .unwrap_or(1)
        .max(1);
    let source_height = snapshot
        .panes
        .iter()
        .map(|pane| pane.rect.bottom())
        .max()
        .unwrap_or(1)
        .max(1);

    for pane in &snapshot.panes {
        let scaled = scale_rect(pane.rect, area, source_width, source_height);
        if scaled.width < 6 || scaled.height < 4 {
            continue;
        }
        draw_preview_box(out, scaled, pane.focused, &pane.title)?;
        let content = scaled.content();
        for (offset, row) in pane.rows_plain.iter().enumerate() {
            if offset as u16 >= content.height {
                break;
            }
            queue!(
                out,
                MoveTo(content.x, content.y + offset as u16),
                Print(truncate(row, content.width))
            )?;
        }
    }

    Ok(())
}

fn scale_rect(source: Rect, area: Rect, source_width: u16, source_height: u16) -> Rect {
    let mut x =
        area.x + ((u32::from(source.x) * u32::from(area.width)) / u32::from(source_width)) as u16;
    let mut y =
        area.y + ((u32::from(source.y) * u32::from(area.height)) / u32::from(source_height)) as u16;
    let mut width = ((u32::from(source.width.max(1)) * u32::from(area.width))
        / u32::from(source_width))
    .max(6) as u16;
    let mut height = ((u32::from(source.height.max(1)) * u32::from(area.height))
        / u32::from(source_height))
    .max(4) as u16;

    if x >= area.right() {
        x = area.right().saturating_sub(1);
    }
    if y >= area.bottom() {
        y = area.bottom().saturating_sub(1);
    }
    width = width.min(area.right().saturating_sub(x));
    height = height.min(area.bottom().saturating_sub(y));

    Rect {
        x,
        y,
        width,
        height,
    }
}

fn draw_preview_box<W: Write>(
    out: &mut W,
    rect: Rect,
    focused: bool,
    title: &str,
) -> std::io::Result<()> {
    if rect.width < 2 || rect.height < 2 {
        return Ok(());
    }
    let border_attr = if focused {
        Attribute::Bold
    } else {
        Attribute::Dim
    };
    let top = format!("┌{}┐", "─".repeat(rect.width.saturating_sub(2) as usize));
    let bottom = format!("└{}┘", "─".repeat(rect.width.saturating_sub(2) as usize));
    queue!(
        out,
        SetAttribute(border_attr),
        MoveTo(rect.x, rect.y),
        Print(top)
    )?;
    for row in rect.y.saturating_add(1)..rect.bottom().saturating_sub(1) {
        queue!(
            out,
            MoveTo(rect.x, row),
            Print("│"),
            MoveTo(rect.right().saturating_sub(1), row),
            Print("│")
        )?;
    }
    queue!(
        out,
        MoveTo(rect.x, rect.bottom().saturating_sub(1)),
        Print(bottom),
        SetAttribute(Attribute::Reset)
    )?;
    if rect.width > 4 {
        queue!(
            out,
            MoveTo(rect.x.saturating_add(2), rect.y),
            Print(truncate(title, rect.width.saturating_sub(4)))
        )?;
    }
    Ok(())
}

fn render_bottom_bar<W: Write>(
    out: &mut W,
    session: &str,
    snapshot: &RenderSnapshot,
    bottom_bar: BottomBar<'_>,
    size: TerminalSize,
) -> std::io::Result<Option<u16>> {
    let content = match bottom_bar {
        BottomBar::Status { message } => {
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
            let right = match message {
                Some(message) => format!(" {message} "),
                None => format!(" pane {} | Ctrl-b d ", snapshot.active_pane_id),
            };
            fit_status_segments(&left, &windows, &right, size.width)
        }
        BottomBar::Prompt {
            buffer,
            completions,
            selected,
            cursor,
        } => {
            let line = render_prompt_line(buffer, completions, selected, size.width);
            queue!(out, MoveTo(0, size.height.saturating_sub(1)), Print(line))?;
            return Ok(Some(
                (1 + cursor).min(size.width.saturating_sub(1) as usize) as u16,
            ));
        }
    };

    queue!(
        out,
        MoveTo(0, size.height.saturating_sub(1)),
        SetAttribute(Attribute::Reverse),
        Print(content),
        SetAttribute(Attribute::Reset)
    )?;
    Ok(None)
}

fn render_cursor<W: Write>(out: &mut W, snapshot: &RenderSnapshot) -> std::io::Result<()> {
    if let Some(pane) = snapshot.panes.iter().find(|pane| pane.focused)
        && let Some(cursor) = &pane.cursor
    {
        queue!(
            out,
            Show,
            MoveTo(pane.rect.x + cursor.col, pane.rect.y + cursor.row)
        )?;
    }
    Ok(())
}

fn render_selection_overlay<W: Write>(
    out: &mut W,
    snapshot: &RenderSnapshot,
    selection: PaneSelection,
) -> std::io::Result<()> {
    let Some(pane) = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == selection.pane_id)
    else {
        return Ok(());
    };
    let selection = selection.selection.normalized();
    for row in selection.start_row..=selection.end_row {
        if row >= pane.rect.height {
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
            pane.rect.width.saturating_sub(1)
        };
        for col in start_col..=end_col.min(pane.rect.width.saturating_sub(1)) {
            let ch = line
                .and_then(|line| line.chars().nth(col as usize))
                .unwrap_or(' ');
            queue!(
                out,
                MoveTo(pane.rect.x + col, pane.rect.y + row),
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

fn render_prompt_line(buffer: &str, completions: &[String], selected: usize, width: u16) -> String {
    let mut line = format!(":{}", buffer);
    if !completions.is_empty() {
        let rendered = completions
            .iter()
            .enumerate()
            .map(|(index, completion)| {
                if index == selected {
                    format!("[{completion}]")
                } else {
                    completion.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        line.push(' ');
        line.push_str(&rendered);
    }
    fit_width(&line, width)
}

fn fit_width(value: &str, width: u16) -> String {
    let mut fitted: String = value.chars().take(width as usize).collect();
    let current = fitted.chars().count();
    if current < width as usize {
        fitted.push_str(&" ".repeat(width as usize - current));
    }
    fitted
}

fn truncate(value: &str, width: u16) -> String {
    value.chars().take(width as usize).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::{PaneCursor, PaneRender, RenderSnapshot};
    use crate::pane::Rect;
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
            BottomBar::Status {
                message: Some("copied 5 chars"),
            },
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
            BottomBar::Status { message: None },
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
        assert!(rendered.contains("\u{1b}[1;2H"));
    }

    #[test]
    fn render_uses_only_internal_separators() {
        let mut snapshot = sample_snapshot();
        snapshot.panes.push(PaneRender {
            pane_id: 2,
            title: "right".into(),
            rect: Rect {
                x: 11,
                y: 0,
                width: 10,
                height: 4,
            },
            focused: false,
            rows_formatted: vec!["world".into()],
            rows_plain: vec!["world".into()],
            cursor: None,
        });
        let mut buf = Vec::new();
        render_session(
            &mut buf,
            "work",
            &snapshot,
            BottomBar::Status { message: None },
            None,
            TerminalSize {
                width: 40,
                height: 8,
            },
        )
        .expect("render session");
        let rendered = String::from_utf8_lossy(&buf);

        assert!(!rendered.contains("#"));
        assert!(!rendered.contains("="));
    }

    #[test]
    fn prompt_line_includes_completion_list() {
        let completions = vec!["split-window".to_string(), "switch-client".to_string()];
        let line = render_prompt_line("sp", &completions, 0, 40);
        assert!(line.contains(":sp [split-window] switch-client"));
    }

    #[test]
    fn help_overlay_renders_content() {
        let mut buf = Vec::new();
        render_help_overlay(
            &mut buf,
            "work",
            &sample_snapshot(),
            &["Keys".into(), "Ctrl-b ?".into()],
            TerminalSize {
                width: 80,
                height: 6,
            },
        )
        .expect("render help");
        let rendered = String::from_utf8_lossy(&buf);

        assert!(rendered.contains("Keys"));
        assert!(rendered.contains("Ctrl-b ?"));
        assert!(rendered.contains("help | q cancel"));
    }
}
