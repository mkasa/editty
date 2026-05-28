pub mod cuelist;
pub mod statusbar;
pub mod timeline;
pub mod video;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::App;

/// The four stacked regions of the UI.
pub struct Areas {
    pub video: Rect,
    pub timeline: Rect,
    pub cues: Rect,
    pub status: Rect,
}

/// Pure layout: derived only from the terminal size, so the app can compute the
/// video rect (for frame sizing) without going through a draw.
pub fn layout(area: Rect) -> Areas {
    let chunks = Layout::vertical([
        Constraint::Min(5),    // video pane (takes remaining space)
        Constraint::Length(3), // timeline
        Constraint::Length(8), // cue list
        Constraint::Length(1), // status bar
    ])
    .split(area);
    Areas {
        video: chunks[0],
        timeline: chunks[1],
        cues: chunks[2],
        status: chunks[3],
    }
}

/// Inset by a 1-cell border, saturating so it never underflows.
pub fn inner(rect: Rect) -> Rect {
    Rect {
        x: rect.x.saturating_add(1),
        y: rect.y.saturating_add(1),
        width: rect.width.saturating_sub(2),
        height: rect.height.saturating_sub(2),
    }
}

pub fn render(f: &mut Frame, app: &App) {
    let areas = layout(f.area());
    video::render(f, app, areas.video);
    timeline::render(f, app, areas.timeline);
    cuelist::render(f, app, areas.cues);
    statusbar::render(f, app, areas.status);
}
