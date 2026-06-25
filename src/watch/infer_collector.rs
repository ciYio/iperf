use crate::watch::types::InferenceMetrics;
use crate::watch::prometheus;

pub struct InferenceCollector {
    client: reqwest::Client,
    metrics_url: String,
    backend: String,
}

impl InferenceCollector {
    pub fn new(base_url: &str, backend: &str) -> Self {
        // Derive /metrics URL: strip /v1 suffix, append /metrics
        let metrics_url = if base_url.ends_with("/v1") {
            format!("{}/metrics", &base_url[..base_url.len() - 3])
        } else if base_url.ends_with("/v1/") {
            format!("{}metrics", &base_url[..base_url.len() - 4])
        } else {
            format!("{}/metrics", base_url.trim_end_matches('/'))
        };

        Self {
            client: reqwest::Client::new(),
            metrics_url,
            backend: backend.to_string(),
        }
    }

    pub async fn collect(&self) -> anyhow::Result<InferenceMetrics> {
        let resp = self.client.get(&self.metrics_url).send().await?;
        let text = resp.text().await?;
        Ok(prometheus::parse_inference_metrics(&text, &self.backend))
    }
}
