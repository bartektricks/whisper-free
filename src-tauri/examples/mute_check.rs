//! Developer check for the output-muting path (decision 0009).
//!
//! Silences whatever the machine is playing, waits, and puts it back. Kept out
//! of the test suite on purpose: it changes a real device's state, which is not
//! something `cargo test` should be doing on the machine running it.
//!
//! Play something audible first, or there is nothing to observe.
//!
//! Run with: `cargo run --example mute_check [seconds]`

use std::time::Duration;

use whisper_free_lib::platform;

fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);

    println!("Muting the default output device…");
    let Some(mute) = platform::mute_system_output() else {
        println!(
            "Nothing to do. Either there is no output device, it offers no way \
             to silence it, or it was already muted. Run with \
             WHISPER_FREE_LOG=whisper_free_lib=debug to see which."
        );
        return;
    };

    println!("Muted. Restoring in {seconds}s.");
    std::thread::sleep(Duration::from_secs(seconds));

    platform::restore_system_output(mute);
    println!("Restored. Sound should be exactly as you left it.");
}
