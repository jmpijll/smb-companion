use super::*;
use std::{
    fs,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "smb-pack-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn write(&self, path: &str, bytes: &[u8]) {
        let p = self.0.join(path);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }
    fn read(&self, path: &str) -> Vec<u8> {
        fs::read(self.0.join(path)).unwrap()
    }
    fn lock(&self) -> ProfileLock {
        ProfileLock::acquire(&self.0).unwrap()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn file(path: &str, bytes: &str, mutable: bool) -> PackFile {
    PackFile {
        path: path.into(),
        bytes: bytes.as_bytes().to_vec(),
        mutable,
    }
}
fn hash(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA512, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[test]
fn adoption_removes_only_exact_owned_files_and_preserves_personal_files() {
    let t = Temp::new();
    t.write("mods/old.jar", b"old");
    t.write("mods/personal.jar", b"personal");
    t.write("saves/world/level.dat", b"world");
    let lock = t.lock();
    let baseline = BTreeMap::from([("mods/old.jar".into(), hash(b"old"))]);
    let report = apply(
        &lock,
        &baseline,
        &[file("mods/new.jar", "new", false)],
        "1.0.1",
        false,
        || false,
    )
    .unwrap();
    assert_eq!((report.written, report.removed), (1, 1));
    assert!(!t.0.join("mods/old.jar").exists());
    assert_eq!(t.read("mods/personal.jar"), b"personal");
    assert_eq!(t.read("saves/world/level.dat"), b"world");
    assert_eq!(load_version(&lock).unwrap().as_deref(), Some("1.0.1"));
    assert_eq!(
        owned_files(&lock).unwrap(),
        BTreeMap::from([("mods/new.jar".into(), hash(b"new"))])
    );
}
#[test]
fn mutable_three_way_preserves_edits_and_deletions_across_upgrades() {
    let t = Temp::new();
    let lock = t.lock();
    let defaults = |s: &str| {
        vec![
            file("config/changed.txt", s, true),
            file("options.txt", s, true),
            file("config/deleted.txt", s, true),
        ]
    };
    apply(
        &lock,
        &BTreeMap::new(),
        &defaults("one"),
        "1.0.0",
        false,
        || false,
    )
    .unwrap();
    t.write("config/changed.txt", b"player");
    fs::remove_file(t.0.join("config/deleted.txt")).unwrap();
    for (version, shipped) in [("1.0.1", "two"), ("1.0.2", "three")] {
        let baseline = owned_files(&lock).unwrap();
        let report = apply(&lock, &baseline, &defaults(shipped), version, false, || {
            false
        })
        .unwrap();
        assert_eq!((report.written, report.preserved), (1, 2));
        assert_eq!(t.read("config/changed.txt"), b"player");
        assert_eq!(t.read("options.txt"), shipped.as_bytes());
        assert!(!t.0.join("config/deleted.txt").exists());
        assert_eq!(
            owned_files(&lock).unwrap()["config/changed.txt"],
            hash(shipped.as_bytes())
        );
    }
}

#[test]
fn removed_then_reintroduced_defaults_preserve_player_edits_and_deletions() {
    let t = Temp::new();
    let lock = t.lock();
    let defaults = |value: &str| {
        [
            file("config/edited.txt", value, true),
            file("config/deleted.txt", value, true),
        ]
    };
    apply(
        &lock,
        &BTreeMap::new(),
        &defaults("old"),
        "1.0.0",
        false,
        || false,
    )
    .unwrap();
    t.write("config/edited.txt", b"player");
    fs::remove_file(t.0.join("config/deleted.txt")).unwrap();
    let baseline = owned_files(&lock).unwrap();
    apply(&lock, &baseline, &[], "1.0.1", false, || false).unwrap();
    let retained = owned_files(&lock).unwrap();
    assert_eq!(retained, baseline);
    apply(&lock, &retained, &defaults("new"), "1.0.2", false, || false).unwrap();
    assert_eq!(t.read("config/edited.txt"), b"player");
    assert!(!t.0.join("config/deleted.txt").exists());
    assert_eq!(
        owned_files(&lock).unwrap()["config/deleted.txt"],
        hash(b"new")
    );
}

#[test]
fn journal_requires_explicit_old_state_and_hash_fields() {
    for field in ["old_state", "old_hash", "new_hash"] {
        let t = Temp::new();
        let lock = t.lock();
        interrupted(&t);
        let mut journal: serde_json::Value =
            serde_json::from_slice(&t.read(".smb-companion/journal.json")).unwrap();
        if field == "old_state" {
            journal.as_object_mut().unwrap().remove(field);
        } else {
            journal["entries"][0].as_object_mut().unwrap().remove(field);
        }
        t.write(
            ".smb-companion/journal.json",
            &serde_json::to_vec(&journal).unwrap(),
        );
        assert!(
            recover(&lock, || false).is_err(),
            "accepted missing {field}"
        );
        assert_eq!(t.read("mods/a.jar"), b"new");
        assert_eq!(t.read("mods/added.jar"), b"added");
    }
}
#[test]
fn immutable_conflicts_and_unowned_collisions_fail_before_any_change() {
    // An owned, changed mod waits for Repair; an unowned file is never replaced.
    for (owned, repair) in [(true, false), (false, true)] {
        let t = Temp::new();
        t.write("mods/z.jar", b"personal");
        let lock = t.lock();
        let baseline = if owned {
            BTreeMap::from([("mods/z.jar".into(), hash(b"old"))])
        } else {
            BTreeMap::new()
        };
        assert!(apply(
            &lock,
            &baseline,
            &[
                file("mods/a.jar", "new", false),
                file("mods/z.jar", "new", false)
            ],
            "1.0.0",
            repair,
            || false
        )
        .is_err());
        assert_eq!(t.read("mods/z.jar"), b"personal");
        assert!(!t.0.join("mods/a.jar").exists());
        assert_eq!(load_version(&lock).unwrap(), None);
        assert!(!has_pending(&t.0).unwrap());
    }
}
#[test]
fn missing_pack_files_come_back_unless_disabled_in_modrinth() {
    let t = Temp::new();
    let lock = t.lock();
    let files = [
        file("mods/a.jar", "old", false),
        file("mods/b.jar", "old", false),
    ];
    apply(&lock, &BTreeMap::new(), &files, "1.0.0", false, || false).unwrap();
    fs::remove_file(t.0.join("mods/a.jar")).unwrap();
    // Modrinth's "disable" renames the jar.
    fs::rename(t.0.join("mods/b.jar"), t.0.join("mods/b.jar.disabled")).unwrap();
    let baseline = owned_files(&lock).unwrap();
    apply(&lock, &baseline, &files, "1.0.0", false, || false).unwrap();
    assert_eq!(t.read("mods/a.jar"), b"old");
    assert!(!t.0.join("mods/b.jar").exists());
    assert_eq!(t.read("mods/b.jar.disabled"), b"old");
}
#[test]
fn repair_replaces_a_changed_pack_mod() {
    let t = Temp::new();
    let lock = t.lock();
    let files = [file("mods/a.jar", "old", false)];
    apply(&lock, &BTreeMap::new(), &files, "1.0.0", false, || false).unwrap();
    t.write("mods/a.jar", b"changed");
    let baseline = owned_files(&lock).unwrap();
    assert!(apply(&lock, &baseline, &files, "1.0.0", false, || false).is_err());
    apply(&lock, &baseline, &files, "1.0.0", true, || false).unwrap();
    assert_eq!(t.read("mods/a.jar"), b"old");
}
#[test]
fn shader_settings_written_by_the_game_stay_the_players() {
    let t = Temp::new();
    let lock = t.lock();
    let path = "shaderpacks/Complementary.zip.txt";
    assert!(crate::minecraft::channel::mutable_path(path));
    apply(
        &lock,
        &BTreeMap::new(),
        &[file(path, "default", true)],
        "1.0.0",
        false,
        || false,
    )
    .unwrap();
    t.write(path, b"player");
    let baseline = owned_files(&lock).unwrap();
    apply(
        &lock,
        &baseline,
        &[file(path, "new default", true)],
        "1.1.0",
        false,
        || false,
    )
    .unwrap();
    assert_eq!(t.read(path), b"player");
}
#[test]
fn a_file_already_matching_the_pack_is_adopted() {
    let t = Temp::new();
    let lock = t.lock();
    t.write("mods/a.jar", b"same");
    apply(
        &lock,
        &BTreeMap::new(),
        &[file("mods/a.jar", "same", false)],
        "1.0.0",
        false,
        || false,
    )
    .unwrap();
    assert_eq!(owned_files(&lock).unwrap()["mods/a.jar"], hash(b"same"));
}
#[test]
fn busy_defers_before_mutation() {
    let t = Temp::new();
    let lock = t.lock();
    assert!(apply(
        &lock,
        &BTreeMap::new(),
        &[file("mods/a.jar", "new", false)],
        "1.0.0",
        false,
        || true
    )
    .is_err());
    assert!(!t.0.join("mods").exists());
    assert!(!has_pending(&t.0).unwrap());
}
#[test]
fn unsafe_and_case_colliding_paths_are_rejected() {
    for path in [
        "../outside",
        "mods/../outside",
        "mods\\evil.jar",
        "mods/C:evil",
        "/mods/a",
        "C:/mods/a",
        "//server/share",
        "mods/CON.jar",
        "mods/a. ",
        "saves/world.dat",
        "mods//a",
        "mods/./a",
        "options.txt/child",
        "mods/aux/file",
    ] {
        let t = Temp::new();
        let lock = t.lock();
        assert!(
            apply(
                &lock,
                &BTreeMap::new(),
                &[file(path, "bad", false)],
                "1.0.0",
                false,
                || false
            )
            .is_err(),
            "accepted {path}"
        );
    }
    let t = Temp::new();
    let lock = t.lock();
    assert!(apply(
        &lock,
        &BTreeMap::new(),
        &[
            file("mods/A.jar", "a", false),
            file("mods/a.jar", "b", false)
        ],
        "1.0.0",
        false,
        || false
    )
    .is_err());
}
#[test]
fn hardlink_replacement_does_not_modify_shared_content() {
    let t = Temp::new();
    t.write("shared", b"old");
    fs::create_dir(t.0.join("mods")).unwrap();
    fs::hard_link(t.0.join("shared"), t.0.join("mods/a.jar")).unwrap();
    let lock = t.lock();
    let baseline = BTreeMap::from([("mods/a.jar".into(), hash(b"old"))]);
    apply(
        &lock,
        &baseline,
        &[file("mods/a.jar", "new", false)],
        "1.0.1",
        false,
        || false,
    )
    .unwrap();
    assert_eq!(t.read("shared"), b"old");
    assert_eq!(t.read("mods/a.jar"), b"new");
}

#[cfg(windows)]
#[test]
#[allow(clippy::permissions_set_readonly_false)] // Restore the Windows read-only attribute on our disposable test file.
fn readonly_file_failure_does_not_poison_later_repairs() {
    let t = Temp::new();
    t.write("mods/a.jar", b"old");
    let lock = t.lock();
    let path = t.0.join("mods/a.jar");
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&path, perms.clone()).unwrap();
    let baseline = BTreeMap::from([("mods/a.jar".into(), hash(b"old"))]);
    let _ = apply(
        &lock,
        &baseline,
        &[file("mods/a.jar", "new", false)],
        "1.0.1",
        false,
        || false,
    );
    perms.set_readonly(false);
    fs::set_permissions(&path, perms).unwrap();
    recover(&lock, || false).unwrap();
    apply(
        &lock,
        &baseline,
        &[file("mods/a.jar", "new", false)],
        "1.0.1",
        true,
        || false,
    )
    .unwrap();
    assert_eq!(t.read("mods/a.jar"), b"new");
}
#[test]
fn profile_lock_serializes_and_is_bounded() {
    let t = Temp::new();
    let lock = t.lock();
    let start = std::time::Instant::now();
    assert!(ProfileLock::acquire(&t.0).is_err());
    assert!(start.elapsed() >= std::time::Duration::from_millis(1800));
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
    drop(lock);
    assert!(ProfileLock::acquire(&t.0).is_ok());
}

#[test]
fn waiting_lock_acquires_after_the_prior_operation_finishes() {
    let t = Temp::new();
    let lock = t.lock();
    let profile = t.0.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let child = std::thread::spawn(move || {
        ready_tx.send(()).unwrap();
        ProfileLock::acquire(&profile)
    });
    ready_rx.recv().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    drop(lock);
    let result = child.join().unwrap();
    assert!(result.is_ok(), "{}", result.err().unwrap_or_default());
}

#[test]
fn profile_lock_serializes_separate_processes() {
    const PROBE: &str = "SMB_PACK_LOCK_TEST_PROFILE";
    if let Some(profile) = std::env::var_os(PROBE) {
        let result = ProfileLock::acquire(Path::new(&profile));
        assert_eq!(
            result.is_ok(),
            std::env::var_os("SMB_PACK_LOCK_TEST_AVAILABLE").is_some()
        );
        return;
    }
    let t = Temp::new();
    let lock = t.lock();
    let probe = |available: bool| {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "minecraft::pack::tests::profile_lock_serializes_separate_processes",
                "--nocapture",
            ])
            .env(PROBE, &t.0);
        if available {
            command.env("SMB_PACK_LOCK_TEST_AVAILABLE", "1");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    probe(false);
    drop(lock);
    probe(true);
}

#[test]
fn metadata_output_limit_cannot_create_an_unrecoverable_journal() {
    let t = Temp::new();
    let lock = t.lock();
    let journal = lock.profile().join(".smb-companion/journal.json");
    let oversized = "x".repeat(MAX_METADATA as usize);
    assert!(atomic_json(&journal, &oversized).is_err());
    assert!(!has_pending(&t.0).unwrap());
}

#[test]
fn missing_profile_is_not_a_safe_disabled_hook_target() {
    let t = Temp::new();
    assert!(has_pending(&t.0.join("absent")).is_err());
}

#[test]
fn actual_partial_apply_retains_journal_and_recovers_old_installation() {
    let t = Temp::new();
    let lock = t.lock();
    apply(
        &lock,
        &BTreeMap::new(),
        &[
            file("mods/a.jar", "old", false),
            file("mods/z.jar", "old", false),
        ],
        "1.0.0",
        false,
        || false,
    )
    .unwrap();
    let baseline = owned_files(&lock).unwrap();
    let busy = || fs::read(t.0.join("mods/a.jar")).unwrap() == b"new";
    assert!(apply(
        &lock,
        &baseline,
        &[
            file("mods/a.jar", "new", false),
            file("mods/z.jar", "new", false)
        ],
        "1.0.1",
        false,
        busy
    )
    .is_err());
    assert!(has_pending(&t.0).unwrap());
    assert_eq!(t.read("mods/a.jar"), b"new");
    assert_eq!(t.read("mods/z.jar"), b"old");
    recover(&lock, || false).unwrap();
    assert_eq!(t.read("mods/a.jar"), b"old");
    assert_eq!(t.read("mods/z.jar"), b"old");
    assert_eq!(load_version(&lock).unwrap().as_deref(), Some("1.0.0"));
    assert_eq!(owned_files(&lock).unwrap(), baseline);
}

#[test]
fn path_validation_accepts_all_allowed_roots_and_rejects_aliases() {
    for path in [
        "mods/a.jar",
        "config/a/b.toml",
        "resourcepacks/pack.zip",
        "shaderpacks/pack.zip",
        "options.txt",
        "servers.dat",
    ] {
        validate_path(path).unwrap();
    }
    for path in [
        "mods/COM1.jar",
        "mods/LPT9",
        "mods/a:stream",
        "mods/dir./x",
        "mods/a\u{0}",
        "mods/é.jar",
        "Mods/a.jar",
    ] {
        assert!(validate_path(path).is_err(), "{path}");
    }
}

#[test]
fn invalid_baseline_existing_case_collision_and_changed_obsolete_file_are_safe() {
    let t = Temp::new();
    let lock = t.lock();
    t.write("mods/A.jar", b"personal");
    assert!(apply(
        &lock,
        &BTreeMap::new(),
        &[file("mods/a.jar", "bad", false)],
        "1.0.0",
        false,
        || false
    )
    .is_err());
    assert_eq!(t.read("mods/A.jar"), b"personal");
    for baseline in [
        BTreeMap::from([("../outside".into(), hash(b"old"))]),
        BTreeMap::from([("mods/A.jar".into(), "bad".into())]),
        BTreeMap::from([("mods/A.jar".into(), hash(b"old"))]),
    ] {
        assert!(apply(&lock, &baseline, &[], "1.0.0", true, || false).is_err());
        assert_eq!(t.read("mods/A.jar"), b"personal");
    }
}

#[test]
fn corrupted_state_and_downgrades_do_not_authorize_writes() {
    let t = Temp::new();
    let lock = t.lock();
    apply(
        &lock,
        &BTreeMap::new(),
        &[file("mods/a.jar", "old", false)],
        "1.0.1",
        false,
        || false,
    )
    .unwrap();
    let baseline = owned_files(&lock).unwrap();
    assert!(apply(
        &lock,
        &baseline,
        &[file("mods/a.jar", "bad", false)],
        "1.0.0",
        false,
        || false
    )
    .is_err());
    t.write(".smb-companion/state.json", b"not json");
    assert!(apply(
        &lock,
        &baseline,
        &[file("mods/a.jar", "bad", false)],
        "1.0.2",
        false,
        || false
    )
    .is_err());
    assert_eq!(t.read("mods/a.jar"), b"old");
}

fn interrupted(t: &Temp) {
    // Simulate a process stopping after file renames and new state write,
    // but before removing the journal (the transaction commit marker).
    t.write("mods/a.jar", b"new");
    t.write("mods/added.jar", b"added");
    t.write(".smb-companion/transaction/backup/0", b"old");
    t.write(
        ".smb-companion/state.json",
        br#"{"schema":1,"version":"1.0.1","files":{}}"#,
    );
    let journal = serde_json::json!({"schema":1,"old_state":null,"entries":[
        {"path":"mods/a.jar","old_hash":hash(b"old"),"new_hash":hash(b"new")},
        {"path":"mods/added.jar","old_hash":null,"new_hash":hash(b"added")}
    ]});
    t.write(
        ".smb-companion/journal.json",
        &serde_json::to_vec(&journal).unwrap(),
    );
}
#[test]
fn interrupted_transaction_rolls_back_before_another_update() {
    let t = Temp::new();
    let lock = t.lock();
    interrupted(&t);
    assert!(has_pending(&t.0).unwrap());
    assert!(load_version(&lock).is_err());
    assert!(recover(&lock, || true).is_err());
    assert_eq!(t.read("mods/a.jar"), b"new");
    recover(&lock, || false).unwrap();
    assert_eq!(t.read("mods/a.jar"), b"old");
    assert!(!t.0.join("mods/added.jar").exists());
    assert_eq!(load_version(&lock).unwrap(), None);
    assert!(!has_pending(&t.0).unwrap());
    recover(&lock, || false).unwrap();
}
#[test]
fn corrupt_or_escaping_journal_never_changes_profile_files() {
    for escape in [false, true] {
        let t = Temp::new();
        let lock = t.lock();
        interrupted(&t);
        if escape {
            let mut j: serde_json::Value =
                serde_json::from_slice(&t.read(".smb-companion/journal.json")).unwrap();
            j["entries"][0]["path"] = "../outside".into();
            t.write(
                ".smb-companion/journal.json",
                &serde_json::to_vec(&j).unwrap(),
            );
        } else {
            t.write(".smb-companion/transaction/backup/0", b"corrupt");
        }
        assert!(recover(&lock, || false).is_err());
        assert_eq!(t.read("mods/a.jar"), b"new");
        assert_eq!(t.read("mods/added.jar"), b"added");
        assert!(has_pending(&t.0).unwrap());
    }
}
#[cfg(windows)]
#[test]
fn junctions_are_rejected_without_touching_their_target() {
    use std::os::windows::process::CommandExt;
    let t = Temp::new();
    let outside = Temp::new();
    outside.write("a.jar", b"outside");
    let status = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(t.0.join("mods"))
        .arg(&outside.0)
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let lock = t.lock();
    assert!(apply(
        &lock,
        &BTreeMap::new(),
        &[file("mods/a.jar", "bad", false)],
        "1.0.0",
        false,
        || false
    )
    .is_err());
    assert_eq!(outside.read("a.jar"), b"outside");
    fs::remove_dir(t.0.join("mods")).unwrap();
}
#[cfg(unix)]
#[test]
fn symlinks_are_rejected_without_touching_their_target() {
    let t = Temp::new();
    let outside = Temp::new();
    outside.write("a.jar", b"outside");
    std::os::unix::fs::symlink(&outside.0, t.0.join("mods")).unwrap();
    let lock = t.lock();
    assert!(apply(
        &lock,
        &BTreeMap::new(),
        &[file("mods/a.jar", "bad", false)],
        "1.0.0",
        false,
        || false
    )
    .is_err());
    assert_eq!(outside.read("a.jar"), b"outside");
}
