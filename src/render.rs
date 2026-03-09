use crossterm::{
    cursor::MoveTo,
    queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{Clear, ClearType},
};
use std::io::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalSize {
    pub width: u16,
    pub height: u16,
}

pub fn render_session<W: Write>(
    out: &mut W,
    session: &str,
    preview: &str,
    size: TerminalSize,
) -> std::io::Result<()> {
    queue!(out, Clear(ClearType::All), MoveTo(0, 0))?;
    let content_height = size.height.saturating_sub(1) as usize;
    for (row, line) in preview.lines().take(content_height).enumerate() {
        queue!(
            out,
            MoveTo(0, row as u16),
            Print(truncate(line, size.width))
        )?;
    }

    let status = format!(" admux | session: {session} | Ctrl-b d detach ");
    let status_row = size.height.saturating_sub(1);
    queue!(
        out,
        MoveTo(0, status_row),
        SetAttribute(Attribute::Reverse),
        Print(truncate(&status, size.width)),
        SetAttribute(Attribute::Reset)
    )?;
    out.flush()
}

fn truncate(value: &str, width: u16) -> String {
    value.chars().take(width as usize).collect()
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
            TerminalSize {
                width: 40,
                height: 6,
            },
        )
        .expect("render session");
        let rendered = String::from_utf8_lossy(&buf);

        assert!(rendered.contains("hello"));
        assert!(rendered.contains("session: work"));
    }
}
