//! Keys and mouse clicks, reduced to what the picker understands.
//!
//! [`classify`] is the one place that knows which physical key does what, so the
//! key table has exactly one implementation and one set of tests.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// What a key press means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// A printable character for the filter.
    Char(char),
    /// Delete the last character of the filter (Backspace, Ctrl+H).
    Backspace,
    /// Clear the filter (Ctrl+U).
    ClearFilter,
    /// Delete the last word of the filter (Ctrl+W).
    DeleteWord,
    /// Move up (Up, Ctrl+P, Ctrl+K).
    Up,
    /// Move down (Down, Ctrl+N, Ctrl+J).
    Down,
    /// A page up.
    PageUp,
    /// A page down.
    PageDown,
    /// The first row.
    Home,
    /// The last row.
    End,
    /// Connect with the machine's own screen choice.
    Enter,
    /// Connect with a private screen (Alt+Enter).
    AltEnter,
    /// Open the screen chooser (Ctrl+O).
    Chooser,
    /// Fix the selected machine (Ctrl+F).
    Fix,
    /// Check every machine again (Ctrl+R).
    Recheck,
    /// Edit the configuration file (Ctrl+E).
    Edit,
    /// Escape.
    Esc,
    /// Leave (Ctrl+C, Ctrl+Q).
    Quit,
    /// Anything else: ignored.
    Other,
}

/// The meaning of a key event. Key releases are [`Input::Other`].
#[must_use]
pub fn classify(key: KeyEvent) -> Input {
    if key.kind == KeyEventKind::Release {
        return Input::Other;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char(c) if ctrl => match c.to_ascii_lowercase() {
            'c' | 'q' => Input::Quit,
            'h' => Input::Backspace,
            'u' => Input::ClearFilter,
            'w' => Input::DeleteWord,
            'p' | 'k' => Input::Up,
            'n' | 'j' => Input::Down,
            'o' => Input::Chooser,
            'f' => Input::Fix,
            'r' => Input::Recheck,
            'e' => Input::Edit,
            _ => Input::Other,
        },
        KeyCode::Char(_) if alt => Input::Other,
        KeyCode::Char(c) if !c.is_control() => Input::Char(c),
        KeyCode::Backspace => Input::Backspace,
        KeyCode::Up => Input::Up,
        KeyCode::Down => Input::Down,
        KeyCode::PageUp => Input::PageUp,
        KeyCode::PageDown => Input::PageDown,
        KeyCode::Home => Input::Home,
        KeyCode::End => Input::End,
        KeyCode::Enter if alt => Input::AltEnter,
        KeyCode::Enter => Input::Enter,
        KeyCode::Esc => Input::Esc,
        _ => Input::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode, mods: KeyModifiers) -> Input {
        classify(KeyEvent::new(code, mods))
    }

    fn ctrl(c: char) -> Input {
        k(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn every_key_of_the_table() {
        let none = KeyModifiers::NONE;
        let cases = [
            (k(KeyCode::Char('a'), none), Input::Char('a')),
            (k(KeyCode::Char('A'), KeyModifiers::SHIFT), Input::Char('A')),
            (k(KeyCode::Char('é'), none), Input::Char('é')),
            (k(KeyCode::Char('@'), none), Input::Char('@')),
            (k(KeyCode::Backspace, none), Input::Backspace),
            (ctrl('h'), Input::Backspace),
            (ctrl('u'), Input::ClearFilter),
            (ctrl('w'), Input::DeleteWord),
            (k(KeyCode::Up, none), Input::Up),
            (ctrl('p'), Input::Up),
            (ctrl('k'), Input::Up),
            (k(KeyCode::Down, none), Input::Down),
            (ctrl('n'), Input::Down),
            (ctrl('j'), Input::Down),
            (k(KeyCode::PageUp, none), Input::PageUp),
            (k(KeyCode::PageDown, none), Input::PageDown),
            (k(KeyCode::Home, none), Input::Home),
            (k(KeyCode::End, none), Input::End),
            (k(KeyCode::Enter, none), Input::Enter),
            (k(KeyCode::Enter, KeyModifiers::ALT), Input::AltEnter),
            (ctrl('o'), Input::Chooser),
            (ctrl('f'), Input::Fix),
            (ctrl('r'), Input::Recheck),
            (ctrl('e'), Input::Edit),
            (k(KeyCode::Esc, none), Input::Esc),
            (ctrl('c'), Input::Quit),
            (ctrl('q'), Input::Quit),
        ];
        for (got, want) in cases {
            assert_eq!(got, want);
        }
    }

    #[test]
    fn upper_case_control_letters_and_unknown_keys_are_harmless() {
        assert_eq!(ctrl('C'), Input::Quit);
        assert_eq!(ctrl('z'), Input::Other);
        assert_eq!(k(KeyCode::Tab, KeyModifiers::NONE), Input::Other);
        assert_eq!(k(KeyCode::Char('x'), KeyModifiers::ALT), Input::Other);
        assert_eq!(k(KeyCode::Left, KeyModifiers::NONE), Input::Other);
        assert_eq!(k(KeyCode::F(5), KeyModifiers::NONE), Input::Other);
    }

    #[test]
    fn releases_are_ignored() {
        let mut e = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        e.kind = KeyEventKind::Release;
        assert_eq!(classify(e), Input::Other);
    }
}
