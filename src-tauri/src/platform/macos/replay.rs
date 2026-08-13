//! Handing a swallowed chord prefix back to the app that should have had it.
//!
//! Same synthesis technique as `text.rs`: a `CGEvent` posted at the HID level,
//! with the modifier flags set on the event rather than pressed, so a modifier
//! the user happens to be holding cannot leak into it.

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

use crate::platform::{Keystroke, ReplayError};

/// Post `keystroke` as a key-down/key-up pair.
pub fn send(keystroke: &Keystroke) -> Result<(), ReplayError> {
    let key =
        virtual_key(&keystroke.code).ok_or_else(|| ReplayError::UnmappedKey(keystroke.code.clone()))?;

    let mut flags = CGEventFlags::empty();
    if keystroke.meta {
        flags |= CGEventFlags::CGEventFlagCommand;
    }
    if keystroke.control {
        flags |= CGEventFlags::CGEventFlagControl;
    }
    if keystroke.alt {
        flags |= CGEventFlags::CGEventFlagAlternate;
    }
    if keystroke.shift {
        flags |= CGEventFlags::CGEventFlagShift;
    }

    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|()| ReplayError::Synthesis("could not create an event source".into()))?;

    let down = CGEvent::new_keyboard_event(source.clone(), key, true)
        .map_err(|()| ReplayError::Synthesis("could not create the key-down event".into()))?;
    down.set_flags(flags);
    down.post(CGEventTapLocation::HID);

    let up = CGEvent::new_keyboard_event(source, key, false)
        .map_err(|()| ReplayError::Synthesis("could not create the key-up event".into()))?;
    up.set_flags(flags);
    up.post(CGEventTapLocation::HID);

    Ok(())
}

