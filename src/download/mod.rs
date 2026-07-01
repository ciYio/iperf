use std::path::{Path, PathBuf};
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use serde::Deserialize;
use sha2::{Sha256, Digest};

use crate::error::Result;

const MAX_RETRY_INTERVAL: Duration = Duration::from_secs(300); // 5 minutes
// const PROGRESS_INTERVAL: Duration = Duration::from_secs(2);

// --- HuggingFace types ---

#[derive(Deserialize)]
struct TreeEntry {
    #[serde(rename = "type")]
    entry_type: String,
    path: String,
    #[serde(default)]
    lfs: Option<LfsInfo>,
}

#[derive(Deserialize)]
struct LfsInfo {
    oid: String,     // "sha256:<hex>"
}

// --- File info with optional checksum ---

struct FileInfo {
    filename: String,
    sha256: Option<String>,
}

// --- Check result ---

pub struct CheckResult {
    pub total: usize,
    pub missing: Vec<String>,
    pub corrupted: Vec<String>,
    pub verified: usize,
    pub no_checksum: usize,
}

impl CheckResult {
    pub fn print_summary(&self) {
        eprintln!("\n=== Check Summary ===");
        eprintln!("Total files: {}", self.total);
        eprintln!("Verified (SHA256): {}", self.verified);
        eprintln!("No checksum available: {}", self.no_checksum);
        eprintln!("Missing: {}", self.missing.len());
        eprintln!("Corrupted: {}", self.corrupted.len());

        if !self.missing.is_empty() {
            eprintln!("\nMissing files:");
            for f in &self.missing {
                eprintln!("  - {}", f);
            }
        }

        if !self.corrupted.is_empty() {
            eprintln!("\nCorrupted files:");
            for f in &self.corrupted {
                eprintln!("  - {}", f);
            }
        }

        if self.missing.is_empty() && self.corrupted.is_empty() {
            eprintln!("\n✓ All files present and verified");
        } else {
            eprintln!("\n✗ Some files are missing or corrupted");
        }
    }

    pub fn is_ok(&self) -> bool {
        self.missing.is_empty() && self.corrupted.is_empty()
    }
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
    pub target: Option<String>,  // Target file to check
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
            target: None,
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

    async fn list_files(&self) -> Result<Vec<FileInfo>> {
        let client = self.new_client()?;
        let url = format!(
            "https://huggingface.co/api/models/{}/tree/{}",
            self.model_id, self.revision
        );
        let resp: Vec<TreeEntry> = client.get(&url).send().await?.json().await?;

        let files = resp.into_iter()
            .filter(|e| e.entry_type == "file")
            .map(|e| {
                let sha256 = e.lfs.map(|lfs| lfs.oid);
                FileInfo {
                    filename: e.path,
                    sha256,
                }
            })
            .collect();

        Ok(files)
    }

