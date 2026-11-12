//! Narrow read-only adapter for Modrinth App's profile database (0.23 schema).
//! No version pin: Modrinth updates itself, so the schema query is the check.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct Instance {
    pub id: String,
    pub name: String,
    pub directory: PathBuf,
    pub imported_version: String,
    pub minecraft: String,
    pub loader: String,
    pub loader_version: String,
    pub pre_launch: String,
    pub global_pre_launch: bool,
    pub sync_resources: bool,
    pub sync_options: bool,
    pub sync_servers: bool,
    /// Modrinth finished installing the pack files (its own install stage).
    pub ready: bool,
}
#[derive(Clone, Debug)]
pub struct Discovery {
    pub executable: PathBuf,
    pub instances: Vec<Instance>,
}
pub fn read_instances(db: &Path, base: &Path) -> Result<Vec<Instance>, String> {
    use rusqlite::{Connection, OpenFlags, OptionalExtension};
    let read = || -> rusqlite::Result<Vec<Instance>> {
        let c = Connection::open_with_flags(
            db,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        c.busy_timeout(std::time::Duration::from_secs(3))?;
        let global: String = c
            .query_row(
                "SELECT coalesce(hook_pre_launch,'') FROM settings LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default();
        let custom: Option<String> = c
            .query_row("SELECT custom_dir FROM settings LIMIT 1", [], |r| r.get(0))
            .optional()?
            .flatten();
        let root = custom
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| base.to_owned())
            .join("profiles");
        let root =
            std::fs::canonicalize(root).map_err(|_| rusqlite::Error::InvalidPath(base.into()))?;
        let mut q=c.prepare("SELECT i.id,i.path,i.name,l.imported_version_number,s.game_version,s.loader,s.loader_version,coalesce(json_extract(o.overrides,'$.hooks.pre_launch'),''),coalesce(i.install_stage,'') FROM instances i JOIN instance_content_sets s ON s.id=i.applied_content_set_id JOIN instance_links l ON l.instance_id=i.id LEFT JOIN instance_launch_overrides o ON o.instance_id=i.id WHERE l.link_kind='imported_modpack' AND l.imported_name='SMB'")?;
        let rows = q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
            ))
        })?;
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            let (id, path, name, imported_version, minecraft, loader, loader_version, hook, stage) =
                row?;
            // Native paths are a single relative folder name. Never follow a profile junction.
            if path.is_empty() || path.contains(['/', '\\', ':']) || path == "." || path == ".." {
                return Err(rusqlite::Error::InvalidPath(path.into()));
            }
            let raw = root.join(&path);
            let metadata = std::fs::symlink_metadata(&raw)
                .map_err(|_| rusqlite::Error::InvalidPath(raw.clone()))?;
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Err(rusqlite::Error::InvalidPath(raw));
            }
            let directory = std::fs::canonicalize(&raw)
                .map_err(|_| rusqlite::Error::InvalidPath(raw.clone()))?;
            if directory.parent() != Some(root.as_path())
                || !directory.is_dir()
                || !seen.insert(directory.clone())
            {
                return Err(rusqlite::Error::InvalidPath(raw));
            }
            let sync = |feature: &str| -> rusqlite::Result<bool> {
                c.query_row("SELECT globally_enabled AND coalesce((SELECT enabled FROM instance_sync_preferences WHERE instance_id=?1 AND feature=?2),new_instance_default) FROM sync_feature_settings WHERE feature=?2",rusqlite::params![id,feature],|r|r.get(0)).optional().map(|v|v.unwrap_or(false))
            };
            let global_pre_launch = hook.is_empty() && !global.is_empty();
            let (sync_resources, sync_options, sync_servers) = (
                sync("resource_packs")?,
                sync("game_options")?,
                sync("multiplayer_servers")?,
            );
            result.push(Instance {
                id,
                name,
                directory,
                imported_version,
                minecraft,
                loader,
                loader_version,
                pre_launch: if hook.is_empty() {
                    global.clone()
                } else {
                    hook
                },
                global_pre_launch,
                sync_resources,
                sync_options,
                sync_servers,
                // Pack files are in place; Minecraft itself may still follow.
                ready: matches!(stage.as_str(), "installed" | "pack_installed"),
            });
        }
        Ok(result)
    };
    read().map_err(|e| format!("Could not read the supported Modrinth profile schema: {e}"))
}
pub fn launch_url(id: &str) -> Result<String, String> {
    if !id.starts_with("local:")
        || id.len() > 200
        || id.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err("Invalid native instance ID".into());
    }
    Ok(format!(
        "modrinth://launch/instance/{}",
        url::form_urlencoded::byte_serialize(id.as_bytes()).collect::<String>()
    ))
}
pub fn hook_command(exe: &Path) -> Result<String, String> {
    let p = exe
        .to_str()
        .ok_or("The app path is not valid Unicode")?
        .replace('\\', "/");
    if p.contains(['"', '$', '\n', '\r']) || !exe.is_absolute() {
        return Err(
            "Move the app to a path without quotes or launcher variables before setting the hook"
                .into(),
        );
    }
    Ok(format!("\"{p}\" --minecraft-prelaunch"))
}

