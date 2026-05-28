//! The video pane. We only draw the bordered frame here; the actual image is
//! painted via the kitty protocol after ratatui's draw (see `App::present`).

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::App;

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

    if !app.kitty_ok {
        let msg = Paragraph::new(
            "kitty graphics not detected.\n\nRun editty in a bare kitty window \
             (not inside tmux/screen) to see video frames.\nScrubbing, cutting \
             and subtitle editing still work without preview.",
        )
        .block(block)
        .alignment(Alignment::Center)
        .style(Style::default().fg(Color::Yellow))
        .wrap(Wrap { trim: true });
        f.render_widget(msg, area);
        return;
    }

    // Image is drawn on top of this empty bordered area by the kitty backend.
    f.render_widget(block, area);
}
