//! Update signatures. Every update feed (`update.json`) carries an Ed25519
//! signature over the new version, the installer's size and its SHA-256. The
//! private key never touches GitHub: `packaging/sign-update.ps1` signs each
//! release on the maintainer's PC. So whoever controls the GitHub account,
//! the release assets or GitHub Pages still cannot push an installer to
//! members' PCs; only a holder of the key can.
//!
//! Shared with `examples/sign_update.rs`, which writes the feed.

/// The public half of the release signing key.
pub const PUBLIC_KEY: [u8; 32] = [
    0xb0, 0xb5, 0x2f, 0x0b, 0xed, 0x23, 0xfe, 0x6d, 0xc7, 0x3a, 0x8f, 0xd1, 0x59, 0xbd, 0xab, 0x30,
    0x92, 0x5a, 0x18, 0xa5, 0xff, 0xc0, 0x63, 0x0e, 0x62, 0xc7, 0xa0, 0xb0, 0xcc, 0x02, 0x32, 0x4e,
];

/// The exact bytes that are signed.
pub fn message(version: &str, size: u64, sha256: &str) -> Vec<u8> {
    format!("smb-companion-update:1\n{version}\n{size}\n{sha256}\n").into_bytes()
}

pub fn verify(key: &[u8], version: &str, size: u64, sha256: &str, signature: &str) -> bool {
    let Some(signature) = unhex(signature) else {
        return false;
    };
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key)
        .verify(&message(version, size, sha256), &signature)
        .is_ok()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}
