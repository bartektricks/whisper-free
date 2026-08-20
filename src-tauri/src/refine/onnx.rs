//! The refinement model, on ONNX Runtime (decision 0005).
//!
//! The only file that may name `ort`, `tokenizers`, a graph input, or the KV
//! cache. Everything above this sees [`TextRefiner`].
//!
//! The runtime is the one already linked into the binary for the speech model,
//! so nothing new ships and behaviour is identical on every platform. What is
//! written here rather than pulled from a crate is the decode loop: prefill the
//! prompt, take the last position's logits, pick the highest, feed it back with
//! the cache the graph just returned. Greedy, so there is no sampler and no
//! randomness — the same transcription refines to the same text every time,
//! which is what makes this testable at all.

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

use super::{
    prompt, RefineError, RefineOptions, Refinement, Template, TextRefiner, MAX_INPUT_CHARS,
    OUTPUT_TOKEN_MARGIN,
};

/// Weights, quantised. The exports we use are self-contained at this size; a
/// model large enough to need external `.onnx_data` would need that file
/// pinning in the registry alongside this one.
pub const MODEL_FILE: &str = "model_q4f16.onnx";
/// The tokeniser, in HuggingFace's portable JSON form.
pub const TOKENIZER_FILE: &str = "tokenizer.json";

/// Tokens that end an assistant turn, across the templates we support.
const STOP_TOKENS: &[&str] = &["<|im_end|>", "<|endoftext|>", "<|eot_id|>"];

/// A refiner backed by a decoder-only ONNX graph.
pub struct OnnxRefiner {
    model_id: &'static str,
    model_dir: PathBuf,
    template: Template,
    loaded: Option<Loaded>,
    cancel: Arc<AtomicBool>,
}

struct Loaded {
    session: Session,
    tokenizer: Tokenizer,
    cache: CacheShape,
    stop_ids: Vec<u32>,
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
    /// Not assumed from the weight quantisation: a `q4f16` export of Qwen3
    /// carries a float16 cache while the same quantisation of Qwen2.5 carries
    /// a float32 one, and feeding the wrong one is a hard refusal from the
    /// runtime rather than a silent conversion.
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
    pub fn new(model_id: &'static str, model_dir: PathBuf, template: Template) -> Self {
        Self {
            model_id,
            model_dir,
            template,
            loaded: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn model_path(&self) -> PathBuf {
        self.model_dir.join(MODEL_FILE)
    }

    fn tokenizer_path(&self) -> PathBuf {
        self.model_dir.join(TOKENIZER_FILE)
    }

    /// Run the greedy decode loop over an already-built prompt.
    fn generate(
        loaded: &mut Loaded,
        prompt: &str,
        cancel: &AtomicBool,
    ) -> Result<Generated, RefineError> {
        let encoded = loaded
            .tokenizer
            .encode(prompt, false)
            // The underlying error is deliberately dropped rather than
            // interpolated: a tokeniser failure can quote the input it choked
            // on, and the input contains the transcription. Logs record shapes,
            // never words.
            .map_err(|_| RefineError::Generation("the prompt could not be tokenised".into()))?;

        let prompt_ids: Vec<i64> = encoded.get_ids().iter().copied().map(i64::from).collect();
        let prompt_len = prompt_ids.len();
        if prompt_len == 0 {
            return Err(RefineError::Generation("empty prompt".into()));
        }

        // A correction is about as long as its input; see OUTPUT_TOKEN_MARGIN.
        let max_new = prompt_len.saturating_add(OUTPUT_TOKEN_MARGIN);

        let mut past = empty_cache(loaded.cache)?;
        let mut prefill = Duration::ZERO;
        let started = Instant::now();
        let mut generated: Vec<u32> = Vec::new();
        // Tokens the model has already seen, i.e. the length of `past`.
        let mut seen: usize = 0;
        let mut step_ids = prompt_ids;

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
            let positions: Vec<i64> = (0..step_len)
                .map(|offset| i64::try_from(seen.saturating_add(offset)).unwrap_or(i64::MAX))
                .collect();
            inputs.push(("position_ids".into(), tensor_i64(&positions, step_len)?.into()));

            for (index, value) in past.into_iter().enumerate() {
                inputs.push((cache_input_name(index), value.into()));
            }

            let mut outputs = loaded
                .session
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
            past = Vec::with_capacity(loaded.cache.slots());
            for index in 0..loaded.cache.slots() {
                let name = cache_output_name(index);
                let value = outputs.remove(name.as_str()).ok_or_else(|| {
                    RefineError::Generation(format!("graph produced no {name}"))
                })?;
                past.push(value);
            }

            if loaded.stop_ids.contains(&next) {
                break;
            }

            generated.push(next);
            seen = total;
            step_ids = vec![i64::from(next)];
        }

        let text = loaded
            .tokenizer
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
        self.model_path().exists() && self.tokenizer_path().exists()
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

    fn refine(
        &mut self,
        text: &str,
        options: &RefineOptions,
    ) -> Result<Refinement, RefineError> {
        let chars = text.chars().count();
        if chars > MAX_INPUT_CHARS {
            return Err(RefineError::TooLong { chars });
        }

        self.cancel.store(false, Ordering::Release);
        self.load()?;

        let template = self.template;
        let cancel = Arc::clone(&self.cancel);
        let loaded = self
            .loaded
            .as_mut()
            .ok_or(RefineError::ModelNotInstalled)?;

        let built = prompt::build(template, text, &options.vocabulary);

        let started = Instant::now();
        let generated = Self::generate(loaded, &built, &cancel)?;
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

/// Open the graph.
///
/// Spelled out rather than chained: each builder step returns an error that
/// carries the builder back, so the types do not line up for `and_then`.
fn build_session(path: &Path) -> Result<Session, String> {
    let builder = Session::builder().map_err(|e| e.to_string())?;
    let mut builder = builder
        .with_optimization_level(GraphOptimizationLevel::Level3)
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
        return Err(RefineError::ModelLoad("past_key_values.0.key is not a tensor".into()));
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
        return Err(RefineError::Generation("logits have an empty vocabulary".into()));
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
            if score > best { (i, score) } else { (best_i, best) }
        });

    u32::try_from(index).map_err(|_| RefineError::Generation("token id out of range".into()))
}