/// `KeyboardEvent.code` name to the `kVK_*` constant for that physical key.
///
/// Covers exactly the keys the hotkey recorder in `src/lib/hotkey.ts` can
/// produce. Anything outside that set returns `None` and is reported rather
/// than silently posted as key 0, which is `A`.
///
/// The values are positions on the keyboard, not characters: `kVK_ANSI_A` is
/// 0 because that is where `A` sits on a US layout, and the same code produces
/// `Q` on AZERTY. That is the right level here — a chord prefix is defined by
/// the physical key the user pressed.
fn virtual_key(code: &str) -> Option<u16> {
    Some(match code {
        // Letters, in keyboard order rather than alphabetical, because that is
        // the order the constants themselves follow.
        "KeyA" => 0,
        "KeyS" => 1,
        "KeyD" => 2,
        "KeyF" => 3,
        "KeyH" => 4,
        "KeyG" => 5,
        "KeyZ" => 6,
        "KeyX" => 7,
        "KeyC" => 8,
        "KeyV" => 9,
        "KeyB" => 11,
        "KeyQ" => 12,
        "KeyW" => 13,
        "KeyE" => 14,
        "KeyR" => 15,
        "KeyY" => 16,
        "KeyT" => 17,
        "KeyO" => 31,
        "KeyU" => 32,
        "KeyI" => 34,
        "KeyP" => 35,
        "KeyL" => 37,
        "KeyJ" => 38,
        "KeyK" => 40,
        "KeyN" => 45,
        "KeyM" => 46,

        "Digit1" => 18,
        "Digit2" => 19,
        "Digit3" => 20,
        "Digit4" => 21,
        "Digit6" => 22,
        "Digit5" => 23,
        "Digit9" => 25,
        "Digit7" => 26,
        "Digit8" => 28,
        "Digit0" => 29,

        "Equal" => 24,
        "Minus" => 27,
        "BracketRight" => 30,
        "BracketLeft" => 33,
        "Quote" => 39,
        "Semicolon" => 41,
        "Backslash" => 42,
        "Comma" => 43,
        "Slash" => 44,
        "Period" => 47,
        "Backquote" => 50,

        "Enter" => 36,
        "Tab" => 48,
        "Space" => 49,
        "Backspace" => 51,
        "Escape" => 53,

        // The function keys are not contiguous: F3 to F12 were assigned after
        // F1, F2 and the F13-F15 block already had numbers.
        "F1" => 122,
        "F2" => 120,
        "F3" => 99,
        "F4" => 118,
        "F5" => 96,
        "F6" => 97,
        "F7" => 98,
        "F8" => 100,
        "F9" => 101,
        "F10" => 109,
        "F11" => 103,
        "F12" => 111,
        "F13" => 105,
        "F14" => 107,
        "F15" => 113,
        "F16" => 106,
        "F17" => 64,
        "F18" => 79,
        "F19" => 80,
        "F20" => 90,

        "Home" => 115,
        "PageUp" => 116,
        "Delete" => 117,
        "End" => 119,
        "PageDown" => 121,
        "ArrowLeft" => 123,
        "ArrowRight" => 124,
        "ArrowDown" => 125,
        "ArrowUp" => 126,

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_letter_keys_map_to_their_positions_not_their_alphabet_order() {
        // A sanity check on the least obvious part of the table: these are
        // physical positions, so A is 0 but B is 11.
        assert_eq!(virtual_key("KeyA"), Some(0));
        assert_eq!(virtual_key("KeyB"), Some(11));
        assert_eq!(virtual_key("KeyK"), Some(40));
    }

    #[test]
    fn v_matches_the_key_code_the_paste_path_already_uses() {
        // `text.rs` hardcodes 9 for V; if these disagree one of them is wrong.
        assert_eq!(virtual_key("KeyV"), Some(9));
    }

    #[test]
    fn every_key_the_recorder_can_produce_has_a_code() {
        // Mirrors `keyName` in src/lib/hotkey.ts. A gap here is a chord prefix
        // that can be swallowed but never handed back.
        let letters = ('A'..='Z').map(|c| format!("Key{c}"));
        let digits = (0..=9).map(|d| format!("Digit{d}"));
        let named = [
            "Space",
            "Enter",
            "Tab",
            "Escape",
            "Backspace",
            "Delete",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "ArrowUp",
            "ArrowDown",
            "ArrowLeft",
            "ArrowRight",
            "Minus",
            "Equal",
            "Backquote",
            "BracketLeft",
            "BracketRight",
            "Backslash",
            "Semicolon",
            "Quote",
            "Comma",
            "Period",
            "Slash",
        ]
        .into_iter()
        .map(String::from);

        for code in letters.chain(digits).chain(named) {
            assert!(virtual_key(&code).is_some(), "no key code for {code}");
        }
    }

    #[test]
    fn the_function_keys_macos_actually_has_are_all_mapped() {
        // F21 upwards have no kVK constant, so the recorder can offer them but
        // this cannot replay them — that is a reported failure, not a silent 0.
        for n in 1..=20 {
            let code = format!("F{n}");
            assert!(virtual_key(&code).is_some(), "no key code for {code}");
        }
        assert_eq!(virtual_key("F21"), None);
    }

    #[test]
    fn an_unknown_code_is_refused_rather_than_posted_as_key_zero() {
        assert_eq!(virtual_key("NotAKey"), None);
        assert_eq!(virtual_key(""), None);
    }

    #[test]
    fn no_two_keys_share_a_code() {
        // A duplicate would mean one of the constants was transcribed wrong,
        // and the symptom would be a prefix replayed as some other key.
        let codes = [
            "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK",
            "KeyL", "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT", "KeyU", "KeyV",
            "KeyW", "KeyX", "KeyY", "KeyZ", "Digit0", "Digit1", "Digit2", "Digit3", "Digit4",
            "Digit5", "Digit6", "Digit7", "Digit8", "Digit9", "Space", "Enter", "Tab", "Escape",
            "Backspace", "Delete", "Home", "End", "PageUp", "PageDown", "ArrowUp", "ArrowDown",
            "ArrowLeft", "ArrowRight", "Minus", "Equal", "Backquote", "BracketLeft",
            "BracketRight", "Backslash", "Semicolon", "Quote", "Comma", "Period", "Slash", "F1",
            "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14",
            "F15", "F16", "F17", "F18", "F19", "F20",
        ];

        let mut seen = std::collections::HashMap::new();
        for code in codes {
            let key = virtual_key(code).expect("should be mapped");
            if let Some(other) = seen.insert(key, code) {
                panic!("{code} and {other} both map to {key}");
            }
        }
    }
}
