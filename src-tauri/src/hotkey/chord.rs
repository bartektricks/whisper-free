//! Two-step chords: `"Cmd+K K"`, the shape VS Code made familiar.
//!
//! The OS global-shortcut APIs (`RegisterEventHotKey`, `RegisterHotKey`) take
//! modifiers plus exactly one key, so a chord cannot be handed to them as one
//! registration. It is assembled here instead: the prefix stays registered, and
//! the second step is registered only for the moment between the prefix firing
//! and the window closing. See `docs/decisions/0003-two-step-hotkey-chords.md`.
//!
//! Everything in this file is pure. Registering, timing out and replaying live
//! in `dictation.rs`, so the rules can be tested without a window server.

use std::str::FromStr;
use std::time::Duration;

use tauri_plugin_global_shortcut::Shortcut;

use super::{HotkeyError, HotkeyEvent};

/// How long the second step has to arrive.
///
/// The prefix is swallowed system-wide while this runs, so it is a direct tax
/// on every other app's use of that combination — long enough to press two keys
/// deliberately, short enough that a mistaken prefix is handed back before the
/// user has given up on it.
pub const CHORD_TIMEOUT: Duration = Duration::from_millis(800);

/// One combination, parsed once so the hot path compares keys and not strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// The accelerator as written, for registering and for logs.
    pub accelerator: String,
    pub shortcut: Shortcut,
}

impl Step {
    fn parse(accelerator: &str) -> Result<Self, HotkeyError> {
        let shortcut = Shortcut::from_str(accelerator)
            .map_err(|_| HotkeyError::Invalid(accelerator.to_string()))?;
        Ok(Self {
            accelerator: accelerator.to_string(),
            shortcut,
        })
    }
}

/// A hotkey: one combination, or two pressed in sequence.
///
/// The single-step case is not a degenerate chord — it is the ordinary hotkey,
/// and it must keep behaving exactly as it did before chords existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub prefix: Step,
    pub second: Option<Step>,
}

impl Chord {
    /// Parse `"Alt+Space"` or `"Cmd+K K"`.
    ///
    /// Steps are separated by whitespace, which is both how VS Code writes them
    /// and something no single accelerator contains.
    ///
    /// # Errors
    ///
    /// [`HotkeyError::Invalid`] when a step does not parse or the string is
    /// empty, [`HotkeyError::TooManySteps`] beyond two, and
    /// [`HotkeyError::RepeatedStep`] when both steps are the same combination —
    /// there would be no way to tell the halves apart as they arrive.
    pub fn parse(accelerator: &str) -> Result<Self, HotkeyError> {
        let mut steps = accelerator.split_whitespace();

        let prefix = steps
            .next()
            .ok_or_else(|| HotkeyError::Invalid(accelerator.to_string()))?;
        let prefix = Step::parse(prefix)?;

        let Some(second) = steps.next() else {
            return Ok(Self {
                prefix,
                second: None,
            });
        };
        let second = Step::parse(second)?;

        if steps.next().is_some() {
            return Err(HotkeyError::TooManySteps(accelerator.to_string()));
        }
        if second.shortcut == prefix.shortcut {
            return Err(HotkeyError::RepeatedStep(accelerator.to_string()));
        }

        Ok(Self {
            prefix,
            second: Some(second),
        })
    }

    /// True when this hotkey has a second step, and so needs the arming dance.
    #[must_use]
    pub const fn is_chord(&self) -> bool {
        self.second.is_some()
    }
}

impl Step {
    /// This combination as a key press to synthesise.
    ///
    /// Only the prefix is ever replayed, and only when its chord was abandoned.
    #[must_use]
    pub fn keystroke(&self) -> crate::platform::Keystroke {
        use tauri_plugin_global_shortcut::Modifiers;

        let mods = self.shortcut.mods;
        crate::platform::Keystroke {
            code: self.shortcut.key.to_string(),
            // `HotKey::new` folds META into SUPER, so both have to be read or a
            // Cmd-prefixed chord would replay without its Command.
            meta: mods.intersects(Modifiers::META | Modifiers::SUPER),
            control: mods.contains(Modifiers::CONTROL),
            alt: mods.contains(Modifiers::ALT),
            shift: mods.contains(Modifiers::SHIFT),
        }
    }
}

/// Where a chord is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// Nothing in progress; only the prefix is registered.
    Closed,
    /// The prefix has fired and the second step is registered, waiting — on a
    /// clock, because the prefix has been taken from the app that wanted it.
    Open,
    /// The second step is pressed and still down.
    ///
    /// The clock stops here. A hold-to-talk recording runs for as long as the
    /// user holds the key, and its release is what ends it — but a release only
    /// arrives while the shortcut is still registered, so the timeout must not
    /// take it away underneath a recording that is still going.
    Held,
}

