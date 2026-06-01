//! Generate subtitles with [WhisperX](https://github.com/m-bain/whisperX),
//! shelling out through a dedicated conda environment.
//!
//! The whole job — creating the conda env and `pip install`ing WhisperX the
//! first time, then transcribing — runs on a background thread so the UI stays
//! responsive. Progress lines and the final result are sent back over a channel
//! that the event loop drains each tick (mirroring how playback streams frames).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, unbounded};

/// Knobs for a transcription run; defaults come from the CLI.
#[derive(Clone)]
pub struct WhisperXConfig {
    /// conda environment name to run (and create, if missing) WhisperX in.
    pub env: String,
    /// Whisper model name, e.g. `large-v3`.
    pub model: String,
    /// `cuda` / `cpu`; `None` auto-detects (CUDA if an NVIDIA GPU is present).
    pub device: Option<String>,
    /// Spoken language; `None` lets WhisperX auto-detect.
    pub language: Option<String>,
}

/// A message from the worker thread to the UI.
pub enum Progress {
    /// A human-readable status line to show in the status bar.
    Status(String),
    /// Transcription finished; the VTT was written to this path.
    Done(PathBuf),
    /// The job failed; carries a short reason.
    Failed(String),
}

/// A running (or finished) WhisperX job. Dropping it cancels the job: the
/// current child process is killed and the worker thread is joined.
pub struct WhisperXJob {
    pub rx: Receiver<Progress>,
    child: Arc<Mutex<Option<Child>>>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl WhisperXJob {
    /// Spawn the worker. `video` is transcribed and the resulting subtitles are
    /// written to `target_vtt`.
    pub fn start(video: PathBuf, target_vtt: PathBuf, cfg: WhisperXConfig) -> Self {
        let (tx, rx) = unbounded();
        let child: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let cancel = Arc::new(AtomicBool::new(false));

        let (c2, cancel2) = (Arc::clone(&child), Arc::clone(&cancel));
        let handle = std::thread::spawn(move || {
            let result = run_job(&video, &target_vtt, &cfg, &tx, &c2, &cancel2);
            let _ = match result {
                Ok(path) => tx.send(Progress::Done(path)),
                Err(e) if cancel2.load(Ordering::SeqCst) => {
                    // Cancelled teardown: stay quiet.
                    let _ = e;
                    Ok(())
                }
                Err(e) => tx.send(Progress::Failed(e)),
            };
        });

        Self { rx, child, cancel, handle: Some(handle) }
    }
}

impl Drop for WhisperXJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The actual work, returning the produced VTT path or a short error.
fn run_job(
    video: &Path,
    target_vtt: &Path,
    cfg: &WhisperXConfig,
    tx: &Sender<Progress>,
    child: &Arc<Mutex<Option<Child>>>,
    cancel: &Arc<AtomicBool>,
) -> Result<PathBuf, String> {
    let conda = conda_exe()
        .ok_or("conda not found — install Anaconda/Miniconda or set CONDA_EXE")?;

    // Ensure the env exists and has the `whisperx` CLI (first-run setup).
    if !whisperx_ready(&conda, &cfg.env) {
        if !env_exists(&conda, &cfg.env) {
            let _ = tx.send(Progress::Status(format!(
                "creating conda env '{}' (python=3.11)…",
                cfg.env
            )));
            let mut cmd = Command::new(&conda);
            cmd.args(["create", "-y", "-n", &cfg.env, "python=3.11"]);
            run_cmd(cmd, child, cancel, tx, false)?;
        }
        let _ = tx.send(Progress::Status(
            "installing WhisperX — downloads several GB, please wait…".into(),
        ));
        let mut cmd = Command::new(&conda);
        cmd.args(["run", "--no-capture-output", "-n", &cfg.env, "pip", "install", "whisperx"]);
        run_cmd(cmd, child, cancel, tx, false)?;
        if !whisperx_ready(&conda, &cfg.env) {
            return Err("WhisperX install did not produce a working `whisperx` command".into());
        }
    }

    let (device, compute) = resolve_device(cfg.device.as_deref());

    let out_dir = std::env::temp_dir().join(format!("editty-whisperx-{}", std::process::id()));
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("temp dir: {e}"))?;

    let lang_note = cfg.language.as_deref().unwrap_or("auto");
    let _ = tx.send(Progress::Status(format!(
        "transcribing with {} on {device} (lang={lang_note}) — first run also downloads the model…",
        cfg.model
    )));

    let mut cmd = Command::new(&conda);
    cmd.args(["run", "--no-capture-output", "-n", &cfg.env, "whisperx"])
        .arg(video)
        .args(["--model", &cfg.model])
        .args(["--device", &device])
        .args(["--compute_type", &compute])
        .args(["--output_format", "vtt"])
        .arg("--output_dir")
        .arg(&out_dir)
        .args(["--print_progress", "True"]);
    if let Some(lang) = &cfg.language {
        cmd.args(["--language", lang]);
    }
    run_cmd(cmd, child, cancel, tx, true)?;

