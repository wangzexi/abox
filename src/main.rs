mod api;
mod disposition;
mod storage;
use std::path::PathBuf;
use storage::Store;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let store = Store::open(
        PathBuf::from(std::env::var("ABOX_DATA_DIR").unwrap_or_else(|_| "data".into())),
        capacity("ABOX_SMALL_CAPACITY_MIB", 256)?,
        capacity("ABOX_LARGE_CAPACITY_MIB", 384)?,
    )
    .await?;
    let base = std::env::var("ABOX_PUBLIC_URL")
        .unwrap_or_else(|_| "http://localhost:3000".into())
        .trim_end_matches('/')
        .to_owned();
    let app = api::router(store, base);
    let address = std::env::var("ABOX_LISTEN").unwrap_or_else(|_| "0.0.0.0:3000".into());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    println!("Abox listening on {address}");
    axum::serve(listener, app).await
}
fn capacity(name: &str, default: u64) -> std::io::Result<u64> {
    let value = match std::env::var(name) {
        Ok(value) => value.parse::<u64>().map_err(std::io::Error::other)?,
        Err(_) => default,
    };
    value
        .checked_mul(1024 * 1024)
        .filter(|v| *v > 0)
        .ok_or_else(|| std::io::Error::other(format!("invalid {name}")))
}
