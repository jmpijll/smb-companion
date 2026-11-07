use crate::client::{self, Api, Asset, Connection, GameData};
use crate::system;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    pub enabled: bool,
    pub runes: bool,
    pub spells: bool,
    pub flash: String,
    pub folder: String,
    pub revision: u64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            runes: true,
            spells: true,
            flash: "f".into(),
            folder: String::new(),
            revision: 0,
        }
    }
}
pub fn preferences_path() -> PathBuf {
    system::data_dir().join("preferences.json")
}
impl Settings {
    pub fn load() -> Self {
        let mut s = Self::default();
        if let Some(v) = fs::read(preferences_path())
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        {
            s.enabled = v["enabled"].as_bool().unwrap_or(true);
            s.runes = v["runes"].as_bool().unwrap_or(true);
            s.spells = v["spells"].as_bool().unwrap_or(true);
            s.flash = if v["flash"] == "d" { "d" } else { "f" }.into();
            s.folder = v["folder"].as_str().unwrap_or("").into();
        }
        s
    }
    pub fn save(&self) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&json!({
            "enabled": self.enabled,
            "runes": self.runes,
            "spells": self.spells,
            "flash": self.flash,
            "folder": self.folder,
        }))
        .map_err(|e| e.to_string())?;
        let path = preferences_path();
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        fs::write(path, bytes).map_err(|_| "Your settings could not be saved.".into())
    }
}

/// What the companion applied (or is applying) for the current pick, using
/// the client's own names and icon paths.
#[derive(Clone, Default, PartialEq, Debug)]
pub struct Setup {
    pub champion: String,
    pub champion_icon: String,
    /// top / jungle / mid / adc / support, or empty for role-less modes.
    pub lane: String,
    /// "ARAM", "Arena", ... for modes without lanes.
    pub mode: String,
    pub keystone: Option<Asset>,
    pub primary: Option<Asset>,
    pub secondary: Option<Asset>,
    /// Spell on D, spell on F.
    pub spells: [Option<Asset>; 2],
    pub runes_applied: bool,
    pub spells_applied: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Phase {
    #[default]
    Offline,
    Idle,
    ChampSelect,
    InGame,
    Paused,
}

#[derive(Clone)]
pub struct Status {
    pub phase: Phase,
    pub busy: bool,
    pub title: String,
    pub detail: String,
    pub rune: String,
    pub spell: String,
    pub activity: String,
    pub error: String,
    pub last_applied: String,
    pub setup: Option<Setup>,
    pub flash_icon: Option<Asset>,
    pub stop: bool,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            phase: Phase::Offline,
            busy: false,
            title: "Waiting for League".into(),
            detail: "Open the client. We'll take it from here.".into(),
            rune: String::new(),
            spell: String::new(),
            activity: "Standing by".into(),
            error: String::new(),
            last_applied: String::new(),
            setup: None,
            flash_icon: None,
            stop: false,
        }
    }
}
pub type SharedSettings = Arc<Mutex<Settings>>;
pub type SharedStatus = Arc<Mutex<Status>>;
/// Icon bytes by client path. `None` marks a fetch that failed (not retried).
pub type Icons = Arc<Mutex<HashMap<String, Option<Arc<[u8]>>>>>;

const FLASH: u64 = 4;

fn fetch_icons(api: &Connection, icons: &Icons, paths: &[&str]) {
    for path in paths {
        if path.is_empty() || icons.lock().unwrap().contains_key(*path) {
            continue;
        }
        let bytes = api.bytes(path).ok().map(Arc::<[u8]>::from);
        icons.lock().unwrap().insert((*path).to_owned(), bytes);
    }
}

fn setup_icons(setup: &Setup) -> Vec<&str> {
    let mut v = vec![setup.champion_icon.as_str()];
    for a in [&setup.keystone, &setup.primary, &setup.secondary]
        .into_iter()
        .chain(setup.spells.iter())
        .flatten()
    {
        v.push(a.icon.as_str());
    }
    v
}

fn mode_label(mode: &str) -> &'static str {
    match mode {
        "aram" => "ARAM",
        "aram_mayhem_classic" => "ARAM Mayhem",
        "urf" => "URF",
        "arena" => "Arena",
        _ => "",
    }
}

fn spell_assets(meta: &GameData, pair: &Value) -> [Option<Asset>; 2] {
    let get = |k: &str| {
        pair[k]
            .as_u64()
            .and_then(|id| meta.spells.get(&id).cloned())
    };
    [get("spell1Id"), get("spell2Id")]
}

