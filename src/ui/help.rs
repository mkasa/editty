//! Centered help overlay listing all keybindings (toggled with `?`).
//!
//! Laid out in two columns so it fits an ordinary 80×24 terminal without
//! clipping. Each `(keys, desc)` row renders as a key/description pair; an empty
//! `keys` with text is a section heading, an empty `keys` with empty text is a
//! spacer.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

const LEFT: &[(&str, &str)] = &[
    ("", "Playback / seek"),
    ("Space", "play / pause"),
    ("-  =", "slower / faster"),
    ("←  →", "seek 1 second"),
    ("<  >", "seek 10 seconds"),
    (",  .", "step one frame"),
    ("0–9", "jump 0–90%"),
    ("Home/End", "jump start / end"),
    ("", ""),
    ("", "Cutting"),
    ("i  o", "set IN / OUT"),
    ("C", "clear markers"),
    ("x", "export fast (copy)"),
    ("X", "export precise"),
    ("", "  then name it, Enter"),
];

const RIGHT: &[(&str, &str)] = &[
    ("", "Subtitles"),
    ("j  k", "prev / next cue"),
    ("Enter", "edit cue text"),
    ("[  ]", "snap cue start / end"),
    ("n  d", "new / delete cue"),
    ("s", "save .vtt"),
    ("G", "generate (WhisperX)"),
    ("", ""),
    ("", "Chapters"),
    ("m  e", "new / edit chapter"),
    ("{  }", "prev / next chapter"),
    ("M", "delete chapter"),
    ("S", "save .chapter.txt"),
    ("", ""),
    ("", "General"),
    ("?", "toggle this help"),
    ("q", "quit"),
];

const KEY_COL: usize = 8;

fn rows_to_lines(rows: &[(&str, &str)]) -> Vec<Line<'static>> {
    rows.iter()
        .map(|(keys, desc)| {
            if keys.is_empty() {
                if desc.is_empty() {
                    Line::default() // spacer
                } else if let Some(cont) = desc.strip_prefix("  ") {
                    // continuation line, dimmed
                    Line::from(Span::styled(
                        format!("  {cont}"),
                        Style::default().fg(Color::DarkGray),
                    ))
                } else {
                    // section heading
                    Line::from(Span::styled(
                        desc.to_string(),
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ))
                }
            } else {
                Line::from(vec![
                    Span::styled(
                        format!("{keys:>KEY_COL$}  "),
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(desc.to_string()),
                ])
            }
        })
        .collect()
}

pub fn render(f: &mut Frame, area: Rect) {
    let left = rows_to_lines(LEFT);
    let right = rows_to_lines(RIGHT);

    // Tallest column drives the height; +2 for the border. Width is two columns
    // plus the border, clamped to the terminal so it can never be clipped.
    let body_h = left.len().max(right.len()) as u16;
    let height = (body_h + 2).min(area.height);
    let width = 84.min(area.width);
    let popup = super::centered(width, height, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" editty — keys (any key to close) ")
        .border_style(Style::default().fg(Color::White));
    let inner = super::inner(popup);

    f.render_widget(Clear, popup);
    f.render_widget(block, popup);

    let cols = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(inner);
    f.render_widget(
        Paragraph::new(left).alignment(Alignment::Left),
        cols[0],
    );
    f.render_widget(
        Paragraph::new(right).alignment(Alignment::Left),
        cols[1],
    );
}
