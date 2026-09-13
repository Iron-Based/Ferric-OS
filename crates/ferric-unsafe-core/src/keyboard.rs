//! On-screen keyboard app: builds the Slint `KeyboardWindow` and drives it
//! from both input paths — the physical PS/2/PL011 keys and the on-screen keys
//! themselves. There is no mouse yet, so on-screen keys are tapped through a
//! stub pointer: the arrow keys move a highlight across the grid and Enter
//! synthesizes a pointer press at the highlighted key, exercising the same
//! `TouchArea` path a real pointer will take.

use alloc::rc::Rc;
use alloc::string::String;
use core::cell::Cell;

use ferric_api::{Key, KeyEvent};
use ferric_safe_core::keyboard::KeyboardModel;
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::{PointerEventButton, WindowEvent};

use crate::slint_platform;

/// Serial proof line emitted once the first keyboard frame is blitted (gates
/// the keyboard step of the smoke test).
pub const KEYBOARD_OK_MARKER: &str = "KEYBOARD OK\n";

/// Prefix of the line emitted whenever the typed text changes.
pub const KEYBOARD_TEXT_PREFIX: &str = "KEYBOARD TEXT ";

/// Prefix of the line emitted when the stub-pointer selection moves.
pub const KEYBOARD_SEL_PREFIX: &str = "KEYBOARD SEL ";

/// Serial proof line emitted when the keyboard exits back to the shell.
pub const KEYBOARD_EXIT_MARKER: &str = "KEYBOARD EXIT OK\n";

// The char-grid layout below must mirror `keyboard.slint`: every grid row is
// flush at KEYS_X0 and each key is KEY_W x KEY_H on a KEY_PITCH grid. The stub
// pointer's synthesized clicks aim at the selected key's center from these.
const KEYS_X0: i32 = 30;
const KEYS_Y0: i32 = 104;
const KEY_W: i32 = 38;
const KEY_H: i32 = 34;
const KEY_PITCH_X: i32 = 42;
const KEY_PITCH_Y: i32 = 42;
/// Key count per char-grid row, in display order (mirror of the `.slint`
/// `row0`..`row3` arrays).
const GRID_ROWS: [i32; 4] = [13, 13, 11, 10];

/// An on-screen key tap deferred until the input drain ends, so Slint's
/// `TouchArea` handlers never run re-entrantly inside another `dispatch_event`.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum PendingKey {
    #[default]
    None,
    // The key's two candidate glyphs; the shared model picks the active one.
    Char(char, char),
    Space,
    Backspace,
    Enter,
}

/// Runs the full-screen on-screen keyboard, returning when Escape is pressed.
/// The caller must repaint the console over the framebuffer afterwards.
pub fn run_keyboard() {
    let ui = ferric_ui::keyboard_window();
    let window = slint_platform::window();
    let (w, h) = {
        let size = window.size();
        (size.width, size.height)
    };

    let state = Rc::new(Cell::new(KeyboardModel::new()));
    let pending = Rc::new(Cell::new(PendingKey::None));

    ui.on_char_pressed({
        let pending = pending.clone();
        move |low, high| pending.set(PendingKey::Char(first_char(&low), first_char(&high)))
    });
    ui.on_space_pressed({
        let pending = pending.clone();
        move || pending.set(PendingKey::Space)
    });
    ui.on_backspace_pressed({
        let pending = pending.clone();
        move || pending.set(PendingKey::Backspace)
    });
    ui.on_enter_pressed({
        let pending = pending.clone();
        move || pending.set(PendingKey::Enter)
    });
    // Modifier keys skip the pointer-shape pipeline: they only flip the shared
    // model, and the loop mirrors it into the window's properties.
    ui.on_shift_pressed({
        let state = state.clone();
        move || state.get().toggle_shift()
    });
    ui.on_caps_pressed({
        let state = state.clone();
        move || state.get().toggle_caps()
    });

    // The TextInput only accepts injected characters while focused; the window
    // is already shown, so hand it the focus before the first input drain.
    ui.invoke_init_focus();

    let mut ok_emitted = false;
    let mut last_text = String::new();
    let (mut sel_row, mut sel_col) = (0i32, 0i32);
    loop {
        slint::platform::update_timers_and_animations();
        while let Some(event) = crate::input::next_key() {
            match event {
                KeyEvent::Press(Key::Escape) => {
                    slint_platform::write_serial(KEYBOARD_EXIT_MARKER);
                    return;
                }
                KeyEvent::Release(Key::Escape) | KeyEvent::Release(Key::Enter) => {}
                // Physical Enter taps the selected on-screen key; the grid
                // needs no scrolling, so arrows just move the selection.
                KeyEvent::Press(Key::Enter) => {
                    click_selected(&window, sel_row, sel_col);
                }
                KeyEvent::Press(Key::Up)
                | KeyEvent::Press(Key::Down)
                | KeyEvent::Press(Key::Left)
                | KeyEvent::Press(Key::Right) => {
                    let (dr, dc) = match event {
                        KeyEvent::Press(Key::Up) => (-1, 0),
                        KeyEvent::Press(Key::Down) => (1, 0),
                        KeyEvent::Press(Key::Left) => (0, -1),
                        _ => (0, 1),
                    };
                    if move_selection(&mut sel_row, &mut sel_col, dr, dc) {
                        ui.set_sel_row(sel_row);
                        ui.set_sel_col(sel_col);
                        let line =
                            alloc::format!("{}{} {}\n", KEYBOARD_SEL_PREFIX, sel_row, sel_col);
                        slint_platform::write_serial(&line);
                    }
                }
                KeyEvent::Release(Key::Up)
                | KeyEvent::Release(Key::Down)
                | KeyEvent::Release(Key::Left)
                | KeyEvent::Release(Key::Right) => {}
                // Physical Shift holds the same momentary state the sticky
                // on-screen Shift toggles, so both paths stay in sync.
                KeyEvent::Press(Key::LeftShift | Key::RightShift) => {
                    state.get().press_shift();
                    if let Some(win_event) = slint_platform::map_key_event(event) {
                        window.dispatch_event(win_event);
                    }
                }
                KeyEvent::Release(Key::LeftShift | Key::RightShift) => {
                    state.get().release_shift();
                    if let Some(win_event) = slint_platform::map_key_event(event) {
                        window.dispatch_event(win_event);
                    }
                }
                KeyEvent::Press(Key::CapsLock) => {
                    state.get().toggle_caps();
                    if let Some(win_event) = slint_platform::map_key_event(event) {
                        window.dispatch_event(win_event);
                    }
                }
                KeyEvent::Release(Key::CapsLock) => {}
                // Printable keys flow through the model too, so a highlighted
                // on-screen SHIFT/CAPS applies to what the physical hand types.
                KeyEvent::Press(Key::Char(ch)) => {
                    let active = state.get().encode(ch, ch);
                    if let Some(text) = slint_platform::key_to_text(Key::Char(active)) {
                        window.dispatch_event(WindowEvent::KeyPressed { text });
                    }
                }
                KeyEvent::Release(Key::Char(_)) => {}
                other => {
                    if let Some(win_event) = slint_platform::map_key_event(other) {
                        window.dispatch_event(win_event);
                    }
                }
            }
        }

        // Deferred on-screen tap: emit its key now that no input is draining.
        let tap = pending.get();
        pending.set(PendingKey::None);
        dispatch_pending_key(&window, &state, tap);

        apply_state(&ui, &state);
        emit_text(&ui, &mut last_text);

        if crate::gui::render_and_blit(&window, w, h) && !ok_emitted {
            slint_platform::write_serial(KEYBOARD_OK_MARKER);
            ok_emitted = true;
        }
        core::hint::spin_loop();
    }
}

