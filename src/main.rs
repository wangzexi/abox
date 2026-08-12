use axum::{
    body::Bytes,
    extract::Path,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::fs;

const INDEX_HTML: &str = include_str!("index.html");
const MIN_ID_LEN: usize = 4;

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/", get(index))
        .route("/save", post(post_save))
        .route("/{id}", get(get_content));

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

/// POST /save: 保存内容, 返回最短可用短链
/// - 计算内容 SHA-256, 从 4 位开始逐步加长, 直到不与已有内容冲突
/// - 内容相同则复用已有 id (幂等, 链接不变)
async fn post_save(headers: HeaderMap, body: Bytes) -> Response {
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

    let hash = sha256_hex(&body);

    // 从 4 位开始找最短不冲突的 id
    let mut n = MIN_ID_LEN;
    let id = loop {
        let candidate = &hash[..n];
        let matches = find_files(candidate).await;

        // 无冲突 → 可用
        if matches.is_empty() {
            break candidate.to_string();
        }
        // 有同名文件: 内容相同则复用, 否则加长
        let mut same = false;
        for p in &matches {
            if let Ok(c) = fs::read(p).await {
                if sha256_hex(&c) == hash {
                    same = true;
                    break;
                }
            }
        }
        if same {
            break candidate.to_string();
        }
        n += 1;
    };

    // 覆盖同 id 的旧文件 (内容相同但类型/扩展名可能变化)
    for p in find_files(&id).await {
        let _ = fs::remove_file(p).await;
    }

    let name = format!("{}{}", id, ext_with_dot(&file_ext));
    let path = PathBuf::from("data").join(&name);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent).await;
    }
    let _ = fs::write(&path, &body).await;

    println!("saved {}/{} ({} bytes)", id, name, body.len());
    (
        [(header::LOCATION, format!("/{}", id))],
        StatusCode::SEE_OTHER,
    )
        .into_response()
}

/// GET /{id}: 按 id 精确匹配 (文件名去掉扩展名 == id)
async fn get_content(Path(id): Path<String>) -> Response {
    let mut entries = match fs::read_dir("data").await {
        Ok(e) => e,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().to_string();
        let stem = std::path::Path::new(&name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");
        if stem != id {
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

/// 查找 data/ 下所有 "文件名(去扩展名) == id" 的文件
async fn find_files(id: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(mut entries) = fs::read_dir("data").await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().to_string();
            let stem = std::path::Path::new(&name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if stem == id {
                out.push(entry.path());
            }
        }
    }
    out
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

fn is_inline(ct: &str) -> bool {
    ["text", "image", "audio", "video", "application/pdf", "application/json"]
        .iter()
        .any(|t| ct.starts_with(t))
}

fn ext_with_dot(ext: &str) -> String {
    if ext.is_empty() {
        String::new()
    } else {
        format!(".{}", ext.trim_start_matches('.'))
    }
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
