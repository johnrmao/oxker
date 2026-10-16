use parking_lot::Mutex;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use crate::{
    config::AppColors,
    ui::{FrameData, GuiState, selection::SelectPane},
};

/// A meta line is one the wrapper injects: the `[claude HH:MM:SS name] …`
/// command header and `… exit N` footer. Everything else is command output.
fn is_meta(line: &str) -> bool {
    line.starts_with("[claude ")
}

/// Word-wrap a single logical line to `width` columns, hard-breaking any word
/// that is itself longer than the width. Returns the physical rows it occupies.
fn wrap_line(line: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![line.to_owned()];
    }
    let mut rows: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in line.split(' ') {
        // A word longer than the pane is split into width-sized pieces first.
        let pieces: Vec<String> = if word.chars().count() > width {
            let chars: Vec<char> = word.chars().collect();
            chars.chunks(width).map(|c| c.iter().collect()).collect()
        } else {
            vec![word.to_owned()]
        };
        for piece in pieces {
            let piece_w = piece.chars().count();
            if current.is_empty() {
                current = piece;
            } else if current.chars().count() + 1 + piece_w <= width {
                current.push(' ');
                current.push_str(&piece);
            } else {
                rows.push(std::mem::take(&mut current));
                current = piece;
            }
        }
    }
    rows.push(current);
    rows
}

/// Draw the claude-activity panel: the tail of the selected container's activity
/// log (mirrored `docker exec` command + output), shown beside the normal logs.
/// Long lines are word-wrapped and the view is anchored to the bottom so the
/// newest output is always visible and nothing is cut off horizontally.
pub fn draw(
    area: Rect,
    colors: AppColors,
    f: &mut Frame,
    fd: &FrameData,
    gui_state: &std::sync::Arc<Mutex<GuiState>>,
) {
    let title = if fd.activity_title.is_empty() {
        " claude-activity ".to_owned()
    } else {
        format!(" claude ▸ {} ", fd.activity_title)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(colors.borders.unselected))
        .title(Line::from(title).left_aligned())
        .bg(colors.logs.background);

    // Inner drawing area (inside the border).
    let inner_w = usize::from(area.width.saturating_sub(2)).max(1);
    let inner_h = usize::from(area.height.saturating_sub(2)).max(1);

    // Colour the wrapper's own command header/footer lines distinctly from the
    // command output, so it's clear where each command starts and ends.
    let meta_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let output_style = Style::default().fg(colors.logs.text);

    let text = if fd.activity_lines.is_empty() {
        Text::from("waiting for activity…")
    } else {
        // Wrap each logical line, tagging every physical row with its source
        // line's kind, then keep only the last `inner_h` rows (newest at the
        // bottom). Wrapped continuation rows keep the colour of their line.
        let mut physical: Vec<(String, bool)> = Vec::new();
        for line in &fd.activity_lines {
            let meta = is_meta(line);
            for row in wrap_line(line, inner_w) {
                physical.push((row, meta));
            }
        }
        let start = physical.len().saturating_sub(inner_h);
        let lines: Vec<Line> = physical[start..]
            .iter()
            .map(|(row, meta)| {
                let style = if *meta { meta_style } else { output_style };
                Line::from(Span::styled(row.clone(), style))
            })
            .collect();
        Text::from(lines)
    };

    // Text is already wrapped to the inner width, so render it as-is.
    let paragraph = Paragraph::new(text).block(block);
    f.render_widget(paragraph, area);

    super::draw_selection(area, f, gui_state, SelectPane::Activity);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::wrap_line;

    #[test]
    /// Short lines are returned unchanged as a single row.
    fn test_wrap_line_short() {
        assert_eq!(wrap_line("hello world", 40), vec!["hello world"]);
    }

    #[test]
    /// Wrapping happens at word boundaries, never mid-word when it fits.
    fn test_wrap_line_word_boundary() {
        let rows = wrap_line("one two three four", 7);
        assert_eq!(rows, vec!["one two", "three", "four"]);
        assert!(rows.iter().all(|r| r.chars().count() <= 7));
    }

    #[test]
    /// A single word longer than the width is hard-broken into width-sized rows.
    fn test_wrap_line_long_word() {
        let rows = wrap_line("abcdefghij", 4);
        assert_eq!(rows, vec!["abcd", "efgh", "ij"]);
    }
}
