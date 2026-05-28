//! A horizontal timeline: playhead, IN/OUT markers, and the selected span.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::util::fmt_timestamp;

/// Map a time to a column index within `width` cells.
fn col_for(t: f64, duration: f64, width: usize) -> usize {
    if duration <= 0.0 || width == 0 {
        return 0;
    }
    let frac = (t / duration).clamp(0.0, 1.0);
    ((frac * (width.saturating_sub(1)) as f64).round() as usize).min(width - 1)
}

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let title = format!(
        " {} / {} ",
        fmt_timestamp(app.playhead),
        fmt_timestamp(app.info.duration)
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = super::inner(area);
    let width = inner.width as usize;

    if width == 0 {
        f.render_widget(block, area);
        return;
    }

    let dur = app.info.duration;
    let in_col = app.mark_in.map(|t| col_for(t, dur, width));
    let out_col = app.mark_out.map(|t| col_for(t, dur, width));
    let head_col = col_for(app.playhead, dur, width);

    let mut spans: Vec<Span> = Vec::with_capacity(width);
    for c in 0..width {
        let in_span = matches!((in_col, out_col), (Some(i), Some(o)) if c >= i && c <= o)
            || matches!((in_col, out_col), (Some(i), None) if c >= i);
        let (ch, color) = if c == head_col {
            ('┃', Color::White)
        } else if Some(c) == in_col {
            ('I', Color::Green)
        } else if Some(c) == out_col {
            ('O', Color::Red)
        } else if in_span {
            ('━', Color::Cyan)
        } else {
            ('─', Color::DarkGray)
        };
        spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
    }

    f.render_widget(Paragraph::new(Line::from(spans)).block(block), area);
}
