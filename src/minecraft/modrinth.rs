//! Narrow read-only adapter for the verified Modrinth 0.23.1 schema.
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
        let mut q=c.prepare("SELECT i.id,i.path,i.name,l.imported_version_number,s.game_version,s.loader,s.loader_version,coalesce(json_extract(o.overrides,'$.hooks.pre_launch'),'') FROM instances i JOIN instance_content_sets s ON s.id=i.applied_content_set_id JOIN instance_links l ON l.instance_id=i.id LEFT JOIN instance_launch_overrides o ON o.instance_id=i.id WHERE l.link_kind='imported_modpack' AND l.imported_name='SMB'")?;
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
            ))
        })?;
        let mut result = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            let (id, path, name, imported_version, minecraft, loader, loader_version, hook) = row?;
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

pub fn discover() -> Result<Discovery, String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("Windows app folders unavailable")?;
    let executable = PathBuf::from(local).join("Modrinth App/Modrinth App.exe");
    if executable_version(&executable)? != [0, 23, 1, 0] {
        return Err("This build supports Modrinth App 0.23.1. A different version needs adapter verification before updates can run.".into());
    }
    let base = PathBuf::from(std::env::var_os("APPDATA").ok_or("Windows app folders unavailable")?)
        .join("ModrinthApp");
    let instances = read_instances(&base.join("app.db"), &base)?;
    Ok(Discovery {
        executable,
        instances,
    })
}

fn executable_version(path: &Path) -> Result<[u16; 4], String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe {
        let size = GetFileVersionInfoSizeW(wide.as_ptr(), std::ptr::null_mut());
        if size == 0 || size > 1 << 20 {
            return Err("Modrinth App 0.23.1 was not found in its standard install folder".into());
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(wide.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return Err("Could not verify Modrinth App version".into());
        }
        let mut p = std::ptr::null_mut();
        let mut n = 0;
        if VerQueryValueW(data.as_ptr().cast(), [92u16, 0].as_ptr(), &mut p, &mut n) == 0
            || p.is_null()
            || n < (std::mem::size_of::<VS_FIXEDFILEINFO>() as u32)
        {
            return Err("Modrinth App version is unreadable".into());
        }
        let v = std::ptr::read_unaligned(p.cast::<VS_FIXEDFILEINFO>());
        if v.dwSignature != 0xFEEF04BD {
            return Err("Modrinth App version is invalid".into());
        }
        Ok([
            (v.dwFileVersionMS >> 16) as u16,
            v.dwFileVersionMS as u16,
            (v.dwFileVersionLS >> 16) as u16,
            v.dwFileVersionLS as u16,
        ])
    }
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
            "Verified Modrinth 0.23.1: {} SMB profiles",
            found.instances.len()
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
        c.execute_batch("CREATE TABLE instances(id TEXT,path TEXT,applied_content_set_id TEXT,name TEXT);CREATE TABLE instance_content_sets(id TEXT,game_version TEXT,loader TEXT,loader_version TEXT);CREATE TABLE instance_links(instance_id TEXT,link_kind TEXT,imported_name TEXT,imported_version_number TEXT);CREATE TABLE instance_launch_overrides(instance_id TEXT,overrides TEXT);CREATE TABLE settings(hook_pre_launch TEXT,custom_dir TEXT);CREATE TABLE instance_sync_preferences(instance_id TEXT,feature TEXT,enabled INTEGER);CREATE TABLE sync_feature_settings(feature TEXT,globally_enabled INTEGER,new_instance_default INTEGER);INSERT INTO instances VALUES('local:test','smb','set','SMB');INSERT INTO instance_content_sets VALUES('set','26.3','fabric','0.20.1');INSERT INTO instance_links VALUES('local:test','imported_modpack','SMB','0.5.0');INSERT INTO settings VALUES(NULL,NULL);").unwrap();
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
        assert_eq!(before, fs::read(db).unwrap());
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
