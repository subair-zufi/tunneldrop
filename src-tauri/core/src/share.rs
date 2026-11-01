use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

#[cfg(target_os = "android")]
use std::os::fd::{AsRawFd, OwnedFd};
#[cfg(target_os = "android")]
use std::sync::Arc;

/// Where a share's bytes come from.
///
/// Desktop picks files by path. Android's picker hands back a `content://` URI,
/// which is not a path and cannot be opened again later — the app is granted
/// access to the descriptor, not to a location on disk. So there we hold the
/// descriptor the ContentResolver gave us and read from it directly. Dropping
/// the share closes it.
#[derive(Clone, Debug)]
pub enum ShareSource {
    Path(PathBuf),
    #[cfg(target_os = "android")]
    Fd(Arc<OwnedFd>),
}

impl From<PathBuf> for ShareSource {
    fn from(p: PathBuf) -> Self {
        ShareSource::Path(p)
    }
}

#[cfg(target_os = "android")]
impl From<OwnedFd> for ShareSource {
    fn from(fd: OwnedFd) -> Self {
        ShareSource::Fd(Arc::new(fd))
    }
}

impl ShareSource {
    /// Opens the source for reading. Every download gets its own handle: two
    /// people pulling the same share at once must not share a read offset.
    pub fn open(&self) -> std::io::Result<std::fs::File> {
        match self {
            ShareSource::Path(p) => std::fs::File::open(p),
            // Re-opening through /proc/self/fd yields a fresh open file
            // description — its own offset — rather than the dup() that
            // try_clone() would give us. That works for anything backed by a
            // real file, which covers the storage provider. A provider that
            // handed us a pipe has nothing to re-open, so fall back to the dup
            // and accept that it streams once from wherever it is.
            #[cfg(target_os = "android")]
            ShareSource::Fd(fd) => {
                std::fs::File::open(format!("/proc/self/fd/{}", fd.as_raw_fd()))
                    .or_else(|_| fd.try_clone().map(std::fs::File::from))
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Share {
    pub token: String,
    pub source: ShareSource,
    pub name: String,
    pub size: u64,
    pub password_hash: Option<String>,
    pub download_count: u64,
    pub created_at: SystemTime,
}

impl Share {
    /// Creates a new share with download_count 0 and created_at now.
    pub fn new(
        token: String,
        source: impl Into<ShareSource>,
        name: String,
        size: u64,
        password_hash: Option<String>,
    ) -> Self {
        Share {
            token,
            source: source.into(),
            name,
            size,
            password_hash,
            download_count: 0,
            created_at: SystemTime::now(),
        }
    }
}

#[derive(Default)]
pub struct ShareRegistry {
    shares: HashMap<String, Share>,
}

impl ShareRegistry {
    pub fn new() -> Self {
        ShareRegistry { shares: HashMap::new() }
    }

    pub fn insert(&mut self, share: Share) {
        self.shares.insert(share.token.clone(), share);
    }

    pub fn get(&self, token: &str) -> Option<&Share> {
        self.shares.get(token)
    }

    pub fn get_mut(&mut self, token: &str) -> Option<&mut Share> {
        self.shares.get_mut(token)
    }

    pub fn remove(&mut self, token: &str) -> Option<Share> {
        self.shares.remove(token)
    }

    pub fn list(&self) -> Vec<Share> {
        self.shares.values().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.shares.is_empty()
    }

    pub fn len(&self) -> usize {
        self.shares.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(token: &str) -> Share {
        Share::new(token.to_string(), PathBuf::from("/tmp/x"), "x".into(), 10, None)
    }

    #[test]
    fn insert_and_get() {
        let mut r = ShareRegistry::new();
        r.insert(sample("abc"));
        assert_eq!(r.get("abc").unwrap().name, "x");
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn remove_makes_it_gone() {
        let mut r = ShareRegistry::new();
        r.insert(sample("abc"));
        assert!(r.remove("abc").is_some());
        assert!(r.get("abc").is_none());
        assert!(r.is_empty());
    }

    #[test]
    fn increment_download_count_via_get_mut() {
        let mut r = ShareRegistry::new();
        r.insert(sample("abc"));
        r.get_mut("abc").unwrap().download_count += 1;
        assert_eq!(r.get("abc").unwrap().download_count, 1);
    }

    #[test]
    fn each_open_gets_its_own_read_offset() {
        // Two concurrent downloads of one share must not consume each other's
        // bytes, so every open() is a fresh handle rather than a stored one.
        use std::io::{Read, Write};
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"hello").unwrap();
        let source = ShareSource::Path(f.path().to_path_buf());

        let mut first = String::new();
        source.open().unwrap().read_to_string(&mut first).unwrap();
        let mut second = String::new();
        source.open().unwrap().read_to_string(&mut second).unwrap();

        assert_eq!(first, "hello");
        assert_eq!(second, "hello");
    }

    #[test]
    fn list_returns_all() {
        let mut r = ShareRegistry::new();
        r.insert(sample("a"));
        r.insert(sample("b"));
        assert_eq!(r.list().len(), 2);
    }
}
