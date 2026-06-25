use crate::watch::types::GpuMetrics;
use crate::watch::nvidia_smi;

pub struct GpuCollector {
    available: bool,
}

impl GpuCollector {
    /// Probe nvidia-smi availability at construction time
    pub async fn new() -> Self {
        let available = tokio::process::Command::new("nvidia-smi")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .is_ok();
        Self { available }
    }

    pub fn is_available(&self) -> bool {
        self.available
    }

    pub async fn collect(&self) -> anyhow::Result<Vec<GpuMetrics>> {
        if !self.available {
            return Ok(vec![]);
        }

        let output = tokio::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu", nvidia_smi::NVIDIA_SMI_QUERY,
                "--format=csv,noheader,nounits",
            ])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("nvidia-smi failed: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        nvidia_smi::parse_gpu_csv(&stdout)
    }
}
