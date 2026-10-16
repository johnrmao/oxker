//! An interactive shell, attached to the selected container, rendered inside
//! oxker's own log area rather than by taking over the whole terminal.
//!
//! The session is a Docker `exec` with a TTY allocated, driven over the same
//! bollard connection the rest of oxker uses. Output bytes are fed into a
//! `vt100` terminal emulator, and `tui-term` renders that emulator's screen as
//! a ratatui widget, so colour, cursor movement and full-screen programs behave
//! as they would in a real terminal.
//!
//! Two independent pieces of state drive the pane:
//! - `enabled`: whether the shell view replaces the logs view for a *running*
//!   container (toggled by the user, see `Keymap::shell_toggle`).
//! - `Status::Shell`: whether the keyboard is currently handed to the shell
//!   (entered with `Keymap::shell_attach`, left with `DETACH`).

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use bollard::{
    Docker,
    exec::{CreateExecOptions, ResizeExecOptions, StartExecOptions, StartExecResults},
};
use crossterm::event::{KeyCode, KeyModifiers};
use futures_util::StreamExt;
use parking_lot::Mutex;
use tokio::{
    io::AsyncWriteExt,
    sync::mpsc::{UnboundedSender, unbounded_channel},
};
use tokio_util::sync::CancellationToken;
use vt100::Parser;

use crate::{
    app_data::{AppData, ContainerId},
    docker_data::DockerMessage,
    ui::Rerender,
};

/// Key that hands the keyboard back to oxker whilst attached. `Ctrl + o` is
/// effectively unused by interactive shells, so it is safe to steal.
pub const DETACH: (KeyCode, KeyModifiers) = (KeyCode::Char('o'), KeyModifiers::CONTROL);

/// Printable description of `DETACH`, for titles and the help panel
pub const DETACH_TEXT: &str = "ctrl + o";

/// How often the manager task reconciles the session with the selected container
const POLL: Duration = Duration::from_millis(250);

/// A container has to stay selected this long before a shell is spawned into it,
/// so scrolling through the container list doesn't spray execs across the host.
const SPAWN_DEBOUNCE: Duration = Duration::from_millis(400);

/// Fallback emulator size, used until the pane has been drawn once
const DEFAULT_SIZE: (u16, u16) = (24, 80);

/// Start an interactive shell, preferring `bash` so the user gets a readline
/// prompt, and falling back to `sh` on minimal images. Note that the existence
/// check has to come first: a failed `exec` ends a non-interactive shell, so
/// `exec bash || exec sh` would never reach its fallback. Nothing may be
/// redirected away either, as bash writes its prompt to stderr.
const SHELL_CMD: &str = "if command -v bash >/dev/null 2>&1; then exec bash; else exec sh; fi";

/// Claimed by the emulator, so the shell emits colour & full-screen escapes
const TERM: &str = "TERM=xterm-256color";

/// A live `docker exec` with a TTY, plus the emulator its output is fed into
struct ShellSession {
    /// Container the exec belongs to, used to detect a selection change
    container: ContainerId,
    /// Docker's id for the exec, needed to resize its TTY
    exec_id: String,
    /// Terminal emulator state; shared with the task reading the exec's output
    parser: Arc<Mutex<Parser>>,
    /// Bytes written to the exec's stdin
    input_tx: UnboundedSender<Vec<u8>>,
    /// Emulator size (rows, cols) currently in effect
    size: (u16, u16),
    /// Cleared when the exec's output stream ends, i.e. the shell exited
    alive: Arc<AtomicBool>,
    /// Stops the output & input tasks when the session is dropped
    cancel: CancellationToken,
}

impl Drop for ShellSession {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Owns the shell session for the currently selected container
pub struct ShellManager {
    /// Whether a running container shows a shell instead of its logs
    enabled: bool,
    session: Option<ShellSession>,
    /// Size the pane last drew at, applied to the session by the manager task
    wanted_size: (u16, u16),
    /// When the container currently awaiting a shell became the selection
    pending_since: Option<Instant>,
}

impl std::fmt::Debug for ShellManager {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("ShellManager")
            .field("enabled", &self.enabled)
            .field("attached_to", &self.session.as_ref().map(|s| &s.container))
            .finish()
    }
}

impl Default for ShellManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellManager {
    pub const fn new() -> Self {
        Self {
            enabled: true,
            session: None,
            wanted_size: DEFAULT_SIZE,
            pending_since: None,
        }
    }

    /// Is the shell view (rather than the logs view) wanted for running containers
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Swap between the shell view and the logs view. Turning the view off also
    /// ends the session, so no shell is left running in the container.
    pub fn toggle(&mut self) -> bool {
        self.enabled = !self.enabled;
        if !self.enabled {
            self.clear();
        }
        self.enabled
    }

