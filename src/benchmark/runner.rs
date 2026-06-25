use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressStyle};
use tokio::time::interval;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

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
    pub trace: bool,
    pub cache_rate: usize,    // 0-100: percentage of prompt to cache
    pub cancel: CancellationToken,
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
        let total_count = Arc::new(AtomicUsize::new(0)); // 用于领取请求编号
        let completed_count = Arc::new(AtomicUsize::new(0)); // 用于进度条显示
        let deadline = Instant::now() + self.duration;
        let request_count = self.request_count;
        let cancel = self.cancel.clone();

        let wall_start = Instant::now();

        // Spawn progress reporter
        let progress_total = completed_count.clone();
        let progress_err = error_count.clone();
        let progress_request_count = request_count;
        let progress_handle = tokio::spawn(async move {
            let pb = if progress_request_count > 0 {
                let pb = ProgressBar::new(progress_request_count as u64);
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
            let cache_rate = self.cache_rate;
            let collector = collector.clone();
            let error_count = error_count.clone();
            let total_count = total_count.clone();
            let completed_count = completed_count.clone();
            let prompt_gen = prompt_gen.clone();
            let cancel = cancel.clone();

            let handle = tokio::spawn(async move {
                loop {
                    // Check cancellation first
                    if cancel.is_cancelled() {
                        break;
                    }

                    // Atomically claim a request slot
                    let req_num = if request_count > 0 {
                        // request_count mode: atomically increment and check
                        let num = total_count.fetch_add(1, Ordering::Relaxed) + 1;
                        if num > request_count {
                            // Rolled back: we exceeded the limit
                            total_count.fetch_sub(1, Ordering::Relaxed);
                            break;
                        }
                        num
                    } else {
                        // duration mode: check deadline before claiming
                        if Instant::now() >= deadline {
                            break;
                        }
                        total_count.fetch_add(1, Ordering::Relaxed) + 1
                    };

                    let prompt = prompt_gen.next();
                    // Cache control (mutually exclusive):
                    //   --no-cache     → backend prepends UUID at start (0% cache)
                    //   --cache-rate N → runner inserts UUID at N% position (1-99 only)
                    let prompt = if !no_cache && (1..=99).contains(&cache_rate) {
                        insert_cache_breaker(&prompt, cache_rate)
                    } else {
                        prompt
                    };
                    let prompt_char_len = prompt.len();

                    let mut req = new_benchmark_request(&model, &prompt, max_tokens);
                    req.no_cache = no_cache;

                    let result = match mode.as_str() {
                        "stream" => {
                            let mut _on_token = |_token: String, _delta: Duration| {};
                            backend.send_stream(req, &mut _on_token).await
                        }
                        _ => {
                            backend.send(req).await
                        }
                    };

                    match result {
                        Ok(mut resp) => {
                            // Estimate prompt_tokens if server didn't return usage
                            if resp.timing.prompt_tokens == 0 {
                                resp.timing.prompt_tokens = prompt_char_len / 4; // ~4 chars per token
                            }
                            // Trace: show details for first 5 requests
                            if trace && req_num <= 5 {
                                eprintln!(
                                    "[trace] req#{} prompt_len={} prompt_tokens={} cached_tokens={} cache_rate={:.1}%",
                                    req_num,
                                    prompt_char_len,
                                    resp.timing.prompt_tokens,
                                    resp.timing.cached_tokens,
                                    if resp.timing.prompt_tokens > 0 {
                                        resp.timing.cached_tokens as f64 / resp.timing.prompt_tokens as f64 * 100.0
                                    } else { 0.0 }
                                );
                            }
                            collector.add(resp.timing.to_sample());
                            if trace && req_num == 1 {
                                let snippet = &resp.content[..resp.content.len().min(100)];
                                eprintln!("[trace] First response: {snippet}");
                            }
                        }
                        Err(e) => {
                            error_count.fetch_add(1, Ordering::Relaxed);
                            if trace {
                                eprintln!("[trace] Error: {e}");
                            }
                        }
                    }

                    // Mark request as completed (for progress bar)
                    completed_count.fetch_add(1, Ordering::Relaxed);
                }
            });
            handles.push(handle);
        }

        // Wait for all workers (with timeout if cancelled)
        if cancel.is_cancelled() {
            // Grace period: wait up to 5 seconds for in-flight requests
            let wait_all = async {
                for h in handles {
                    let _ = h.await;
                }
            };
            if tokio::time::timeout(Duration::from_secs(5), wait_all).await.is_err() {
                eprintln!("  Grace period expired, some in-flight requests were aborted.");
            }
        } else {
            for h in handles {
                let _ = h.await;
            }
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

/// Insert a UUID at the position determined by cache_rate.
/// Everything before the UUID is a shared prefix (cacheable),
/// the UUID and everything after it is unique per request.
///
/// cache_rate=50 → UUID at 50% of prompt length
/// cache_rate=0  → no insertion (fully unique)
/// cache_rate=100→ no insertion (fully cacheable)
fn insert_cache_breaker(prompt: &str, cache_rate: usize) -> String {
    let pos = prompt.len() * cache_rate / 100;
    let pos = find_word_boundary(prompt, pos);
    let uuid = Uuid::new_v4();
    let mut result = String::with_capacity(prompt.len() + 37);
    result.push_str(&prompt[..pos]);
    result.push_str(&format!("\n[{}]\n", uuid));
    result.push_str(&prompt[pos..]);
    result
}

/// Find the nearest space at or after `pos` to avoid splitting mid-word.
fn find_word_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() { return s.len(); }
    s[pos..].find(' ')
        .map(|i| pos + i + 1) // skip past the space
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_cache_breaker_50() {
        let prompt = "hello world this is a test prompt for cache rate testing purposes";
        let result = insert_cache_breaker(prompt, 50);
        // UUID is 36 chars + "\n[\n" + "]\n" = ~40 chars
        assert!(result.len() > prompt.len());
        // Should contain a UUID pattern
        assert!(result.contains("["));
        assert!(result.contains("]"));
        // Prefix before UUID should match ~50% of original prompt
        let uuid_start = result.find("[").unwrap();
        // Rough check: UUID position is near 50% of original length
        assert!(uuid_start >= prompt.len() * 45 / 100);
        assert!(uuid_start <= prompt.len() * 65 / 100);
    }

    #[test]
    fn test_insert_cache_breaker_unique() {
        let prompt = "hello world foo bar baz qux test";
        let r1 = insert_cache_breaker(prompt, 50);
        let r2 = insert_cache_breaker(prompt, 50);
        // Same prefix, different UUIDs
        assert_ne!(r1, r2);
        // But same prefix up to the UUID position
        let p1 = r1.find("[").unwrap();
        let p2 = r2.find("[").unwrap();
        assert_eq!(&r1[..p1], &r2[..p2]);
    }

    #[test]
    fn test_find_word_boundary() {
        let s = "hello world foo bar";
        assert_eq!(find_word_boundary(s, 0), 6);   // after "hello "
        assert_eq!(find_word_boundary(s, 6), 12);   // after "world "
        assert_eq!(find_word_boundary(s, 19), 19);  // at end
    }
}
