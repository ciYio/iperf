use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct HistogramSnapshot {
    pub count: u64,
    pub sum: f64,
    /// Bucket boundaries: (le, cumulative_count)
    #[allow(dead_code)]
    #[serde(skip)]
    pub buckets: Vec<(f64, u64)>,
}

impl HistogramSnapshot {
    pub fn avg_secs(&self) -> f64 {
        if self.count > 0 { self.sum / self.count as f64 } else { 0.0 }
    }

    pub fn avg_ms(&self) -> f64 {
        self.avg_secs() * 1000.0
    }

    /// Approximate percentile from histogram buckets using linear interpolation
    #[allow(dead_code)]
    pub fn percentile(&self, p: f64) -> f64 {
        if self.buckets.is_empty() || self.count == 0 { return 0.0; }
        let target = (p / 100.0 * self.count as f64) as u64;
        let mut prev_count = 0u64;
        let mut prev_bound = 0.0f64;
        for &(bound, cumulative) in &self.buckets {
            if cumulative >= target {
                let bucket_count = cumulative - prev_count;
                if bucket_count == 0 { return bound; }
                let fraction = (target - prev_count) as f64 / bucket_count as f64;
                return prev_bound + fraction * (bound - prev_bound);
            }
            prev_count = cumulative;
            prev_bound = bound;
        }
        self.buckets.last().map(|&(b, _)| b).unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct InferenceMetrics {
    // Gauges
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_requests_running: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_requests_waiting: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_requests_swapped: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_cache_usage_perc: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_cache_usage_perc: Option<f64>,

    // Counters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_requests_processed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_tokens_processed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_tokens_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_prefill_requests: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_decode_requests: Option<u64>,

    // Histograms (stored as avg_ms for JSONL output)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttft_avg_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tpot_avg_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub e2e_latency_avg_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduler_wait_latency_avg_ms: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GpuMetrics {
    pub gpu_index: u32,
    pub gpu_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_utilization: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_utilization: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_used: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_free: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_draw: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_limit: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_gpu: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fan_speed: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clock_sm: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clock_memory: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pstate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throttle_reasons: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct NsysMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_path: Option<String>,
    pub kernel_summaries: Vec<KernelSummary>,
    pub total_kernel_duration_us: f64,
    pub total_memcpy_duration_us: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct KernelSummary {
    pub kernel_name: String,
    pub call_count: u64,
    pub avg_duration_us: f64,
    pub min_duration_us: f64,
    pub max_duration_us: f64,
    pub total_duration_us: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DerivedMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_tokens_per_sec: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_per_sec: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests_per_sec: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throughput_per_gpu: Option<f64>,
}

/// A single JSONL record — one line per watch tick
#[derive(Debug, Clone, Serialize)]
pub struct WatchRecord {
    pub session_id: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gpu: Vec<GpuMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inference: Option<InferenceMetrics>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derived: Option<DerivedMetrics>,
}