    /// The emulator to render, if a session has been established
    pub fn parser(&self) -> Option<Arc<Mutex<Parser>>> {
        self.session.as_ref().map(|s| Arc::clone(&s.parser))
    }

    /// Is there a session whose shell process is still running
    pub fn alive(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.alive.load(Ordering::SeqCst))
    }

    /// Record the size the pane was drawn at; the manager task resizes the
    /// emulator and the remote TTY to match on its next pass
    pub const fn set_size(&mut self, rows: u16, cols: u16) {
        if rows > 0 && cols > 0 {
            self.wanted_size = (rows, cols);
        }
    }

    /// The size the pane last drew at (rows, cols)
    #[cfg(test)]
    pub const fn wanted_size(&self) -> (u16, u16) {
        self.wanted_size
    }

    /// Write bytes to the shell's stdin, dropping them if there is no session
    pub fn send(&self, bytes: Vec<u8>) {
        if let Some(session) = self.session.as_ref() {
            session.input_tx.send(bytes).ok();
        }
    }

    /// Send pasted text to the shell. The paste markers are only added when the
    /// program on the far end has actually turned bracketed paste on, which is
    /// what a real terminal does - sending them regardless would leave the
    /// escapes sitting in the prompt of anything that hasn't.
    pub fn send_paste(&self, text: &str) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let bracketed = session.parser.lock().screen().bracketed_paste();
        let bytes = if bracketed {
            format!("\x1b[200~{text}\x1b[201~").into_bytes()
        } else {
            text.as_bytes().to_vec()
        };
        session.input_tx.send(bytes).ok();
    }

    /// End the session, if any, and allow a new one to be spawned
    pub fn clear(&mut self) {
        self.session = None;
        self.pending_since = None;
    }
}

/// Translate a crossterm key press into the bytes a TTY expects.
/// Returns `None` for keys with no terminal representation.
#[allow(clippy::cast_possible_truncation)]
pub fn key_to_bytes(code: KeyCode, modifiers: KeyModifiers) -> Option<Vec<u8>> {
    let csi = |tail: &str| Some(format!("\x1b[{tail}").into_bytes());
    match code {
        KeyCode::Char(c) => {
            if modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl maps the letter block onto bytes 0x00..=0x1f
                let byte = match c.to_ascii_lowercase() {
                    c @ 'a'..='z' => c as u8 - b'a' + 1,
                    ' ' | '@' => 0,
                    '[' => 0x1b,
                    '\\' => 0x1c,
                    ']' => 0x1d,
                    '^' => 0x1e,
                    '_' | '?' => 0x1f,
                    _ => return None,
                };
                Some(vec![byte])
            } else if modifiers.contains(KeyModifiers::ALT) {
                let mut out = vec![0x1b];
                out.extend_from_slice(c.to_string().as_bytes());
                Some(out)
            } else {
                Some(c.to_string().into_bytes())
            }
        }
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::BackTab => csi("Z"),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => csi("A"),
        KeyCode::Down => csi("B"),
        KeyCode::Right => csi("C"),
        KeyCode::Left => csi("D"),
        KeyCode::Home => csi("H"),
        KeyCode::End => csi("F"),
        KeyCode::Insert => csi("2~"),
        KeyCode::Delete => csi("3~"),
        KeyCode::PageUp => csi("5~"),
        KeyCode::PageDown => csi("6~"),
        // F1-F4 use SS3, F5-F12 use CSI with a numeric parameter
        KeyCode::F(n @ 1..=4) => Some(format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes()),
        KeyCode::F(n @ 5..=12) => {
            let param = match n {
                5 => 15,
                6..=10 => n + 11,
                _ => n + 12,
            };
            csi(&format!("{param}~"))
        }
        _ => None,
    }
}

