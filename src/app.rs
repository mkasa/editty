//! Application state and the main event loop.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::Rect;

use crate::chapter::Chapters;
use crate::ffmpeg::MediaInfo;
use crate::ffmpeg::cut::{self, CutMode};
use crate::ffmpeg::frame::{self, fit_dims};
use crate::ffmpeg::playback::Playback;
use crate::keymap::{self, Action};
use crate::player::{CellSize, KittyPane, Transport, VideoBackend, query_cell_size};
use crate::search;
use crate::textinput::TextInput;
use crate::ui;
use crate::vtt::VttDoc;
use crate::whisperx::{Progress, WhisperXConfig, WhisperXJob};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Mode {
    Normal,
    /// Editing a cue's or chapter's text; keys feed the edit buffer.
    Editing,
    /// Typing the output filename for an export; keys feed the edit buffer.
    Naming,
    /// Typing a string to find in the cue text; keys feed the edit buffer.
    Searching,
}

/// What the inline editor ([`Mode::Editing`]) is currently editing.
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum EditTarget {
    Cue,
    Chapter,
}

/// Length (seconds) of a freshly created cue.
const NEW_CUE_LEN: f64 = 2.0;

/// Selectable playback speeds; `DEFAULT_SPEED_IDX` is normal speed.
const SPEEDS: [f64; 5] = [0.25, 0.5, 1.0, 1.5, 2.0];
const DEFAULT_SPEED_IDX: usize = 2;

pub struct App {
    pub info: MediaInfo,
    pub vtt_path: Option<PathBuf>,
    pub vtt: Option<VttDoc>,
    pub vtt_dirty: bool,
    pub selected_cue: usize,
    pub chapters_path: PathBuf,
    pub chapters: Chapters,
    pub chapters_dirty: bool,
    pub selected_chapter: usize,
    pub mode: Mode,
    /// Which list the inline editor is editing (valid while `mode == Editing`).
    pub edit_target: EditTarget,
    /// The line being edited, with its cursor (also used by the naming and
    /// search prompts).
    pub edit_input: TextInput,
    /// The string being searched for, highlighted wherever it occurs in the cue
    /// list. Empty when no search is set.
    pub search: String,
    pub show_help: bool,
    pub playhead: f64,
    pub mark_in: Option<f64>,
    pub mark_out: Option<f64>,
    pub kitty_ok: bool,
    pub cell: CellSize,
    pub status: String,

    pane: Box<dyn VideoBackend>,
    area: Rect,
    needs_frame: bool,
    /// When the pending scrub decode should be frame-accurate (set by stepping).
    precise_seek: bool,
    should_quit: bool,
    /// Active playback session, if currently playing.
    playback: Option<Playback>,
    /// Index into [`SPEEDS`] for the current playback rate.
    speed_idx: usize,
    /// Set after a quit attempt with unsaved subtitle edits; a second quit confirms.
    quit_armed: bool,
    /// An export awaiting a filename (while in [`Mode::Naming`]).
    pending_export: Option<PendingExport>,
    /// An export awaiting overwrite confirmation.
    pending_cut: Option<PendingCut>,
    /// WhisperX subtitle generation config (from the CLI).
    wx_cfg: WhisperXConfig,
    /// A running WhisperX subtitle-generation job, if any.
    whisperx: Option<WhisperXJob>,
}

/// A marked span waiting for the user to type its output filename.
struct PendingExport {
    mode: CutMode,
    start: f64,
    end: f64,
}

struct PendingCut {
    output: PathBuf,
    mode: CutMode,
    start: f64,
    end: f64,
}

