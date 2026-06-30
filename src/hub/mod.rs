use std::path::{Path, PathBuf};

use tokio::io::{AsyncReadExt, AsyncSeekExt};
use axum::{
    Router,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::get,
};
use serde_json::json;

#[derive(Clone)]
struct HubState {
    models_dir: PathBuf,
}

pub async fn serve(models_dir: &str, addr: &str) -> anyhow::Result<()> {
    let state = HubState {
        models_dir: PathBuf::from(models_dir),
    };

    let app = Router::new()
        .route("/", get(list_models))
        .route("/models/*model_path", get(list_files_or_serve))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!("Serving models from {models_dir} on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn list_models(
    State(state): State<HubState>,
) -> Json<serde_json::Value> {
    let models = match std::fs::read_dir(&state.models_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                e.path().is_dir() && !name.starts_with('.')
            })
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect::<Vec<_>>(),
        Err(_) => vec![],
    };
    Json(json!(models))
}

async fn list_files_or_serve(
    State(state): State<HubState>,
    AxumPath(model_path): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    // model_path is like "Qwen/Qwen3-ForcedAligner-0.6B-hf" or "Qwen/Qwen3-ForcedAligner-0.6B-hf/file.txt"
    let parts: Vec<&str> = model_path.splitn(2, '/').collect();
    if parts.len() < 2 {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Check if it's a file request (model_path contains a file)
    let model_id = parts[0];
    let rest = parts[1];

    // Check if rest contains another slash (it's a file path)
    if let Some(slash_pos) = rest.find('/') {
        // It's a file request: model_id/file_path
        let file = &rest[slash_pos + 1..];
        let file_model_id = &rest[..slash_pos];
        let full_model_id = format!("{}/{}", model_id, file_model_id);
        return serve_file_by_path(&state, &full_model_id, file, headers).await;
    }

    // It's a list request: just model_id
    let dir = state.models_dir.join(&model_path);
    let files = walk_files(&dir, &dir);
    Json(json!(files)).into_response()
}

async fn serve_file_by_path(
    state: &HubState,
    model_id: &str,
    file: &str,
    headers: HeaderMap,
) -> Response {
    let full_path = state.models_dir.join(model_id).join(file);

    if !full_path.exists() || !full_path.is_file() {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Prevent path traversal
    let canonical = match full_path.canonicalize() {
        Ok(p) => p,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let models_root = match state.models_dir.canonicalize() {
        Ok(p) => p,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if !canonical.starts_with(&models_root) {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Handle Range requests
    if let Some(range) = headers.get("range").and_then(|v| v.to_str().ok()) {
        if let Some(response) = serve_range(&canonical, range).await {
            return response;
        }
    }

    // Serve full file (async, non-blocking)
    match tokio::fs::read(&canonical).await {
        Ok(data) => {
            let mut resp = data.into_response();
            resp.headers_mut().insert(
                "content-type",
                "application/octet-stream".parse().unwrap(),
            );
            resp
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn walk_files(root: &Path, prefix: &Path) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if name == ".cache" || name.starts_with('.') {
                    continue;
                }
                files.extend(walk_files(&path, prefix));
            } else {
                let rel = path.strip_prefix(prefix).unwrap_or(&path);
                files.push(rel.to_string_lossy().to_string());
            }
        }
    }
    files
}

async fn serve_range(path: &Path, range: &str) -> Option<Response> {
    // Parse "bytes=START-END" or "bytes=START-"
    let range = range.strip_prefix("bytes=")?;
    let parts: Vec<&str> = range.split('-').collect();
    let start: u64 = parts.first()?.parse().ok()?;
    let end: Option<u64> = parts.get(1).and_then(|s| if s.is_empty() { None } else { s.parse().ok() });

    let file_size = tokio::fs::metadata(path).await.ok()?.len();
    let end = end.unwrap_or(file_size - 1).min(file_size - 1);
    let content_length = end - start + 1;

    let mut file = tokio::fs::File::open(path).await.ok()?;
    file.seek(std::io::SeekFrom::Start(start)).await.ok()?;

    let mut buf = vec![0u8; content_length as usize];
    file.read_exact(&mut buf).await.ok()?;

    let mut resp = buf.into_response();
    *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
    let _ = resp.headers_mut().insert(
        "content-range",
        format!("bytes {start}-{end}/{file_size}").parse().ok()?,
    );
    let _ = resp.headers_mut().insert(
        "content-length",
        content_length.to_string().parse().ok()?,
    );
    Some(resp)
}
