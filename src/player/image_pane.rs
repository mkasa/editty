//! Render video frames into a terminal rectangle using the kitty graphics
//! protocol, emitted directly as escape sequences.
//!
//! Why direct bytes instead of a widget crate: we only ever target kitty, and
//! the image-widget crates pull in sixel/quantization deps with a newer-rustc
//! MSRV than this toolchain. Direct emission also gives us exact control over
//! the fast frame-replacement path used during playback.
//!
//! Integration with ratatui: ratatui draws the whole UI but leaves the video
//! rect empty; after each `terminal.draw()` the app calls [`VideoBackend::present`],
//! which positions the cursor at the rect and (re)transmits the frame. kitty
//! images render above the cell background, so they survive ratatui's diffed
//! redraws as long as those cells aren't rewritten.

use std::io::{self, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ratatui::layout::Rect;

use crate::ffmpeg::frame::Frame;

/// Pixel dimensions of a single terminal cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CellSize {
    pub w: u16,
    pub h: u16,
}

/// Query the terminal's pixel-per-cell size; falls back to a common default
/// when the terminal doesn't report pixel geometry.
pub fn query_cell_size() -> CellSize {
    if let Ok(ws) = ratatui::crossterm::terminal::window_size() {
        if ws.width > 0 && ws.height > 0 && ws.columns > 0 && ws.rows > 0 {
            return CellSize {
                w: (ws.width / ws.columns).max(1),
                h: (ws.height / ws.rows).max(1),
            };
        }
    }
    CellSize { w: 8, h: 16 }
}

/// Backend that paints frames into a terminal rect. Abstracted so the rest of
/// the app never depends on the kitty protocol directly.
pub trait VideoBackend {
    /// Stash a freshly decoded frame to be drawn on the next `present`.
    fn set_frame(&mut self, frame: Frame);
    /// Force a redraw on the next `present` (e.g. after a full-screen repaint).
    fn invalidate(&mut self);
    /// Emit escape codes to draw the current frame inside `rect`, if needed.
    fn present(&mut self, out: &mut dyn Write, rect: Rect, cell: CellSize) -> io::Result<()>;
    /// Remove any drawn image (on resize teardown / exit).
    fn clear(&mut self, out: &mut dyn Write) -> io::Result<()>;
}

const IMAGE_ID: u32 = 1;
/// Max base64 payload bytes per graphics escape, per the protocol.
const CHUNK: usize = 4096;

pub struct KittyPane {
    frame: Option<Frame>,
    dirty: bool,
    last_rect: Option<Rect>,
    drawn: bool,
}

impl KittyPane {
    pub fn new() -> Self {
        Self { frame: None, dirty: false, last_rect: None, drawn: false }
    }
}

impl VideoBackend for KittyPane {
    fn set_frame(&mut self, frame: Frame) {
        self.frame = Some(frame);
        self.dirty = true;
    }

    fn invalidate(&mut self) {
        self.dirty = true;
        self.last_rect = None;
    }

    fn present(&mut self, out: &mut dyn Write, rect: Rect, cell: CellSize) -> io::Result<()> {
        let Some(frame) = &self.frame else { return Ok(()) };
        let rect_changed = self.last_rect != Some(rect);
        if !self.dirty && !rect_changed {
            return Ok(());
        }
        if rect.width == 0 || rect.height == 0 {
            return Ok(());
        }

        // Center the image within the rect (cells).
        let img_cols = frame.width.div_ceil(cell.w as u32) as u16;
        let img_rows = frame.height.div_ceil(cell.h as u32) as u16;
        let col = rect.x + rect.width.saturating_sub(img_cols) / 2;
        let row = rect.y + rect.height.saturating_sub(img_rows) / 2;

        // Drop the previous placement first so a smaller new frame leaves no
        // remnants; batched into the same flush so there's no visible gap.
        if self.drawn {
            write!(out, "\x1b_Ga=d,d=i,i={IMAGE_ID}\x1b\\")?;
        }
        // Cursor home is 1-based.
        write!(out, "\x1b[{};{}H", row + 1, col + 1)?;
        transmit(out, frame, true)?;
        out.flush()?;

        self.drawn = true;
        self.dirty = false;
        self.last_rect = Some(rect);
        Ok(())
    }

    fn clear(&mut self, out: &mut dyn Write) -> io::Result<()> {
        if self.drawn {
            write!(out, "\x1b_Ga=d,d=I,i={IMAGE_ID}\x1b\\")?;
            out.flush()?;
            self.drawn = false;
            self.last_rect = None;
        }
        Ok(())
    }
}

/// Emit a single frame at the current cursor position and let the cursor
/// advance past it. Used by the standalone `--show` spike to verify the
/// protocol works in a given terminal.
pub fn print_frame(out: &mut dyn Write, frame: &Frame) -> io::Result<()> {
    transmit(out, frame, false)?;
    writeln!(out)?;
    out.flush()
}

/// Transmit + display a frame as RGBA, chunked to the protocol's payload limit.
/// When `keep_cursor` is set, the cursor stays put (`C=1`) so a TUI redraw isn't
/// disturbed; otherwise the cursor advances past the image.
fn transmit(out: &mut dyn Write, frame: &Frame, keep_cursor: bool) -> io::Result<()> {
    let b64 = STANDARD.encode(&frame.rgba);
    let bytes = b64.as_bytes();
    let (w, h) = (frame.width, frame.height);
    let cursor = if keep_cursor { ",C=1" } else { "" };

    if bytes.len() <= CHUNK {
        write!(
            out,
            "\x1b_Ga=T,q=2,f=32,s={w},v={h},i={IMAGE_ID},p={IMAGE_ID}{cursor};"
        )?;
        out.write_all(bytes)?;
        return write!(out, "\x1b\\");
    }

    let mut offset = 0;
    let mut first = true;
    while offset < bytes.len() {
        let end = (offset + CHUNK).min(bytes.len());
        let more = u8::from(end < bytes.len());
        if first {
            write!(
                out,
                "\x1b_Ga=T,q=2,f=32,s={w},v={h},i={IMAGE_ID},p={IMAGE_ID}{cursor},m={more};"
            )?;
            first = false;
        } else {
            write!(out, "\x1b_Gm={more};")?;
        }
        out.write_all(&bytes[offset..end])?;
        write!(out, "\x1b\\")?;
        offset = end;
    }
    Ok(())
}
