use std::{
    collections::VecDeque,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
};

/// Maximum number of activity lines kept in memory per selected container.
const MAX_LINES: usize = 1000;

/// Tails a per-container "activity" log so oxker can show it in its own pane.
///
/// Convention (no config needed): for the currently selected container `<name>`,
/// the file `$HOME/.config/claude-activity/logs/<name>.log` is followed. The pane
/// is shown only when that file exists, so opting a container in is simply a
/// matter of a log existing for it. The file is produced on the host by the
/// transparent `docker` wrapper that mirrors `docker exec` command + output.
#[derive(Debug, Clone)]
pub struct ActivityState {
    dir: PathBuf,
    /// Container name currently being tailed.
    current: Option<String>,
    /// Byte offset already consumed from the file.
    offset: u64,
    /// Bytes read without a trailing newline yet, held until the line completes.
    partial: String,
    /// Completed lines, oldest first, capped at `MAX_LINES`.
    pub lines: VecDeque<String>,
    /// Whether a log file exists for the current container.
    pub available: bool,
}

impl Default for ActivityState {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivityState {
    pub fn new() -> Self {
        let dir = directories::BaseDirs::new().map_or_else(
            || PathBuf::from("/tmp/claude-activity/logs"),
            |b| b.home_dir().join(".config/claude-activity/logs"),
        );
        Self {
            dir,
            current: None,
            offset: 0,
            partial: String::new(),
            lines: VecDeque::new(),
            available: false,
        }
    }

    #[cfg(test)]
    pub fn with_dir(dir: PathBuf) -> Self {
        let mut s = Self::new();
        s.dir = dir;
        s
    }

    fn reset(&mut self) {
        self.offset = 0;
        self.partial.clear();
        self.lines.clear();
        self.available = false;
    }

    fn path_for(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.log"))
    }

    /// Follow the activity file for `selected`, appending any new lines.
    /// Returns true if the buffer changed, so the caller can request a redraw.
    pub fn poll(&mut self, selected: Option<&str>) -> bool {
        // Selection changed (or cleared): start the feed over.
        if selected != self.current.as_deref() {
            self.current = selected.map(ToString::to_string);
            self.reset();
        }

        let Some(name) = self.current.clone() else {
            return false;
        };
        let path = self.path_for(&name);

        let Ok(meta) = std::fs::metadata(&path) else {
            let changed = self.available;
            if changed {
                self.reset();
            }
            return changed;
        };
        self.available = true;

        let len = meta.len();
        // File was truncated or rotated: restart from the top.
        if len < self.offset {
            self.offset = 0;
            self.partial.clear();
            self.lines.clear();
        }
        if len == self.offset {
            return false;
        }

        let Ok(mut file) = std::fs::File::open(&path) else {
            return false;
        };
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return false;
        }
        let mut buf = String::new();
        if file
            .take(len - self.offset)
            .read_to_string(&mut buf)
            .is_err()
        {
            // Non-UTF8 or read error: skip past it so we don't spin on it.
            self.offset = len;
            return false;
        }
        self.offset = len;

        let mut data = std::mem::take(&mut self.partial);
        data.push_str(&buf);
        let ends_with_nl = data.ends_with('\n');
        let mut parts: Vec<&str> = data.split('\n').collect();
        if ends_with_nl {
            parts.pop(); // trailing empty element after the final newline
        } else {
            self.partial = parts.pop().unwrap_or_default().to_owned();
        }
        let changed = !parts.is_empty();
        for line in parts {
            self.lines.push_back(line.to_owned());
            while self.lines.len() > MAX_LINES {
                self.lines.pop_front();
            }
        }
        changed
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::ActivityState;
    use std::io::Write;

    /// A fresh selection with no file: nothing available, no lines.
    #[test]
    fn test_activity_absent_file() {
        let dir = std::env::temp_dir().join(format!("oxker_act_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut act = ActivityState::with_dir(dir);
        assert!(!act.poll(Some("ghost")));
        assert!(!act.available);
        assert!(act.lines.is_empty());
    }

    /// Appended whole lines are picked up; a trailing partial waits for its newline.
    #[test]
    fn test_activity_tails_appended_lines() {
        let dir = std::env::temp_dir().join(format!("oxker_act_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kali.log");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "line one").unwrap();
        write!(f, "partial").unwrap();
        f.flush().unwrap();

        let mut act = ActivityState::with_dir(dir);
        assert!(act.poll(Some("kali")));
        assert!(act.available);
        assert_eq!(act.lines.len(), 1);
        assert_eq!(act.lines[0], "line one");

        // Completing the partial line and adding another yields two more lines.
        writeln!(f, " now complete").unwrap();
        writeln!(f, "line three").unwrap();
        f.flush().unwrap();
        assert!(act.poll(Some("kali")));
        assert_eq!(act.lines.len(), 3);
        assert_eq!(act.lines[1], "partial now complete");
        assert_eq!(act.lines[2], "line three");
    }

    /// Changing the selected container resets the buffer.
    #[test]
    fn test_activity_reset_on_selection_change() {
        let dir = std::env::temp_dir().join(format!("oxker_act_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("kali.log"), "a\nb\n").unwrap();
        let mut act = ActivityState::with_dir(dir);
        act.poll(Some("kali"));
        assert_eq!(act.lines.len(), 2);
        act.poll(Some("other"));
        assert!(act.lines.is_empty());
        assert!(!act.available);
    }
}