/// The live hotkey, and where any chord in progress has got to.
#[derive(Debug)]
pub struct Arming {
    chord: Chord,
    /// Bumped whenever the window opens or closes, so a timeout firing late can
    /// tell that the window it was started for has already gone — and not
    /// abandon a chord some later press has since armed.
    generation: u64,
    window: Window,
}

impl Arming {
    #[must_use]
    pub const fn new(chord: Chord) -> Self {
        Self {
            chord,
            generation: 0,
            window: Window::Closed,
        }
    }

    #[must_use]
    pub const fn chord(&self) -> &Chord {
        &self.chord
    }

    #[must_use]
    pub const fn window(&self) -> Window {
        self.window
    }

    /// Whether the second step is currently claimed from the OS.
    ///
    /// True in both open states, which is what stops a re-arm from registering
    /// a shortcut that is already registered.
    #[must_use]
    pub const fn holds_second_step(&self) -> bool {
        !matches!(self.window, Window::Closed)
    }

    /// Open the window, returning the generation the timeout should quote.
    pub const fn arm(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.window = Window::Open;
        self.generation
    }

    /// The second step is down: stop the clock, keep the registration.
    pub const fn hold(&mut self) {
        self.window = Window::Held;
    }

    /// Close the window, invalidating any timeout still in flight.
    pub const fn close(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.window = Window::Closed;
    }

    /// Whether a timeout quoting `generation` still speaks for the open window.
    ///
    /// False once the chord has been completed, which is what keeps the timeout
    /// from cutting a recording short.
    #[must_use]
    pub const fn still_open(&self, generation: u64) -> bool {
        matches!(self.window, Window::Open) && self.generation == generation
    }

    /// Replace the hotkey, abandoning any window that was open for the old one.
    pub fn set(&mut self, chord: Chord) {
        self.chord = chord;
        self.close();
    }
}

/// What a fired shortcut means for the chord machinery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordStep {
    /// Open the window: register the second step and start the timeout.
    Arm,
    /// This is the dictation trigger — hand the edge to [`super::decide`].
    Trigger,
    /// Trigger, and hold the window open until the second step is released.
    ///
    /// The registration has to outlive this press: hold-to-talk ends on the
    /// release, and a release only arrives while the shortcut is registered.
    TriggerAndHold,
    /// Trigger, and then close the window: stop stealing the second step.
    ///
    /// Without this a toggle-mode recording would leave a bare key swallowed
    /// system-wide for as long as it ran.
    TriggerAndClose,
    /// A repeat, a stray edge, or a step this hotkey does not use.
    Ignore,
}

/// Decide what a fired shortcut means, given the hotkey and where any chord in
/// progress has got to.
///
/// `window` is passed in rather than read here so this stays a pure function of
/// its inputs, in the same spirit as [`super::decide`].
#[must_use]
pub fn classify(chord: &Chord, fired: &Shortcut, edge: HotkeyEvent, window: Window) -> ChordStep {
    let Some(second) = chord.second.as_ref() else {
        // A plain hotkey: the prefix is the whole thing, both edges included.
        return if *fired == chord.prefix.shortcut {
            ChordStep::Trigger
        } else {
            ChordStep::Ignore
        };
    };

    if *fired == chord.prefix.shortcut {
        return match edge {
            // Re-arming rather than ignoring the repeat is deliberate: Windows
            // resends `WM_HOTKEY` while the key is held, and a user who holds
            // the prefix down for a moment should still get a full window for
            // the second step rather than a window that started at first touch.
            //
            // It is also the way out of a `Held` window whose release never
            // arrived, which would otherwise keep a bare key claimed for good.
            HotkeyEvent::Pressed => ChordStep::Arm,
            // Letting go of the prefix is how a chord is normally typed, so it
            // cannot mean anything on its own.
            HotkeyEvent::Released => ChordStep::Ignore,
        };
    }

    if *fired == second.shortcut && !matches!(window, Window::Closed) {
        return match edge {
            HotkeyEvent::Pressed => ChordStep::TriggerAndHold,
            HotkeyEvent::Released => ChordStep::TriggerAndClose,
        };
    }

    ChordStep::Ignore
}

#[cfg(test)]
mod tests {
    use super::ChordStep::*;
    use super::*;
    use crate::hotkey::HotkeyEvent::*;

    fn chord(accel: &str) -> Chord {
        Chord::parse(accel).expect("should parse")
    }

