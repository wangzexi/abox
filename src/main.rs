use axum::{
    body::Bytes,
    extract::Path,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use std::path::PathBuf;
use tokio::fs;

const INDEX_HTML: &str = include_str!("index.html");

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(index))
        .route("/{id}", get(get_content).post(post_content));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("服务器已启动在端口 3000");
    axum::serve(listener, app).await.unwrap();
}

async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        INDEX_HTML,
    )
}

async fn get_content(Path(id): Path<String>) -> Response {
    let mut entries = match fs::read_dir("data").await {
        Ok(e) => e,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&id) {
            continue;
        }
        let content = match fs::read(entry.path()).await {
            Ok(c) => c,
            Err(_) => continue,
        };
        let ext = std::path::Path::new(&name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();

        let mut content_type = mime_guess::from_ext(&ext)
            .first()
            .map(|m| m.to_string())
            .unwrap_or_else(|| "text/plain".to_string());
        if !content_type.contains("charset") {
            content_type.push_str("; charset=utf-8");
        }

        let mut resp = Response::builder()
            .header(header::CONTENT_TYPE, content_type.clone())
            .body(axum::body::Body::from(content))
            .unwrap();

        if !is_inline(&content_type) {
            let filename = format!("{}.{}", id, ext);
            resp.headers_mut().insert(
                header::CONTENT_DISPOSITION,
                header::HeaderValue::from_str(&format!(
                    "attachment; filename=\"{}\"",
                    filename
                ))
                .unwrap(),
            );
        }
        return resp;
    }
    StatusCode::NOT_FOUND.into_response()
}

async fn post_content(
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // 删除旧文件
    if let Ok(mut entries) = fs::read_dir("data").await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&id) {
                let _ = fs::remove_file(entry.path()).await;
            }
        }
    }

    // 写入新文件
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/plain")
        .to_string();

    let file_ext = headers
        .get("x-file-extension")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| ext_for(&content_type));

    let path = PathBuf::from("data").join(format!("{}{}", id, if file_ext.is_empty() { String::new() } else { format!(".{}", file_ext.trim_start_matches('.')) }));
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent).await;
    }
    let _ = fs::write(&path, &body).await;

    (
        [(header::LOCATION, format!("/{}", id))],
        StatusCode::SEE_OTHER,
    )
        .into_response()
}

fn is_inline(ct: &str) -> bool {
    ["text", "image", "audio", "video", "application/pdf", "application/json"]
        .iter()
        .any(|t| ct.starts_with(t))
}

// ext_for 从 Content-Type 推断扩展名 (对应前端 mime.extension)
fn ext_for(content_type: &str) -> String {
    let ct = content_type.split(';').next().unwrap_or("").trim();
    match ct {
        "text/plain" => ".txt".into(),
        "text/html" => ".html".into(),
        "text/markdown" => ".md".into(),
        "text/csv" => ".csv".into(),
        "application/json" => ".json".into(),
        "application/pdf" => ".pdf".into(),
        "application/zip" => ".zip".into(),
        "application/javascript" => ".js".into(),
        "image/png" => ".png".into(),
        "image/jpeg" => ".jpg".into(),
        "image/gif" => ".gif".into(),
        "image/webp" => ".webp".into(),
        "image/svg+xml" => ".svg".into(),
        "audio/mpeg" => ".mp3".into(),
        "video/mp4" => ".mp4".into(),
        _ => String::new(),
    }
}