/// Create the exec, attach to it, and spawn the tasks that pump its output into
/// the emulator and the queued keystrokes into its stdin.
async fn spawn_session(
    container: &ContainerId,
    docker: &Arc<Docker>,
    rerender: &Arc<Rerender>,
    size: (u16, u16),
) -> Option<ShellSession> {
    let (rows, cols) = size;
    let exec = docker
        .create_exec(
            container.get(),
            CreateExecOptions {
                attach_stdin: Some(true),
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                tty: Some(true),
                cmd: Some(vec!["sh", "-c", SHELL_CMD]),
                env: Some(vec![TERM]),
                ..Default::default()
            },
        )
        .await
        .ok()?;

    let StartExecResults::Attached { output, input } = docker
        .start_exec(
            &exec.id,
            Some(StartExecOptions {
                detach: false,
                ..Default::default()
            }),
        )
        .await
        .ok()?
    else {
        return None;
    };

    // Size the remote TTY before anything is drawn, so the prompt wraps correctly
    docker
        .resize_exec(
            &exec.id,
            ResizeExecOptions {
                height: rows,
                width: cols,
            },
        )
        .await
        .ok();

    let parser = Arc::new(Mutex::new(Parser::new(rows, cols, 0)));
    let alive = Arc::new(AtomicBool::new(true));
    let cancel = CancellationToken::new();
    let (input_tx, mut input_rx) = unbounded_channel::<Vec<u8>>();

    // Output: docker -> emulator, asking for a redraw whenever the screen changes
    {
        let (parser, alive, rerender, cancel) = (
            Arc::clone(&parser),
            Arc::clone(&alive),
            Arc::clone(rerender),
            cancel.clone(),
        );
        let mut output = output;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    chunk = output.next() => {
                        match chunk {
                            Some(Ok(msg)) => {
                                parser.lock().process(&msg.into_bytes());
                                rerender.update_draw();
                            }
                            // Stream ended or errored: the shell is gone
                            _ => break,
                        }
                    }
                }
            }
            alive.store(false, Ordering::SeqCst);
            rerender.update_draw();
        });
    }

    // Input: queued keystrokes -> docker
    {
        let cancel = cancel.clone();
        let mut input = input;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => break,
                    bytes = input_rx.recv() => {
                        let Some(bytes) = bytes else { break };
                        if input.write_all(&bytes).await.is_err() {
                            break;
                        }
                        input.flush().await.ok();
                    }
                }
            }
        });
    }

    Some(ShellSession {
        container: container.clone(),
        exec_id: exec.id,
        parser,
        input_tx,
        size,
        alive,
        cancel,
    })
}

/// What the manager task decided to do on one pass
enum Action {
    /// Nothing to change
    Idle,
    /// Tear the current session down (selection changed, stopped, or shell exited)
    Drop,
    /// Spawn a shell for this container at this size
    Spawn(ContainerId, (u16, u16)),
    /// Resize the existing session's TTY
    Resize(String, (u16, u16)),
}

/// Decide what to do, reading all shared state under a single short lock.
fn next_action(app_data: &Arc<Mutex<AppData>>) -> Action {
    let mut data = app_data.lock();

    let selected = data
        .get_selected_container_id_state_name()
        .and_then(|(id, state, _)| state.is_alive().then_some(id));

    let Some(wanted) = selected.filter(|_| data.shell.enabled()) else {
        data.shell.pending_since = None;
        return if data.shell.session.is_some() {
            Action::Drop
        } else {
            Action::Idle
        };
    };

    let wanted_size = data.shell.wanted_size;
    if let Some(session) = data.shell.session.as_ref() {
        if session.container != wanted || !session.alive.load(Ordering::SeqCst) {
            return Action::Drop;
        }
        if session.size == wanted_size {
            return Action::Idle;
        }
        return Action::Resize(session.exec_id.clone(), wanted_size);
    }

    // No session yet: wait for the selection to settle before spawning one
    let since = *data.shell.pending_since.get_or_insert_with(Instant::now);
    if since.elapsed() < SPAWN_DEBOUNCE {
        return Action::Idle;
    }
    Action::Spawn(wanted, wanted_size)
}

