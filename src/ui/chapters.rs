//! Chapter list: named markers with the active chapter (under the playhead) and
//! the selected chapter highlighted, plus an inline editor when editing a title.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, EditTarget, Mode};
use crate::util::fmt_clock;

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let dirty = if app.chapters_dirty { " *" } else { "" };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" chapters{dirty} "))
        .border_style(Style::default().fg(Color::DarkGray));

    let rows = app.chapters.rows();
    if rows.is_empty() {
        f.render_widget(
            Paragraph::new("(no chapters — press m to add one at the playhead)")
                .block(block)
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    }

    let active = app.chapters.active(app.playhead);
    let editing_here = app.mode == Mode::Editing && app.edit_target == EditTarget::Chapter;
    let height = super::inner(area).height as usize;

    // Window the list so the selected chapter stays visible.
    let start = app
        .selected_chapter
        .saturating_sub(height.saturating_sub(1) / 2)
        .min(rows.len().saturating_sub(height).max(0));

    let mut lines: Vec<Line> = Vec::new();
    for (i, ch) in rows.iter().enumerate().skip(start).take(height) {
        let selected = i == app.selected_chapter;
        let is_active = active == Some(i);

        let body = if selected && editing_here {
            format!("{}\u{258f}", app.edit_buffer)
        } else {
            ch.title.replace('\n', " ")
        };

        let marker = if selected { "▶" } else { " " };
        let content = format!("{marker}{:>2} {}  {}", i + 1, fmt_clock(ch.time), body);

        let mut style = Style::default();
        if is_active {
            style = style.fg(Color::Cyan);
        }
        if selected {
            style = style
                .fg(if editing_here { Color::Yellow } else { Color::White })
                .add_modifier(Modifier::BOLD | Modifier::REVERSED);
        }
        lines.push(Line::styled(content, style));
    }

    f.render_widget(Paragraph::new(lines).block(block), area);
}
