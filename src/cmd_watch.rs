use std::path::PathBuf;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::cli::WatchArgs;
use crate::cmd_run::parse_duration_secs;
use crate::watch::gpu_collector::GpuCollector;
use crate::watch::infer_collector::InferenceCollector;
use crate::watch::nsys_collector::NsysCollector;
use crate::watch::types::{DerivedMetrics, InferenceMetrics, WatchRecord};

pub async fn run(args: WatchArgs) -> anyhow::Result<()> {
    let session_id = Uuid::new_v4().to_string();
    let base_url = args.target.as_deref().unwrap_or("http://localhost:8000/v1");
    let backend = args.backend.as_deref().unwrap_or("vllm");
    let interval = args.interval;

    // Resolve output directory (default: next to binary)
    let output_dir = args.output_dir.clone().unwrap_or_else(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("output")))
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "./output".into())
    });
    std::fs::create_dir_all(&output_dir)?;

    // Build JSONL filename: {model}-{tag}.jsonl
    let file_stem = match (&args.model, &args.tag) {
        (Some(m), Some(t)) => format!("{}-{}", m, t),
        (Some(m), None) => m.clone(),
        _ => "watch".to_string(),
    };
    let jsonl_path = PathBuf::from(&output_dir).join(format!("{}.jsonl", file_stem));

    eprintln!("Watch session {} started", session_id);
    eprintln!("  Target: {} (backend={})", base_url, backend);
    eprintln!("  Interval: {}s | Output: {}", interval, jsonl_path.display());

    // Create collectors
    let infer = InferenceCollector::new(base_url, backend);
    let gpu = GpuCollector::new().await;
    if !gpu.is_available() {
        eprintln!("  nvidia-smi not found — GPU metrics will be skipped");
    }

    // nsys setup
    let nsys = if args.nsys {
        match NsysCollector::new(PathBuf::from(&output_dir)) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("  nsys unavailable: {} — tracing disabled", e);
                None
            }
        }
    } else {
        None
    };

    let nsys_session = if let Some(ref nc) = nsys {
        match nc.start_profiling().await {
            Ok(s) => { eprintln!("  nsys profiling started"); Some(s) }
            Err(e) => { eprintln!("  nsys start failed: {} — continuing without tracing", e); None }
        }
    } else {
        None
    };

    // Signal handler
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_clone.cancel();
        }
    });

    // Watch loop
    let mut ticker = tokio::time::interval(Duration::from_secs(interval));
    let deadline = args.duration.as_deref()
        .map(parse_duration_secs)
        .transpose()?
        .map(|secs| tokio::time::Instant::now() + Duration::from_secs(secs));

    let mut prev_inference: Option<InferenceMetrics> = None;
    let mut jsonl_file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&jsonl_path)
        .await?;
    let mut tick_count = 0u64;

    loop {
        ticker.tick().await;

        // Check deadline
        if let Some(dl) = deadline {
            if tokio::time::Instant::now() >= dl { break; }
        }
        if cancel.is_cancelled() { break; }

        // Collect metrics in parallel
        let (infer_result, gpu_result) = tokio::join!(
            infer.collect(),
            gpu.collect(),
        );

        let inference = match infer_result {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!("  [warn] inference metrics: {}", e);
                None
            }
        };

        let gpus = match gpu_result {
            Ok(g) => g,
            Err(e) => {
                if gpu.is_available() {
                    eprintln!("  [warn] GPU metrics: {}", e);
                }
                vec![]
            }
        };

        // Derived metrics from counter deltas
        let derived = inference.as_ref().map(|inf| {
            compute_derived(inf, prev_inference.as_ref(), gpus.len(), interval as f64)
        });

        let record = WatchRecord {
            session_id: session_id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            gpu: gpus,
            inference,
            derived,
        };

        // Write JSONL
        let mut line = serde_json::to_string(&record)?;
        line.push('\n');
        if let Err(e) = jsonl_file.write_all(line.as_bytes()).await {
            eprintln!("  [warn] JSONL write failed: {}", e);
        }

        // stderr summary line
        print_summary(&record, tick_count);

        prev_inference = record.inference;
        tick_count += 1;
    }

    // Stop nsys and display summary
    if let (Some(nc), Some(session)) = (nsys, nsys_session) {
        eprintln!("  Stopping nsys profiling...");
        match nc.stop_and_collect(session).await {
            Ok(m) => {
                eprintln!("  nsys report: {}", m.report_path.as_deref().unwrap_or("?"));
                eprintln!("  Total kernel time: {:.1} ms", m.total_kernel_duration_us / 1000.0);
                eprintln!("  Total memcpy time: {:.1} ms", m.total_memcpy_duration_us / 1000.0);
                for k in &m.kernel_summaries.iter().take(10).collect::<Vec<_>>() {
                    eprintln!("    {} — count={}, avg={:.1}µs, total={:.1}µs",
                        if k.kernel_name.len() > 50 { &k.kernel_name[..50] } else { &k.kernel_name },
                        k.call_count, k.avg_duration_us, k.total_duration_us);
                }
            }
            Err(e) => eprintln!("  nsys collection failed: {}", e),
        }
    }

    eprintln!("Watch session ended — {} ticks saved to {}", tick_count, jsonl_path.display());
    Ok(())
}

