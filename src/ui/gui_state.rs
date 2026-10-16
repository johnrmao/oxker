use parking_lot::Mutex;
use ratatui::layout::{Constraint, Rect};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    app_data::{AppData, ContainerId, Header, ScrollDirection},
    exec::ExecMode,
};

use super::{
    Rerender,
    selection::{SelectPane, Selection},
};

#[derive(Debug, Default, Clone, Copy, Eq, Hash, PartialEq)]
pub enum SelectablePanel {
    #[default]
    Containers,
    Commands,
    Logs,
}

impl SelectablePanel {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Containers => "Containers",
            Self::Logs => "Logs",
            Self::Commands => "",
        }
    }
    /// The tab order, which deliberately skips Logs. The log area may be
    /// showing the shell or the activity feed rather than the logs, neither of
    /// which is a panel, so tabbing onto it would select something the user
    /// cannot see. The shell is reached with its own attach key instead, and
    /// the logs panel by clicking it.
    pub const fn next(self) -> Self {
        match self {
            Self::Containers => Self::Commands,
            Self::Commands | Self::Logs => Self::Containers,
        }
    }
    pub const fn prev(self) -> Self {
        match self {
            Self::Containers | Self::Logs => Self::Commands,
            Self::Commands => Self::Containers,
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum Region {
    Panel(SelectablePanel),
    Header(Header),
    HelpPanel,
    Delete(DeleteButton),
}

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub enum DeleteButton {
    Confirm,
    Cancel,
}

#[allow(unused)]
#[derive(Debug, Clone, Copy)]
pub enum BoxLocation {
    TopLeft,
    TopCentre,
    TopRight,
    MiddleLeft,
    MiddleCentre,
    MiddleRight,
    BottomLeft,
    BottomCentre,
    BottomRight,
}

impl BoxLocation {
    /// Screen is divided into 3x3 sections
    pub const fn get_indexes(self) -> (usize, usize) {
        match self {
            Self::TopLeft => (0, 0),
            Self::TopCentre => (0, 1),
            Self::TopRight => (0, 2),
            Self::MiddleLeft => (1, 0),
            Self::MiddleCentre => (1, 1),
            Self::MiddleRight => (1, 2),
            Self::BottomLeft => (2, 0),
            Self::BottomCentre => (2, 1),
            Self::BottomRight => (2, 2),
        }
    }

    /// Get both the vertical and hoziztonal constrains
    pub const fn get_constraints(
        self,
        blank_horizontal: u16,
        blank_vertical: u16,
        text_lines: u16,
        text_width: u16,
    ) -> ([Constraint; 3], [Constraint; 3]) {
        (
            Self::get_horizontal_constraints(self, blank_horizontal, text_width),
            Self::get_vertical_constraints(self, blank_vertical, text_lines),
        )
    }

    const fn get_horizontal_constraints(
        self,
        blank_horizontal: u16,
        text_width: u16,
    ) -> [Constraint; 3] {
        match self {
            Self::TopLeft | Self::MiddleLeft | Self::BottomLeft => [
                Constraint::Min(text_width),
                Constraint::Max(blank_horizontal),
                Constraint::Max(blank_horizontal),
            ],
            Self::TopCentre | Self::MiddleCentre | Self::BottomCentre => [
                Constraint::Max(blank_horizontal),
                Constraint::Min(text_width),
                Constraint::Max(blank_horizontal),
            ],
            Self::TopRight | Self::MiddleRight | Self::BottomRight => [
                Constraint::Max(blank_horizontal),
                Constraint::Max(blank_horizontal),
                Constraint::Min(text_width),
            ],
        }
    }

    const fn get_vertical_constraints(
        self,
        blank_vertical: u16,
        number_lines: u16,
    ) -> [Constraint; 3] {
        match self {
            Self::TopLeft | Self::TopCentre | Self::TopRight => [
                Constraint::Min(number_lines),
                Constraint::Max(blank_vertical),
                Constraint::Max(blank_vertical),
            ],
            Self::MiddleLeft | Self::MiddleCentre | Self::MiddleRight => [
                Constraint::Max(blank_vertical),
                Constraint::Min(number_lines),
                Constraint::Max(blank_vertical),
            ],
            Self::BottomLeft | Self::BottomCentre | Self::BottomRight => [
                Constraint::Max(blank_vertical),
                Constraint::Max(blank_vertical),
                Constraint::Min(number_lines),
            ],
        }
    }
}

// loading animation frames
const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const FRAMES_LEN: u8 = 9;

/// The application gui state can be in multiple of these four states at the same time
/// Various functions (e.g input handler), operate differently depending upon current Status
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum Status {
    DeleteConfirm,
    DockerConnect(Option<String>),
    Error,
    Exec,
    Filter,
    Help,
    Init,
    Inspect,
    Logs,
    SearchLogs,
    /// The keyboard is handed to the shell attached to the selected container
    Shell,
}

#[derive(Debug, Default, Copy, Clone, Hash, PartialEq, Eq)]
pub struct ScrollOffset {
    pub x: usize,
    pub y: usize,
}

/// Global gui_state, stored in an Arc<Mutex>
#[derive(Debug)]
pub struct GuiState {
    delete_container_id: Option<ContainerId>,
    exec_mode: Option<ExecMode>,
    intersect_delete: HashMap<DeleteButton, Rect>,
    intersect_heading: HashMap<Header, Rect>,
    intersect_help: Option<Rect>,
    intersect_panel: HashMap<SelectablePanel, Rect>,
    /// Panels registered so far by the frame currently being drawn
    intersect_panel_pending: HashMap<SelectablePanel, Rect>,
    loading_handle: Option<CancellationToken>,
    loading_index: u8,
    loading_set: HashSet<Uuid>,
    log_height: u16,
    rerender: Arc<Rerender>,
    selected_panel: SelectablePanel,
    screen_width: u16,
    show_logs: bool,
    /// Interior rect of each selectable-text pane, as of the last whole frame
    select_areas: HashMap<SelectPane, Rect>,
    /// Panes registered so far by the frame currently being drawn
    select_areas_pending: HashMap<SelectPane, Rect>,
    /// The in-progress or just-finished mouse selection
    selection: Option<Selection>,
    /// Text waiting to be handed to the terminal's clipboard
    clipboard: Option<String>,
    inspect_offset: ScrollOffset,
    inspect_offset_max: ScrollOffset,
    status: HashSet<Status>,
    pub info_box_text: Option<(String, Instant)>,
}
impl GuiState {
    pub fn new(redraw: &Arc<Rerender>, show_logs: bool) -> Self {
        Self {
            delete_container_id: None,
            exec_mode: None,
            info_box_text: None,
            intersect_delete: HashMap::new(),
            intersect_heading: HashMap::new(),
            intersect_help: None,
            intersect_panel: HashMap::new(),
            intersect_panel_pending: HashMap::new(),
            inspect_offset: ScrollOffset::default(),
            inspect_offset_max: ScrollOffset::default(),
            loading_handle: None,
            loading_index: 0,
            loading_set: HashSet::new(),
            log_height: 75,
            screen_width: 0,
            rerender: Arc::clone(redraw),
            selected_panel: SelectablePanel::default(),
            show_logs,
            select_areas: HashMap::new(),
            select_areas_pending: HashMap::new(),
            selection: None,
            clipboard: None,
            status: HashSet::new(),
        }
    }

    /// Record a pane's interior as it is drawn, so a mouse press can be mapped
    /// back to the pane it landed in. Held aside until the frame is complete,
    /// so a press is never tested against a half-built set of panes.
    pub fn set_select_area(&mut self, pane: SelectPane, inner: Rect) {
        self.select_areas_pending.insert(pane, inner);
    }

    /// Publish what the finished frame actually drew, dropping anything it did
    /// not. The log area holds either the logs panel or the shell, so without
    /// this a click could still land on the panel that is no longer on screen.
    pub fn commit_areas(&mut self) {
        self.select_areas = std::mem::take(&mut self.select_areas_pending);
        self.intersect_panel = std::mem::take(&mut self.intersect_panel_pending);
    }

    /// Begin a selection if the press landed inside a pane that supports it.
    /// Returns true if it did, so the caller knows not to treat the press as a
    /// panel change as well.
    pub fn selection_start(&mut self, x: u16, y: u16) -> bool {
        let hit = self
            .select_areas
            .iter()
            .find(|(_, area)| area.contains((x, y).into()))
            .map(|(pane, _)| *pane);
        self.selection = hit.map(|pane| Selection::new(pane, (x, y)));
        self.rerender.update_draw();
        hit.is_some()
    }

    /// Extend the current selection to the dragged-to cell
    pub fn selection_drag(&mut self, x: u16, y: u16) {
        if let Some(selection) = self.selection.as_mut() {
            selection.drag_to((x, y));
            self.rerender.update_draw();
        }
    }

    /// Finish a selection. An empty one (a plain click) is dropped rather than
    /// left on screen, and only a real drag asks for a copy.
    pub fn selection_finish(&mut self) {
        match self.selection.as_mut() {
            Some(selection) if selection.is_empty() => self.selection = None,
            Some(selection) => selection.copy_pending = true,
            None => return,
        }
        self.rerender.update_draw();
    }

    pub fn selection_clear(&mut self) {
        if self.selection.take().is_some() {
            self.rerender.update_draw();
        }
    }

    /// The selection, if it is in the given pane
    pub fn get_selection(&self, pane: SelectPane) -> Option<Selection> {
        self.selection.filter(|i| i.pane == pane)
    }

    /// Mark the pending copy as served, so the text is only read out once
    pub fn selection_copied(&mut self, text: String) {
        if let Some(selection) = self.selection.as_mut() {
            selection.copy_pending = false;
        }
        if !text.is_empty() {
            self.clipboard = Some(text);
        }
    }

    /// Take any text waiting to go to the clipboard
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard.take()
    }
    /// Increase the height of the log panel, then rerender
    pub fn log_height_increase(&mut self) {
        if self.show_logs && self.log_height <= 75 {
            self.log_height = self.log_height.saturating_add(5);
            self.rerender.update_draw();
        }
    }

    /// Reduce the height of the logs panel, then rerender
    /// Unselect logs panel if currently selected
    pub fn log_height_decrease(&mut self) {
        if self.show_logs {
            self.log_height = self.log_height.saturating_sub(5);
            if self.log_height == 0 && self.selected_panel == SelectablePanel::Logs {
                self.show_logs = false;
                self.selected_panel = SelectablePanel::Containers;
            }
            self.rerender.update_draw();
        }
    }

    pub fn set_inspect_offset(&mut self, sd: &ScrollDirection) {
        match sd {
            ScrollDirection::Up => self.inspect_offset.y = self.inspect_offset.y.saturating_sub(1),
            ScrollDirection::Down => {
                self.inspect_offset.y = self
                    .inspect_offset
                    .y
                    .saturating_add(1)
                    .min(self.inspect_offset_max.y)
            }
            ScrollDirection::Left => {
                self.inspect_offset.x = self.inspect_offset.x.saturating_sub(1)
            }
            ScrollDirection::Right => {
                self.inspect_offset.x = self
                    .inspect_offset
                    .x
                    .saturating_add(1)
                    .min(self.inspect_offset_max.x)
            }
        }
        self.rerender.update_draw();
    }

    pub fn get_inspect_offset(&self) -> ScrollOffset {
        self.inspect_offset
    }

    pub fn set_inspect_offset_max(&mut self, offset: ScrollOffset) {
        self.inspect_offset_max = offset
    }

    pub fn set_inspect_offset_y_to_max(&mut self) {
        self.inspect_offset.y = self.inspect_offset_max.y;
        self.rerender.update_draw();
    }

    pub fn clear_inspect_offset(&mut self) {
        self.inspect_offset.x = 0;
        self.inspect_offset.y = 0;
        self.inspect_offset_max = ScrollOffset::default();
        self.rerender.update_draw();
    }

    /// Set the screen width, used for offset char calculations
    pub const fn set_screen_width(&mut self, width: u16) {
        self.screen_width = width;
    }

    /// Get the screen width, used for offset char calculations
    pub const fn get_screen_width(&self) -> u16 {
        self.screen_width
    }

    pub const fn get_show_logs(&self) -> bool {
        self.show_logs
    }

    pub fn toggle_show_logs(&mut self) {
        self.show_logs = !self.show_logs;
        if !self.show_logs && self.selected_panel == SelectablePanel::Logs {
            self.selected_panel = SelectablePanel::Containers;
        }
        self.rerender.update_draw();
    }

    /// Set the log_height to zero, for now only used by tests
    #[cfg(test)]
    pub const fn log_height_zero(&mut self) {
        self.log_height = 0;
    }

    /// Get the log height, *should* be a u8 between 0 and 80, essentially a percentage
    pub const fn get_log_height(&self) -> u16 {
        self.log_height
    }

    /// Clear panels hash map, so on resize can fix the sizes for mouse clicks
    pub fn clear_area_map(&mut self) {
        self.intersect_panel.clear();
        self.intersect_panel_pending.clear();
        self.select_areas.clear();
        self.select_areas_pending.clear();
    }

    /// Set the rerender clear to true, to flush the screen and redraw
    pub fn set_clear(&self) {
        self.rerender.set_clear();
    }

    /// Get the currently selected panel
    pub const fn get_selected_panel(&self) -> SelectablePanel {
        self.selected_panel
    }

    /// Drop a selection of the logs panel, for when the log area is drawing the
    /// shell instead: the panel would be selected but have nothing on screen to
    /// show it, which just reads as the selection having vanished.
    pub fn unselect_logs_panel(&mut self) {
        if self.selected_panel == SelectablePanel::Logs {
            self.selected_panel = SelectablePanel::Containers;
            self.rerender.update_draw();
        }
    }

    /// Check if a given Rect (a clicked area of 1x1), interacts with any known panels
    pub fn check_panel_intersect(&mut self, rect: Rect) {
        if let Some(data) = self
            .intersect_panel
            .iter()
            .filter(|i| i.1.intersects(rect))
            .collect::<Vec<_>>()
            .first()
        {
            self.selected_panel = *data.0;
            self.rerender.update_draw();
        }
    }

    /// Check if a given Rect (a clicked area of 1x1), interacts with any known delete button
    pub fn get_intersect_button(&self, rect: Rect) -> Option<DeleteButton> {
        self.intersect_delete
            .iter()
            .filter(|i| i.1.intersects(rect))
            .collect::<Vec<_>>()
            .first()
            .map(|data| *data.0)
    }

    /// Check if a given Rect (a clicked area of 1x1), interacts with any known panels
    pub fn get_intersect_header(&self, rect: Rect) -> Option<Header> {
        self.intersect_heading
            .iter()
            .filter(|i| i.1.intersects(rect))
            .collect::<Vec<_>>()
            .first()
            .map(|data| *data.0)
    }

    /// Check if a the "show/hide help" section has been clicked
    pub fn get_intersect_help(&self, rect: Rect) -> bool {
        self.intersect_help
            .as_ref()
            .is_some_and(|i| i.intersects(rect))
    }

    /// Insert, or updates header area panel into heading_map
    pub fn update_region_map(&mut self, region: Region, area: Rect) {
        match region {
            Region::Header(header) => {
                self.intersect_heading
                    .entry(header)
                    .and_modify(|w| *w = area)
                    .or_insert(area);
            }
            Region::Panel(panel) => {
                self.intersect_panel_pending
                    .entry(panel)
                    .and_modify(|w| *w = area)
                    .or_insert(area);
            }
            Region::Delete(button) => {
                self.intersect_delete
                    .entry(button)
                    .and_modify(|w| *w = area)
                    .or_insert(area);
            }
            Region::HelpPanel => {
                self.intersect_help = Some(area);
            }
        }
    }

    /// Check if an ContainerId is set in the delete_container field
    pub fn get_delete_container(&self) -> Option<ContainerId> {
        self.delete_container_id.clone()
    }

    /// Set either a ContainerId, or None, to the delete_container field
    /// If Some, will also insert the DeleteConfirm status into self.status
    pub fn set_delete_container(&mut self, id: Option<ContainerId>) {
        if id.is_some() {
            self.status.insert(Status::DeleteConfirm);
        } else {
            self.intersect_delete.clear();
            self.status_del(Status::DeleteConfirm);
        }
        self.delete_container_id = id;
        self.rerender.update_draw();
    }

    /// Return a copy of the Status HashSet
    pub fn get_status(&self) -> HashSet<Status> {
        self.status.clone()
    }

    /// Remove a gui_status into the current gui_status HashSet
    /// Remove exec mode & deleteConfirm is required
    pub fn status_del(&mut self, status: Status) {
        self.status.remove(&status);
        match status {
            Status::DeleteConfirm => {
                self.status.remove(&Status::DeleteConfirm);
            }
            Status::Exec => {
                self.exec_mode = None;
            }
            _ => (),
        }
        self.rerender.update_draw();
    }

    /// Inset the ExecMode into self, and set the Status as exec
    /// Using StatusPush with Status::Exec won't insert into the hash map
    /// To force self.exec_mode to be set
    pub fn set_exec_mode(&mut self, mode: ExecMode) {
        self.exec_mode = Some(mode);
        self.status.insert(Status::Exec);
        self.rerender.update_draw();
    }

    pub fn get_exec_mode(&self) -> Option<ExecMode> {
        self.exec_mode.clone()
    }

    /// Insert a gui_status into the current gui_status HashSet
    /// If the status is Exec, it won't get inserted, set_exec_mode() should be used instead
    pub fn status_push(&mut self, status: Status) {
        if status != Status::Exec {
            self.status.insert(status);
            self.rerender.update_draw();
        }
    }

    pub fn set_logs_panel_selected(&mut self, app_data: &Arc<Mutex<AppData>>) {
        self.selected_panel = SelectablePanel::Logs;
        if (app_data.lock().get_container_len() == 0
            && self.get_selected_panel() == SelectablePanel::Commands)
            || (self.log_height == 0 && self.get_selected_panel() == SelectablePanel::Logs)
        {
            self.selected_panel = self.selected_panel.next();
        }
        self.rerender.update_draw();
    }
    /// Change to next selectable panel
    pub fn selectable_panel_next(&mut self, app_data: &Arc<Mutex<AppData>>) {
        self.selected_panel = self.selected_panel.next();
        // The commands panel isn't drawn when there are no containers
        if app_data.lock().get_container_len() == 0
            && self.get_selected_panel() == SelectablePanel::Commands
        {
            self.selected_panel = self.selected_panel.next();
        }
        self.rerender.update_draw();
    }

    /// Change to previous selectable panel
    pub fn selectable_panel_previous(&mut self, app_data: &Arc<Mutex<AppData>>) {
        self.selected_panel = self.selected_panel.prev();
        if (app_data.lock().get_container_len() == 0
            && self.get_selected_panel() == SelectablePanel::Commands)
            || (self.log_height == 0 && self.get_selected_panel() == SelectablePanel::Logs)
        {
            self.selected_panel = self.selected_panel.prev();
        }
        self.rerender.update_draw();
    }

    /// Insert a new loading_uuid into HashSet, and advance the loading_index by one frame, or reset to 0 if at end of array
    pub fn next_loading(&mut self, uuid: Uuid) {
        if self.loading_index == FRAMES_LEN {
            self.loading_index = 0;
        } else {
            self.loading_index += 1;
        }
        self.loading_set.insert(uuid);
        self.rerender.update_draw();
    }

    pub fn is_loading(&self) -> bool {
        !self.loading_set.is_empty()
    }
    /// If is_loading has any entries, return the char at FRAMES[index], else an empty char, which needs to take up the same space, hence ' '
    pub fn get_loading(&self) -> char {
        if self.is_loading() {
            FRAMES[usize::from(self.loading_index)]
        } else {
            ' '
        }
    }

    /// Animate the loading icon in its own Tokio thread
    /// This should only be able to executed once, rather than multiple spawns
    pub fn start_loading_animation(gui_state: &Arc<Mutex<Self>>, loading_uuid: Uuid) {
        if !gui_state.lock().is_loading() {
            let inner_state = Arc::clone(gui_state);
            let token = CancellationToken::new();

            gui_state.lock().loading_handle = Some(token.clone());
            tokio::spawn(async move {
                tokio::select! {
                    _ = token.cancelled() => (),
                    _ = async {
                        loop {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            inner_state.lock().next_loading(loading_uuid);
                        }
                    } => (),
                }
            });
        }
        gui_state.lock().next_loading(loading_uuid);
    }

    /// Stop the loading_spin function, and reset gui loading status
    pub fn stop_loading_animation(&mut self, loading_uuid: Uuid) {
        self.loading_set.remove(&loading_uuid);
        self.rerender.update_draw();
        if self.loading_set.is_empty() {
            self.loading_index = 0;
            if let Some(h) = &self.loading_handle {
                h.cancel();
            }
            self.loading_handle = None;
        }
    }

    /// Set info box content
    pub fn set_info_box(&mut self, text: &str) {
        self.info_box_text = Some((text.to_owned(), std::time::Instant::now()));
        self.rerender.update_draw();
    }

    /// Remove info box content
    pub fn reset_info_box(&mut self) {
        self.info_box_text = None;
        self.rerender.update_draw();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Arc;

    use super::{GuiState, Rerender, SelectablePanel};

    fn test_gui_state() -> GuiState {
        GuiState::new(&Arc::new(Rerender::new()), true)
    }

    #[test]
    /// Tab cycles between the containers and commands panels only, never onto
    /// the logs panel, whose area may be showing the shell instead
    fn test_selectable_panel_tab_order_skips_logs() {
        let mut panel = SelectablePanel::Containers;
        for _ in 0..4 {
            panel = panel.next();
            assert_ne!(panel, SelectablePanel::Logs);
        }
        assert_eq!(
            SelectablePanel::Containers.next(),
            SelectablePanel::Commands
        );
        assert_eq!(
            SelectablePanel::Commands.next(),
            SelectablePanel::Containers
        );

        let mut panel = SelectablePanel::Containers;
        for _ in 0..4 {
            panel = panel.prev();
            assert_ne!(panel, SelectablePanel::Logs);
        }
        assert_eq!(
            SelectablePanel::Containers.prev(),
            SelectablePanel::Commands
        );
        assert_eq!(
            SelectablePanel::Commands.prev(),
            SelectablePanel::Containers
        );
    }

    #[test]
    /// A logs panel reached by clicking it is still left alone by Tab, and both
    /// directions lead back out of it rather than sticking
    fn test_selectable_panel_leaves_logs() {
        assert_eq!(SelectablePanel::Logs.next(), SelectablePanel::Containers);
        assert_eq!(SelectablePanel::Logs.prev(), SelectablePanel::Commands);
    }

    #[test]
    /// Selecting the logs panel is dropped when the shell takes the log area,
    /// since the selection would otherwise show nowhere
    fn test_unselect_logs_panel() {
        let mut gui_state = test_gui_state();
        gui_state.selected_panel = SelectablePanel::Logs;
        gui_state.unselect_logs_panel();
        assert_eq!(gui_state.get_selected_panel(), SelectablePanel::Containers);

        // Any other panel is left as it is
        gui_state.selected_panel = SelectablePanel::Commands;
        gui_state.unselect_logs_panel();
        assert_eq!(gui_state.get_selected_panel(), SelectablePanel::Commands);
    }

    #[test]
    /// A frame's panels only become clickable once that frame is complete, so a
    /// pane the frame stopped drawing cannot still catch a click
    fn test_commit_areas_drops_undrawn_panels() {
        use super::{Rect, Region};
        let mut gui_state = test_gui_state();
        let area = Rect::new(0, 0, 10, 10);

        gui_state.update_region_map(Region::Panel(SelectablePanel::Logs), area);
        // Not committed yet, so a click finds nothing
        gui_state.check_panel_intersect(Rect::new(1, 1, 1, 1));
        assert_eq!(gui_state.get_selected_panel(), SelectablePanel::Containers);

        gui_state.commit_areas();
        gui_state.check_panel_intersect(Rect::new(1, 1, 1, 1));
        assert_eq!(gui_state.get_selected_panel(), SelectablePanel::Logs);

        // A frame that draws no panels clears them, as the shell pane does
        gui_state.commit_areas();
        gui_state.selected_panel = SelectablePanel::Containers;
        gui_state.check_panel_intersect(Rect::new(1, 1, 1, 1));
        assert_eq!(gui_state.get_selected_panel(), SelectablePanel::Containers);
    }
}
