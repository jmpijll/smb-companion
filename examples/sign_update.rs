//! Writes the signed update feed for one release. Run on the maintainer's PC,
//! never in CI: the private key must stay off GitHub (see src/signing.rs).
//! `packaging/sign-update.ps1` calls this; by hand:
//!
//!   cargo run --example sign_update -- keygen KEYFILE
//!       a new key (PKCS#8) in KEYFILE, never overwriting one; prints the public
//!       key for src/signing.rs
//!   cargo run --example sign_update -- sign KEYFILE VERSION URL INSTALLER OUT
//!       update.json for the installer at INSTALLER, to be downloaded from URL
use ring::{
    rand::SystemRandom,
    signature::{Ed25519KeyPair, KeyPair},
};
use std::{fs, path::Path, process::ExitCode};

#[path = "../src/signing.rs"]
#[allow(dead_code)]
mod signing;

const MAX_SETUP: u64 = 64 << 20;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["keygen", key] => keygen(Path::new(key)),
        ["sign", key, version, url, installer, out] => sign(
            Path::new(key),
            version,
            url,
            Path::new(installer),
            Path::new(out),
        ),
        _ => {
            Err("usage: sign_update keygen KEYFILE | sign KEYFILE VERSION URL INSTALLER OUT".into())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ERROR: {e}");
            ExitCode::FAILURE
        }
    }
}

fn keygen(path: &Path) -> Result<(), String> {
    if path.exists() {
        return Err(format!(
            "{} already exists; refusing to replace a signing key",
            path.display()
        ));
    }
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| "key generation failed")?;
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|_| "key generation failed")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(path, pkcs8.as_ref()).map_err(|e| e.to_string())?;
    let public = pair.public_key().as_ref();
    println!(
        "Key written to {}. Back it up; without it, no update can be shipped.",
        path.display()
    );
    println!(
        "Public key for src/signing.rs: {}",
        public
            .iter()
            .map(|b| format!("0x{b:02x}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

fn sign(key: &Path, version: &str, url: &str, installer: &Path, out: &Path) -> Result<(), String> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3
        || !parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
    {
        return Err(format!("version must be X.Y.Z, got {version}"));
    }
    if !url.starts_with("https://") {
        return Err("the download URL must be https".into());
    }
    let pkcs8 =
        fs::read(key).map_err(|e| format!("cannot read the signing key {}: {e}", key.display()))?;
    let pair = Ed25519KeyPair::from_pkcs8(&pkcs8)
        .map_err(|_| "the signing key is not an Ed25519 PKCS#8 key")?;
    if pair.public_key().as_ref() != signing::PUBLIC_KEY {
        return Err(
            "this key does not match the public key built into the app (src/signing.rs)".into(),
        );
    }
    let setup =
        fs::read(installer).map_err(|e| format!("cannot read {}: {e}", installer.display()))?;
    let size = setup.len() as u64;
    if size == 0 || size > MAX_SETUP {
        return Err("the installer is empty or larger than 64 MiB".into());
    }
    let sha256 = signing::hex(ring::digest::digest(&ring::digest::SHA256, &setup).as_ref());
    let signature = signing::hex(
        pair.sign(&signing::message(version, size, &sha256))
            .as_ref(),
    );
    if !signing::verify(&signing::PUBLIC_KEY, version, size, &sha256, &signature) {
        return Err("the signature does not verify".into());
    }
    let feed = serde_json::json!({
        "schema": 1, "version": version, "url": url, "size": size, "sha256": sha256, "signature": signature,
    });
    let mut text = serde_json::to_string_pretty(&feed).map_err(|e| e.to_string())?;
    text.push('\n');
    fs::write(out, text).map_err(|e| e.to_string())?;
    println!("Signed {version}: {size} bytes, sha256 {sha256}");
    Ok(())
}
