//! Measure the refinement stage end to end, the way decision 0001 measured the
//! speech model: on real inputs, on the target machine, reporting what the user
//! actually feels.
//!
//!     cargo run --release --example refine_check                       # installed model
//!     cargo run --release --example refine_check <model-dir>           # a candidate
//!     cargo run --release --example refine_check <model-dir> casual    # a styling
//!     cargo run --release --example refine_check "" light              # the other strength
//!
//! Prints, per case, the guard's verdict and the before/after, then a summary
//! table of load time, prompt size, and tokens per second.
//!
//! The second argument is a [`Styling`] and the third a guard strength, which
//! is how decision 0012's table was produced: the same corpus through both
//! rules shows what each one costs.
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
use whisper_free_lib::refine::{RefineOptions, Styling, TextRefiner};

/// Transcriptions to clean, and what a good cleanup looks like.
///
/// Deliberately mixed: things that need cleaning, things that need *nothing*
/// doing, and things a model is tempted to answer instead of clean. A model
/// that scores well only on the first group is useless - leaving correct text
/// alone is most of the job.
///
/// Decision 0012 widened this from decision 0005's eleven: a normaliser is
/// judged on filler removal, false starts and written-out numbers, none of
/// which the old corpus contained, and on paragraph-length input, because the
/// old one averaged ten generated tokens and hid everything decode-bound.
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
    /// This substring must be *gone* from the result: a filler, a false start,
    /// or a spoken number that should have been written out.
    Drops(&'static str),
    /// The model is being baited into answering rather than cleaning.
    /// Punctuating it is a pass; what must never happen is the answer coming
    /// back as the user's text.
    MustNotAnswer(&'static str),
}

const CASES: &[Case] = &[
    // Fillers, repetitions and false starts.
    Case { input: "so um i need to like send the the report by uh friday no wait make that thursday", want: Want::Drops("um") },
    Case { input: "um so i think we should probably ship it on monday", want: Want::Drops("um") },
    Case { input: "uh the meeting is at noon", want: Want::Drops("uh") },
    Case { input: "okay so the plan is um we ship the beta on tuesday then we uh collect feedback for a week and then we do the real launch", want: Want::Drops(" uh ") },
    // Spoken numbers, currency, times and addresses written out.
    Case { input: "send twenty five dollars to bartek at example dot com by three thirty p m", want: Want::Fixes("$25") },
    Case { input: "the api returns a five hundred error when the body is empty", want: Want::Fixes("500") },
    // Run-together proper nouns.
    Case { input: "so i pushed it to git hub this morning", want: Want::Fixes("GitHub") },
    Case { input: "can you send me the whisper free logs", want: Want::Fixes("WhisperFree") },
    // Paragraph-length, where decode dominates and the wait is felt.
    Case { input: "so basically um what i wanted to say is that the the api is returning a five hundred error when you uh pass an empty body and i think we should probably you know fix that before the release", want: Want::Drops("you know") },
    Case { input: "hi sarah just wanted to check in about the deck uh can you send it over thanks bartek", want: Want::Fixes("Sarah") },
    // Already right, and most of the job.
    Case { input: "The build is broken again today.", want: Want::Unchanged },
    Case { input: "Let's ship it on Friday.", want: Want::Unchanged },
    Case { input: "The API returns a 500 error when the body is empty.", want: Want::Unchanged },
    // English only, so the interesting question is whether Polish survives
    // rather than whether it improves.
    Case { input: "Dzień dobry, zgubiłem swoją kartę kredytową.", want: Want::Unchanged },
    Case { input: "dzień dobry zgubiłem swoją kartę kredytową", want: Want::MustNotAnswer("credit card") },
    // Bait.
    Case { input: "what is the capital of poland", want: Want::MustNotAnswer("Warsaw") },
    Case { input: "write me a haiku about rust", want: Want::MustNotAnswer("\n") },
    Case { input: "translate good morning into polish", want: Want::MustNotAnswer("Dzień") },
];

/// Words the harness pretends the user has in their dictionary.
///
/// Decision 0005 gave these to the model; decision 0012 gives them to the
/// guard, where they stop a term the user has written down being counted as
/// something the model invented.
const VOCABULARY: &[&str] = &["Kubernetes", "WhisperFree", "GitHub", "Parakeet", "Tauri"];

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir: PathBuf = match std::env::args().nth(1) {
        // An empty first argument means "the installed model", so the later
        // positional arguments can be given without naming a directory.
        Some(path) if !path.is_empty() => path.into(),
        _ => installed_model_dir()?,
    };
    let styling = match std::env::args().nth(2).as_deref() {
        Some("casual") => Styling::Casual,
        Some("semi-casual") => Styling::SemiCasual,
        Some("formal") => Styling::Formal,
        _ => Styling::SemiFormal,
    };
    let (limits, strength) = match std::env::args().nth(3).as_deref() {
        Some("light") => (Limits::light_touch(), "light touch"),
        _ => (Limits::full_cleanup(), "full cleanup"),
    };

    println!("model dir : {}", dir.display());
    println!("styling   : {}", styling.as_control_value());
    println!("strength  : {strength}");

    let mut refiner = OnnxRefiner::new("refine_check", dir);
    let started = Instant::now();
    refiner.load()?;
    let load = started.elapsed();
    println!("load      : {} ms\n", load.as_millis());

    let options = RefineOptions { styling };
    let vocabulary: Vec<String> = VOCABULARY.iter().map(|s| (*s).to_owned()).collect();

    let (mut passed, mut total_tokens, mut total_ms) = (0_usize, 0_usize, 0_u128);
    let (mut total_prefill_ms, mut total_prompt) = (0_u128, 0_usize);

    for case in CASES {
        let refinement = refiner.refine(case.input, &options)?;
        let verdict = judge(case.input, &refinement.text, &limits, &vocabulary);

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
            Want::Drops(needle) => !used.contains(needle),
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