impl App {
    pub fn new(info: MediaInfo, vtt_path: Option<PathBuf>, wx_cfg: WhisperXConfig) -> Self {
        // Graphics support is probed at runtime once the terminal is in raw mode
        // (see `detect_graphics`); assume none until then.
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

        // Chapters live beside the video as `<stem>.chapter.txt`, auto-loaded.
        let chapters_path = info.path.with_extension("chapter.txt");
        let chapters = if chapters_path.exists() {
            match Chapters::load(&chapters_path) {
                Ok(c) => c,
                Err(e) => {
                    status = format!("chapter load failed: {e}");
                    Chapters::empty()
                }
            }
        } else {
            Chapters::empty()
        };

        App {
            info,
            vtt_path,
            vtt,
            vtt_dirty: false,
            selected_cue: 0,
            chapters_path,
            chapters,
            chapters_dirty: false,
            selected_chapter: 0,
            mode: Mode::Normal,
            edit_target: EditTarget::Cue,
            edit_input: TextInput::default(),
            search: String::new(),
            show_help: false,
            playhead: 0.0,
            mark_in: None,
            mark_out: None,
            kitty_ok: false,
            cell,
            status,
            pane: Box::new(KittyPane::new(Transport::Direct)),
            area: Rect::default(),
            needs_frame: true,
            precise_seek: false,
            should_quit: false,
            playback: None,
            speed_idx: DEFAULT_SPEED_IDX,
            quit_armed: false,
            pending_export: None,
            pending_cut: None,
            wx_cfg,
            whisperx: None,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut out = io::stdout();
        self.detect_graphics();
        loop {
            let size = terminal.size().context("querying terminal size")?;
            self.area = Rect::new(0, 0, size.width, size.height);

            // Advance playback: take the newest due frame and follow its clock.
            if self.playback.is_some() {
                let (frame, clock, finished) = {
                    let pb = self.playback.as_mut().unwrap();
                    (pb.poll(), pb.clock(), pb.is_finished())
                };
                if let Some(f) = frame {
                    self.pane.set_frame(f);
                }
                self.playhead = clock.min(self.info.duration);
                self.follow_playhead();
                if finished {
                    self.playback = None;
                    self.needs_frame = true;
                    self.status = "playback finished".into();
                }
            }

            // Drain any progress from a running subtitle-generation job.
            if self.whisperx.is_some() {
                self.poll_whisperx();
            }

            // Scrub-decode the pending frame only when idle and not playing,
            // which naturally debounces bursts of held-key seeks.
            if self.playback.is_none()
                && self.needs_frame
                && self.kitty_ok
                && !event::poll(Duration::ZERO).unwrap_or(false)
            {
                self.load_frame();
            }

            terminal.draw(|f| ui::render(f, self))?;

            // The kitty image draws above text, so while the help overlay is up
            // we hide it; otherwise paint the current frame.
            if self.show_help {
                self.pane.clear(&mut out).ok();
            } else if self.kitty_ok {
                let vrect = self.picture_rect();
                self.pane.present(&mut out, vrect, self.cell).ok();
            }

            // Tick fast while playing so frames present on time; idle otherwise.
            let tick = if self.playback.is_some() {
                Duration::from_millis(8)
            } else {
                Duration::from_millis(100)
            };
            if event::poll(tick).unwrap_or(false) {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => match self.mode {
                        Mode::Editing => self.handle_edit_key(k),
                        Mode::Naming => self.handle_naming_key(k),
                        Mode::Searching => self.handle_search_key(k),
                        Mode::Normal => {
                            if self.show_help {
                                // Any key dismisses help; restore the frame.
                                self.show_help = false;
                                self.pane.invalidate();
                            } else if self.pending_cut.is_some() {
                                self.handle_confirm(k.code);
                            } else {
                                self.apply(keymap::map(k));
                            }
                        }
                    },
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
        // Stop audio/video processes promptly, then remove the image.
        self.playback = None;
        // Cancel any in-flight transcription (kills the child process).
        self.whisperx = None;
        self.pane.clear(&mut out).ok();
        Ok(())
    }

    /// Ask the terminal (now in raw mode) whether it supports the kitty graphics
    /// protocol and the shared-memory transport, and configure the pane to match.
    fn detect_graphics(&mut self) {
        #[cfg(unix)]
        {
            let allow_shm = std::env::var_os("EDITTY_NO_SHM").is_none();
            let support = crate::player::probe::probe(allow_shm);
            self.kitty_ok = support.graphics;
            let transport = if support.shm {
                Transport::SharedMemory
            } else {
                Transport::Direct
            };
            self.pane = Box::new(KittyPane::new(transport));
        }
    }

    fn apply(&mut self, action: Action) {
        // Any action other than a repeated quit disarms the unsaved-quit guard.
        if action != Action::Quit {
            self.quit_armed = false;
        }
        match action {
            Action::Quit => {
                if (self.vtt_dirty || self.chapters_dirty) && !self.quit_armed {
                    self.quit_armed = true;
                    self.status = "unsaved edits — q again to quit, s/S to save".into();
                } else {
                    self.should_quit = true;
                }
            }
            Action::TogglePlay => self.toggle_play(),
            Action::SpeedDown => self.change_speed(-1),
            Action::SpeedUp => self.change_speed(1),
            Action::SeekBy(d) => self.seek_to(self.playhead + d),
            Action::FrameStep(n) => {
                let dt = n as f64 / self.info.fps.max(1.0);
                self.seek_to(self.playhead + dt);
                // Stepping wants the exact adjacent frame, not the nearest keyframe.
                self.precise_seek = true;
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
            Action::SearchStart => self.begin_search(),
            Action::SearchNext => self.find(true, false),
            Action::SearchPrev => self.find(false, false),
            Action::SnapStart => self.snap_cue(true),
            Action::SnapEnd => self.snap_cue(false),
            Action::NewCue => self.new_cue(),
            Action::DeleteCue => self.delete_cue(),
            Action::SaveVtt => self.save_vtt(),
            Action::NewChapter => self.new_chapter(),
            Action::EditChapter => self.begin_edit_chapter(),
            Action::DeleteChapter => self.delete_chapter(),
            Action::ChapterNext => self.select_chapter(1),
            Action::ChapterPrev => self.select_chapter(-1),
            Action::SaveChapters => self.save_chapters(),
            Action::GenerateSubs => self.generate_subs(),
            Action::Help => self.show_help = true,
            Action::Nothing => {}
        }
    }

    /// Point both lists at whatever covers the playhead. Each list windows on its
    /// selection, so this is what makes them scroll along with playback and with
    /// a seek — and it leaves you on the cue you just heard, ready to edit.
    ///
    /// Held back while the inline editor is open (a commit would land on an entry
    /// other than the one being typed into), and wherever nothing covers the
    /// playhead — a gap between cues, or before the first chapter — since there
    /// is nothing there to follow.
    fn follow_playhead(&mut self) {
        if self.mode != Mode::Normal {
            return;
        }
        if let Some(i) = self.vtt.as_ref().and_then(|d| d.active_cue(self.playhead)) {
            self.selected_cue = i;
        }
        if let Some(i) = self.chapters.active(self.playhead) {
            self.selected_chapter = i;
        }
    }

    fn select_cue(&mut self, delta: i32) {
        let Some(doc) = &self.vtt else { return };
        let count = doc.cue_count();
        if count == 0 {
            return;
        }
        let next = (self.selected_cue as i32 + delta).clamp(0, count as i32 - 1) as usize;
        self.go_to_cue(next);
    }

    /// Select cue `i` and put the playhead on its start, so the frame follows
    /// the selection. Seeks *before* recording the selection: the seek follows
    /// the playhead into whichever cue covers that instant, and where cues
    /// overlap that is not the one asked for.
    fn go_to_cue(&mut self, i: usize) {
        if let Some((start, _)) = self.vtt.as_ref().and_then(|d| d.cue_times(i)) {
            self.seek_to(start);
        }
        self.selected_cue = i;
    }

    /// Ask for a string to find in the cue text. Committing an empty one clears
    /// the search (and with it the highlight).
    fn begin_search(&mut self) {
        if self.vtt.is_none() {
            self.status = "no subtitles loaded".into();
            return;
        }
        self.edit_input = TextInput::default();
        self.mode = Mode::Searching;
        self.status = "find in cues — Enter to search, Esc to cancel".into();
    }

    fn handle_search_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.search = self.edit_input.take();
                self.mode = Mode::Normal;
                if self.search.is_empty() {
                    self.status = "search cleared".into();
                } else {
                    // Include the selected cue: searching for what is on screen
                    // shouldn't skip past it.
                    self.find(true, true);
                }
            }
            KeyCode::Esc => {
                self.edit_input.clear();
                self.mode = Mode::Normal;
                self.status = "search cancelled".into();
            }
            _ => self.edit_input.handle_key(key),
        }
    }

    /// Jump to the next (or previous) cue whose text matches the search,
    /// wrapping around the ends. `include_current` keeps the selected cue
    /// eligible, which is what a freshly typed search wants.
    fn find(&mut self, forward: bool, include_current: bool) {
        if self.search.is_empty() {
            self.status = "no search — press / first".into();
            return;
        }
        let Some(doc) = &self.vtt else {
            self.status = "no subtitles loaded".into();
            return;
        };
        let hits: Vec<usize> = doc
            .cue_rows()
            .iter()
            .enumerate()
            .filter(|(_, (_, _, text))| search::contains(text, &self.search))
            .map(|(i, _)| i)
            .collect();
        let Some(&first) = hits.first() else {
            self.status = format!("no match for “{}”", self.search);
            return;
        };

        let cur = self.selected_cue;
        let next = if forward {
            let from = hits.iter().find(|&&i| if include_current { i >= cur } else { i > cur });
            *from.unwrap_or(&first)
        } else {
            let from = hits.iter().rev().find(|&&i| if include_current { i <= cur } else { i < cur });
            *from.unwrap_or(hits.last().expect("hits is not empty"))
        };

        let nth = hits.iter().position(|&i| i == next).expect("next is a hit") + 1;
        self.go_to_cue(next);
        self.status = format!("match {nth} of {} for “{}”", hits.len(), self.search);
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
        self.edit_input = TextInput::with_text(text.replace('\n', " "));
        self.edit_target = EditTarget::Cue;
        self.mode = Mode::Editing;
        self.status = "editing cue".into();
    }

    fn handle_edit_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                let text = self.edit_input.take();
                match self.edit_target {
                    EditTarget::Cue => {
                        if let Some(doc) = &mut self.vtt {
                            doc.set_cue_text(self.selected_cue, &text);
                            self.vtt_dirty = true;
                        }
                        self.status = "cue updated".into();
                    }
                    EditTarget::Chapter => {
                        self.chapters.set_title(self.selected_chapter, text.trim());
                        self.chapters_dirty = true;
                        self.status = "chapter updated".into();
                    }
                }
                self.mode = Mode::Normal;
            }
            KeyCode::Esc => {
                self.edit_input.clear();
                self.mode = Mode::Normal;
                self.status = "edit cancelled".into();
            }
            _ => self.edit_input.handle_key(key),
        }
    }

    /// Type the filename for the export of the marked span (prefilled with a
    /// suggested name that you can replace).
    fn export(&mut self, mode: CutMode) {
        let (Some(start), Some(end)) = (self.mark_in, self.mark_out) else {
            self.status = "set IN (i) and OUT (o) first".into();
            return;
        };
        let stem = self
            .info
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("clip");
        let ext = self
            .info
            .path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("mp4");
        self.edit_input = TextInput::with_text(format!("{stem}-clip.{ext}"));
        self.pending_export = Some(PendingExport { mode, start, end });
        self.mode = Mode::Naming;
        self.status = "save clip as (in source folder) — Enter to cut, Esc to cancel, Ctrl-U clears".into();
    }

    fn handle_naming_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                let name = self.edit_input.take();
                self.mode = Mode::Normal;
                let Some(pe) = self.pending_export.take() else { return };
                if name.trim().is_empty() {
                    self.status = "export cancelled (empty name)".into();
                    return;
                }
                let output = resolve_output(&self.info.path, &name);
                if output.exists() {
                    self.status =
                        format!("{} exists — y to overwrite, n to cancel", output.display());
                    self.pending_cut = Some(PendingCut {
                        output,
                        mode: pe.mode,
                        start: pe.start,
                        end: pe.end,
                    });
                } else {
                    self.run_cut(&output, pe.mode, pe.start, pe.end, false);
                }
            }
            KeyCode::Esc => {
                self.edit_input.clear();
                self.pending_export = None;
                self.mode = Mode::Normal;
                self.status = "export cancelled".into();
            }
            _ => self.edit_input.handle_key(key),
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
        if self.vtt.is_none() {
            self.status = "nothing to save".into();
            return;
        }

        // Preserve the pristine original before the first in-place overwrite.
        let backup = match crate::util::backup_once(&path) {
            Ok(b) => b,
            Err(e) => {
                self.status = format!("backup failed, not saving: {e}");
                return;
            }
        };

        let doc = self.vtt.as_ref().expect("vtt present");
        match doc.save(&path) {
            Ok(()) => {
                self.vtt_dirty = false;
                self.status = match backup {
                    Some(b) => format!("saved {} (original → {})", path.display(), b.display()),
                    None => format!("saved {}", path.display()),
                };
            }
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    /// Move the chapter selection and seek to the newly selected chapter.
    fn select_chapter(&mut self, delta: i32) {
        let count = self.chapters.len();
        if count == 0 {
            self.status = "no chapters (press m to add one)".into();
            return;
        }
        let next = (self.selected_chapter as i32 + delta).clamp(0, count as i32 - 1) as usize;
        // Seek first, then record the selection: see the note in `select_cue`.
        if let Some(t) = self.chapters.time(next) {
            self.seek_to(t);
        }
        self.selected_chapter = next;
    }

    /// Add a chapter at the playhead and start editing its title inline.
    fn new_chapter(&mut self) {
        let idx = self.chapters.add(self.playhead, "");
        self.selected_chapter = idx;
        self.chapters_dirty = true;
        self.begin_edit_chapter();
    }

    fn begin_edit_chapter(&mut self) {
        if self.chapters.is_empty() {
            self.status = "no chapters (press m to add one)".into();
            return;
        }
        let title = self
            .chapters
            .rows()
            .get(self.selected_chapter)
            .map(|c| c.title.clone())
            .unwrap_or_default();
        self.edit_input = TextInput::with_text(title.replace('\n', " "));
        self.edit_target = EditTarget::Chapter;
        self.mode = Mode::Editing;
        self.status = "editing chapter".into();
    }

    fn delete_chapter(&mut self) {
        if self.chapters.is_empty() {
            return;
        }
        self.chapters.delete(self.selected_chapter);
        self.selected_chapter = self
            .selected_chapter
            .min(self.chapters.len().saturating_sub(1));
        self.chapters_dirty = true;
        self.status = "chapter deleted".into();
    }

    fn save_chapters(&mut self) {
        if self.chapters.is_empty() {
            self.status = "no chapters to save".into();
            return;
        }
        let path = self.chapters_path.clone();
        // Preserve the pristine original before the first in-place overwrite.
        let backup = match crate::util::backup_once(&path) {
            Ok(b) => b,
            Err(e) => {
                self.status = format!("backup failed, not saving: {e}");
                return;
            }
        };
        match self.chapters.save(&path) {
            Ok(()) => {
                self.chapters_dirty = false;
                self.status = match backup {
                    Some(b) => format!("saved {} (original → {})", path.display(), b.display()),
                    None => format!("saved {}", path.display()),
                };
            }
            Err(e) => self.status = format!("chapter save failed: {e}"),
        }
    }

    /// Start generating subtitles with WhisperX (key `G`). Only meaningful when
    /// there are no subtitles yet and the video has an audio track.
    fn generate_subs(&mut self) {
        if self.whisperx.is_some() {
            self.status = "subtitle generation already running".into();
            return;
        }
        if self.vtt.as_ref().is_some_and(|d| d.cue_count() > 0) {
            self.status = "subtitles already loaded".into();
            return;
        }
        if self.info.audio_codec.is_none() {
            self.status = "no audio track to transcribe".into();
            return;
        }
        // Write to the chosen --vtt path, or a sibling <video>.vtt.
        let target = self
            .vtt_path
            .clone()
            .unwrap_or_else(|| self.info.path.with_extension("vtt"));
        self.vtt_path = Some(target.clone());
        self.whisperx = Some(WhisperXJob::start(
            self.info.path.clone(),
            target,
            self.wx_cfg.clone(),
        ));
        self.status = "whisperx: starting…".into();
    }

    /// Drain progress from the WhisperX worker: update the status line, and on
    /// completion load the generated subtitles (or report the failure).
    fn poll_whisperx(&mut self) {
        let mut events = Vec::new();
        if let Some(job) = &self.whisperx {
            while let Ok(msg) = job.rx.try_recv() {
                events.push(msg);
            }
        }
        for msg in events {
            match msg {
                Progress::Status(s) => self.status = format!("whisperx: {s}"),
                Progress::Done(path) => {
                    self.whisperx = None; // joins the worker thread
                    match VttDoc::load(&path) {
                        Ok(doc) => {
                            let n = doc.cue_count();
                            self.vtt = Some(doc);
                            self.vtt_path = Some(path.clone());
                            self.vtt_dirty = false;
                            self.selected_cue = 0;
                            // Cues arriving open the caption strip, which takes
                            // its rows from the picture: refit the frame to it.
                            self.pane.invalidate();
                            self.needs_frame = true;
                            self.status =
                                format!("generated {n} cues → {}", path.display());
                        }
                        Err(e) => {
                            self.status = format!("generated subtitles but load failed: {e}")
                        }
                    }
                }
                Progress::Failed(e) => {
                    self.whisperx = None;
                    self.status = format!("subtitle generation failed: {e}");
                }
            }
        }
    }

    fn handle_confirm(&mut self, code: KeyCode) {
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
            Ok(()) => {
                let mut msg = format!("saved {}", output.display());
                if let Some(extra) = self.export_companion_vtt(output, start, end) {
                    msg.push_str(&format!(" + {extra}"));
                }
                if let Some(extra) = self.export_companion_chapters(output, start, end) {
                    msg.push_str(&format!(" + {extra}"));
                }
                self.status = msg;
            }
            Err(e) => self.status = format!("export failed: {e}"),
        }
    }

    /// After a successful clip export, write the trimmed subtitles beside it as
    /// `<clip>.vtt`. Returns a note for the status line (or `None` if there were
    /// no cues in range). Uses the in-memory (edited) subtitles, so any unsaved
    /// cue edits are reflected in the clip.
    fn export_companion_vtt(&self, output: &std::path::Path, start: f64, end: f64) -> Option<String> {
        let doc = self.vtt.as_ref()?;
        let clip = doc.cut(start, end);
        if clip.cue_count() == 0 {
            return None;
        }
        let vtt_out = output.with_extension("vtt");
        let _ = crate::util::backup_once(&vtt_out);
        Some(match clip.save(&vtt_out) {
            Ok(()) => vtt_out.display().to_string(),
            Err(e) => format!("subtitles failed: {e}"),
        })
    }

    /// After a successful clip export, write the clipped chapters beside it as
    /// `<clip>.chapter.txt`. Returns a note for the status line (or `None` if
    /// there were no chapters in range). Uses the in-memory chapters.
    fn export_companion_chapters(&self, output: &std::path::Path, start: f64, end: f64) -> Option<String> {
        let clip = self.chapters.cut(start, end);
        if clip.is_empty() {
            return None;
        }
        let ch_out = output.with_extension("chapter.txt");
        let _ = crate::util::backup_once(&ch_out);
        Some(match clip.save(&ch_out) {
            Ok(()) => ch_out.display().to_string(),
            Err(e) => format!("chapters failed: {e}"),
        })
    }

    /// Whether there are cues to caption the video with.
    pub fn has_cues(&self) -> bool {
        self.vtt.as_ref().is_some_and(|d| d.cue_count() > 0)
    }

    /// The rect the video frame is painted into: the pane less the caption strip.
    fn picture_rect(&self) -> Rect {
        ui::video_split(ui::layout(self.area).video, self.has_cues()).0
    }

    pub fn is_playing(&self) -> bool {
        self.playback.is_some()
    }

    pub fn speed(&self) -> f64 {
        SPEEDS[self.speed_idx]
    }

    /// Space toggles playback. Pausing freezes the playhead where the clock was;
    /// starting streams from the current playhead (restarting from 0 if at end).
    fn toggle_play(&mut self) {
        if let Some(pb) = self.playback.take() {
            let pos = pb.clock();
            drop(pb); // kills ffmpeg/ffplay
            self.playhead = pos.clamp(0.0, self.info.duration);
            self.needs_frame = true;
            self.status = "paused".into();
        } else {
            self.start_playback();
        }
    }

    /// Stream from the current playhead at the current speed.
    fn start_playback(&mut self) {
        if !self.kitty_ok {
            self.status = "playback needs the kitty preview".into();
            return;
        }
        if self.playhead >= self.info.duration {
            self.playhead = 0.0;
        }
        let vrect = self.picture_rect();
        let max_w = vrect.width as u32 * self.cell.w as u32;
        let max_h = vrect.height as u32 * self.cell.h as u32;
        if max_w < 2 || max_h < 2 {
            return;
        }
        let (w, h) = fit_dims(self.info.width, self.info.height, max_w, max_h);
        let speed = self.speed();
        match Playback::start(&self.info, self.playhead, speed, w, h) {
            Ok(pb) => {
                self.playback = Some(pb);
                self.needs_frame = false;
                self.status = format!("playing {}", crate::util::fmt_speed(speed));
            }
            Err(e) => self.status = format!("play failed: {e}"),
        }
    }

    /// Step the playback speed through [`SPEEDS`]. Restarts the stream in place
    /// when playing (ffplay can't retempo live).
    fn change_speed(&mut self, delta: i32) {
        let next = (self.speed_idx as i32 + delta).clamp(0, SPEEDS.len() as i32 - 1) as usize;
        if next == self.speed_idx {
            return;
        }
        self.speed_idx = next;
        self.status = format!("speed {}", crate::util::fmt_speed(self.speed()));
        if self.playback.is_some() {
            // playhead tracks the clock during playback, so resume from here.
            self.playback = None;
            self.start_playback();
        }
    }

    fn seek_to(&mut self, t: f64) {
        // Any manual seek pauses playback.
        if self.playback.take().is_some() {
            self.status = "paused".into();
        }
        // Coarse by default; FrameStep re-enables precise after calling this.
        self.precise_seek = false;
        let clamped = t.clamp(0.0, self.info.duration);
        if (clamped - self.playhead).abs() > f64::EPSILON {
            self.playhead = clamped;
            self.needs_frame = true;
            // Both lists follow the playhead wherever it lands, not just during
            // playback, so a seek scrolls them to where you jumped.
            self.follow_playhead();
        }
    }

    fn load_frame(&mut self) {
        let vrect = self.picture_rect();
        let max_w = vrect.width as u32 * self.cell.w as u32;
        let max_h = vrect.height as u32 * self.cell.h as u32;
        if max_w < 2 || max_h < 2 {
            return;
        }
        let (w, h) = fit_dims(self.info.width, self.info.height, max_w, max_h);
        let precise = self.precise_seek;
        match frame::extract_rgba(&self.info.path, self.playhead, w, h, precise) {
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

/// Resolve a typed name to an output path: a bare name lands in `source`'s
/// folder; a name with a directory or absolute path is used as-is. If no
/// extension is given, `source`'s extension is appended.
fn resolve_output(source: &std::path::Path, name: &str) -> PathBuf {
    let typed = std::path::Path::new(name.trim());
    let has_dir =
        typed.is_absolute() || typed.parent().is_some_and(|p| !p.as_os_str().is_empty());
    let mut out = if has_dir {
        typed.to_path_buf()
    } else {
        match source.parent() {
            Some(dir) => dir.join(typed),
            None => typed.to_path_buf(),
        }
    };
    if out.extension().is_none() {
        if let Some(ext) = source.extension() {
            out.set_extension(ext);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;

    use crate::util::display_width;

    const TERM: (u16, u16) = (100, 30);

    /// An app over `cues` of `(start, end, text)`, with nothing playing.
    fn app_with_cues(cues: &[(f64, f64, String)]) -> App {
        let info = MediaInfo {
            path: PathBuf::from("/tmp/editty-test.mp4"),
            duration: 60.0,
            fps: 30.0,
            width: 1920,
            height: 1080,
            video_codec: "h264".into(),
            audio_codec: None,
        };
        let wx = WhisperXConfig {
            env: "whisperx".into(),
            model: "large-v3".into(),
            device: None,
            language: None,
        };
        let mut app = App::new(info, None, wx);
        let mut doc = VttDoc::empty();
        for (start, end, text) in cues {
            doc.add_cue(*start, *end, text);
        }
        app.vtt = Some(doc);
        app
    }

    fn app_with_cue(text: &str) -> App {
        app_with_cues(&[(0.0, 3.0, text.to_string())])
    }

    /// Twenty cues two seconds apart: cue *n* covers `2n..2n+2`, with a one
    /// second gap left open before the last one. Chapters every ten seconds,
    /// the first of them ten seconds in, so `0..10` is before all of them.
    fn app_with_a_reel() -> App {
        let mut cues: Vec<(f64, f64, String)> = (0..19)
            .map(|i| (i as f64 * 2.0, i as f64 * 2.0 + 2.0, format!("せりふ {}", i + 1)))
            .collect();
        cues.push((39.0, 41.0, "さいご".to_string()));
        let mut app = app_with_cues(&cues);
        for i in 1..=4 {
            app.chapters.add(i as f64 * 10.0, &format!("しょう {i}"));
        }
        app
    }

    fn press(app: &mut App, code: KeyCode, times: usize) {
        for _ in 0..times {
            app.handle_edit_key(KeyEvent::from(code));
        }
    }

    /// Draw the whole UI and read the subtitle pane back: its rows of text, and
    /// the symbol in the inverted (cursor) cell if there is one.
    fn cue_pane(app: &App) -> (Vec<String>, Option<String>) {
        pane_text(app, |areas| ui::inner(areas.cues))
    }

    /// The caption strip at the foot of the video pane.
    fn caption_rows(app: &App) -> Vec<String> {
        pane_text(app, |areas| ui::video_split(areas.video, true).1).0
    }

    fn pane_text(app: &App, pick: fn(&ui::Areas) -> Rect) -> (Vec<String>, Option<String>) {
        let (w, h) = TERM;
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        terminal.draw(|f| ui::render(f, app)).expect("draw");
        let buf = terminal.backend().buffer().clone();
        let pane = pick(&ui::layout(Rect::new(0, 0, w, h)));

        let mut cursor = None;
        let mut rows = Vec::new();
        for dy in 0..pane.height {
            let mut row = String::new();
            let mut dx = 0;
            while dx < pane.width {
                let cell = &buf[(pane.x + dx, pane.y + dy)];
                if cursor.is_none() && cell.modifier.contains(Modifier::REVERSED) {
                    cursor = Some(cell.symbol().to_string());
                }
                row.push_str(cell.symbol());
                // Step over the blank cell a double-width character occupies.
                dx += (display_width(cell.symbol()) as u16).max(1);
            }
            rows.push(row);
        }
        (rows, cursor)
    }

    #[test]
    fn typing_lands_where_the_cursor_was_moved_to() {
        let text = "これは長い日本語の字幕です。カーソルを動かして書き足せる必要があります。";
        let mut app = app_with_cue(text);
        app.begin_edit();

        // Walk ten characters back from the end and insert there.
        press(&mut app, KeyCode::Left, 10);
        press(&mut app, KeyCode::Char('X'), 1);

        let at = text.char_indices().nth_back(9).expect("ten chars in").0;
        assert_eq!(
            app.edit_input.text(),
            format!("{}X{}", &text[..at], &text[at..]),
            "the insertion belongs where the cursor was, not at the end"
        );

        // On screen, the cursor sits on the character it is in front of.
        let (_, cursor) = cue_pane(&app);
        assert_eq!(cursor.as_deref(), Some(&text[at..]).map(|s| &s[..3]));
    }

    #[test]
    fn a_long_cue_wraps_and_scrolls_to_the_end() {
        // Far more text than the six-row pane can hold at once.
        let text = "字幕がとても長い場合の話です。".repeat(12);
        let mut app = app_with_cue(&text);
        app.begin_edit(); // cursor starts at the end of the text

        let (rows, cursor) = cue_pane(&app);
        assert!(
            rows.iter().filter(|r| !r.trim().is_empty()).count() > 1,
            "a long cue has to wrap onto further rows: {rows:#?}"
        );
        assert!(
            rows.iter().any(|r| r.contains("長い場合の話です。")),
            "the pane scrolled to the cursor, so the tail is on screen: {rows:#?}"
        );
        assert_eq!(cursor.as_deref(), Some(" "), "the end-of-line cursor is a blank cell");

        // Back to the start: the first row carries the cue's timings again.
        press(&mut app, KeyCode::Home, 1);
        let (rows, _) = cue_pane(&app);
        assert!(
            rows[0].contains("00:00:00.000→00:00:03.000"),
            "Home scrolls back to the head of the cue: {rows:#?}"
        );
        assert!(rows[0].contains("字幕がとても"), "{rows:#?}");
    }

    #[test]
    fn the_video_pane_captions_the_cue_under_the_playhead() {
        // A sentence far too long for the 60%-wide cue list to show whole.
        let long = "これは非常に長い字幕で、字幕ウィンドウの右端では見切れてしまう\
                    ぐらいの長さがあります。";
        let mut app = app_with_cues(&[
            (0.0, 2.0, "みじかい".to_string()),
            (2.0, 6.0, long.to_string()),
        ]);

        app.playhead = 0.5;
        assert_eq!(caption_rows(&app).join("").trim(), "みじかい");

        app.playhead = 3.0;
        let shown = caption_rows(&app).join("");
        assert_eq!(shown.replace(' ', ""), long, "the whole cue, wrapped");

        // The cue list really does cut it off, which is what the strip is for.
        let (rows, _) = cue_pane(&app);
        assert!(
            !rows.iter().any(|r| r.contains("長さがあります")),
            "the list shows only the head of it: {rows:#?}"
        );

        // Between cues a player shows nothing, and so does this.
        app.playhead = 8.0;
        assert_eq!(caption_rows(&app).join("").trim(), "");
    }

    #[test]
    fn a_caption_too_long_even_for_the_strip_says_so() {
        let huge = "字".repeat(400);
        let mut app = app_with_cues(&[(0.0, 4.0, huge)]);
        app.playhead = 1.0;
        let shown = caption_rows(&app);
        assert_eq!(shown.len(), 3, "the strip is three rows");
        assert!(
            shown.last().unwrap().trim_end().ends_with('…'),
            "a clipped caption must not pass for a whole one: {shown:#?}"
        );
    }

    #[test]
    fn playback_carries_the_selection_to_the_spoken_cue() {
        let mut app = app_with_a_reel();
        assert_eq!(app.selected_cue, 0);

        app.playhead = 31.5; // inside cue 16 (30..32)
        app.follow_playhead();
        assert_eq!(app.selected_cue, 15);

        // Playing on through the reel, the selection keeps up.
        for t in [33.0, 35.0, 37.0] {
            app.playhead = t;
            app.follow_playhead();
        }
        assert_eq!(app.selected_cue, 18, "cue 19 covers 36..38");

        // A gap between cues has nothing to follow: stay on the last one heard.
        app.playhead = 38.5;
        app.follow_playhead();
        assert_eq!(app.selected_cue, 18);

        app.playhead = 40.0;
        app.follow_playhead();
        assert_eq!(app.selected_cue, 19);
    }

    #[test]
    fn the_chapter_selection_follows_too() {
        let mut app = app_with_a_reel();
        assert_eq!(app.selected_chapter, 0);

        app.playhead = 25.0; // chapter 2 runs from 20 until 30
        app.follow_playhead();
        assert_eq!(app.selected_chapter, 1);

        // Before the first chapter there is nothing to follow: stay put.
        app.playhead = 5.0;
        app.follow_playhead();
        assert_eq!(app.selected_chapter, 1);
    }

    #[test]
    fn a_seek_scrolls_both_lists_to_where_it_landed() {
        let mut app = app_with_a_reel();
        app.seek_to(31.5);
        assert_eq!(app.selected_cue, 15, "cue 16 covers 30..32");
        assert_eq!(app.selected_chapter, 2, "chapter 3 runs from 30");

        // Seeking back carries them both with it.
        app.seek_to(3.0);
        assert_eq!(app.selected_cue, 1);
        assert_eq!(app.selected_chapter, 2, "nothing precedes chapter 1 at 3s");
    }

    #[test]
    fn navigation_lands_on_the_entry_it_picked_despite_overlap() {
        // A cue that spans a shorter one: seeking to the short cue's start puts
        // the playhead inside the long one too, and `active_cue` prefers the
        // first. Selecting must still land on the cue that was picked.
        let mut app = app_with_cues(&[
            (0.0, 10.0, "ながい".to_string()),
            (5.0, 6.0, "みじかい".to_string()),
        ]);
        app.apply(Action::CueNext);
        assert_eq!(app.selected_cue, 1);
        assert_eq!(app.playhead, 5.0);

        // Same for chapters, whose "active" is the last one at or before the
        // playhead — seeking exactly onto one is the boundary case.
        app.chapters.add(0.0, "いち");
        app.chapters.add(20.0, "に");
        app.apply(Action::ChapterNext);
        assert_eq!(app.selected_chapter, 1);
        assert_eq!(app.playhead, 20.0);
    }

    #[test]
    fn following_holds_off_while_the_editor_is_open() {
        let mut app = app_with_a_reel();
        app.begin_edit(); // editing cue 1
        app.playhead = 31.5;
        app.follow_playhead();
        assert_eq!(
            app.selected_cue, 0,
            "the commit has to land on the cue being typed into"
        );

        // Once the edit is committed, following resumes.
        app.handle_edit_key(KeyEvent::from(KeyCode::Enter));
        app.follow_playhead();
        assert_eq!(app.selected_cue, 15);
    }

    #[test]
    fn the_cue_list_scrolls_along_with_playback() {
        let mut app = app_with_a_reel();
        let (rows, _) = cue_pane(&app);
        assert!(rows[0].contains("せりふ 1"), "starts at the top: {rows:#?}");

        app.playhead = 31.5;
        app.follow_playhead();
        let (rows, _) = cue_pane(&app);
        let shown = rows.join("\n");
        assert!(
            shown.contains("せりふ 16"),
            "the spoken cue has to be on screen: {rows:#?}"
        );
        assert!(
            !shown.contains("せりふ 1 "),
            "and the top of the list has scrolled away: {rows:#?}"
        );
    }

    /// Four cues, two of them mentioning the projector.
    fn app_for_search() -> App {
        app_with_cues(&[
            (0.0, 2.0, "はじめまして".to_string()),
            (2.0, 4.0, "プロジェクターの話".to_string()),
            (4.0, 6.0, "べつの話です".to_string()),
            (6.0, 8.0, "プロジェクターをつかう".to_string()),
        ])
    }

    /// Type a search the way the keyboard does: `/`, the text, Enter.
    fn type_search(app: &mut App, query: &str) {
        app.apply(Action::SearchStart);
        assert_eq!(app.mode, Mode::Searching);
        for c in query.chars() {
            app.handle_search_key(KeyEvent::from(KeyCode::Char(c)));
        }
        app.handle_search_key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Normal);
    }

    /// The text of every highlighted (yellow) cell in the subtitle pane.
    fn highlighted(app: &App) -> String {
        let (w, h) = TERM;
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        terminal.draw(|f| ui::render(f, app)).expect("draw");
        let buf = terminal.backend().buffer().clone();
        let pane = ui::inner(ui::layout(Rect::new(0, 0, w, h)).cues);

        let mut out = String::new();
        for dy in 0..pane.height {
            let mut dx = 0;
            while dx < pane.width {
                let cell = &buf[(pane.x + dx, pane.y + dy)];
                if cell.bg == ratatui::style::Color::Yellow {
                    out.push_str(cell.symbol());
                }
                dx += (display_width(cell.symbol()) as u16).max(1);
            }
        }
        out
    }

    #[test]
    fn a_search_jumps_to_the_matching_cue() {
        let mut app = app_for_search();
        type_search(&mut app, "プロジェクター");

        assert_eq!(app.selected_cue, 1);
        assert_eq!(app.playhead, 2.0, "and the video goes there too");
        assert!(app.status.contains("match 1 of 2"), "{}", app.status);

        // Tab / Shift-Tab walk the matches, wrapping at both ends.
        app.apply(Action::SearchNext);
        assert_eq!(app.selected_cue, 3);
        assert!(app.status.contains("match 2 of 2"), "{}", app.status);
        app.apply(Action::SearchNext);
        assert_eq!(app.selected_cue, 1, "wrapped back to the first match");
        app.apply(Action::SearchPrev);
        assert_eq!(app.selected_cue, 3, "and wrapped the other way");
    }

    #[test]
    fn a_search_ignores_case_and_reports_a_miss() {
        let mut app = app_with_cues(&[
            (0.0, 2.0, "nothing here".to_string()),
            (2.0, 4.0, "the UTOR system".to_string()),
        ]);
        type_search(&mut app, "utor");
        assert_eq!(app.selected_cue, 1);

        type_search(&mut app, "ビルドリ");
        assert_eq!(app.selected_cue, 1, "a miss leaves the selection alone");
        assert!(app.status.contains("no match"), "{}", app.status);
    }

    #[test]
    fn an_empty_search_clears_it_and_escape_keeps_the_old_one() {
        let mut app = app_for_search();
        type_search(&mut app, "プロジェクター");
        assert!(!highlighted(&app).is_empty());

        // Esc while typing a new one keeps the search that was already set.
        app.apply(Action::SearchStart);
        app.handle_search_key(KeyEvent::from(KeyCode::Char('x')));
        app.handle_search_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.search, "プロジェクター");

        type_search(&mut app, "");
        assert_eq!(app.search, "");
        assert_eq!(highlighted(&app), "", "clearing takes the highlight with it");
    }

    #[test]
    fn the_matched_word_is_highlighted_wherever_it_shows() {
        let mut app = app_for_search();
        type_search(&mut app, "プロジェクター");
        // Both matching cues are on screen, so the word is marked twice — and
        // nothing else is.
        assert_eq!(highlighted(&app), "プロジェクタープロジェクター");
    }

    #[test]
    fn escape_leaves_the_cue_untouched() {
        let mut app = app_with_cue("そのまま");
        app.begin_edit();
        press(&mut app, KeyCode::Char('あ'), 1);
        app.handle_edit_key(KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.vtt.as_ref().unwrap().cue_text(0).as_deref(), Some("そのまま"));
        assert!(!app.vtt_dirty);
    }

    #[test]
    fn bare_name_lands_in_source_folder() {
        let out = resolve_output(Path::new("/movies/talk.mp4"), "intro.mp4");
        assert_eq!(out, PathBuf::from("/movies/intro.mp4"));
    }

    #[test]
    fn missing_extension_inherits_source() {
        let out = resolve_output(Path::new("/movies/talk.mkv"), "intro");
        assert_eq!(out, PathBuf::from("/movies/intro.mkv"));
    }

    #[test]
    fn explicit_directory_is_respected() {
        let out = resolve_output(Path::new("/movies/talk.mp4"), "/tmp/clip.mp4");
        assert_eq!(out, PathBuf::from("/tmp/clip.mp4"));
    }
}
