//! Application state and the main event loop.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::Rect;

use crate::ffmpeg::MediaInfo;
use crate::ffmpeg::cut::{self, CutMode};
use crate::ffmpeg::frame::{self, fit_dims};
use crate::keymap::{self, Action};
use crate::player::{CellSize, KittyPane, VideoBackend, query_cell_size};
use crate::ui;
use crate::vtt::VttDoc;

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Mode {
    Normal,
    /// Editing the selected cue's text; keys feed the edit buffer.
    Editing,
}

/// Length (seconds) of a freshly created cue.
const NEW_CUE_LEN: f64 = 2.0;

pub struct App {
    pub info: MediaInfo,
    pub vtt_path: Option<PathBuf>,
    pub vtt: Option<VttDoc>,
    pub vtt_dirty: bool,
    pub selected_cue: usize,
    pub mode: Mode,
    pub edit_buffer: String,
    pub playhead: f64,
    pub mark_in: Option<f64>,
    pub mark_out: Option<f64>,
    pub kitty_ok: bool,
    pub cell: CellSize,
    pub status: String,

    pane: Box<dyn VideoBackend>,
    area: Rect,
    needs_frame: bool,
    should_quit: bool,
    /// Set after a quit attempt with unsaved subtitle edits; a second quit confirms.
    quit_armed: bool,
    /// An export awaiting overwrite confirmation.
    pending_cut: Option<PendingCut>,
}

struct PendingCut {
    output: PathBuf,
    mode: CutMode,
    start: f64,
    end: f64,
}

impl App {
    pub fn new(info: MediaInfo, vtt_path: Option<PathBuf>) -> Self {
        let kitty_ok = detect_kitty();
        let cell = query_cell_size();

        let mut status = String::new();
        let vtt = match &vtt_path {
            Some(p) if p.exists() => match VttDoc::load(p) {
                Ok(doc) => Some(doc),
                Err(e) => {
                    status = format!("vtt load failed: {e}");
                    None
                }
            },
            // A --vtt path that doesn't exist yet: start an empty document.
            Some(_) => Some(VttDoc::empty()),
            None => None,
        };

        App {
            info,
            vtt_path,
            vtt,
            vtt_dirty: false,
            selected_cue: 0,
            mode: Mode::Normal,
            edit_buffer: String::new(),
            playhead: 0.0,
            mark_in: None,
            mark_out: None,
            kitty_ok,
            cell,
            status,
            pane: Box::new(KittyPane::new()),
            area: Rect::default(),
            needs_frame: true,
            should_quit: false,
            quit_armed: false,
            pending_cut: None,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut out = io::stdout();
        loop {
            let size = terminal.size().context("querying terminal size")?;
            self.area = Rect::new(0, 0, size.width, size.height);

            // Decode the pending frame only when the input queue is idle, which
            // naturally debounces bursts of held-key seeks.
            if self.needs_frame
                && self.kitty_ok
                && !event::poll(Duration::ZERO).unwrap_or(false)
            {
                self.load_frame();
            }

            terminal.draw(|f| ui::render(f, self))?;

            if self.kitty_ok {
                let vrect = ui::inner(ui::layout(self.area).video);
                self.pane.present(&mut out, vrect, self.cell).ok();
            }

            if event::poll(Duration::from_millis(100)).unwrap_or(false) {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => {
                        if self.mode == Mode::Editing {
                            self.handle_edit_key(k.code);
                        } else if self.pending_cut.is_some() {
                            self.handle_confirm(k.code);
                        } else {
                            self.apply(keymap::map(k));
                        }
                    }
                    Event::Resize(_, _) => {
                        self.pane.invalidate();
                        self.needs_frame = true;
                    }
                    _ => {}
                }
            }

            if self.should_quit {
                break;
            }
        }
        self.pane.clear(&mut out).ok();
        Ok(())
    }

