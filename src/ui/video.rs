//! The video pane. We only draw the bordered frame and the caption strip here;
//! the actual image is painted via the kitty protocol after ratatui's draw (see
//! `App::present`), into the rect above the strip.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::App;
use crate::util::{display_width, wrap};

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let audio = match &app.info.audio_codec {
        Some(a) => format!(" + {a}"),
        None => " (no audio)".to_string(),
    };
    let title = format!(
        " {}×{} {}{} ",
        app.info.width, app.info.height, app.info.video_codec, audio
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(block, area);

    let (picture, strip) = super::video_split(area, app.has_cues());

    if !app.kitty_ok {
        let msg = Paragraph::new(
            "kitty graphics not detected.\n\nRun editty in a bare kitty window \
             (not inside tmux/screen) to see video frames.\nScrubbing, cutting \
             and subtitle editing still work without preview.",
        )
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Yellow))
        .wrap(Wrap { trim: true });
        f.render_widget(msg, picture);
    }

    if strip.height > 0 {
        f.render_widget(
            Paragraph::new(caption(app, strip.width as usize, strip.height as usize))
                .alignment(Alignment::Center),
            strip,
        );
    }
}

/// The cue under the playhead, wrapped to the strip. Nothing between cues, the
/// same as a player. A cue too long even for the strip ends in an ellipsis, so a
/// clipped caption never passes for a whole one.
fn caption(app: &App, width: usize, height: usize) -> Vec<Line<'static>> {
    let Some(text) = app
        .vtt
        .as_ref()
        .and_then(|doc| Some((doc, doc.active_cue(app.playhead)?)))
        .and_then(|(doc, i)| doc.cue_text(i))
    else {
        return Vec::new();
    };

    // A cue's own line breaks are how the subtitle was written; keep them, and
    // wrap what is still too wide.
    let mut lines: Vec<String> = Vec::new();
    for part in text.lines() {
        lines.extend(wrap(part, width).into_iter().map(|r| part[r].to_string()));
    }

    let clipped = lines.len() > height;
    lines.truncate(height);
    if let Some(last) = lines.last_mut().filter(|_| clipped) {
        // The last row is full, so trim it back to make room — two cells, as the
        // ellipsis is East-Asian ambiguous and can render double width.
        while !last.is_empty() && display_width(last) + 2 > width {
            last.pop();
        }
        last.push('…');
    }

    let style = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    lines.into_iter().map(|l| Line::styled(l, style)).collect()
}
