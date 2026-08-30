//! The refinement model, on ONNX Runtime (decisions 0005 and 0012).
//!
//! The only file that may name `ort`, `tokenizers`, a graph input, or the KV
//! cache. Everything above this sees [`TextRefiner`].
//!
//! The runtime is the one already linked into the binary for the speech model,
//! so nothing new ships and behaviour is identical on every platform. What is
//! written here rather than pulled from a crate is the decode loop: prefill the
//! prompt, take the last position's logits, pick the highest, feed it back with
//! the cache the graph just returned. Greedy, so there is no sampler and no
//! randomness - the same transcription refines to the same text every time,
//! which is what makes this testable at all.
//!
//! Three things about the S1-mini export are not what decision 0005 found on
//! the Qwen2.5 one, and all three are load-bearing:
//!
//! - **It has no `position_ids` input.** Positions are derived inside the graph
//!   from `attention_mask`. Feeding one is an unknown-input error, so which
//!   optional inputs exist is read off the graph at load rather than assumed.
//! - **It requires `num_logits_to_keep`.** Setting it to 1 is not a formality:
//!   the language-model head is 151 936 x 1 024, and without it that runs over
//!   every prompt position instead of the one whose logits we read.
//! - **It uses `com.microsoft.GroupQueryAttention`**, which 0005 recorded the
//!   Qwen2.5 export did not. It declares no extra graph inputs, so there is
//!   nothing here to reconstruct - but it is why that note is now stale.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use half::f16;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::{Session, SessionInputValue};
use ort::value::TensorElementType;
use ort::value::{DynValue, Tensor, ValueType};
use tokenizers::Tokenizer;

use super::prompt::Styling;
use super::{
    prompt, RefineError, RefineOptions, Refinement, TextRefiner, MAX_INPUT_CHARS,
    OUTPUT_TOKEN_MARGIN,
};

/// Weights, quantised to 4 bits.
///
/// `q4` and not `q4f16`: measured on an M1 Pro the fp32 graph runs at 33.3
/// tok/s against 22.0, because the CPU provider has no fp16 kernels to reach
/// for and pays for the casts. It also keeps the cache and the logits in f32,
/// which is what the rest of this file reads.
pub const MODEL_FILE: &str = "model_q4.onnx";
/// The weights themselves, which do not fit in the graph file.
///
/// ONNX Runtime finds this by the location string recorded inside
/// `model_q4.onnx`, resolved next to it, so the name is not ours to choose.
pub const EXTERNAL_DATA_FILE: &str = "model_q4.onnx_data";
/// The tokeniser, in HuggingFace's portable JSON form.
pub const TOKENIZER_FILE: &str = "tokenizer.json";

/// Tokens that end an assistant turn, across the templates we support.
const STOP_TOKENS: &[&str] = &["<|im_end|>", "<|endoftext|>", "<|eot_id|>"];

/// Most threads we will give the runtime.
///
/// Measured, and the single largest speed lever here. On an M1 Pro - eight
/// performance cores and two efficiency cores - the mean dictation takes 774 ms
/// at six threads and 2 067 ms at ten, which is roughly what the runtime picks
/// on its own. Decode is memory-bound and scales badly past this; scheduling it
/// onto the efficiency cores makes every thread wait for the slowest.
///
/// One machine's number. If it is ever re-measured elsewhere, this is the one
/// place to change.
const MAX_INTRA_THREADS: usize = 6;

/// A refiner backed by a decoder-only ONNX graph.
pub struct OnnxRefiner {
    model_id: &'static str,
    model_dir: PathBuf,
    loaded: Option<Loaded>,
    cancel: Arc<AtomicBool>,
}

struct Loaded {
    session: Session,
    tokenizer: Tokenizer,
    cache: CacheShape,
    stop_ids: Vec<u32>,
    optional: OptionalInputs,
    prefix: Option<PrefixCache>,
}

/// Inputs that some exports of this architecture declare and others do not.
///
/// Read off the graph rather than assumed, because the two models this app has
/// shipped disagree about both of them and guessing wrong is a hard refusal
/// from the runtime, not a silent fallback.
#[derive(Debug, Clone, Copy)]
struct OptionalInputs {
    position_ids: bool,
    num_logits_to_keep: bool,
}

