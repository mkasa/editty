//! Time helpers shared across the app. Internally we represent positions as
//! `f64` seconds; these convert to/from the string forms ffmpeg and WebVTT use.

use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// How many terminal cells `s` occupies (CJK characters take two).
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// The `.orig` sidecar path for a file (e.g. `subs.vtt` -> `subs.vtt.orig`).
/// Appends rather than replacing the extension so the source extension stays
/// visible and two files with the same stem can't collide.
pub fn orig_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".orig");
    PathBuf::from(name)
}

/// Preserve the pristine original before the first overwrite: if `path` exists
/// and its `.orig` sidecar does not, copy `path` to `<path>.orig`. Does nothing
/// if `path` is new or a backup already exists (so `.orig` always holds the
/// true pre-edit version). Returns the backup path when one is created.
pub fn backup_once(path: &Path) -> io::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let backup = orig_path(path);
    if backup.exists() {
        return Ok(None);
    }
    std::fs::copy(path, &backup)?;
    Ok(Some(backup))
}

/// Wrap `text` to `width` columns, breaking after a blank when the line has one
/// (so words stay whole) and mid-run otherwise. The blank stays on the line it
/// ended, and no byte is ever dropped: the ranges tile the whole string, which
/// is what lets the caller map a cursor offset onto a line.
pub fn wrap(text: &str, width: usize) -> Vec<Range<usize>> {
    let width = width.max(1);
    let mut lines: Vec<Range<usize>> = Vec::new();
    let mut start = 0;
    let mut col = 0;
    // Byte offset just past the last blank on the current line, if any.
    let mut after_blank: Option<usize> = None;

    for (i, c) in text.char_indices() {
        let cw = c.width().unwrap_or(0);
        // `i > start` keeps at least one character per line, so a character
        // wider than the pane can't spin here forever.
        if col + cw > width && i > start {
            let brk = match after_blank {
                Some(b) if b > start && b < i => b,
                _ => i,
            };
            lines.push(start..brk);
            start = brk;
            after_blank = None;
            col = display_width(&text[start..i]);
        }
        col += cw;
        if c.is_whitespace() {
            after_blank = Some(i + c.len_utf8());
        }
    }
    lines.push(start..text.len());
    lines
}

/// Format seconds as `HH:MM:SS.mmm` (WebVTT-style, always with hours).
pub fn fmt_timestamp(secs: f64) -> String {
    let secs = secs.max(0.0);
    let total_ms = (secs * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_s = total_ms / 1000;
    let s = total_s % 60;
    let m = (total_s / 60) % 60;
    let h = total_s / 3600;
    format!("{h:02}:{m:02}:{s:02}.{ms:03}")
}

/// Compact `M:SS` / `H:MM:SS` for status bars.
pub fn fmt_clock(secs: f64) -> String {
    let secs = secs.max(0.0);
    let total = secs.round() as u64;
    let s = total % 60;
    let m = (total / 60) % 60;
    let h = total / 3600;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Format seconds the way ffmpeg's `-ss`/`-to` accept (plain seconds with ms).
pub fn fmt_ffmpeg_time(secs: f64) -> String {
    format!("{:.3}", secs.max(0.0))
}

/// Format a playback speed compactly: `1x`, `0.5x`, `1.5x`.
pub fn fmt_speed(s: f64) -> String {
    if s.fract().abs() < 1e-9 {
        format!("{}x", s as i64)
    } else {
        format!("{s}x")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_measures_cells_not_chars() {
        // Each kana is two cells wide, so four fit in eight columns.
        let text = "あいうえおか";
        let lines = wrap(text, 8);
        assert_eq!(
            lines.iter().map(|r| &text[r.clone()]).collect::<Vec<_>>(),
            vec!["あいうえ", "おか"]
        );
    }

    #[test]
    fn wrapping_keeps_words_whole_when_it_can() {
        let text = "the quick brown fox";
        let lines = wrap(text, 10);
        assert_eq!(
            lines.iter().map(|r| &text[r.clone()]).collect::<Vec<_>>(),
            vec!["the quick ", "brown fox"]
        );
    }

    #[test]
    fn wrapping_breaks_a_word_too_long_to_fit() {
        let text = "ab wwwwwwwwwwww";
        let lines = wrap(text, 6);
        // Contiguous, and no line is empty, however long the word is.
        assert!(lines.windows(2).all(|w| w[0].end == w[1].start));
        assert_eq!(lines.first().unwrap().start, 0);
        assert_eq!(lines.last().unwrap().end, text.len());
        assert!(lines.iter().all(|r| !r.is_empty()));
    }

    #[test]
    fn wrapping_tiles_the_whole_text() {
        let text = "字幕は長くなることがあります。 with some latin too";
        for width in [1, 2, 3, 7, 40] {
            let lines = wrap(text, width);
            assert_eq!(lines.first().unwrap().start, 0, "width {width}");
            assert_eq!(lines.last().unwrap().end, text.len(), "width {width}");
            assert!(
                lines.windows(2).all(|w| w[0].end == w[1].start),
                "width {width} left a gap"
            );
        }
    }

    #[test]
    fn backup_once_preserves_pristine_original() {
        let dir = std::env::temp_dir().join(format!("editty_backup_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("subs.vtt");
        std::fs::write(&file, b"original").unwrap();

        // First save: a backup is made with the original contents.
        let b = backup_once(&file).unwrap();
        assert_eq!(b, Some(dir.join("subs.vtt.orig")));
        assert_eq!(std::fs::read(dir.join("subs.vtt.orig")).unwrap(), b"original");

        // Simulate an edit, then a second save: backup is NOT overwritten.
        std::fs::write(&file, b"edited").unwrap();
        assert_eq!(backup_once(&file).unwrap(), None);
        assert_eq!(
            std::fs::read(dir.join("subs.vtt.orig")).unwrap(),
            b"original",
            ".orig must keep the pristine pre-edit version"
        );

        // A brand-new file (no existing original) needs no backup.
        let fresh = dir.join("new.vtt");
        assert_eq!(backup_once(&fresh).unwrap(), None);

        std::fs::remove_dir_all(&dir).ok();
    }
}
