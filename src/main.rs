mod app;
mod chapter;
mod cli;
mod ffmpeg;
mod keymap;
mod player;
mod ui;
mod util;
mod vtt;

use std::io;

use anyhow::{Context, Result};
use clap::Parser;

use app::App;
use cli::Cli;

fn main() -> Result<()> {
    let args = Cli::parse();

    if !args.video.exists() {
        anyhow::bail!("video not found: {}", args.video.display());
    }

    let info = ffmpeg::probe(&args.video).context("probing video")?;

    if let Some(secs) = args.show {
        return show_frame(&info, secs);
    }

    let vtt = args.resolve_vtt();
    let mut app = App::new(info, vtt);
    let mut terminal = ratatui::try_init().context(
        "could not initialize the terminal (editty needs an interactive terminal; \
         run it in a bare kitty window)",
    )?;
    let result = app.run(&mut terminal);
    ratatui::restore();
    result
}

/// Spike/diagnostic: extract one frame and paint it via the kitty protocol.
fn show_frame(info: &ffmpeg::MediaInfo, secs: f64) -> Result<()> {
    use ffmpeg::frame::{self, fit_dims};
    use player::{image_pane, query_cell_size};

    let cell = query_cell_size();
    // Target ~half the typical terminal: 80x24 cells minus a margin.
    let (max_w, max_h) = (70 * cell.w as u32, 20 * cell.h as u32);
    let (w, h) = fit_dims(info.width, info.height, max_w, max_h);
    let frame = frame::extract_rgba(&info.path, secs, w, h, false)
        .context("extracting frame for --show")?;
    image_pane::print_frame(&mut io::stdout(), &frame).context("emitting frame")?;
    Ok(())
}
