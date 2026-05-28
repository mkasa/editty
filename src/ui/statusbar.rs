//! Single-line status bar: position, markers, file, and any transient message.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Mode};
use crate::util::fmt_clock;

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    // Naming an export takes over the bar with the editable filename prompt.
    if app.mode == Mode::Naming {
        let line = Line::from(vec![
            Span::styled(
                " NAME ",
                Style::default()
                    .bg(Color::Magenta)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" save clip as: "),
            Span::styled(
                format!("{}\u{258f}", app.edit_buffer),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "   Enter cut · Esc cancel · Ctrl-U clear",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }

    let mut parts: Vec<Span> = Vec::new();

    let (badge, badge_bg) = match app.mode {
        Mode::Editing => (" EDIT ", Color::Magenta),
        Mode::Naming => (" NAME ", Color::Magenta),
        Mode::Normal => (" PLAY ", Color::Blue),
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
        Mode::Editing => " type text   Enter commit   Esc cancel ",
        Mode::Naming => "",
        Mode::Normal => {
            " ←/→ seek  ,/. frame  i/o mark  x/X cut  j/k cue  ⏎ edit  s save  ? help  q quit "
        }
    };
    parts.push(Span::styled(hint, Style::default().fg(Color::DarkGray)));

    f.render_widget(Paragraph::new(Line::from(parts)), area);
}
