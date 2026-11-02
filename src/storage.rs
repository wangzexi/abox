use rand::distr::{Alphanumeric, SampleString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{fs, sync::Mutex};

pub const MAX_UPLOAD: u64 = 256 * 1024 * 1024;
pub const LARGE_THRESHOLD: u64 = 100 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub filename: String,
    #[serde(default)]
    pub original_name: Option<String>,
    #[serde(default)]
    pub key_hash: Option<String>,
    pub content_type: String,
    pub size: u64,
    pub created_at: u64,
    pub last_accessed_at: u64,
    #[serde(default)]
    pub protected_until: u64,
}
impl Record {
    pub fn download_name(&self) -> &str {
        self.original_name.as_deref().unwrap_or(&self.filename)
    }
}

pub struct Upload {
    pub size: u64,
    pub content_type: String,
    pub filename: Option<String>,
}

pub struct Identity {
    pub target: Option<String>,
    pub key_hash: String,
}

pub fn new_key() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), 32)
}
pub fn hash_key(key: &str) -> String {
    format!("{:x}", Sha256::digest(key.as_bytes()))
}

pub struct Store {
    root: PathBuf,
    records: Mutex<HashMap<String, Record>>,
    small_capacity: u64,
    large_capacity: u64,
}

// Removing an unfinished upload also works when the request future is cancelled.
pub struct PendingUpload {
    pub path: PathBuf,
}
impl Drop for PendingUpload {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub fn random_id() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), 12)
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

impl Store {
    pub async fn open(root: PathBuf, small_capacity: u64, large_capacity: u64) -> io::Result<Self> {
        fs::create_dir_all(root.join(".metadata")).await?;
        fs::create_dir_all(root.join(".objects")).await?;
        let mut records = HashMap::new();
        let mut migrated = Vec::new();
        let mut metadata = fs::read_dir(root.join(".metadata")).await?;
        while let Some(entry) = metadata.next_entry().await? {
            if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let r: Record = serde_json::from_slice(&fs::read(entry.path()).await?)?;
            if !r.filename.starts_with(".objects/") {
                continue;
            }
            let object = r.filename.strip_prefix(".objects/").unwrap();
            if !valid_id(&r.id)
                || !object
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                || entry.file_name().to_string_lossy() != format!("{}.json", r.id)
            {
                return Err(io::Error::other("invalid object metadata"));
            }
            if fs::metadata(root.join(&r.filename)).await?.len() != r.size {
                return Err(io::Error::other("object size mismatch"));
            }
            records.insert(r.id.clone(), r);
        }
        // Unreferenced object versions are interrupted writes or deletions, never public files.
        let mut objects = fs::read_dir(root.join(".objects")).await?;
        let referenced: std::collections::HashSet<_> =
            records.values().map(|r| r.filename.clone()).collect();
        while let Some(entry) = objects.next_entry().await? {
            let name = format!(".objects/{}", entry.file_name().to_string_lossy());
            if !referenced.contains(&name) {
                fs::remove_file(entry.path()).await?;
            }
        }
        let mut files = fs::read_dir(&root).await?;
        while let Some(entry) = files.next_entry().await? {
            let filename = entry.file_name().to_string_lossy().to_string();
            if filename.starts_with('.') {
                // One process owns this directory; staging files left by crashes are disposable.
                if filename.starts_with(".upload-") {
                    fs::remove_file(entry.path()).await?;
                }
                continue;
            }
            let stat = entry.metadata().await?;
            if !stat.is_file() {
                continue;
            }
            let Some(id) = Path::new(&filename).file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !valid_id(id) {
                continue;
            }
            if records.contains_key(id) {
                return Err(io::Error::other(format!("duplicate stored id: {id}")));
            }
            let meta_path = root.join(".metadata").join(format!("{id}.json"));
            let record = match fs::read(&meta_path).await {
                Ok(bytes) => {
                    let r: Record = serde_json::from_slice(&bytes)?;
                    if r.id != id || r.filename != filename || r.size != stat.len() {
                        return Err(io::Error::other(format!("invalid metadata for {id}")));
                    }
                    r
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    // Register existing links once with a fresh 30-day protection period.
                    let timestamp = now();
                    let record = Record {
                        id: id.into(),
                        filename: filename.clone(),
                        original_name: None,
                        key_hash: None,
                        content_type: mime_guess::from_path(&filename)
                            .first_or_octet_stream()
                            .to_string(),
                        size: stat.len(),
                        created_at: timestamp,
                        last_accessed_at: timestamp,
                        protected_until: timestamp + 30 * 86_400_000,
                    };
                    migrated.push(record.clone());
                    record
                }
                Err(e) => return Err(e),
            };
            records.insert(id.to_owned(), record);
        }
        let store = Self {
            root,
            records: Mutex::new(records),
            small_capacity,
            large_capacity,
        };
        for record in &migrated {
            store.write_metadata(record).await?;
        }
        if !migrated.is_empty() {
            println!(
                "Registered {} existing files with 30-day protection",
                migrated.len()
            );
        }
        Ok(store)
    }

