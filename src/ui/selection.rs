//! Mouse text selection, scoped to a single pane.
//!
//! oxker grabs the mouse, so the terminal's own selection is unavailable - and
//! would be the wrong shape anyway, since it can only take full-width rows
//! spanning every pane side by side. Selecting inside oxker instead means the
//! selection is clipped to one pane's interior, so dragging across the shell
//! copies the shell and nothing else.
//!
//! The selection is linewise within that pane, the same shape a terminal gives
//! you: from the anchor to the right edge, whole rows in between, then the
//! final row up to the cursor. Text is read back out of the rendered buffer, so
//! what gets copied is exactly what is on screen, with no second guess at how
//! the pane wrapped its content.

use ratatui::{buffer::Buffer, layout::Rect};

/// The panes that can be selected in. Each is a full-width pane in the log
/// area, only ever one of which is drawn at a time in each half.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectPane {
    /// The interactive shell attached to a running container
    Shell,
    /// The claude-activity feed
    Activity,
    /// The container's docker logs
    Logs,
}

/// An in-progress or completed drag inside one pane. Coordinates are absolute
/// screen cells; they are clamped to the pane's interior when used, so a drag
/// that leaves the pane selects up to its edge rather than nothing at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub pane: SelectPane,
    anchor: (u16, u16),
    cursor: (u16, u16),
    /// Set when the button is released: the next draw of this pane reads the
    /// text out of the buffer and hands it to the clipboard.
    pub copy_pending: bool,
}

impl Selection {
    pub const fn new(pane: SelectPane, at: (u16, u16)) -> Self {
        Self {
            pane,
            anchor: at,
            cursor: at,
            copy_pending: false,
        }
    }

    pub const fn drag_to(&mut self, at: (u16, u16)) {
        self.cursor = at;
    }

    /// Has the drag covered more than the single cell it started on
    pub const fn is_empty(&self) -> bool {
        self.anchor.0 == self.cursor.0 && self.anchor.1 == self.cursor.1
    }

    /// The two ends in reading order, each clamped into `inner`
    fn ordered(&self, inner: Rect) -> ((u16, u16), (u16, u16)) {
        let clamp = |(x, y): (u16, u16)| {
            (
                x.clamp(inner.left(), inner.right().saturating_sub(1)),
                y.clamp(inner.top(), inner.bottom().saturating_sub(1)),
            )
        };
        let (a, b) = (clamp(self.anchor), clamp(self.cursor));
        // Compare by row first: a drag upwards still reads top to bottom
        if (a.1, a.0) <= (b.1, b.0) {
            (a, b)
        } else {
            (b, a)
        }
    }

    /// Is this absolute screen cell part of the selection
    pub fn contains(&self, x: u16, y: u16, inner: Rect) -> bool {
        if !inner.contains((x, y).into()) {
            return false;
        }
        let (start, end) = self.ordered(inner);
        if y < start.1 || y > end.1 {
            return false;
        }
        // On the first and last rows the selection starts/stops mid-row; every
        // row between them is taken in full
        let from = if y == start.1 { start.0 } else { inner.left() };
        let to = if y == end.1 {
            end.0
        } else {
            inner.right().saturating_sub(1)
        };
        (from..=to).contains(&x)
    }

