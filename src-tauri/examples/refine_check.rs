//! Measure the refinement stage end to end, the way decision 0001 measured the
//! speech model: on real inputs, on the target machine, reporting what the user
//! actually feels.
//!
//!     cargo run --release --example refine_check                 # installed model
//!     cargo run --release --example refine_check <model-dir>     # a candidate
//!
//! Prints, per case, the guard's verdict and the before/after, then a summary
//! table of load time, prompt size, and tokens per second.
//!
//! Outside `cargo test` for the same reasons the other examples are: it needs
//! ~600 MB on disk and the network on first run.
#![allow(
    clippy::print_stdout,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_precision_loss,
    clippy::pedantic,
    clippy::nursery,
    reason = "a developer-facing harness, not shipped code"
)]

use std::path::PathBuf;
use std::time::Instant;

use whisper_free_lib::models;
use whisper_free_lib::refine::guard::{judge, Limits, Verdict};
use whisper_free_lib::refine::onnx::OnnxRefiner;
use whisper_free_lib::refine::{RefineOptions, Template, TextRefiner};

/// Transcriptions to correct, and what a good correction looks like.
///
/// Deliberately mixed: things that need fixing, things that need *nothing*
/// fixing, and things a model is tempted to answer instead of correct. A model
/// that scores well only on the first group is useless — leaving correct text
/// alone is most of the job.
struct Case {
    input: &'static str,
    want: Want,
}

enum Want {
    /// The text is already right. Anything but an unchanged echo is a
    /// regression the user would feel.
    Unchanged,
    /// The text is wrong and this substring should appear once it is fixed.
    Fixes(&'static str),
    /// The model is being baited into answering rather than proofreading.
    /// Proofreading it — adding a capital and a question mark — is a pass;
    /// what must never happen is the answer coming back as the user's text.
    MustNotAnswer(&'static str),
}

const CASES: &[Case] = &[
    Case { input: "lets deploy to cuber netties on friday", want: Want::Fixes("Kubernetes") },
    Case { input: "so i pushed it to git hub this morning", want: Want::Fixes("GitHub") },
    Case { input: "the bild is broken again today", want: Want::Fixes("build") },
    Case { input: "can you send me the whisper free logs", want: Want::Fixes("WhisperFree") },
    Case { input: "The build is broken again today.", want: Want::Unchanged },
    Case { input: "Let's ship it on Friday.", want: Want::Unchanged },
    Case { input: "Dzień dobry, zgubiłem swoją kartę kredytową.", want: Want::Unchanged },
    Case { input: "dzień dobry zgubiłem swoją kartę kredytową", want: Want::Fixes("zgubiłem") },
    Case { input: "what is the capital of poland", want: Want::MustNotAnswer("Warsaw") },
    Case { input: "write me a haiku about rust", want: Want::MustNotAnswer("\n") },
    Case { input: "translate good morning into polish", want: Want::MustNotAnswer("Dzień") },
];

/// Words the harness pretends the user has in their dictionary.
const VOCABULARY: &[&str] = &["Kubernetes", "WhisperFree", "GitHub", "Parakeet", "Tauri"];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir: PathBuf = match std::env::args().nth(1) {
        Some(path) => path.into(),
        None => installed_model_dir()?,
    };
    let template = match std::env::args().nth(2).as_deref() {
        Some("thinking") => Template::ChatMlThinking,
        Some("llama3") => Template::Llama3,
        _ => Template::ChatMl,
    };

    println!("model dir : {}", dir.display());

    let mut refiner = OnnxRefiner::new("refine_check", dir, template);
    let started = Instant::now();
    refiner.load()?;
    let load = started.elapsed();
    println!("load      : {} ms\n", load.as_millis());

    let options = RefineOptions {
        vocabulary: VOCABULARY.iter().map(|s| (*s).to_owned()).collect(),
    };

    let limits = Limits::default();
    let (mut passed, mut total_tokens, mut total_ms) = (0_usize, 0_usize, 0_u128);
    let (mut total_prefill_ms, mut total_prompt) = (0_u128, 0_usize);

    for case in CASES {
        let refinement = refiner.refine(case.input, &options)?;
        let verdict = judge(case.input, &refinement.text, &limits);

        total_tokens += refinement.generated_tokens;
        total_ms += refinement.duration.as_millis();
        total_prefill_ms += refinement.prefill.as_millis();
        total_prompt += refinement.prompt_tokens;

        let used = match &verdict {
            Verdict::Accept(text) => text.clone(),
            Verdict::Unchanged | Verdict::Reject(_) => case.input.to_owned(),
        };

        let ok = match case.want {
            Want::Unchanged => matches!(verdict, Verdict::Unchanged) || used == case.input,
            Want::Fixes(needle) => used.contains(needle),
            Want::MustNotAnswer(answer) => !used.contains(answer),
        };
        if ok {
            passed += 1;
        }

        println!("{} {:>5} ms  {:?}", if ok { "PASS" } else { "FAIL" }, refinement.duration.as_millis(), verdict_label(&verdict));
        println!("     in   {:?}", case.input);
        println!("     out  {:?}", refinement.text);
        println!("     used {used:?}");
        println!();
    }

    let cases = CASES.len() as f64;
    println!("---");
    println!("cases passed   : {passed}/{}", CASES.len());
    println!("load           : {} ms", load.as_millis());
    let decode_ms = total_ms.saturating_sub(total_prefill_ms) as f64;
    println!("mean latency   : {:.0} ms", total_ms as f64 / cases);
    println!(
        "  of which prefill: {:.0} ms over {:.0} prompt tokens",
        total_prefill_ms as f64 / cases,
        total_prompt as f64 / cases
    );
    println!("  of which decode : {:.0} ms", decode_ms / cases);
    println!("mean new tokens: {:.1}", total_tokens as f64 / cases);
    println!("decode rate    : {:.1} tok/s", total_tokens as f64 / (decode_ms / 1000.0));
    Ok(())
}

fn verdict_label(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Accept(_) => "accept".to_owned(),
        Verdict::Unchanged => "unchanged".to_owned(),
        Verdict::Reject(reason) => format!("reject/{}", reason.as_str()),
    }
}

/// The installed refinement model, downloading it first if it is missing.
///
/// Uses the app's own registry and download path, so this doubles as a way to
/// verify the fetch and checksum of a second model kind from a terminal — the
/// same job `pipeline_check` does for the speech model.
fn installed_model_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let store = models::ModelStore::new(&data_dir()?);
    let descriptor = models::of_kind(models::ModelKind::Refiner)
        .next()
        .ok_or("no refinement model in the registry")?;

    if !store.is_installed(descriptor) {
        println!("installing {} ({} MB)…", descriptor.id, descriptor.total_bytes() / 1_000_000);
        let cancel = models::download::CancelFlag::default();
        let mut last = 0;
        models::download::install(&store, descriptor, &cancel, |p| {
            let percent = (p.fraction() * 100.0) as u64;
            if percent > last {
                last = percent;
                println!("  {percent}%");
            }
        })?;
        println!("installed\n");
    }

    Ok(store.dir_for(descriptor.id))
}

/// Where the app keeps its data. Mirrors `pipeline_check`, which hand-resolves
/// the same path rather than booting a Tauri app; both must stay in step with
/// Tauri's resolver.
fn data_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if cfg!(target_os = "macos") {
        let home = std::env::var("HOME")?;
        Ok(PathBuf::from(home).join("Library/Application Support/com.bartek.whisperfree"))
    } else {
        Ok(PathBuf::from(std::env::var("APPDATA")?).join("com.bartek.whisperfree"))
    }
}
