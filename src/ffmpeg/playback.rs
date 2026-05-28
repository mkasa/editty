//! Best-effort playback: one ffmpeg process streams raw RGBA frames into a
//! reader thread, audio plays via a separate `ffplay` process, and a wall clock
//! (started when the first frame arrives) drives A/V sync. The UI polls for the
//! newest frame due at the current clock and drops any it's behind on.
//!
//! Sync is intentionally coarse — good enough to preview a cut, not a media
//! player. Audio is decoded by ffplay; we never parse its clock.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, TryRecvError, bounded};

use super::frame::Frame;

/// Cap the streamed frame rate; preview doesn't need more and it bounds work.
const MAX_FPS: f64 = 30.0;
/// Small buffer so ffmpeg can run a touch ahead without racing far in front.
const FRAME_QUEUE: usize = 3;

pub struct Playback {
    rx: Option<Receiver<Frame>>,
    reader: Option<JoinHandle<()>>,
    video: Child,
    audio: Option<Child>,
    /// Path + start position kept so audio can be spawned lazily with the clock.
    audio_path: Option<PathBuf>,
    start_pos: f64,
    duration: f64,
    fps: f64,
    /// Set when the first frame arrives, so frame 0 shows at elapsed 0 (no skip).
    clock_start: Option<Instant>,
    /// The next frame read but not yet due, with its presentation time.
    head: Option<(f64, Frame)>,
    frame_index: u64,
    eof: bool,
}

impl Playback {
    /// Start streaming from `start_pos`, scaled to `width`x`height`. `has_audio`
    /// gates whether an ffplay process is launched.
    pub fn start(
        path: &Path,
        has_audio: bool,
        start_pos: f64,
        duration: f64,
        source_fps: f64,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let fps = source_fps.clamp(1.0, MAX_FPS);
        let scale = format!("scale={width}:{height}:flags=fast_bilinear,fps={fps}");
        let mut video = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-ss"])
            .arg(format!("{start_pos:.3}"))
            .arg("-i")
            .arg(path)
            .args(["-an", "-vf", &scale, "-pix_fmt", "rgba", "-f", "rawvideo", "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to spawn ffmpeg for playback")?;

        let stdout = video.stdout.take().expect("piped stdout");
        let frame_size = (width as usize) * (height as usize) * 4;
        let (tx, rx) = bounded::<Frame>(FRAME_QUEUE);

        let reader = std::thread::spawn(move || {
            let mut stdout = stdout;
            loop {
                let mut buf = vec![0u8; frame_size];
                // read_exact errors at EOF / on a short final read — that ends playback.
                if stdout.read_exact(&mut buf).is_err() {
                    break;
                }
                let frame = Frame { width, height, rgba: buf };
                if tx.send(frame).is_err() {
                    break; // receiver dropped: we're being torn down.
                }
            }
        });

        Ok(Self {
            rx: Some(rx),
            reader: Some(reader),
            video,
            audio: None,
            audio_path: has_audio.then(|| path.to_path_buf()),
            start_pos,
            duration,
            fps,
            clock_start: None,
            head: None,
            frame_index: 0,
            eof: false,
        })
    }

    /// Current playback position in seconds (frozen at `start_pos` until the
    /// first frame arrives and the clock starts).
    pub fn clock(&self) -> f64 {
        match self.clock_start {
            Some(t) => self.start_pos + t.elapsed().as_secs_f64(),
            None => self.start_pos,
        }
    }

    /// Playback is done once the stream ended and we've shown everything, or we
    /// reached the end of the media.
    pub fn is_finished(&self) -> bool {
        (self.eof && self.head.is_none()) || self.clock() >= self.duration
    }

    /// Return the newest frame whose presentation time has arrived, dropping any
    /// older ones we fell behind on. `None` if nothing new is due yet.
    pub fn poll(&mut self) -> Option<Frame> {
        let clock = self.clock();
        let mut chosen = None;
        loop {
            self.ensure_head();
            let Some((pts, _)) = self.head.as_ref() else { break };
            if *pts <= clock {
                chosen = self.head.take().map(|(_, f)| f);
            } else {
                break;
            }
        }
        chosen
    }

    /// Pull the next frame from the channel into `head` (with its pts), starting
    /// the clock and audio on the very first frame.
    fn ensure_head(&mut self) {
        if self.head.is_some() || self.eof {
            return;
        }
        let Some(rx) = &self.rx else { return };
        match rx.try_recv() {
            Ok(frame) => {
                if self.clock_start.is_none() {
                    self.clock_start = Some(Instant::now());
                    self.spawn_audio();
                }
                let pts = self.start_pos + self.frame_index as f64 / self.fps;
                self.frame_index += 1;
                self.head = Some((pts, frame));
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.eof = true,
        }
    }

    fn spawn_audio(&mut self) {
        let Some(path) = &self.audio_path else { return };
        self.audio = Command::new("ffplay")
            .args(["-nodisp", "-vn", "-autoexit", "-loglevel", "quiet", "-ss"])
            .arg(format!("{:.3}", self.start_pos))
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;
    use std::time::Duration;

    #[test]
    fn streams_frames_advances_clock_and_finishes() {
        let input = Path::new("assets/sample.mp4");
        if !input.exists() {
            eprintln!("skipping: assets/sample.mp4 missing");
            return;
        }
        let (w, h) = (64u32, 36u32);
        // Start near the end (audio OFF so the test stays silent) so it finishes fast.
        let mut pb =
            Playback::start(input, false, 7.5, 8.0, 30.0, w, h).expect("start playback");
        assert!(pb.audio.is_none(), "audio must not spawn when has_audio=false");

        let mut frames = 0usize;
        let mut clock_after_first = None;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(frame) = pb.poll() {
                assert_eq!(frame.width, w);
                assert_eq!(frame.rgba.len(), (w * h * 4) as usize);
                frames += 1;
                if clock_after_first.is_none() {
                    clock_after_first = Some(pb.clock());
                }
            }
            if pb.is_finished() {
                break;
            }
            sleep(Duration::from_millis(10));
        }

        assert!(frames > 0, "should have presented at least one frame");
        assert!(pb.is_finished(), "playback should finish near the media end");
        assert!(
            pb.clock() > clock_after_first.unwrap(),
            "clock should advance during playback"
        );
        // Drop runs here — must not hang.
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        // Kill the video process (closing the pipe), then drop the receiver so a
        // reader blocked on a full channel unblocks, then join it.
        let _ = self.video.kill();
        drop(self.rx.take());
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let _ = self.video.wait();
        if let Some(mut audio) = self.audio.take() {
            let _ = audio.kill();
            let _ = audio.wait();
        }
    }
}