fn executable_path() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("Windows app folders unavailable")?;
    Ok(PathBuf::from(local).join("Modrinth App/Modrinth App.exe"))
}

/// Modrinth App is not installed in its standard per-user folder.
pub fn missing() -> bool {
    executable_path().map(|p| !p.is_file()).unwrap_or(true)
}

/// Modrinth App's executable. Its version is not pinned: Modrinth updates
/// itself, and the profile query in `read_instances` fails closed on a schema
/// it does not recognise.
pub fn executable() -> Result<PathBuf, String> {
    let executable = executable_path()?;
    if !executable.is_file() {
        return Err("Modrinth App is not installed".into());
    }
    Ok(executable)
}

fn base() -> Result<PathBuf, String> {
    Ok(
        PathBuf::from(std::env::var_os("APPDATA").ok_or("Windows app folders unavailable")?)
            .join("ModrinthApp"),
    )
}

/// Minecraft started by Modrinth is running. Modrinth records the games it
/// starts; its own Java runtimes are checked too, in case a record is missing.
/// When Modrinth's database cannot be read, any Java process counts.
pub fn game_running() -> bool {
    let javas = crate::system::java_processes();
    let Ok(base) = base() else {
        return !javas.is_empty();
    };
    match running_in(&base.join("app.db"), &base, &javas) {
        Ok(running) => running,
        Err(_) => !javas.is_empty(),
    }
}

fn running_in(db: &Path, base: &Path, javas: &[(u32, Option<PathBuf>)]) -> Result<bool, String> {
    use rusqlite::{Connection, OpenFlags};
    if !db.is_file() {
        return Err("no Modrinth database".into());
    }
    let c = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())?;
    c.busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    let recorded: Vec<u32> = c
        .prepare("SELECT pid FROM processes")
        .and_then(|mut q| {
            q.query_map([], |r| r.get::<_, i64>(0))?
                .map(|r| r.map(|p| p as u32))
                .collect()
        })
        .map_err(|e| e.to_string())?;
    let runtimes: Vec<PathBuf> = c
        .prepare("SELECT path FROM java_versions")
        .and_then(|mut q| {
            q.query_map([], |r| r.get::<_, String>(0))?
                .map(|r| r.map(PathBuf::from))
                .collect()
        })
        .unwrap_or_default();
    let lower = |p: &Path| p.to_string_lossy().to_lowercase().replace('/', "\\");
    let base = lower(base);
    let runtimes: Vec<String> = runtimes.iter().map(|p| lower(p)).collect();
    Ok(javas.iter().any(|(pid, path)| match path {
        // A Java process Windows won't describe: assume it might be the game.
        None => true,
        Some(path) => {
            let path = lower(path);
            recorded.contains(pid) || path.starts_with(&base) || runtimes.contains(&path)
        }
    }))
}

