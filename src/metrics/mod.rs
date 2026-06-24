pub mod stats;

use std::sync::Mutex;
use std::time::Duration;

// Re-export stats for convenience
pub use stats::{Stats, calc_stats};

#[derive(Debug, Clone)]
pub struct Sample {
    pub ttft: Duration,
    #[allow(dead_code)]
    pub prefill_dur: Duration,
    #[allow(dead_code)]
    pub decode_dur: Duration,
    #[allow(dead_code)]
    pub total_dur: Duration,
    pub prompt_tokens: usize,
    pub output_tokens: usize,
    pub cached_tokens: usize,
    pub tpot: Duration,
    pub token_timings: Vec<Duration>,
}

/// Thread-safe sample collector.
pub struct Collector {
    samples: Mutex<Vec<Sample>>,
}

impl Collector {
    pub fn new() -> Self {
        Self { samples: Mutex::new(Vec::new()) }
    }

    pub fn add(&self, s: Sample) {
        self.samples.lock().unwrap().push(s);
    }

    pub fn samples(&self) -> Vec<Sample> {
        self.samples.lock().unwrap().clone()
    }

    #[allow(dead_code)]
    pub fn count(&self) -> usize {
        self.samples.lock().unwrap().len()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PrefillDecodeSummary {
    pub prefill_throughput: f64,
    pub decode_throughput: f64,
}

pub fn calc_prefill_decode(samples: &[Sample], wall_clock: Duration) -> PrefillDecodeSummary {
    if samples.is_empty() {
        return PrefillDecodeSummary::default();
    }
    let total_prompt: usize = samples.iter().map(|s| s.prompt_tokens).sum();
    let total_output: usize = samples.iter().map(|s| s.output_tokens).sum();
    let secs = wall_clock.as_secs_f64();
    PrefillDecodeSummary {
        prefill_throughput: if secs > 0.0 { total_prompt as f64 / secs } else { 0.0 },
        decode_throughput: if secs > 0.0 { total_output as f64 / secs } else { 0.0 },
    }
}