    fn apply(&mut self, action: Action) {
        // Any action other than a repeated quit disarms the unsaved-quit guard.
        if action != Action::Quit {
            self.quit_armed = false;
        }
        match action {
            Action::Quit => {
                if self.vtt_dirty && !self.quit_armed {
                    self.quit_armed = true;
                    self.status = "unsaved subtitle edits — q again to quit, s to save".into();
                } else {
                    self.should_quit = true;
                }
            }
            Action::SeekBy(d) => self.seek_to(self.playhead + d),
            Action::FrameStep(n) => {
                let dt = n as f64 / self.info.fps.max(1.0);
                self.seek_to(self.playhead + dt);
            }
            Action::SeekStart => self.seek_to(0.0),
            Action::SeekEnd => self.seek_to(self.info.duration),
            Action::SeekFraction(frac) => self.seek_to(self.info.duration * frac),
            Action::SetIn => {
                self.mark_in = Some(self.playhead);
                if let Some(o) = self.mark_out {
                    if o < self.playhead {
                        self.mark_out = None;
                    }
                }
                self.status = "set IN".into();
            }
            Action::SetOut => {
                if self.mark_in.map(|i| self.playhead <= i).unwrap_or(false) {
                    self.status = "OUT must be after IN".into();
                } else {
                    self.mark_out = Some(self.playhead);
                    self.status = "set OUT".into();
                }
            }
            Action::ClearMarks => {
                self.mark_in = None;
                self.mark_out = None;
                self.status = "cleared marks".into();
            }
            Action::ExportFast => self.export(CutMode::Fast),
            Action::ExportPrecise => self.export(CutMode::Precise),
            Action::CueNext => self.select_cue(1),
            Action::CuePrev => self.select_cue(-1),
            Action::EditCue => self.begin_edit(),
            Action::SnapStart => self.snap_cue(true),
            Action::SnapEnd => self.snap_cue(false),
            Action::NewCue => self.new_cue(),
            Action::DeleteCue => self.delete_cue(),
            Action::SaveVtt => self.save_vtt(),
            Action::Nothing => {}
        }
    }

    fn select_cue(&mut self, delta: i32) {
        let Some(doc) = &self.vtt else { return };
        let count = doc.cue_count();
        if count == 0 {
            return;
        }
        let next = (self.selected_cue as i32 + delta).clamp(0, count as i32 - 1) as usize;
        self.selected_cue = next;
        // Seek to the selected cue's start so the frame follows the selection.
        if let Some((start, _)) = doc.cue_times(next) {
            self.seek_to(start);
        }
    }

    fn begin_edit(&mut self) {
        let Some(doc) = &self.vtt else {
            self.status = "no subtitles loaded".into();
            return;
        };
        let Some(text) = doc.cue_text(self.selected_cue) else {
            return;
        };
        // Single-line editor for v1: collapse multi-line payloads with a space.
        self.edit_buffer = text.replace('\n', " ");
        self.mode = Mode::Editing;
        self.status = "editing — Enter to commit, Esc to cancel".into();
    }

    fn handle_edit_key(&mut self, code: ratatui::crossterm::event::KeyCode) {
        use ratatui::crossterm::event::KeyCode;
        match code {
            KeyCode::Char(c) => self.edit_buffer.push(c),
            KeyCode::Backspace => {
                self.edit_buffer.pop();
            }
            KeyCode::Enter => {
                let text = std::mem::take(&mut self.edit_buffer);
                if let Some(doc) = &mut self.vtt {
                    doc.set_cue_text(self.selected_cue, &text);
                    self.vtt_dirty = true;
                }
                self.mode = Mode::Normal;
                self.status = "cue updated".into();
            }
            KeyCode::Esc => {
                self.edit_buffer.clear();
                self.mode = Mode::Normal;
                self.status = "edit cancelled".into();
            }
            _ => {}
        }
    }

    fn snap_cue(&mut self, start: bool) {
        let playhead = self.playhead;
        let sel = self.selected_cue;
        if let Some(doc) = &mut self.vtt {
            if start {
                doc.set_cue_start(sel, playhead);
            } else {
                doc.set_cue_end(sel, playhead);
            }
            self.vtt_dirty = true;
            self.status = format!("snapped cue {} {}", sel + 1, if start { "start" } else { "end" });
        }
    }

