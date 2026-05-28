//! Centered help overlay listing all keybindings (toggled with `?`).

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// `(keys, description)` rows; an empty `keys` renders as a section heading.
const ROWS: &[(&str, &str)] = &[
    ("", "Playback / seek"),
    ("←  →", "seek 1 second"),
    ("<  >", "seek 10 seconds"),
    (",  .", "step one frame"),
    ("0–9", "jump to 0–90% of duration"),
    ("Home / End", "jump to start / end"),
    ("", "Cutting"),
    ("i  o", "set IN / OUT marker"),
    ("C", "clear markers"),
    ("x", "export clip — fast (stream copy)"),
    ("X", "export clip — precise (re-encode)"),
    ("", "  then type a filename, Enter to cut"),
    ("", "Subtitles"),
    ("j  k", "select previous / next cue (seeks to it)"),
    ("Enter", "edit selected cue text"),
    ("[  ]", "snap cue start / end to playhead"),
    ("n", "new cue at playhead"),
    ("d", "delete selected cue"),
    ("s", "save .vtt (backs up original to .vtt.orig)"),
    ("", "General"),
    ("?", "toggle this help"),
    ("q", "quit"),
];

pub fn render(f: &mut Frame, area: Rect) {
    let key_col = 12;
    let mut lines: Vec<Line> = Vec::with_capacity(ROWS.len());
    for (keys, desc) in ROWS {
        if keys.is_empty() && !desc.starts_with("  ") {
            // Section heading.
            lines.push(Line::from(Span::styled(
                *desc,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
        } else {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:>width$}  ", keys, width = key_col),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(*desc),
            ]));
        }
    }

    // Size to content (+2 for borders), centered.
    let width = 56;
    let height = lines.len() as u16 + 2;
    let popup = super::centered(width, height, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" editty — keys (any key to close) ")
        .border_style(Style::default().fg(Color::White));

    f.render_widget(Clear, popup);
    f.render_widget(
        Paragraph::new(lines).block(block).alignment(Alignment::Left),
        popup,
    );
}
