use super::*;
const MIB: u64 = 1024 * 1024;
struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("abox-test-{}", random_id())))
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
async fn put(store: &Store, size: u64) -> Record {
    let (pending, file) = store.pending().await.unwrap();
    file.set_len(size).await.unwrap();
    drop(file);
    store
        .commit(
            &pending,
            Upload {
                size,
                content_type: "application/octet-stream".into(),
                filename: None,
            },
            Identity {
                target: None,
                key_hash: hash_key("test-only"),
            },
        )
        .await
        .unwrap()
}
async fn unprotect(store: &Store, id: &str, age: u64) {
    let mut records = store.records.lock().await;
    let r = records.get_mut(id).unwrap();
    r.protected_until = 0;
    r.last_accessed_at = age;
    store.write_metadata(r).await.unwrap();
}
#[tokio::test]
async fn bucket_boundary_and_large_eviction_leave_small_files_intact() {
    let dir = TestDir::new();
    let store = Store::open(dir.0.clone(), 200 * MIB, 256 * MIB)
        .await
        .unwrap();
    let small = put(&store, 100 * MIB).await;
    let large = put(&store, 100 * MIB + 1).await;
    unprotect(&store, &large.id, 1).await;
    let replacement = put(&store, 256 * MIB).await;
    assert!(store.read(&large.id).await.unwrap().is_none());
    assert!(store.read(&small.id).await.unwrap().is_some());
    assert!(store.read(&replacement.id).await.unwrap().is_some());
    assert!(!dir.0.join(large.filename).exists());
}
#[tokio::test]
async fn successful_access_changes_lru_victim_and_survives_restart() {
    let dir = TestDir::new();
    let store = Store::open(dir.0.clone(), 10, 256 * MIB).await.unwrap();
    let a = put(&store, 5).await;
    let b = put(&store, 5).await;
    unprotect(&store, &a.id, 1).await;
    unprotect(&store, &b.id, 2).await;
    store.read(&a.id).await.unwrap().unwrap();
    drop(store);
    let store = Store::open(dir.0.clone(), 10, 256 * MIB).await.unwrap();
    put(&store, 5).await;
    assert!(store.read(&a.id).await.unwrap().is_some());
    assert!(store.read(&b.id).await.unwrap().is_none());
}
#[tokio::test]
async fn full_protected_bucket_rejects_upload_without_partial_eviction() {
    let dir = TestDir::new();
    let store = Store::open(dir.0.clone(), 10, 256 * MIB).await.unwrap();
    let a = put(&store, 5).await;
    let b = put(&store, 5).await;
    unprotect(&store, &a.id, 1).await;
    let (pending, file) = store.pending().await.unwrap();
    file.set_len(6).await.unwrap();
    drop(file);
    assert!(store
        .commit(
            &pending,
            Upload {
                size: 6,
                content_type: "application/octet-stream".into(),
                filename: None
            },
            Identity {
                target: None,
                key_hash: hash_key("test-only")
            }
        )
        .await
        .is_err());
    assert!(store.read(&a.id).await.unwrap().is_some());
    assert!(store.read(&b.id).await.unwrap().is_some());
}
#[tokio::test]
async fn managed_updates_switch_versions_and_keep_key_after_restart() {
    let dir = TestDir::new();
    let store = Store::open(dir.0.clone(), 10, 256 * MIB).await.unwrap();
    let key = "test-only-management-key";
    let (pending, file) = store.pending().await.unwrap();
    file.set_len(5).await.unwrap();
    drop(file);
    let first = store
        .commit(
            &pending,
            Upload {
                size: 5,
                content_type: "text/plain".into(),
                filename: Some("note.txt".to_owned()),
            },
            Identity {
                target: None,
                key_hash: hash_key(key),
            },
        )
        .await
        .unwrap();
    let (pending, file) = store.pending().await.unwrap();
    file.set_len(6).await.unwrap();
    drop(file);
    let updated = store
        .commit(
            &pending,
            Upload {
                size: 6,
                content_type: "text/html".into(),
                filename: None,
            },
            Identity {
                target: Some(first.id.clone()),
                key_hash: hash_key(key),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.id, first.id);
    assert_eq!(updated.download_name(), "note.txt");
    assert!(!dir.0.join(first.filename).exists());
    let orphan = dir.0.join(".objects/orphan-version");
    fs::write(&orphan, "interrupted").await.unwrap();
    drop(store);
    let store = Store::open(dir.0.clone(), 10, 256 * MIB).await.unwrap();
    assert!(!orphan.exists());
    assert_eq!(store.authorize(&updated.id, key).await, Some(true));
    assert_eq!(store.authorize(&updated.id, "incorrect").await, Some(false));
    assert_eq!(store.read(&updated.id).await.unwrap().unwrap().0.size, 6);
    assert!(store.delete(&updated.id, "incorrect").await.is_err());
    assert!(store.delete(&updated.id, key).await.unwrap());
    drop(store);
    let store = Store::open(dir.0.clone(), 10, 256 * MIB).await.unwrap();
    assert!(store.read(&updated.id).await.unwrap().is_none());
}
#[tokio::test]
async fn failed_metadata_switch_keeps_previous_content() {
    let dir = TestDir::new();
    let store = Store::open(dir.0.clone(), 10, 256 * MIB).await.unwrap();
    let hash = hash_key("test-only-key");
    let (pending, file) = store.pending().await.unwrap();
    file.set_len(5).await.unwrap();
    drop(file);
    let original = store
        .commit(
            &pending,
            Upload {
                size: 5,
                content_type: "text/plain".into(),
                filename: None,
            },
            Identity {
                target: None,
                key_hash: hash.clone(),
            },
        )
        .await
        .unwrap();
    let bad_temp = dir.0.join(".metadata").join(format!("{}.tmp", original.id));
    fs::create_dir(&bad_temp).await.unwrap();
    let (pending, file) = store.pending().await.unwrap();
    file.set_len(6).await.unwrap();
    drop(file);
    assert!(store
        .commit(
            &pending,
            Upload {
                size: 6,
                content_type: "text/plain".into(),
                filename: None
            },
            Identity {
                target: Some(original.id.clone()),
                key_hash: hash
            }
        )
        .await
        .is_err());
    fs::remove_dir(bad_temp).await.unwrap();
    drop(store);
    let store = Store::open(dir.0.clone(), 10, 256 * MIB).await.unwrap();
    let kept = store.read(&original.id).await.unwrap().unwrap().0;
    assert_eq!(kept.size, 5);
    assert_eq!(kept.filename, original.filename);
}
#[tokio::test]
async fn legacy_files_keep_links_and_interrupted_uploads_are_removed() {
    let dir = TestDir::new();
    fs::create_dir_all(&dir.0).await.unwrap();
    fs::write(dir.0.join("abcd.txt"), "legacy").await.unwrap();
    fs::write(dir.0.join(".upload-abandoned"), "partial")
        .await
        .unwrap();
    let store = Store::open(dir.0.clone(), 100, 256 * MIB).await.unwrap();
    let (record, _) = store.read("abcd").await.unwrap().unwrap();
    assert_eq!(record.content_type, "text/plain");
    assert!(record.protected_until >= now() + 30 * 86_400_000 - 1000);
    let registered = record.created_at;
    let stored: Record =
        serde_json::from_slice(&fs::read(dir.0.join(".metadata/abcd.json")).await.unwrap())
            .unwrap();
    assert_eq!(stored.created_at, registered);
    let restarted = Store::open(dir.0.clone(), 100, 256 * MIB).await.unwrap();
    assert_eq!(
        restarted.read("abcd").await.unwrap().unwrap().0.created_at,
        registered
    );
    assert!(!dir.0.join(".upload-abandoned").exists());
    let (pending, file) = store.pending().await.unwrap();
    let path = pending.path.clone();
    drop(file);
    drop(pending);
    assert!(!path.exists());
}
