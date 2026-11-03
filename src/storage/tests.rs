use super::*;
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
#[tokio::test]
async fn expiry_renewal_and_capacity_are_enforced() {
    let dir = TestDir::new();
    let store = Store::open(dir.0.clone(), 10).await.unwrap();
    let a = put(&store, 5).await;
    let b = put(&store, 5).await;
    assert!(a.expires_at >= now() + DEFAULT_LIFETIME_MS - 1000);
    let (pending, file) = store.pending().await.unwrap();
    file.set_len(1).await.unwrap();
    drop(file);
    assert!(store
        .commit(
            &pending,
            Upload {
                size: 1,
                content_type: "text/plain".into(),
                filename: None
            },
            Identity {
                target: None,
                key_hash: hash_key("test-only")
            }
        )
        .await
        .is_err());
    assert!(store.renew(&a.id, "wrong", 100).await.is_err());
    let renewed = store.renew(&a.id, "test-only", 60).await.unwrap();
    assert!(renewed.expires_at <= now() + 60_000);
    {
        let mut records = store.records.lock().await;
        let expired = records.get_mut(&a.id).unwrap();
        expired.expires_at = now() - 1;
        store.write_metadata(expired).await.unwrap();
    }
    assert!(store.read(&a.id).await.unwrap().is_none());
    store.expire().await.unwrap();
    assert!(!dir.0.join(&a.filename).exists());
    assert!(store.read(&b.id).await.unwrap().is_some());
    drop(store);
    let store = Store::open(dir.0.clone(), 10).await.unwrap();
    assert!(store.read(&a.id).await.unwrap().is_none());
    assert!(store.read(&b.id).await.unwrap().is_some());
}

#[tokio::test]
async fn older_metadata_gets_one_fresh_expiry() {
    let dir = TestDir::new();
    let store = Store::open(dir.0.clone(), 100).await.unwrap();
    let original = put(&store, 5).await;
    drop(store);
    let path = dir
        .0
        .join(".metadata")
        .join(format!("{}.json", original.id));
    let mut old: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).await.unwrap()).unwrap();
    old.as_object_mut().unwrap().remove("expires_at");
    let old_protection = now() + 30 * 86_400_000;
    old["protected_until"] = serde_json::json!(old_protection);
    fs::write(&path, serde_json::to_vec(&old).unwrap())
        .await
        .unwrap();
    let store = Store::open(dir.0.clone(), 100).await.unwrap();
    let migrated = store.read(&original.id).await.unwrap().unwrap().0;
    assert_eq!(migrated.expires_at, old_protection);
    drop(store);
    let restarted = Store::open(dir.0.clone(), 100).await.unwrap();
    assert_eq!(
        restarted
            .read(&original.id)
            .await
            .unwrap()
            .unwrap()
            .0
            .expires_at,
        migrated.expires_at
    );
}
#[tokio::test]
async fn managed_updates_switch_versions_and_keep_key_after_restart() {
    let dir = TestDir::new();
    let store = Store::open(dir.0.clone(), 10).await.unwrap();
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
    let store = Store::open(dir.0.clone(), 10).await.unwrap();
    assert!(!orphan.exists());
    assert_eq!(store.authorize(&updated.id, key).await, Some(true));
    assert_eq!(store.authorize(&updated.id, "incorrect").await, Some(false));
    assert_eq!(store.read(&updated.id).await.unwrap().unwrap().0.size, 6);
    assert!(store.delete(&updated.id, "incorrect").await.is_err());
    assert!(store.delete(&updated.id, key).await.unwrap());
    drop(store);
    let store = Store::open(dir.0.clone(), 10).await.unwrap();
    assert!(store.read(&updated.id).await.unwrap().is_none());
}
#[tokio::test]
async fn failed_metadata_switch_keeps_previous_content() {
    let dir = TestDir::new();
    let store = Store::open(dir.0.clone(), 10).await.unwrap();
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
    let store = Store::open(dir.0.clone(), 10).await.unwrap();
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
    let store = Store::open(dir.0.clone(), 100).await.unwrap();
    let (record, _) = store.read("abcd").await.unwrap().unwrap();
    assert_eq!(record.content_type, "text/plain");
    assert!(record.expires_at >= now() + DEFAULT_LIFETIME_MS - 1000);
    let registered = record.created_at;
    let stored: Record =
        serde_json::from_slice(&fs::read(dir.0.join(".metadata/abcd.json")).await.unwrap())
            .unwrap();
    assert_eq!(stored.created_at, registered);
    let restarted = Store::open(dir.0.clone(), 100).await.unwrap();
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
