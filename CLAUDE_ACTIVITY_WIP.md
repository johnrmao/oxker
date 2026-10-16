# oxker fork notes

Four additions on top of oxker v0.13.4:

1. **claude-activity pane** — a second log pane per container showing the
   command + output of `docker exec` calls made into it.
2. **Live shell pane** — while a container is running, the main log pane is an
   interactive shell attached to it, rendered in place.
3. **Colour-coded log timestamps** — the leading timestamp on each log line is
   styled separately from the log content.
4. **Pane-scoped mouse selection** — drag to select inside one pane, copy on
   release, and paste into the attached shell.

## Status: WORKING
- `cargo build`, `cargo test` (242), `cargo clippy --all-targets`, `cargo fmt` all pass.
- Verified live via tmux capture against the `kali` container: shell prompt,
  typed commands, `ls --color` output, attach/detach, the `t` toggle back to
  logs, and the coloured timestamp all render correctly. Drag-select copies
  only the dragged pane (clamped at its edge, no borders, no neighbouring
  pane), single- and multi-line pastes reach the shell intact, and Tab
  alternates between two panels without ever selecting the shell pane.

## 1. claude-activity pane
- Host side: `~/.local/bin/docker` is a transparent wrapper (ahead of
  `/usr/bin/docker` in PATH). For a non-interactive `docker exec` into a
  container listed in `~/.config/claude-activity/mirror.conf` (one name per
  line, `*` = all), it mirrors the command line + combined output to
  `~/.config/claude-activity/logs/<container>.log`. All other docker usage is
  passed straight through.
- oxker side: a background task tails that per-container file for the SELECTED
  container into `ActivityState`; the log area is split 50/50 and the feed is
  drawn on the right. The pane only appears when the file exists, so "opting a
  container in" = a log existing for it (add its name to mirror.conf).

## 2. Live shell pane
- The main pane shows **a live shell whilst the container is running**, and the
  **normal logs once it isn't**. `e` still works for a full-screen exec.
- Mechanics: a `docker exec` with `tty: true` over the existing bollard
  connection, output fed into a `vt100` emulator, rendered by `tui-term`'s
  `PseudoTerminal` widget. Full colour and cursor control, as if exec'd into.
- Tab cycles **containers ↔ commands only**. The log area may be showing the
  shell or the activity feed, neither of which is a panel, so tabbing onto it
  selected something with nothing on screen to show it. The logs panel is still
  reachable by clicking it, and the shell by its attach key.
- Two separate pieces of state:
  - **visible** (`ShellManager::enabled`) — shell view vs logs view, toggled
    with `t`.
  - **attached** (`Status::Shell`) — whether the keyboard goes to the shell.
    Press `a` to attach, `ctrl + o` to detach. Whilst attached every key,
    `q` and `ctrl + c` included, goes to the shell, and mouse events are
    ignored so the panes can't move under you.
- The pane sizes the shell: the drawn inner size is recorded on each frame and
  the manager task resizes both the emulator and the remote TTY to match.
- A container must stay selected for 400ms before a shell is spawned, so
  scrolling the container list doesn't spray execs across the host. Switching
  container, stopping it, exiting the shell, or toggling to logs ends the
  session.

## 3. Colour-coded timestamps
- `update_log_by_id` now styles the timestamp as its own span ahead of the
  sanitized log content, so container ansi can't recolour it.
- New config key `colors.logs.timestamp` (default `DarkGray`); set to
  `#6c7086` in `~/.config/oxker/config.toml`.
- Same file sets `timestamp_format = "%Y-%m-%dT%H:%M:%S"` — seconds, no
  fractional part.

## 4. Pane-scoped mouse selection
- The problem: oxker grabs the mouse, so the terminal's own selection is both
  unavailable and the wrong shape - it can only take full-width rows spanning
  every pane side by side.
- Selection now happens inside oxker, clipped to one pane's interior. Drag with
  the left button in the shell, activity or logs pane; the selection is
  linewise, the same shape a terminal gives you. Release copies.
- Text is read back out of the *rendered buffer*, so what is copied is exactly
  what is on screen - no second guess at how a pane wrapped its content, and it
  works the same for all three panes whatever drew them.
- The clipboard is set with OSC 52, so it still works over ssh and inside a
  multiplexer. An info box confirms how many lines were copied.
- `?1002h` is now enabled alongside the existing mouse modes, which is what
  reports motion whilst a button is held. All-motion (`?1003h`) is still off.
- Selection works whilst attached to the shell too - you usually want to copy
  output you just produced. A plain click still focuses a panel; scrolling or
  typing drops the selection, as the content moves out from under it.
- Pastes arrive as bracketed pastes and are forwarded whole to the shell,
  re-bracketed only if the program on the far end turned bracketed paste on.
- Panes register themselves as selectable while drawing, into a pending set
  that is published only once the frame is complete, so a press is never
  tested against a half-built set of panes.

## Files changed in the fork
- `src/app_data/activity.rs` (new): `ActivityState` tailer + unit tests.
- `src/shell.rs` (new): `ShellManager`, the session/exec lifecycle, the
  crossterm-key → TTY-bytes encoder, + unit tests.
- `src/ui/draw_blocks/activity.rs`, `src/ui/draw_blocks/shell.rs` (new) + both
  registered in `draw_blocks/mod.rs`.
- `src/ui/selection.rs` (new): the `Selection` shape, buffer text extraction,
  OSC 52 + base64, with unit tests.
- `src/ui/draw_blocks/mod.rs`: `draw_selection`, called by all three panes.
- `src/ui/gui_state.rs`: the selection, the pane registry, clipboard handoff,
  the tab order, and `commit_areas` - which also fixed a latent bug where
  `intersect_panel` was only cleared on resize, so a click in the shell pane
  could still land on the logs panel that was no longer drawn.
- `src/app_data/mod.rs`: `activity`/`shell` fields, coloured timestamp spans.
- `src/ui/mod.rs`: FrameData gains the activity + shell fields; `draw_frame`
  picks shell vs logs for the main pane and splits off the activity pane.
- `src/input_handler/mod.rs`: `handle_shell`, the attach/toggle keys, the
  guards that stop `q`/`ctrl + c`/mouse acting on oxker whilst attached, plus
  `mouse_selection` and `paste`.
- `src/input_handler/message.rs`: `InputMessages::Paste`.
- `src/config/keymap_parser.rs`: `shell_attach` (`a`), `shell_toggle` (`t`).
- `src/config/color_parser.rs`: `logs.timestamp`.
- `src/ui/gui_state.rs`: `Status::Shell`.
- `src/ui/draw_blocks/help.rs`: both new keys in the help panel.
- `src/main.rs`: `activity_init` + `shell::run` tasks.
- `src/ui/mod.rs`: `?1002h`, bracketed paste, drag/release/paste event
  forwarding, `write_clipboard`.
- `Cargo.toml`: `tui-term`, `vt100`.

## Build / install / run
- `cd ~/GitRepos/oxker && cargo build --release`
- Installed to `~/.local/bin/oxker` (ahead of the pacman `/usr/bin/oxker`).
  Replace it with `rm` then `cp`, not `cp` alone, or a running instance gives
  "Text file busy". To revert: remove `~/.local/bin/oxker`.

## Possible next steps
- Scrollback in the shell and activity panes (both are tail views).
- Block/rectangular selection on a modifier, for copying columns.
- Opt the activity pane in via a Docker label instead of mirror.conf.