/// Reconcile the shell session with the selected container, forever. Spawned as
/// its own task so the draw loop and input handler only ever touch shared state.
pub async fn run(
    app_data: Arc<Mutex<AppData>>,
    docker_tx: tokio::sync::mpsc::Sender<DockerMessage>,
    is_running: Arc<AtomicBool>,
    rerender: Arc<Rerender>,
) {
    let mut docker: Option<Arc<Docker>> = None;

    while is_running.load(Ordering::SeqCst) {
        match next_action(&app_data) {
            Action::Idle => (),
            Action::Drop => {
                app_data.lock().shell.clear();
                rerender.update_draw();
            }
            Action::Resize(exec_id, (rows, cols)) => {
                if let Some(docker) = docker.as_ref() {
                    docker
                        .resize_exec(
                            &exec_id,
                            ResizeExecOptions {
                                height: rows,
                                width: cols,
                            },
                        )
                        .await
                        .ok();
                }
                // Record the new size even if the resize failed, else every pass retries
                if let Some(session) = app_data.lock().shell.session.as_mut() {
                    session.parser.lock().screen_mut().set_size(rows, cols);
                    session.size = (rows, cols);
                }
                rerender.update_draw();
            }
            Action::Spawn(container, size) => {
                if docker.is_none() {
                    let (sx, rx) = tokio::sync::oneshot::channel();
                    docker_tx.send(DockerMessage::Exec(sx)).await.ok();
                    docker = rx.await.ok();
                }
                if let Some(docker) = docker.as_ref() {
                    let session = spawn_session(&container, docker, &rerender, size).await;
                    let mut data = app_data.lock();
                    // The selection may have moved on whilst the exec was created
                    let still_wanted = data
                        .get_selected_container_id_state_name()
                        .is_some_and(|(id, state, _)| id == container && state.is_alive());
                    if session.is_some() && still_wanted && data.shell.enabled() {
                        data.shell.session = session;
                    }
                    data.shell.pending_since = None;
                }
                rerender.update_draw();
            }
        }
        tokio::time::sleep(POLL).await;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{DETACH, ShellManager, key_to_bytes};
    use crossterm::event::{KeyCode, KeyModifiers};

    #[test]
    /// Plain characters are sent as their own utf8 bytes
    fn test_key_to_bytes_char() {
        assert_eq!(
            key_to_bytes(KeyCode::Char('l'), KeyModifiers::NONE).unwrap(),
            b"l"
        );
        assert_eq!(
            key_to_bytes(KeyCode::Char('é'), KeyModifiers::NONE).unwrap(),
            "é".as_bytes()
        );
    }

    #[test]
    /// Ctrl + letter collapses onto the matching control byte
    fn test_key_to_bytes_control() {
        assert_eq!(
            key_to_bytes(KeyCode::Char('c'), KeyModifiers::CONTROL).unwrap(),
            vec![0x03]
        );
        assert_eq!(
            key_to_bytes(KeyCode::Char('d'), KeyModifiers::CONTROL).unwrap(),
            vec![0x04]
        );
        // The detach combination is still encodable, the input handler
        // intercepts it before it ever reaches here
        assert_eq!(key_to_bytes(DETACH.0, DETACH.1).unwrap(), vec![0x0f]);
    }

    #[test]
    /// Alt + char is sent as an escape prefix, per xterm
    fn test_key_to_bytes_alt() {
        assert_eq!(
            key_to_bytes(KeyCode::Char('b'), KeyModifiers::ALT).unwrap(),
            vec![0x1b, b'b']
        );
    }

    #[test]
    /// Navigation keys use the standard CSI/SS3 sequences
    fn test_key_to_bytes_navigation() {
        assert_eq!(
            key_to_bytes(KeyCode::Up, KeyModifiers::NONE).unwrap(),
            b"\x1b[A"
        );
        assert_eq!(
            key_to_bytes(KeyCode::Left, KeyModifiers::NONE).unwrap(),
            b"\x1b[D"
        );
        assert_eq!(
            key_to_bytes(KeyCode::Enter, KeyModifiers::NONE).unwrap(),
            b"\r"
        );
        assert_eq!(
            key_to_bytes(KeyCode::Backspace, KeyModifiers::NONE).unwrap(),
            vec![0x7f]
        );
        assert_eq!(
            key_to_bytes(KeyCode::F(1), KeyModifiers::NONE).unwrap(),
            b"\x1bOP"
        );
        assert_eq!(
            key_to_bytes(KeyCode::F(5), KeyModifiers::NONE).unwrap(),
            b"\x1b[15~"
        );
        assert_eq!(
            key_to_bytes(KeyCode::F(12), KeyModifiers::NONE).unwrap(),
            b"\x1b[24~"
        );
        assert!(key_to_bytes(KeyCode::CapsLock, KeyModifiers::NONE).is_none());
    }

    #[test]
    /// Toggling the view off ends the session; toggling back on only re-enables
    /// the view, the manager task is what actually spawns a new shell
    fn test_shell_manager_toggle() {
        let mut manager = ShellManager::new();
        assert!(manager.enabled());
        assert!(!manager.toggle());
        assert!(!manager.enabled());
        assert!(manager.parser().is_none());
        assert!(manager.toggle());
        assert!(manager.enabled());
    }

    #[test]
    /// A zero-sized pane is ignored, so the emulator never gets an invalid size
    fn test_shell_manager_set_size() {
        let mut manager = ShellManager::new();
        manager.set_size(40, 100);
        assert_eq!(manager.wanted_size, (40, 100));
        manager.set_size(0, 100);
        assert_eq!(manager.wanted_size, (40, 100));
    }

    #[test]
    /// With no session, keystrokes are dropped rather than panicking
    fn test_shell_manager_send_without_session() {
        let manager = ShellManager::new();
        manager.send(b"ls\r".to_vec());
        assert!(!manager.alive());
    }
}
