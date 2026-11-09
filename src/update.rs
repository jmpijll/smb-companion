//! Keeps installed copies current without asking anything: a daily look at
//! this repository's latest GitHub release, a checksum-checked download, and
//! a silent install at a moment the player is not queueing, in champion
//! select, in a game or looking at the window. Portable copies only note
//! that a newer version exists.
use crate::{
    client, system,
    worker::{Phase, SharedStatus},
};
use serde_json::Value;
use std::{path::PathBuf, process::Command, thread, time::Duration};

const FIRST_CHECK: Duration = Duration::from_secs(3 * 60);
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
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
pub struct Release {
    pub version: String,
    pub setup_name: String,
    pub setup_url: String,
    pub sums_url: String,
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

fn check_and_install(status: &SharedStatus) -> Result<(), String> {
    let latest = client::github(
        &format!(
            "https://api.github.com/repos/{}/releases/latest",
            client::REPOSITORY
        ),
        1 << 20,
    )?;
    let latest: Value =
        serde_json::from_slice(&latest).map_err(|_| "GitHub sent an unreadable release.")?;
    let Some(release) = newer_release(&latest, env!("CARGO_PKG_VERSION")) else {
        return Ok(());
    };
    if !system::installed() {
        let mut s = status.lock().unwrap();
        if s.update != release.version {
            system::log(&format!("Version {} is available.", release.version));
            s.update = release.version;
        }
        return Ok(());
    }
    let sums = client::github(&release.sums_url, 64 << 10)?;
    let expected = checksum_for(&String::from_utf8_lossy(&sums), &release.setup_name)
        .ok_or("The release has no checksum for its installer.")?;
    let setup = client::github(&release.setup_url, 64 << 20)?;
    if sha256_hex(&setup) != expected {
        return Err("The downloaded installer did not match its checksum; discarded.".into());
    }
    let dir = std::env::temp_dir().join("smb-lol-update");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(&release.setup_name);
    std::fs::write(&path, &setup).map_err(|_| "Could not save the installer.")?;
    system::log(&format!(
        "Version {} downloaded; installing when League is quiet.",
        release.version
    ));
    while !quiet_moment(status) {
        if status.lock().unwrap().stop {
            return Ok(());
        }
        thread::sleep(Duration::from_secs(60));
    }
    install(&path, &release.version)
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
        && !system::window_visible()
}

/// The installer closes this copy (`--quit`), installs over it with the
/// player's earlier choices and starts the new version in the tray.
fn install(setup: &PathBuf, version: &str) -> Result<(), String> {
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

/// The latest release, if it is newer than `current` and carries both the
/// installer and its checksum list.
pub fn newer_release(latest: &Value, current: &str) -> Option<Release> {
    if latest["draft"] == true || latest["prerelease"] == true {
        return None;
    }
    let tag = latest["tag_name"].as_str()?;
    if version(tag)? <= version(current)? {
        return None;
    }
    let setup_name = format!("smb-companion-{tag}-setup.exe");
    let asset = |name: &str| {
        latest["assets"]
            .as_array()?
            .iter()
            .find(|a| a["name"] == name)?["browser_download_url"]
            .as_str()
            .map(str::to_owned)
    };
    Some(Release {
        version: tag.trim_start_matches('v').to_owned(),
        setup_url: asset(&setup_name)?,
        sums_url: asset("SHA256SUMS.txt")?,
        setup_name,
    })
}

/// The hash listed for `name` in a `sha256sum`-style file.
pub fn checksum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.trim().split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name
            && hash.len() == 64
            && hash.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
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

    fn release(tag: &str) -> Value {
        let base = "https://github.com/jmpijll/smb-companion/releases/download";
        json!({
            "tag_name": tag, "draft": false, "prerelease": false,
            "assets": [
                {"name": format!("smb-companion-{tag}-setup.exe"), "browser_download_url": format!("{base}/{tag}/smb-companion-{tag}-setup.exe")},
                {"name": format!("smb-companion-{tag}-portable.zip"), "browser_download_url": format!("{base}/{tag}/smb-companion-{tag}-portable.zip")},
                {"name": "SHA256SUMS.txt", "browser_download_url": format!("{base}/{tag}/SHA256SUMS.txt")}
            ]
        })
    }

    #[test]
    fn only_newer_complete_releases_are_installed() {
        let r = newer_release(&release("v0.3.1"), "0.3.0").unwrap();
        assert_eq!(r.version, "0.3.1");
        assert_eq!(r.setup_name, "smb-companion-v0.3.1-setup.exe");
        assert!(r
            .setup_url
            .ends_with("/v0.3.1/smb-companion-v0.3.1-setup.exe"));
        assert!(newer_release(&release("v0.10.0"), "0.9.9").is_some());
        assert!(newer_release(&release("v0.3.0"), "0.3.0").is_none());
        assert!(newer_release(&release("v0.2.9"), "0.3.0").is_none());
        let mut draft = release("v9.0.0");
        draft["prerelease"] = json!(true);
        assert!(newer_release(&draft, "0.3.0").is_none());
        let mut bare = release("v9.0.0");
        bare["assets"] = json!([]);
        assert!(newer_release(&bare, "0.3.0").is_none());
        assert!(newer_release(&json!({"tag_name": "nightly"}), "0.3.0").is_none());
    }

    #[test]
    fn checksums_are_matched_by_exact_file_name() {
        let h = "a".repeat(64);
        let sums = format!(
            "{h}  smb-companion-v0.3.1-portable.zip\r\n{}  smb-companion-v0.3.1-setup.exe\r\n",
            "B".repeat(64)
        );
        assert_eq!(
            checksum_for(&sums, "smb-companion-v0.3.1-setup.exe"),
            Some("b".repeat(64))
        );
        assert_eq!(checksum_for(&sums, "setup.exe"), None);
        assert_eq!(checksum_for("short  smb.exe", "smb.exe"), None);
    }

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// Talks to GitHub: `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_release_downloads_and_verifies() {
        let latest = client::github(
            &format!(
                "https://api.github.com/repos/{}/releases/latest",
                client::REPOSITORY
            ),
            1 << 20,
        )
        .unwrap();
        let latest: Value = serde_json::from_slice(&latest).unwrap();
        let r = newer_release(&latest, "0.0.1").expect("latest release with installer");
        let sums = client::github(&r.sums_url, 64 << 10).unwrap();
        let expected = checksum_for(&String::from_utf8_lossy(&sums), &r.setup_name).unwrap();
        let setup = client::github(&r.setup_url, 64 << 20).unwrap();
        assert_eq!(sha256_hex(&setup), expected);
        assert!(client::github("https://example.com/x", 10).is_err());
    }
}
