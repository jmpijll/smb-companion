//! Keeps installed copies current without asking anything: a daily look at
//! the signed update feed, a download that must match the size and SHA-256
//! the feed's signature vouches for, and a silent install at a moment the
//! player is not queueing, in champion select, in a game or looking at the
//! window. Portable copies only note that a newer version exists.
//!
//! The feed (`update.json`) is on GitHub Pages, with the same file on the
//! latest GitHub release as a fallback. Only a feed signed with the release
//! key (`signing.rs`) counts, so neither host can push an installer.
use crate::{
    client, signing, system,
    worker::{Phase, SharedStatus},
};
use serde_json::Value;
use std::{path::PathBuf, process::Command, thread, time::Duration};

const FIRST_CHECK: Duration = Duration::from_secs(3 * 60);
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_FEED: usize = 64 << 10;
const MAX_SETUP: u64 = 64 << 20;
/// League phases during which the app must keep running.
const BUSY: [&str; 6] = [
    "Matchmaking",
    "ReadyCheck",
    "ChampSelect",
    "GameStart",
    "InProgress",
    "Reconnect",
];

#[derive(Debug, PartialEq)]
pub struct Update {
    pub version: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// GitHub Pages first; the latest release's copy if Pages is down or moved.
pub fn feeds() -> [String; 2] {
    let (owner, repo) = client::REPOSITORY.split_once('/').unwrap_or_default();
    [
        format!("https://{owner}.github.io/{repo}/update.json"),
        format!(
            "https://github.com/{}/releases/latest/download/update.json",
            client::REPOSITORY
        ),
    ]
}

pub fn start(status: SharedStatus) {
    thread::spawn(move || {
        thread::sleep(FIRST_CHECK);
        loop {
            if status.lock().unwrap().stop {
                return;
            }
            if let Err(e) = check_and_install(&status) {
                system::log(&format!("Update: {e}"));
            }
            thread::sleep(INTERVAL);
        }
    });
}

/// The newest correctly signed update across all feeds, if newer than this copy.
fn find_update() -> Result<Option<Update>, String> {
    let mut found = Vec::new();
    let mut errors = Vec::new();
    for url in feeds() {
        match client::download(&url, MAX_FEED)
            .and_then(|bytes| parse_feed(&bytes, &signing::PUBLIC_KEY))
        {
            Ok(update) => found.push(update),
            Err(e) => errors.push(e),
        }
    }
    if found.is_empty() {
        return Err(format!(
            "no update feed could be read ({})",
            errors.join("; ")
        ));
    }
    Ok(newest(found, env!("CARGO_PKG_VERSION")))
}

fn check_and_install(status: &SharedStatus) -> Result<(), String> {
    let Some(update) = find_update()? else {
        return Ok(());
    };
    if !system::installed() {
        let mut s = status.lock().unwrap();
        if s.update != update.version {
            system::log(&format!("Version {} is available.", update.version));
            s.update = update.version;
        }
        return Ok(());
    }
    let setup = client::download(&update.url, MAX_SETUP as usize)?;
    if setup.len() as u64 != update.size || sha256_hex(&setup) != update.sha256 {
        return Err("The downloaded installer did not match the signed update; discarded.".into());
    }
    let dir = std::env::temp_dir().join("smb-lol-update");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("smb-companion-v{}-setup.exe", update.version));
    std::fs::write(&path, &setup).map_err(|_| "Could not save the installer.")?;
    system::log(&format!(
        "Version {} downloaded; installing when League is quiet.",
        update.version
    ));
    while !quiet_moment(status) {
        if status.lock().unwrap().stop {
            return Ok(());
        }
        thread::sleep(Duration::from_secs(60));
    }
    install(&path, &update.version)
}

/// Not queueing, not in champion select or a game, window not on screen.
fn quiet_moment(status: &SharedStatus) -> bool {
    let (phase, flow) = {
        let s = status.lock().unwrap();
        (s.phase, s.gameflow.clone())
    };
    !BUSY.contains(&flow.as_str())
        && !matches!(phase, Phase::ChampSelect | Phase::InGame)
        && !system::game_running()
        && !system::minecraft_running()
        && !crate::minecraft::service::operation_active()
        && !system::window_visible()
}

/// The installer closes this copy (`--quit`), installs over it with the
/// player's earlier choices and starts the new version in the tray.
fn install(setup: &PathBuf, version: &str) -> Result<(), String> {
    let _pack_pause = crate::minecraft::service::Operation::acquire()?;
    if system::minecraft_running() {
        return Err("App update deferred while Minecraft or Java is running.".into());
    }
    system::log(&format!("Installing version {version}."));
    let log = system::data_dir().join("update-install.log");
    Command::new(setup)
        .args([
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/SP-",
            &format!("/LOG={}", log.display()),
        ])
        .spawn()
        .map_err(|e| format!("Could not start the installer: {e}"))?;
    // Normally this process is closed by the installer well before this.
    thread::sleep(Duration::from_secs(120));
    Err("The installer did not finish; trying again tomorrow.".into())
}

fn version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    let mut next = || parts.next()?.parse::<u64>().ok();
    let v = (next()?, next()?, next()?);
    Some(v)
}

fn plain_version(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
        && version(text).is_some()
}

