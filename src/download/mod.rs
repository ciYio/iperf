use std::path::{Path, PathBuf};
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use serde::Deserialize;

use crate::error::{AppError, Result};

const MAX_RETRIES: usize = 5;
// const PROGRESS_INTERVAL: Duration = Duration::from_secs(2);

// --- HuggingFace types ---

#[derive(Deserialize)]
struct ModelInfo {
    siblings: Vec<Sibling>,
}

#[derive(Deserialize)]
struct Sibling {
    rfilename: String,
}

// --- HuggingFace Downloader ---

pub struct Downloader {
    pub model_id: String,
    pub revision: String,
    pub dest_dir: PathBuf,
    pub proxy: Option<String>,
    pub offset: usize,
    pub count: usize,
    pub role: Option<(usize, usize)>,  // (current, total) e.g. (1, 4)
}

impl Downloader {
    pub fn new(model_id: &str, revision: &str, dest_dir: &Path, proxy: Option<&str>) -> Self {
        Self {
            model_id: model_id.to_string(),
            revision: revision.to_string(),
            dest_dir: dest_dir.to_path_buf(),
            proxy: proxy.map(String::from),
            offset: 0,
            count: 0,
            role: None,
        }
    }

    fn new_client(&self) -> Result<Client> {
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(300))
            .danger_accept_invalid_certs(true);
        if let Some(ref proxy) = self.proxy {
            builder = builder.proxy(reqwest::Proxy::all(proxy)?);
        }
        Ok(builder.build()?)
    }

    pub async fn list_files(&self) -> Result<Vec<String>> {
        let client = self.new_client()?;
        let url = format!("https://huggingface.co/api/models/{}", self.model_id);
        let resp: ModelInfo = client.get(&url).send().await?.json().await?;
        Ok(resp.siblings.into_iter().map(|s| s.rfilename).collect())
    }

    fn file_url(&self, filename: &str) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.model_id, self.revision, filename
        )
    }

    pub async fn download_all(&self) -> Result<()> {
        let files = self.list_files().await?;

        // Separate safetensors and other files
        let (safetensors_files, other_files): (Vec<_>, Vec<_>) = files.into_iter()
            .partition(|f| f.ends_with(".safetensors"));

        // Handle role-based offset/count
        let (download_safetensors, safetensors_offset, safetensors_count) = if let Some((current, total_parts)) = self.role {
            let chunk_size = safetensors_files.len().div_ceil(total_parts);
            let offset = (current - 1) * chunk_size;
            // Role 1 downloads all other files + first chunk of safetensors
            (true, offset, chunk_size)
        } else {
            (true, self.offset, self.count)
        };

        // Build final file list
        let mut final_files: Vec<String> = Vec::new();

        // If this is role 1 (or no role), download all non-safetensors files first
        if let Some((current, _)) = self.role {
            if current == 1 {
                final_files.extend(other_files);
            }
        } else {
            // No role: download everything
            final_files.extend(other_files);
        }

        // Add safetensors chunk
        if download_safetensors {
            let iter = safetensors_files.into_iter().skip(safetensors_offset);
            let safetensors_chunk: Vec<String> = if safetensors_count > 0 {
                iter.take(safetensors_count).collect()
            } else {
                iter.collect()  // count=0 means take all
            };
            final_files.extend(safetensors_chunk);
        }

        let total = final_files.len();
        eprintln!("Downloading {} files for {}", total, self.model_id);

        std::fs::create_dir_all(&self.dest_dir)?;

        for (idx, file) in final_files.iter().enumerate() {
            if let Err(e) = self.download_file(file, idx, total).await {
                eprintln!("  Failed to download {file}: {e}");
            }
        }
        Ok(())
    }

    async fn download_file(&self, filename: &str, file_idx: usize, total: usize) -> Result<()> {
        let dest = self.dest_dir.join(filename);
        let part_path = PathBuf::from(format!("{}.part", dest.display()));

        // Create parent directory
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let client = self.new_client()?;
        let url = self.file_url(filename);

        // Check if already complete
        if dest.exists() {
            eprintln!("  [{}/{}] {} (already exists)", file_idx + 1, total, filename);
            return Ok(());
        }

        // Check .part file for resume
        let mut offset = 0u64;
        if part_path.exists() {
            offset = tokio::fs::metadata(&part_path).await?.len();
        }

        // HEAD request to get total size
        let head_resp = client.head(&url).send().await?;
        let total_size: u64 = head_resp.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        if offset >= total_size && total_size > 0 {
            std::fs::rename(&part_path, &dest)?;
            eprintln!("  [{}/{}] {} (resumed, complete)", file_idx + 1, total, filename);
            return Ok(());
        }

        // Download with retry
        for attempt in 1..=MAX_RETRIES {
            match self.download_once(&client, &url, &part_path, offset, filename, file_idx, total).await {
                Ok(()) => {
                    std::fs::rename(&part_path, &dest)?;
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("  [{}/{}] {} attempt {}/{}: {e}",
                        file_idx + 1, total, filename, attempt, MAX_RETRIES);
                    // Update offset for resume
                    offset = tokio::fs::metadata(&part_path).await.map(|m| m.len()).unwrap_or(0);
                    if attempt < MAX_RETRIES {
                        tokio::time::sleep(Duration::from_secs(attempt as u64)).await;
                    }
                }
            }
        }
        Err(AppError::Backend(format!("failed to download {filename} after {MAX_RETRIES} attempts")))
    }

    async fn download_once(
        &self,
        client: &Client,
        url: &str,
        part_path: &Path,
        offset: u64,
        filename: &str,
        file_idx: usize,
        total: usize,
    ) -> Result<()> {
        use futures_util::StreamExt;

        let mut req = client.get(url);
        if offset > 0 {
            req = req.header("Range", format!("bytes={offset}-"));
        }

        let resp = req.send().await?;
        let total_size: u64 = resp.headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let pb = ProgressBar::new(total_size + offset);
        pb.set_position(offset);
        pb.reset_elapsed();  // 重置计时器，速度只算本次下载
        pb.set_style(ProgressStyle::default_bar()
            .template(&format!("  [{}/{}] {} [{{bar:30}}] {{bytes}}/{{total_bytes}} ({{bytes_per_sec}})",
                file_idx + 1, total, filename))
            .unwrap_or_else(|_| ProgressStyle::default_bar()));

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(part_path)
            .await?;

        let mut stream = resp.bytes_stream();
        use tokio::io::AsyncWriteExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            pb.inc(chunk.len() as u64);
        }
        pb.finish();
        Ok(())
    }
}