    fn new_cue(&mut self) {
        let start = self.playhead;
        let end = (start + NEW_CUE_LEN).min(self.info.duration);
        if let Some(doc) = &mut self.vtt {
            let idx = doc.add_cue(start, end, "");
            self.selected_cue = idx;
            self.vtt_dirty = true;
            self.begin_edit();
        } else {
            self.status = "no subtitle file (pass --vtt to create one)".into();
        }
    }

    fn delete_cue(&mut self) {
        let sel = self.selected_cue;
        if let Some(doc) = &mut self.vtt {
            if doc.cue_count() == 0 {
                return;
            }
            doc.delete_cue(sel);
            self.selected_cue = sel.min(doc.cue_count().saturating_sub(1));
            self.vtt_dirty = true;
            self.status = "cue deleted".into();
        }
    }

    fn save_vtt(&mut self) {
        let Some(path) = self.vtt_path.clone() else {
            self.status = "no subtitle path to save to".into();
            return;
        };
        match &self.vtt {
            Some(doc) => match doc.save(&path) {
                Ok(()) => {
                    self.vtt_dirty = false;
                    self.status = format!("saved {}", path.display());
                }
                Err(e) => self.status = format!("save failed: {e}"),
            },
            None => self.status = "nothing to save".into(),
        }
    }

    /// Begin an export of the marked span, prompting if the target exists.
    fn export(&mut self, mode: CutMode) {
        let (Some(start), Some(end)) = (self.mark_in, self.mark_out) else {
            self.status = "set IN (i) and OUT (o) first".into();
            return;
        };
        let output = cut::default_output(&self.info.path, start, end);
        if output.exists() {
            self.status = format!("{} exists — y to overwrite, n to cancel", output.display());
            self.pending_cut = Some(PendingCut { output, mode, start, end });
        } else {
            self.run_cut(&output, mode, start, end, false);
        }
    }

    fn handle_confirm(&mut self, code: ratatui::crossterm::event::KeyCode) {
        use ratatui::crossterm::event::KeyCode;
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let pc = self.pending_cut.take().expect("pending cut present");
                self.run_cut(&pc.output, pc.mode, pc.start, pc.end, true);
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.pending_cut = None;
                self.status = "export cancelled".into();
            }
            _ => {}
        }
    }

    fn run_cut(&mut self, output: &std::path::Path, mode: CutMode, start: f64, end: f64, overwrite: bool) {
        let kind = match mode {
            CutMode::Fast => "fast",
            CutMode::Precise => "precise",
        };
        self.status = format!("exporting ({kind})…");
        match cut::cut(&self.info.path, output, start, end, mode, overwrite) {
            Ok(()) => self.status = format!("saved {}", output.display()),
            Err(e) => self.status = format!("export failed: {e}"),
        }
    }

    fn seek_to(&mut self, t: f64) {
        let clamped = t.clamp(0.0, self.info.duration);
        if (clamped - self.playhead).abs() > f64::EPSILON {
            self.playhead = clamped;
            self.needs_frame = true;
            self.status.clear();
        }
    }

    fn load_frame(&mut self) {
        let vrect = ui::inner(ui::layout(self.area).video);
        let max_w = vrect.width as u32 * self.cell.w as u32;
        let max_h = vrect.height as u32 * self.cell.h as u32;
        if max_w < 2 || max_h < 2 {
            return;
        }
        let (w, h) = fit_dims(self.info.width, self.info.height, max_w, max_h);
        match frame::extract_rgba(&self.info.path, self.playhead, w, h, false) {
            Ok(frame) => {
                self.pane.set_frame(frame);
                self.needs_frame = false;
            }
            Err(e) => {
                self.status = format!("frame error: {e}");
                self.needs_frame = false;
            }
        }
    }
}

/// Heuristic: are we in a terminal that speaks the kitty graphics protocol?
fn detect_kitty() -> bool {
    if std::env::var_os("KITTY_WINDOW_ID").is_some() {
        return true;
    }
    matches!(std::env::var("TERM"), Ok(t) if t.contains("kitty"))
}
