use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::error::Result;
use crate::metrics::stats::Stats;
use crate::metrics::PrefillDecodeSummary;

pub struct Renderer {
    pub format: String,
    pub output_dir: String,
    pub model: String,
    pub backend: String,
    pub base_url: String,
    pub concurrent: usize,
    pub mode: String,
    pub tag: String,
    pub prompt_tokens: usize,
    pub output_tokens: usize,
    pub duration_secs: u64,
    pub no_cache: bool,
    pub seed: i64,
    pub prompt_stddev: usize,
    pub http_proxy: String,
    pub cache_rate: usize,
    pub num_prefix_prompts: usize,
}

/// JSON output matching Go project's jsonOutput structure
#[derive(Serialize)]
struct JsonOutput {
    // Config/metadata
    backend: String,
    base_url: String,
    model: String,
    concurrent: usize,
    duration: String,
    mode: String,
    prompt_tokens: usize,
    output_tokens: usize,
    #[serde(skip_serializing_if = "is_true")]
    no_cache: bool,
    #[serde(skip_serializing_if = "is_zero_i64")]
    seed: i64,
    #[serde(skip_serializing_if = "is_zero")]
    prompt_stddev: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    http_proxy: String,
    #[serde(skip_serializing_if = "is_zero")]
    cache_rate: usize,
    #[serde(skip_serializing_if = "is_zero")]
    num_prefix_prompts: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    tag: String,

    // Results
    total_requests: usize,
    errors: usize,
    requests_per_sec: f64,

    // TTFT
    ttft_mean: String,
    ttft_p50: String,
    ttft_p90: String,
    ttft_p95: String,
    ttft_min: String,
    ttft_max: String,

    // TPOT
    tpot_mean: String,
    tpot_p50: String,
    tpot_p90: String,
    tpot_p95: String,
    tpot_min: String,
    tpot_max: String,

    // Throughput
    prefill_tokens_per_sec: f64,
    decode_tokens_per_sec: f64,
    total_tokens_per_sec: f64,
    tpm: String,

    // Token counts
    total_prompt_tokens: usize,
    total_output_tokens: usize,
}

fn is_zero(v: &usize) -> bool { *v == 0 }
fn is_zero_i64(v: &i64) -> bool { *v == 0 }
fn is_true(v: &bool) -> bool { *v }

fn fmt_dur(d: std::time::Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms >= 1000.0 {
        format!("{:.2}s", ms / 1000.0)
    } else if ms >= 1.0 {
        format!("{:.1}ms", ms)
    } else if ms >= 0.001 {
        format!("{:.1}µs", ms * 1000.0)
    } else {
        format!("{:.0}ns", d.as_nanos())
    }
}

/// Format duration as Go-style string (e.g. "30s", "5m0s", "1h0m0s")
fn fmt_duration_go(secs: u64) -> String {
    if secs >= 3600 {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{h}h{m}m{s}s")
    } else if secs >= 60 {
        let m = secs / 60;
        let s = secs % 60;
        format!("{m}m{s}s")
    } else {
        format!("{secs}s")
    }
}

pub fn format_tpm(tpm: f64) -> String {
    if tpm >= 1_000_000.0 {
        format!("{:.2}M", tpm / 1_000_000.0)
    } else if tpm >= 1_000.0 {
        format!("{:.2}K", tpm / 1_000.0)
    } else {
        format!("{:.0}", tpm)
    }
}

impl Renderer {
    pub fn render(&self, stats: &Stats, pd: &PrefillDecodeSummary, errors: usize, total: usize) -> Result<()> {
        match self.format.as_str() {
            "json" => self.render_json(stats, pd, errors, total),
            _ => self.render_table(stats, pd, errors, total),
        }
    }

