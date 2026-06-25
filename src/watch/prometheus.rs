use std::collections::HashMap;
use crate::watch::types::{HistogramSnapshot, InferenceMetrics};

/// Parse Prometheus text format into InferenceMetrics.
/// Supports vLLM, SGLang, and bare metric name prefixes.
pub fn parse_inference_metrics(input: &str, backend: &str) -> InferenceMetrics {
    let mut m = InferenceMetrics::default();
    let parsed = parse_prometheus_text(input);

    let prefixes: Vec<&str> = match backend {
        "vllm" => vec!["vllm:", ""],
        "sglang" => vec!["sglang:", ""],
        _ => vec!["", "vllm:", "sglang:"],
    };

    // Gauges
    m.num_requests_running = parsed.get_scalar("num_requests_running", &prefixes).map(|v| v as u64);
    m.num_requests_waiting = parsed.get_scalar("num_requests_waiting", &prefixes).map(|v| v as u64);
    m.num_requests_swapped = parsed.get_scalar("num_requests_swapped", &prefixes).map(|v| v as u64);
    m.num_tokens = parsed.get_scalar("num_tokens", &prefixes).map(|v| v as u64);
    m.gpu_cache_usage_perc = parsed.get_scalar("gpu_cache_usage_perc", &prefixes);
    m.cpu_cache_usage_perc = parsed.get_scalar("cpu_cache_usage_perc", &prefixes);

    // Counters
    m.num_requests_processed = parsed.get_scalar("num_requests_processed", &prefixes).map(|v| v as u64);
    m.num_tokens_processed = parsed.get_scalar("num_tokens_processed", &prefixes).map(|v| v as u64);
    m.prompt_tokens_total = parsed.get_scalar("prompt_tokens_total", &prefixes).map(|v| v as u64);
    m.generation_tokens_total = parsed.get_scalar("generation_tokens_total", &prefixes).map(|v| v as u64);
    m.num_prefill_requests = parsed.get_scalar("num_prefill_requests", &prefixes).map(|v| v as u64);
    m.num_decode_requests = parsed.get_scalar("num_decode_requests", &prefixes).map(|v| v as u64);

    // Histograms → avg_ms
    if let Some(h) = parsed.get_histogram("time_to_first_token_seconds", &prefixes) {
        m.ttft_avg_ms = Some(h.avg_ms());
    }
    if let Some(h) = parsed.get_histogram("time_per_output_token_seconds", &prefixes) {
        m.tpot_avg_ms = Some(h.avg_ms());
    }
    if let Some(h) = parsed.get_histogram("e2e_request_latency_seconds", &prefixes) {
        m.e2e_latency_avg_ms = Some(h.avg_ms());
    }
    if let Some(h) = parsed.get_histogram("scheduler_wait_latency_seconds", &prefixes) {
        m.scheduler_wait_latency_avg_ms = Some(h.avg_ms());
    }

    m
}

// ─── Internal parser ─────────────────────────────────────────────────

struct ParsedPrometheus {
    scalars: HashMap<String, f64>,
    histograms: HashMap<String, HistogramData>,
}

#[derive(Default)]
struct HistogramData {
    count: u64,
    sum: f64,
    buckets: Vec<(f64, u64)>,
}

impl ParsedPrometheus {
    fn get_scalar(&self, name: &str, prefixes: &[&str]) -> Option<f64> {
        for prefix in prefixes {
            let key = format!("{}{}", prefix, name);
            if let Some(&v) = self.scalars.get(&key) {
                return Some(v);
            }
        }
        None
    }

    fn get_histogram(&self, name: &str, prefixes: &[&str]) -> Option<HistogramSnapshot> {
        for prefix in prefixes {
            let key = format!("{}{}", prefix, name);
            if let Some(h) = self.histograms.get(&key) {
                return Some(HistogramSnapshot {
                    count: h.count,
                    sum: h.sum,
                    buckets: h.buckets.clone(),
                });
            }
        }
        None
    }
}

fn parse_prometheus_text(input: &str) -> ParsedPrometheus {
    let mut scalars = HashMap::new();
    let mut histograms: HashMap<String, HistogramData> = HashMap::new();
    let mut hist_buckets: HashMap<String, Vec<(f64, u64)>> = HashMap::new();

    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let (name, labels, value) = match parse_metric_line(line) {
            Some(v) => v,
            None => continue,
        };

        if let Some(base) = name.strip_suffix("_bucket") {
            let le = labels.iter()
                .find(|(k, _)| k == "le")
                .and_then(|(_, v)| v.parse::<f64>().ok())
                .unwrap_or(f64::INFINITY);
            hist_buckets.entry(base.to_string())
                .or_default()
                .push((le, value as u64));
        } else if let Some(base) = name.strip_suffix("_count") {
            histograms.entry(base.to_string()).or_default().count = value as u64;
        } else if let Some(base) = name.strip_suffix("_sum") {
            histograms.entry(base.to_string()).or_default().sum = value;
        } else {
            scalars.insert(name, value);
        }
    }

    for (name, mut buckets) in hist_buckets {
        buckets.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        histograms.entry(name).or_default().buckets = buckets;
    }

    ParsedPrometheus { scalars, histograms }
}

