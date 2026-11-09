//! Bounded, hash-verified transport. No credentials, private APIs, or implicit redirects.
use super::{
    channel::{self, Channel, Content, Pack, PackRef},
    pack::PackFile,
};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
const DEADLINE: Duration = Duration::from_secs(10 * 60);
pub struct Source {
    location: String,
    cache: PathBuf,
    started: Instant,
}
impl Source {
    pub fn new(location: String, cache: PathBuf) -> Self {
        Self {
            location,
            cache,
            started: Instant::now(),
        }
    }
    fn remaining(&self) -> Result<Duration, String> {
        DEADLINE
            .checked_sub(self.started.elapsed())
            .filter(|d| *d > Duration::from_millis(50))
            .ok_or_else(|| {
                "The pack check took too long. Try again when your connection is ready.".into()
            })
    }
    pub fn channel(&self) -> Result<Channel, String> {
        Channel::parse(&self.fetch(&self.location, 1024 * 1024, false)?)
    }
    /// The verified archive on disk, downloaded first when needed.
    pub fn archive_path(&self, r: &PackRef) -> Result<PathBuf, String> {
        self.archive(r)?;
        Ok(self.cache.join(format!("{}.mrpack", r.sha256)))
    }
    pub fn archive(&self, r: &PackRef) -> Result<Pack, String> {
        self.remaining()?;
        let path = self.cache.join(format!("{}.mrpack", r.sha256));
        if let Ok(bytes) = bounded_file(&path, r.size) {
            if let Ok(pack) = channel::read_pack(&bytes, r) {
                return Ok(pack);
            }
        }
        let location = if self.location.contains("://") {
            let base = url::Url::parse(&self.location).map_err(|_| "Invalid pack channel URL")?;
            base.join(&r.url)
                .map_err(|_| "Invalid pack archive URL")?
                .to_string()
        } else {
            let root = Path::new(&self.location)
                .parent()
                .ok_or("Local channel has no parent folder")?
                .canonicalize()
                .map_err(|_| "Local channel folder unavailable")?;
            let p = root
                .join(&r.url)
                .canonicalize()
                .map_err(|_| "The local pack archive is missing")?;
            if !p.starts_with(root) {
                return Err("The pack archive is outside the local channel".into());
            }
            p.to_string_lossy().into_owned()
        };
        let bytes = self.fetch(&location, r.size, false)?;
        let pack = channel::read_pack(&bytes, r)?;
        store(&path, &bytes)?;
        Ok(pack)
    }
    pub fn materialize(
        &self,
        pack: &Pack,
        baseline: &BTreeMap<String, String>,
        profile: &Path,
    ) -> Result<Vec<PackFile>, String> {
        let mut result = Vec::new();
        for (path, entry) in pack {
            self.remaining()?;
            if entry.optional && !baseline.contains_key(path) && !profile.join(path).exists() {
                continue;
            }
            let bytes = match &entry.content {
                Content::Inline(b) => b.clone(),
                Content::Download { urls, size } => {
                    let cached = self.cache.join(&entry.sha512);
                    if let Ok(b) = bounded_file(&cached, *size) {
                        if b.len() as u64 == *size && channel::hash512(&b) == entry.sha512 {
                            result.push(PackFile {
                                path: path.clone(),
                                bytes: b,
                                mutable: entry.mutable,
                            });
                            continue;
                        }
                    }
                    let mut downloaded = None;
                    let mut failure = String::new();
                    for url in urls {
                        match self.fetch(url, *size, true) {
                            Ok(b)
                                if b.len() as u64 == *size
                                    && channel::hash512(&b) == entry.sha512 =>
                            {
                                downloaded = Some(b);
                                break;
                            }
                            Ok(_) => failure = "A mod download did not match its checksum".into(),
                            Err(e) => failure = e,
                        }
                    }
                    let b =
                        downloaded.ok_or_else(|| format!("Could not prepare {path}: {failure}"))?;
                    store(&cached, &b)?;
                    b
                }
            };
            if channel::hash512(&bytes) != entry.sha512 {
                return Err("A staged pack file failed verification".into());
            }
            result.push(PackFile {
                path: path.clone(),
                bytes,
                mutable: entry.mutable,
            });
        }
        Ok(result)
    }
    fn fetch(&self, location: &str, max: u64, mod_file: bool) -> Result<Vec<u8>, String> {
        if !location.contains("://") {
            if mod_file {
                return Err("Mod downloads require HTTPS".into());
            }
            self.remaining()?;
            return bounded_file(Path::new(location), max);
        }
        let mut current = location.to_owned();
        for redirects in 0..=5 {
            validate_url(&current, mod_file)?;
            let timeout = self.remaining()?.min(Duration::from_secs(35));
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(timeout.min(Duration::from_secs(8)))
                .timeout(timeout)
                .redirects(0)
                .build();
            let response = match agent
                .get(&current)
                .set(
                    "User-Agent",
                    "SMB-Companion/0.3 minecraft (github.com/jmpijll/smb-companion)",
                )
                .call()
            {
                Ok(r) => r,
                Err(ureq::Error::Status(_, r)) => r,
                Err(_) => return Err(
                    "The SMB client channel or download is unavailable. No pack changes were made."
                        .into(),
                ),
            };
            if matches!(response.status(), 301 | 302 | 303 | 307 | 308) {
                if redirects == 5 {
                    return Err("Too many download redirects".into());
                }
                let next = response
                    .header("Location")
                    .ok_or("Download redirect has no destination")?;
                current = url::Url::parse(&current)
                    .and_then(|u| u.join(next))
                    .map_err(|_| "Invalid redirect")?
                    .to_string();
                continue;
            }
            if response.status() != 200 {
                return Err(format!("The client download returned HTTP {}. The public SMB pack feed may not be published yet.",response.status()));
            }
            if response
                .header("Content-Length")
                .and_then(|s| s.parse::<u64>().ok())
                .is_some_and(|n| n > max)
            {
                return Err("Download exceeds its size limit".into());
            }
            let mut reader = response.into_reader();
            let mut data = Vec::new();
            let mut buffer = [0u8; 65536];
            loop {
                self.remaining()?;
                let n = reader
                    .read(&mut buffer)
                    .map_err(|_| "Download interrupted")?;
                if n == 0 {
                    break;
                }
                if data.len() as u64 + n as u64 > max {
                    return Err("Download exceeds its size limit".into());
                }
                data.extend_from_slice(&buffer[..n]);
            }
            return Ok(data);
        }
        Err("Download redirects exhausted".into())
    }
}
fn validate_url(location: &str, mod_file: bool) -> Result<(), String> {
    #[cfg(test)]
    if url::Url::parse(location)
        .is_ok_and(|u| u.scheme() == "http" && u.host_str() == Some("127.0.0.1"))
    {
        return Ok(());
    }
    channel::mod_url(location)?;
    let u = url::Url::parse(location).map_err(|_| "Invalid URL")?;
    if u.port_or_known_default() != Some(443) || u.fragment().is_some() {
        return Err("Unsupported download URL".into());
    }
    if !mod_file {
        let allowed = (u.host_str() == Some("raw.githubusercontent.com")
            && u.path().starts_with("/jmpijll/smb-minecraft-client/"))
            || (u.host_str() == Some("github.com")
                && u.path().starts_with("/jmpijll/smb-minecraft-client/"))
            || matches!(
                u.host_str(),
                Some("release-assets.githubusercontent.com" | "objects.githubusercontent.com")
            );
        if !allowed {
            return Err("Pack metadata must come from the SMB client release repository".into());
        }
    }
    Ok(())
}
fn bounded_file(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    let mut f = fs::File::open(path).map_err(|_| "A local channel/cache file is unavailable")?;
    if f.metadata().map_err(|e| e.to_string())?.len() > max {
        return Err("A local file exceeds its size limit".into());
    }
    let mut b = Vec::new();
    f.by_ref()
        .take(max + 1)
        .read_to_end(&mut b)
        .map_err(|e| e.to_string())?;
    if b.len() as u64 > max {
        return Err("A local file exceeds its size limit".into());
    }
    Ok(b)
}
fn store(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let parent = path.parent().ok_or("Cache folder missing")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temp = path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| e.to_string())?;
    let result = (|| {
        f.write_all(bytes).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
        fs::rename(&temp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    fn server(response: &str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let response = response.to_owned();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            stream.write_all(response.as_bytes()).unwrap();
        });
        format!("http://{address}/file")
    }
    #[test]
    fn local_http_reads_are_bounded_and_redirects_revalidated() {
        let source = Source::new("unused".into(), std::env::temp_dir());
        let url = server("HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc");
        assert_eq!(source.fetch(&url, 3, false).unwrap(), b"abc");
        let url = server("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\nabcdef");
        assert!(source.fetch(&url, 3, false).is_err());
        let url=server("HTTP/1.1 302 Found\r\nLocation: http://example.com/evil\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        assert!(source.fetch(&url, 1024, false).is_err());
    }
    #[test]
    fn rejects_untrusted_metadata_and_downgrade_redirects() {
        assert!(validate_url(
            "http://raw.githubusercontent.com/jmpijll/smb-minecraft-client/main/stable.json",
            false
        )
        .is_err());
        assert!(validate_url(
            "https://raw.githubusercontent.com/other/project/main/stable.json",
            false
        )
        .is_err());
        assert!(validate_url(
            "https://github.com/jmpijll/smb-minecraft-client/releases/download/v1/a",
            false
        )
        .is_ok());
        assert!(validate_url("https://cdn.modrinth.com/data/a.jar", true).is_ok());
        assert!(validate_url("https://cdn.modrinth.com:444/data/a.jar", true).is_err());
        assert!(validate_url("https://evil.test/mod.jar", true).is_err());
    }
}
