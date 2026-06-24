use std::path::{Path, PathBuf};

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
        .route("/models/{model_id}", get(list_files))
        .route("/models/{model_id}/{*file}", get(serve_file))
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

async fn list_files(
    State(state): State<HubState>,
    AxumPath(model_id): AxumPath<String>,
) -> Json<serde_json::Value> {
    let dir = state.models_dir.join(&model_id);
    let files = walk_files(&dir, &dir);
    Json(json!(files))
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

async fn serve_file(
    State(state): State<HubState>,
    AxumPath((model_id, file)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let full_path = state.models_dir.join(&model_id).join(&file);

    if !full_path.exists() || !full_path.is_file() {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Handle Range requests
    if let Some(range) = headers.get("range").and_then(|v| v.to_str().ok()) {
        if let Some(response) = serve_range(&full_path, range) {
            return response;
        }
    }

    // Serve full file
    match tokio::fs::read(&full_path).await {
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

fn serve_range(path: &Path, range: &str) -> Option<Response> {
    // Parse "bytes=START-END" or "bytes=START-"
    let range = range.strip_prefix("bytes=")?;
    let parts: Vec<&str> = range.split('-').collect();
    let start: u64 = parts.first()?.parse().ok()?;
    let end: Option<u64> = parts.get(1).and_then(|s| if s.is_empty() { None } else { s.parse().ok() });

    let file_size = std::fs::metadata(path).ok()?.len();
    let end = end.unwrap_or(file_size - 1).min(file_size - 1);
    let content_length = end - start + 1;

    let file = std::fs::File::open(path).ok()?;
    use std::io::{Read, Seek};
    let mut file = file;
    file.seek(std::io::SeekFrom::Start(start)).ok()?;

    let mut buf = vec![0u8; content_length as usize];
    file.read_exact(&mut buf).ok()?;

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
