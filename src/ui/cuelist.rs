//! Subtitle cue list: shows cues with the active cue (under the playhead) and
//! the selected cue highlighted, and an inline editor when editing.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, EditTarget, Mode};
use crate::util::fmt_timestamp;

/// The fixed part of a row: marker, ordinal and the cue's timings.
fn prefix(i: usize, start: f64, end: f64, selected: bool) -> String {
    let marker = if selected { "▶" } else { " " };
    format!(
        "{marker}{:>2} {}→{}  ",
        i + 1,
        fmt_timestamp(start),
        fmt_timestamp(end)
    )
}

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let dirty = if app.vtt_dirty { " *" } else { "" };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" subtitles{dirty} "))
        .border_style(Style::default().fg(Color::DarkGray));

    let Some(doc) = &app.vtt else {
        f.render_widget(
            Paragraph::new("(no subtitles — press G to generate with WhisperX, or --vtt <file>)")
                .block(block)
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    };

    let rows = doc.cue_rows();
    if rows.is_empty() {
        f.render_widget(
            Paragraph::new("(no cues — n to add one, or G to generate with WhisperX)")
                .block(block)
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    }

    let active = doc.active_cue(app.playhead);
    let inner = super::inner(area);
    let (height, width) = (inner.height as usize, inner.width as usize);
    let selected_cue = app.selected_cue.min(rows.len() - 1);

    // While editing, the cue grows to as many rows as its text needs and the
    // whole list scrolls by line, so the end of a long sentence stays reachable.
    let editing = app.mode == Mode::Editing && app.edit_target == EditTarget::Cue;
    let edit = editing.then(|| {
        let (s, e, _) = &rows[selected_cue];
        // No REVERSED here: the inverted cell is the cursor, and nothing else.
        let style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
        super::edit_rows(
            &prefix(selected_cue, *s, *e, true),
            &app.edit_input,
            width,
            style,
        )
    });

    let lines = super::windowed_rows(
        rows.len(),
        selected_cue,
        edit.as_ref().map(|(lines, cursor)| (lines.as_slice(), *cursor)),
        height,
        |i| {
            let (s, e, text) = &rows[i];
            let selected = i == selected_cue;

            let mut style = Style::default();
            if active == Some(i) {
                style = style.fg(Color::Cyan);
            }
            if selected {
                style = style
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED);
            }

            let mut spans = vec![Span::styled(prefix(i, *s, *e, selected), style)];
            spans.extend(super::highlight_spans(
                &text.replace('\n', " "),
                &app.search,
                style,
            ));
            Line::from(spans)
        },
    );

    f.render_widget(Paragraph::new(lines).block(block), area);
}
