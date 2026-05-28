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

/// How frame bytes reach the terminal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transport {
    /// Base64 the pixels into the escape stream (works everywhere, incl. SSH).
    Direct,
    /// Hand the terminal a POSIX shm object name (local only, much faster).
    SharedMemory,
}

/// Pick the best transport: shared memory when we're a local kitty, else direct.
/// Shared memory cannot work to a remote terminal, so any SSH marker, or the
/// `EDITTY_NO_SHM` escape hatch, forces the direct transport.
pub fn detect_transport() -> Transport {
    let ssh = std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_TTY").is_some()
        || std::env::var_os("SSH_CLIENT").is_some();
    let disabled = std::env::var_os("EDITTY_NO_SHM").is_some();
    if cfg!(unix) && !ssh && !disabled {
        Transport::SharedMemory
    } else {
        Transport::Direct
    }
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

/// Max base64 payload bytes per graphics escape, per the protocol.
const CHUNK: usize = 4096;
/// The two image ids we ping-pong between for flicker-free double buffering.
const ID_A: u32 = 1;
const ID_B: u32 = 2;

pub struct KittyPane {
    frame: Option<Frame>,
    dirty: bool,
    last_rect: Option<Rect>,
    /// Id of the placement currently on screen, if any.
    current_id: Option<u32>,
    transport: Transport,
}

impl KittyPane {
    pub fn new(transport: Transport) -> Self {
        Self {
            frame: None,
            dirty: false,
            last_rect: None,
            current_id: None,
            transport,
        }
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

        // Double buffering for zero flicker: draw the new frame on the *other*
        // image id first (same size/position, and newer placements render on
        // top, so it fully covers the old frame), THEN delete the old
        // placement. Everything goes out in a single flush, so kitty composites
        // and repaints once — the old frame is never cleared to empty, so there
        // is no flash between frames.
        let next_id = match self.current_id {
            Some(ID_A) => ID_B,
            _ => ID_A,
        };
        // Cursor home is 1-based.
        write!(out, "\x1b[{};{}H", row + 1, col + 1)?;
        transmit(out, frame, next_id, self.transport, true)?;
        if let Some(prev) = self.current_id {
            write!(out, "\x1b_Ga=d,d=i,i={prev}\x1b\\")?;
        }
        out.flush()?;

        self.current_id = Some(next_id);
        self.dirty = false;
        self.last_rect = Some(rect);
        Ok(())
    }

    fn clear(&mut self, out: &mut dyn Write) -> io::Result<()> {
        if self.current_id.take().is_some() {
            // Free both buffers regardless of which is showing.
            write!(out, "\x1b_Ga=d,d=I,i={ID_A}\x1b\\\x1b_Ga=d,d=I,i={ID_B}\x1b\\")?;
            out.flush()?;
            self.last_rect = None;
        }
        Ok(())
    }
}

/// Emit a single frame at the current cursor position and let the cursor
/// advance past it. Used by the standalone `--show` spike to verify the
/// protocol works in a given terminal.
pub fn print_frame(out: &mut dyn Write, frame: &Frame) -> io::Result<()> {
    transmit(out, frame, ID_A, Transport::Direct, false)?;
    writeln!(out)?;
    out.flush()
}

/// Transmit + display a frame under image/placement `id` using `transport`.
/// Shared memory falls back to the direct path on any shm failure. When
/// `keep_cursor` is set, the cursor stays put (`C=1`) so a TUI redraw isn't
/// disturbed; otherwise the cursor advances past the image.
fn transmit(
    out: &mut dyn Write,
    frame: &Frame,
    id: u32,
    transport: Transport,
    keep_cursor: bool,
) -> io::Result<()> {
    #[cfg(unix)]
    if transport == Transport::SharedMemory {
        if let Ok(name) = crate::player::shm::write(&frame.rgba) {
            return transmit_shm(out, frame, id, &name, keep_cursor);
        }
        // shm failed (e.g. ENOSPC) — fall through to the direct transport.
    }
    let _ = transport;
    transmit_direct(out, frame, id, keep_cursor)
}

/// Direct transport: base64 the RGBA into the escape stream, chunked to the
/// protocol's payload limit.
fn transmit_direct(out: &mut dyn Write, frame: &Frame, id: u32, keep_cursor: bool) -> io::Result<()> {
    let b64 = STANDARD.encode(&frame.rgba);
    let bytes = b64.as_bytes();
    let (w, h) = (frame.width, frame.height);
    let cursor = if keep_cursor { ",C=1" } else { "" };

    if bytes.len() <= CHUNK {
        write!(out, "\x1b_Ga=T,q=2,f=32,s={w},v={h},i={id},p={id}{cursor};")?;
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
                "\x1b_Ga=T,q=2,f=32,s={w},v={h},i={id},p={id}{cursor},m={more};"
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

/// Shared-memory transport: the payload is just the base64 shm object name; the
/// raw bytes (`S`) are read by the terminal from that object. One escape, no
/// chunking, no per-frame PTY bandwidth.
#[cfg(unix)]
fn transmit_shm(
    out: &mut dyn Write,
    frame: &Frame,
    id: u32,
    name: &str,
    keep_cursor: bool,
) -> io::Result<()> {
    let (w, h) = (frame.width, frame.height);
    let len = frame.rgba.len();
    let cursor = if keep_cursor { ",C=1" } else { "" };
    let b64name = STANDARD.encode(name.as_bytes());
    write!(
        out,
        "\x1b_Ga=T,q=2,f=32,t=s,s={w},v={h},S={len},i={id},p={id}{cursor};{b64name}\x1b\\"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Frame {
        Frame { width: 2, height: 2, rgba: vec![0u8; 16] }
    }

    const CELL: CellSize = CellSize { w: 8, h: 16 };

    fn rect() -> Rect {
        Rect::new(0, 0, 40, 20)
    }

    #[test]
    fn double_buffer_alternates_ids_and_draws_before_deleting() {
        let mut pane = KittyPane::new(Transport::Direct);

        pane.set_frame(tiny());
        let mut b1 = Vec::new();
        pane.present(&mut b1, rect(), CELL).unwrap();
        let s1 = String::from_utf8_lossy(&b1);
        assert!(s1.contains("i=1,p=1"), "first frame uses id 1");
        assert!(!s1.contains("a=d"), "first frame deletes nothing");

        pane.set_frame(tiny());
        let mut b2 = Vec::new();
        pane.present(&mut b2, rect(), CELL).unwrap();
        let s2 = String::from_utf8_lossy(&b2);
        assert!(s2.contains("i=2,p=2"), "second frame uses id 2");
        assert!(s2.contains("a=d,d=i,i=1"), "second frame deletes id 1");
        // The flicker fix: new frame is transmitted BEFORE the old is deleted.
        assert!(
            s2.find("a=T").unwrap() < s2.find("a=d").unwrap(),
            "draw must precede delete"
        );

        pane.set_frame(tiny());
        let mut b3 = Vec::new();
        pane.present(&mut b3, rect(), CELL).unwrap();
        let s3 = String::from_utf8_lossy(&b3);
        assert!(s3.contains("i=1,p=1"), "third frame ping-pongs back to id 1");
        assert!(s3.contains("a=d,d=i,i=2"), "third frame deletes id 2");
    }

    #[test]
    fn present_is_noop_when_not_dirty() {
        let mut pane = KittyPane::new(Transport::Direct);
        pane.set_frame(tiny());
        let mut b1 = Vec::new();
        pane.present(&mut b1, rect(), CELL).unwrap();
        assert!(!b1.is_empty());

        // No new frame, same rect: nothing should be emitted (no idle flicker).
        let mut b2 = Vec::new();
        pane.present(&mut b2, rect(), CELL).unwrap();
        assert!(b2.is_empty(), "idle present must emit nothing");
    }

    #[test]
    fn clear_frees_both_buffers() {
        let mut pane = KittyPane::new(Transport::Direct);
        pane.set_frame(tiny());
        pane.present(&mut Vec::new(), rect(), CELL).unwrap();
        let mut out = Vec::new();
        pane.clear(&mut out).unwrap();
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("i=1") && s.contains("i=2"), "clear frees both ids");
    }
}
