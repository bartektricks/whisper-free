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
//!
//! Defaults to Parakeet with automatic language detection. To drive another
//! model, or a model that has to be told what it is listening to:
//!
//! ```sh
//! cargo run --release --example pipeline_check -- \
//!     --model canary-180m-flash --language en clip.wav
//! ```

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

use whisper_free_lib::asr::onnx::{OnnxEngine, OnnxRecognizer};
use whisper_free_lib::asr::{
    AudioBuffer, LanguageSelection, SpeechRecognizer, TranscriptionOptions,
};
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

struct Args {
    files: Vec<PathBuf>,
    model_id: String,
    language: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        files: Vec::new(),
        model_id: "parakeet-tdt-0.6b-v3".to_string(),
        language: None,
    };

    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--model" => args.model_id = raw.next().unwrap_or_default(),
            "--language" => args.language = raw.next(),
            _ => args.files.push(PathBuf::from(arg)),
        }
    }

    if args.files.is_empty() {
        eprintln!("usage: pipeline_check [--model <id>] [--language <code>] <file.wav>...");
        eprintln!("\nmodels:");
        for m in whisper_free_lib::models::of_kind(whisper_free_lib::models::ModelKind::Speech) {
            eprintln!("  {:<20} {}", m.id, m.name);
        }
        std::process::exit(2);
    }
    args
}

fn install_if_missing(store: &ModelStore, descriptor: &whisper_free_lib::models::ModelDescriptor) {
    if store.is_installed(descriptor) {
        println!("Model already installed.\n");
        return;
    }

    println!(
        "Installing {} ({:.0} MB missing)...",
        descriptor.name,
        (descriptor.total_bytes() - store.bytes_on_disk(descriptor)) as f64 / 1e6
    );
    let cancel = download::CancelFlag::new();
    let mut last_pct = -1i64;
    let result = download::install(store, descriptor, &cancel, |p| {
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

fn main() {
    let Args {
        files,
        model_id,
        language,
    } = parse_args();

    let store = ModelStore::new(&data_dir());
    let Some(descriptor) = whisper_free_lib::models::find(&model_id) else {
        eprintln!("unknown model \"{model_id}\"");
        std::process::exit(2);
    };

    // The same rule Settings applies, so the example cannot ask a model for
    // something the app would never ask it for.
    let selection = whisper_free_lib::models::normalise_language(
        descriptor,
        language.map_or(LanguageSelection::Auto, LanguageSelection::Fixed),
    );
    println!("Language: {selection:?}");

    println!("Model store: {}", store.root().display());
    install_if_missing(&store, descriptor);

    let engine = match descriptor.engine {
        whisper_free_lib::models::EngineKind::Parakeet => OnnxEngine::Parakeet,
        whisper_free_lib::models::EngineKind::Canary => OnnxEngine::Canary,
        whisper_free_lib::models::EngineKind::RefinerOnnx => {
            eprintln!("{} is a cleanup model and cannot transcribe", descriptor.id);
            std::process::exit(2);
        }
    };
    let mut recognizer = OnnxRecognizer::new(
        descriptor.id,
        engine,
        store.dir_for(descriptor.id),
        descriptor.languages(),
        descriptor.capabilities.to_vec(),
        descriptor.files.iter().map(|f| f.name.to_string()).collect(),
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

        let options = TranscriptionOptions {
            language: selection.clone(),
        };
        match recognizer.transcribe(&audio, &options) {
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
