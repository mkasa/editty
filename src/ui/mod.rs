pub mod chapters;
pub mod cuelist;
pub mod help;
pub mod statusbar;
pub mod timeline;
pub mod video;

use std::ops::Range;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::app::App;
use crate::textinput::TextInput;
use crate::util::display_width;

/// The stacked regions of the UI. The list row is split side by side into the
/// subtitle cue list and the chapter list.
pub struct Areas {
    pub video: Rect,
    pub timeline: Rect,
    pub cues: Rect,
    pub chapters: Rect,
    pub status: Rect,
}

/// Pure layout: derived only from the terminal size, so the app can compute the
/// video rect (for frame sizing) without going through a draw.
pub fn layout(area: Rect) -> Areas {
    let chunks = Layout::vertical([
        Constraint::Min(5),    // video pane (takes remaining space)
        Constraint::Length(3), // timeline
        Constraint::Length(8), // cue / chapter lists
        Constraint::Length(1), // status bar
    ])
    .split(area);
    let lists = Layout::horizontal([
        Constraint::Percentage(60), // subtitle cues
        Constraint::Percentage(40), // chapters
    ])
    .split(chunks[2]);
    Areas {
        video: chunks[0],
        timeline: chunks[1],
        cues: lists[0],
        chapters: lists[1],
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

/// A `width`×`height` rectangle centered within `area` (clamped to fit).
pub fn centered(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

/// Spans for `text[range]`, with the cell at `cursor` drawn inverted when the
/// cursor falls in that slice. A cursor sitting just past the last character
/// gets a blank cell to live in, so it is visible at end of line.
pub fn cursor_spans(
    text: &str,
    range: Range<usize>,
    cursor: Option<usize>,
    style: Style,
) -> Vec<Span<'static>> {
    let Some(at) = cursor.filter(|c| range.contains(c) || *c == range.end) else {
        return vec![Span::styled(text[range].to_string(), style)];
    };
    let inverted = style.add_modifier(Modifier::REVERSED);
    let mut spans = vec![Span::styled(text[range.start..at].to_string(), style)];
    match text[at..range.end].chars().next() {
        Some(c) => {
            spans.push(Span::styled(c.to_string(), inverted));
            spans.push(Span::styled(
                text[at + c.len_utf8()..range.end].to_string(),
                style,
            ));
        }
        None => spans.push(Span::styled(" ", inverted)),
    }
    spans
}

/// Render `input` as the rows of a list: `prefix` labels the first row, and the
/// text wraps onto as many further rows as it needs, indented to line up under
/// itself. Returns the rows and which one holds the cursor.
pub fn edit_rows(
    prefix: &str,
    input: &TextInput,
    width: usize,
    style: Style,
) -> (Vec<Line<'static>>, usize) {
    let indent = display_width(prefix);
    let wrapped = input.wrap(width.saturating_sub(indent));
    let text = input.text();
    let rows = wrapped
        .lines
        .iter()
        .enumerate()
        .map(|(i, range)| {
            let head = if i == 0 {
                prefix.to_string()
            } else {
                " ".repeat(indent)
            };
            let mut spans = vec![Span::styled(head, style)];
            let cursor = (i == wrapped.cursor_line).then(|| input.cursor());
            spans.extend(cursor_spans(text, range.clone(), cursor, style));
            Line::from(spans)
        })
        .collect();
    (rows, wrapped.cursor_line)
}

/// Window a list of `count` rows into a pane `height` lines tall, keeping the
/// selection visible. Rows are one line each except the one being edited, which
/// is `edit`'s block of already-rendered lines plus the index of its cursor line
/// — so the list scrolls by *line*, and a long cue can be edited to its end.
pub fn windowed_rows(
    count: usize,
    selected: usize,
    edit: Option<(&[Line<'static>], usize)>,
    height: usize,
    row: impl Fn(usize) -> Line<'static>,
) -> Vec<Line<'static>> {
    if count == 0 || height == 0 {
        return Vec::new();
    }
    let selected = selected.min(count - 1);
    let block_h = edit.map(|(rows, _)| rows.len().max(1)).unwrap_or(1);
    let total = count - 1 + block_h;
    // Rows above the selection are one line each, so it begins at its own index.
    let cursor_line = selected + edit.map(|(_, c)| c).unwrap_or(0);
    let max_start = total.saturating_sub(height);
    let start = match edit {
        // Pin the edited row's first line to the top, then follow the cursor
        // down as the text wraps past the bottom of the pane.
        Some(_) => {
            let pinned = selected.min(max_start);
            let scrolled = (cursor_line + 1).saturating_sub(height);
            pinned.max(scrolled)
        }
        None => selected
            .saturating_sub(height.saturating_sub(1) / 2)
            .min(max_start),
    };

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(height);
    let mut line = 0;
    for i in 0..count {
        if lines.len() >= height {
            break;
        }
        match edit {
            Some((block, _)) if i == selected => {
                for (k, l) in block.iter().enumerate() {
                    if line + k >= start && lines.len() < height {
                        lines.push(l.clone());
                    }
                }
                line += block.len();
            }
            _ => {
                if line >= start {
                    lines.push(row(i));
                }
                line += 1;
            }
        }
    }
    lines
}

pub fn render(f: &mut Frame, app: &App) {
    let areas = layout(f.area());
    video::render(f, app, areas.video);
    timeline::render(f, app, areas.timeline);
    cuelist::render(f, app, areas.cues);
    chapters::render(f, app, areas.chapters);
    statusbar::render(f, app, areas.status);

    if app.show_help {
        help::render(f, f.area());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn row(i: usize) -> Line<'static> {
        Line::from(format!("row{i}"))
    }

    fn block(n: usize) -> Vec<Line<'static>> {
        (0..n).map(|k| Line::from(format!("edit{k}"))).collect()
    }

    #[test]
    fn a_list_shorter_than_the_pane_is_shown_whole() {
        let lines = windowed_rows(3, 0, None, 8, row);
        assert_eq!(texts(&lines), ["row0", "row1", "row2"]);
    }

    #[test]
    fn the_selection_stays_visible_while_scrolling() {
        let lines = windowed_rows(20, 15, None, 5, row);
        assert_eq!(texts(&lines), ["row13", "row14", "row15", "row16", "row17"]);
        // At the end of the list the window stops rather than running past it.
        let lines = windowed_rows(20, 19, None, 5, row);
        assert_eq!(texts(&lines), ["row15", "row16", "row17", "row18", "row19"]);
    }

    #[test]
    fn the_edited_row_grows_and_takes_the_top() {
        let b = block(3);
        let lines = windowed_rows(10, 5, Some((&b, 0)), 4, row);
        assert_eq!(texts(&lines), ["edit0", "edit1", "edit2", "row6"]);
    }

    #[test]
    fn text_taller_than_the_pane_scrolls_to_the_cursor() {
        let b = block(6);
        // Cursor on the block's last line: it must be on screen, so the window
        // has scrolled past the block's first lines.
        let lines = windowed_rows(10, 2, Some((&b, 5)), 4, row);
        assert_eq!(texts(&lines), ["edit2", "edit3", "edit4", "edit5"]);
        // Cursor back at the top: the block's first line leads again.
        let lines = windowed_rows(10, 2, Some((&b, 0)), 4, row);
        assert_eq!(texts(&lines), ["edit0", "edit1", "edit2", "edit3"]);
    }

    #[test]
    fn the_cursor_cell_is_the_inverted_one() {
        let text = "あいう";
        let at = "あ".len(); // between the first and second kana
        let spans = cursor_spans(text, 0..text.len(), Some(at), Style::default());
        let inverted: Vec<&str> = spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(inverted, ["い"]);
        // Past the last character the cursor gets a blank cell of its own.
        let spans = cursor_spans(text, 0..text.len(), Some(text.len()), Style::default());
        assert_eq!(spans.last().unwrap().content.as_ref(), " ");
    }

    #[test]
    fn edit_rows_indent_under_the_prefix() {
        let input = TextInput::with_text("あいうえおか");
        // 8 columns of pane, 2 taken by the prefix: two kana per row.
        let (rows, cursor) = edit_rows("> ", &input, 8, Style::default());
        let shown = texts(&rows);
        assert_eq!(shown, ["> あいう", "  えおか", "   "]);
        assert_eq!(cursor, 2, "the trailing cursor sits on its own row");
    }
}
