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
    let secs = secs.max(0.0);
    let scale = format!("scale={width}:{height}:flags=fast_bilinear");

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error"]);
    if precise {
        // Two-stage seek: a fast input seek to a keyframe shortly before the
        // target, then a short accurate decode forward to the exact frame. This
        // stays frame-accurate while only decoding ~one GOP, even deep into a
        // long file (a plain output seek would decode from the very start).
        const PRESEEK: f64 = 0.5;
        let coarse = (secs - PRESEEK).max(0.0);
        cmd.args(["-ss", &format!("{coarse:.3}")]);
        cmd.arg("-i").arg(path);
        cmd.args(["-ss", &format!("{:.3}", secs - coarse)]);
    } else {
        // Fast input seek to the nearest keyframe (good enough for scrubbing).
        cmd.args(["-ss", &format!("{secs:.3}")]);
        cmd.arg("-i").arg(path);
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
            return Err(anyhow!("ffmpeg failed to extract frame at {secs:.3}s"));
        }
        return Err(anyhow!(
            "short frame at {secs:.3}s: got {} bytes, expected {expected}",
            rgba.len()
        ));
    }
    rgba.truncate(expected);

    Ok(Frame { width, height, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_dims_preserves_aspect_and_is_even() {
        // 640x360 (16:9) into a 200x200 box -> width-bound, even dims.
        let (w, h) = fit_dims(640, 360, 200, 200);
        assert_eq!((w, h), (200, 112));
        assert_eq!(w % 2, 0);
        assert_eq!(h % 2, 0);
        // Never upscales beyond the source.
        assert_eq!(fit_dims(640, 360, 4000, 4000), (640, 360));
    }

    #[test]
    fn precise_step_yields_distinct_adjacent_frames() {
        let input = std::path::Path::new("assets/sample.mp4");
        if !input.exists() {
            eprintln!("skipping: assets/sample.mp4 missing");
            return;
        }
        let (w, h) = (96, 54);
        let a = extract_rgba(input, 2.0, w, h, true).expect("frame a");
        let b = extract_rgba(input, 2.0 + 1.0 / 30.0, w, h, true).expect("frame b");
        assert_eq!(a.rgba.len(), b.rgba.len());
        assert_ne!(
            a.rgba, b.rgba,
            "stepping one frame with precise seek must change the image"
        );
    }
}
