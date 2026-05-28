//! WebVTT document model over `subtp`.
//!
//! We keep the parsed `WebVtt` (all blocks) as the source of truth so NOTE,
//! STYLE and REGION blocks survive a load/edit/save round-trip. Editing
//! operations address cues by their ordinal position among the cue blocks.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use subtp::vtt::{VttBlock, VttCue, VttTimestamp, VttTimings, WebVtt};

pub struct VttDoc {
    doc: WebVtt,
}

fn secs_to_ts(secs: f64) -> VttTimestamp {
    VttTimestamp::from(Duration::from_secs_f64(secs.max(0.0)))
}

fn ts_to_secs(ts: &VttTimestamp) -> f64 {
    let d: Duration = (*ts).into();
    d.as_secs_f64()
}

impl VttDoc {
    /// An empty document (a `--vtt` target that doesn't exist yet).
    pub fn empty() -> Self {
        Self { doc: WebVtt::default() }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let doc = WebVtt::parse(&text)
            .map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))?;
        Ok(Self { doc })
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.doc.render())
            .with_context(|| format!("writing {}", path.display()))
    }

    /// Block indices that are cues, in document order.
    fn cue_indices(&self) -> Vec<usize> {
        self.doc
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(i, b)| matches!(b, VttBlock::Que(_)).then_some(i))
            .collect()
    }

    pub fn cue_count(&self) -> usize {
        self.doc
            .blocks
            .iter()
            .filter(|b| matches!(b, VttBlock::Que(_)))
            .count()
    }

    fn nth_cue(&self, n: usize) -> Option<&VttCue> {
        self.cue_indices()
            .get(n)
            .and_then(|&bi| match &self.doc.blocks[bi] {
                VttBlock::Que(c) => Some(c),
                _ => None,
            })
    }

    fn nth_cue_mut(&mut self, n: usize) -> Option<&mut VttCue> {
        let bi = *self.cue_indices().get(n)?;
        match &mut self.doc.blocks[bi] {
            VttBlock::Que(c) => Some(c),
            _ => None,
        }
    }

    /// `(start, end, text)` for each cue, in order — for rendering.
    pub fn cue_rows(&self) -> Vec<(f64, f64, String)> {
        self.cue_indices()
            .iter()
            .filter_map(|&bi| match &self.doc.blocks[bi] {
                VttBlock::Que(c) => Some((
                    ts_to_secs(&c.timings.start),
                    ts_to_secs(&c.timings.end),
                    c.payload.join("\n"),
                )),
                _ => None,
            })
            .collect()
    }

    pub fn cue_times(&self, n: usize) -> Option<(f64, f64)> {
        self.nth_cue(n)
            .map(|c| (ts_to_secs(&c.timings.start), ts_to_secs(&c.timings.end)))
    }

    pub fn cue_text(&self, n: usize) -> Option<String> {
        self.nth_cue(n).map(|c| c.payload.join("\n"))
    }

    /// The cue active at `playhead` (start <= t < end), if any.
    pub fn active_cue(&self, playhead: f64) -> Option<usize> {
        self.cue_rows()
            .iter()
            .position(|&(s, e, _)| playhead >= s && playhead < e)
    }

    pub fn set_cue_text(&mut self, n: usize, text: &str) {
        if let Some(c) = self.nth_cue_mut(n) {
            c.payload = text.lines().map(|l| l.to_string()).collect();
            if c.payload.is_empty() {
                c.payload = vec![String::new()];
            }
        }
    }

    /// Snap a cue's start to `secs`, keeping start <= end.
    pub fn set_cue_start(&mut self, n: usize, secs: f64) {
        if let Some(c) = self.nth_cue_mut(n) {
            let end = ts_to_secs(&c.timings.end);
            c.timings.start = secs_to_ts(secs.min(end));
        }
    }

    /// Snap a cue's end to `secs`, keeping end >= start.
    pub fn set_cue_end(&mut self, n: usize, secs: f64) {
        if let Some(c) = self.nth_cue_mut(n) {
            let start = ts_to_secs(&c.timings.start);
            c.timings.end = secs_to_ts(secs.max(start));
        }
    }

    /// Insert a new cue, returning its ordinal index. Cues are kept in start order.
    pub fn add_cue(&mut self, start: f64, end: f64, text: &str) -> usize {
        let cue = VttCue {
            identifier: None,
            timings: VttTimings {
                start: secs_to_ts(start),
                end: secs_to_ts(end.max(start)),
            },
            settings: None,
            payload: if text.is_empty() {
                vec![String::new()]
            } else {
                text.lines().map(|l| l.to_string()).collect()
            },
        };

        // Insert after the last cue whose start <= new start, preserving order.
        let indices = self.cue_indices();
        let mut insert_at = self.doc.blocks.len();
        let mut new_ordinal = indices.len();
        for (ord, &bi) in indices.iter().enumerate() {
            if let VttBlock::Que(c) = &self.doc.blocks[bi] {
                if ts_to_secs(&c.timings.start) > start {
                    insert_at = bi;
                    new_ordinal = ord;
                    break;
                }
            }
        }
        self.doc.blocks.insert(insert_at, VttBlock::Que(cue));
        new_ordinal
    }

    pub fn delete_cue(&mut self, n: usize) {
        if let Some(&bi) = self.cue_indices().get(n) {
            self.doc.blocks.remove(bi);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_notes_and_edits() {
        let path = Path::new("assets/sample.vtt");
        if !path.exists() {
            eprintln!("skipping: assets/sample.vtt missing");
            return;
        }
        let mut doc = VttDoc::load(path).expect("load");
        assert_eq!(doc.cue_count(), 3);

        // active cue lookup
        assert_eq!(doc.active_cue(1.0), Some(0));
        assert_eq!(doc.active_cue(4.0), Some(1));
        assert_eq!(doc.active_cue(2.7), None); // gap between cue 0 and 1

        // edit + snap
        doc.set_cue_text(1, "edited second caption");
        doc.set_cue_start(1, 2.75);
        assert_eq!(doc.cue_text(1).as_deref(), Some("edited second caption"));
        assert!((doc.cue_times(1).unwrap().0 - 2.75).abs() < 0.01);

        let out = std::env::temp_dir().join("editty_roundtrip.vtt");
        doc.save(&out).expect("save");
        let rendered = std::fs::read_to_string(&out).unwrap();
        assert!(rendered.contains("NOTE"), "NOTE block must survive round-trip");
        assert!(rendered.contains("edited second caption"));

        // reload and confirm persistence
        let doc2 = VttDoc::load(&out).expect("reload");
        assert_eq!(doc2.cue_count(), 3);
        assert_eq!(doc2.cue_text(1).as_deref(), Some("edited second caption"));
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn add_and_delete_keep_order() {
        let mut doc = VttDoc::load(Path::new("assets/sample.vtt")).expect("load");
        let n = doc.add_cue(2.6, 2.9, "inserted");
        assert_eq!(n, 1, "should slot between cue 0 and old cue 1");
        assert_eq!(doc.cue_count(), 4);
        assert_eq!(doc.cue_text(1).as_deref(), Some("inserted"));
        doc.delete_cue(1);
        assert_eq!(doc.cue_count(), 3);
    }
}
