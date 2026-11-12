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
    time::{Duration, Instant},
};

/// The public SMB pack feed on the clan site; smb-minecraft's releases publish it.
const CHANNEL: &str = "https://sexymaarbescheiden.nl/minecraft/stable.json";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub instance_id: String,
    pub automatic: bool,
    pub direct_hook: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            instance_id: String::new(),
            automatic: true,
            direct_hook: true,
        }
    }
}
#[derive(Clone, Default)]
pub struct View {
    pub instances: Vec<Instance>,
    pub selected: Option<Instance>,
    pub busy: bool,
    /// What is happening while busy, in a word.
    pub title: String,
    pub version: String,
    pub latest: String,
    pub hook_ready: bool,
    pub error: String,
    /// The pack was handed to Modrinth; waiting for it to show up there.
    pub installing: bool,
    /// Modrinth App is not installed in its standard folder.
    pub modrinth_missing: bool,
    /// Play handed the game to Modrinth at this moment.
    pub launched: Option<Instant>,
    /// Modrinth has been looked at at least once.
    pub discovered: bool,
}

impl View {
    /// The page's state in a word or two. Problems stay under Advanced.
    pub fn headline(&self) -> String {
        if self.busy {
            return if self.title.is_empty() {
                "Checking"
            } else {
                &self.title
            }
            .into();
        }
        let newer = |a: &str, b: &str| match (channel::version(a), channel::version(b)) {
            (Ok(a), Ok(b)) => a > b,
            _ => false,
        };
        match &self.selected {
            _ if self.modrinth_missing => "Get Modrinth",
            Some(i) if !i.ready => "Installing",
            Some(_)
                if self
                    .launched
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(10)) =>
            {
                "Launching"
            }
            Some(_) if self.installing => "Confirm in Modrinth",
            Some(_) if newer(&self.latest, &self.version) => "Update ready",
            Some(_) if self.latest.is_empty() => "Ready",
            Some(_) => "Up to date",
            None if self.installing => "Confirm in Modrinth",
            None if !self.discovered => "Connecting",
            None if !self.error.is_empty() => "Not connected",
            None => "Not installed",
        }
        .into()
    }
}
#[derive(Clone, Copy)]
pub enum Action {
    Discover,
    /// Background look at Modrinth while something is still missing; it never
    /// marks the page busy or replaces the status the player is reading.
    Look,
    Check,
    Play,
    Repair,
    /// Download the latest stable pack and hand it to Modrinth to install.
    Install,
    /// Scheduled check while Minecraft is closed: updates only when the
    /// server needs it, quietly, so Modrinth's own Play button is fine too.
    Background,
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
                ready: true,
                ..Default::default()
            };
            config.instance_id = i.id.clone();
            v.discovered = true;
            v.instances = vec![i.clone()];
            v.selected = Some(i);
            v.version = "0.13.0".into();
            v.latest = "0.13.0".into();
            v.hook_ready = true;
            let args: Vec<_> = std::env::args().collect();
            let state = args
                .iter()
                .position(|s| s == "--minecraft-demo-state")
                .and_then(|n| args.get(n + 1))
                .map(String::as_str);
            if state == Some("modrinth") {
                v.instances.clear();
                v.selected = None;
                v.version.clear();
                v.modrinth_missing = true;
            }
            if state == Some("install") {
                v.instances.clear();
                v.selected = None;
                v.version.clear();
            }
            if state == Some("update") {
                v.latest = "0.14.0".into();
            }
            if state == Some("error") {
                v.hook_ready = false;
                v.error = "Can't reach the SMB pack feed. Nothing was changed.".into();
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
            let schedule = this.tx.clone();
            // Pack checks while Minecraft is closed: shortly after start, every
            // half hour, and the moment Modrinth opens (just before Play).
            std::thread::spawn(move || {
                let mut next = Instant::now() + Duration::from_secs(60);
                let mut modrinth_open = false;
                loop {
                    std::thread::sleep(Duration::from_secs(15));
                    let open = system::process_running("Modrinth App.exe");
                    if (open && !modrinth_open) || Instant::now() >= next {
                        let _ = schedule.try_send(Action::Background);
                        next = Instant::now() + Duration::from_secs(30 * 60);
                    }
                    modrinth_open = open;
                }
            });
            std::thread::spawn(move || {
                let _ = refresh(&settings.lock().unwrap(), &view, false);
                remember_choice(&settings, &view);
                while let Ok(action) = rx.recv() {
                    let config = settings.lock().unwrap().clone();
                    let quiet = matches!(action, Action::Look | Action::Background);
                    if !quiet {
                        let mut v = view.lock().unwrap();
                        v.busy = true;
                        v.error.clear();
                        v.title = "Checking".into();
                    }
                    let result = match action {
                        Action::Discover | Action::Look => refresh(&config, &view, quiet),
                        Action::Install => install(&view),
                        Action::Background => background(&config, &view),
                        _ => perform(&config, action, &view),
                    };
                    {
                        let mut v = view.lock().unwrap();
                        v.busy = false;
                        match (&result, quiet) {
                            (Err(e), true) if matches!(action, Action::Background) => {
                                system::log(&format!("Minecraft pack check skipped: {e}"));
                            }
                            (Err(e), false) => {
                                system::log(&format!("Minecraft: {e}"));
                                v.error = e.clone();
                            }
                            _ => {}
                        }
                    }
                    remember_choice(&settings, &view);
                }
            });
        }
        this
    }
    /// A page over a fixed view, for layout tests.
    #[cfg(test)]
    pub fn with_view(view: View) -> Self {
        let (tx, _) = mpsc::sync_channel(1);
        Self {
            settings: Default::default(),
            view: Arc::new(Mutex::new(view)),
            tx,
            live: false,
        }
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
/// The instance the companion settled on (the newest SMB one, unless the
/// player picked another under Advanced) is remembered for the launch hook.
fn remember_choice(settings: &Arc<Mutex<Settings>>, view: &Arc<Mutex<View>>) {
    let Some(id) = view.lock().unwrap().selected.as_ref().map(|i| i.id.clone()) else {
        return;
    };
    let mut s = settings.lock().unwrap();
    if s.instance_id != id {
        s.instance_id = id;
        if let Err(e) = save(&s) {
            system::log(&format!("Minecraft settings not saved: {e}"));
        }
    }
}

/// The newest SMB pack in Modrinth is the one kept current; among equals,
/// the one used before. A pack for a new Minecraft version arrives as a new
/// instance, so this follows it without asking.
fn choose<'a>(instances: &'a [Instance], id: &str) -> Option<&'a Instance> {
    instances.iter().max_by_key(|i| {
        (
            i.ready,
            channel::version(&i.imported_version).unwrap_or_default(),
            i.id == id,
        )
    })
}

