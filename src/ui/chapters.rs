//! Chapter list: named markers with the active chapter (under the playhead) and
//! the selected chapter highlighted, plus an inline editor when editing a title.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, EditTarget, Mode};
use crate::util::fmt_clock;

/// The fixed part of a row: marker, ordinal and the chapter's time.
fn prefix(i: usize, time: f64, selected: bool) -> String {
    let marker = if selected { "▶" } else { " " };
    format!("{marker}{:>2} {}  ", i + 1, fmt_clock(time))
}

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
    let inner = super::inner(area);
    let (height, width) = (inner.height as usize, inner.width as usize);
    let selected_chapter = app.selected_chapter.min(rows.len() - 1);

    let editing = app.mode == Mode::Editing && app.edit_target == EditTarget::Chapter;
    let edit = editing.then(|| {
        // No REVERSED here: the inverted cell is the cursor, and nothing else.
        let style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
        super::edit_rows(
            &prefix(selected_chapter, rows[selected_chapter].time, true),
            &app.edit_input,
            width,
            style,
        )
    });

    let lines = super::windowed_rows(
        rows.len(),
        selected_chapter,
        edit.as_ref().map(|(lines, cursor)| (lines.as_slice(), *cursor)),
        height,
        |i| {
            let ch = &rows[i];
            let selected = i == selected_chapter;
            let content = format!(
                "{}{}",
                prefix(i, ch.time, selected),
                ch.title.replace('\n', " ")
            );

            let mut style = Style::default();
            if active == Some(i) {
                style = style.fg(Color::Cyan);
            }
            if selected {
                style = style
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED);
            }
            Line::styled(content, style)
        },
    );

    f.render_widget(Paragraph::new(lines).block(block), area);
}
