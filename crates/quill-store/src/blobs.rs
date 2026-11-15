//! Raw messages on disk, named by the SHA-256 of their contents.
//!
//! Content addressing means the same message downloaded twice is stored
//! once, and a file can be checked against its name.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

pub struct BlobStore {
    place: Place,
}

enum Place {
    Disk(PathBuf),
    /// For the demo mailbox and tests, which write nothing.
    Memory(Mutex<HashMap<String, Vec<u8>>>),
}

impl BlobStore {
    /// Blobs live under `root`, which is created on first write.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            place: Place::Disk(root.into()),
        }
    }

    /// Blobs held in memory, gone when the store is dropped.
    pub fn in_memory() -> Self {
        Self {
            place: Place::Memory(Mutex::new(HashMap::new())),
        }
    }

    /// The name a blob with these contents has.
    pub fn hash(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn path(root: &Path, hash: &str) -> Option<PathBuf> {
        // Only our own hashes: never a path someone else chose.
        let valid = hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        valid.then(|| root.join(&hash[..2]).join(&hash[2..]))
    }

    /// Stores `bytes` and returns their hash. Storing the same bytes again
    /// is free.
    pub fn put(&self, bytes: &[u8]) -> io::Result<String> {
        let hash = Self::hash(bytes);
        let root = match &self.place {
            Place::Disk(root) => root,
            Place::Memory(blobs) => {
                blobs
                    .lock()
                    .map_err(|_| io::Error::other("the blob store was poisoned"))?
                    .entry(hash.clone())
                    .or_insert_with(|| bytes.to_vec());
                return Ok(hash);
            }
        };
        let Some(path) = Self::path(root, &hash) else {
            return Err(io::Error::other("unreachable: a SHA-256 is 64 hex digits"));
        };
        if path.is_file() {
            return Ok(hash);
        }
        let Some(dir) = path.parent() else {
            return Err(io::Error::other("a blob path has a parent"));
        };
        create_private_dir(dir)?;
        // Written under another name and renamed, so a reader never sees
        // half a message and a crash leaves no blob with the wrong contents.
        let partial = dir.join(format!(".{}.{}", &hash[2..], std::process::id()));
        let mut file = private_file(&partial)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&partial, &path)?;
        Ok(hash)
    }

    /// The bytes stored under `hash`, or `None` if there are none.
    pub fn get(&self, hash: &str) -> io::Result<Option<Vec<u8>>> {
        let root = match &self.place {
            Place::Disk(root) => root,
            Place::Memory(blobs) => {
                return Ok(blobs
                    .lock()
                    .map_err(|_| io::Error::other("the blob store was poisoned"))?
                    .get(hash)
                    .cloned());
            }
        };
        let Some(path) = Self::path(root, hash) else {
            return Ok(None);
        };
        match fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

fn private_file(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blobs_in_memory_are_stored_and_read_back() {
        let blobs = BlobStore::in_memory();
        let hash = blobs.put(b"Subject: hello\r\n\r\nbody\r\n").unwrap();
        assert_eq!(hash, BlobStore::hash(b"Subject: hello\r\n\r\nbody\r\n"));
        assert_eq!(blobs.put(b"Subject: hello\r\n\r\nbody\r\n").unwrap(), hash);
        assert_eq!(
            blobs.get(&hash).unwrap().as_deref(),
            Some(&b"Subject: hello\r\n\r\nbody\r\n"[..])
        );
        assert_eq!(blobs.get(&"0".repeat(64)).unwrap(), None);
        assert_eq!(blobs.get("../etc/passwd").unwrap(), None);
    }

    #[test]
    fn a_blob_is_stored_under_its_hash_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::new(dir.path().join("blobs"));
        let hash = blobs.put(b"Subject: hello\r\n\r\nbody\r\n").unwrap();
        assert_eq!(hash, BlobStore::hash(b"Subject: hello\r\n\r\nbody\r\n"));
        assert_eq!(hash.len(), 64);
        assert_eq!(
            blobs.get(&hash).unwrap().as_deref(),
            Some(&b"Subject: hello\r\n\r\nbody\r\n"[..])
        );
        let path = dir.path().join("blobs").join(&hash[..2]).join(&hash[2..]);
        assert!(path.is_file());
    }

    #[test]
    fn the_same_bytes_are_stored_once_and_no_partial_file_is_left() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::new(dir.path());
        let first = blobs.put(b"same").unwrap();
        let second = blobs.put(b"same").unwrap();
        assert_eq!(first, second);
        let shard = dir.path().join(&first[..2]);
        let names: Vec<_> = fs::read_dir(shard)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
    }

    #[test]
    fn a_missing_or_malformed_hash_reads_as_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::new(dir.path());
        assert_eq!(blobs.get(&"0".repeat(64)).unwrap(), None);
        for bad in [
            "",
            "abc",
            "../../etc/passwd",
            &"g".repeat(64),
            &"0".repeat(65),
        ] {
            assert_eq!(blobs.get(bad).unwrap(), None, "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn blobs_are_private_to_their_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let blobs = BlobStore::new(dir.path().join("blobs"));
        let hash = blobs.put(b"private mail").unwrap();
        let shard = dir.path().join("blobs").join(&hash[..2]);
        assert_eq!(
            fs::metadata(&shard).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let file = shard.join(&hash[2..]);
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