fn refresh(config: &Settings, view: &Arc<Mutex<View>>, quiet: bool) -> Result<(), String> {
    let missing = modrinth::missing();
    {
        let mut v = view.lock().unwrap();
        v.modrinth_missing = missing;
        v.discovered = true;
    }
    if missing {
        let mut v = view.lock().unwrap();
        v.instances.clear();
        v.selected = None;
        return Ok(());
    }
    let d = modrinth::discover().inspect_err(|e| {
        if !quiet {
            view.lock().unwrap().error = e.clone();
        }
    })?;
    publish_discovery(config, d, view, quiet)?;
    if !quiet {
        // Just the small feed, so the page can say whether an update waits.
        let cache = system::data_dir().join("minecraft-cache");
        if let Ok(channel) = Source::new(channel_location(), cache).channel() {
            view.lock().unwrap().latest = channel.pack.version;
        }
    }
    Ok(())
}

/// One click: the latest verified stable pack goes to Modrinth, which
/// installs it as its own instance after its safety prompt. Discovery then
/// picks the new instance up by itself.
fn install(view: &Arc<Mutex<View>>) -> Result<(), String> {
    let executable = modrinth::executable()?;
    view.lock().unwrap().title = "Downloading".into();
    let cache = system::data_dir().join("minecraft-cache");
    let source = Source::new(channel_location(), cache.clone());
    let channel = source.channel()?;
    let verified = source.archive_path(&channel.pack)?;
    // Modrinth shows this file name in its prompt.
    let named = cache
        .join("install")
        .join(format!("SMB {}.mrpack", channel.pack.version));
    fs::create_dir_all(named.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::copy(&verified, &named).map_err(|_| "Could not prepare the pack for Modrinth")?;
    modrinth::install(&executable, &named)?;
    system::log(&format!(
        "Handed SMB pack {} to Modrinth to install.",
        channel.pack.version
    ));
    let mut v = view.lock().unwrap();
    v.installing = true;
    v.latest = channel.pack.version.clone();
    Ok(())
}

/// Scheduled, quiet: only runs with updates on, Minecraft closed and the
/// selected instance installed, and only writes when the server needs it.
fn background(config: &Settings, view: &Arc<Mutex<View>>) -> Result<(), String> {
    if !config.automatic || system::minecraft_running() {
        return Ok(());
    }
    let (_, i) = selected(config)?;
    if !i.ready {
        return Ok(());
    }
    update_profile(config, &i, false, true, view)
}

fn channel_location() -> String {
    std::env::var("SMB_MINECRAFT_CHANNEL").unwrap_or_else(|_| CHANNEL.into())
}
fn publish_discovery(
    config: &Settings,
    d: modrinth::Discovery,
    view: &Arc<Mutex<View>>,
    quiet: bool,
) -> Result<(), String> {
    let selected = choose(&d.instances, &config.instance_id).cloned();
    let hook_ready = |i: &Instance| {
        std::env::current_exe()
            .ok()
            .and_then(|p| modrinth::hook_command(&p).ok())
            .is_some_and(|h| h == i.pre_launch)
    };
    {
        // A background look with the same instance only refreshes what it
        // found; the status and any problem on screen stay as they are.
        let mut v = view.lock().unwrap();
        let same = v.selected.as_ref().map(|i| (&i.id, i.ready))
            == selected.as_ref().map(|i| (&i.id, i.ready));
        if quiet && same && selected.is_some() {
            v.hook_ready = selected.as_ref().is_some_and(hook_ready);
            v.instances = d.instances;
            v.selected = selected;
            return Ok(());
        }
    }
    let managed_result = (|| -> Result<Option<String>, String> {
        Ok(if let Some(i) = &selected {
            if i.directory.join(".smb-companion/state.json").exists() {
                let lock = pack::ProfileLock::acquire(&i.directory)?;
                if pack::has_pending(&i.directory)? {
                    // Finished safely by the next update (Play or the
                    // background check); nothing for the player to do.
                    return Ok(None);
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
    v.discovered = true;
    v.instances = d.instances;
    v.selected = selected.clone();
    v.error.clear();
    if !v.instances.is_empty() {
        v.installing = false;
    }
    v.hook_ready = selected.as_ref().is_some_and(hook_ready);
    // Discovery remains available even when validation requires recovery. The
    // player must be able to select this instance and invoke Check/Repair.
    let managed_version = managed_result.unwrap_or_else(|e| {
        v.error = e;
        None
    });
    if let Some(i) = selected {
        v.version = managed_version.unwrap_or(i.imported_version);
    } else if !v.installing {
        v.version.clear();
    }
    Ok(())
}
fn selected(config: &Settings) -> Result<(PathBuf, Instance), String> {
    let d = modrinth::discover()?;
    let i = choose(&d.instances, &config.instance_id)
        .cloned()
        .ok_or("No SMB instance in Modrinth yet")?;
    Ok((d.executable, i))
}
fn target<'a>(
    channel: &'a Channel,
    installed: &str,
    explicit: bool,
) -> Result<Option<&'a PackRef>, String> {
    // Always the latest stable pack; Repair re-applies it. Never a downgrade.
    let installed = channel::version(installed)?;
    let latest = channel::version(&channel.pack.version)?;
    if installed > latest || (!explicit && installed == latest) {
        Ok(None)
    } else {
        Ok(Some(&channel.pack))
    }
}
/// Start of the error for a pack on a newer Minecraft or loader: Modrinth
/// has to install that one as a new instance (Play does it).
const NEW_RUNTIME: &str = "New Minecraft version";
fn runtime(i: &Instance, r: &PackRef) -> Result<(), String> {
    if i.minecraft != r.minecraft || i.loader != r.loader || i.loader_version != r.loader_version {
        return Err(format!(
            "{NEW_RUNTIME}: SMB {} needs Minecraft {} with {} {}; Play installs it in Modrinth",
            r.version, r.minecraft, r.loader, r.loader_version
        ));
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
    only_if_needed: bool,
    view: &Arc<Mutex<View>>,
) -> Result<(), String> {
    let _operation = Operation::acquire()?;
    if system::minecraft_running() {
        return Err(
            "Minecraft or another Java app is running. Close it before checking the pack.".into(),
        );
    }
    let source = Source::new(
        channel_location(),
        system::data_dir().join("minecraft-cache"),
    );
    synchronize(
        i,
        explicit,
        only_if_needed,
        view,
        &source,
        system::minecraft_running,
        || selected(config).map(|(_, i)| i),
    )
}
fn synchronize(
    i: &Instance,
    explicit: bool,
    only_if_needed: bool,
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
    // Scheduled checks leave a pack the server still accepts exactly as it is.
    if only_if_needed && wanted.is_none() {
        return Ok(());
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
        v.busy = true;
        v.title = "Updating".into();
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
    if report.written + report.removed > 0 {
        system::log(&format!(
            "Minecraft pack updated to {} ({} written, {} removed, {} kept as the player left them).",
            chosen.version, report.written, report.removed, report.preserved
        ));
    }
    view.lock().unwrap().version = chosen.version.clone();
    Ok(())
}
fn perform(config: &Settings, action: Action, view: &Arc<Mutex<View>>) -> Result<(), String> {
    let (executable, i) = selected(config)?;
    if !i.ready {
        return Err("Modrinth is still installing SMB".into());
    }
    if !matches!(action, Action::Play) {
        return update_profile(config, &i, matches!(action, Action::Repair), false, view);
    }
    // Play: bring the pack up to date, then hand over to Modrinth. An update
    // that can't run now never stands between the player and the game.
    if config.automatic {
        match update_profile(config, &i, false, false, view) {
            Ok(()) => {}
            Err(e) if e.starts_with(NEW_RUNTIME) => return install(view),
            Err(e) => {
                system::log(&format!("Minecraft pack not updated before play: {e}"));
                view.lock().unwrap().error = e;
            }
        }
    }
    let (_, fresh) = selected(config)?;
    if fresh.directory != i.directory {
        return Err("The SMB instance in Modrinth changed; press Play again".into());
    }
    ensure_consistent(&fresh.directory)?;
    modrinth::handoff(&executable, &i.id)?;
    view.lock().unwrap().launched = Some(Instant::now());
    Ok(())
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
    update_profile(
        &config,
        &i,
        false,
        false,
        &Arc::new(Mutex::new(View::default())),
    )
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
    fn always_the_latest_pack_and_never_a_downgrade() {
        let c = Channel {
            schema: 1,
            pack: reference("0.13.0", "0.11.0"),
            history: vec![],
        };
        assert_eq!(
            target(&c, "0.11.0", false).unwrap().unwrap().version,
            "0.13.0"
        );
        assert!(target(&c, "0.13.0", false).unwrap().is_none());
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
        assert!(runtime(&i, &r).unwrap_err().starts_with(NEW_RUNTIME));
    }
    #[test]
    fn the_newest_smb_instance_is_kept_current() {
        let at = |id: &str, version: &str, ready: bool| Instance {
            id: id.into(),
            imported_version: version.into(),
            ready,
            ..Default::default()
        };
        let found = [
            at("local:old", "0.5.0", true),
            at("local:main", "0.17.0", true),
            at("local:copy", "0.17.0", true),
            at("local:installing", "0.18.0", false),
        ];
        assert_eq!(choose(&found, "").unwrap().id, "local:copy");
        assert_eq!(choose(&found, "local:main").unwrap().id, "local:main");
        // An older pick gives way to a newer pack (a new Minecraft version).
        assert_eq!(choose(&found, "local:old").unwrap().id, "local:copy");
        assert!(choose(&[], "local:main").is_none());
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
            false,
        );
        // The next update finishes it; nothing is put in front of the player.
        assert!(result.is_ok());
        let v = view.lock().unwrap();
        assert_eq!(v.selected.as_ref().unwrap().id, i.id);
        assert_eq!(v.instances.len(), 1);
        assert!(v.error.is_empty());
        assert!(!v.busy);
    }
    #[test]
    fn background_look_keeps_the_status_and_problem_on_screen() {
        let i = Instance {
            id: "local:smb".into(),
            imported_version: "0.13.0".into(),
            pre_launch: "hook pasted since".into(),
            ready: true,
            ..Default::default()
        };
        let config = Settings {
            instance_id: i.id.clone(),
            ..Default::default()
        };
        let view = Arc::new(Mutex::new(View {
            selected: Some(Instance {
                pre_launch: String::new(),
                ..i.clone()
            }),
            title: "Checking".into(),
            error: "Java is running".into(),
            ..Default::default()
        }));
        let found = || modrinth::Discovery {
            executable: PathBuf::new(),
            instances: vec![
                i.clone(),
                Instance {
                    id: "local:other".into(),
                    ..i.clone()
                },
            ],
        };
        publish_discovery(&config, found(), &view, true).unwrap();
        {
            let v = view.lock().unwrap();
            assert_eq!(v.error, "Java is running");
            assert_eq!(v.instances.len(), 2);
            assert_eq!(v.selected.as_ref().unwrap().pre_launch, "hook pasted since");
            assert!(!v.busy);
        }
        // An explicit look does refresh the status.
        publish_discovery(&config, found(), &view, false).unwrap();
        let v = view.lock().unwrap();
        assert!(v.error.is_empty());
        assert_eq!(v.version, "0.13.0");
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
        synchronize(&i, false, false, &view, &source, || false, || Ok(i.clone())).unwrap();
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
        synchronize(&i, false, false, &view, &source, || false, || Ok(i.clone())).unwrap();
        assert_eq!(view.lock().unwrap().version, "0.13.0");
        let lock = pack::ProfileLock::acquire(&profile).unwrap();
        assert_eq!(
            pack::owned_files(&lock).unwrap()["config/test.json"],
            channel::hash512(b"new default")
        );
    }
    fn fixture_channel(t: &Temp) -> String {
        let old = archive(&t.0, "0.5.0", "26.3", &[("mods/old.jar", b"old")]);
        let mid = archive(&t.0, "0.11.0", "26.3", &[("mods/mid.jar", b"mid")]);
        let new = archive(&t.0, "0.13.0", "26.3", &[("mods/new.jar", b"new")]);
        let channel = t.0.join("stable.json");
        fs::write(
            &channel,
            serde_json::to_vec(&Channel {
                schema: 1,
                pack: new,
                history: vec![mid, old],
            })
            .unwrap(),
        )
        .unwrap();
        channel.to_string_lossy().into_owned()
    }
    fn fixture_instance(profile: &Path, version: &str) -> Instance {
        Instance {
            id: "local:fixture".into(),
            directory: profile.to_owned(),
            imported_version: version.into(),
            minecraft: "26.3".into(),
            loader: "fabric".into(),
            loader_version: "0.20.1".into(),
            ready: true,
            ..Default::default()
        }
    }
    #[test]
    fn scheduled_check_brings_a_compatible_pack_to_the_latest_release() {
        let t = Temp::new();
        let profile = t.0.join("profile");
        fs::write(profile.join("mods/mid.jar"), b"mid").unwrap();
        let i = fixture_instance(&profile, "0.11.0");
        let source = Source::new(fixture_channel(&t), t.0.join("cache"));
        let view = Arc::new(Mutex::new(View::default()));
        synchronize(&i, false, true, &view, &source, || false, || Ok(i.clone())).unwrap();
        assert_eq!(fs::read(profile.join("mods/new.jar")).unwrap(), b"new");
        assert!(!profile.join("mods/mid.jar").exists());
        assert_eq!(view.lock().unwrap().version, "0.13.0");
        // Already the latest: a scheduled check writes nothing at all.
        let state = fs::read(profile.join(".smb-companion/state.json")).unwrap();
        let latest = fixture_instance(&profile, "0.13.0");
        synchronize(
            &latest,
            false,
            true,
            &view,
            &source,
            || false,
            || Ok(latest.clone()),
        )
        .unwrap();
        assert_eq!(
            fs::read(profile.join(".smb-companion/state.json")).unwrap(),
            state
        );
    }
    #[test]
    fn scheduled_check_updates_when_the_server_needs_a_newer_pack() {
        let t = Temp::new();
        let profile = t.0.join("profile");
        fs::write(profile.join("mods/old.jar"), b"old").unwrap();
        fs::write(profile.join("mods/personal.jar"), b"personal").unwrap();
        let i = fixture_instance(&profile, "0.5.0");
        let source = Source::new(fixture_channel(&t), t.0.join("cache"));
        let view = Arc::new(Mutex::new(View::default()));
        synchronize(&i, false, true, &view, &source, || false, || Ok(i.clone())).unwrap();
        assert_eq!(fs::read(profile.join("mods/new.jar")).unwrap(), b"new");
        assert!(!profile.join("mods/old.jar").exists());
        assert_eq!(
            fs::read(profile.join("mods/personal.jar")).unwrap(),
            b"personal"
        );
        assert_eq!(view.lock().unwrap().version, "0.13.0");
    }
    #[test]
    fn install_states_read_as_one_short_line() {
        let config = Settings::default();
        let view = Arc::new(Mutex::new(View {
            installing: true,
            ..Default::default()
        }));
        let found = |instances| modrinth::Discovery {
            executable: PathBuf::new(),
            instances,
        };
        // Handed to Modrinth, nothing there yet: confirm there.
        publish_discovery(&config, found(vec![]), &view, true).unwrap();
        assert_eq!(view.lock().unwrap().headline(), "Confirm in Modrinth");
        // Modrinth is still installing the pack it was given.
        let mut i = Instance {
            id: "local:new".into(),
            imported_version: "0.13.0".into(),
            ..Default::default()
        };
        let config = Settings {
            instance_id: i.id.clone(),
            ..Default::default()
        };
        publish_discovery(&config, found(vec![i.clone()]), &view, true).unwrap();
        {
            let v = view.lock().unwrap();
            assert_eq!(v.headline(), "Installing");
            assert!(!v.installing);
        }
        // Done: a quiet look notices and the card turns ready.
        i.ready = true;
        publish_discovery(&config, found(vec![i]), &view, true).unwrap();
        assert_eq!(view.lock().unwrap().headline(), "Ready");
        {
            let mut v = view.lock().unwrap();
            v.version = "0.13.0".into();
            v.latest = "0.14.0".into();
            assert_eq!(v.headline(), "Update ready");
            v.latest = "0.13.0".into();
            assert_eq!(v.headline(), "Up to date");
            v.launched = Some(Instant::now());
            assert_eq!(v.headline(), "Launching");
        }
        // Nothing found and nothing handed over: offer the install.
        let view = Arc::new(Mutex::new(View::default()));
        assert_eq!(view.lock().unwrap().headline(), "Connecting");
        publish_discovery(&Settings::default(), found(vec![]), &view, false).unwrap();
        assert_eq!(view.lock().unwrap().headline(), "Not installed");
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
#[cfg(test)]
mod live_tests {
    use super::*;
    /// SMB_PROFILE_COPY: a disposable copy of a real SMB profile's pack folders.
    #[test]
    #[ignore = "Updates a disposable copy of a real profile from the live feed"]
    fn real_profile_copy_updates_from_the_live_feed() {
        let copy = PathBuf::from(std::env::var("SMB_PROFILE_COPY").expect("SMB_PROFILE_COPY"))
            .canonicalize()
            .unwrap();
        let found = modrinth::discover().unwrap();
        let real = choose(&found.instances, "").unwrap().clone();
        assert_ne!(real.directory, copy, "never the real profile");
        let i = Instance {
            directory: copy.clone(),
            ..real
        };
        let source = Source::new(
            CHANNEL.into(),
            PathBuf::from(std::env::var("APPDATA").unwrap())
                .join("SMB/LoL Companion/minecraft-cache"),
        );
        let view = Arc::new(Mutex::new(View::default()));
        for run in 1..=2 {
            synchronize(&i, false, false, &view, &source, || false, || Ok(i.clone())).unwrap();
            let v = view.lock().unwrap();
            println!("run {run}: {} (latest {})", v.version, v.latest);
            assert_eq!(v.version, v.latest);
        }
    }
}
