//! Export the segment between two timestamps via ffmpeg.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use crate::util::fmt_ffmpeg_time;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CutMode {
    /// Stream-copy: instant, but cut points snap to keyframes.
    Fast,
    /// Re-encode: frame-accurate cut points, slower.
    Precise,
}

/// Cut `[start, end)` from `input` into `output`. Refuses to overwrite unless
/// `overwrite` is set. Returns ffmpeg's stderr tail on failure.
pub fn cut(
    input: &Path,
    output: &Path,
    start: f64,
    end: f64,
    mode: CutMode,
    overwrite: bool,
) -> Result<()> {
    if end <= start {
        return Err(anyhow!("OUT marker must be after IN marker"));
    }
    if output.exists() && !overwrite {
        return Err(anyhow!("{} already exists", output.display()));
    }

    let start_s = fmt_ffmpeg_time(start);
    let to_s = fmt_ffmpeg_time(end);

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    match mode {
        CutMode::Fast => {
            // Input seek before -i for speed; copy streams (snaps to keyframes).
            cmd.args(["-ss", &start_s, "-to", &to_s]);
            cmd.arg("-i").arg(input);
            cmd.args(["-c", "copy"]);
        }
        CutMode::Precise => {
            // Output seek after -i for frame accuracy; re-encode.
            cmd.arg("-i").arg(input);
            cmd.args(["-ss", &start_s, "-to", &to_s]);
            cmd.args(["-c:v", "libx264", "-preset", "veryfast", "-c:a", "aac"]);
        }
    }
    cmd.arg(output);

    let out = cmd.output().context("failed to spawn ffmpeg")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("ffmpeg cut failed: {}", stderr.trim()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::probe;

    #[test]
    fn rejects_inverted_marks() {
        let out = std::env::temp_dir().join("editty_inverted.mp4");
        let err = cut(
            std::path::Path::new("assets/sample.mp4"),
            &out,
            5.0,
            2.0,
            CutMode::Fast,
            true,
        );
        assert!(err.is_err());
    }

    #[test]
    fn precise_cut_has_expected_duration() {
        let input = std::path::Path::new("assets/sample.mp4");
        if !input.exists() {
            eprintln!("skipping: assets/sample.mp4 missing");
            return;
        }
        let out = std::env::temp_dir().join("editty_precise_cut.mp4");
        let _ = std::fs::remove_file(&out);
        cut(input, &out, 2.0, 5.0, CutMode::Precise, true).expect("cut should succeed");
        let info = probe(&out).expect("probe output");
        // Precise (re-encode) cut should be within a frame or two of 3s.
        assert!(
            (info.duration - 3.0).abs() < 0.2,
            "expected ~3s, got {}",
            info.duration
        );
        let _ = std::fs::remove_file(&out);
    }
}