    pub async fn pending(&self) -> io::Result<(PendingUpload, fs::File)> {
        loop {
            let path = self.root.join(format!(".upload-{}", random_id()));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .await
            {
                Ok(file) => return Ok((PendingUpload { path }, file)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
    }

    async fn write_metadata(&self, record: &Record) -> io::Result<()> {
        let path = self
            .root
            .join(".metadata")
            .join(format!("{}.json", record.id));
        let temp = path.with_extension("tmp");
        let bytes = serde_json::to_vec(record)?;
        fs::write(&temp, bytes).await?;
        fs::rename(temp, path).await
    }

    pub async fn commit(
        &self,
        pending: &PendingUpload,
        upload: Upload,
        identity: Identity,
    ) -> io::Result<Record> {
        let Upload {
            size,
            content_type,
            filename: original_name,
        } = upload;
        let mut records = self.records.lock().await;
        let previous = if let Some(target) = identity.target.as_deref() {
            let record = records.get(target).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "file was removed during update")
            })?;
            if !record
                .key_hash
                .as_ref()
                .is_some_and(|h| bool::from(h.as_bytes().ct_eq(identity.key_hash.as_bytes())))
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "invalid key",
                ));
            }
            Some(record.clone())
        } else {
            None
        };
        let previous_id = previous.as_ref().map(|r| r.id.as_str());
        let is_large = size > LARGE_THRESHOLD;
        let capacity = if is_large {
            self.large_capacity
        } else {
            self.small_capacity
        };
        if size > capacity {
            return Err(io::Error::other("file exceeds bucket capacity"));
        }
        let mut used: u64 = records
            .values()
            .filter(|r| {
                (r.size > LARGE_THRESHOLD) == is_large && Some(r.id.as_str()) != previous_id
            })
            .map(|r| r.size)
            .sum();
        let mut victims: Vec<_> = records
            .values()
            .filter(|r| {
                (r.size > LARGE_THRESHOLD) == is_large
                    && r.protected_until <= now()
                    && Some(r.id.as_str()) != previous_id
            })
            .cloned()
            .collect();
        if used + size > capacity + victims.iter().map(|r| r.size).sum::<u64>() {
            return Err(io::Error::other("bucket is full of protected files"));
        }
        victims.sort_by(|a, b| (a.last_accessed_at, &a.id).cmp(&(b.last_accessed_at, &b.id)));
        for victim in victims {
            if used + size <= capacity {
                break;
            }
            // Removing the index first makes an interrupted eviction an unreachable object.
            let metadata = self
                .root
                .join(".metadata")
                .join(format!("{}.json", victim.id));
            match fs::remove_file(metadata).await {
                Ok(()) => (),
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                Err(e) => return Err(e),
            }
            records.remove(&victim.id);
            fs::remove_file(self.root.join(&victim.filename)).await?;
            used -= victim.size;
        }
        let id = if let Some(old) = &previous {
            old.id.clone()
        } else {
            loop {
                let id = random_id();
                if !records.contains_key(&id) {
                    break id;
                }
            }
        };
        let filename = format!(".objects/{id}-{}", random_id());
        let timestamp = now();
        let record = Record {
            id: id.clone(),
            filename,
            original_name: original_name
                .or_else(|| previous.as_ref().and_then(|r| r.original_name.clone()))
                .or_else(|| {
                    let ext = mime_guess::get_mime_extensions_str(
                        content_type.split(';').next().unwrap_or("").trim(),
                    )
                    .and_then(|a| a.first())
                    .unwrap_or(&"bin");
                    Some(format!("{id}.{ext}"))
                }),
            key_hash: Some(identity.key_hash),
            size,
            content_type,
            created_at: previous.as_ref().map(|r| r.created_at).unwrap_or(timestamp),
            last_accessed_at: timestamp,
            protected_until: timestamp + 30 * 86_400_000,
        };
        // Publish a new object version, then atomically switch its metadata pointer.
        fs::rename(&pending.path, self.root.join(&record.filename)).await?;
        if let Err(e) = self.write_metadata(&record).await {
            let _ = fs::remove_file(self.root.join(&record.filename)).await;
            return Err(e);
        }
        records.insert(record.id.clone(), record.clone());
        if let Some(old) = previous {
            if let Err(e) = fs::remove_file(self.root.join(old.filename)).await {
                eprintln!("old object cleanup: {e}");
            }
        }
        Ok(record)
    }

    pub async fn authorize(&self, id: &str, key: &str) -> Option<bool> {
        self.records.lock().await.get(id).map(|r| {
            r.key_hash.as_ref().is_some_and(|expected| {
                bool::from(expected.as_bytes().ct_eq(hash_key(key).as_bytes()))
            })
        })
    }

    pub async fn delete(&self, id: &str, key: &str) -> io::Result<bool> {
        let mut records = self.records.lock().await;
        let Some(record) = records.get(id).cloned() else {
            return Ok(false);
        };
        if !record
            .key_hash
            .as_ref()
            .is_some_and(|h| bool::from(h.as_bytes().ct_eq(hash_key(key).as_bytes())))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "invalid key",
            ));
        }
        // Managed files are reachable only through metadata. Orphan objects are reclaimed at startup.
        fs::remove_file(self.root.join(".metadata").join(format!("{id}.json"))).await?;
        records.remove(id);
        fs::remove_file(self.root.join(record.filename)).await?;
        Ok(true)
    }

    // Open while holding the same lock as eviction: the response owns the open file afterward.
    pub async fn read(&self, id: &str) -> io::Result<Option<(Record, fs::File)>> {
        let mut records = self.records.lock().await;
        let Some(existing) = records.get(id) else {
            return Ok(None);
        };
        let mut record = existing.clone();
        let file = fs::File::open(self.root.join(&record.filename)).await?;
        record.last_accessed_at = now().max(record.last_accessed_at + 1);
        self.write_metadata(&record).await?;
        records.insert(id.to_owned(), record.clone());
        Ok(Some((record, file)))
    }
}

#[cfg(test)]
mod tests;
