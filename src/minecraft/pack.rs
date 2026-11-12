//! Locked, recoverable synchronization of explicitly owned profile files.
//!
//! Files are copied into a private transaction directory before publishing the
//! journal. Replacements use rename, so Modrinth's hardlinked cache is never
//! written through. Removing the journal commits the new state; until then
//! recovery restores the prior state from retained backup copies.
use fs2::FileExt;
use ring::digest::{Context, SHA512};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const CONTROL: &str = ".smb-companion";
const MAX_FILES: usize = 10_000;
const MAX_METADATA: u64 = 8 * 1024 * 1024;
const MAX_FILE: u64 = 256 * 1024 * 1024;
const MAX_TOTAL: usize = 768 * 1024 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);

pub struct ProfileLock {
    profile: PathBuf,
    _file: File,
}

impl ProfileLock {
    /// Acquire before ownership reads, adoption, staging, or recovery. The
    /// retained file handle holds the OS lock for the entire operation.
    pub fn acquire(profile: &Path) -> Result<Self, String> {
        check_ancestors(profile)?;
        let profile = profile.canonicalize().map_err(io_error)?;
        if !profile.is_dir() {
            return Err("The selected Minecraft profile is not a directory".into());
        }
        let control = control_dir(&profile)?;
        if !control.exists() {
            fs::create_dir(&control).map_err(io_error)?;
        }
        let lock_path = control.join("operation.lock");
        check_regular_or_absent(&lock_path)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(io_error)?;
        let start = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => break,
                Err(e) if e.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                    if start.elapsed() >= LOCK_TIMEOUT {
                        return Err(
                            "Another SMB pack operation is using this profile; try again shortly"
                                .into(),
                        );
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                Err(e) => return Err(io_error(e)),
            }
        }
        control_dir(&profile)?;
        check_regular_or_absent(&lock_path)?;
        Ok(Self {
            profile,
            _file: file,
        })
    }
    pub fn profile(&self) -> &Path {
        &self.profile
    }
}

