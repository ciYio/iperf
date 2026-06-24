use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressStyle};
use tokio::time::interval;

use crate::backend::Backend;
use crate::metrics::{calc_prefill_decode, calc_stats, Collector, PrefillDecodeSummary, Sample, Stats};
use crate::error::Result;

use super::{new_benchmark_request, PromptGenerator};

pub struct Runner {
    pub backend: Arc<dyn Backend>,
    pub model: String,
    pub concurrent: usize,
    pub duration: Duration,
    pub request_count: usize, // 0 = unlimited
    pub mode: String,         // "single" | "stream" | "continuous"
    pub max_tokens: usize,
    pub no_cache: bool,
    pub warmup: usize,
    pub trace: bool,
}

pub struct BenchResult {
    pub stats: Stats,
    pub prefill_decode: PrefillDecodeSummary,
    #[allow(dead_code)]
    pub samples: Vec<Sample>,
    pub errors: usize,
    pub total_requests: usize,
    #[allow(dead_code)]
    pub wall_clock: Duration,
}

impl Runner {
    pub async fn run(&self, prompt_gen: &PromptGenerator) -> Result<BenchResult> {
        let collector = Arc::new(Collector::new());
        let error_count = Arc::new(AtomicUsize::new(0));
        let total_count = Arc::new(AtomicUsize::new(0));
        let warmup_remaining = Arc::new(AtomicUsize::new(self.warmup));
        let deadline = Instant::now() + self.duration;
        let request_count = self.request_count;
        let warmup = self.warmup;

        let wall_start = Instant::now();

        // Spawn progress reporter
        let progress_total = total_count.clone();
        let progress_err = error_count.clone();
        let progress_request_count = request_count;
        let progress_handle = tokio::spawn(async move {
            let pb = if progress_request_count > 0 {
                let pb = ProgressBar::new((progress_request_count + warmup) as u64);
                pb.set_style(ProgressStyle::default_bar()
                    .template("  [{bar:30}] {pos}/{len} requests, {msg}")
                    .unwrap()
                    .progress_chars("=>-"));
                Some(pb)
            } else {
                None
            };

            let mut ticker = interval(Duration::from_secs(1));
            loop {
                ticker.tick().await;
                let done = progress_total.load(Ordering::Relaxed);
                let errs = progress_err.load(Ordering::Relaxed);

                if let Some(ref pb) = pb {
                    pb.set_position(done as u64);
                    pb.set_message(format!("{errs} errors"));
                } else {
                    eprint!("\r  [{done} requests, {errs} errors]");
                }
            }
        });

        // Spawn workers — PromptGenerator is cheaply cloneable (Arc-wrapped internals)
        let mut handles = Vec::new();
        for _i in 0..self.concurrent {
            let backend = self.backend.clone();
            let model = self.model.clone();
            let mode = self.mode.clone();
            let max_tokens = self.max_tokens;
            let no_cache = self.no_cache;
            let trace = self.trace;
            let collector = collector.clone();
            let error_count = error_count.clone();
            let total_count = total_count.clone();
            let warmup_remaining = warmup_remaining.clone();
            let prompt_gen = prompt_gen.clone();

            let handle = tokio::spawn(async move {
                loop {
                    if request_count > 0 {
                        // request_count mode: ignore duration, stop only when count reached
                        if total_count.load(Ordering::Relaxed) >= request_count + warmup {
                            break;
                        }
                    } else {
                        // duration mode: stop when deadline reached
                        if Instant::now() >= deadline {
                            break;
                        }
                    }

                    let prompt = prompt_gen.next();
                    let prompt_char_len = prompt.len();

                    let mut req = new_benchmark_request(&model, &prompt, max_tokens);
                    req.no_cache = no_cache;

                    let is_warmup = warmup_remaining.load(Ordering::Relaxed) > 0;

                    let result = match mode.as_str() {
                        "stream" => {
                            let mut _on_token = |_token: String, _delta: Duration| {};
                            backend.send_stream(req, &mut _on_token).await
                        }
                        _ => {
                            backend.send(req).await
                        }
                    };

                    total_count.fetch_add(1, Ordering::Relaxed);

                    match result {
                        Ok(mut resp) => {
                            // Estimate prompt_tokens if server didn't return usage
                            if resp.timing.prompt_tokens == 0 {
                                resp.timing.prompt_tokens = prompt_char_len / 4; // ~4 chars per token
                            }
                            if !is_warmup {
                                collector.add(resp.timing.to_sample());
                            } else {
                                warmup_remaining.fetch_sub(1, Ordering::Relaxed);
                            }
                            if trace && total_count.load(Ordering::Relaxed) == 1 {
                                let snippet = &resp.content[..resp.content.len().min(100)];
                                eprintln!("\n[trace] First response: {snippet}");
                            }
                        }
                        Err(e) => {
                            error_count.fetch_add(1, Ordering::Relaxed);
                            if trace {
                                eprintln!("\n[trace] Error: {e}");
                            }
                        }
                    }
                }
            });
            handles.push(handle);
        }

        // Wait for all workers
        for h in handles {
            let _ = h.await;
        }

        progress_handle.abort();
        eprintln!(); // newline after progress

        let wall_clock = wall_start.elapsed();
        let samples = collector.samples();
        let errors = error_count.load(Ordering::Relaxed);
        let total_requests = total_count.load(Ordering::Relaxed);

        let stats = calc_stats(&samples, wall_clock);
        let pd = calc_prefill_decode(&samples, wall_clock);

        Ok(BenchResult {
            stats,
            prefill_decode: pd,
            samples,
            errors,
            total_requests,
            wall_clock,
        })
    }
}
