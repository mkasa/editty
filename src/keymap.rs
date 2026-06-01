//! Map raw key events to high-level [`Action`]s. Keeping this separate from the
//! app lets the keymap grow per feature without tangling the event loop.

use ratatui::crossterm::event::{KeyCode, KeyEvent};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Quit,
    TogglePlay,
    SpeedUp,
    SpeedDown,
    /// Seek relative to the playhead, in seconds.
    SeekBy(f64),
    /// Step whole frames (sign = direction).
    FrameStep(i32),
    SeekStart,
    SeekEnd,
    /// Jump to a fraction (0.0..=1.0) of the duration.
    SeekFraction(f64),
    SetIn,
    SetOut,
    ClearMarks,
    ExportFast,
    ExportPrecise,
    CueNext,
    CuePrev,
    EditCue,
    SnapStart,
    SnapEnd,
    NewCue,
    DeleteCue,
    SaveVtt,
    NewChapter,
    EditChapter,
    DeleteChapter,
    ChapterNext,
    ChapterPrev,
    SaveChapters,
    Help,
    Nothing,
}

pub fn map(key: KeyEvent) -> Action {
    use Action::*;
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => Quit,
        KeyCode::Char(' ') => TogglePlay,
        KeyCode::Char('-') | KeyCode::Char('_') => SpeedDown,
        KeyCode::Char('=') | KeyCode::Char('+') => SpeedUp,
        KeyCode::Left => SeekBy(-1.0),
        KeyCode::Right => SeekBy(1.0),
        KeyCode::Char('<') => SeekBy(-10.0),
        KeyCode::Char('>') => SeekBy(10.0),
        KeyCode::Char(',') => FrameStep(-1),
        KeyCode::Char('.') => FrameStep(1),
        KeyCode::Home => SeekStart,
        KeyCode::End => SeekEnd,
        KeyCode::Char('i') => SetIn,
        KeyCode::Char('o') => SetOut,
        KeyCode::Char('C') => ClearMarks,
        KeyCode::Char('x') => ExportFast,
        KeyCode::Char('X') => ExportPrecise,
        KeyCode::Char('j') | KeyCode::Down => CueNext,
        KeyCode::Char('k') | KeyCode::Up => CuePrev,
        KeyCode::Enter => EditCue,
        KeyCode::Char('[') => SnapStart,
        KeyCode::Char(']') => SnapEnd,
        KeyCode::Char('n') => NewCue,
        KeyCode::Char('d') => DeleteCue,
        KeyCode::Char('s') => SaveVtt,
        KeyCode::Char('m') => NewChapter,
        KeyCode::Char('e') => EditChapter,
        KeyCode::Char('M') => DeleteChapter,
        KeyCode::Char('}') => ChapterNext,
        KeyCode::Char('{') => ChapterPrev,
        KeyCode::Char('S') => SaveChapters,
        KeyCode::Char('?') => Help,
        KeyCode::Char(d @ '0'..='9') => SeekFraction((d as u8 - b'0') as f64 / 10.0),
        _ => Nothing,
    }
}
