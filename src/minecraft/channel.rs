//! Client-only release channel and verified Modrinth archives.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
};

pub const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;
pub const MAX_FILE: u64 = 256 * 1024 * 1024;
pub const MAX_TOTAL: u64 = 768 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PackRef {
    pub version: String,
    pub min_version: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub minecraft: String,
    pub loader: String,
    pub loader_version: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Channel {
    pub schema: u32,
    pub pack: PackRef,
    #[serde(default)]
    pub history: Vec<PackRef>,
}
#[derive(Clone, Debug)]
pub enum Content {
    Download { urls: Vec<String>, size: u64 },
    Inline(Vec<u8>),
}
#[derive(Clone, Debug)]
pub struct Entry {
    pub sha512: String,
    pub optional: bool,
    pub mutable: bool,
    pub content: Content,
}
pub type Pack = BTreeMap<String, Entry>;

pub fn version(value: &str) -> Result<[u32; 3], String> {
    let pieces: Vec<_> = value.split('.').collect();
    if pieces.len() != 3
        || pieces.iter().any(|s| {
            s.is_empty()
                || s.len() > 9
                || !s.bytes().all(|b| b.is_ascii_digit())
                || (s.len() > 1 && s.starts_with('0'))
        })
    {
        return Err("The pack version must be a stable release, such as 0.13.0.".into());
    }
    Ok([
        pieces[0].parse().map_err(|_| "Invalid pack version")?,
        pieces[1].parse().map_err(|_| "Invalid pack version")?,
        pieces[2].parse().map_err(|_| "Invalid pack version")?,
    ])
}
impl Channel {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > 1024 * 1024 {
            return Err("The pack channel is unexpectedly large.".into());
        }
        let channel: Self =
            serde_json::from_slice(bytes).map_err(|_| "The SMB pack channel could not be read.")?;
        if channel.schema != 1 || channel.history.len() > 1000 {
            return Err("This pack channel needs a newer companion.".into());
        }
        let mut seen = BTreeSet::new();
        for pack in std::iter::once(&channel.pack).chain(channel.history.iter()) {
            if version(&pack.min_version)? > version(&pack.version)?
                || !seen.insert(pack.version.clone())
            {
                return Err("The pack channel contains conflicting versions.".into());
            }
            if !hex(&pack.sha256, 64)
                || pack.size == 0
                || pack.size > MAX_ARCHIVE
                || pack.loader != "fabric"
                || !runtime_id(&pack.minecraft)
                || !runtime_id(&pack.loader_version)
                || pack.url.is_empty()
                || pack.url.len() > 4096
            {
                return Err("The pack channel has invalid archive metadata.".into());
            }
        }
        Ok(channel)
    }
    pub fn find(&self, wanted: &str) -> Option<&PackRef> {
        std::iter::once(&self.pack)
            .chain(self.history.iter())
            .find(|p| p.version == wanted)
    }
}
pub fn read_pack(bytes: &[u8], expected: &PackRef) -> Result<Pack, String> {
    if bytes.len() as u64 != expected.size
        || bytes.len() as u64 > MAX_ARCHIVE
        || hash256(bytes) != expected.sha256
    {
        return Err("The downloaded pack did not match its checksum; nothing changed.".into());
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|_| "The downloaded pack is not a valid archive.")?;
    if archive.len() > 10000 {
        return Err("The pack has too many files.".into());
    }
    let mut names = BTreeSet::new();
    for i in 0..archive.len() {
        let f = archive
            .by_index(i)
            .map_err(|_| "Could not inspect the pack archive.")?;
        if !names.insert(f.name().to_owned())
            || f.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            return Err("The pack contains duplicate files or links.".into());
        }
    }
    let index = {
        let mut file = archive
            .by_name("modrinth.index.json")
            .map_err(|_| "The pack has no Modrinth manifest.")?;
        limited(&mut file, 4 * 1024 * 1024)?
    };
    let manifest: serde_json::Value =
        serde_json::from_slice(&index).map_err(|_| "The Modrinth pack manifest is unreadable.")?;
    if manifest["formatVersion"] != 1
        || manifest["game"] != "minecraft"
        || manifest["name"] != "SMB"
        || manifest["versionId"] != expected.version
        || manifest["dependencies"]["minecraft"] != expected.minecraft
        || manifest["dependencies"]["fabric-loader"] != expected.loader_version
        || expected.loader != "fabric"
        || manifest["dependencies"]
            .as_object()
            .is_none_or(|d| d.keys().any(|k| k != "minecraft" && k != "fabric-loader"))
    {
        return Err(
            "The pack version or Minecraft/Fabric runtime does not match its release channel."
                .into(),
        );
    }
    let list = manifest["files"]
        .as_array()
        .filter(|v| v.len() <= 10000)
        .ok_or("The pack file list is invalid.")?;
    let mut pack = BTreeMap::new();
    let mut paths = BTreeMap::new();
    let mut total = 0u64;
    for file in list {
        let environment = file["env"]["client"].as_str().unwrap_or("required");
        if environment == "unsupported" {
            continue;
        }
        if environment != "required" && environment != "optional" {
            return Err("The pack has an unknown client file type.".into());
        }
        let path = file["path"]
            .as_str()
            .ok_or("The pack contains an unnamed file.")?;
        target(path)?;
        check_case(&mut paths, path)?;
        if pack.contains_key(path) {
            return Err("The pack contains duplicate target files.".into());
        }
        let sha = file["hashes"]["sha512"]
            .as_str()
            .filter(|s| hex(s, 128))
            .ok_or("A pack file has no valid checksum.")?;
        let size = file["fileSize"]
            .as_u64()
            .filter(|s| *s <= MAX_FILE)
            .ok_or("A pack file is too large or has no size.")?;
        total = total
            .checked_add(size)
            .filter(|s| *s <= MAX_TOTAL)
            .ok_or("The pack would use too much space.")?;
        let urls: Vec<String> = file["downloads"]
            .as_array()
            .filter(|v| !v.is_empty() && v.len() <= 8)
            .ok_or("A pack file has no download source.")?
            .iter()
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| String::from("Invalid file URL"))
                    .and_then(|s| {
                        mod_url(s)?;
                        Ok(s.to_owned())
                    })
            })
            .collect::<Result<_, _>>()?;
        pack.insert(
            path.to_owned(),
            Entry {
                sha512: sha.to_owned(),
                optional: environment == "optional",
                mutable: mutable_path(path),
                content: Content::Download { urls, size },
            },
        );
    }
    // The client-specific layer deliberately replaces the same exact path.
    for prefix in ["overrides/", "client-overrides/"] {
        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|_| "Could not read a pack override.")?;
            if file.is_dir() {
                continue;
            }
            let Some(path) = file.name().strip_prefix(prefix).map(str::to_owned) else {
                continue;
            };
            target(&path)?;
            check_case(&mut paths, &path)?;
            let data = limited(&mut file, MAX_FILE)?;
            total = total
                .checked_add(data.len() as u64)
                .filter(|s| *s <= MAX_TOTAL)
                .ok_or("The expanded pack is too large.")?;
            pack.insert(
                path.clone(),
                Entry {
                    sha512: hash512(&data),
                    optional: false,
                    mutable: mutable_path(&path),
                    content: Content::Inline(data),
                },
            );
        }
    }
    if pack.is_empty() {
        return Err("The SMB pack contains no client files.".into());
    }
    Ok(pack)
}
fn limited(reader: &mut impl Read, max: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read a pack file.")?;
    if bytes.len() as u64 > max {
        return Err("A pack file exceeds the size limit.".into());
    }
    Ok(bytes)
}
fn hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn runtime_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
}
/// Files the game or the player rewrites: the pack's version is a default.
/// Untouched, it follows new releases; once changed, the player's copy stays.
pub fn mutable_path(path: &str) -> bool {
    path.starts_with("config/")
        || matches!(path, "options.txt" | "servers.dat")
        // Iris keeps each shader's settings next to it as `<shader>.txt`.
        || (path.starts_with("shaderpacks/") && path.ends_with(".txt"))
}
fn check_case(paths: &mut BTreeMap<String, String>, path: &str) -> Result<(), String> {
    let lower = path.to_ascii_lowercase();
    if paths.get(&lower).is_some_and(|s| s != path) {
        return Err("The pack contains conflicting Windows filenames.".into());
    }
    paths.insert(lower, path.into());
    Ok(())
}
fn target(path: &str) -> Result<(), String> {
    super::pack::validate_path(path)
}
pub fn mod_url(value: &str) -> Result<(), String> {
    let u = url::Url::parse(value).map_err(|_| "Invalid mod download URL")?;
    if u.scheme() != "https"
        || !u.username().is_empty()
        || u.password().is_some()
        || !matches!(
            u.host_str(),
            Some(
                "cdn.modrinth.com"
                    | "github.com"
                    | "raw.githubusercontent.com"
                    | "gitlab.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
    {
        return Err("A mod download uses an unsupported source.".into());
    }
    Ok(())
}
pub fn hash512(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA512, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn hash256(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Cursor, Write};
    fn archive(extra: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("modrinth.index.json", options).unwrap();
        zip.write_all(serde_json::to_string(&json!({
            "formatVersion":1,"game":"minecraft","name":"SMB","versionId":"0.13.0",
            "dependencies":{"minecraft":"26.3","fabric-loader":"0.20.1"},
            "files":[
                {"path":"mods/client.jar","hashes":{"sha512":hash512(b"client")},"downloads":["https://cdn.modrinth.com/client.jar"],"fileSize":6},
                {"path":"mods/server.jar","hashes":{"sha512":hash512(b"server")},"downloads":["https://cdn.modrinth.com/server.jar"],"fileSize":6,"env":{"client":"unsupported"}}
            ]
        })).unwrap().as_bytes()).unwrap();
        for (name, bytes) in extra {
            zip.start_file(*name, options).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }
    fn reference(bytes: &[u8]) -> PackRef {
        PackRef {
            version: "0.13.0".into(),
            min_version: "0.11.0".into(),
            url: "packs/smb-0.13.0.mrpack".into(),
            sha256: hash256(bytes),
            size: bytes.len() as u64,
            minecraft: "26.3".into(),
            loader: "fabric".into(),
            loader_version: "0.20.1".into(),
        }
    }
    #[test]
    fn accepts_stable_client_channel_and_finds_historical_release() {
        let bytes = archive(&[]);
        let current = reference(&bytes);
        let mut old = current.clone();
        old.version = "0.5.0".into();
        old.min_version = "0.5.0".into();
        let parsed = Channel::parse(
            &serde_json::to_vec(&json!({"schema":1,"pack":current,"history":[old]})).unwrap(),
        )
        .unwrap();
        assert_eq!(parsed.find("0.5.0").unwrap().version, "0.5.0");
        assert_eq!(version("0.13.0").unwrap(), [0, 13, 0]);
    }
    #[test]
    fn applies_client_override_precedence_and_excludes_server_files() {
        let bytes = archive(&[
            ("overrides/config/test.json", b"default"),
            ("client-overrides/config/test.json", b"client default"),
            ("server-overrides/mods/server2.jar", b"server"),
        ]);
        let pack = read_pack(&bytes, &reference(&bytes)).unwrap();
        assert!(!pack.contains_key("mods/server.jar"));
        assert!(!pack.contains_key("mods/server2.jar"));
        assert_eq!(pack["config/test.json"].sha512, hash512(b"client default"));
        assert!(pack["config/test.json"].mutable);
        assert_eq!(pack.len(), 2);
    }
    #[test]
    fn rejects_dev_versions_duplicate_history_and_invalid_minimum() {
        for bad in ["0.14.0-dev", "v0.13.0", "01.2.3", "1.2", "1.2.3.4"] {
            assert!(version(bad).is_err());
        }
        let bytes = archive(&[]);
        let p = reference(&bytes);
        assert!(Channel::parse(
            &serde_json::to_vec(&json!({"schema":1,"pack":p,"history":[p]})).unwrap()
        )
        .is_err());
        let mut p = reference(&bytes);
        p.min_version = "0.14.0".into();
        assert!(
            Channel::parse(&serde_json::to_vec(&json!({"schema":1,"pack":p})).unwrap()).is_err()
        );
    }
    #[test]
    fn archive_integrity_runtime_and_unsafe_overrides_fail_closed() {
        let bytes = archive(&[]);
        let mut r = reference(&bytes);
        r.sha256 = "0".repeat(64);
        assert!(read_pack(&bytes, &r).is_err());
        let mut r = reference(&bytes);
        r.loader_version = "0.20.2".into();
        assert!(read_pack(&bytes, &r).is_err());
        for path in [
            "overrides/../secret",
            "overrides/C:/secret",
            "overrides/saves/world.dat",
            "overrides/mods/CON.jar",
        ] {
            let b = archive(&[(path, b"bad")]);
            assert!(read_pack(&b, &reference(&b)).is_err(), "{path}");
        }
        let b = archive(&[
            ("overrides/config/A.json", b"a"),
            ("client-overrides/config/a.json", b"b"),
        ]);
        assert!(read_pack(&b, &reference(&b)).is_err());
    }
}
