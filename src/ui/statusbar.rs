//! Single-line status bar: position, markers, file, and any transient message.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Mode};
use crate::util::{fmt_clock, fmt_speed};

/// A prompt that takes over the whole bar: badge, label, the line being typed
/// with its cursor, then the keys that end it.
fn prompt(f: &mut Frame, app: &App, area: Rect, badge: &str, label: &str, keys: &str) {
    let mut spans = vec![
        Span::styled(
            badge.to_string(),
            Style::default()
                .bg(Color::Magenta)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(label.to_string()),
    ];
    let typed = app.edit_input.text();
    spans.extend(super::cursor_spans(
        typed,
        0..typed.len(),
        Some(app.edit_input.cursor()),
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(
        format!("   {keys}"),
        Style::default().fg(Color::DarkGray),
    ));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    // Naming an export or typing a search takes the bar over entirely.
    match app.mode {
        Mode::Naming => {
            let keys = "Enter cut · Esc cancel · Ctrl-U clear";
            return prompt(f, app, area, " NAME ", " save clip as: ", keys);
        }
        Mode::Searching => {
            let keys = "Enter find · Esc cancel · empty clears";
            return prompt(f, app, area, " FIND ", " find in cues: ", keys);
        }
        _ => {}
    }

    let mut parts: Vec<Span> = Vec::new();

    let (badge, badge_bg) = match app.mode {
        Mode::Editing => (" EDIT ", Color::Magenta),
        Mode::Naming | Mode::Searching => (" NAME ", Color::Magenta),
        Mode::Normal if app.is_playing() => (" ▶ PLAY ", Color::Green),
        Mode::Normal => (" ▮▮ PAUSE ", Color::Blue),
    };
    parts.push(Span::styled(
        badge,
        Style::default()
            .bg(badge_bg)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    ));

    parts.push(Span::styled(
        format!(" {} ", fmt_clock(app.playhead)),
        Style::default().add_modifier(Modifier::BOLD),
    ));

    // Highlight the speed only when it's off normal, to avoid clutter at 1x.
    let speed = app.speed();
    let speed_style = if (speed - 1.0).abs() > 1e-9 {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    parts.push(Span::styled(format!(" {} ", fmt_speed(speed)), speed_style));

    let mark = |label: &str, v: Option<f64>, color: Color| -> Span<'static> {
        match v {
            Some(t) => Span::styled(format!(" {label}{} ", fmt_clock(t)), Style::default().fg(color)),
            None => Span::styled(format!(" {label}-- "), Style::default().fg(Color::DarkGray)),
        }
    };
    parts.push(mark("in ", app.mark_in, Color::Green));
    parts.push(mark("out ", app.mark_out, Color::Red));

    if !app.status.is_empty() {
        parts.push(Span::styled(
            format!(" {} ", app.status),
            Style::default().fg(Color::Yellow),
        ));
    }

    let hint = match app.mode {
        Mode::Editing => " ←/→ move  Ctrl-←/→ word  Home/End  BS/Del  Enter commit  Esc cancel ",
        Mode::Naming | Mode::Searching => "",
        Mode::Normal => {
            " Space play  ←/→ seek  i/o mark  x/X cut  j/k cue  / find  s save  ? help  q quit "
        }
    };
    parts.push(Span::styled(hint, Style::default().fg(Color::DarkGray)));

    f.render_widget(Paragraph::new(Line::from(parts)), area);
}
