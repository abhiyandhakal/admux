use std::io::Write;

use crossterm::{
    cursor::{MoveTo, Show},
    queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{Clear, ClearType},
};

use crate::{
    config::{StatusPosition, UiConfig},
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
    CopyMode,
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
    ui: &UiConfig,
    size: TerminalSize,
) -> std::io::Result<()> {
    queue!(out, Clear(ClearType::All), MoveTo(0, 0))?;

    for pane in &snapshot.panes {
        render_pane(out, pane, ui)?;
    }
    render_split_separators(out, snapshot, ui)?;
    if let Some(selection) = selection {
        render_selection_overlay(out, snapshot, selection, ui)?;
    }
    let prompt_cursor = render_bottom_bar(out, session, snapshot, bottom_bar, ui, size)?;
    if let Some(col) = prompt_cursor {
        queue!(out, Show, MoveTo(col, status_row(ui, size)))?;
    } else {
        render_cursor(out, snapshot, ui)?;
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
    ui: &UiConfig,
    size: TerminalSize,
) -> std::io::Result<()> {
    queue!(out, Clear(ClearType::All), MoveTo(0, 0))?;

    let body_height = size.height.saturating_sub(1);
    let body_start = body_start_row(ui);
    let list_height = body_height.min((lines.len() as u16).saturating_add(1).min(8));

    for (index, line) in lines.iter().enumerate() {
        if index as u16 >= list_height {
            break;
        }
        let row = body_start + index as u16;
        let prefix = if line.has_children {
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
        MoveTo(0, body_start + list_height),
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
        y: body_start + list_height.saturating_add(1),
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
        ui,
        size,
    )?;
    out.flush()
}

pub fn render_help_overlay<W: Write>(
    out: &mut W,
    session: &str,
    snapshot: &RenderSnapshot,
    lines: &[String],
    ui: &UiConfig,
    size: TerminalSize,
) -> std::io::Result<()> {
    queue!(out, Clear(ClearType::All), MoveTo(0, 0))?;
    let body_height = size.height.saturating_sub(1);
    let body_start = body_start_row(ui);

    for (index, line) in lines.iter().enumerate() {
        if index as u16 >= body_height {
            break;
        }
        queue!(
            out,
            MoveTo(0, body_start + index as u16),
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
        ui,
        size,
    )?;
    out.flush()
}

fn render_pane<W: Write>(out: &mut W, pane: &PaneRender, ui: &UiConfig) -> std::io::Result<()> {
    for (offset, row) in pane.rows_formatted.iter().enumerate() {
        if offset as u16 >= pane.rect.height {
            break;
        }
        queue!(
            out,
            MoveTo(pane.rect.x, offset_row(pane.rect.y + offset as u16, ui))
        )?;
        out.write_all(row.as_bytes())?;
        out.write_all(b"\x1b[0m")?;
    }
    Ok(())
}

fn render_split_separators<W: Write>(
    out: &mut W,
    snapshot: &RenderSnapshot,
    ui: &UiConfig,
) -> std::io::Result<()> {
    for divider in &snapshot.dividers {
        queue!(
            out,
            MoveTo(divider.x, offset_row(divider.y, ui)),
            Print(connection_glyph(divider.mask))
        )?;
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
    ui: &UiConfig,
    size: TerminalSize,
) -> std::io::Result<Option<u16>> {
    let row = status_row(ui, size);
    let content = match bottom_bar {
        BottomBar::Status { message } => render_status_line(session, snapshot, message, ui, size.width),
        BottomBar::CopyMode => vec![StatusSegment::active(
            " COPY MODE | h j k l move | 0/$ line | g/G top/bottom | PgUp/PgDn scroll | Space select | y copy | q quit ",
        )],
        BottomBar::Prompt {
            buffer,
            completions,
            selected,
            cursor,
        } => {
            let line = render_prompt_line(buffer, completions, selected, size.width);
            queue!(
                out,
                MoveTo(0, row),
                SetAttribute(Attribute::Reverse),
                Print(line),
                SetAttribute(Attribute::Reset)
            )?;
            return Ok(Some(
                (1 + cursor).min(size.width.saturating_sub(1) as usize) as u16,
            ));
        }
    };

    render_status_segments(out, row, &content, size.width)?;
    Ok(None)
}

fn render_cursor<W: Write>(
    out: &mut W,
    snapshot: &RenderSnapshot,
    ui: &UiConfig,
) -> std::io::Result<()> {
    if let Some(pane) = snapshot.panes.iter().find(|pane| pane.focused)
        && let Some(cursor) = &pane.cursor
    {
        queue!(
            out,
            Show,
            MoveTo(
                pane.rect.x + cursor.col,
                offset_row(pane.rect.y + cursor.row, ui)
            )
        )?;
    }
    Ok(())
}

fn render_selection_overlay<W: Write>(
    out: &mut W,
    snapshot: &RenderSnapshot,
    selection: PaneSelection,
    ui: &UiConfig,
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
                MoveTo(pane.rect.x + col, offset_row(pane.rect.y + row, ui)),
                SetAttribute(Attribute::Reverse),
                Print(ch)
            )?;
        }
    }
    queue!(out, SetAttribute(Attribute::Reset))?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StatusSegment {
    text: String,
    attrs: Vec<Attribute>,
}

impl StatusSegment {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attrs: vec![Attribute::Reverse],
        }
    }

    fn active(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attrs: vec![Attribute::Reverse, Attribute::Bold],
        }
    }

    fn dim(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            attrs: vec![Attribute::Reverse, Attribute::Dim],
        }
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }
}

