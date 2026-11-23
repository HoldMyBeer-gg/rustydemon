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

use crate::{
    error::CascError,
    types::{CdnArchiveLocation, Md5Hash},
};

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
    /// are provided or `cdn_path` is empty: both are required to build a
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
        let bytes = self.download_from_any_host(&self.data_path(&hex), None)?;
        self.store(&cached, &bytes)?;
        Ok(bytes)
    }

    /// Fetch `ekey`'s block out of the CDN archive that holds it, asking
    /// for just its byte range.  Cached like a loose blob, under the ekey.
    pub fn fetch_archive_range(
        &self,
        ekey: &Md5Hash,
        at: &CdnArchiveLocation,
    ) -> Result<Vec<u8>, CascError> {
        let cached = self.cache_path(ekey);
        if cached.exists() {
            return fs::read(&cached).map_err(|e| cdn_io_err(&cached, e));
        }
        if at.size == 0 || at.archive.len() < 4 {
            return Err(CascError::Config(format!(
                "CdnFetcher: no usable archive location for {}",
                ekey.to_hex()
            )));
        }

        let last = at.offset + u64::from(at.size) - 1;
        let range = format!("bytes={}-{last}", at.offset);
        let rel = self.data_path(&at.archive.to_lowercase());
        let bytes = self.download_from_any_host(&rel, Some(&range))?;
        // A server that ignores the range sends the whole archive.
        if bytes.len() != at.size as usize {
            return Err(CascError::Config(format!(
                "CdnFetcher: asked {rel} for {} bytes ({range}) and got {}",
                at.size,
                bytes.len()
            )));
        }
        self.store(&cached, &bytes)?;
        Ok(bytes)
    }

    /// Fetch `ekey`'s block from the CDN: out of its archive when `at` says
    /// where that is, as a loose blob otherwise or when the archive read
    /// fails.
    pub fn fetch_block(
        &self,
        ekey: &Md5Hash,
        at: Option<&CdnArchiveLocation>,
    ) -> Result<Vec<u8>, CascError> {
        let Some(at) = at else {
            return self.fetch(ekey);
        };
        self.fetch_archive_range(ekey, at).or_else(|archive_err| {
            self.fetch(ekey).map_err(|loose_err| {
                CascError::Config(format!("{archive_err}; as a loose file: {loose_err}"))
            })
        })
    }

    /// `<cdn_path>/data/<ab>/<cd>/<name>` for a lowercase hex `name`.
    fn data_path(&self, name: &str) -> String {
        format!(
            "{}/data/{}/{}/{}",
            self.cdn_path,
            &name[..2],
            &name[2..4],
            name
        )
    }

    /// Try each host in turn; the first that answers wins.
    fn download_from_any_host(&self, rel: &str, range: Option<&str>) -> Result<Vec<u8>, CascError> {
        let mut last_error: Option<CascError> = None;
        for host in &self.hosts {
            match self.download(&format!("http://{host}/{rel}"), range) {
                Ok(bytes) => return Ok(bytes),
                Err(e) => last_error = Some(e),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            CascError::Config("CdnFetcher: all hosts failed (no hosts configured?)".into())
        }))
    }

    /// Write `bytes` to the cache through a `.partial` file and a rename,
    /// so an interrupted download never leaves a truncated blob behind.
    fn store(&self, cached: &Path, bytes: &[u8]) -> Result<(), CascError> {
        if let Some(parent) = cached.parent() {
            fs::create_dir_all(parent).map_err(|e| cdn_io_err(parent, e))?;
        }
        let tmp = cached.with_extension("partial");
        fs::write(&tmp, bytes).map_err(|e| cdn_io_err(&tmp, e))?;
        fs::rename(&tmp, cached).map_err(|e| cdn_io_err(cached, e))
    }

    /// Fetch a config blob (BuildKey / CDNKey) from CDN and cache it under
    /// `cache_dir/<ab>/<cd>/<key>`: the same layout the game uses for its
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
            match self.download(&url, None) {
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

    fn download(&self, url: &str, range: Option<&str>) -> Result<Vec<u8>, CascError> {
        let mut request = self.agent.get(url);
        if let Some(range) = range {
            request = request.set("Range", range);
        }
        let resp = request
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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// A server that answers one request with `status` and `body`, and
    /// hands back the request head it was sent.
    pub(crate) fn serve_once(
        status: &'static str,
        body: Vec<u8>,
    ) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = listener.local_addr().unwrap().to_string();
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut head = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line.trim().is_empty() {
                    break;
                }
                head.push_str(&line);
            }
            let mut stream = reader.into_inner();
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
            head
        });
        (host, handle)
    }

    fn fetcher(host: String, name: &str) -> CdnFetcher {
        let cache = std::env::temp_dir().join(format!("rd-cdn-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&cache);
        CdnFetcher::new(vec![host], "tpr/wow".into(), cache).unwrap()
    }

    fn in_archive() -> CdnArchiveLocation {
        CdnArchiveLocation {
            archive: "c405cf87ca4ac10b3d7ae2d378ec1fd6".into(),
            offset: 100,
            size: 4,
        }
    }

    #[test]
    fn a_block_inside_an_archive_is_fetched_by_byte_range() {
        let (host, server) = serve_once("206 Partial Content", b"BLTE".to_vec());
        let fetcher = fetcher(host, "range");
        let ekey = Md5Hash([0xFE; 16]);

        let bytes = fetcher.fetch_block(&ekey, Some(&in_archive())).unwrap();
        assert_eq!(bytes, b"BLTE");

        let head = server.join().unwrap().to_lowercase();
        assert!(
            head.contains("get /tpr/wow/data/c4/05/c405cf87ca4ac10b3d7ae2d378ec1fd6 "),
            "{head}"
        );
        assert!(head.contains("range: bytes=100-103"), "{head}");

        // The second read comes from the cache: the server is gone.
        assert_eq!(
            fetcher.fetch_block(&ekey, Some(&in_archive())).unwrap(),
            b"BLTE"
        );
        let _ = fs::remove_dir_all(&fetcher.cache_dir);
    }

    #[test]
    fn a_whole_archive_sent_in_reply_to_a_range_request_is_refused() {
        let (host, server) = serve_once("200 OK", b"the whole archive, not four bytes".to_vec());
        let fetcher = fetcher(host, "whole");
        let ekey = Md5Hash([0xFD; 16]);

        let err = fetcher
            .fetch_archive_range(&ekey, &in_archive())
            .unwrap_err();
        assert!(err.to_string().contains("4 bytes"), "{err}");
        assert!(!fetcher.cache_path(&ekey).exists());
        server.join().unwrap();
        let _ = fs::remove_dir_all(&fetcher.cache_dir);
    }
}
