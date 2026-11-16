//! Bindings use the exact installed llama.cpp ABI; models stay local and never execute Python.
use crate::ffi::*;
use sha2::{Digest, Sha256};
use std::{
    ffi::{CString, c_char, c_void},
    path::Path,
    sync::Mutex,
};
pub type Result<T> = std::result::Result<T, String>;
pub struct Model {
    pub api: LlamaApi,
    model: *mut llama_model,
    context: *mut llama_context,
    vocab: *const llama_vocab,
    generation: Mutex<()>,
}
unsafe extern "C" fn discard_log(_: ggml_log_level, _: *const c_char, _: *mut c_void) {}
impl Model {
    pub fn load(library: &Path, file: &Path, cpu: bool) -> Result<Self> {
        // These generated structs are pinned to the installed 0.3.36 native ABI.
        // Refuse an unreviewed ABI instead of passing mismatched by-value layouts into C.
        let bytes =
            std::fs::read(library).map_err(|_| "Pinned native model runtime is unavailable")?;
        if format!("{:x}", Sha256::digest(bytes))
            != "ef606d1f0d7a2bc278b2198a6cfd9abdd572d01f1917b8905c44dee7d549cb4b"
        {
            return Err("Native model ABI differs from the reviewed pinned runtime".into());
        }
        let api = unsafe { LlamaApi::new(library) }
            .map_err(|_| "Pinned native model runtime is unavailable")?;
        unsafe {
            api.llama_log_set(Some(discard_log), std::ptr::null_mut());
            api.llama_backend_init();
        }
        let mut params = unsafe { api.llama_model_default_params() };
        params.n_gpu_layers = if cpu { 0 } else { -1 };
        let path = CString::new(file.to_str().ok_or("Invalid local model path")?)
            .map_err(|_| "Invalid local model path")?;
        let model = unsafe { api.llama_model_load_from_file(path.as_ptr(), params) };
        if model.is_null() {
            return Err("The pinned local model could not be loaded".into());
        }
        let mut result = Self {
            api,
            model,
            context: std::ptr::null_mut(),
            vocab: std::ptr::null(),
            generation: Mutex::new(()),
        };
        result.vocab = unsafe { result.api.llama_model_get_vocab(model) };
        let mut options = unsafe { result.api.llama_context_default_params() };
        options.n_ctx = 8192;
        options.n_batch = 128;
        options.n_ubatch = 128;
        options.n_threads = 4;
        options.n_threads_batch = 4;
        options.no_perf = true;
        options.flash_attn_type = llama_flash_attn_type_LLAMA_FLASH_ATTN_TYPE_DISABLED;
        result.context = unsafe { result.api.llama_init_from_model(model, options) };
        if result.context.is_null() || result.vocab.is_null() {
            return Err("Native model context could not be allocated".into());
        }
        Ok(result)
    }
    pub fn tokenize(&self, text: &str, special: bool) -> Result<Vec<i32>> {
        if text.len() > 1_000_000 {
            return Err("Model prompt exceeds the worker limit".into());
        }
        let mut tokens = vec![0; text.len() + 32];
        let count = unsafe {
            self.api.llama_tokenize(
                self.vocab,
                text.as_ptr().cast(),
                text.len() as i32,
                tokens.as_mut_ptr(),
                tokens.len() as i32,
                false,
                special,
            )
        };
        if count < 0 || count as usize > tokens.len() {
            return Err("Model tokenizer failed".into());
        }
        tokens.truncate(count as usize);
        Ok(tokens)
    }
    pub fn piece(&self, token: i32) -> Result<Vec<u8>> {
        let mut bytes = vec![0u8; 256];
        let mut count = unsafe {
            self.api.llama_token_to_piece(
                self.vocab,
                token,
                bytes.as_mut_ptr().cast(),
                bytes.len() as i32,
                0,
                true,
            )
        };
        if count < 0 {
            let size = count.unsigned_abs() as usize;
            if size > 65536 {
                return Err("Invalid model token".into());
            }
            bytes.resize(size, 0);
            count = unsafe {
                self.api.llama_token_to_piece(
                    self.vocab,
                    token,
                    bytes.as_mut_ptr().cast(),
                    bytes.len() as i32,
                    0,
                    true,
                )
            };
        }
        if count < 0 || count as usize > bytes.len() {
            return Err("Model token decoding failed".into());
        }
        bytes.truncate(count as usize);
        Ok(bytes)
    }
    pub fn text(&self, tokens: &[i32]) -> Result<String> {
        let mut bytes = vec![];
        for token in tokens {
            bytes.extend(self.piece(*token)?);
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }
    fn decode(&self, tokens: &mut [i32]) -> Result<()> {
        let batch = unsafe {
            self.api
                .llama_batch_get_one(tokens.as_mut_ptr(), tokens.len() as i32)
        };
        if unsafe { self.api.llama_decode(self.context, batch) } != 0 {
            Err("Native model evaluation failed".into())
        } else {
            Ok(())
        }
    }
    pub fn generate(
        &self,
        prompt: &str,
        limit: usize,
        json: bool,
        temperature: f32,
        mut emit: impl FnMut(&str) -> Result<()>,
    ) -> Result<Generated> {
        let _generation = self
            .generation
            .try_lock()
            .map_err(|_| "A model generation is already in progress")?;
        if !temperature.is_finite() || !(0.0..=2.0).contains(&temperature) {
            return Err("Invalid sampling temperature".into());
        }
        let mut tokens = self.tokenize(prompt, true)?;
        if tokens.is_empty() || tokens.len() + limit > 8192 {
            return Err("Model prompt exceeds the actual context budget".into());
        }
        let prompt_tokens = tokens.len();
        unsafe {
            self.api
                .llama_memory_clear(self.api.llama_get_memory(self.context), true);
        }
        for batch in tokens.chunks_mut(128) {
            self.decode(batch)?;
        }
        let chain = unsafe {
            self.api
                .llama_sampler_chain_init(self.api.llama_sampler_chain_default_params())
        };
        if chain.is_null() {
            return Err("Model sampler initialization failed".into());
        }
        let _guard = Sampler {
            api: &self.api,
            pointer: chain,
        };
        if json {
            let grammar = CString::new(JSON_GRAMMAR).expect("static grammar");
            let root = c"root";
            let sampler = unsafe {
                self.api
                    .llama_sampler_init_grammar(self.vocab, grammar.as_ptr(), root.as_ptr())
            };
            if sampler.is_null() {
                return Err("Native JSON grammar is unavailable".into());
            }
            unsafe {
                self.api.llama_sampler_chain_add(chain, sampler);
            }
        }
        let samplers = unsafe {
            [
                self.api.llama_sampler_init_penalties(
                    self.api.llama_vocab_n_tokens(self.vocab),
                    64,
                    1.1,
                    0.0,
                    0.0,
                ),
                self.api.llama_sampler_init_top_k(40),
                self.api.llama_sampler_init_top_p(0.95, 1),
                self.api.llama_sampler_init_min_p(0.05, 1),
                self.api.llama_sampler_init_temp(temperature),
                self.api.llama_sampler_init_dist(u32::MAX),
            ]
        };
        if !samplers[0].is_null() {
            for token in tokens.iter().skip(tokens.len().saturating_sub(64)) {
                unsafe {
                    self.api.llama_sampler_accept(samplers[0], *token);
                }
            }
        }
        for sampler in samplers {
            if sampler.is_null() {
                return Err("Model sampler initialization failed".into());
            }
            unsafe {
                self.api.llama_sampler_chain_add(chain, sampler);
            }
        }
        let mut decoder = Utf8Stream::default();
        let mut output = String::new();
        let mut count = 0;
        let mut stopped = false;
        for _ in 0..limit {
            let token = unsafe { self.api.llama_sampler_sample(chain, self.context, -1) };
            if unsafe { self.api.llama_vocab_is_eog(self.vocab, token) } {
                stopped = true;
                break;
            }
            count += 1;
            let delta = decoder.push(&self.piece(token)?);
            output.push_str(&delta);
            if !delta.is_empty() {
                emit(&delta)?;
            }
            if ["<|return|>", "<|fim_suffix|>", "<|call|>"]
                .iter()
                .any(|stop| output.ends_with(stop))
            {
                stopped = true;
                break;
            }
            self.decode(&mut [token])?;
        }
        let tail = decoder.finish();
        if !tail.is_empty() {
            output.push_str(&tail);
            emit(&tail)?;
        }
        Ok(Generated {
            text: output,
            prompt_tokens,
            completion_tokens: count,
            finish: if stopped { "stop" } else { "length" },
        })
    }
}
impl Drop for Model {
    fn drop(&mut self) {
        unsafe {
            if !self.context.is_null() {
                self.api.llama_free(self.context);
            }
            if !self.model.is_null() {
                self.api.llama_model_free(self.model);
            }
        }
    }
}
struct Sampler<'a> {
    api: &'a LlamaApi,
    pointer: *mut llama_sampler,
}
impl Drop for Sampler<'_> {
    fn drop(&mut self) {
        unsafe {
            self.api.llama_sampler_free(self.pointer);
        }
    }
}
pub struct Generated {
    pub text: String,
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub finish: &'static str,
}
#[derive(Default)]
struct Utf8Stream {
    pending: Vec<u8>,
}
impl Utf8Stream {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut result = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    result.push_str(text);
                    self.pending.clear();
                    break;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    result.push_str(
                        std::str::from_utf8(&self.pending[..valid]).expect("validated UTF-8"),
                    );
                    if let Some(n) = e.error_len() {
                        result.push('\u{fffd}');
                        self.pending.drain(..valid + n);
                    } else {
                        self.pending.drain(..valid);
                        break;
                    }
                }
            }
        }
        result
    }
    fn finish(&mut self) -> String {
        let value = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        value
    }
}
const JSON_GRAMMAR: &str = r#"root ::= object
object ::= "{" ws (string ws ":" ws value (ws "," ws string ws ":" ws value)*)? ws "}"
array ::= "[" ws (value (ws "," ws value)*)? ws "]"
value ::= object | array | string | number | "true" | "false" | "null"
string ::= "\"" ([^"\\\x00-\x1F] | "\\" (["\\/bfnrt] | "u" [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F]))* "\""
number ::= "-"? ("0" | [1-9] [0-9]*) ("." [0-9]+)? ([eE] [+-]? [0-9]+)?
ws ::= [ \t\n\r]*
"#;
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn token_stream_preserves_split_korean_and_invalid_bytes() {
        let mut stream = Utf8Stream::default();
        let b = "한글".as_bytes();
        assert_eq!(stream.push(&b[..1]), "");
        assert_eq!(stream.push(&b[1..4]), "한");
        assert_eq!(stream.push(&b[4..]), "글");
        assert_eq!(stream.finish(), "");
        assert_eq!(stream.push(&[255, b'a']), "\u{fffd}a");
    }
}
