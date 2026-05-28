//! Time helpers shared across the app. Internally we represent positions as
//! `f64` seconds; these convert to/from the string forms ffmpeg and WebVTT use.

use std::io;
use std::path::{Path, PathBuf};

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

#[cfg(test)]
mod tests {
    use super::*;

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