    // WhisperX names the output after the input file's stem.
    let stem = video
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("bad video filename")?;
    let produced = out_dir.join(format!("{stem}.vtt"));
    if !produced.exists() {
        return Err("WhisperX finished but wrote no .vtt".into());
    }

    // Preserve any existing target, then move the result into place.
    let _ = crate::util::backup_once(target_vtt);
    std::fs::copy(&produced, target_vtt)
        .map_err(|e| format!("writing {}: {e}", target_vtt.display()))?;
    let _ = std::fs::remove_dir_all(&out_dir);

    Ok(target_vtt.to_path_buf())
}

/// `(device, compute_type)` for WhisperX. `cpu` uses int8; GPU uses float16.
fn resolve_device(requested: Option<&str>) -> (String, String) {
    match requested {
        Some("cpu") => ("cpu".into(), "int8".into()),
        Some(dev) => (dev.to_string(), "float16".into()),
        None if has_nvidia_gpu() => ("cuda".into(), "float16".into()),
        None => ("cpu".into(), "int8".into()),
    }
}

fn has_nvidia_gpu() -> bool {
    Command::new("nvidia-smi")
        .arg("-L")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run `cmd`, storing the child so it can be killed on cancel. Streams stderr;
/// when `stream` is set, each line is forwarded as a status update. Returns the
/// last few stderr lines as the error on non-zero exit.
fn run_cmd(
    mut cmd: Command,
    child_slot: &Arc<Mutex<Option<Child>>>,
    cancel: &Arc<AtomicBool>,
    tx: &Sender<Progress>,
    stream: bool,
) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut spawned = cmd.spawn().map_err(|e| format!("failed to start: {e}"))?;
    let stderr = spawned.stderr.take();
    *child_slot.lock().unwrap() = Some(spawned);

    let mut tail: VecDeque<String> = VecDeque::with_capacity(16);
    if let Some(err) = stderr {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            // tqdm rewrites a line with '\r'; keep only the latest state.
            let line = line.rsplit('\r').next().unwrap_or("").trim().to_string();
            if line.is_empty() {
                continue;
            }
            if stream {
                let _ = tx.send(Progress::Status(tail_chars(&line, 72)));
            }
            tail.push_back(line);
            if tail.len() > 12 {
                tail.pop_front();
            }
        }
    }

    let mut taken = child_slot.lock().unwrap().take();
    let status = taken.as_mut().and_then(|c| c.wait().ok());
    if cancel.load(Ordering::SeqCst) {
        return Err("cancelled".into());
    }
    match status {
        Some(s) if s.success() => Ok(()),
        _ => {
            let msg = tail.into_iter().collect::<Vec<_>>().join(" | ");
            Err(if msg.is_empty() { "command failed".into() } else { tail_chars(&msg, 200) })
        }
    }
}

/// Is the `whisperx` CLI runnable in `env`?
fn whisperx_ready(conda: &Path, env: &str) -> bool {
    Command::new(conda)
        .args(["run", "--no-capture-output", "-n", env, "whisperx", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Does a conda env named `env` exist?
fn env_exists(conda: &Path, env: &str) -> bool {
    let Ok(out) = Command::new(conda).args(["env", "list"]).output() else {
        return false;
    };
    String::from_utf8_lossy(&out.stdout).lines().any(|line| {
        let line = line.trim();
        !line.starts_with('#') && line.split_whitespace().next() == Some(env)
    })
}

/// Locate the `conda` executable: `CONDA_EXE`, then `PATH`, then common dirs.
fn conda_exe() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CONDA_EXE").map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let cand = dir.join("conda");
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for sub in ["anaconda3", "miniconda3", "miniforge3", "mambaforge"] {
            let cand = home.join(sub).join("bin").join("conda");
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// Keep the last `n` characters of `s`, prefixing an ellipsis if truncated.
/// Operates on chars so it never splits a multi-byte sequence.
fn tail_chars(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        s.to_string()
    } else {
        format!("…{}", chars[chars.len() - n..].iter().collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_resolution() {
        assert_eq!(resolve_device(Some("cpu")), ("cpu".into(), "int8".into()));
        assert_eq!(resolve_device(Some("cuda")), ("cuda".into(), "float16".into()));
        // auto (None) depends on the host GPU; just assert it picks a valid pair.
        let (d, c) = resolve_device(None);
        assert!(d == "cuda" || d == "cpu");
        assert!(c == "float16" || c == "int8");
    }

    #[test]
    fn tail_chars_truncates_on_char_boundaries() {
        assert_eq!(tail_chars("hello", 10), "hello");
        assert_eq!(tail_chars("hello", 3), "…llo");
        // multi-byte: must not panic and must keep whole chars
        let s = "あいうえお";
        assert_eq!(tail_chars(s, 2), "…えお");
    }
}