fn parse_metric_line(line: &str) -> Option<(String, Vec<(String, String)>, f64)> {
    let line = line.trim();
    if line.is_empty() { return None; }

    let (name, labels_str, rest) = if let Some(brace_start) = line.find('{') {
        let name = line[..brace_start].to_string();
        let brace_end = line.find('}').unwrap_or(line.len());
        let labels = &line[brace_start + 1..brace_end];
        let rest = line[brace_end + 1..].trim();
        (name, Some(labels), rest)
    } else {
        let mut parts = line.splitn(2, char::is_whitespace);
        let name = parts.next()?.to_string();
        let rest = parts.next()?.trim();
        (name, None, rest)
    };

    let labels = labels_str.map(parse_labels).unwrap_or_default();

    let value_str = rest.split_whitespace().next()?;
    let value: f64 = value_str.parse().ok()?;

    Some((name, labels, value))
}

fn parse_labels(s: &str) -> Vec<(String, String)> {
    let mut labels = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while let Some(&c) = chars.peek() {
            if c == ',' || c.is_whitespace() { chars.next(); } else { break; }
        }
        if chars.peek().is_none() { break; }

        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' { break; }
            key.push(c);
            chars.next();
        }
        if chars.peek().is_none() { break; }
        chars.next(); // consume '='

        let mut val = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(c) = chars.next() {
                if c == '"' { break; }
                if c == '\\' {
                    if let Some(escaped) = chars.next() {
                        val.push(escaped);
                    }
                } else {
                    val.push(c);
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ',' || c.is_whitespace() { break; }
                val.push(c);
                chars.next();
            }
        }

        labels.push((key.trim().to_string(), val));
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_gauge() {
        let input = "vllm:num_requests_running 5\n";
        let parsed = parse_prometheus_text(input);
        assert_eq!(parsed.scalars.get("vllm:num_requests_running"), Some(&5.0));
    }

    #[test]
    fn test_parse_histogram() {
        let input = r#"
# HELP vllm:time_to_first_token_seconds Time to first token
# TYPE vllm:time_to_first_token_seconds histogram
vllm:time_to_first_token_seconds_bucket{le="0.1"} 10
vllm:time_to_first_token_seconds_bucket{le="0.5"} 50
vllm:time_to_first_token_seconds_bucket{le="1.0"} 90
vllm:time_to_first_token_seconds_bucket{le="+Inf"} 100
vllm:time_to_first_token_seconds_count 100
vllm:time_to_first_token_seconds_sum 25.5
"#;
        let parsed = parse_prometheus_text(input);
        let hist = parsed.histograms.get("vllm:time_to_first_token_seconds").unwrap();
        assert_eq!(hist.count, 100);
        assert!((hist.sum - 25.5).abs() < 0.001);
        assert_eq!(hist.buckets.len(), 4);
    }

    #[test]
    fn test_parse_with_labels() {
        let input = r#"metric_name{label1="val1",label2="val2"} 42.5"#;
        let (name, labels, value) = parse_metric_line(input).unwrap();
        assert_eq!(name, "metric_name");
        assert_eq!(labels.len(), 2);
        assert!((value - 42.5).abs() < 0.001);
    }

    #[test]
    fn test_parse_inference_metrics_vllm() {
        let input = r#"
vllm:num_requests_running 5
vllm:num_requests_waiting 2
vllm:gpu_cache_usage_perc 0.75
vllm:prompt_tokens_total 50000
vllm:generation_tokens_total 10000
vllm:time_to_first_token_seconds_count 100
vllm:time_to_first_token_seconds_sum 12.5
"#;
        let m = parse_inference_metrics(input, "vllm");
        assert_eq!(m.num_requests_running, Some(5));
        assert_eq!(m.num_requests_waiting, Some(2));
        assert!((m.gpu_cache_usage_perc.unwrap() - 0.75).abs() < 0.001);
        assert_eq!(m.prompt_tokens_total, Some(50000));
        assert!((m.ttft_avg_ms.unwrap() - 125.0).abs() < 0.1); // 12.5/100 * 1000
    }

    #[test]
    fn test_parse_inference_metrics_no_prefix() {
        let input = "num_requests_running 3\n";
        let m = parse_inference_metrics(input, "unknown");
        assert_eq!(m.num_requests_running, Some(3));
    }
}