    fn key(accel: &str) -> Shortcut {
        Shortcut::from_str(accel).expect("should parse")
    }

    #[test]
    fn a_plain_accelerator_parses_as_a_single_step() {
        let c = chord("Alt+Space");
        assert!(!c.is_chord());
        assert_eq!(c.second, None);
        assert_eq!(c.prefix.accelerator, "Alt+Space");
    }

    #[test]
    fn two_accelerators_separated_by_a_space_parse_as_a_chord() {
        let c = chord("Cmd+K K");
        assert!(c.is_chord());
        assert_eq!(c.prefix.shortcut, key("Cmd+K"));
        assert_eq!(c.second.unwrap().shortcut, key("K"));
    }

    #[test]
    fn a_second_step_may_carry_its_own_modifiers() {
        // `Cmd+K Cmd+C` is the other shape VS Code uses, and the one that
        // works without letting go of the modifier between steps.
        let c = chord("Cmd+K Cmd+C");
        assert_eq!(c.second.unwrap().shortcut, key("Cmd+C"));
    }

    #[test]
    fn three_steps_are_refused() {
        assert!(matches!(
            Chord::parse("Cmd+K K J"),
            Err(HotkeyError::TooManySteps(_))
        ));
    }

    #[test]
    fn a_chord_of_one_combination_twice_is_refused() {
        // Both halves arrive as the same registration, so there would be no
        // way to know which one just fired.
        assert!(matches!(
            Chord::parse("Cmd+K Cmd+K"),
            Err(HotkeyError::RepeatedStep(_))
        ));
    }

    #[test]
    fn nonsense_in_either_step_is_refused() {
        assert!(Chord::parse("").is_err());
        assert!(Chord::parse("Cmd+K NotAKey").is_err());
        assert!(Chord::parse("NotAKey K").is_err());
    }

    #[test]
    fn surrounding_whitespace_does_not_invent_a_second_step() {
        assert!(!chord("  Alt+Space  ").is_chord());
    }

    #[test]
    fn a_plain_hotkey_triggers_on_both_edges() {
        let c = chord("Alt+Space");
        let closed = Window::Closed;
        assert_eq!(classify(&c, &key("Alt+Space"), Pressed, closed), Trigger);
        assert_eq!(classify(&c, &key("Alt+Space"), Released, closed), Trigger);
    }

    #[test]
    fn a_chord_prefix_arms_instead_of_recording() {
        // The whole point: pressing ⌘K must not start dictation, or the second
        // step would be decoration.
        let c = chord("Cmd+K K");
        assert_eq!(classify(&c, &key("Cmd+K"), Pressed, Window::Closed), Arm);
    }

    #[test]
    fn releasing_the_chord_prefix_does_nothing() {
        let c = chord("Cmd+K K");
        assert_eq!(classify(&c, &key("Cmd+K"), Released, Window::Open), Ignore);
    }

    #[test]
    fn holding_the_prefix_re_arms_rather_than_starting_the_clock_once() {
        let c = chord("Cmd+K K");
        assert_eq!(classify(&c, &key("Cmd+K"), Pressed, Window::Open), Arm);
    }

    #[test]
    fn the_prefix_is_the_way_out_of_a_held_window() {
        // If the second step's release were ever missed, the window would stay
        // Held and a bare key would be claimed for good. Pressing the prefix
        // again re-arms, and the fresh timeout releases it.
        let c = chord("Cmd+K K");
        assert_eq!(classify(&c, &key("Cmd+K"), Pressed, Window::Held), Arm);
    }

    #[test]
    fn the_second_step_triggers_only_once_the_window_is_open() {
        let c = chord("Cmd+K K");
        assert_eq!(
            classify(&c, &key("K"), Pressed, Window::Open),
            TriggerAndHold
        );
        // Closed it is not even registered, so this is belt and braces.
        assert_eq!(classify(&c, &key("K"), Pressed, Window::Closed), Ignore);
    }

    #[test]
    fn releasing_the_second_step_triggers_and_closes_the_window() {
        // Hold-to-talk needs the release to stop the recording, and nothing
        // needs the second step registered afterwards.
        let c = chord("Cmd+K K");
        assert_eq!(
            classify(&c, &key("K"), Released, Window::Held),
            TriggerAndClose
        );
    }

    #[test]
    fn a_repeat_of_a_held_second_step_still_reports_it_as_held() {
        // macOS resends Pressed while a key is down; `decide` ignores those,
        // but they must not knock the window out of Held.
        let c = chord("Cmd+K K");
        assert_eq!(
            classify(&c, &key("K"), Pressed, Window::Held),
            TriggerAndHold
        );
    }