pub struct PackFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub mutable: bool,
}
#[derive(Debug, Default)]
pub struct InstallReport {
    pub written: usize,
    pub removed: usize,
    pub preserved: usize,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Owned {
    hash: String,
    mutable: bool,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct State {
    schema: u32,
    version: String,
    files: BTreeMap<String, Owned>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Change {
    path: String,
    #[serde(deserialize_with = "required_option")]
    old_hash: Option<String>,
    #[serde(deserialize_with = "required_option")]
    new_hash: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema: u32,
    #[serde(deserialize_with = "required_option")]
    old_state: Option<State>,
    entries: Vec<Change>,
}

// Null is meaningful; an absent field is a truncated/corrupt journal. Serde's
// default Option handling otherwise silently treats either case as None.
fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn io_error(e: impl std::fmt::Display) -> String {
    format!("Minecraft profile filesystem error: {e}")
}

/// Lexical archive/ownership validation. On-disk link and case checks are
/// additionally performed by the transaction before opening any target.
pub fn validate_path(path: &str) -> Result<(), String> {
    let fail = || format!("Unsafe or unsupported Minecraft pack path: {path}");
    if path.is_empty() || path.len() > 240 || !path.is_ascii() || path.contains('\\') {
        return Err(fail());
    }
    let parts: Vec<_> = path.split('/').collect();
    for part in &parts {
        if part.is_empty()
            || *part == "."
            || *part == ".."
            || part.ends_with(['.', ' '])
            || part
                .chars()
                .any(|c| c.is_control() || "<>:\"|?*".contains(c))
        {
            return Err(fail());
        }
        let stem = part.split('.').next().unwrap().to_ascii_uppercase();
        if matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || ["COM", "LPT"].iter().any(|p| {
            stem.strip_prefix(p)
                .is_some_and(|n| matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"))
        }) {
            return Err(fail());
        }
    }
    if (parts.len() >= 2
        && matches!(
            parts[0],
            "mods" | "config" | "resourcepacks" | "shaderpacks"
        ))
        || (parts.len() == 1 && matches!(path, "options.txt" | "servers.dat"))
    {
        Ok(())
    } else {
        Err(fail())
    }
}

fn mutable_default(path: &str) -> bool {
    super::channel::mutable_path(path)
}

/// `<path>.disabled` exists: the player switched this file off in Modrinth.
fn disabled_copy(profile: &Path, path: &str) -> bool {
    target(profile, &format!("{path}.disabled"))
        .and_then(|copy| metadata(&copy))
        .is_ok_and(|m| m.is_some_and(|m| m.is_file()))
}

fn is_link(meta: &Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}
fn metadata(path: &Path) -> Result<Option<Metadata>, String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if is_link(&meta) => Err(format!(
            "Refusing a symlink or junction: {}",
            path.display()
        )),
        Ok(meta) => Ok(Some(meta)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_error(e)),
    }
}
fn check_ancestors(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        metadata(ancestor)?;
    }
    Ok(())
}
fn check_regular_or_absent(path: &Path) -> Result<(), String> {
    if metadata(path)?.is_some_and(|m| !m.is_file()) {
        return Err(format!("Expected a regular file: {}", path.display()));
    }
    Ok(())
}
fn exact_case(parent: &Path, component: &str) -> Result<(), String> {
    if !parent.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(parent).map_err(io_error)? {
        let name = entry.map_err(io_error)?.file_name();
        let name = name.to_string_lossy();
        if name.eq_ignore_ascii_case(component) && name != component {
            return Err(format!("Case-colliding Minecraft pack path: {component}"));
        }
    }
    Ok(())
}
fn control_dir(profile: &Path) -> Result<PathBuf, String> {
    if !metadata(profile)?.is_some_and(|m| m.is_dir()) {
        return Err("The selected Minecraft profile directory is missing".into());
    }
    exact_case(profile, CONTROL)?;
    let control = profile.join(CONTROL);
    if metadata(&control)?.is_some_and(|m| !m.is_dir()) {
        return Err("The Minecraft companion state path is not a directory".into());
    }
    Ok(control)
}
fn target(profile: &Path, path: &str) -> Result<PathBuf, String> {
    validate_path(path)?;
    metadata(profile)?;
    let mut current = profile.to_owned();
    let parts: Vec<_> = path.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        exact_case(&current, part)?;
        current.push(part);
        if let Some(meta) = metadata(&current)? {
            let last = i + 1 == parts.len();
            if (last && !meta.is_file()) || (!last && !meta.is_dir()) {
                return Err(format!(
                    "Unexpected file or directory at {}",
                    current.display()
                ));
            }
        }
    }
    Ok(current)
}
fn validate_paths<'a>(paths: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
    let mut cases = BTreeMap::new();
    let mut files = BTreeSet::new();
    for path in paths {
        validate_path(path)?;
        if !files.insert(path.to_owned()) {
            continue;
        }
        let mut prefix = String::new();
        for part in path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if let Some(old) = cases.insert(prefix.to_ascii_lowercase(), prefix.clone()) {
                if old != prefix {
                    return Err(format!("Case-colliding Minecraft paths: {old}, {prefix}"));
                }
            }
        }
    }
    for path in &files {
        let mut parent = Path::new(path).parent();
        while let Some(p) = parent {
            if files.contains(&p.to_string_lossy().to_string()) {
                return Err(format!("Pack file is also a directory: {}", p.display()));
            }
            parent = p.parent();
        }
    }
    if files.len() > MAX_FILES {
        return Err("Minecraft pack has too many files".into());
    }
    Ok(())
}
fn valid_hash(hash: &str) -> bool {
    hash.len() == 128
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn version_tuple(version: &str) -> Result<(u64, u64, u64), String> {
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|p| {
            p.is_empty()
                || !p.bytes().all(|b| b.is_ascii_digit())
                || (p.len() > 1 && p.starts_with('0'))
        })
    {
        return Err("Pack version must be a stable three-component numeric version".into());
    }
    Ok((
        parts[0].parse().map_err(io_error)?,
        parts[1].parse().map_err(io_error)?,
        parts[2].parse().map_err(io_error)?,
    ))
}
fn validate_state(state: &State) -> Result<(), String> {
    if state.schema != 1 {
        return Err("Unsupported Minecraft ownership state".into());
    }
    version_tuple(&state.version)?;
    validate_paths(state.files.keys().map(String::as_str))?;
    for (path, owned) in &state.files {
        if !valid_hash(&owned.hash) || (owned.mutable && !mutable_default(path)) {
            return Err("Invalid Minecraft ownership state".into());
        }
    }
    Ok(())
}
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    check_regular_or_absent(path)?;
    let Some(meta) = metadata(path)? else {
        return Ok(None);
    };
    if meta.len() > MAX_METADATA {
        return Err("Minecraft transaction metadata is too large".into());
    }
    let bytes = fs::read(path).map_err(io_error)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("Corrupt Minecraft transaction metadata: {e}"))
}
fn read_state(lock: &ProfileLock) -> Result<Option<State>, String> {
    if has_pending(lock.profile())? {
        return Err("An interrupted Minecraft pack update needs recovery".into());
    }
    let state = read_json::<State>(&control_dir(lock.profile())?.join("state.json"))?;
    if let Some(state) = &state {
        validate_state(state)?;
    }
    Ok(state)
}
pub fn load_version(lock: &ProfileLock) -> Result<Option<String>, String> {
    Ok(read_state(lock)?.map(|s| s.version))
}
pub fn owned_files(lock: &ProfileLock) -> Result<BTreeMap<String, String>, String> {
    Ok(read_state(lock)?
        .map(|s| s.files.into_iter().map(|(p, o)| (p, o.hash)).collect())
        .unwrap_or_default())
}
/// Read-only disabled-hook gate: never permit launch with a pending journal.
pub fn has_pending(profile: &Path) -> Result<bool, String> {
    check_ancestors(profile)?;
    let journal = control_dir(profile)?.join("journal.json");
    check_regular_or_absent(&journal)?;
    Ok(metadata(&journal)?.is_some())
}
fn hash_bytes(bytes: &[u8]) -> String {
    ring::digest::digest(&SHA512, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn hash_file(path: &Path) -> Result<Option<String>, String> {
    check_regular_or_absent(path)?;
    let Some(meta) = metadata(path)? else {
        return Ok(None);
    };
    if meta.len() > MAX_FILE {
        return Err(format!(
            "Minecraft pack file exceeds size limit: {}",
            path.display()
        ));
    }
    let mut file = File::open(path).map_err(io_error)?;
    let mut context = Context::new(&SHA512);
    let mut buf = [0; 64 * 1024];
    let mut total = 0;
    loop {
        let n = file.read(&mut buf).map_err(io_error)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > MAX_FILE {
            return Err("Minecraft pack file grew beyond size limit".into());
        }
        context.update(&buf[..n]);
    }
    Ok(Some(
        context
            .finish()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    ))
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.sync_all().map_err(io_error)
}
fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(io_error)?;
    if bytes.len() as u64 > MAX_METADATA {
        return Err("Minecraft transaction metadata is too large".into());
    }
    check_regular_or_absent(path)?;
    let temp = path.with_extension("json.tmp");
    check_regular_or_absent(&temp)?;
    if temp.exists() {
        fs::remove_file(&temp).map_err(io_error)?;
    }
    write_new(&temp, &bytes)?;
    fs::rename(temp, path).map_err(io_error)
}
fn busy_check(busy: &impl Fn() -> bool) -> Result<(), String> {
    if busy() {
        Err("Minecraft or another Java app is running; close it before updating the pack".into())
    } else {
        Ok(())
    }
}
fn check_private_tree(path: &Path, count: &mut usize) -> Result<(), String> {
    let Some(meta) = metadata(path)? else {
        return Ok(());
    };
    *count += 1;
    if *count > MAX_FILES * 4 + 16 {
        return Err("Unexpectedly large Minecraft transaction directory".into());
    }
    if meta.is_dir() {
        for entry in fs::read_dir(path).map_err(io_error)? {
            check_private_tree(&entry.map_err(io_error)?.path(), count)?;
        }
    } else if !meta.is_file() {
        return Err("Unexpected Minecraft transaction entry".into());
    }
    Ok(())
}
fn cleanup_transaction(control: &Path) -> Result<(), String> {
    let txn = control.join("transaction");
    check_private_tree(&txn, &mut 0)?;
    if txn.exists() {
        fs::remove_dir_all(txn).map_err(io_error)?;
    }
    Ok(())
}

pub fn recover(lock: &ProfileLock, busy: impl Fn() -> bool) -> Result<(), String> {
    let control = control_dir(lock.profile())?;
    let Some(journal) = read_json::<Journal>(&control.join("journal.json"))? else {
        return Ok(());
    };
    busy_check(&busy)?;
    if journal.schema != 1 {
        return Err("Unsupported Minecraft interruption journal".into());
    }
    if let Some(state) = &journal.old_state {
        validate_state(state)?;
    }
    validate_paths(journal.entries.iter().map(|e| e.path.as_str()))?;
    if journal.entries.len() > MAX_FILES {
        return Err("Minecraft journal has too many changes".into());
    }
    let txn = control.join("transaction");
    check_private_tree(&txn, &mut 0)?;
    let mut unique = BTreeSet::new();
    // Validate every destination and backup before restoring anything. Backups
    // remain intact so recovery itself can be interrupted and safely retried.
    for (i, entry) in journal.entries.iter().enumerate() {
        if !unique.insert(&entry.path)
            || entry
                .old_hash
                .iter()
                .chain(&entry.new_hash)
                .any(|h| !valid_hash(h))
        {
            return Err("Corrupt Minecraft interruption journal".into());
        }
        if entry.old_hash.is_none() && entry.new_hash.is_none() {
            return Err("Empty Minecraft journal change".into());
        }
        let dest = target(lock.profile(), &entry.path)?;
        let current = hash_file(&dest)?;
        if current.is_some() && current != entry.old_hash && current != entry.new_hash {
            return Err(format!(
                "{} changed after interruption; preserve it and repair manually",
                entry.path
            ));
        }
        if let Some(old_hash) = &entry.old_hash {
            if hash_file(&txn.join("backup").join(i.to_string()))?.as_ref() != Some(old_hash) {
                return Err(format!(
                    "Missing or corrupt recovery backup for {}",
                    entry.path
                ));
            }
        }
    }
    check_regular_or_absent(&control.join("state.json"))?;
    busy_check(&busy)?;
    for (i, entry) in journal.entries.iter().enumerate() {
        busy_check(&busy)?;
        let dest = target(lock.profile(), &entry.path)?;
        if entry.old_hash.is_some() {
            fs::create_dir_all(dest.parent().unwrap()).map_err(io_error)?;
            let restore = txn.join(format!("restore-{i}"));
            check_regular_or_absent(&restore)?;
            if restore.exists() {
                fs::remove_file(&restore).map_err(io_error)?;
            }
            fs::copy(txn.join("backup").join(i.to_string()), &restore).map_err(io_error)?;
            OpenOptions::new()
                .write(true)
                .open(&restore)
                .map_err(io_error)?
                .sync_all()
                .map_err(io_error)?;
            fs::rename(restore, dest).map_err(io_error)?;
        } else if dest.exists() {
            fs::remove_file(dest).map_err(io_error)?;
        }
    }
    if let Some(state) = &journal.old_state {
        atomic_json(&control.join("state.json"), state)?;
    } else if control.join("state.json").exists() {
        fs::remove_file(control.join("state.json")).map_err(io_error)?;
    }
    fs::remove_file(control.join("journal.json")).map_err(io_error)?;
    cleanup_transaction(&control)
}

pub fn apply(
    lock: &ProfileLock,
    baseline: &BTreeMap<String, String>,
    files: &[PackFile],
    version: &str,
    repair: bool,
    busy: impl Fn() -> bool,
) -> Result<InstallReport, String> {
    busy_check(&busy)?;
    recover(lock, &busy)?;
    let old_state = read_state(lock)?;
    let next_version = version_tuple(version)?;
    if let Some(state) = &old_state {
        if next_version < version_tuple(&state.version)? {
            return Err("Refusing to downgrade the installed Minecraft pack".into());
        }
        let owned: BTreeMap<_, _> = state
            .files
            .iter()
            .map(|(p, o)| (p.clone(), o.hash.clone()))
            .collect();
        if &owned != baseline {
            return Err("Minecraft ownership changed; check the pack again".into());
        }
    }
    validate_paths(
        baseline
            .keys()
            .map(String::as_str)
            .chain(files.iter().map(|f| f.path.as_str())),
    )?;
    if baseline.values().any(|h| !valid_hash(h)) {
        return Err("Invalid SHA512 in adopted pack ownership".into());
    }
    let mut new_files = BTreeMap::new();
    let mut total = 0usize;
    for file in files {
        total = total
            .checked_add(file.bytes.len())
            .ok_or("Minecraft pack is too large")?;
        if total > MAX_TOTAL || file.bytes.len() as u64 > MAX_FILE {
            return Err("Minecraft pack exceeds size limits".into());
        }
        if file.mutable && !mutable_default(&file.path) {
            return Err(format!("Unsupported mutable pack path: {}", file.path));
        }
        if new_files.insert(file.path.clone(), file).is_some() {
            return Err(format!("Duplicate pack path: {}", file.path));
        }
    }
    let mut report = InstallReport::default();
    let mut entries = Vec::new();
    let all_paths: BTreeSet<_> = baseline.keys().chain(new_files.keys()).collect();
    for path in all_paths {
        let dest = target(lock.profile(), path)?;
        let current = hash_file(&dest)?;
        let previous = baseline.get(path);
        let new = new_files.get(path);
        // Settings the game rewrites (configs, shader options) are the
        // player's once touched, even if an older release owned them strictly.
        let mutable = mutable_default(path)
            || old_state
                .as_ref()
                .and_then(|s| s.files.get(path))
                .is_some_and(|o| o.mutable);
        // Modrinth's own "disable mod" renames the jar; that choice stands.
        let disabled = !mutable && current.is_none() && disabled_copy(lock.profile(), path);
        match (previous, new) {
            (None, Some(new)) => {
                if current.as_deref() == Some(hash_bytes(&new.bytes).as_str()) {
                    // Already exactly the pack's file: adopt it as it is.
                    continue;
                }
                if current.is_some() {
                    return Err(format!(
                        "Unowned file conflicts with the pack: {path}; move it aside first"
                    ));
                }
                if disabled {
                    report.preserved += 1;
                    continue;
                }
                entries.push(Change {
                    path: path.clone(),
                    old_hash: None,
                    new_hash: Some(hash_bytes(&new.bytes)),
                });
                report.written += 1;
            }
            (Some(old), Some(new)) => {
                if mutable && current.as_ref() != Some(old) {
                    report.preserved += 1;
                    continue;
                }
                // Repair puts the pack's own file back; otherwise a changed
                // mod stays untouched and the update waits.
                if !mutable && !repair && current.is_some() && current.as_ref() != Some(old) {
                    return Err(format!(
                        "Pack-owned file was modified: {path}; Repair pack replaces it"
                    ));
                }
                if disabled {
                    report.preserved += 1;
                    continue;
                }
                // A missing pack file is simply put back.
                let next = hash_bytes(&new.bytes);
                if current.as_ref() != Some(&next) {
                    entries.push(Change {
                        path: path.clone(),
                        old_hash: current,
                        new_hash: Some(next),
                    });
                    report.written += 1;
                }
            }
            (Some(old), None) => {
                if mutable {
                    report.preserved += 1;
                    continue;
                }
                if current.is_some() && current.as_ref() != Some(old) {
                    return Err(format!(
                        "Obsolete pack file was modified: {path}; move it aside first"
                    ));
                }
                if current.is_some() {
                    entries.push(Change {
                        path: path.clone(),
                        old_hash: current,
                        new_hash: None,
                    });
                    report.removed += 1;
                }
            }
            (None, None) => unreachable!(),
        }
    }
    let mut next_state = State {
        schema: 1,
        version: version.into(),
        files: new_files
            .iter()
            .map(|(p, f)| {
                (
                    p.clone(),
                    Owned {
                        hash: hash_bytes(&f.bytes),
                        mutable: f.mutable,
                    },
                )
            })
            .collect(),
    };
    // Remember defaults omitted by this release. This preserves a player's
    // edits/deletions if a later release reintroduces that path. The hash stays
    // the last shipped hash; local player bytes never become a baseline.
    for (path, hash) in baseline {
        let mutable = old_state
            .as_ref()
            .and_then(|s| s.files.get(path))
            .map(|o| o.mutable)
            .unwrap_or_else(|| mutable_default(path));
        if mutable {
            next_state
                .files
                .entry(path.clone())
                .or_insert_with(|| Owned {
                    hash: hash.clone(),
                    mutable: true,
                });
        }
    }
    let control = control_dir(lock.profile())?;
    busy_check(&busy)?;
    cleanup_transaction(&control)?;
    let txn = control.join("transaction");
    fs::create_dir(&txn).map_err(io_error)?;
    fs::create_dir(txn.join("backup")).map_err(io_error)?;
    fs::create_dir(txn.join("stage")).map_err(io_error)?;
    for (i, entry) in entries.iter().enumerate() {
        let dest = target(lock.profile(), &entry.path)?;
        if hash_file(&dest)? != entry.old_hash {
            return Err(format!(
                "{} changed while preparing the update; try again",
                entry.path
            ));
        }
        if let Some(old_hash) = &entry.old_hash {
            let backup = txn.join("backup").join(i.to_string());
            fs::copy(dest, &backup).map_err(io_error)?;
            OpenOptions::new()
                .write(true)
                .open(&backup)
                .map_err(io_error)?
                .sync_all()
                .map_err(io_error)?;
            if hash_file(&backup)?.as_ref() != Some(old_hash) {
                return Err("Profile changed while backing up the pack".into());
            }
        }
        if entry.new_hash.is_some() {
            write_new(
                &txn.join("stage").join(i.to_string()),
                &new_files[&entry.path].bytes,
            )?;
        }
    }
    busy_check(&busy)?;
    let journal = Journal {
        schema: 1,
        old_state,
        entries,
    };
    atomic_json(&control.join("journal.json"), &journal)?;
    let result = (|| -> Result<(), String> {
        for (i, entry) in journal.entries.iter().enumerate() {
            busy_check(&busy)?;
            let dest = target(lock.profile(), &entry.path)?;
            if hash_file(&dest)? != entry.old_hash {
                return Err(format!(
                    "{} changed before replacement; try again",
                    entry.path
                ));
            }
            if entry.new_hash.is_some() {
                fs::create_dir_all(dest.parent().unwrap()).map_err(io_error)?;
                fs::rename(txn.join("stage").join(i.to_string()), dest).map_err(io_error)?;
            } else {
                fs::remove_file(dest).map_err(io_error)?;
            }
        }
        busy_check(&busy)?;
        atomic_json(&control.join("state.json"), &next_state)?;
        fs::remove_file(control.join("journal.json")).map_err(io_error)?;
        Ok(())
    })();
    if let Err(error) = result {
        // If a game has appeared, retain the journal rather than mutate files
        // during rollback. Launch remains blocked until safe recovery.
        if let Err(recovery) = recover(lock, &busy) {
            return Err(format!("{error}. Recovery pending: {recovery}"));
        }
        return Err(error);
    }
    // Cleanup is retried on the next operation if Windows has a file open.
    let _ = cleanup_transaction(&control);
    Ok(report)
}

#[cfg(test)]
#[path = "pack_tests.rs"]
mod tests;
