//! Shift/caps state machine for the on-screen keyboard: decides which of a
//! key's two glyphs is typed and tracks the sticky on-screen modifiers plus
//! the momentary physical Shift. Pure logic — `ferric-unsafe-core` drives it
//! from both the PS/2/PL011 keys and the `.slint` keys so the on-screen
//! display and the characters typed stay consistent.

/// Sticky/momentary shift and sticky caps state for one keyboard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyboardModel {
    shifted: bool,
    caps: bool,
}

impl KeyboardModel {
    /// Caps-off, shift-up keyboard.
    pub const fn new() -> Self {
        Self {
            shifted: false,
            caps: false,
        }
    }

    pub const fn shifted(&self) -> bool {
        self.shifted
    }

    pub const fn caps(&self) -> bool {
        self.caps
    }

    /// Physical Shift is being held down.
    pub fn press_shift(&mut self) {
        self.shifted = true;
    }

    /// Physical Shift was released.
    pub fn release_shift(&mut self) {
        self.shifted = false;
    }

    /// On-screen Shift toggles: sticky until tapped again.
    pub fn toggle_shift(&mut self) {
        self.shifted = !self.shifted;
    }

    pub fn toggle_caps(&mut self) {
        self.caps = !self.caps;
    }

    /// Picks which glyph of a key is active. Letters follow caps XOR shift
    /// (caps + shift cancels back to lowercase); punctuation rises with shift
    /// only, mirroring the PS/2 scancode decoder. Non-graphic keys (e.g.
    /// space) are never raised.
    pub fn encode(&self, low: char, high: char) -> char {
        if low.is_ascii_lowercase() {
            if self.caps ^ self.shifted { high } else { low }
        } else if low.is_ascii_graphic() && self.shifted {
            high
        } else {
            low
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_unshifted_lowercase() {
        let m = KeyboardModel::new();
        assert!(!m.shifted());
        assert!(!m.caps());
        assert_eq!(m.encode('a', 'A'), 'a');
        assert_eq!(m.encode('1', '!'), '1');
        assert_eq!(m.encode(',', '<'), ',');
    }

    #[test]
    fn physical_shift_is_momentary() {
        let mut m = KeyboardModel::new();
        m.press_shift();
        assert!(m.shifted());
        assert_eq!(m.encode('a', 'A'), 'A');
        assert_eq!(m.encode('1', '!'), '!');
        m.release_shift();
        assert!(!m.shifted());
        assert_eq!(m.encode('a', 'A'), 'a');
        assert_eq!(m.encode('1', '!'), '1');
    }

    #[test]
    fn on_screen_shift_is_sticky() {
        let mut m = KeyboardModel::new();
        m.toggle_shift();
        assert!(m.shifted());
        assert_eq!(m.encode('a', 'A'), 'A');
        m.toggle_shift();
        assert!(!m.shifted());
        assert_eq!(m.encode('a', 'A'), 'a');
    }

    #[test]
    fn caps_alone_raises_letters_not_punctuation() {
        let mut m = KeyboardModel::new();
        m.toggle_caps();
        assert!(m.caps());
        assert_eq!(m.encode('a', 'A'), 'A');
        assert_eq!(m.encode('1', '!'), '1');
        assert_eq!(m.encode(',', '<'), ',');
    }

    #[test]
    fn caps_plus_shift_cancels_for_letters_not_punctuation() {
        let mut m = KeyboardModel::new();
        m.toggle_caps();
        m.press_shift();
        assert_eq!(m.encode('a', 'A'), 'a');
        assert_eq!(m.encode('1', '!'), '!');
    }

    #[test]
    fn space_is_never_raised() {
        let mut m = KeyboardModel::new();
        m.toggle_shift();
        m.toggle_caps();
        assert_eq!(m.encode(' ', ' '), ' ');
    }
}
