use super::Sample;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Stats {
    pub total_requests: usize,
    #[allow(dead_code)]
    pub total_duration: Duration,
    pub requests_per_sec: f64,

    // TTFT
    pub ttft_mean: Duration,
    pub ttft_p50: Duration,
    pub ttft_p90: Duration,
    pub ttft_p95: Duration,
    pub ttft_min: Duration,
    pub ttft_max: Duration,

    // TPOT
    pub tpot_mean: Duration,
    pub tpot_p50: Duration,
    pub tpot_p90: Duration,
    pub tpot_p95: Duration,
    pub tpot_min: Duration,
    pub tpot_max: Duration,

    // Throughput
    #[allow(dead_code)]
    pub prefill_tokens_per_sec: f64,
    #[allow(dead_code)]
    pub decode_tokens_per_sec: f64,
    pub total_tokens_per_sec: f64,
    pub tpm: f64,

    // Token counts
    pub total_prompt_tokens: usize,
    pub total_output_tokens: usize,
}

impl Stats {
    pub fn empty() -> Self {
        Self {
            total_requests: 0,
            total_duration: Duration::ZERO,
            requests_per_sec: 0.0,
            ttft_mean: Duration::ZERO, ttft_p50: Duration::ZERO,
            ttft_p90: Duration::ZERO, ttft_p95: Duration::ZERO,
            ttft_min: Duration::ZERO, ttft_max: Duration::ZERO,
            tpot_mean: Duration::ZERO, tpot_p50: Duration::ZERO,
            tpot_p90: Duration::ZERO, tpot_p95: Duration::ZERO,
            tpot_min: Duration::ZERO, tpot_max: Duration::ZERO,
            prefill_tokens_per_sec: 0.0, decode_tokens_per_sec: 0.0,
            total_tokens_per_sec: 0.0, tpm: 0.0,
            total_prompt_tokens: 0, total_output_tokens: 0,
        }
    }
}

pub fn calc_stats(samples: &[Sample], wall_clock: Duration) -> Stats {
    if samples.is_empty() {
        return Stats::empty();
    }

    let n = samples.len();

    let mut ttfts: Vec<Duration> = samples.iter().map(|s| s.ttft).collect();

    // Collect ALL per-token inter-token latencies for accurate TPOT percentiles
    let mut all_token_latencies: Vec<Duration> = Vec::new();
    for s in samples {
        all_token_latencies.extend(&s.token_timings);
    }
    // Fallback: if no per-token timings, use per-request average TPOT
    if all_token_latencies.is_empty() {
        all_token_latencies = samples.iter()
            .filter(|s| s.tpot > Duration::ZERO)
            .map(|s| s.tpot)
            .collect();
    }

    let ttft_sum: Duration = samples.iter().map(|s| s.ttft).sum();
    let tpot_sum: Duration = all_token_latencies.iter().sum();

    let total_prompt: usize = samples.iter().map(|s| s.prompt_tokens).sum();
    let total_output: usize = samples.iter().map(|s| s.output_tokens).sum();

    let wall_secs = wall_clock.as_secs_f64();

    Stats {
        total_requests: n,
        total_duration: wall_clock,
        requests_per_sec: safe_div(n as f64, wall_secs),
        ttft_mean: ttft_sum / n as u32,
        ttft_p50: percentile(&mut ttfts, 50),
        ttft_p90: percentile(&mut ttfts, 90),
        ttft_p95: percentile(&mut ttfts, 95),
        ttft_min: ttfts.iter().copied().min().unwrap_or(Duration::ZERO),
        ttft_max: ttfts.iter().copied().max().unwrap_or(Duration::ZERO),
        tpot_mean: if all_token_latencies.is_empty() { Duration::ZERO } else { tpot_sum / all_token_latencies.len() as u32 },
        tpot_p50: percentile(&mut all_token_latencies, 50),
        tpot_p90: percentile(&mut all_token_latencies, 90),
        tpot_p95: percentile(&mut all_token_latencies, 95),
        tpot_min: all_token_latencies.iter().copied().min().unwrap_or(Duration::ZERO),
        tpot_max: all_token_latencies.iter().copied().max().unwrap_or(Duration::ZERO),
        prefill_tokens_per_sec: safe_div(total_prompt as f64, wall_secs),
        decode_tokens_per_sec: safe_div(total_output as f64, wall_secs),
        total_tokens_per_sec: safe_div((total_prompt + total_output) as f64, wall_secs),
        tpm: safe_div((total_prompt + total_output) as f64 * 60.0, wall_secs),
        total_prompt_tokens: total_prompt,
        total_output_tokens: total_output,
    }
}

fn percentile(d: &mut [Duration], p: i32) -> Duration {
    if d.is_empty() { return Duration::ZERO; }
    d.sort();
    let idx = ((p as f64 / 100.0 * d.len() as f64).ceil() as usize).saturating_sub(1);
    d[idx.min(d.len() - 1)]
}

#[allow(dead_code)]
fn mean(vals: &[f64]) -> f64 {
    if vals.is_empty() { 0.0 } else { vals.iter().sum::<f64>() / vals.len() as f64 }
}

fn safe_div(num: f64, den: f64) -> f64 {
    if den == 0.0 { 0.0 } else { num / den }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calc_stats_empty() {
        let stats = calc_stats(&[], Duration::from_secs(10));
        assert_eq!(stats.total_requests, 0);
    }

    #[test]
    fn test_calc_stats_basic() {
        let samples = vec![
            Sample {
                ttft: Duration::from_millis(50),
                prefill_dur: Duration::from_millis(50),
                decode_dur: Duration::from_millis(200),
                total_dur: Duration::from_millis(250),
                prompt_tokens: 100,
                output_tokens: 50,
                tpot: Duration::from_millis(4),
                token_timings: vec![Duration::from_millis(4), Duration::from_millis(5)],
            },
            Sample {
                ttft: Duration::from_millis(30),
                prefill_dur: Duration::from_millis(30),
                decode_dur: Duration::from_millis(150),
                total_dur: Duration::from_millis(180),
                prompt_tokens: 80,
                output_tokens: 40,
                tpot: Duration::from_millis(3),
                token_timings: vec![Duration::from_millis(3), Duration::from_millis(4)],
            },
        ];
        let stats = calc_stats(&samples, Duration::from_secs(1));
        assert_eq!(stats.total_requests, 2);
        assert_eq!(stats.total_prompt_tokens, 180);
        assert_eq!(stats.total_output_tokens, 90);
        assert!(stats.requests_per_sec > 0.0);
        assert_eq!(stats.ttft_p50, Duration::from_millis(30));
        assert_eq!(stats.ttft_p95, Duration::from_millis(50));
    }

    #[test]
    fn test_percentile() {
        let mut d = vec![
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(30),
            Duration::from_millis(40),
            Duration::from_millis(50),
        ];
        assert_eq!(percentile(&mut d, 50), Duration::from_millis(30));
        assert_eq!(percentile(&mut d, 100), Duration::from_millis(50));
    }
}
