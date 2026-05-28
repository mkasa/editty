//! Subtitle cue list: shows cues with the active cue (under the playhead) and
//! the selected cue highlighted, and an inline editor when editing.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::{App, Mode};
use crate::util::fmt_timestamp;

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let dirty = if app.vtt_dirty { " *" } else { "" };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" subtitles{dirty} "))
        .border_style(Style::default().fg(Color::DarkGray));

    let Some(doc) = &app.vtt else {
        f.render_widget(
            Paragraph::new("(no subtitles — pass --vtt <file> to create or edit)")
                .block(block)
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    };

    let rows = doc.cue_rows();
    if rows.is_empty() {
        f.render_widget(
            Paragraph::new("(no cues — press n to add one at the playhead)")
                .block(block)
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
        return;
    }

    let active = doc.active_cue(app.playhead);
    let height = super::inner(area).height as usize;

    // Window the list so the selected cue stays visible.
    let start = app
        .selected_cue
        .saturating_sub(height.saturating_sub(1) / 2)
        .min(rows.len().saturating_sub(height).max(0));

    let mut lines: Vec<Line> = Vec::new();
    for (i, (s, e, text)) in rows.iter().enumerate().skip(start).take(height) {
        let selected = i == app.selected_cue;
        let is_active = active == Some(i);

        let body = if selected && app.mode == Mode::Editing {
            format!("{}▏", app.edit_buffer)
        } else {
            text.replace('\n', " ")
        };

        let marker = if selected { "▶" } else { " " };
        let content = format!(
            "{marker}{:>2} {}→{}  {}",
            i + 1,
            fmt_timestamp(*s),
            fmt_timestamp(*e),
            body
        );

        let mut style = Style::default();
        if is_active {
            style = style.fg(Color::Cyan);
        }
        if selected {
            style = style
                .fg(if app.mode == Mode::Editing { Color::Yellow } else { Color::White })
                .add_modifier(Modifier::BOLD | Modifier::REVERSED);
        }
        lines.push(Line::styled(content, style));
    }

    f.render_widget(Paragraph::new(lines).block(block), area);
}