// --- Custom Hub Downloader ---

pub struct HubDownloader {
    pub base_url: String,
    pub model_id: String,
    pub dest_dir: PathBuf,
    pub proxy: Option<String>,
    pub offset: usize,
    pub count: usize,
    pub role: Option<(usize, usize)>,  // (current, total) e.g. (1, 4)
}

impl HubDownloader {
    pub fn new(base_url: &str, model_id: &str, dest_dir: &Path, proxy: Option<&str>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model_id: model_id.to_string(),
            dest_dir: dest_dir.to_path_buf(),
            proxy: proxy.map(String::from),
            offset: 0,
            count: 0,
            role: None,
        }
    }

    fn new_client(&self) -> Result<Client> {
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(300))
            .danger_accept_invalid_certs(true);
        if let Some(ref proxy) = self.proxy {
            builder = builder.proxy(reqwest::Proxy::all(proxy)?);
        }
        Ok(builder.build()?)
    }

    pub async fn list_files(&self) -> Result<Vec<String>> {
        let client = self.new_client()?;
        let url = format!("{}/models/{}", self.base_url, self.model_id);
        let resp: Vec<String> = client.get(&url).send().await?.json().await?;
        Ok(resp)
    }

    fn file_url(&self, filename: &str) -> String {
        format!("{}/models/{}/{}", self.base_url, self.model_id, filename)
    }

    pub async fn download_all(&self) -> Result<()> {
        let files = self.list_files().await?;

        // Separate safetensors and other files
        let (safetensors_files, other_files): (Vec<_>, Vec<_>) = files.into_iter()
            .partition(|f| f.ends_with(".safetensors"));

        // Handle role-based offset/count
        let (download_safetensors, safetensors_offset, safetensors_count) = if let Some((current, total_parts)) = self.role {
            let chunk_size = safetensors_files.len().div_ceil(total_parts);
            let offset = (current - 1) * chunk_size;
            (true, offset, chunk_size)
        } else {
            (true, self.offset, self.count)
        };

        // Build final file list
        let mut final_files: Vec<String> = Vec::new();

        // If this is role 1 (or no role), download all non-safetensors files first
        if let Some((current, _)) = self.role {
            if current == 1 {
                final_files.extend(other_files);
            }
        } else {
            // No role: download everything
            final_files.extend(other_files);
        }

        // Add safetensors chunk
        if download_safetensors {
            let iter = safetensors_files.into_iter().skip(safetensors_offset);
            let safetensors_chunk: Vec<String> = if safetensors_count > 0 {
                iter.take(safetensors_count).collect()
            } else {
                iter.collect()  // count=0 means take all
            };
            final_files.extend(safetensors_chunk);
        }

        let total = final_files.len();
        eprintln!("Downloading {} files for {} from {}", total, self.model_id, self.base_url);

        std::fs::create_dir_all(&self.dest_dir)?;

        for (idx, file) in final_files.iter().enumerate() {
            let dest = self.dest_dir.join(file);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Simple download without resume for hub (same pattern could be added)
            let client = self.new_client()?;
            let url = self.file_url(file);
            let resp = client.get(&url).send().await?;
            let bytes = resp.bytes().await?;
            std::fs::write(&dest, &bytes)?;
            eprintln!("  [{}/{}] {}", idx + 1, total, file);
        }
        Ok(())
    }
}