/// Hands a verified .mrpack to Modrinth, exactly like opening the file:
/// Modrinth installs it as a native instance after its own safety prompt.
pub fn install(executable: &Path, mrpack: &Path) -> Result<(), String> {
    if mrpack.extension().is_none_or(|e| e != "mrpack") || !mrpack.is_absolute() {
        return Err("Only a local .mrpack file can be handed to Modrinth".into());
    }
    std::process::Command::new(executable)
        .arg(mrpack)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open Modrinth: {e}"))
}

pub fn discover() -> Result<Discovery, String> {
    let executable = executable()?;
    let base = base()?;
    let instances = read_instances(&base.join("app.db"), &base)?;
    Ok(Discovery {
        executable,
        instances,
    })
}

pub fn handoff(executable: &Path, id: &str) -> Result<(), String> {
    std::process::Command::new(executable)
        .arg(launch_url(id)?)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not hand launch to Modrinth: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };
    #[test]
    #[ignore = "Read-only check of this machine's installed launcher"]
    fn native_discovery_readonly() {
        let found = discover().unwrap();
        println!(
            "Modrinth: {} SMB profiles, game running: {}",
            found.instances.len(),
            game_running()
        );
        assert!(!found.instances.is_empty());
        for i in found.instances {
            println!(
                "{} / {} / {} Fabric {}",
                i.id, i.imported_version, i.minecraft, i.loader_version
            );
        }
    }
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "smb-launcher-test-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(p.join("profiles/smb")).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn database(t: &Temp) -> PathBuf {
        let p = t.0.join("app.db");
        let c = Connection::open(&p).unwrap();
        c.execute_batch("CREATE TABLE instances(id TEXT,path TEXT,applied_content_set_id TEXT,name TEXT,install_stage TEXT);CREATE TABLE instance_content_sets(id TEXT,game_version TEXT,loader TEXT,loader_version TEXT);CREATE TABLE instance_links(instance_id TEXT,link_kind TEXT,imported_name TEXT,imported_version_number TEXT);CREATE TABLE instance_launch_overrides(instance_id TEXT,overrides TEXT);CREATE TABLE settings(hook_pre_launch TEXT,custom_dir TEXT);CREATE TABLE instance_sync_preferences(instance_id TEXT,feature TEXT,enabled INTEGER);CREATE TABLE sync_feature_settings(feature TEXT,globally_enabled INTEGER,new_instance_default INTEGER);INSERT INTO instances VALUES('local:test','smb','set','SMB','installed');INSERT INTO instance_content_sets VALUES('set','26.3','fabric','0.20.1');INSERT INTO instance_links VALUES('local:test','imported_modpack','SMB','0.5.0');INSERT INTO settings VALUES(NULL,NULL);").unwrap();
        p
    }
    #[test]
    fn discovers_canonical_profile_and_runtime_without_writing_database() {
        let t = Temp::new();
        let db = database(&t);
        let before = fs::read(&db).unwrap();
        let i = read_instances(&db, &t.0).unwrap();
        assert_eq!(i.len(), 1);
        assert_eq!(i[0].id, "local:test");
        assert_eq!(i[0].imported_version, "0.5.0");
        assert_eq!(i[0].loader_version, "0.20.1");
        assert_eq!(
            i[0].directory,
            fs::canonicalize(t.0.join("profiles/smb")).unwrap()
        );
        assert!(i[0].ready);
        assert_eq!(before, fs::read(db).unwrap());
    }
    #[test]
    fn only_minecraft_from_modrinth_holds_pack_updates_back() {
        let t = Temp::new();
        let db = t.0.join("app.db");
        let c = Connection::open(&db).unwrap();
        c.execute_batch(r"CREATE TABLE processes(pid INTEGER);CREATE TABLE java_versions(path TEXT);INSERT INTO java_versions VALUES('C:\Java\zulu21\bin\javaw.exe');").unwrap();
        let java = |pid: u32, path: &str| (pid, Some(PathBuf::from(path)));
        let base = Path::new("C:/Users/P/AppData/Roaming/ModrinthApp");
        // No Java, or only an IDE's Java: free to update.
        assert!(!running_in(&db, base, &[]).unwrap());
        let ide = [java(10, "C:/Program Files/JetBrains/jbr/bin/java.exe")];
        assert!(!running_in(&db, base, &ide).unwrap());
        // Modrinth's own runtime, a configured runtime, or a recorded game: wait.
        let bundled = [java(
            11,
            "C:/Users/P/AppData/Roaming/ModrinthApp/meta/java_versions/zulu/bin/javaw.exe",
        )];
        assert!(running_in(&db, base, &bundled).unwrap());
        assert!(running_in(&db, base, &[java(12, "c:/java/ZULU21/bin/javaw.exe")]).unwrap());
        c.execute("INSERT INTO processes VALUES(10)", []).unwrap();
        assert!(running_in(&db, base, &ide).unwrap());
        // A Java process Windows won't describe might be the game.
        assert!(!running_in(&db, base, &[]).unwrap());
        assert!(running_in(&db, base, &[(13, None)]).unwrap());
        // No readable database: not trusted.
        assert!(running_in(&t.0.join("missing.db"), base, &ide).is_err());
    }
    #[test]
    fn an_instance_modrinth_is_still_installing_is_found_but_not_ready() {
        let t = Temp::new();
        let db = database(&t);
        let c = Connection::open(&db).unwrap();
        for (stage, ready) in [
            ("pack_installing", false),
            ("not_installed", false),
            ("minecraft_installing", false),
            ("pack_installed", true),
            ("installed", true),
        ] {
            c.execute("UPDATE instances SET install_stage=?1", [stage])
                .unwrap();
            let i = read_instances(&db, &t.0).unwrap();
            assert_eq!(i.len(), 1);
            assert_eq!(i[0].ready, ready, "{stage}");
        }
    }
    #[test]
    fn only_local_mrpack_files_are_handed_to_modrinth() {
        let exe = Path::new("C:/missing/Modrinth App.exe");
        assert!(install(exe, Path::new("relative/SMB.mrpack")).is_err());
        assert!(install(exe, Path::new("C:/packs/SMB.zip")).is_err());
        assert!(install(exe, Path::new("modrinth://launch/instance/x")).is_err());
    }
    #[test]
    fn encodes_the_native_instance_id_and_quotes_executable_without_a_shell() {
        assert_eq!(
            launch_url("local:test-id").unwrap(),
            "modrinth://launch/instance/local%3Atest-id"
        );
        let h = hook_command(Path::new("C:/Program Files/SMB/smb-lol.exe")).unwrap();
        assert_eq!(
            h,
            "\"C:/Program Files/SMB/smb-lol.exe\" --minecraft-prelaunch"
        );
        assert!(hook_command(Path::new("C:/bad\"path/app.exe")).is_err());
        assert!(launch_url("local:bad\n").is_err());
    }
    #[test]
    fn rejects_instance_paths_outside_profiles_and_non_imported_links() {
        let t = Temp::new();
        let db = database(&t);
        let c = Connection::open(&db).unwrap();
        c.execute("UPDATE instances SET path='../'", []).unwrap();
        assert!(read_instances(&db, &t.0).is_err());
        c.execute("UPDATE instances SET path='smb'", []).unwrap();
        c.execute("UPDATE instance_links SET link_kind='modrinth'", [])
            .unwrap();
        assert!(read_instances(&db, &t.0).unwrap().is_empty());
    }
    #[test]
    fn reads_effective_hook_and_native_sync_flags() {
        let t = Temp::new();
        let db = database(&t);
        let c = Connection::open(&db).unwrap();
        c.execute(
            "UPDATE settings SET hook_pre_launch='existing custom hook'",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO sync_feature_settings VALUES('resource_packs',1,1)",
            [],
        )
        .unwrap();
        let i = read_instances(&db, &t.0).unwrap();
        assert!(i[0].global_pre_launch);
        assert_eq!(i[0].pre_launch, "existing custom hook");
        assert!(i[0].sync_resources);
    }
}
