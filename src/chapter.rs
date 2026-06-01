//! Named chapter markers, stored alongside the video as `<stem>.chapter.txt`.
//!
//! Chapters are *points*: each has a start time and a title and runs until the
//! next chapter. The on-disk format is the YouTube-style one line per chapter,
//! `M:SS Title` (or `H:MM:SS Title` past an hour) — human-editable and widely
//! understood. Chapters are kept sorted by time.

use std::path::Path;

use anyhow::{Context, Result};

use crate::util::fmt_clock;

#[derive(Clone, Debug, PartialEq)]
pub struct Chapter {
    pub time: f64,
    pub title: String,
}

#[derive(Default)]
pub struct Chapters {
    chapters: Vec<Chapter>,
}

/// Parse a YouTube-style timestamp token (`SS`, `M:SS`, or `H:MM:SS`, each part
/// optionally fractional) into seconds. Returns `None` if it isn't a timestamp.
fn parse_clock(tok: &str) -> Option<f64> {
    let parts: Vec<&str> = tok.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    let mut total = 0.0f64;
    for p in &parts {
        let v: f64 = p.parse().ok()?;
        if v < 0.0 || !v.is_finite() {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

impl Chapters {
    pub fn empty() -> Self {
        Self { chapters: Vec::new() }
    }

    /// Load chapters from a `.chapter.txt` file. Blank lines and lines that
    /// don't begin with a timestamp are ignored (so a stray header or comment
    /// won't fail the load).
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let mut chapters = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            let mut it = line.splitn(2, char::is_whitespace);
            let ts = it.next().unwrap_or("");
            let Some(time) = parse_clock(ts) else { continue };
            let title = it.next().unwrap_or("").trim().to_string();
            chapters.push(Chapter { time, title });
        }
        chapters.sort_by(|a, b| a.time.total_cmp(&b.time));
        Ok(Self { chapters })
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let mut out = String::new();
        for c in &self.chapters {
            out.push_str(&fmt_clock(c.time));
            out.push(' ');
            out.push_str(&c.title);
            out.push('\n');
        }
        std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))
    }

    pub fn len(&self) -> usize {
        self.chapters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chapters.is_empty()
    }

    /// The chapters in order — for rendering.
    pub fn rows(&self) -> &[Chapter] {
        &self.chapters
    }

    pub fn time(&self, n: usize) -> Option<f64> {
        self.chapters.get(n).map(|c| c.time)
    }

    /// The chapter active at `playhead` (the last one whose time <= playhead).
    pub fn active(&self, playhead: f64) -> Option<usize> {
        self.chapters.iter().rposition(|c| c.time <= playhead)
    }

    pub fn set_title(&mut self, n: usize, title: &str) {
        if let Some(c) = self.chapters.get_mut(n) {
            c.title = title.to_string();
        }
    }

    /// Insert a chapter, keeping the list sorted by time; returns its index.
    pub fn add(&mut self, time: f64, title: &str) -> usize {
        let idx = self.chapters.partition_point(|c| c.time <= time);
        self.chapters.insert(
            idx,
            Chapter { time: time.max(0.0), title: title.to_string() },
        );
        idx
    }

    pub fn delete(&mut self, n: usize) {
        if n < self.chapters.len() {
            self.chapters.remove(n);
        }
    }

    /// Produce the chapters for a clip spanning `[start, end)` (seconds),
    /// rebased so the clip begins at 0. The chapter active at `start` is carried
    /// over as the clip's `0:00` chapter; later chapters inside the range keep
    /// their (rebased) times.
    pub fn cut(&self, start: f64, end: f64) -> Chapters {
        let mut out: Vec<Chapter> = Vec::new();
        if let Some(i) = self.active(start) {
            out.push(Chapter { time: 0.0, title: self.chapters[i].title.clone() });
        }
        for c in &self.chapters {
            if c.time > start && c.time < end {
                out.push(Chapter { time: c.time - start, title: c.title.clone() });
            }
        }
        Chapters { chapters: out }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_str(s: &str) -> Chapters {
        let dir = std::env::temp_dir().join(format!("editty_ch_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.chapter.txt");
        std::fs::write(&p, s).unwrap();
        let c = Chapters::load(&p).unwrap();
        std::fs::remove_file(&p).ok();
        c
    }

    #[test]
    fn parses_youtube_format_and_sorts() {
        let c = from_str("1:23 Topic A\n0:00 Intro\n1:02:10 Late\n\n# ignored\n");
        assert_eq!(c.len(), 3);
        assert_eq!(c.rows()[0].title, "Intro");
        assert!((c.rows()[0].time - 0.0).abs() < 0.01);
        assert!((c.rows()[1].time - 83.0).abs() < 0.01);
        assert!((c.rows()[2].time - 3730.0).abs() < 0.01);
    }

    #[test]
    fn save_round_trips() {
        let mut c = Chapters::empty();
        c.add(0.0, "Intro");
        c.add(83.0, "Topic A");
        let dir = std::env::temp_dir().join(format!("editty_chrt_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.chapter.txt");
        c.save(&p).unwrap();
        let back = Chapters::load(&p).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back.rows()[1].title, "Topic A");
        assert!((back.rows()[1].time - 83.0).abs() < 0.01);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn add_keeps_order_and_returns_index() {
        let mut c = Chapters::empty();
        c.add(0.0, "a");
        c.add(20.0, "c");
        let i = c.add(10.0, "b");
        assert_eq!(i, 1);
        assert_eq!(c.rows()[1].title, "b");
    }

    #[test]
    fn active_finds_covering_chapter() {
        let mut c = Chapters::empty();
        c.add(0.0, "a");
        c.add(10.0, "b");
        c.add(20.0, "c");
        assert_eq!(c.active(5.0), Some(0));
        assert_eq!(c.active(10.0), Some(1));
        assert_eq!(c.active(25.0), Some(2));
        // before the first chapter
        let mut d = Chapters::empty();
        d.add(5.0, "x");
        assert_eq!(d.active(2.0), None);
    }

    #[test]
    fn cut_carries_active_as_zero_and_rebases() {
        let mut c = Chapters::empty();
        c.add(0.0, "Intro");
        c.add(30.0, "Body");
        c.add(90.0, "Outro");
        // clip [40, 100): "Body" is active at 40 -> 0:00, "Outro" -> 50.
        let clip = c.cut(40.0, 100.0);
        assert_eq!(clip.len(), 2);
        assert_eq!(clip.rows()[0].title, "Body");
        assert!((clip.rows()[0].time - 0.0).abs() < 0.01);
        assert_eq!(clip.rows()[1].title, "Outro");
        assert!((clip.rows()[1].time - 50.0).abs() < 0.01);

        // a chapter exactly at start is not duplicated
        let clip2 = c.cut(30.0, 95.0);
        assert_eq!(clip2.len(), 2);
        assert_eq!(clip2.rows()[0].title, "Body");
        assert!((clip2.rows()[0].time - 0.0).abs() < 0.01);
    }
}
