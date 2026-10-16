use std::sync::Arc;

use parking_lot::Mutex;
use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Modifier, Style, Stylize},
    text::Line,
    widgets::{Block, BorderType, Borders, Paragraph},
};
use tui_term::widget::PseudoTerminal;

use crate::{
    app_data::AppData,
    config::{AppColors, Keymap},
    shell::DETACH_TEXT,
    ui::FrameData,
};

/// Build the pane's border, titled with the container name on the left and the
/// keys that act on it on the right, so the pane documents itself.
fn block<'a>(colors: AppColors, fd: &FrameData, keymap: &Keymap) -> Block<'a> {
    let border = if fd.shell_attached {
        colors.borders.selected
    } else {
        colors.borders.unselected
    };

    let title = if fd.shell_title.is_empty() {
        " shell ".to_owned()
    } else {
        format!(" shell ▸ {} ", fd.shell_title)
    };

    let hint = if fd.shell_attached {
        format!(" ● attached — {DETACH_TEXT} to detach ")
    } else {
        format!(
            " {} attach · {} logs ",
            keymap.shell_attach.0, keymap.shell_toggle.0
        )
    };

    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .title(Line::from(title).left_aligned())
        .title(Line::from(hint).right_aligned())
        .bg(colors.logs.background)
}

/// Draw the interactive shell attached to the selected container.
///
/// The emulator is sized by the pane itself: the size drawn here is handed back
/// to the `ShellManager`, whose task resizes both the `vt100` emulator and the
/// remote TTY to match on its next pass. Until that has happened at least once
/// the emulator renders at its previous size, which the pane simply clips.
pub fn draw(
    app_data: &Arc<Mutex<AppData>>,
    area: Rect,
    colors: AppColors,
    f: &mut Frame,
    fd: &FrameData,
    keymap: &Keymap,
) {
    let block = block(colors, fd, keymap);

    // Tell the manager what size to run the shell at (inside the border)
    let rows = area.height.saturating_sub(2);
    let cols = area.width.saturating_sub(2);

    // Take the emulator out from under the lock, so rendering never holds it
    let parser = {
        let mut data = app_data.lock();
        data.shell.set_size(rows, cols);
        data.shell.parser()
    };

    if let Some(parser) = parser {
        let parser = parser.lock();
        f.render_widget(PseudoTerminal::new(parser.screen()).block(block), area);
    } else {
        let paragraph = Paragraph::new(format!("connecting to a shell {}", fd.loading_icon))
            .block(block)
            .fg(colors.logs.text)
            .add_modifier(Modifier::DIM)
            .alignment(Alignment::Center);
        f.render_widget(paragraph, area);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use insta::assert_snapshot;
    use ratatui::style::Color;

    use crate::{
        config::{AppColors, Keymap},
        ui::draw_blocks::tests::{BORDER_CHARS, get_result, test_setup},
    };

    #[test]
    /// With no session yet, the pane shows the connecting message and the keys
    fn test_draw_blocks_shell_connecting() {
        let mut setup = test_setup(60, 6, true, true);
        let colors = setup.app_data.lock().config.app_colors;
        let keymap = Keymap::new();

        setup
            .terminal
            .draw(|f| {
                super::draw(&setup.app_data, setup.area, colors, f, &setup.fd, &keymap);
            })
            .unwrap();

        assert_snapshot!(setup.terminal.backend());

        // Detached, so the border uses the unselected colour
        for (_, result_row) in get_result(&setup) {
            for result_cell in result_row {
                if BORDER_CHARS.contains(&result_cell.symbol()) {
                    assert_eq!(result_cell.fg, Color::Gray);
                }
            }
        }
    }

    #[test]
    /// Attaching highlights the border and swaps the hint for the detach key
    fn test_draw_blocks_shell_attached() {
        let mut setup = test_setup(60, 6, true, true);
        let colors = AppColors::new();
        let keymap = Keymap::new();
        setup.fd.shell_attached = true;
        setup.fd.shell_title = "container_1".to_owned();

        setup
            .terminal
            .draw(|f| {
                super::draw(&setup.app_data, setup.area, colors, f, &setup.fd, &keymap);
            })
            .unwrap();

        assert_snapshot!(setup.terminal.backend());

        for (_, result_row) in get_result(&setup) {
            for result_cell in result_row {
                if BORDER_CHARS.contains(&result_cell.symbol()) {
                    assert_eq!(result_cell.fg, Color::LightCyan);
                }
            }
        }
    }

    #[test]
    /// Drawing the pane records its inner size, which is what the shell runs at
    fn test_draw_blocks_shell_records_size() {
        let mut setup = test_setup(60, 10, true, true);
        let colors = AppColors::new();
        let keymap = Keymap::new();

        setup
            .terminal
            .draw(|f| {
                super::draw(&setup.app_data, setup.area, colors, f, &setup.fd, &keymap);
            })
            .unwrap();

        // 60x10 pane, minus the border on each side
        assert_eq!(setup.app_data.lock().shell.wanted_size(), (8, 58));
    }
}
