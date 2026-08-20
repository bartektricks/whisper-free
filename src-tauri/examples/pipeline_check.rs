//! Developer check for the transcription pipeline (plan milestones 5, 7, 8).
//!
//! Installs the model if needed (downloading only what is missing), loads it
//! through the `SpeechRecognizer` boundary, transcribes a WAV file, and runs
//! the result through a dictionary.
//!
//! Kept out of `cargo test` because it needs ~671 MB on disk and a network
//! connection the first time.
//!
//! Run with: `cargo run --release --example pipeline_check <file.wav>...`

// A developer diagnostic, not shipped code. Parsing a WAV header is byte
// indexing and integer arithmetic by nature, and a command-line tool reports a
// bad file by exiting non-zero. The crate itself stays strict.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::exit,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used
)]

use std::path::PathBuf;

use whisper_free_lib::asr::{AudioBuffer, SpeechRecognizer, TranscriptionOptions};
use whisper_free_lib::dictionary::Dictionary;
use whisper_free_lib::models::{download, ModelStore};

/// The same directory the app uses, without a Tauri app to ask.
///
/// This deliberately duplicates Tauri's resolution rather than starting an app
/// just to read a path — but it has to stay in step with it, so a model
/// installed here is the one the app finds.
fn data_dir() -> PathBuf {
    const BUNDLE_ID: &str = "com.bartek.whisperfree";

    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").expect("HOME is not set");
        PathBuf::from(home)
            .join("Library/Application Support")
            .join(BUNDLE_ID)
    }

    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").expect("APPDATA is not set");
        PathBuf::from(appdata).join(BUNDLE_ID)
    }
}

/// Minimal 16-bit mono WAV reader — enough for the sample files.
fn read_wav(path: &PathBuf) -> Result<AudioBuffer, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a RIFF/WAVE file".into());
    }

    let u16_at = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);

    let mut pos = 12;
    let (mut rate, mut channels, mut bits) = (0u32, 0u16, 0u16);
    let mut samples = Vec::new();

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32_at(pos + 4) as usize;
        let body = pos + 8;

        if id == b"fmt " {
            channels = u16_at(body + 2);
            rate = u32_at(body + 4);
            bits = u16_at(body + 14);
        } else if id == b"data" {
            if bits != 16 {
                return Err(format!("expected 16-bit samples, found {bits}"));
            }
            let end = (body + size).min(bytes.len());
            // `as_chunks` rather than `chunks_exact(2)`: the const-generic
            // form hands back `&[u8; 2]`, which `from_le_bytes` takes whole.
            // Both drop a trailing odd byte, so a truncated `data` chunk is
            // handled the same way.
            samples = bytes[body..end]
                .as_chunks::<2>()
                .0
                .iter()
                // i16::MAX, matching how transcribe-rs scales WAV input.
                .map(|c| f32::from(i16::from_le_bytes(*c)) / f32::from(i16::MAX))
                .collect();
        }
        pos = body + size + (size & 1);
    }

    if channels != 1 {
        return Err(format!("expected mono audio, found {channels} channels"));
    }
    Ok(AudioBuffer::new(samples, rate))
}

fn main() {
    let files: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();
    if files.is_empty() {
        eprintln!("usage: pipeline_check <file.wav>...");
        std::process::exit(2);
    }

    let store = ModelStore::new(&data_dir());
    let descriptor = whisper_free_lib::models::find("parakeet-tdt-0.6b-v3").unwrap();

    println!("Model store: {}", store.root().display());
    if store.is_installed(descriptor) {
        println!("Model already installed.\n");
    } else {
        println!(
            "Installing {} ({:.0} MB missing)...",
            descriptor.name,
            (descriptor.total_bytes() - store.bytes_on_disk(descriptor)) as f64 / 1e6
        );
        let cancel = download::CancelFlag::new();
        let mut last_pct = -1i64;
        let result = download::install(&store, descriptor, &cancel, |p| {
            let pct = (p.fraction() * 100.0) as i64;
            if pct != last_pct {
                last_pct = pct;
                println!("  {pct:3}%  {}", p.file);
            }
        });
        match result {
            Ok(()) => println!("Installed and every file verified against its SHA-256.\n"),
            Err(e) => {
                eprintln!("install failed: {e}");
                std::process::exit(1);
            }
        }
    }

    let mut recognizer = whisper_free_lib::asr::parakeet::ParakeetRecognizer::new(
        descriptor.id,
        store.dir_for(descriptor.id),
        descriptor.languages(),
        descriptor.capabilities.to_vec(),
    );

    let started = std::time::Instant::now();
    if let Err(e) = recognizer.load() {
        eprintln!("load failed: {e}");
        std::process::exit(1);
    }
    println!("Model loaded in {} ms\n", started.elapsed().as_millis());

    // The plan's own dictionary example, plus a Polish one.
    let mut dictionary = Dictionary::default();
    dictionary.add("cotlin", "Kotlin").unwrap();
    dictionary.add("type script", "TypeScript").unwrap();
    dictionary.add("americans", "Americans").unwrap();

    for path in &files {
        let audio = match read_wav(path) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                continue;
            }
        };

        match recognizer.transcribe(&audio, &TranscriptionOptions::default()) {
            Ok(t) => {
                println!("{}", path.display());
                println!(
                    "  {:.2}s audio -> {} ms  (RTF {:.3})",
                    t.audio_duration.as_secs_f64(),
                    t.duration.as_millis(),
                    t.real_time_factor()
                );
                if t.is_empty() {
                    println!("  EMPTY RESULT (surfaced to the user, not swallowed)");
                } else {
                    println!("  asr        : {}", t.text);
                    let corrected = dictionary.apply(&t.text);
                    if corrected != t.text {
                        println!("  dictionary : {corrected}");
                    }
                }
                println!();
            }
            Err(e) => println!("{}: transcription failed: {e}\n", path.display()),
        }
    }
}
