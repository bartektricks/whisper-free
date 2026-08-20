//! Developer check for the capture path (plan milestone 2).
//!
//! Lists input devices, records for a few seconds, and reports what arrived.
//! Kept out of the test suite on purpose: it needs a real microphone and a
//! granted permission, neither of which belongs in `cargo test`.
//!
//! Run with: `cargo run --example mic_check [seconds]`

// A diagnostic, not shipped code: it prints numbers about audio, so the casts
// and the arithmetic are the point. The crate itself stays strict.
#![allow(clippy::as_conversions, clippy::cast_precision_loss)]

use std::time::Duration;

use whisper_free_lib::audio::AudioEngine;

fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);

    let engine = AudioEngine::spawn();

    println!("Input devices:");
    match engine.list_devices() {
        Ok(devices) => {
            for d in &devices {
                let marker = if d.is_default { "*" } else { " " };
                println!("  {marker} {}  [{}]", d.name, d.id);
            }
        }
        Err(e) => {
            println!("  could not list devices: {e}");
            return;
        }
    }

    println!("\nRecording for {seconds}s from the default device — say something.");
    if let Err(e) = engine.start(None) {
        println!("start failed: {e}");
        println!("user-facing message: {}", e.user_message());
        return;
    }

    std::thread::sleep(Duration::from_secs(seconds));

    match engine.stop() {
        Ok(buffer) => {
            let peak = buffer.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let rms = (buffer.samples.iter().map(|s| s * s).sum::<f32>()
                / buffer.samples.len().max(1) as f32)
                .sqrt();

            println!("\nCaptured:");
            println!("  duration    : {:.2}s", buffer.duration().as_secs_f64());
            println!("  sample rate : {} Hz", buffer.sample_rate);
            println!("  samples     : {}", buffer.samples.len());
            println!("  peak        : {peak:.4}");
            println!("  rms         : {rms:.4}");

            let expected = seconds as f64 * f64::from(buffer.sample_rate);
            let drift = (buffer.samples.len() as f64 - expected).abs() / expected;
            println!(
                "  length      : {} (within {:.1}% of {seconds}s)",
                if drift < 0.05 { "OK" } else { "OFF" },
                drift * 100.0
            );

            if peak < 0.0005 {
                println!(
                    "\n  SILENT — microphone permission is likely denied, or the wrong \
                     device is selected."
                );
            } else {
                println!("\n  Audio is valid.");
            }
        }
        Err(e) => println!("stop failed: {e}"),
    }
}
