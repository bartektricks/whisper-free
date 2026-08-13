//! Handing a swallowed chord prefix back to the app that should have had it.
//!
//! The asymmetry `text.rs` documents applies here too: `SendInput` states no
//! flags of its own, so the modifiers are pressed and released around the key,
//! and anything the user is still holding has to be cleared first — otherwise a
//! held Shift would replay `Ctrl+K` as `Ctrl+Shift+K`.

use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT,
    KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};

use crate::platform::{Keystroke, ReplayError};

/// Every modifier, so the ones this keystroke does not want can be released.
const ALL_MODIFIERS: [VIRTUAL_KEY; 5] = [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN];

/// Press `keystroke`, holding exactly the modifiers it names and no others.
pub fn send(keystroke: &Keystroke) -> Result<(), ReplayError> {
    let key = virtual_key(&keystroke.code)
        .ok_or_else(|| ReplayError::UnmappedKey(keystroke.code.clone()))?;

    let wanted = wanted_modifiers(keystroke);

    // Release whatever the user is still holding that this keystroke does not
    // want. `SendInput` has no absolute-flags concept, so the receiving app
    // reads the real keyboard and would see the extra modifier.
    let mut inputs: Vec<INPUT> = ALL_MODIFIERS
        .into_iter()
        .filter(|m| !wanted.contains(m) && key_is_down(*m))
        .map(|m| key_event(m, true))
        .collect();

    inputs.extend(wanted.iter().map(|m| key_event(*m, false)));
    inputs.push(key_event(key, false));
    inputs.push(key_event(key, true));
    // Release in reverse, the order a real keyboard would produce.
    inputs.extend(wanted.iter().rev().map(|m| key_event(*m, true)));

    send_inputs(&inputs)
}

/// The modifier keys this keystroke wants held.
///
/// Split out so the choice can be tested without a keyboard, the same way
/// `text.rs` separates `modifiers_to_clear`.
fn wanted_modifiers(keystroke: &Keystroke) -> Vec<VIRTUAL_KEY> {
    let mut wanted = Vec::new();
    if keystroke.control {
        wanted.push(VK_CONTROL);
    }
    if keystroke.alt {
        wanted.push(VK_MENU);
    }
    if keystroke.shift {
        wanted.push(VK_SHIFT);
    }
    // Tauri's `Super`/`Cmd` is the left Windows key here; there is no combined
    // code to press, so pick one.
    if keystroke.meta {
        wanted.push(VK_LWIN);
    }
    wanted
}

fn key_is_down(key: VIRTUAL_KEY) -> bool {
    // Safety: a pure query taking one virtual-key code.
    let state = unsafe { GetAsyncKeyState(i32::from(key)) };
    // The high bit means "currently down"; the low bit is a since-last-call
    // latch that would report keys already let go of.
    state < 0
}

