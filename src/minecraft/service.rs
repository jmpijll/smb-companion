//! Background orchestration shared by the GUI and native synchronous launch hook.
use super::{
    channel::{self, Channel, Pack, PackRef},
    modrinth::{self, Instance},
    pack,
    transport::Source,
};
use crate::system;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
};

const CHANNEL: &str =
    "https://raw.githubusercontent.com/jmpijll/smb-minecraft-client/main/stable.json";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub instance_id: String,
    pub automatic: bool,
    pub launch_after_update: bool,
    pub direct_hook: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            instance_id: String::new(),
            automatic: true,
            launch_after_update: true,
            direct_hook: true,
        }
    }
}
#[derive(Clone, Default)]
pub struct View {
    pub instances: Vec<Instance>,
    pub selected: Option<Instance>,
    pub busy: bool,
    pub title: String,
    pub detail: String,
    pub version: String,
    pub latest: String,
    pub hook_ready: bool,
    pub error: String,
}
#[derive(Clone, Copy)]
pub enum Action {
    Discover,
    Check,
    Play,
    Repair,
}
#[derive(Clone)]
pub struct Handle {
    settings: Arc<Mutex<Settings>>,
    view: Arc<Mutex<View>>,
    tx: mpsc::SyncSender<Action>,
    live: bool,
}