    fn file_url(&self, filename: &str) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.model_id, self.revision, filename
        )
    }

    /// Compute SHA256 hash of a file with progress bar
    async fn compute_sha256(&self, path: &Path, filename: &str, file_idx: usize, total: usize) -> Result<String> {
        use tokio::io::AsyncReadExt;
        let metadata = tokio::fs::metadata(path).await?;
        let file_size = metadata.len();

        let pb = ProgressBar::new(file_size);
        pb.set_style(ProgressStyle::default_bar()
            .template(&format!("  [{}/{}] {} - COMPUTING [{{bar:30}}] {{bytes}}/{{total_bytes}} ({{bytes_per_sec}})",
                file_idx + 1, total, filename))
            .unwrap_or_else(|_| ProgressStyle::default_bar()));

        let mut file = tokio::fs::File::open(path).await?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 8192];

        loop {
            let bytes_read = file.read(&mut buffer).await?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
            pb.inc(bytes_read as u64);
        }

        pb.finish_and_clear();
        let result = hasher.finalize();
        Ok(hex::encode(result))
    }

    /// Check file integrity without downloading
    pub async fn check_files(&self) -> Result<CheckResult> {
        let files = self.list_files().await?;

        // Separate safetensors and other files
        let (safetensors_files, other_files): (Vec<_>, Vec<_>) = files.into_iter()
            .partition(|f| f.filename.ends_with(".safetensors"));

        // Handle role-based offset/count
        let (check_safetensors, safetensors_offset, safetensors_count) = if let Some((current, total_parts)) = self.role {
            let chunk_size = safetensors_files.len().div_ceil(total_parts);
            let offset = (current - 1) * chunk_size;
            (true, offset, chunk_size)
        } else {
            (true, self.offset, self.count)
        };

        // Build final file list
        let mut final_files: Vec<FileInfo> = Vec::new();

        if let Some((current, _)) = self.role {
            if current == 1 {
                final_files.extend(other_files);
            }
        } else {
            final_files.extend(other_files);
        }

        if check_safetensors {
            let iter = safetensors_files.into_iter().skip(safetensors_offset);
            let safetensors_chunk: Vec<FileInfo> = if safetensors_count > 0 {
                iter.take(safetensors_count).collect()
            } else {
                iter.collect()
            };
            final_files.extend(safetensors_chunk);
        }

        // Filter by target file if specified
        if let Some(ref target) = self.target {
            final_files.retain(|f| f.filename == *target);
        }

        let total = final_files.len();
        eprintln!("Checking {} files for {}", total, self.model_id);

        let mut result = CheckResult {
            total,
            missing: Vec::new(),
            corrupted: Vec::new(),
            verified: 0,
            no_checksum: 0,
        };

        for (idx, file_info) in final_files.iter().enumerate() {
            let dest = self.dest_dir.join(&file_info.filename);
            let status = if !dest.exists() {
                result.missing.push(file_info.filename.clone());
                "MISSING".to_string()
            } else if let Some(expected) = &file_info.sha256 {
                match self.compute_sha256(&dest, &file_info.filename, idx, total).await {
                    Ok(actual) => {
                        if &actual == expected {
                            result.verified += 1;
                            "OK".to_string()
                        } else {
                            result.corrupted.push(file_info.filename.clone());
                            "CORRUPTED".to_string()
                        }
                    }
                    Err(e) => {
                        eprintln!("  [{}/{}] {} - Failed to compute SHA256: {}", idx + 1, total, file_info.filename, e);
                        result.no_checksum += 1;
                        "ERROR".to_string()
                    }
                }
            } else {
                result.no_checksum += 1;
                "OK (no checksum)".to_string()
            };

            let sha_info = if let Some(sha) = &file_info.sha256 {
                format!(" [sha256: {}...]", &sha[..16.min(sha.len())])
            } else {
                String::new()
            };

            eprintln!("  [{}/{}] {} - {}{}", idx + 1, total, file_info.filename, status, sha_info);
        }

        Ok(result)
    }

    pub async fn download_all(&self) -> Result<()> {
        let files = self.list_files().await?;

        // Separate safetensors and other files
        let (safetensors_files, other_files): (Vec<_>, Vec<_>) = files.into_iter()
            .partition(|f| f.filename.ends_with(".safetensors"));

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
        let mut final_files: Vec<FileInfo> = Vec::new();

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
            let safetensors_chunk: Vec<FileInfo> = if safetensors_count > 0 {
                iter.take(safetensors_count).collect()
            } else {
                iter.collect()  // count=0 means take all
            };
            final_files.extend(safetensors_chunk);
        }

        let total = final_files.len();
        eprintln!("Downloading {} files for {}", total, self.model_id);

        std::fs::create_dir_all(&self.dest_dir)?;

        for (idx, file_info) in final_files.iter().enumerate() {
            if let Err(e) = self.download_file(&file_info.filename, file_info.sha256.as_deref(), idx, total).await {
                eprintln!("  Failed to download {}: {e}", file_info.filename);
            }
        }
        Ok(())
    }

    async fn download_file(&self, filename: &str, expected_sha256: Option<&str>, file_idx: usize, total: usize) -> Result<()> {
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
            // Verify SHA256 for existing file
            if let Some(expected) = expected_sha256 {
                match self.compute_sha256(&dest, filename, file_idx, total).await {
                    Ok(actual) => {
                        if actual != expected {
                            eprintln!("  [{}/{}] {} exists but SHA256 mismatch! Expected: {}, Got: {}. Redownloading...",
                                file_idx + 1, total, filename, expected, actual);
                            let _ = tokio::fs::remove_file(&dest).await;
                        } else {
                            eprintln!("  [{}/{}] {} (already exists, SHA256 verified: {})", file_idx + 1, total, filename, &actual[..16]);
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        eprintln!("  [{}/{}] {} exists, failed to compute SHA256: {e}", file_idx + 1, total, filename);
                        eprintln!("  [{}/{}] {} (already exists, skipping verification)", file_idx + 1, total, filename);
                        return Ok(());
                    }
                }
            } else {
                eprintln!("  [{}/{}] {} (already exists)", file_idx + 1, total, filename);
                return Ok(());
            }
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
            // Verify SHA256 for resumed .part file
            if let Some(expected) = expected_sha256 {
                match self.compute_sha256(&part_path, filename, file_idx, total).await {
                    Ok(actual) => {
                        if actual != expected {
                            eprintln!("  [{}/{}] {} resumed .part SHA256 mismatch! Expected: {}, Got: {}. Redownloading...",
                                file_idx + 1, total, filename, expected, actual);
                            let _ = tokio::fs::remove_file(&part_path).await;
                            offset = 0;
                        } else {
                            std::fs::rename(&part_path, &dest)?;
                            eprintln!("  [{}/{}] {} (resumed, complete, SHA256 verified: {})", file_idx + 1, total, filename, &actual[..16]);
                            return Ok(());
                        }
                    }
                    Err(e) => {
                        eprintln!("  [{}/{}] {} resumed .part, failed to compute SHA256: {e}", file_idx + 1, total, filename);
                        std::fs::rename(&part_path, &dest)?;
                        eprintln!("  [{}/{}] {} (resumed, complete, skipping verification)", file_idx + 1, total, filename);
                        return Ok(());
                    }
                }
            } else {
                std::fs::rename(&part_path, &dest)?;
                eprintln!("  [{}/{}] {} (resumed, complete)", file_idx + 1, total, filename);
                return Ok(());
            }
        }

        // Download with retry until success
        let mut attempt = 0;
        let mut retry_interval = Duration::from_secs(1);
        loop {
            attempt += 1;
            match self.download_once(&client, &url, &part_path, offset, filename, file_idx, total).await {
                Ok(()) => {
                    // Verify SHA256 before renaming
                    if let Some(expected) = expected_sha256 {
                        match self.compute_sha256(&part_path, filename, file_idx, total).await {
                            Ok(actual) => {
                                if actual != expected {
                                    eprintln!("  [{}/{}] {} SHA256 mismatch! Expected: {}, Got: {}",
                                        file_idx + 1, total, filename, expected, actual);
                                    // Delete corrupted file and retry
                                    let _ = tokio::fs::remove_file(&part_path).await;
                                    offset = 0;
                                    eprintln!("    Retrying in {}s...", retry_interval.as_secs());
                                    tokio::time::sleep(retry_interval).await;
                                    retry_interval = Duration::from_secs((retry_interval.as_secs() * 2).min(MAX_RETRY_INTERVAL.as_secs()));
                                    continue;
                                }
                                eprintln!("  [{}/{}] {} SHA256 verified: {}", file_idx + 1, total, filename, &actual[..16]);
                            }
                            Err(e) => {
                                eprintln!("  [{}/{}] {} Failed to compute SHA256: {e}", file_idx + 1, total, filename);
                                // Continue without verification
                            }
                        }
                    }
                    std::fs::rename(&part_path, &dest)?;
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("  [{}/{}] {} attempt {}: {e}",
                        file_idx + 1, total, filename, attempt);
                    // Update offset for resume
                    offset = tokio::fs::metadata(&part_path).await.map(|m| m.len()).unwrap_or(0);
                    eprintln!("    Retrying in {}s...", retry_interval.as_secs());
                    tokio::time::sleep(retry_interval).await;
                    // Exponential backoff, capped at MAX_RETRY_INTERVAL
                    retry_interval = Duration::from_secs((retry_interval.as_secs() * 2).min(MAX_RETRY_INTERVAL.as_secs()));
                }
            }
        }
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
    pub target: Option<String>,  // Target file to check
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
            target: None,
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

    /// Check file existence without downloading (no SHA256 verification for custom hubs)
    pub async fn check_files(&self) -> Result<CheckResult> {
        let files = self.list_files().await?;

        // Separate safetensors and other files
        let (safetensors_files, other_files): (Vec<_>, Vec<_>) = files.into_iter()
            .partition(|f| f.ends_with(".safetensors"));

        // Handle role-based offset/count
        let (check_safetensors, safetensors_offset, safetensors_count) = if let Some((current, total_parts)) = self.role {
            let chunk_size = safetensors_files.len().div_ceil(total_parts);
            let offset = (current - 1) * chunk_size;
            (true, offset, chunk_size)
        } else {
            (true, self.offset, self.count)
        };

        // Build final file list
        let mut final_files: Vec<String> = Vec::new();

        if let Some((current, _)) = self.role {
            if current == 1 {
                final_files.extend(other_files);
            }
        } else {
            final_files.extend(other_files);
        }

        if check_safetensors {
            let iter = safetensors_files.into_iter().skip(safetensors_offset);
            let safetensors_chunk: Vec<String> = if safetensors_count > 0 {
                iter.take(safetensors_count).collect()
            } else {
                iter.collect()
            };
            final_files.extend(safetensors_chunk);
        }

        // Filter by target file if specified
        if let Some(ref target) = self.target {
            final_files.retain(|f| f == target);
        }

        let total = final_files.len();
        eprintln!("Checking {} files for {} from {}", total, self.model_id, self.base_url);

        let mut result = CheckResult {
            total,
            missing: Vec::new(),
            corrupted: Vec::new(),
            verified: 0,
            no_checksum: 0,
        };

        for (idx, filename) in final_files.iter().enumerate() {
            let dest = self.dest_dir.join(filename);
            let status = if !dest.exists() {
                result.missing.push(filename.clone());
                "MISSING"
            } else {
                result.no_checksum += 1;
                "EXISTS (no checksum available)"
            };

            eprintln!("  [{}/{}] {} - {}", idx + 1, total, filename, status);
        }

        Ok(result)
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

            // Download with retry until success
            let client = self.new_client()?;
            let url = self.file_url(file);
            let mut attempt = 0;
            let mut retry_interval = Duration::from_secs(1);

            loop {
                attempt += 1;
                match client.get(&url).send().await {
                    Ok(resp) => {
                        match resp.bytes().await {
                            Ok(bytes) => {
                                if let Err(e) = std::fs::write(&dest, &bytes) {
                                    eprintln!("  [{}/{}] {} attempt {}: write error: {e}", idx + 1, total, file, attempt);
                                    tokio::time::sleep(retry_interval).await;
                                    retry_interval = Duration::from_secs((retry_interval.as_secs() * 2).min(MAX_RETRY_INTERVAL.as_secs()));
                                    continue;
                                }
                                eprintln!("  [{}/{}] {}", idx + 1, total, file);
                                break;
                            }
                            Err(e) => {
                                eprintln!("  [{}/{}] {} attempt {}: read error: {e}", idx + 1, total, file, attempt);
                                eprintln!("    Retrying in {}s...", retry_interval.as_secs());
                                tokio::time::sleep(retry_interval).await;
                                retry_interval = Duration::from_secs((retry_interval.as_secs() * 2).min(MAX_RETRY_INTERVAL.as_secs()));
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("  [{}/{}] {} attempt {}: {e}", idx + 1, total, file, attempt);
                        eprintln!("    Retrying in {}s...", retry_interval.as_secs());
                        tokio::time::sleep(retry_interval).await;
                        // Exponential backoff, capped at MAX_RETRY_INTERVAL
                        retry_interval = Duration::from_secs((retry_interval.as_secs() * 2).min(MAX_RETRY_INTERVAL.as_secs()));
                    }
                }
            }
        }
        Ok(())
    }
}