fn first_char(s: &slint::SharedString) -> char {
    s.as_str().chars().next().unwrap_or(' ')
}

/// Feeds a deferred on-screen key tap into the focused `TextInput`: the model
/// picks the active glyph and the key-to-text mapping from Session 9.3 turns it
/// into a Slint key-press event.
fn dispatch_pending_key(
    window: &Rc<MinimalSoftwareWindow>,
    state: &Rc<Cell<KeyboardModel>>,
    pending: PendingKey,
) {
    let key = match pending {
        PendingKey::Char(low, high) => Key::Char(state.get().encode(low, high)),
        PendingKey::Space => Key::Char(' '),
        PendingKey::Backspace => Key::Backspace,
        PendingKey::Enter => Key::Enter,
        PendingKey::None => return,
    };
    if let Some(text) = slint_platform::key_to_text(key) {
        window.dispatch_event(WindowEvent::KeyPressed { text });
    }
}

/// Synthesizes a pointer press+release at the selected key's center; the
/// click lands on the same `TouchArea` a real pointer would hit.
fn click_selected(window: &Rc<MinimalSoftwareWindow>, sel_row: i32, sel_col: i32) {
    let x = (KEYS_X0 + sel_col * KEY_PITCH_X + KEY_W / 2) as f32;
    let y = (KEYS_Y0 + sel_row * KEY_PITCH_Y + KEY_H / 2) as f32;
    let position = slint::LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

/// Moves the selection by `(dr, dc)`, clamping to the grid; returns whether it
/// actually moved.
fn move_selection(row: &mut i32, col: &mut i32, dr: i32, dc: i32) -> bool {
    let target_row = (*row + dr).clamp(0, GRID_ROWS.len() as i32 - 1);
    let target_col = (*col + dc).clamp(0, GRID_ROWS[target_row as usize] - 1);
    if (target_row, target_col) == (*row, *col) {
        return false;
    }
    *row = target_row;
    *col = target_col;
    true
}

/// Pushes the model's modifier state into the window.
fn apply_state(ui: &ferric_ui::KeyboardWindow, state: &Rc<Cell<KeyboardModel>>) {
    ui.set_shifted(state.get().shifted());
    ui.set_caps_on(state.get().caps());
}

/// Emits `KEYBOARD TEXT <text>` on every change of the typed text, so the
/// smoke test can watch characters land in the `TextInput`.
fn emit_text(ui: &ferric_ui::KeyboardWindow, last: &mut String) {
    let text = ui.get_typed_text();
    if text.as_str() == last.as_str() {
        return;
    }
    last.clear();
    last.push_str(text.as_str());
    let mut line = String::with_capacity(last.len() + KEYBOARD_TEXT_PREFIX.len() + 1);
    line.push_str(KEYBOARD_TEXT_PREFIX);
    line.push_str(last);
    line.push('\n');
    slint_platform::write_serial(&line);
}
