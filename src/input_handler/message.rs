use crossterm::event::{KeyCode, KeyModifiers, MouseEvent};

#[derive(Debug, Clone)]
pub enum InputMessages {
    ButtonPress((KeyCode, KeyModifiers)),
    MouseEvent((MouseEvent, KeyModifiers)),
    /// A bracketed paste, forwarded whole to the shell when attached
    Paste(String),
}