    #[test]
    fn an_unrelated_shortcut_is_ignored_in_both_shapes() {
        assert_eq!(
            classify(&chord("Alt+Space"), &key("Cmd+J"), Pressed, Window::Closed),
            Ignore
        );
        assert_eq!(
            classify(&chord("Cmd+K K"), &key("Cmd+J"), Pressed, Window::Open),
            Ignore
        );
    }

    #[test]
    fn arming_and_closing_invalidate_an_earlier_timeout() {
        let mut arming = Arming::new(chord("Cmd+K K"));
        assert!(!arming.holds_second_step());

        let first = arming.arm();
        assert!(arming.still_open(first));

        // The user let the window lapse and pressed the prefix again: the first
        // timeout must not abandon the second window when it wakes up.
        let second = arming.arm();
        assert!(!arming.still_open(first));
        assert!(arming.still_open(second));

        arming.close();
        assert!(!arming.holds_second_step());
        assert!(!arming.still_open(second));
    }

    #[test]
    fn a_held_second_step_survives_its_own_timeout() {
        // The hold-to-talk bug: the window stayed open while the key was down,
        // so the 800 ms timeout unregistered the second step mid-recording and
        // the release that ends the recording never arrived.
        let mut arming = Arming::new(chord("Cmd+K K"));
        let generation = arming.arm();

        arming.hold();

        assert!(
            !arming.still_open(generation),
            "the timeout would cut the recording short"
        );
        assert!(
            arming.holds_second_step(),
            "the release that ends the recording needs the shortcut registered"
        );
    }

    #[test]
    fn closing_a_held_window_still_gives_the_second_step_back() {
        let mut arming = Arming::new(chord("Cmd+K K"));
        arming.arm();
        arming.hold();

        arming.close();

        assert!(!arming.holds_second_step());
        assert_eq!(arming.window(), Window::Closed);
    }

    #[test]
    fn changing_the_hotkey_abandons_a_window_open_for_the_old_one() {
        let mut arming = Arming::new(chord("Cmd+K K"));
        let generation = arming.arm();

        arming.set(chord("Alt+Space"));

        assert!(!arming.still_open(generation));
        assert!(!arming.chord().is_chord());
    }

    #[test]
    fn a_prefix_replays_with_the_modifiers_it_was_recorded_with() {
        let c = chord("Cmd+Shift+K K");
        let keystroke = c.prefix.keystroke();
        assert_eq!(keystroke.code, "KeyK");
        assert!(keystroke.meta, "Cmd was dropped from the replay");
        assert!(keystroke.shift);
        assert!(!keystroke.control);
        assert!(!keystroke.alt);
    }

    #[test]
    fn a_prefix_with_no_modifiers_replays_as_a_bare_key() {
        let c = chord("F13 K");
        let keystroke = c.prefix.keystroke();
        assert_eq!(keystroke.code, "F13");
        assert!(!keystroke.meta && !keystroke.control && !keystroke.alt && !keystroke.shift);
    }

    #[test]
    fn a_full_chord_walks_arm_then_hold_then_close() {
        // Driven through `Arming` rather than with hand-written window states,
        // so the two halves cannot drift apart.
        let mut arming = Arming::new(chord("Cmd+K K"));
        let c = arming.chord().clone();
        let mut steps = Vec::new();

        for (fired, edge) in [
            (key("Cmd+K"), Pressed),
            (key("K"), Pressed),
            (key("K"), Released),
        ] {
            let step = classify(&c, &fired, edge, arming.window());
            match step {
                Arm => {
                    arming.arm();
                }
                TriggerAndHold => arming.hold(),
                TriggerAndClose => arming.close(),
                _ => {}
            }
            steps.push(step);
        }

        assert_eq!(steps, [Arm, TriggerAndHold, TriggerAndClose]);
        assert!(!arming.holds_second_step(), "the second step was left claimed");
    }

    #[test]
    fn a_hold_that_outlasts_the_window_still_ends_on_release() {
        // The whole hold-to-talk failure, start to finish: the timeout fires
        // while the key is still down and must change nothing.
        let mut arming = Arming::new(chord("Cmd+K K"));
        let c = arming.chord().clone();

        let generation = arming.arm();
        assert_eq!(
            classify(&c, &key("K"), Pressed, arming.window()),
            TriggerAndHold
        );
        arming.hold();

        // 800 ms passes with the key still down.
        assert!(!arming.still_open(generation), "the timeout would fire");

        assert_eq!(
            classify(&c, &key("K"), Released, arming.window()),
            TriggerAndClose,
            "the release that stops the recording was lost"
        );
    }
}