fn settings_path() -> PathBuf {
    system::data_dir().join("minecraft.json")
}
fn load() -> Result<Settings, String> {
    load_from(&settings_path())
}
fn load_from(path: &Path) -> Result<Settings, String> {
    let f = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(_) => return Err("Minecraft settings could not be opened".into()),
    };
    let mut b = Vec::new();
    f.take(65537)
        .read_to_end(&mut b)
        .map_err(|_| "Minecraft settings could not be read")?;
    if b.len() > 65536 {
        return Err("Minecraft settings are too large".into());
    }
    serde_json::from_slice(&b)
        .map_err(|_| "Minecraft settings are unreadable. Choose your profile again in SMB.".into())
}
fn save(s: &Settings) -> Result<(), String> {
    if s.instance_id.len() > 200 || s.instance_id.chars().any(char::is_control) {
        return Err("Invalid profile selection".into());
    }
    let p = settings_path();
    fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    let temp = p.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(
        &temp,
        serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(temp, p).map_err(|e| e.to_string())
}
impl Handle {
    pub fn new(live: bool, demo: bool) -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        let loaded = if live {
            load()
        } else {
            Ok(Settings::default())
        };
        let error = loaded.as_ref().err().cloned().unwrap_or_default();
        let mut config = loaded.unwrap_or_default();
        let mut v = View {
            title: "Choose your SMB instance".into(),
            detail: "Keep the pack ready before you play.".into(),
            error,
            ..Default::default()
        };
        if demo {
            let i = Instance {
                id: "local:demo".into(),
                name: "SMB".into(),
                directory: PathBuf::from("C:/Games/Modrinth/profiles/smb-0.13.0"),
                imported_version: "0.13.0".into(),
                minecraft: "26.3".into(),
                loader: "fabric".into(),
                loader_version: "0.20.1".into(),
                ..Default::default()
            };
            config.instance_id = i.id.clone();
            v.instances = vec![i.clone()];
            v.selected = Some(i);
            v.title = "Ready when you are".into();
            v.detail =
                "Your pack is compatible with the SMB server. Personal settings stay yours.".into();
            v.version = "0.13.0".into();
            v.latest = "0.13.0".into();
            v.hook_ready = true;
            let args: Vec<_> = std::env::args().collect();
            let state = args
                .iter()
                .position(|s| s == "--minecraft-demo-state")
                .and_then(|n| args.get(n + 1))
                .map(String::as_str);
            if state == Some("setup") {
                v.hook_ready = false;
                v.title = "One small setup, then play".into();
                v.detail = "Select your Modrinth instance and add the launch hook once.".into();
            }
            if state == Some("error") {
                v.hook_ready = false;
                v.title = "Pack left untouched".into();
                v.error="The SMB client channel is unavailable. Check your connection, then try again. No installed files were changed.".into();
            }
        }
        let this = Self {
            settings: Arc::new(Mutex::new(config)),
            view: Arc::new(Mutex::new(v)),
            tx,
            live,
        };
        if live {
            let settings = this.settings.clone();
            let view = this.view.clone();
            std::thread::spawn(move || {
                let _ = refresh(&settings.lock().unwrap(), &view);
                while let Ok(action) = rx.recv() {
                    let config = settings.lock().unwrap().clone();
                    {
                        let mut v = view.lock().unwrap();
                        v.busy = true;
                        v.error.clear();
                        v.title = "Checking your pack".into();
                        v.detail =
                            "SMB stays quiet while the selected instance is prepared.".into();
                    }
                    let result = if matches!(action, Action::Discover) {
                        refresh(&config, &view)
                    } else {
                        perform(&config, action, &view)
                    };
                    let mut v = view.lock().unwrap();
                    v.busy = false;
                    if let Err(e) = result {
                        v.title = "Needs your attention".into();
                        v.error = e;
                    }
                }
            });
        }
        this
    }
    pub fn snapshot(&self) -> View {
        self.view.lock().unwrap().clone()
    }
    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }
    pub fn save(&self, s: Settings) -> Result<(), String> {
        if self.live {
            save(&s)?;
        }
        *self.settings.lock().unwrap() = s;
        if self.live {
            self.send(Action::Discover);
        }
        Ok(())
    }
    pub fn send(&self, a: Action) {
        if self.live {
            let _ = self.tx.try_send(a);
        }
    }
}
fn refresh(config: &Settings, view: &Arc<Mutex<View>>) -> Result<(), String> {
    let d = modrinth::discover().inspect_err(|e| {
        let mut v = view.lock().unwrap();
        v.error = e.clone();
        v.title = "Modrinth setup needed".into();
    })?;
    publish_discovery(config, d, view)
}
fn publish_discovery(
    config: &Settings,
    d: modrinth::Discovery,
    view: &Arc<Mutex<View>>,
) -> Result<(), String> {
    let selected = d
        .instances
        .iter()
        .find(|i| i.id == config.instance_id)
        .cloned();
    let managed_result = (|| -> Result<Option<String>, String> {
        Ok(if let Some(i) = &selected {
            if i.directory.join(".smb-companion/state.json").exists() {
                let lock = pack::ProfileLock::acquire(&i.directory)?;
                if pack::has_pending(&i.directory)? {
                    return Err(
                        "An interrupted update needs recovery. Use Check update before launching."
                            .into(),
                    );
                }
                pack::load_version(&lock)?
            } else {
                None
            }
        } else {
            None
        })
    })();
    let mut v = view.lock().unwrap();
    v.instances = d.instances;
    v.selected = selected.clone();
    v.error.clear();
    v.hook_ready = selected.as_ref().is_some_and(|i| {
        std::env::current_exe()
            .ok()
            .and_then(|p| modrinth::hook_command(&p).ok())
            .is_some_and(|h| h == i.pre_launch)
    });
    // Discovery remains available even when validation requires recovery. The
    // player must be able to select this instance and invoke Check/Repair.
    let managed_version = match managed_result {
        Ok(version) => version,
        Err(e) => {
            v.title = "Recovery needed".into();
            v.detail = "Use Check update to recover the interrupted pack before play.".into();
            v.error = e.clone();
            return Err(e);
        }
    };
    if let Some(i) = selected {
        v.version = managed_version.unwrap_or(i.imported_version);
        v.title = "Ready to check".into();
        v.detail = "Play checks the selected pack before handing launch to Modrinth.".into();
    } else {
        v.version.clear();
        v.title = "Choose your SMB instance".into();
        v.detail = "Import the SMB client pack in Modrinth, then select its profile here.".into();
    }
    Ok(())
}
fn selected(config: &Settings) -> Result<(PathBuf, Instance), String> {
    let d = modrinth::discover()?;
    let i = d
        .instances
        .into_iter()
        .find(|i| i.id == config.instance_id)
        .ok_or("Select your SMB instance in the companion first")?;
    Ok((d.executable, i))
}
fn target<'a>(
    channel: &'a Channel,
    installed: &str,
    explicit: bool,
) -> Result<Option<&'a PackRef>, String> {
    let installed = channel::version(installed)?;
    let latest = channel::version(&channel.pack.version)?;
    if installed > latest
        || (!explicit && installed >= channel::version(&channel.pack.min_version)?)
    {
        Ok(None)
    } else {
        Ok(Some(&channel.pack))
    }
}
fn runtime(i: &Instance, r: &PackRef) -> Result<(), String> {
    if i.minecraft != r.minecraft || i.loader != r.loader || i.loader_version != r.loader_version {
        return Err(format!("This pack needs Minecraft {} / Fabric {}. Import the new pack as a native Modrinth instance, then select it in SMB. Automatic updates only support the same runtime; no pack files were changed.",r.minecraft,r.loader_version));
    }
    Ok(())
}
fn filter_native(pack: &mut Pack, i: &Instance) {
    pack.retain(|p, _| {
        !(i.sync_options && p == "options.txt" || i.sync_servers && p == "servers.dat")
    });
}
fn baseline(pack: &Pack, profile: &Path) -> BTreeMap<String, String> {
    pack.iter()
        .filter(|(p, e)| !e.optional || profile.join(p).exists())
        .map(|(p, e)| (p.clone(), e.sha512.clone()))
        .collect()
}
fn update_profile(
    config: &Settings,
    i: &Instance,
    explicit: bool,
    view: &Arc<Mutex<View>>,
) -> Result<(), String> {
    let _operation = Operation::acquire()?;
    if system::minecraft_running() {
        return Err(
            "Minecraft or another Java app is running. Close it before checking the pack.".into(),
        );
    }
    let source = Source::new(
        std::env::var("SMB_MINECRAFT_CHANNEL").unwrap_or_else(|_| CHANNEL.into()),
        system::data_dir().join("minecraft-cache"),
    );
    synchronize(
        i,
        explicit,
        view,
        &source,
        system::minecraft_running,
        || selected(config).map(|(_, i)| i),
    )
}
fn synchronize(
    i: &Instance,
    explicit: bool,
    view: &Arc<Mutex<View>>,
    source: &Source,
    busy: impl Fn() -> bool,
    revalidate: impl Fn() -> Result<Instance, String>,
) -> Result<(), String> {
    if busy() {
        return Err("Minecraft or another Java app is running".into());
    }
    let lock = pack::ProfileLock::acquire(&i.directory)?;
    pack::recover(&lock, &busy)?;
    let managed = pack::load_version(&lock)?;
    let installed = managed.as_deref().unwrap_or(&i.imported_version);
    channel::version(installed)?;
    let channel = source.channel()?;
    let wanted = target(&channel, installed, explicit)?;
    {
        let mut v = view.lock().unwrap();
        v.version = installed.into();
        v.latest = channel.pack.version.clone();
    }
    // A newer unmanaged pack cannot be proved by an older channel. Never downgrade or claim validation.
    if channel::version(installed)? > channel::version(&channel.pack.version)? {
        return Err("Your instance is newer than the stable SMB channel. It was left untouched; use Modrinth directly until its exact release is in the channel.".into());
    }
    let current=channel.find(installed).ok_or("The stable channel has no exact archive for this installed pack. Import a supported stable SMB pack in Modrinth before enabling updates.")?;
    let chosen = wanted.unwrap_or(current);
    runtime(i, current)?;
    runtime(i, chosen)?;
    if i.sync_resources {
        return Err("Turn off Modrinth resource-pack synchronization for this instance before enabling SMB pack updates. Your resources have not been changed.".into());
    }
    let mut old_pack = source.archive(current)?;
    filter_native(&mut old_pack, i);
    let mut owned = if managed.is_some() {
        pack::owned_files(&lock)?
    } else {
        baseline(&old_pack, &i.directory)
    };
    if managed.is_some()
        && owned
            .keys()
            .any(|p| i.sync_options && p == "options.txt" || i.sync_servers && p == "servers.dat")
    {
        return Err("Modrinth synchronization now overlaps SMB-owned options or servers. Turn off that native sync setting before updating this managed instance.".into());
    }
    owned.retain(|p, _| {
        !(i.sync_options && p == "options.txt" || i.sync_servers && p == "servers.dat")
    });
    let mut next = if chosen.version == current.version {
        old_pack
    } else {
        source.archive(chosen)?
    };
    filter_native(&mut next, i);
    {
        let mut v = view.lock().unwrap();
        v.title = "Preparing pack files".into();
        v.detail = "Downloading and verifying before any installed file changes.".into();
    }
    let files = source.materialize(&next, &owned, &i.directory)?;
    // Re-read authoritative native profile metadata after downloads, before any mutation.
    let fresh = revalidate()?;
    if fresh.directory != i.directory
        || fresh.minecraft != i.minecraft
        || fresh.loader != i.loader
        || fresh.loader_version != i.loader_version
        || fresh.sync_options != i.sync_options
        || fresh.sync_resources != i.sync_resources
        || fresh.sync_servers != i.sync_servers
    {
        return Err(
            "The Modrinth instance changed during the check. Refresh and try again.".into(),
        );
    }
    let report = pack::apply(&lock, &owned, &files, &chosen.version, explicit, busy)?;
    let mut v = view.lock().unwrap();
    v.version = chosen.version.clone();
    v.title = "Ready when you are".into();
    v.detail = format!(
        "Pack {} · {} files updated · {} personal files preserved",
        chosen.version,
        report.written + report.removed,
        report.preserved
    );
    Ok(())
}
fn perform(config: &Settings, action: Action, view: &Arc<Mutex<View>>) -> Result<(), String> {
    let (executable, i) = selected(config)?;
    if should_update(config, action) {
        update_profile(config, &i, matches!(action, Action::Repair), view)?;
    } else {
        ensure_consistent(&i.directory)?;
        let mut v = view.lock().unwrap();
        v.title = "Automatic updates paused".into();
        v.detail = "Launching the selected pack without a channel check.".into();
    }
    if matches!(action, Action::Play) && config.launch_after_update {
        let (_, fresh) = selected(config)?;
        if fresh.directory != i.directory {
            return Err("Your selected Modrinth profile changed. Refresh before launching.".into());
        }
        ensure_consistent(&fresh.directory)?;
        modrinth::handoff(&executable, &i.id)?;
        let mut v = view.lock().unwrap();
        v.title = "Handed to Modrinth".into();
        v.detail = "Modrinth handles login and game startup. SMB will stay out of the way.".into();
    }
    Ok(())
}
fn should_update(config: &Settings, action: Action) -> bool {
    match action {
        Action::Check | Action::Repair => true,
        Action::Play => config.automatic || !config.launch_after_update,
        Action::Discover => false,
    }
}
fn ensure_consistent(profile: &Path) -> Result<(), String> {
    if pack::has_pending(profile)? {
        return Err("An interrupted pack update needs recovery. Enable updates and check the pack before launching.".into());
    }
    Ok(())
}
/// Called before any GUI, tray or single-instance work. Never activates a window.
pub fn prelaunch() -> Result<(), String> {
    let config = load()?;
    let dir = std::env::var_os("INST_DIR").ok_or(
        "Modrinth did not provide INST_DIR. Set this command as the instance pre-launch hook.",
    )?;
    let (_, i) = selected(&config)?;
    validate_hook_directory(&i.directory, Path::new(&dir))?;
    if !config.direct_hook || !config.automatic {
        return ensure_consistent(&i.directory);
    }
    update_profile(&config, &i, false, &Arc::new(Mutex::new(View::default())))
}
fn validate_hook_directory(selected: &Path, provided: &Path) -> Result<(), String> {
    let provided = provided
        .canonicalize()
        .map_err(|_| "The launcher profile folder is unavailable")?;
    if selected != provided {
        return Err("This hook belongs to another instance. Select this profile in SMB or remove its SMB hook in Modrinth.".into());
    }
    Ok(())
}
pub(crate) struct Operation(fs::File);
impl Operation {
    pub(crate) fn acquire() -> Result<Self, String> {
        use fs2::FileExt;
        let p = system::data_dir().join("minecraft-operation.lock");
        fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        let f = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(p)
            .map_err(|e| e.to_string())?;
        f.try_lock_exclusive()
            .map_err(|_| "Another SMB pack operation is active. Try again shortly.")?;
        Ok(Self(f))
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}
pub fn operation_active() -> bool {
    use fs2::FileExt;
    let p = system::data_dir().join("minecraft-operation.lock");
    if !p.exists() {
        return false;
    }
    match fs::OpenOptions::new().read(true).write(true).open(p) {
        Ok(f) => f.try_lock_exclusive().is_err(),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reference(v: &str, min: &str) -> PackRef {
        PackRef {
            version: v.into(),
            min_version: min.into(),
            url: "packs/test.mrpack".into(),
            sha256: "a".repeat(64),
            size: 1,
            minecraft: "26.3".into(),
            loader: "fabric".into(),
            loader_version: "0.20.1".into(),
        }
    }
    #[test]
    fn server_only_release_retains_compatible_pack_and_never_downgrades() {
        let c = Channel {
            schema: 1,
            pack: reference("0.13.0", "0.11.0"),
            history: vec![],
        };
        assert!(target(&c, "0.11.0", false).unwrap().is_none());
        assert_eq!(
            target(&c, "0.5.0", false).unwrap().unwrap().version,
            "0.13.0"
        );
        assert!(target(&c, "0.14.0", true).unwrap().is_none());
        assert!(target(&c, "0.5.0-dev", false).is_err());
        assert!(target(&c, "0.13.0", true).unwrap().is_some());
    }
    #[test]
    fn runtime_mismatch_blocks_before_file_work() {
        let i = Instance {
            minecraft: "26.3".into(),
            loader: "fabric".into(),
            loader_version: "0.20.1".into(),
            ..Default::default()
        };
        assert!(runtime(&i, &reference("0.13.0", "0.11.0")).is_ok());
        let mut r = reference("0.13.0", "0.11.0");
        r.minecraft = "26.4".into();
        assert!(runtime(&i, &r).is_err());
    }
    #[test]
    fn native_sync_excludes_both_old_and_new_ownership() {
        let mut p = Pack::new();
        for path in ["options.txt", "servers.dat", "config/a.json"] {
            p.insert(
                path.into(),
                channel::Entry {
                    sha512: "a".repeat(128),
                    optional: false,
                    mutable: true,
                    content: channel::Content::Inline(vec![]),
                },
            );
        }
        filter_native(
            &mut p,
            &Instance {
                sync_options: true,
                sync_servers: true,
                ..Default::default()
            },
        );
        assert_eq!(p.len(), 1);
        assert!(p.contains_key("config/a.json"));
    }
    #[test]
    fn bad_settings_fail_closed_and_missing_settings_default() {
        let p = std::env::temp_dir().join(format!("smb-settings-test-{}", std::process::id()));
        fs::write(&p, b"invalid").unwrap();
        assert!(load_from(&p).is_err());
        fs::remove_file(&p).unwrap();
        assert!(load_from(&p).unwrap().automatic);
    }
    #[test]
    fn synthetic_view_does_not_load_live_settings_or_schedule_work() {
        let h = Handle::new(false, true);
        assert_eq!(h.snapshot().version, "0.13.0");
        h.send(Action::Repair);
        assert!(!h.snapshot().busy);
    }
}
#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::io::{Cursor, Write};
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "smb-pipeline-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(p.join("profile/mods")).unwrap();
            fs::create_dir_all(p.join("profile/config")).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn archive(dir: &Path, v: &str, runtime: &str, files: &[(&str, &[u8])]) -> PackRef {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("modrinth.index.json", opts).unwrap();
        zip.write_all(serde_json::to_vec(&serde_json::json!({"formatVersion":1,"game":"minecraft","name":"SMB","versionId":v,"dependencies":{"minecraft":runtime,"fabric-loader":"0.20.1"},"files":[]})).unwrap().as_slice()).unwrap();
        for (path, bytes) in files {
            zip.start_file(format!("overrides/{path}"), opts).unwrap();
            zip.write_all(bytes).unwrap();
        }
        let b = zip.finish().unwrap().into_inner();
        let name = format!("smb-{v}.mrpack");
        fs::write(dir.join(&name), &b).unwrap();
        PackRef {
            version: v.into(),
            min_version: if v == "0.13.0" {
                "0.11.0".into()
            } else {
                v.into()
            },
            url: name,
            sha256: channel::hash256(&b),
            size: b.len() as u64,
            minecraft: runtime.into(),
            loader: "fabric".into(),
            loader_version: "0.20.1".into(),
        }
    }
    #[test]
    fn pending_managed_profile_remains_selected_with_recovery_actions_available() {
        let t = Temp::new();
        let profile = t.0.join("profile");
        {
            let lock = pack::ProfileLock::acquire(&profile).unwrap();
            pack::apply(
                &lock,
                &BTreeMap::new(),
                &[pack::PackFile {
                    path: "mods/a.jar".into(),
                    bytes: b"old".to_vec(),
                    mutable: false,
                }],
                "0.5.0",
                false,
                || false,
            )
            .unwrap();
        }
        fs::write(profile.join(".smb-companion/journal.json"), b"{}").unwrap();
        let i = Instance {
            id: "local:pending".into(),
            directory: profile,
            imported_version: "0.5.0".into(),
            ..Default::default()
        };
        let config = Settings {
            instance_id: i.id.clone(),
            ..Default::default()
        };
        let view = Arc::new(Mutex::new(View::default()));
        let result = publish_discovery(
            &config,
            modrinth::Discovery {
                executable: PathBuf::new(),
                instances: vec![i.clone()],
            },
            &view,
        );
        assert!(result.is_err());
        let v = view.lock().unwrap();
        assert_eq!(v.selected.as_ref().unwrap().id, i.id);
        assert_eq!(v.instances.len(), 1);
        assert!(!v.error.is_empty());
        assert!(!v.busy);
    }
    #[test]
    fn explicit_update_only_action_checks_even_when_automatic_updates_are_off() {
        for (automatic, launch_after_update, expected) in [
            (false, false, true),
            (false, true, false),
            (true, false, true),
            (true, true, true),
        ] {
            let config = Settings {
                automatic,
                launch_after_update,
                ..Default::default()
            };
            assert_eq!(
                should_update(&config, Action::Play),
                expected,
                "automatic={automatic}, launch_after_update={launch_after_update}"
            );
            assert!(should_update(&config, Action::Check));
            assert!(should_update(&config, Action::Repair));
        }
    }
    #[test]
    fn full_channel_archive_adoption_update_and_second_launch_preserve_personal_data() {
        let t = Temp::new();
        let profile = t.0.join("profile");
        fs::write(profile.join("mods/old.jar"), b"old").unwrap();
        fs::write(profile.join("mods/personal.jar"), b"personal").unwrap();
        fs::write(profile.join("config/test.json"), b"personal settings").unwrap();
        let old = archive(
            &t.0,
            "0.5.0",
            "26.3",
            &[
                ("mods/old.jar", b"old"),
                ("config/test.json", b"old default"),
            ],
        );
        let new = archive(
            &t.0,
            "0.13.0",
            "26.3",
            &[
                ("mods/new.jar", b"new"),
                ("config/test.json", b"new default"),
            ],
        );
        let channel = t.0.join("stable.json");
        fs::write(
            &channel,
            serde_json::to_vec(&Channel {
                schema: 1,
                pack: new,
                history: vec![old],
            })
            .unwrap(),
        )
        .unwrap();
        let i = Instance {
            id: "local:fixture".into(),
            directory: profile.clone(),
            imported_version: "0.5.0".into(),
            minecraft: "26.3".into(),
            loader: "fabric".into(),
            loader_version: "0.20.1".into(),
            ..Default::default()
        };
        let source = Source::new(channel.to_string_lossy().into_owned(), t.0.join("cache"));
        let view = Arc::new(Mutex::new(View::default()));
        synchronize(&i, false, &view, &source, || false, || Ok(i.clone())).unwrap();
        assert!(!profile.join("mods/old.jar").exists());
        assert_eq!(fs::read(profile.join("mods/new.jar")).unwrap(), b"new");
        assert_eq!(
            fs::read(profile.join("mods/personal.jar")).unwrap(),
            b"personal"
        );
        assert_eq!(
            fs::read(profile.join("config/test.json")).unwrap(),
            b"personal settings"
        );
        synchronize(&i, false, &view, &source, || false, || Ok(i.clone())).unwrap();
        assert_eq!(view.lock().unwrap().version, "0.13.0");
        let lock = pack::ProfileLock::acquire(&profile).unwrap();
        assert_eq!(
            pack::owned_files(&lock).unwrap()["config/test.json"],
            channel::hash512(b"new default")
        );
    }
    #[test]
    #[ignore = "Read-only archive verification of the prepared local release feed"]
    fn actual_export_archives_match_channel() {
        let p = std::env::var("SMB_MINECRAFT_CHANNEL")
            .expect("Set explicit local candidate stable.json path");
        let temp = Temp::new();
        let source = Source::new(p, temp.0.join("cache"));
        let channel = source.channel().unwrap();
        for r in std::iter::once(&channel.pack).chain(channel.history.iter()) {
            let pack = source.archive(r).unwrap();
            println!(
                "Verified SMB {}: {} client paths, Minecraft {} Fabric {}",
                r.version,
                pack.len(),
                r.minecraft,
                r.loader_version
            );
            assert!(!pack.is_empty());
        }
    }
    #[test]
    #[ignore = "Downloads verified real client mods into disposable staging, never a real instance"]
    fn actual_export_downloads_stage_only() {
        let p = std::env::var("SMB_MINECRAFT_CHANNEL")
            .expect("Set explicit local candidate stable.json path");
        let temp = Temp::new();
        let source = Source::new(p, temp.0.join("cache"));
        let channel = source.channel().unwrap();
        let pack = source.archive(&channel.pack).unwrap();
        let files = source
            .materialize(&pack, &BTreeMap::new(), &temp.0.join("profile"))
            .unwrap();
        println!(
            "Verified real SMB {}: {} staged files, {} bytes; no installed profile changed",
            channel.pack.version,
            files.len(),
            files.iter().map(|f| f.bytes.len()).sum::<usize>()
        );
        assert!(!files.is_empty());
    }
}