    /// Read the selected text out of a rendered buffer. Trailing blanks are
    /// trimmed from each row, since a pane's rows are padded out to its width.
    pub fn extract(&self, buffer: &Buffer, inner: Rect) -> String {
        if inner.is_empty() {
            return String::new();
        }
        let (start, end) = self.ordered(inner);
        (start.1..=end.1)
            .map(|y| {
                let row = (inner.left()..inner.right())
                    .filter(|x| self.contains(*x, y, inner))
                    .map(|x| {
                        buffer
                            .cell((x, y))
                            .map_or(" ", ratatui::buffer::Cell::symbol)
                    })
                    .collect::<String>();
                row.trim_end().to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Wrap text in the OSC 52 escape that asks the terminal to put it on the
/// system clipboard. This works over ssh and inside multiplexers, where a
/// direct clipboard call from the process cannot reach the user's display.
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

/// Minimal standard base64, as OSC 52 requires its payload encoded
fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        let idx = [
            (n >> 18) & 0x3f,
            (n >> 12) & 0x3f,
            (n >> 6) & 0x3f,
            n & 0x3f,
        ];
        for (i, &v) in idx.iter().enumerate() {
            // The final chunk is padded out with '=' for the bytes it lacks
            if i <= chunk.len() {
                out.push(char::from(TABLE[usize::try_from(v).unwrap_or_default()]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{SelectPane, Selection, base64, osc52};
    use ratatui::{buffer::Buffer, layout::Rect, text::Line};

    /// A 10x4 buffer at origin, filled with four numbered rows
    fn test_buffer() -> (Buffer, Rect) {
        let area = Rect::new(0, 0, 10, 4);
        let mut buffer = Buffer::empty(area);
        for (y, text) in ["one line", "two", "three", "four"].iter().enumerate() {
            buffer.set_line(0, u16::try_from(y).unwrap(), &Line::from(*text), 10);
        }
        (buffer, area)
    }

    #[test]
    /// A drag within one row selects just that run of cells
    fn test_selection_single_row() {
        let (buffer, area) = test_buffer();
        let mut selection = Selection::new(SelectPane::Shell, (0, 0));
        selection.drag_to((2, 0));
        assert_eq!(selection.extract(&buffer, area), "one");
        assert!(selection.contains(1, 0, area));
        assert!(!selection.contains(3, 0, area));
        assert!(!selection.contains(0, 1, area));
    }

    #[test]
    /// Across rows the shape is linewise: to the right edge, then whole rows,
    /// then up to the cursor - and trailing padding is trimmed
    fn test_selection_multi_row() {
        let (buffer, area) = test_buffer();
        let mut selection = Selection::new(SelectPane::Shell, (4, 0));
        selection.drag_to((2, 2));
        assert_eq!(selection.extract(&buffer, area), "line\ntwo\nthr");
        // Middle rows are taken in full, including their left edge
        assert!(selection.contains(0, 1, area));
        assert!(selection.contains(9, 1, area));
        // The first row starts at the anchor, not the left edge
        assert!(!selection.contains(3, 0, area));
    }

    #[test]
    /// Dragging upwards selects the same text as dragging down
    fn test_selection_reversed() {
        let (buffer, area) = test_buffer();
        let mut down = Selection::new(SelectPane::Shell, (4, 0));
        down.drag_to((2, 2));
        let mut up = Selection::new(SelectPane::Shell, (2, 2));
        up.drag_to((4, 0));
        assert_eq!(down.extract(&buffer, area), up.extract(&buffer, area));
    }

    #[test]
    /// A drag that leaves the pane is clamped to its edge, not discarded
    fn test_selection_clamped_to_pane() {
        let (buffer, _) = test_buffer();
        // Pane interior is only rows 1-2 and the first four columns
        let inner = Rect::new(0, 1, 4, 2);
        let mut selection = Selection::new(SelectPane::Shell, (0, 1));
        selection.drag_to((40, 40));
        // "three" is clipped to the pane's four columns, and row 3 is outside it
        assert_eq!(selection.extract(&buffer, inner), "two\nthre");
        assert!(!selection.contains(0, 0, inner));
        assert!(!selection.contains(5, 1, inner));
    }

    #[test]
    /// A click with no drag selects nothing
    fn test_selection_empty() {
        let selection = Selection::new(SelectPane::Shell, (3, 1));
        assert!(selection.is_empty());
        let mut dragged = selection;
        dragged.drag_to((4, 1));
        assert!(!dragged.is_empty());
    }

    #[test]
    /// base64 matches the reference encoding, padding included
    fn test_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64("ls -la\n".as_bytes()), "bHMgLWxhCg==");
    }

    #[test]
    /// The clipboard escape is a well formed OSC 52 sequence
    fn test_osc52() {
        assert_eq!(osc52("foo"), "\x1b]52;c;Zm9v\x07");
    }
}
