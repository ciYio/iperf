use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "iperf", version, about = "AI inference backend performance benchmark")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run a benchmark
    Run(RunArgs),
    /// Generate default config.yaml
    Config(ConfigArgs),
    /// Model download and serving
    Hub(HubArgs),
}

/// All fields are Option so we can distinguish "user set" from "default"
/// when merging with config.yaml
#[derive(clap::Args, Clone)]
pub struct RunArgs {
    /// Path to config.yaml
    #[arg(long)]
    pub conf: Option<String>,
    /// Backend type (vllm, sglang)
    #[arg(long, short = 'b')]
    pub backend: Option<String>,
    /// Model name to benchmark
    #[arg(long, short = 'm')]
    pub model: Option<String>,
    /// Number of concurrent workers
    #[arg(long, short = 'c')]
    pub concurrency: Option<usize>,
    /// Benchmark duration (e.g. "60s", "5m", "1h")
    #[arg(long, short = 'd')]
    pub duration: Option<String>,
    /// Max requests (0 = unlimited)
    #[arg(long)]
    pub request_count: Option<usize>,
    /// Request mode: single, stream, continuous
    #[arg(long, short = 'M')]
    pub mode: Option<String>,
    /// Input prompt tokens
    #[arg(long)]
    pub prompt_tokens: Option<usize>,
    /// Max output tokens
    #[arg(long)]
    pub output_tokens: Option<usize>,
    /// Prepend UUID to each request (disable KV cache)
    #[arg(long)]
    pub no_cache: bool,
    /// Prompt pool size
    #[arg(long)]
    pub num_prefix_prompts: Option<usize>,
    /// Cache hit percentage (overrides num_prefix_prompts)
    #[arg(long)]
    pub cache_rate: Option<usize>,
    /// Random seed
    #[arg(long)]
    pub seed: Option<i64>,
    /// Prompt length standard deviation
    #[arg(long)]
    pub prompt_tokens_stddev: Option<usize>,
    /// Output format: table, json
    #[arg(long, short = 'f')]
    pub format: Option<String>,
    /// JSONL output directory
    #[arg(long)]
    pub output_dir: Option<String>,
    /// HTTP proxy URL
    #[arg(long)]
    pub http_proxy: Option<String>,
    /// Debug: print first request/response
    #[arg(long)]
    pub trace: bool,
    /// Tag for results
    #[arg(long)]
    pub tag: Option<String>,
    /// Warmup requests (excluded from metrics)
    #[arg(long)]
    pub warmup: Option<usize>,
    /// Target base URL (positional)
    #[arg(index = 1)]
    pub target: Option<String>,
}

#[derive(clap::Args)]
pub struct ConfigArgs {
    /// Output file path
    #[arg(long, short = 'o', default_value = "config.yaml")]
    pub output: String,
}

#[derive(clap::Args)]
pub struct HubArgs {
    #[command(subcommand)]
    pub command: HubCommands,
}

#[derive(Subcommand)]
pub enum HubCommands {
    /// Download model from HuggingFace or custom hub
    Download(HubDownloadArgs),
    /// Serve models via HTTP
    Serve(HubServeArgs),
}

#[derive(clap::Args)]
pub struct HubDownloadArgs {
    /// HuggingFace model ID (e.g. "meta-llama/Llama-3-8B")
    pub model_id: String,
    /// Local directory
    #[arg(long)]
    pub local_dir: Option<String>,
    /// Branch/revision (HuggingFace only)
    #[arg(long, short = 'r', default_value = "main")]
    pub revision: String,
    /// Custom hub URL
    #[arg(long)]
    pub source: Option<String>,
    /// HTTP proxy
    #[arg(long)]
    pub http_proxy: Option<String>,
    /// Skip first N files
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
    /// Download N files (0 = all)
    #[arg(long, default_value_t = 0)]
    pub count: usize,
}

#[derive(clap::Args)]
pub struct HubServeArgs {
    /// Models directory
    #[arg(long)]
    pub local_dir: String,
    /// Listen address
    #[arg(long, short = 'a', default_value = "0.0.0.0:8080")]
    pub addr: String,
}
