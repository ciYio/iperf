use std::path::PathBuf;
use tokio::process::Child;
use crate::watch::types::{NsysMetrics, KernelSummary};

pub struct NsysSession {
    child: Child,
    report_path: PathBuf,
}

pub struct NsysCollector {
    output_dir: PathBuf,
}

impl NsysCollector {
    pub fn new(output_dir: PathBuf) -> anyhow::Result<Self> {
        // Check nsys availability
        std::process::Command::new("nsys")
            .arg("status")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        Ok(Self { output_dir })
    }

    pub async fn start_profiling(&self) -> anyhow::Result<NsysSession> {
        let report_stem = self.output_dir.join("iperf-watch");
        let report_path = report_stem.with_extension("nsys-rep");

        let child = tokio::process::Command::new("nsys")
            .args([
                "profile",
                "--trace=cuda,nvtx,osrt",
                "--output", &report_stem.to_string_lossy(),
                "--force-overwrite=true",
                "--capture-range=all",
                "--sample=none",
            ])
            .spawn()?;

        Ok(NsysSession { child, report_path })
    }

    pub async fn stop_and_collect(&self, mut session: NsysSession) -> anyhow::Result<NsysMetrics> {
        // Send SIGINT for graceful stop (nsys needs it to finalize the report)
        stop_nsys_gracefully(&mut session.child).await?;

        if !session.report_path.exists() {
            anyhow::bail!("nsys report not found at {}", session.report_path.display());
        }

        // Kernel summary
        let kern_output = tokio::process::Command::new("nsys")
            .args([
                "stats",
                "--report=cuda_gpu_kern_sum",
                "--format=csv",
                &session.report_path.to_string_lossy(),
            ])
            .output()
            .await?;

        let stdout = String::from_utf8_lossy(&kern_output.stdout);
        let kernel_summaries = parse_nsys_kern_sum(&stdout)?;
        let total_kernel_us: f64 = kernel_summaries.iter().map(|k| k.total_duration_us).sum();

        // Memcpy summary
        let memcpy_output = tokio::process::Command::new("nsys")
            .args([
                "stats",
                "--report=cuda_memcpy_sum",
                "--format=csv",
                &session.report_path.to_string_lossy(),
            ])
            .output()
            .await;

        let total_memcpy_us = if let Ok(out) = memcpy_output {
            let stdout = String::from_utf8_lossy(&out.stdout);
            parse_nsys_memcpy_sum(&stdout)
        } else {
            0.0
        };

        Ok(NsysMetrics {
            report_path: Some(session.report_path.to_string_lossy().into_owned()),
            kernel_summaries,
            total_kernel_duration_us: total_kernel_us,
            total_memcpy_duration_us: total_memcpy_us,
        })
    }
}

async fn stop_nsys_gracefully(child: &mut Child) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            unsafe {
                unsafe extern "C" { fn kill(pid: i32, sig: i32) -> i32; }
                kill(pid as i32, 2); // SIGINT = 2
            }
        }
        let _ = child.wait().await;
    }
    #[cfg(not(unix))]
    {
        child.start_kill()?;
        let _ = child.wait().await;
    }
    Ok(())
}

/// Parse `nsys stats --report=cuda_gpu_kern_sum --format=csv`
/// CSV columns: "Kernel Name", "Count", "Avg (ns)", "Min (ns)", "Max (ns)", "Total (ns)"
fn parse_nsys_kern_sum(csv: &str) -> anyhow::Result<Vec<KernelSummary>> {
    let mut summaries = Vec::new();
    let mut header_seen = false;

    for line in csv.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        if !header_seen { header_seen = true; continue; }

        let fields = parse_csv_line(line);
        if fields.len() < 6 { continue; }

        summaries.push(KernelSummary {
            kernel_name: fields[0].trim_matches('"').to_string(),
            call_count: fields[1].parse().unwrap_or(0),
            avg_duration_us: fields[2].parse::<f64>().unwrap_or(0.0) / 1000.0,
            min_duration_us: fields[3].parse::<f64>().unwrap_or(0.0) / 1000.0,
            max_duration_us: fields[4].parse::<f64>().unwrap_or(0.0) / 1000.0,
            total_duration_us: fields[5].parse::<f64>().unwrap_or(0.0) / 1000.0,
        });
    }
    Ok(summaries)
}

fn parse_nsys_memcpy_sum(csv: &str) -> f64 {
    let mut total = 0.0;
    let mut header_seen = false;
    for line in csv.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        if !header_seen { header_seen = true; continue; }
        let fields = parse_csv_line(line);
        if fields.len() >= 6 {
            total += fields[5].parse::<f64>().unwrap_or(0.0) / 1000.0;
        }
    }
    total
}

/// Simple CSV line parser handling quoted fields
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for c in line.chars() {
        match c {
            '"' => { in_quotes = !in_quotes; }
            ',' if !in_quotes => {
                fields.push(current.trim().to_string());
                current = String::new();
            }
            _ => { current.push(c); }
        }
    }
    fields.push(current.trim().to_string());
    fields
}
