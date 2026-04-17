//! Optional CDN fetcher for loose metadata blobs.
//!
//! Enabled via the `cdn` crate feature.  Used by [`CascHandler`] to fall
//! back to a Blizzard CDN download when an ekey can't be resolved from any
//! locally-present `.idx` or `.index` file.  This is the path Steam D2R
//! 3.1.2+ needs for its loose metadata blobs (ENCODING, DOWNLOAD, TVFS
//! root) that aren't stored in any local `data.NNN` archive.
//!
//! Fetched blobs are cached on disk at
//! `<cache_dir>/<ab>/<cd>/<full_ekey>` so subsequent opens don't re-download.
//!
//! Keep this module feature-gated so default builds stay network-free and
//! don't drag a TLS stack into the dependency tree.

use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{error::CascError, types::Md5Hash};

/// Builder for a CDN client that mirrors the `host/cdn_path` pair from
/// `.build.info` and knows where to cache downloaded blobs.
pub struct CdnFetcher {
    hosts: Vec<String>,
    cdn_path: String,
    cache_dir: PathBuf,
    agent: ureq::Agent,
}

impl CdnFetcher {
    /// Construct a fetcher from the CDN hosts + path already parsed by
    /// [`CascConfig`](crate::CascConfig).  Returns an error if no hosts
    /// are provided or `cdn_path` is empty — both are required to build a
    /// URL.  The `cache_dir` is created on first use; passing a path
    /// inside the game install is fine and what [`CascHandler`] does by
    /// default.
    pub fn new(
        hosts: Vec<String>,
        cdn_path: String,
        cache_dir: PathBuf,
    ) -> Result<Self, CascError> {
        if hosts.is_empty() {
            return Err(CascError::Config(
                "CdnFetcher: no CDN hosts provided (is this a non-Battle.net install?)".into(),
            ));
        }
        if cdn_path.is_empty() {
            return Err(CascError::Config("CdnFetcher: empty CDN path".into()));
        }

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(60))
            .user_agent(concat!("rustydemon-lib/", env!("CARGO_PKG_VERSION")))
            .build();

        Ok(Self {
            hosts,
            cdn_path,
            cache_dir,
            agent,
        })
    }

    /// Path where a blob for `ekey` would be cached on disk.  Follows the
    /// CDN layout convention: `<cache>/<ab>/<cd>/<full_ekey>`.
    pub fn cache_path(&self, ekey: &Md5Hash) -> PathBuf {
        let hex = ekey.to_hex().to_lowercase();
        self.cache_dir.join(&hex[..2]).join(&hex[2..4]).join(&hex)
    }

    /// Fetch `ekey` as a loose blob from CDN, returning the raw bytes.
    ///
    /// Cache-first: if the blob is already on disk at [`Self::cache_path`]
    /// we return it without touching the network.  On a network fetch the
    /// first host that returns 200 OK wins; all hosts must fail before the
    /// call returns an error.  Downloaded blobs are written to the cache
    /// atomically via a `.partial` temp file + rename.
    pub fn fetch(&self, ekey: &Md5Hash) -> Result<Vec<u8>, CascError> {
        let cached = self.cache_path(ekey);
        if cached.exists() {
            return fs::read(&cached).map_err(|e| cdn_io_err(&cached, e));
        }

        let hex = ekey.to_hex().to_lowercase();
        let rel = format!(
            "{}/data/{}/{}/{}",
            self.cdn_path,
            &hex[..2],
            &hex[2..4],
            hex
        );

        let mut last_error: Option<CascError> = None;
        for host in &self.hosts {
            let url = format!("http://{host}/{rel}");
            match self.download(&url) {
                Ok(bytes) => {
                    // Persist to the cache atomically so a partial download
                    // never leaves a truncated file on disk.
                    if let Some(parent) = cached.parent() {
                        if let Err(e) = fs::create_dir_all(parent) {
                            return Err(cdn_io_err(parent, e));
                        }
                    }
                    let tmp = cached.with_extension("partial");
                    fs::write(&tmp, &bytes).map_err(|e| cdn_io_err(&tmp, e))?;
                    fs::rename(&tmp, &cached).map_err(|e| cdn_io_err(&cached, e))?;
                    return Ok(bytes);
                }
                Err(e) => last_error = Some(e),
            }
        }

        Err(last_error.unwrap_or_else(|| {
            CascError::Config("CdnFetcher: all hosts failed (no hosts configured?)".into())
        }))
    }

    /// Fetch a config blob (BuildKey / CDNKey) from CDN and cache it under
    /// `cache_dir/<ab>/<cd>/<key>` — the same layout the game uses for its
    /// local `Data/config/` tree, so passing the game's config directory
    /// makes the file available for future opens without re-downloading.
    ///
    /// Uses the `config/` CDN prefix instead of the `data/` prefix used by
    /// [`Self::fetch`].
    pub fn fetch_config_key(&self, key: &str, cache_dir: &Path) -> Result<Vec<u8>, CascError> {
        if key.len() < 4 {
            return Err(CascError::Config(format!(
                "CdnFetcher: config key too short: {key}"
            )));
        }
        let ab = &key[..2];
        let cd = &key[2..4];
        let cached = cache_dir.join(ab).join(cd).join(key);
        if cached.exists() {
            return fs::read(&cached).map_err(|e| cdn_io_err(&cached, e));
        }

        let rel = format!("{}/config/{ab}/{cd}/{key}", self.cdn_path);
        let mut last_error: Option<CascError> = None;
        for host in &self.hosts {
            let url = format!("http://{host}/{rel}");
            match self.download(&url) {
                Ok(bytes) => {
                    if let Some(parent) = cached.parent() {
                        if let Err(e) = fs::create_dir_all(parent) {
                            return Err(cdn_io_err(parent, e));
                        }
                    }
                    let tmp = cached.with_extension("partial");
                    fs::write(&tmp, &bytes).map_err(|e| cdn_io_err(&tmp, e))?;
                    fs::rename(&tmp, &cached).map_err(|e| cdn_io_err(&cached, e))?;
                    return Ok(bytes);
                }
                Err(e) => last_error = Some(e),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            CascError::Config("CdnFetcher: all hosts failed (no hosts configured?)".into())
        }))
    }

    fn download(&self, url: &str) -> Result<Vec<u8>, CascError> {
        let resp = self
            .agent
            .get(url)
            .call()
            .map_err(|e| CascError::Config(format!("CdnFetcher: GET {url} failed: {e}")))?;

        let status = resp.status();
        if !(200..300).contains(&status) {
            return Err(CascError::Config(format!(
                "CdnFetcher: GET {url} returned HTTP {status}"
            )));
        }

        // ureq streams the body; read up to a safety cap to avoid
        // unbounded memory use on a hostile CDN.  Real CDN blobs are
        // typically <100 MiB; ENCODING for D2R is ~10 MiB.
        const MAX_BLOB: usize = 512 * 1024 * 1024;
        let mut reader = resp.into_reader().take(MAX_BLOB as u64 + 1);
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .map_err(|e| CascError::Config(format!("CdnFetcher: body read {url}: {e}")))?;
        if bytes.len() > MAX_BLOB {
            return Err(CascError::Config(format!(
                "CdnFetcher: {url} exceeded {MAX_BLOB}-byte safety cap"
            )));
        }
        Ok(bytes)
    }
}

fn cdn_io_err(path: &Path, e: io::Error) -> CascError {
    CascError::Io(io::Error::new(
        e.kind(),
        format!("cdn cache {}: {e}", path.display()),
    ))
}