fn render_status_line(
    session: &str,
    snapshot: &RenderSnapshot,
    message: Option<&str>,
    ui: &UiConfig,
    width: u16,
) -> Vec<StatusSegment> {
    let active_window = snapshot.windows.iter().find(|window| window.active);
    let mut left = vec![
        StatusSegment::active(" admux "),
        StatusSegment::plain(format!(" {} ", session)),
    ];
    if let Some(window) = active_window {
        left.push(StatusSegment::active(format!(" {}:{} ", window.index, window.name)));
    }

    let mut center = if ui.status_show_window_list {
        snapshot
            .windows
            .iter()
            .map(|window| {
                if window.active {
                    StatusSegment::active(format!(" [{}:{}] ", window.index, window.name))
                } else {
                    StatusSegment::dim(format!(" {} ", window.index))
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    let mut right = Vec::new();
    if let Some(message) = message {
        right.push(StatusSegment::active(format!(" {} ", truncate(message, width))));
    } else if ui.status_show_pane {
        let pane_title = active_pane_title(snapshot);
        right.push(StatusSegment::plain(format!(
            " pane {}:{} ",
            snapshot.active_pane_id, pane_title
        )));
    }
    if ui.status_clock
        && let Some(clock) = local_clock()
    {
        right.push(StatusSegment::active(format!(" {} ", clock)));
    }

    fit_status_layout(&mut left, &mut center, &mut right, width);
    let mut result = left;
    result.extend(center);
    result.extend(right);
    result
}

fn fit_status_layout(
    left: &mut Vec<StatusSegment>,
    center: &mut Vec<StatusSegment>,
    right: &mut Vec<StatusSegment>,
    width: u16,
) {
    while total_len(left, center, right) > width as usize {
        if right.len() > 1 {
            right.pop();
            continue;
        }
        if left.len() > 2 {
            left.pop();
            continue;
        }
        if let Some(segment) = center
            .iter_mut()
            .find(|segment| segment.attrs.contains(&Attribute::Dim) && segment.text.contains(':'))
        {
            if let Some(index) = segment.text.find(':') {
                let prefix = segment.text[..index].trim();
                segment.text = format!(" {} ", prefix.trim_matches(['[', ']']));
                continue;
            }
        }
        if let Some(segment) = right.first_mut()
            && segment.len() > 12
        {
            segment.text = format!(" {} ", truncate(segment.text.trim(), 10));
            continue;
        }
        if right.len() == 1 && right.first().is_some_and(|segment| !segment.attrs.contains(&Attribute::Bold)) {
            right.clear();
            continue;
        }
        if let Some(segment) = left.get_mut(1)
            && segment.len() > 10
        {
            segment.text = format!(" {} ", truncate(segment.text.trim(), 8));
            continue;
        }
        if let Some(segment) = center
            .iter_mut()
            .find(|segment| segment.attrs.contains(&Attribute::Bold) && segment.text.contains(':'))
        {
            if let Some(index) = segment.text.find(':') {
                let prefix = segment.text[..index].trim();
                segment.text = format!(" [{}] ", prefix.trim_matches(['[', ']']));
                continue;
            }
        }
        break;
    }
}

fn total_len(
    left: &[StatusSegment],
    center: &[StatusSegment],
    right: &[StatusSegment],
) -> usize {
    left.iter()
        .chain(center.iter())
        .chain(right.iter())
        .map(StatusSegment::len)
        .sum()
}

fn render_status_segments<W: Write>(
    out: &mut W,
    row: u16,
    segments: &[StatusSegment],
    width: u16,
) -> std::io::Result<()> {
    queue!(out, MoveTo(0, row), SetAttribute(Attribute::Reverse))?;
    let mut written = 0usize;
    for segment in segments {
        queue!(out, SetAttribute(Attribute::Reset))?;
        for attr in &segment.attrs {
            queue!(out, SetAttribute(*attr))?;
        }
        let remaining = width as usize - written;
        if remaining == 0 {
            break;
        }
        let text = truncate(&segment.text, remaining as u16);
        written += text.chars().count();
        queue!(out, Print(text))?;
    }
    if written < width as usize {
        queue!(out, SetAttribute(Attribute::Reverse), Print(" ".repeat(width as usize - written)))?;
    }
    queue!(out, SetAttribute(Attribute::Reset))?;
    Ok(())
}

fn active_pane_title(snapshot: &RenderSnapshot) -> String {
    snapshot
        .panes
        .iter()
        .find(|pane| pane.focused)
        .map(|pane| {
            if pane.title.trim().is_empty() {
                format!("pane {}", pane.pane_id)
            } else {
                pane.title.clone()
            }
        })
        .unwrap_or_else(|| format!("pane {}", snapshot.active_pane_id))
}

fn local_clock() -> Option<String> {
    let now = time::OffsetDateTime::now_local().ok()?;
    let format =
        time::format_description::parse("[hour repr:24 padding:zero]:[minute padding:zero]")
            .ok()?;
    now.format(&format).ok()
}

fn status_row(ui: &UiConfig, size: TerminalSize) -> u16 {
    match ui.status_position {
        StatusPosition::Top => 0,
        StatusPosition::Bottom => size.height.saturating_sub(1),
    }
}

fn offset_row(row: u16, ui: &UiConfig) -> u16 {
    match ui.status_position {
        StatusPosition::Top => row.saturating_add(1),
        StatusPosition::Bottom => row,
    }
}

fn body_start_row(ui: &UiConfig) -> u16 {
    match ui.status_position {
        StatusPosition::Top => 1,
        StatusPosition::Bottom => 0,
    }
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
    use crate::config::{StatusPosition, StatusStyle, UiConfig};
    use crate::ipc::{PaneCursor, PaneRender, RenderSnapshot};
    use crate::layout::{LayoutTree, SplitAxis};
    use crate::pane::PaneId;
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
            dividers: Vec::new(),
            active_window_id: 1,
            active_pane_id: 1,
        }
    }

    fn sample_ui() -> UiConfig {
        UiConfig {
            status_position: StatusPosition::Bottom,
            show_pane_labels: true,
            status_clock: true,
            status_show_pane: true,
            status_show_window_list: true,
            status_style: StatusStyle::TmuxPlus,
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
            &sample_ui(),
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
        assert!(rendered.contains("\u{1b}[0m"));
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
            &sample_ui(),
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
            &sample_ui(),
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
            &sample_ui(),
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

    #[test]
    fn connection_mask_produces_joined_tee() {
        let mask = CONNECT_UP | CONNECT_DOWN | CONNECT_RIGHT;
        assert_eq!(connection_glyph(mask), '├');
    }

    #[test]
    fn copy_mode_bottom_bar_renders_help_text() {
        let mut buf = Vec::new();
        render_session(
            &mut buf,
            "work",
            &sample_snapshot(),
            BottomBar::CopyMode,
            None,
            &sample_ui(),
            TerminalSize {
                width: 140,
                height: 6,
            },
        )
        .expect("render session");
        let rendered = String::from_utf8_lossy(&buf);

        assert!(rendered.contains("COPY MODE"));
        assert!(rendered.contains("Space select"));
    }

    #[test]
    fn render_displays_mixed_axis_right_branch_junction() {
        let mut tree = LayoutTree::new(PaneId(1));
        tree.split_active(SplitAxis::Vertical, PaneId(2));
        tree.active = PaneId(2);
        tree.split_active(SplitAxis::Horizontal, PaneId(3));

        let mut snapshot = sample_snapshot();
        snapshot.dividers = tree.divider_cells(Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 8,
        });

        let mut buf = Vec::new();
        render_session(
            &mut buf,
            "work",
            &snapshot,
            BottomBar::Status { message: None },
            None,
            &sample_ui(),
            TerminalSize {
                width: 20,
                height: 9,
            },
        )
        .expect("render session");
        let rendered = String::from_utf8_lossy(&buf);

        assert!(rendered.contains('├'));
    }

    #[test]
    fn render_displays_mixed_axis_bottom_branch_junction() {
        let mut tree = LayoutTree::new(PaneId(1));
        tree.split_active(SplitAxis::Horizontal, PaneId(2));
        tree.active = PaneId(2);
        tree.split_active(SplitAxis::Vertical, PaneId(3));

        let mut snapshot = sample_snapshot();
        snapshot.dividers = tree.divider_cells(Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 8,
        });

        let mut buf = Vec::new();
        render_session(
            &mut buf,
            "work",
            &snapshot,
            BottomBar::Status { message: None },
            None,
            &sample_ui(),
            TerminalSize {
                width: 20,
                height: 9,
            },
        )
        .expect("render session");
        let rendered = String::from_utf8_lossy(&buf);

        assert!(rendered.contains('┬'));
    }

    #[test]
    fn status_line_prefers_message_over_pane_metadata() {
        let segments = render_status_line(
            "work",
            &sample_snapshot(),
            Some("copied 5 chars"),
            &sample_ui(),
            60,
        );
        let joined = segments.into_iter().map(|segment| segment.text).collect::<String>();
        assert!(joined.contains("copied 5 chars"));
        assert!(!joined.contains("pane 1:shell"));
    }

    #[test]
    fn status_line_compresses_inactive_windows_when_narrow() {
        let mut snapshot = sample_snapshot();
        snapshot.windows = vec![
            WindowSummary { id: 1, index: 0, name: "shell".into(), active: false },
            WindowSummary { id: 2, index: 1, name: "editor".into(), active: true },
            WindowSummary { id: 3, index: 2, name: "logs".into(), active: false },
        ];
        snapshot.active_window_id = 2;
        let joined = render_status_line("work", &snapshot, None, &sample_ui(), 32)
            .into_iter()
            .map(|segment| segment.text)
            .collect::<String>();
        assert!(joined.contains("[1:editor]"));
        assert!(joined.contains(" 0 "));
        assert!(joined.contains(" 2 "));
    }

    #[test]
    fn top_status_position_moves_bar_to_first_row() {
        let mut ui = sample_ui();
        ui.status_position = StatusPosition::Top;
        let mut buf = Vec::new();
        render_session(
            &mut buf,
            "work",
            &sample_snapshot(),
            BottomBar::Status { message: None },
            None,
            &ui,
            TerminalSize { width: 40, height: 6 },
        )
        .expect("render session");
        let rendered = String::from_utf8_lossy(&buf);
        assert!(rendered.contains("\u{1b}[1;1H"));
    }
}
