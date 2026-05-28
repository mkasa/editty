//! Time helpers shared across the app. Internally we represent positions as
//! `f64` seconds; these convert to/from the string forms ffmpeg and WebVTT use.

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

/// A filesystem-safe stamp for generated output names, e.g. `00-01-23.456`.
pub fn fmt_filename_stamp(secs: f64) -> String {
    fmt_timestamp(secs).replace([':', '.'], "-")
}
