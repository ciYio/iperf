use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::{AppError, Result};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_backend")]
    pub backend: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default = "default_duration")]
    pub duration: u64,
    #[serde(default)]
    pub request_count: usize,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default = "default_prompt_tokens")]
    pub prompt_tokens: usize,
    #[serde(default = "default_output_tokens")]
    pub output_tokens: usize,
    #[serde(default)]
    pub no_cache: bool,
    #[serde(default = "default_num_prefix_prompts")]
    pub num_prefix_prompts: usize,
    #[serde(default)]
    pub cache_rate: usize,
    #[serde(default)]
    pub seed: i64,
    #[serde(default)]
    pub prompt_tokens_stddev: usize,
    #[serde(default)]
    pub system_prompt_tokens: usize,
    #[serde(default = "default_num_system_prompts")]
    pub num_system_prompts: usize,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default = "default_output_dir")]
    pub output_dir: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub http_proxy: String,
    #[serde(default)]
    pub trace: bool,
}

fn default_backend() -> String { "vllm".into() }
fn default_base_url() -> String { "http://localhost:8000/v1".into() }
fn default_concurrency() -> usize { 1 }
fn default_duration() -> u64 { 0 }
fn default_mode() -> String { "stream".into() }
fn default_prompt_tokens() -> usize { 256 }
fn default_output_tokens() -> usize { 256 }
fn default_num_prefix_prompts() -> usize { 100 }
fn default_num_system_prompts() -> usize { 1 }
fn default_format() -> String { "table".into() }
fn default_output_dir() -> String {
    // Default output dir is next to the iperf binary
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("output")))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "./output".into())
}

impl Default for Config {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            base_url: default_base_url(),
            model: String::new(),
            concurrency: default_concurrency(),
            duration: default_duration(),
            request_count: 0,
            mode: default_mode(),
            prompt_tokens: default_prompt_tokens(),
            output_tokens: default_output_tokens(),
            no_cache: false,
            num_prefix_prompts: default_num_prefix_prompts(),
            cache_rate: 0,
            seed: 0,
            prompt_tokens_stddev: 0,
            system_prompt_tokens: 0,
            num_system_prompts: default_num_system_prompts(),
            format: default_format(),
            output_dir: default_output_dir(),
            tag: String::new(),
            http_proxy: String::new(),
            trace: false,
        }
    }
}

impl Config {
    pub fn load_yaml(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let cfg: Config = serde_yaml::from_str(&content)?;
        Ok(cfg)
    }

    pub fn generate_default_yaml(path: &Path) -> Result<()> {
        let cfg = Config::default();
        let tmpl = ConfigTemplate::from(&cfg);
        let yaml = serde_yaml::to_string(&tmpl)
            .map_err(|e| AppError::Config(e.to_string()))?;
        std::fs::write(path, yaml)?;
        Ok(())
    }

    pub fn merge_overrides(&mut self, o: &ConfigOverrides) {
        if let Some(ref v) = o.backend      { self.backend = v.clone(); }
        if let Some(ref v) = o.base_url     { self.base_url = v.clone(); }
        if let Some(ref v) = o.model        { self.model = v.clone(); }
        if let Some(v) = o.concurrency     { self.concurrency = v; }
        if let Some(v) = o.duration    { self.duration = v; }
        if let Some(v) = o.request_count    { self.request_count = v; }
        if let Some(ref v) = o.mode         { self.mode = v.clone(); }
        if let Some(v) = o.prompt_tokens    { self.prompt_tokens = v; }
        if let Some(v) = o.output_tokens    { self.output_tokens = v; }
        if let Some(v) = o.no_cache         { self.no_cache = v; }
        if let Some(v) = o.num_prefix_prompts { self.num_prefix_prompts = v; }
        if let Some(v) = o.cache_rate       { self.cache_rate = v; }
        if let Some(v) = o.seed             { self.seed = v; }
        if let Some(v) = o.prompt_tokens_stddev    { self.prompt_tokens_stddev = v; }
        if let Some(v) = o.system_prompt_tokens { self.system_prompt_tokens = v; }
        if let Some(v) = o.num_system_prompts { self.num_system_prompts = v; }
        if let Some(ref v) = o.format       { self.format = v.clone(); }
        if let Some(ref v) = o.output_dir   { self.output_dir = v.clone(); }
        if let Some(ref v) = o.http_proxy   { self.http_proxy = v.clone(); }
        if let Some(v) = o.trace            { self.trace = v; }
        if let Some(ref v) = o.tag          { self.tag = v.clone(); }
    }

    pub fn duration(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.duration)
    }
}

/// Partial config from CLI flags — None means "not specified, keep existing value"
pub struct ConfigOverrides {
    pub backend: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub concurrency: Option<usize>,
    pub duration: Option<u64>,
    pub request_count: Option<usize>,
    pub mode: Option<String>,
    pub prompt_tokens: Option<usize>,
    pub output_tokens: Option<usize>,
    pub no_cache: Option<bool>,
    pub num_prefix_prompts: Option<usize>,
    pub cache_rate: Option<usize>,
    pub seed: Option<i64>,
    pub prompt_tokens_stddev: Option<usize>,
    pub system_prompt_tokens: Option<usize>,
    pub num_system_prompts: Option<usize>,
    pub format: Option<String>,
    pub output_dir: Option<String>,
    pub http_proxy: Option<String>,
    pub trace: Option<bool>,
    pub tag: Option<String>,
}

/// Subset of Config fields written by `config -o` template.
/// Intentionally excludes runtime-only fields (duration, format, output_dir,
/// tag, http_proxy, trace, prompt_tokens_stddev).
#[derive(Serialize)]
struct ConfigTemplate {
    backend: String,
    base_url: String,
    model: String,
    concurrency: usize,
    request_count: usize,
    mode: String,
    prompt_tokens: usize,
    output_tokens: usize,
    no_cache: bool,
    num_prefix_prompts: usize,
    cache_rate: usize,
    seed: i64,
}

impl From<&Config> for ConfigTemplate {
    fn from(c: &Config) -> Self {
        Self {
            backend: c.backend.clone(),
            base_url: c.base_url.clone(),
            model: c.model.clone(),
            concurrency: c.concurrency,
            request_count: c.request_count,
            mode: c.mode.clone(),
            prompt_tokens: c.prompt_tokens,
            output_tokens: c.output_tokens,
            no_cache: c.no_cache,
            num_prefix_prompts: c.num_prefix_prompts,
            cache_rate: c.cache_rate,
            seed: c.seed,
        }
    }
}
