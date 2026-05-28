//! Probe a media file via `ffprobe -print_format json` into `MediaInfo`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

/// Everything the UI needs to seed the clock, timeline, and frame extraction.
#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub path: PathBuf,
    pub duration: f64,
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub video_codec: String,
    pub audio_codec: Option<String>,
}

#[derive(Deserialize)]
struct ProbeOutput {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<Format>,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
    avg_frame_rate: Option<String>,
    duration: Option<String>,
}

#[derive(Deserialize)]
struct Format {
    duration: Option<String>,
}

/// Parse an ffmpeg rational like `30000/1001` into frames per second.
fn parse_rational(s: &str) -> Option<f64> {
    let (num, den) = s.split_once('/')?;
    let num: f64 = num.trim().parse().ok()?;
    let den: f64 = den.trim().parse().ok()?;
    if den == 0.0 { None } else { Some(num / den) }
}

pub fn probe(path: &Path) -> Result<MediaInfo> {
    let output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .context("failed to spawn ffprobe (is ffmpeg installed and on PATH?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("ffprobe failed for {}: {}", path.display(), stderr.trim()));
    }

    let parsed: ProbeOutput =
        serde_json::from_slice(&output.stdout).context("could not parse ffprobe JSON")?;

    let video = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| anyhow!("no video stream found in {}", path.display()))?;

    let audio = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("audio"));

    // Prefer avg_frame_rate (handles some VFR sources better), fall back to r_frame_rate.
    let fps = video
        .avg_frame_rate
        .as_deref()
        .and_then(parse_rational)
        .filter(|&f| f > 0.0)
        .or_else(|| video.r_frame_rate.as_deref().and_then(parse_rational))
        .unwrap_or(30.0);

    let duration = parsed
        .format
        .as_ref()
        .and_then(|f| f.duration.as_deref())
        .or(video.duration.as_deref())
        .and_then(|d| d.parse::<f64>().ok())
        .ok_or_else(|| anyhow!("could not determine duration of {}", path.display()))?;

    Ok(MediaInfo {
        path: path.to_path_buf(),
        duration,
        fps,
        width: video.width.unwrap_or(0),
        height: video.height.unwrap_or(0),
        video_codec: video.codec_name.clone().unwrap_or_else(|| "?".into()),
        audio_codec: audio.and_then(|a| a.codec_name.clone()),
    })
}