    fn render_table(&self, stats: &Stats, pd: &PrefillDecodeSummary, errors: usize, total: usize) -> Result<()> {
        println!();
        println!("IPERF Benchmark Results");
        println!();
        println!("  Requests:        {}/{} (success/total)", stats.total_requests, total);
        println!("  Throughput:      {:.2} req/sec", stats.requests_per_sec);
        println!();
        println!("  Latency (TTFT)");
        println!("    Mean:          {}", fmt_dur(stats.ttft_mean));
        println!("    P50:           {}", fmt_dur(stats.ttft_p50));
        println!("    P90:           {}", fmt_dur(stats.ttft_p90));
        println!("    P95:           {}", fmt_dur(stats.ttft_p95));
        println!("    Min:           {}", fmt_dur(stats.ttft_min));
        println!("    Max:           {}", fmt_dur(stats.ttft_max));
        println!();
        println!("  Latency (TPOT)");
        println!("    Mean:          {}", fmt_dur(stats.tpot_mean));
        println!("    P50:           {}", fmt_dur(stats.tpot_p50));
        println!("    P90:           {}", fmt_dur(stats.tpot_p90));
        println!("    P95:           {}", fmt_dur(stats.tpot_p95));
        println!("    Min:           {}", fmt_dur(stats.tpot_min));
        println!("    Max:           {}", fmt_dur(stats.tpot_max));
        println!();
        println!("  Throughput (Tokens/sec)");
        println!("    Prefill:       {:.2} tok/sec", pd.prefill_throughput);
        println!("    Decode:        {:.2} tok/sec", pd.decode_throughput);
        println!("    Overall:       {:.2} tok/sec", stats.total_tokens_per_sec);
        println!("    TPM:           {}", format_tpm(stats.tpm));
        println!();
        println!("  Prompt tokens:   {}", stats.total_prompt_tokens);
        println!("  Output tokens:   {}", stats.total_output_tokens);
        println!("  Errors:          {errors}");
        println!();
        Ok(())
    }

    fn build_json(&self, stats: &Stats, pd: &PrefillDecodeSummary, errors: usize, total: usize) -> JsonOutput {
        JsonOutput {
            backend: self.backend.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            concurrent: self.concurrent,
            duration: fmt_duration_go(self.duration_secs),
            mode: self.mode.clone(),
            prompt_tokens: self.prompt_tokens,
            output_tokens: self.output_tokens,
            no_cache: self.no_cache,
            seed: self.seed,
            prompt_stddev: self.prompt_stddev,
            http_proxy: self.http_proxy.clone(),
            cache_rate: self.cache_rate,
            num_prefix_prompts: self.num_prefix_prompts,
            tag: self.tag.clone(),
            total_requests: total,
            errors,
            requests_per_sec: stats.requests_per_sec,
            ttft_mean: fmt_dur(stats.ttft_mean),
            ttft_p50: fmt_dur(stats.ttft_p50),
            ttft_p90: fmt_dur(stats.ttft_p90),
            ttft_p95: fmt_dur(stats.ttft_p95),
            ttft_min: fmt_dur(stats.ttft_min),
            ttft_max: fmt_dur(stats.ttft_max),
            tpot_mean: fmt_dur(stats.tpot_mean),
            tpot_p50: fmt_dur(stats.tpot_p50),
            tpot_p90: fmt_dur(stats.tpot_p90),
            tpot_p95: fmt_dur(stats.tpot_p95),
            tpot_min: fmt_dur(stats.tpot_min),
            tpot_max: fmt_dur(stats.tpot_max),
            prefill_tokens_per_sec: pd.prefill_throughput,
            decode_tokens_per_sec: pd.decode_throughput,
            total_tokens_per_sec: stats.total_tokens_per_sec,
            tpm: format_tpm(stats.tpm),
            total_prompt_tokens: stats.total_prompt_tokens,
            total_output_tokens: stats.total_output_tokens,
        }
    }

    fn render_json(&self, stats: &Stats, pd: &PrefillDecodeSummary, errors: usize, total: usize) -> Result<()> {
        let output = self.build_json(stats, pd, errors, total);
        let json = serde_json::to_string_pretty(&output)?;
        println!("{json}");
        Ok(())
    }

    pub fn render_jsonl(&self, stats: &Stats, pd: &PrefillDecodeSummary, errors: usize, total: usize) -> Result<()> {
        let dir = Path::new(&self.output_dir);
        fs::create_dir_all(dir)?;

        let output = self.build_json(stats, pd, errors, total);

        // Sanitize model name: replace '/' with '_'
        let model_name = self.model.replace('/', "_");
        let filename = if self.tag.is_empty() {
            format!("{model_name}.jsonl")
        } else {
            format!("{model_name}-{}.jsonl", self.tag)
        };

        let path = dir.join(filename);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        use std::io::Write;
        let line = serde_json::to_string(&output)?;
        writeln!(file, "{line}")?;
        Ok(())
    }
}