/// Compute derived metrics from counter deltas between consecutive snapshots.
fn compute_derived(
    current: &InferenceMetrics,
    previous: Option<&InferenceMetrics>,
    gpu_count: usize,
    interval_secs: f64,
) -> DerivedMetrics {
    let mut d = DerivedMetrics::default();
    if interval_secs <= 0.0 { return d; }

    if let Some(prev) = previous {
        if let (Some(cur), Some(prv)) = (current.generation_tokens_total, prev.generation_tokens_total) {
            let rate = cur.saturating_sub(prv) as f64 / interval_secs;
            d.generation_tokens_per_sec = Some(rate);
            if gpu_count > 0 {
                d.throughput_per_gpu = Some(rate / gpu_count as f64);
            }
        }
        if let (Some(cur), Some(prv)) = (current.prompt_tokens_total, prev.prompt_tokens_total) {
            d.prompt_tokens_per_sec = Some(cur.saturating_sub(prv) as f64 / interval_secs);
        }
        if let (Some(cur), Some(prv)) = (current.num_requests_processed, prev.num_requests_processed) {
            d.requests_per_sec = Some(cur.saturating_sub(prv) as f64 / interval_secs);
        }
    }
    d
}

fn print_summary(record: &WatchRecord, tick: u64) {
    let ts = chrono::Local::now().format("%H:%M:%S");

    let gpu_str = record.gpu.first().map(|g| {
        let util = g.gpu_utilization.map(|v| format!("{}%", v)).unwrap_or_else(|| "?".into());
        let mem = match (g.memory_used, g.memory_total) {
            (Some(u), Some(t)) => format!("{}/{}MiB", u, t),
            _ => "?".into(),
        };
        let pwr = g.power_draw.map(|v| format!("{:.0}W", v)).unwrap_or_else(|| "?".into());
        format!("GPU{}: {} | Mem: {} | Pwr: {}", g.gpu_index, util, mem, pwr)
    }).unwrap_or_else(|| "GPU: N/A".into());

    let infer_str = record.inference.as_ref().map(|inf| {
        let running = inf.num_requests_running.map(|v| format!("run:{}", v)).unwrap_or_default();
        let waiting = inf.num_requests_waiting.filter(|&v| v > 0).map(|v| format!(" wait:{}", v)).unwrap_or_default();
        let ttft = inf.ttft_avg_ms.map(|v| format!(" TTFT:{:.0}ms", v)).unwrap_or_default();
        let tpot = inf.tpot_avg_ms.map(|v| format!(" TPOT:{:.1}ms", v)).unwrap_or_default();
        format!("{}{}{}{}", running, waiting, ttft, tpot)
    }).unwrap_or_default();

    let derived_str = record.derived.as_ref().map(|d| {
        let gen_rate = d.generation_tokens_per_sec.map(|v| format!(" Gen:{:.0}tok/s", v)).unwrap_or_default();
        let req = d.requests_per_sec.map(|v| format!(" Req:{:.1}/s", v)).unwrap_or_default();
        format!("{}{}", gen_rate, req)
    }).unwrap_or_default();

    eprintln!("[{}] {} | {}{}", ts, gpu_str, infer_str, derived_str);
    let _ = tick; // suppress unused warning
}
