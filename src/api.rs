use crate::{
    disposition,
    storage::{self, Record, Store, MAX_UPLOAD},
};
use axum::{
    body::Body,
    extract::{OriginalUri, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
    Json, Router,
};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use tokio::{io::AsyncWriteExt, sync::Semaphore};
use tokio_util::io::ReaderStream;

const INDEX: &str = include_str!("web/index.html");
const API_INSTRUCTIONS: &str = include_str!("web/api.txt");
#[derive(Clone)]
struct App {
    store: Arc<Store>,
    uploads: Arc<Semaphore>,
    base: String,
}

pub fn router(store: Store, base: String) -> Router {
    Router::new()
        .route("/", get(index).post(upload))
        .route("/save", get(content).post(legacy_upload))
        .route("/assets/style.css", get(styles))
        .route("/assets/preview.js", get(preview_script))
        .route("/{id}", get(content).put(update).delete(remove))
        .with_state(App {
            store: Arc::new(store),
            uploads: Arc::new(Semaphore::new(1)),
            base,
        })
}
async fn index(headers: HeaderMap) -> Response {
    let wants_html = headers
        .get(header::ACCEPT)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|accept| {
            accept.split(',').any(|entry| {
                let mut parts = entry.split(';');
                parts
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/html"))
                    && !parts.any(|param| {
                        param
                            .trim()
                            .strip_prefix("q=")
                            .and_then(|q| q.parse::<f32>().ok())
                            .is_some_and(|q| q <= 0.0)
                    })
            })
        });
    let mut response = if wants_html {
        Html(INDEX.replace("__API_INSTRUCTIONS__", API_INSTRUCTIONS)).into_response()
    } else {
        (
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            API_INSTRUCTIONS,
        )
            .into_response()
    };
    response
        .headers_mut()
        .insert(header::VARY, "Accept".parse().unwrap());
    response
}
async fn styles() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("web/style.css"),
    )
}
async fn preview_script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("web/preview.js"),
    )
}
fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}
fn internal(e: std::io::Error) -> Response {
    eprintln!("storage error: {e}");
    error(
        StatusCode::INSUFFICIENT_STORAGE,
        "Storage operation failed; the upload was not accepted. Please retry later.",
    )
}
fn details(record: &Record, base: &str) -> serde_json::Value {
    json!({ "url": format!("{base}/{}", record.id) })
}
async fn upload(State(app): State<App>, headers: HeaderMap, body: Body) -> Response {
    save(app, headers, body, false, None).await
}
async fn legacy_upload(State(app): State<App>, headers: HeaderMap, body: Body) -> Response {
    save(app, headers, body, true, None).await
}
fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|value| {
            let (scheme, key) = value.split_once(' ')?;
            (scheme.eq_ignore_ascii_case("Bearer") && !key.is_empty()).then_some(key)
        })
}
async fn check_key(app: &App, id: &str, headers: &HeaderMap) -> Result<(), Response> {
    if !storage::valid_id(id) {
        return Err(StatusCode::NOT_FOUND.into_response());
    }
    let Some(key) = bearer(headers) else {
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "Supply Authorization: Bearer <key>.",
        ));
    };
    match app.store.authorize(id, key).await {
        None => Err(StatusCode::NOT_FOUND.into_response()),
        Some(false) => Err(error(StatusCode::FORBIDDEN, "Invalid key.")),
        Some(true) => Ok(()),
    }
}
async fn update(
    State(app): State<App>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if let Err(response) = check_key(&app, &id, &headers).await {
        return response;
    }
    save(app, headers, body, false, Some(id)).await
}
async fn remove(State(app): State<App>, Path(id): Path<String>, headers: HeaderMap) -> Response {
    if let Err(response) = check_key(&app, &id, &headers).await {
        return response;
    }
    let key = bearer(&headers).unwrap().to_owned();
    match tokio::spawn(async move { app.store.delete(&id, &key).await }).await {
        Ok(Ok(true)) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(false)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => mutation_error(e),
        Err(e) => internal(std::io::Error::other(e)),
    }
}
fn mutation_error(e: std::io::Error) -> Response {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NOT_FOUND.into_response(),
        std::io::ErrorKind::PermissionDenied => error(StatusCode::FORBIDDEN, "Invalid key."),
        _ => internal(e),
    }
}
async fn save(
    app: App,
    headers: HeaderMap,
    body: Body,
    legacy: bool,
    target: Option<String>,
) -> Response {
    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .is_some_and(|n| n > MAX_UPLOAD)
    {
        return error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Maximum upload size is 256 MiB.",
        );
    }
    let Ok(_permit) = app.uploads.clone().try_acquire_owned() else {
        let mut response = error(
            StatusCode::TOO_MANY_REQUESTS,
            "Another upload is in progress; retry shortly.",
        );
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, "5".parse().unwrap());
        return response;
    };
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .unwrap_or("application/octet-stream");
    if content_type.starts_with("multipart/") {
        return error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Send raw file bytes with --data-binary, not multipart/form-data.",
        );
    }
    let original_name = match headers.get(header::CONTENT_DISPOSITION) {
        Some(value) => match value.to_str().ok().map(disposition::filename) {
            Some(Ok(name)) => name,
            Some(Err(message)) => return error(StatusCode::BAD_REQUEST, message),
            None => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "Use filename*=UTF-8''encoded-name for a Unicode filename.",
                )
            }
        },
        None => None,
    };
    let (pending, mut file) = match app.store.pending().await {
        Ok(p) => p,
        Err(e) => return internal(e),
    };
    let mut stream = body.into_data_stream();
    let mut size = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(_) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "Upload interrupted or invalid request body.",
                )
            }
        };
        size += chunk.len() as u64;
        if size > MAX_UPLOAD {
            return error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Maximum upload size is 256 MiB.",
            );
        }
        if let Err(e) = file.write_all(&chunk).await {
            return internal(e);
        }
    }
    if let Err(e) = file.sync_all().await {
        return internal(e);
    }
    drop(file);
    let key = if target.is_none() {
        storage::new_key()
    } else {
        bearer(&headers).unwrap().to_owned()
    };
    let creating = target.is_none();
    let identity = storage::Identity {
        target,
        key_hash: storage::hash_key(&key),
    };
    let store = app.store.clone();
    let content_type = content_type.to_owned();
    // Complete the metadata switch even if the HTTP client disconnects mid-commit.
    let record = match tokio::spawn(async move {
        let _permit = _permit;
        store
            .commit(
                &pending,
                storage::Upload {
                    size,
                    content_type,
                    filename: original_name,
                },
                identity,
            )
            .await
    })
    .await
    {
        Ok(Ok(record)) => record,
        Ok(Err(e)) => return mutation_error(e),
        Err(e) => return internal(std::io::Error::other(e)),
    };
    let wants_json = headers
        .get(header::ACCEPT)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|a| a.contains("application/json"));
    if legacy && !wants_json {
        return (
            StatusCode::SEE_OTHER,
            [(header::LOCATION, format!("/{}", record.id))],
        )
            .into_response();
    }
    let mut result = details(&record, &app.base);
    if creating {
        result["key"] = json!(key);
    }
    (
        if creating {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        [
            (header::LOCATION, format!("/{}", record.id)),
            (header::CACHE_CONTROL, "no-store".into()),
        ],
        Json(result),
    )
        .into_response()
}
#[derive(Deserialize)]
struct Download {
    download: Option<String>,
}
async fn content(
    State(app): State<App>,
    OriginalUri(uri): OriginalUri,
    Query(query): Query<Download>,
) -> Response {
    let id = uri.path().trim_start_matches('/');
    if !storage::valid_id(id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let (record, file) = match app.store.read(id).await {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal(e),
    };
    let inline = [
        "text/",
        "image/",
        "audio/",
        "video/",
        "application/pdf",
        "application/json",
    ]
    .iter()
    .any(|t| record.content_type.starts_with(t));
    let disposition = if query.download.is_some() || !inline {
        "attachment"
    } else {
        "inline"
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &record.content_type)
        .header(header::CONTENT_LENGTH, record.size)
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        // Uploaded documents cannot obtain the application's origin, even when opened directly.
        .header(
            "content-security-policy",
            "sandbox allow-scripts allow-forms allow-popups allow-downloads",
        )
        .header(
            header::CONTENT_DISPOSITION,
            disposition::response_header(disposition, record.download_name()),
        )
        .body(Body::from_stream(ReaderStream::new(file)))
        .unwrap()
}