/// The KV cache for the fixed head of the prompt, kept between dictations.
///
/// The system turn and the control line are the same every time for a given
/// [`Styling`], and they are 69 of the 78 fixed prompt tokens. Running them
/// once at load takes the mean prefill from 215 ms to 84 ms. The tensors are
/// passed to the graph as views, so holding them costs one copy of the cache
/// and each run copies nothing.
struct PrefixCache {
    styling: Styling,
    tokens: usize,
    cache: Vec<DynValue>,
}

/// One completed generation, before the guard has seen it.
struct Generated {
    text: String,
    tokens: usize,
    prompt_tokens: usize,
    prefill: Duration,
}

/// The KV cache layout, read off the graph rather than hardcoded, so a
/// different model with the same input naming needs no code change.
#[derive(Debug, Clone, Copy)]
struct CacheShape {
    layers: usize,
    heads: i64,
    head_dim: i64,
    /// Not assumed from the weight quantisation: a `q4f16` export carries a
    /// float16 cache while the `q4` one we ship carries a float32 cache, and
    /// feeding the wrong one is a hard refusal from the runtime rather than a
    /// silent conversion.
    element: CacheElement,
}

/// The element type of the KV cache tensors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheElement {
    Float16,
    Float32,
}

impl OnnxRefiner {
    #[must_use]
    pub fn new(model_id: &'static str, model_dir: PathBuf) -> Self {
        Self {
            model_id,
            model_dir,
            loaded: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn model_path(&self) -> PathBuf {
        self.model_dir.join(MODEL_FILE)
    }

    fn external_data_path(&self) -> PathBuf {
        self.model_dir.join(EXTERNAL_DATA_FILE)
    }

    fn tokenizer_path(&self) -> PathBuf {
        self.model_dir.join(TOKENIZER_FILE)
    }

    /// Run the greedy decode loop over the transcript.
    fn generate(
        loaded: &mut Loaded,
        styling: Styling,
        transcript: &str,
        cancel: &AtomicBool,
    ) -> Result<Generated, RefineError> {
        ensure_prefix(loaded, styling)?;

        // Split so the cache can be read while the session is driven. They are
        // separate fields, so this is one borrow each rather than two of the
        // same thing.
        let Loaded {
            session,
            tokenizer,
            cache,
            stop_ids,
            optional,
            prefix,
        } = loaded;
        let prefix = prefix
            .as_ref()
            .ok_or_else(|| RefineError::Generation("the prompt prefix was not built".into()))?;

        let suffix = prompt::suffix(transcript);
        let encoded = tokenizer
            .encode(suffix.as_str(), false)
            // The underlying error is deliberately dropped rather than
            // interpolated: a tokeniser failure can quote the input it choked
            // on, and the input contains the transcription. Logs record shapes,
            // never words.
            .map_err(|_| RefineError::Generation("the prompt could not be tokenised".into()))?;

        let mut step_ids: Vec<i64> = encoded.get_ids().iter().copied().map(i64::from).collect();
        let suffix_len = step_ids.len();
        if suffix_len == 0 {
            return Err(RefineError::Generation("empty prompt".into()));
        }
        let prompt_len = prefix.tokens.saturating_add(suffix_len);

        // The model card's own ceiling: a cleaned transcript is about as long
        // as its input, and the margin covers punctuation and expanded
        // numerals. Past it the model has started writing something else, and
        // stopping mid-sentence costs nothing because the guard would have
        // rejected the result anyway.
        let max_new = suffix_len
            .saturating_mul(13)
            .saturating_div(10)
            .saturating_add(OUTPUT_TOKEN_MARGIN);

        let mut past: Option<Vec<DynValue>> = None;
        let mut prefill = Duration::ZERO;
        let started = Instant::now();
        let mut generated: Vec<u32> = Vec::new();
        // Tokens the model has already seen, i.e. the length of the cache.
        let mut seen: usize = prefix.tokens;

        for _ in 0..max_new {
            // Checked before every step, so Escape lands within one token
            // rather than at the end of the run.
            if cancel.load(Ordering::Acquire) {
                return Err(RefineError::Cancelled);
            }

            let step_len = step_ids.len();
            let total = seen.saturating_add(step_len);

            let mut inputs: Vec<(String, SessionInputValue<'_>)> = Vec::new();
            inputs.push(("input_ids".into(), tensor_i64(&step_ids, step_len)?.into()));
            inputs.push((
                "attention_mask".into(),
                tensor_i64(&vec![1_i64; total], total)?.into(),
            ));
            if optional.position_ids {
                let positions: Vec<i64> = (0..step_len)
                    .map(|offset| i64::try_from(seen.saturating_add(offset)).unwrap_or(i64::MAX))
                    .collect();
                inputs.push(("position_ids".into(), tensor_i64(&positions, step_len)?.into()));
            }
            if optional.num_logits_to_keep {
                // Only the final position is ever read, and asking for it alone
                // keeps the vocabulary-sized head off every other one.
                inputs.push(("num_logits_to_keep".into(), scalar_i64(1)?.into()));
            }

            // Views, not moves: on the first step these are the cached prefix,
            // which has to survive into the next dictation.
            let history: &[DynValue] = past.as_deref().unwrap_or(&prefix.cache);
            for (index, value) in history.iter().enumerate() {
                inputs.push((cache_input_name(index), value.into()));
            }

            let mut outputs = session
                .run(inputs)
                .map_err(|e| RefineError::Generation(format!("inference failed: {e}")))?;

            // The first step consumes the whole prompt; every later one adds a
            // single token. Reported apart because they are tuned apart: one
            // shrinks by shortening the prompt, the other only by changing
            // model.
            if prefill.is_zero() {
                prefill = started.elapsed();
            }

            let next = {
                let (shape, data) = outputs
                    .get("logits")
                    .ok_or_else(|| RefineError::Generation("graph produced no logits".into()))?
                    .try_extract_tensor::<f32>()
                    .map_err(|e| RefineError::Generation(format!("could not read logits: {e}")))?;
                argmax_last_position(shape, data)?
            };

            // Reclaim this step's cache as the next step's history. These are
            // reference-counted handles, so nothing is copied.
            let mut next_past = Vec::with_capacity(cache.slots());
            for index in 0..cache.slots() {
                let name = cache_output_name(index);
                let value = outputs
                    .remove(name.as_str())
                    .ok_or_else(|| RefineError::Generation(format!("graph produced no {name}")))?;
                next_past.push(value);
            }
            past = Some(next_past);

            if stop_ids.contains(&next) {
                break;
            }

            generated.push(next);
            seen = total;
            step_ids = vec![i64::from(next)];
        }

        let text = tokenizer
            .decode(&generated, true)
            // Dropped for the same reason as the tokenisation error above.
            .map_err(|_| RefineError::Generation("the output could not be detokenised".into()))?;

        Ok(Generated {
            text,
            tokens: generated.len(),
            prompt_tokens: prompt_len,
            prefill,
        })
    }
}

/// Populate the cached prefix, unless it is already the one this styling wants.
fn ensure_prefix(loaded: &mut Loaded, styling: Styling) -> Result<(), RefineError> {
    if loaded.prefix.as_ref().is_some_and(|p| p.styling == styling) {
        return Ok(());
    }

    let text = prompt::prefix(styling);
    let encoded = loaded
        .tokenizer
        .encode(text.as_str(), false)
        // The prefix holds no transcription - it is a constant and the user's
        // styling - so there is nothing here to keep out of the log. It is
        // dropped anyway, to keep one rule for the whole file.
        .map_err(|_| RefineError::Generation("the prompt prefix could not be tokenised".into()))?;
    let ids: Vec<i64> = encoded.get_ids().iter().copied().map(i64::from).collect();
    let tokens = ids.len();
    if tokens == 0 {
        return Err(RefineError::Generation("empty prompt prefix".into()));
    }

    let started = Instant::now();
    let empty = empty_cache(loaded.cache)?;

    let mut inputs: Vec<(String, SessionInputValue<'_>)> = Vec::new();
    inputs.push(("input_ids".into(), tensor_i64(&ids, tokens)?.into()));
    inputs.push((
        "attention_mask".into(),
        tensor_i64(&vec![1_i64; tokens], tokens)?.into(),
    ));
    if loaded.optional.position_ids {
        let positions: Vec<i64> = (0..tokens)
            .map(|offset| i64::try_from(offset).unwrap_or(i64::MAX))
            .collect();
        inputs.push(("position_ids".into(), tensor_i64(&positions, tokens)?.into()));
    }
    if loaded.optional.num_logits_to_keep {
        inputs.push(("num_logits_to_keep".into(), scalar_i64(1)?.into()));
    }
    for (index, value) in empty.into_iter().enumerate() {
        inputs.push((cache_input_name(index), value.into()));
    }

    let mut outputs = loaded
        .session
        .run(inputs)
        .map_err(|e| RefineError::Generation(format!("could not prime the prompt: {e}")))?;

    let mut cache = Vec::with_capacity(loaded.cache.slots());
    for index in 0..loaded.cache.slots() {
        let name = cache_output_name(index);
        let value = outputs
            .remove(name.as_str())
            .ok_or_else(|| RefineError::Generation(format!("graph produced no {name}")))?;
        cache.push(value);
    }

    tracing::debug!(
        event = "refiner_prefix_primed",
        tokens,
        primed_ms = crate::millis(started.elapsed())
    );

    loaded.prefix = Some(PrefixCache {
        styling,
        tokens,
        cache,
    });
    Ok(())
}

impl CacheShape {
    /// Number of cache tensors: a key and a value per layer.
    const fn slots(self) -> usize {
        self.layers.saturating_mul(2)
    }
}

impl TextRefiner for OnnxRefiner {
    fn model_id(&self) -> &str {
        self.model_id
    }

    fn is_available(&self) -> bool {
        self.model_path().exists()
            && self.external_data_path().exists()
            && self.tokenizer_path().exists()
    }

    fn load(&mut self) -> Result<(), RefineError> {
        if self.loaded.is_some() {
            return Ok(());
        }
        if !self.is_available() {
            return Err(RefineError::ModelNotInstalled);
        }

        let started = Instant::now();

        let session = build_session(&self.model_path()).map_err(RefineError::ModelLoad)?;

        let tokenizer = Tokenizer::from_file(self.tokenizer_path())
            .map_err(|e| RefineError::ModelLoad(format!("tokeniser: {e}")))?;

        let cache = read_cache_shape(&session)?;
        let optional = read_optional_inputs(&session);
        let stop_ids = STOP_TOKENS
            .iter()
            .filter_map(|token| tokenizer.token_to_id(token))
            .collect();

        tracing::info!(
            event = "refiner_loaded",
            model = self.model_id,
            layers = cache.layers,
            load_ms = crate::millis(started.elapsed())
        );

        self.loaded = Some(Loaded {
            session,
            tokenizer,
            cache,
            stop_ids,
            optional,
            prefix: None,
        });
        Ok(())
    }

    fn unload(&mut self) {
        if self.loaded.take().is_some() {
            tracing::info!(event = "refiner_unloaded", model = self.model_id);
        }
    }

    fn is_loaded(&self) -> bool {
        self.loaded.is_some()
    }

    fn refine(&mut self, text: &str, options: &RefineOptions) -> Result<Refinement, RefineError> {
        let chars = text.chars().count();
        if chars > MAX_INPUT_CHARS {
            return Err(RefineError::TooLong { chars });
        }

        self.cancel.store(false, Ordering::Release);
        self.load()?;

        let cancel = Arc::clone(&self.cancel);
        let loaded = self.loaded.as_mut().ok_or(RefineError::ModelNotInstalled)?;

        let started = Instant::now();
        let generated = Self::generate(loaded, options.styling, text, &cancel)?;
        let duration = started.elapsed();

        Ok(Refinement {
            text: generated.text,
            duration,
            prefill: generated.prefill,
            generated_tokens: generated.tokens,
            prompt_tokens: generated.prompt_tokens,
        })
    }

    fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// How many threads to let the runtime use. See [`MAX_INTRA_THREADS`].
fn intra_threads() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().min(MAX_INTRA_THREADS))
}

/// Open the graph.
///
/// Spelled out rather than chained: each builder step returns an error that
/// carries the builder back, so the types do not line up for `and_then`.
fn build_session(path: &Path) -> Result<Session, String> {
    let builder = Session::builder().map_err(|e| e.to_string())?;
    let builder = builder
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|e| e.to_string())?;
    let builder = builder
        .with_intra_threads(intra_threads())
        .map_err(|e| e.to_string())?;
    // One stream of work, one token at a time; there are no independent
    // branches for a second thread pool to overlap.
    let mut builder = builder
        .with_parallel_execution(false)
        .map_err(|e| e.to_string())?;
    builder.commit_from_file(path).map_err(|e| e.to_string())
}

/// `past_key_values.0.key`, `past_key_values.0.value`, `past_key_values.1.key`, …
fn cache_input_name(slot: usize) -> String {
    let (layer, part) = cache_slot(slot);
    format!("past_key_values.{layer}.{part}")
}

/// The matching `present.N.key` / `present.N.value` the graph returns.
fn cache_output_name(slot: usize) -> String {
    let (layer, part) = cache_slot(slot);
    format!("present.{layer}.{part}")
}

/// Slots alternate key, value, key, value — layer `n` owns slots `2n` and `2n+1`.
const fn cache_slot(slot: usize) -> (usize, &'static str) {
    (slot / 2, if slot.is_multiple_of(2) { "key" } else { "value" })
}

/// An all-zero-length cache: the shape the graph wants before any token has
/// been seen. It holds no elements, so no f16 value is ever computed.
///
/// Built through an array rather than a raw slice because `ort` refuses a
/// zero-length dimension on the latter, and zero is the whole point here.
fn empty_cache(shape: CacheShape) -> Result<Vec<DynValue>, RefineError> {
    let heads = usize::try_from(shape.heads).unwrap_or(0);
    let head_dim = usize::try_from(shape.head_dim).unwrap_or(0);
    let dims = (1, heads, 0, head_dim);

    (0..shape.slots())
        .map(|_| {
            // `from_shape_vec` rather than `zeros`: there is nothing to zero,
            // and `zeros` would demand a numeric trait f16 does not carry.
            let value = match shape.element {
                CacheElement::Float16 => {
                    let empty = ndarray::Array4::<f16>::from_shape_vec(dims, Vec::new())
                        .map_err(|e| shaping_failed(&e))?;
                    Tensor::from_array(empty).map(ort::value::Value::into_dyn)
                }
                CacheElement::Float32 => {
                    let empty = ndarray::Array4::<f32>::from_shape_vec(dims, Vec::new())
                        .map_err(|e| shaping_failed(&e))?;
                    Tensor::from_array(empty).map(ort::value::Value::into_dyn)
                }
            };
            value.map_err(|e| RefineError::Generation(format!("could not build the cache: {e}")))
        })
        .collect()
}

fn shaping_failed(e: &ndarray::ShapeError) -> RefineError {
    RefineError::Generation(format!("could not shape the cache: {e}"))
}

fn tensor_i64(data: &[i64], len: usize) -> Result<Tensor<i64>, RefineError> {
    let width = i64::try_from(len).unwrap_or(i64::MAX);
    Tensor::from_array((vec![1_i64, width], data.to_vec().into_boxed_slice()))
        .map_err(|e| RefineError::Generation(format!("could not build an input: {e}")))
}

/// A rank-zero tensor: an empty shape holding one value.
fn scalar_i64(value: i64) -> Result<Tensor<i64>, RefineError> {
    Tensor::from_array((Vec::<i64>::new(), vec![value].into_boxed_slice()))
        .map_err(|e| RefineError::Generation(format!("could not build an input: {e}")))
}

/// Which of the inputs that vary between exports this graph declares.
fn read_optional_inputs(session: &Session) -> OptionalInputs {
    let has = |name: &str| session.inputs().iter().any(|input| input.name() == name);
    OptionalInputs {
        position_ids: has("position_ids"),
        num_logits_to_keep: has("num_logits_to_keep"),
    }
}

/// Read the number of layers and the per-layer cache dimensions off the graph.
fn read_cache_shape(session: &Session) -> Result<CacheShape, RefineError> {
    // Halved rather than counting the `.key` half: a suffix test on a name
    // ending in `.key` reads to clippy as a file-extension comparison.
    let layers = session
        .inputs()
        .iter()
        .filter(|input| input.name().starts_with("past_key_values."))
        .count()
        / 2;

    if layers == 0 {
        return Err(RefineError::ModelLoad(
            "the graph has no past_key_values inputs; it is not a cached decoder".into(),
        ));
    }

    let first = session
        .inputs()
        .iter()
        .find(|input| input.name() == "past_key_values.0.key")
        .ok_or_else(|| RefineError::ModelLoad("the graph has no past_key_values.0.key".into()))?;

    let ValueType::Tensor { ty, shape, .. } = first.dtype() else {
        return Err(RefineError::ModelLoad(
            "past_key_values.0.key is not a tensor".into(),
        ));
    };

    let element = match ty {
        TensorElementType::Float16 => CacheElement::Float16,
        TensorElementType::Float32 => CacheElement::Float32,
        other => {
            return Err(RefineError::ModelLoad(format!(
                "unsupported cache element type {other:?}"
            )))
        }
    };

    let dims: &[i64] = shape;
    let heads = dims.get(1).copied().unwrap_or(-1);
    let head_dim = dims.get(3).copied().unwrap_or(-1);
    if heads <= 0 || head_dim <= 0 {
        return Err(RefineError::ModelLoad(format!(
            "unexpected cache shape {dims:?}"
        )));
    }

    Ok(CacheShape {
        layers,
        heads,
        head_dim,
        element,
    })
}

/// Highest-scoring token id at the final position of a `[batch, seq, vocab]`
/// logits tensor.
fn argmax_last_position(shape: &[i64], data: &[f32]) -> Result<u32, RefineError> {
    let vocab = usize::try_from(shape.get(2).copied().unwrap_or(0))
        .map_err(|_| RefineError::Generation("logits have no vocabulary axis".into()))?;
    if vocab == 0 {
        return Err(RefineError::Generation(
            "logits have an empty vocabulary".into(),
        ));
    }

    // Only the final position matters: everything before it is a prediction
    // about a token we already have.
    let last = data
        .chunks_exact(vocab)
        .last()
        .ok_or_else(|| RefineError::Generation("logits are empty".into()))?;

    let (index, _) = last
        .iter()
        .enumerate()
        .fold((0_usize, f32::NEG_INFINITY), |(best_i, best), (i, &score)| {
            if score > best {
                (i, score)
            } else {
                (best_i, best)
            }
        });

    u32::try_from(index).map_err(|_| RefineError::Generation("token id out of range".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_thread_cap_is_never_zero_and_never_over_the_measured_ceiling() {
        // Zero would mean "let the runtime decide", which is the 2.8x-slower
        // configuration this constant exists to avoid.
        let threads = intra_threads();
        assert!(threads >= 1, "asked the runtime for {threads} threads");
        assert!(threads <= MAX_INTRA_THREADS);
    }

    #[test]
    fn cache_slots_alternate_key_and_value_within_a_layer() {
        assert_eq!(cache_slot(0), (0, "key"));
        assert_eq!(cache_slot(1), (0, "value"));
        assert_eq!(cache_slot(2), (1, "key"));
        assert_eq!(cache_input_name(3), "past_key_values.1.value");
        assert_eq!(cache_output_name(3), "present.1.value");
    }

    #[test]
    fn argmax_reads_only_the_final_position() {
        // Two positions, three tokens. The first position's winner is 0 and the
        // second's is 2; anything but 2 means the loop is picking a prediction
        // about a token it already has.
        let shape = [1_i64, 2, 3];
        let data = [9.0_f32, 0.0, 0.0, 0.0, 1.0, 5.0];
        assert_eq!(argmax_last_position(&shape, &data).unwrap_or(99), 2);
    }

    #[test]
    fn argmax_rejects_logits_with_no_vocabulary() {
        assert!(argmax_last_position(&[1, 1, 0], &[]).is_err());
    }
}
