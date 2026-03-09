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
    formatted_preview: &str,
    formatted_cursor: &str,
    status_message: Option<&str>,
    size: TerminalSize,
) -> std::io::Result<()> {
    queue!(out, Clear(ClearType::All), MoveTo(0, 0))?;
    out.write_all(formatted_preview.as_bytes())?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_includes_status_and_preview() {
        let mut buf = Vec::new();
        render_session(
            &mut buf,
            "work",
            "\u{1b}[31mhello\u{1b}[0m\nworld",
            "\u{1b}[2;3H",
            Some("copied 5 chars"),
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
}