fn rune_assets(setup: &mut Setup, meta: &GameData, page: &Value) {
    let id = |v: &Value| v.as_u64().unwrap_or(0);
    setup.primary = meta.styles.get(&id(&page["primaryStyleId"])).cloned();
    setup.secondary = meta.styles.get(&id(&page["subStyleId"])).cloned();
    setup.keystone = meta.perks.get(&id(&page["selectedPerkIds"][0])).cloned();
}

pub fn start(settings: SharedSettings, status: SharedStatus, icons: Icons) {
    thread::spawn(move || {
        let mut connection: Option<Connection> = None;
        let mut meta = GameData::default();
        let mut last_discovery = Instant::now() - Duration::from_secs(6);
        let mut active_key = String::new();
        let mut done_runes = false;
        let mut done_spells = false;
        let mut retry_at = Instant::now();
        let mut pause = Duration::from_millis(900);
        loop {
            thread::sleep(pause);
            pause = Duration::from_millis(900);
            if status.lock().unwrap().stop {
                break;
            }
            let config = settings.lock().unwrap().clone();
            if !config.enabled || (!config.runes && !config.spells) {
                let mut s = status.lock().unwrap();
                s.phase = Phase::Paused;
                s.busy = false;
                s.title = "Paused".into();
                s.detail = "Your League setup stays exactly as it is.".into();
                s.activity = "Resume whenever you're ready".into();
                continue;
            }
            if connection.is_none() && last_discovery.elapsed() > Duration::from_secs(5) {
                connection = client::discover(&config.folder);
                last_discovery = Instant::now();
            }
            let Some(api) = connection.as_ref() else {
                let mut s = status.lock().unwrap();
                s.phase = Phase::Offline;
                s.busy = false;
                s.title = "Waiting for League".into();
                s.detail = "Open the client. We'll take it from here.".into();
                s.activity = "Standing by".into();
                active_key.clear();
                continue;
            };
            let phase = match api.call("GET", "/lol-gameflow/v1/gameflow-phase", None) {
                Ok(r) if r.status == 200 => r.body.as_str().unwrap_or("").to_owned(),
                _ => {
                    connection = None;
                    meta = GameData::default();
                    let mut s = status.lock().unwrap();
                    s.phase = Phase::Offline;
                    s.error.clear();
                    continue;
                }
            };
            if meta.is_empty() {
                meta = client::game_data(api);
                if let Some(flash) = meta.spells.get(&FLASH).cloned() {
                    fetch_icons(api, &icons, &[&flash.icon]);
                    status.lock().unwrap().flash_icon = Some(flash);
                }
            }
            if phase != "ChampSelect" {
                active_key.clear();
                let in_game = matches!(phase.as_str(), "InProgress" | "Reconnect" | "GameStart");
                let mut s = status.lock().unwrap();
                s.busy = false;
                s.error.clear();
                if in_game {
                    // Poll gently while the game runs; nothing to do until it ends.
                    pause = Duration::from_secs(4);
                    s.phase = Phase::InGame;
                    s.title = "In game".into();
                    s.detail = "Keeping a low profile until you're back.".into();
                    s.activity = "Running quietly in the tray".into();
                } else {
                    s.phase = Phase::Idle;
                    s.title = "Standing by".into();
                    s.detail = "Runes and spells land the moment you pick.".into();
                    s.activity = "League connected".into();
                }
                continue;
            }
            status.lock().unwrap().phase = Phase::ChampSelect;
            let session = match api
                .call("GET", client::SESSION, None)
                .and_then(client::success)
            {
                Ok(s) => s,
                Err(e) => {
                    status.lock().unwrap().error = e;
                    continue;
                }
            };
            let flow = match api
                .call("GET", "/lol-gameflow/v1/session", None)
                .and_then(client::success)
            {
                Ok(v) => v,
                Err(e) => {
                    status.lock().unwrap().error = e;
                    continue;
                }
            };
            let queue = flow
                .pointer("/gameData/queue/id")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if queue == 0 {
                status.lock().unwrap().error =
                    "Waiting for League to identify the game mode.".into();
                continue;
            }
            if client::is_tft(queue) {
                let mut s = status.lock().unwrap();
                s.title = "TFT".into();
                s.detail = "No runes in TFT. We'll sit this one out.".into();
                continue;
            }
            let map = client::game_mode(&flow);
            let Some((champion, mut lane)) = client::selection_for_queue(&session, queue) else {
                let mut s = status.lock().unwrap();
                s.busy = false;
                s.title = "Pick your champion".into();
                s.detail = "Your setup follows your pick.".into();
                s.activity = "Champion select".into();
                continue;
            };
            let assigned_lane = lane.clone();
            let (mode, tier, default_lane) = client::mode(queue, map);
            let key = format!(
                "{}:{champion}:{lane}:{queue}:{}",
                session["gameId"], config.revision
            );
            if key != active_key {
                active_key = key;
                done_runes = false;
                done_spells = false;
                retry_at = Instant::now();
                let mut s = status.lock().unwrap();
                s.error.clear();
                s.rune = "Waiting to apply".into();
                s.spell = "Waiting to apply".into();
                s.setup = None;
            }
            if (done_runes || !config.runes) && (done_spells || !config.spells) {
                continue;
            }
            if Instant::now() < retry_at {
                continue;
            }
            retry_at = Instant::now() + Duration::from_secs(10);
            let provider_champion = client::recommendation_champion(champion, queue, map);
            let champion_name = client::champion_name(api, provider_champion);
            let mut setup = status
                .lock()
                .unwrap()
                .setup
                .clone()
                .unwrap_or_else(|| Setup {
                    champion: champion_name.clone(),
                    champion_icon: client::champion_icon(provider_champion),
                    lane: assigned_lane.clone(),
                    mode: mode_label(mode).into(),
                    ..Default::default()
                });
            fetch_icons(api, &icons, &[&setup.champion_icon]);
            {
                let mut s = status.lock().unwrap();
                s.busy = true;
                s.title = "Sorting your setup".into();
                s.detail = client::champion_label(&champion_name, &lane);
                s.activity = "Fetching the recommended runes and spells".into();
                s.setup = Some(setup.clone());
            }
            // Swiftplay has its own Flash/Smite/Heal IDs and skips provider spells,
            // so they are applied even when recommendations cannot be fetched.
            if config.spells
                && !done_spells
                && client::is_jade(queue, map)
                && *settings.lock().unwrap() == config
                && still_selected(api, champion, &assigned_lane, queue)
            {
                let pair = client::jade_spells(&assigned_lane, &config.flash);
                match client::apply_spells(api, &pair) {
                    Ok(()) => {
                        done_spells = true;
                        setup.spells = spell_assets(&meta, &pair);
                        setup.spells_applied = true;
                        status.lock().unwrap().spell = "Applied and confirmed".into();
                    }
                    Err(e) => {
                        let mut s = status.lock().unwrap();
                        s.busy = false;
                        s.spell = "Could not apply".into();
                        s.error = e;
                        continue;
                    }
                }
                if !config.runes {
                    fetch_icons(api, &icons, &setup_icons(&setup));
                    let mut s = status.lock().unwrap();
                    s.busy = false;
                    s.title = "Setup applied".into();
                    s.detail = "That went reasonably well.".into();
                    s.activity = "Confirmed by the League client".into();
                    s.last_applied = system::local_time();
                    s.setup = Some(setup);
                    continue;
                }
            }
            if !default_lane.is_empty() {
                lane = default_lane.into();
            }
            if lane.is_empty() || mode == "classic" {
                let summary = client::remote(&format!(
                    "/api/GLOBAL/champions/{mode}/{provider_champion}/summaries?tier={tier}&version="
                ));
                // Without a usable summary, recommendations start from top lane.
                lane = client::recommended_lane(summary.ok().as_ref(), &lane, mode);
            }
            if mode_label(mode).is_empty() {
                setup.lane = lane.clone();
            }
            let path = client::recommendation_path(mode, provider_champion, &lane, tier);
            let data = match client::remote(&path) {
                Ok(d) => d,
                Err(e) => {
                    let mut s = status.lock().unwrap();
                    s.busy = false;
                    s.error = e;
                    continue;
                }
            };
            // A network response can arrive after the player changed champion or left select.
            if *settings.lock().unwrap() != config
                || !still_selected(api, champion, &assigned_lane, queue)
            {
                active_key.clear();
                continue;
            }
            let mut errors = Vec::new();
            if config.runes && !done_runes {
                match client::rune_page(&data, &champion_name, &lane).and_then(|p| {
                    client::apply_runes(api, &p)?;
                    Ok(p)
                }) {
                    Ok(page) => {
                        done_runes = true;
                        rune_assets(&mut setup, &meta, &page);
                        setup.runes_applied = true;
                        status.lock().unwrap().rune = "Applied and confirmed".into();
                    }
                    Err(e) => {
                        status.lock().unwrap().rune = "Could not apply".into();
                        errors.push(e);
                    }
                }
            }
            if config.spells
                && !done_spells
                && *settings.lock().unwrap() == config
                && still_selected(api, champion, &assigned_lane, queue)
            {
                match client::recommended_spells(&data, queue, map, &config.flash)
                    .ok_or_else(|| "No complete spell recommendation is available.".to_string())
                    .and_then(|p| {
                        client::apply_spells(api, &p)?;
                        Ok(p)
                    }) {
                    Ok(pair) => {
                        done_spells = true;
                        setup.spells = spell_assets(&meta, &pair);
                        setup.spells_applied = true;
                        status.lock().unwrap().spell = "Applied and confirmed".into();
                    }
                    Err(e) => {
                        status.lock().unwrap().spell = "Could not apply".into();
                        errors.push(e);
                    }
                }
            }
            fetch_icons(api, &icons, &setup_icons(&setup));
            let mut s = status.lock().unwrap();
            s.busy = false;
            s.error = errors.join(" ");
            s.setup = Some(setup);
            if (done_runes || !config.runes) && (done_spells || !config.spells) {
                s.title = "Setup applied".into();
                s.detail = "That went reasonably well.".into();
                s.activity = "Confirmed by the League client".into();
                s.last_applied = system::local_time();
            } else {
                s.title = "Needs a hand".into();
                s.detail = "See the note below. We'll retry on our own.".into();
            }
        }
    });
}
fn still_selected(api: &dyn Api, champion: u64, lane: &str, queue: u64) -> bool {
    let phase = api
        .call("GET", "/lol-gameflow/v1/gameflow-phase", None)
        .and_then(client::success);
    if !matches!(phase,Ok(ref v) if v=="ChampSelect") {
        return false;
    }
    api.call("GET", client::SESSION, None)
        .and_then(client::success)
        .ok()
        .and_then(|v| client::selection_for_queue(&v, queue))
        .is_some_and(|(id, role)| id == champion && role == lane)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::Reply;
    use std::cell::RefCell;
    struct SessionApi {
        phase: Value,
        session: Value,
        calls: RefCell<Vec<String>>,
    }
    impl Api for SessionApi {
        fn call(&self, _method: &str, path: &str, _body: Option<&Value>) -> Result<Reply, String> {
            self.calls.borrow_mut().push(path.into());
            Ok(Reply {
                status: 200,
                body: if path.ends_with("gameflow-phase") {
                    self.phase.clone()
                } else {
                    self.session.clone()
                },
            })
        }
    }
    #[test]
    fn stale_champion_or_role_is_not_applied() {
        let api = SessionApi {
            phase: json!("ChampSelect"),
            session: json!({"localPlayerCellId":2,"myTeam":[{"cellId":2,"championId":64,"assignedPosition":"jungle"}]}),
            calls: RefCell::new(vec![]),
        };
        assert!(still_selected(&api, 64, "jungle", 420));
        assert!(!still_selected(&api, 99, "jungle", 420));
        assert!(!still_selected(&api, 64, "mid", 420));
    }
    #[test]
    fn no_write_after_champion_select_ends() {
        let api = SessionApi {
            phase: json!("InProgress"),
            session: Value::Null,
            calls: RefCell::new(vec![]),
        };
        assert!(!still_selected(&api, 64, "jungle", 420));
        assert_eq!(api.calls.borrow().len(), 1);
    }
    #[test]
    fn setup_card_uses_client_names_and_key_order() {
        let mut meta = GameData::default();
        let asset = |n: &str| Asset {
            name: n.into(),
            icon: format!("/lol-game-data/assets/{n}.png"),
        };
        meta.spells.insert(4, asset("Flash"));
        meta.spells.insert(11, asset("Smite"));
        meta.styles.insert(8000, asset("Precision"));
        meta.styles.insert(8100, asset("Domination"));
        meta.perks.insert(8010, asset("Conqueror"));
        let [d, f] = spell_assets(&meta, &json!({"spell1Id":11,"spell2Id":4}));
        assert_eq!(d.unwrap().name, "Smite");
        assert_eq!(f.unwrap().name, "Flash");
        let mut setup = Setup::default();
        rune_assets(
            &mut setup,
            &meta,
            &json!({"primaryStyleId":8000,"subStyleId":8100,"selectedPerkIds":[8010,9111]}),
        );
        assert_eq!(setup.keystone.unwrap().name, "Conqueror");
        assert_eq!(setup.secondary.unwrap().name, "Domination");
        assert_eq!(mode_label("aram"), "ARAM");
        assert_eq!(mode_label("ranked"), "");
    }
}
