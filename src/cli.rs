use std::path::PathBuf;

use clap::Parser;

/// editty — a terminal video editor for kitty.
///
/// Scrub and preview a video in the terminal, mark IN/OUT points to cut a
/// segment, and edit the associated WebVTT subtitles.
#[derive(Parser, Debug)]
#[command(name = "editty", version, about)]
pub struct Cli {
    /// Video file to open.
    pub video: PathBuf,

    /// WebVTT subtitle file. Defaults to a sibling `<video-stem>.vtt` if present.
    #[arg(long)]
    pub vtt: Option<PathBuf>,

    /// Spike mode: print a single frame at the given time (seconds) via the
    /// kitty graphics protocol and exit. Use this to confirm graphics work in
    /// your terminal before launching the full TUI.
    #[arg(long, value_name = "SECONDS")]
    pub show: Option<f64>,

    /// Whisper model used when generating subtitles (key `G`).
    #[arg(long, value_name = "MODEL", default_value = "large-v3")]
    pub whisper_model: String,

    /// Device for WhisperX (`cuda` / `cpu`). Defaults to auto-detect.
    #[arg(long, value_name = "DEVICE")]
    pub whisper_device: Option<String>,

    /// Spoken language for WhisperX. Defaults to auto-detect.
    #[arg(long, value_name = "LANG")]
    pub whisper_lang: Option<String>,

    /// conda environment to run (and create on first use) WhisperX in.
    #[arg(long, value_name = "ENV", default_value = "whisperx")]
    pub whisper_env: String,
}

impl Cli {
    /// Resolve the VTT path: explicit `--vtt`, else a sibling `.vtt`, else None.
    pub fn resolve_vtt(&self) -> Option<PathBuf> {
        if let Some(p) = &self.vtt {
            return Some(p.clone());
        }
        let sibling = self.video.with_extension("vtt");
        sibling.exists().then_some(sibling)
    }

    /// Build the WhisperX configuration from the CLI flags.
    pub fn whisperx_config(&self) -> crate::whisperx::WhisperXConfig {
        crate::whisperx::WhisperXConfig {
            env: self.whisper_env.clone(),
            model: self.whisper_model.clone(),
            device: self.whisper_device.clone(),
            language: self.whisper_lang.clone(),
        }
    }
}