/// A feed whose signature checks out under `key`; anything else is refused.
pub fn parse_feed(bytes: &[u8], key: &[u8]) -> Result<Update, String> {
    let feed: Value = serde_json::from_slice(bytes).map_err(|_| "the update feed is not JSON")?;
    let text = |field: &str| feed[field].as_str().unwrap_or_default().to_owned();
    let (version, url, sha256) = (text("version"), text("url"), text("sha256"));
    let size = feed["size"].as_u64().unwrap_or_default();
    if feed["schema"] != 1 {
        return Err("the update feed has an unknown format".into());
    }
    if !plain_version(&version) {
        return Err("the update feed has an invalid version".into());
    }
    if !url.starts_with("https://") || size == 0 || size > MAX_SETUP {
        return Err("the update feed has an invalid download".into());
    }
    if sha256.len() != 64
        || !sha256
            .bytes()
            .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err("the update feed has an invalid checksum".into());
    }
    if !signing::verify(key, &version, size, &sha256, &text("signature")) {
        return Err("the update feed is not signed with the release key; ignored".into());
    }
    Ok(Update {
        version,
        url,
        size,
        sha256,
    })
}

/// The highest version among `found`, if it is newer than `current`.
pub fn newest(found: Vec<Update>, current: &str) -> Option<Update> {
    let current = version(current)?;
    found
        .into_iter()
        .filter(|u| version(&u.version).is_some_and(|v| v > current))
        .max_by_key(|u| version(&u.version))
}

fn sha256_hex(bytes: &[u8]) -> String {
    signing::hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use serde_json::json;

    fn pair() -> Ed25519KeyPair {
        Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap()
    }

    fn feed(pair: &Ed25519KeyPair, version: &str, size: u64, sha256: &str) -> Value {
        let signature = signing::hex(pair.sign(&signing::message(version, size, sha256)).as_ref());
        json!({
            "schema": 1, "version": version, "size": size, "sha256": sha256, "signature": signature,
            "url": format!("https://github.com/jmpijll/smb-companion/releases/download/v{version}/smb-companion-v{version}-setup.exe"),
        })
    }

    fn parse(feed: &Value, key: &[u8]) -> Result<Update, String> {
        parse_feed(feed.to_string().as_bytes(), key)
    }

    #[test]
    fn a_correctly_signed_feed_is_accepted() {
        let pair = pair();
        let sha = "a".repeat(64);
        let update = parse(
            &feed(&pair, "0.4.0", 1234, &sha),
            pair.public_key().as_ref(),
        )
        .unwrap();
        assert_eq!(update.version, "0.4.0");
        assert_eq!(update.size, 1234);
        assert_eq!(update.sha256, sha);
        assert!(update
            .url
            .ends_with("/v0.4.0/smb-companion-v0.4.0-setup.exe"));
    }

    #[test]
    fn any_change_to_a_signed_field_or_another_key_is_refused() {
        let pair = pair();
        let key = pair.public_key().as_ref();
        let good = feed(&pair, "0.4.0", 1234, &"a".repeat(64));
        for (field, value) in [
            ("version", json!("9.9.9")),
            ("size", json!(1235)),
            ("sha256", json!("b".repeat(64))),
            ("signature", json!("00".repeat(64))),
            ("signature", json!("not hex")),
            ("schema", json!(2)),
            ("url", json!("http://example.com/setup.exe")),
        ] {
            let mut bad = good.clone();
            bad[field] = value;
            assert!(parse(&bad, key).is_err(), "{field} accepted");
        }
        let other = Ed25519KeyPair::from_seed_unchecked(&[8; 32]).unwrap();
        assert!(parse(&good, other.public_key().as_ref()).is_err());
        // The URL is not signed on purpose: the bytes are, wherever they come from.
        let mut mirrored = good.clone();
        mirrored["url"] = json!("https://example.com/setup.exe");
        assert!(parse(&mirrored, key).is_ok());
        for version in ["0.4", "0.4.0.1", "v0.4.0", "0.4.x", "+1.0.0"] {
            let bad = feed(&pair, version, 1, &"a".repeat(64));
            assert!(parse(&bad, key).is_err(), "{version} accepted");
        }
        assert!(parse(&feed(&pair, "0.4.0", 0, &"a".repeat(64)), key).is_err());
        assert!(parse(&feed(&pair, "0.4.0", 1, &"A".repeat(64)), key).is_err());
        assert!(parse_feed(b"<html>", key).is_err());
    }

    #[test]
    fn the_builtin_key_refuses_the_test_key() {
        let pair = pair();
        let feed = feed(&pair, "0.4.0", 1, &"a".repeat(64));
        assert!(parse(&feed, &signing::PUBLIC_KEY).is_err());
    }

    #[test]
    fn only_a_newer_version_is_installed_and_the_highest_wins() {
        let update = |v: &str| Update {
            version: v.into(),
            url: String::new(),
            size: 1,
            sha256: String::new(),
        };
        assert_eq!(
            newest(
                vec![update("0.3.1"), update("0.10.0"), update("0.4.0")],
                "0.3.0"
            ),
            Some(update("0.10.0"))
        );
        assert_eq!(
            newest(vec![update("0.3.0"), update("0.2.9")], "0.3.0"),
            None
        );
        assert_eq!(newest(vec![], "0.3.0"), None);
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// The live feeds: `cargo test update::tests::live -- --ignored`.
    #[test]
    #[ignore]
    fn live_feeds_are_signed_and_their_installer_verifies() {
        for url in feeds() {
            let bytes = client::download(&url, MAX_FEED).unwrap_or_else(|e| panic!("{url}: {e}"));
            let update =
                parse_feed(&bytes, &signing::PUBLIC_KEY).unwrap_or_else(|e| panic!("{url}: {e}"));
            let setup = client::download(&update.url, MAX_SETUP as usize).unwrap();
            assert_eq!(setup.len() as u64, update.size, "{url}");
            assert_eq!(sha256_hex(&setup), update.sha256, "{url}");
        }
        assert!(client::download("http://example.com/x", 10).is_err());
    }
}