fn key_event(key: VIRTUAL_KEY, up: bool) -> INPUT {
    // Safety: a pure lookup; an unmapped key returns 0, which we pass through.
    let scan = unsafe { MapVirtualKeyW(u32::from(key), MAPVK_VK_TO_VSC) };

    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: u16::try_from(scan).unwrap_or(0),
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send_inputs(inputs: &[INPUT]) -> Result<(), ReplayError> {
    let count = u32::try_from(inputs.len())
        .map_err(|_| ReplayError::Synthesis("too many key events".into()))?;
    let size = i32::try_from(std::mem::size_of::<INPUT>())
        .map_err(|_| ReplayError::Synthesis("unexpected input size".into()))?;

    // Safety: `inputs` points at `count` valid `INPUT` values, and `size` is
    // the size of that element type, which is what SendInput documents.
    let sent = unsafe { SendInput(count, inputs.as_ptr(), size) };
    if sent == count {
        return Ok(());
    }

    Err(ReplayError::Synthesis(format!(
        "SendInput accepted {sent} of {count} events: {}",
        std::io::Error::last_os_error()
    )))
}

/// `KeyboardEvent.code` name to the Windows virtual-key code.
///
/// Covers exactly the keys the hotkey recorder in `src/lib/hotkey.ts` can
/// produce. The `VK_OEM_*` codes are positional on a US layout, which is the
/// right level here — a chord prefix is the physical key that was pressed.
fn virtual_key(code: &str) -> Option<VIRTUAL_KEY> {
    // Letters and digits are contiguous and match their ASCII values.
    if let Some(letter) = code.strip_prefix("Key") {
        let mut chars = letter.chars();
        return match (chars.next(), chars.next()) {
            (Some(c @ 'A'..='Z'), None) => u16::try_from(u32::from(c)).ok(),
            _ => None,
        };
    }
    if let Some(digit) = code.strip_prefix("Digit") {
        let mut chars = digit.chars();
        return match (chars.next(), chars.next()) {
            (Some(c @ '0'..='9'), None) => u16::try_from(u32::from(c)).ok(),
            _ => None,
        };
    }
    if let Some(n) = code.strip_prefix('F') {
        // VK_F1 is 0x70 and the block runs unbroken to VK_F24.
        return match n.parse::<u16>() {
            Ok(n @ 1..=24) => n
                .checked_sub(1)
                .and_then(|offset| 0x70_u16.checked_add(offset)),
            _ => None,
        };
    }

    Some(match code {
        "Backspace" => 0x08,
        "Tab" => 0x09,
        "Enter" => 0x0D,
        "Escape" => 0x1B,
        "Space" => 0x20,
        "PageUp" => 0x21,
        "PageDown" => 0x22,
        "End" => 0x23,
        "Home" => 0x24,
        "ArrowLeft" => 0x25,
        "ArrowUp" => 0x26,
        "ArrowRight" => 0x27,
        "ArrowDown" => 0x28,
        "Delete" => 0x2E,

        // The OEM block, whose names say nothing about which key they are.
        "Semicolon" => 0xBA,   // VK_OEM_1
        "Equal" => 0xBB,       // VK_OEM_PLUS
        "Comma" => 0xBC,       // VK_OEM_COMMA
        "Minus" => 0xBD,       // VK_OEM_MINUS
        "Period" => 0xBE,      // VK_OEM_PERIOD
        "Slash" => 0xBF,       // VK_OEM_2
        "Backquote" => 0xC0,   // VK_OEM_3
        "BracketLeft" => 0xDB, // VK_OEM_4
        "Backslash" => 0xDC,   // VK_OEM_5
        "BracketRight" => 0xDD, // VK_OEM_6
        "Quote" => 0xDE,       // VK_OEM_7

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keystroke(code: &str) -> Keystroke {
        Keystroke {
            code: code.into(),
            meta: false,
            control: false,
            alt: false,
            shift: false,
        }
    }

    #[test]
    fn letters_and_digits_follow_their_ascii_values() {
        assert_eq!(virtual_key("KeyA"), Some(0x41));
        assert_eq!(virtual_key("KeyZ"), Some(0x5A));
        assert_eq!(virtual_key("Digit0"), Some(0x30));
        assert_eq!(virtual_key("Digit9"), Some(0x39));
    }

    #[test]
    fn the_function_key_block_is_contiguous() {
        assert_eq!(virtual_key("F1"), Some(0x70));
        assert_eq!(virtual_key("F12"), Some(0x7B));
        assert_eq!(virtual_key("F24"), Some(0x87));
        assert_eq!(virtual_key("F25"), None);
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
    fn near_misses_are_refused_rather_than_mapped_to_something_else() {
        assert_eq!(virtual_key("KeyAA"), None);
        assert_eq!(virtual_key("Key"), None);
        assert_eq!(virtual_key("Digit10"), None);
        assert_eq!(virtual_key("NotAKey"), None);
    }

    #[test]
    fn a_plain_key_wants_no_modifiers_held() {
        assert!(wanted_modifiers(&keystroke("KeyK")).is_empty());
    }

    #[test]
    fn each_flag_presses_its_own_modifier() {
        let mut k = keystroke("KeyK");
        k.control = true;
        k.shift = true;
        assert_eq!(wanted_modifiers(&k), vec![VK_CONTROL, VK_SHIFT]);
    }

    #[test]
    fn meta_presses_the_left_windows_key() {
        // There is no combined code the way there is for Ctrl and Shift.
        let mut k = keystroke("KeyK");
        k.meta = true;
        assert_eq!(wanted_modifiers(&k), vec![VK_LWIN]);
    }
}
