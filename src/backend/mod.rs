pub mod openai;
pub mod vllm;
pub mod sglang;

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use crate::error::{AppError, Result};
use crate::metrics::Sample;

#[derive(Debug, Clone)]
pub struct Timing {
    pub ttft: Duration,
    pub prefill_dur: Duration,
    pub decode_dur: Duration,
    pub total_dur: Duration,
    pub prompt_tokens: usize,
    pub output_tokens: usize,
    pub cached_tokens: usize,
    pub tpot: Duration,
    pub token_timings: Vec<Duration>,
}

impl Timing {
    pub fn to_sample(&self) -> Sample {
        Sample {
            ttft: self.ttft,
            prefill_dur: self.prefill_dur,
            decode_dur: self.decode_dur,
            total_dur: self.total_dur,
            prompt_tokens: self.prompt_tokens,
            output_tokens: self.output_tokens,
            cached_tokens: self.cached_tokens,
            tpot: self.tpot,
            token_timings: self.token_timings.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Request {
    pub model: String,
    pub messages: Vec<Message>,
    pub max_tokens: usize,
    pub temperature: f64,
    pub no_cache: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
}

#[derive(Debug)]
pub struct Response {
    pub content: String,
    pub timing: Timing,
}

#[async_trait]
pub trait Backend: Send + Sync {
    #[allow(dead_code)]
    fn name(&self) -> &str;
    async fn send(&self, req: Request) -> Result<Response>;
    async fn send_stream(
        &self,
        req: Request,
        on_token: &mut (dyn FnMut(String, Duration) + Send),
    ) -> Result<Response>;
    /// Apply proxy settings, return new backend if supported
    fn with_proxy_opt(&self, _proxy: &str) -> Option<Box<dyn Backend>> {
        None
    }
}

// --- Registry ---

type BackendCtor = Box<dyn Fn(&str) -> Box<dyn Backend> + Send + Sync>;

static REGISTRY: LazyLock<Mutex<HashMap<String, BackendCtor>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn register(name: &str, ctor: impl Fn(&str) -> Box<dyn Backend> + Send + Sync + 'static) {
    REGISTRY.lock().unwrap().insert(name.to_string(), Box::new(ctor));
}

pub fn new_backend(name: &str, base_url: &str, http_proxy: &str) -> Result<Box<dyn Backend>> {
    let registry = REGISTRY.lock().unwrap();
    let ctor = registry.get(name).ok_or_else(|| AppError::UnknownBackend(name.to_string()))?;
    let backend = ctor(base_url);
    // Apply proxy if supported
    if !http_proxy.is_empty() {
        if let Some(backend_with_proxy) = backend.with_proxy_opt(http_proxy) {
            return Ok(backend_with_proxy);
        }
    }
    Ok(backend)
}

pub fn init_backends() {
    vllm::register_vllm();
    sglang::register_sglang();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry() {
        init_backends();
        assert!(new_backend("vllm", "http://localhost:8000/v1", "").is_ok());
        assert!(new_backend("sglang", "http://localhost:8000/v1", "").is_ok());
        assert!(new_backend("unknown", "http://localhost:8000/v1", "").is_err());
    }
}
