//! Extract a single frame at a timestamp as raw RGBA bytes, for scrubbing.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};

/// A decoded frame: tightly-packed RGBA8 of `width * height * 4` bytes.
#[derive(Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Fit `(src_w, src_h)` inside `(max_w, max_h)` preserving aspect ratio,
/// rounding to even dimensions (ffmpeg scalers prefer even sizes). Never
/// upscales beyond the source.
pub fn fit_dims(src_w: u32, src_h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    if src_w == 0 || src_h == 0 || max_w == 0 || max_h == 0 {
        return (2, 2);
    }
    let scale = (max_w as f64 / src_w as f64)
        .min(max_h as f64 / src_h as f64)
        .min(1.0);
    let w = ((src_w as f64 * scale).round() as u32).max(2) & !1;
    let h = ((src_h as f64 * scale).round() as u32).max(2) & !1;
    (w, h)
}

/// Extract one frame at `secs`, scaled to exactly `width x height` RGBA.
///
/// `precise = false` seeks before `-i` (fast keyframe seek, good for scrubbing);
/// `precise = true` seeks after `-i` (decode-from-keyframe, exact frame, slower).
pub fn extract_rgba(
    path: &Path,
    secs: f64,
    width: u32,
    height: u32,
    precise: bool,
) -> Result<Frame> {
    let ts = format!("{:.3}", secs.max(0.0));
    let scale = format!("scale={width}:{height}:flags=fast_bilinear");

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error"]);
    if !precise {
        cmd.args(["-ss", &ts]);
    }
    cmd.arg("-i").arg(path);
    if precise {
        cmd.args(["-ss", &ts]);
    }
    cmd.args([
        "-frames:v", "1",
        "-vf", &scale,
        "-pix_fmt", "rgba",
        "-f", "rawvideo",
        "-",
    ]);
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().context("failed to spawn ffmpeg")?;
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    child
        .stdout
        .take()
        .expect("piped stdout")
        .read_to_end(&mut rgba)
        .context("reading ffmpeg frame output")?;
    let status = child.wait().context("waiting for ffmpeg")?;

    let expected = (width as usize) * (height as usize) * 4;
    if rgba.len() < expected {
        if !status.success() {
            return Err(anyhow!("ffmpeg failed to extract frame at {ts}s"));
        }
        return Err(anyhow!(
            "short frame at {ts}s: got {} bytes, expected {expected}",
            rgba.len()
        ));
    }
    rgba.truncate(expected);

    Ok(Frame { width, height, rgba })
}
