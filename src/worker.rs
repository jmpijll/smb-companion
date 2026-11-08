use crate::client::{self, Api, Asset, Connection, GameData};
use crate::system;
use serde_json::{json, Value};
use std::{
    cell::OnceCell,
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
    /// Item set and skill order in the shop. Off unless the player opts in.
    pub items: bool,
    pub flash: String,
    /// Optional League folder; the running client is normally found by itself.
    pub folder: String,
    pub revision: u64,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            runes: true,
            spells: true,
            items: false,
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
            s.items = v["items"].as_bool().unwrap_or(false);
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
            "items": self.items,
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
    /// "E Q W, max Q > W > E" when the item set is on.
    pub skills: String,
    pub runes_applied: bool,
    pub spells_applied: bool,
    pub items_applied: bool,
    /// Runes or spells came from League's own recommendations (OP.GG was out).
    pub from_league: bool,
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
    /// League's own gameflow phase ("Lobby", "Matchmaking", ...).
    pub gameflow: String,
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
    /// Labels of the player's summoner spell keys, as bound in League.
    pub keys: [String; 2],
    /// A newer release, for copies that cannot update themselves.
    pub update: String,
    pub stop: bool,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            phase: Phase::Offline,
            gameflow: String::new(),
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
            keys: ["D".into(), "F".into()],
            update: String::new(),
            stop: false,
        }
    }
}
pub type SharedSettings = Arc<Mutex<Settings>>;
pub type SharedStatus = Arc<Mutex<Status>>;
/// Icon bytes by client path. `None` marks a fetch that failed (not retried).
pub type Icons = Arc<Mutex<HashMap<String, Option<Arc<[u8]>>>>>;

const FLASH: u64 = 4;
/// Swiftplay's own Flash.
const FLASH_SWIFTPLAY: u64 = 74;

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

/// The key Flash sits on in a spell selection ("d" or "f").
fn flash_key(selection: &Value) -> Option<&'static str> {
    let flash = |k: &str| matches!(selection[k].as_u64(), Some(FLASH | FLASH_SWIFTPLAY));
    match (flash("spell1Id"), flash("spell2Id")) {
        (true, false) => Some("d"),
        (false, true) => Some("f"),
        _ => None,
    }
}

/// Follows the player's own habit instead of asking: where Flash was left
/// last game, or where they move it in champion select.
fn learn_flash(settings: &SharedSettings, config: &mut Settings, key: &str, why: &str) {
    if config.flash == key {
        return;
    }
    config.flash = key.into();
    let mut s = settings.lock().unwrap();
    s.flash = key.into();
    let _ = s.save();
    system::log(&format!(
        "Flash goes on {} now ({why}).",
        key.to_uppercase()
    ));
}

#[derive(Default)]
struct Done {
    runes: bool,
    spells: bool,
    items: bool,
}
impl Done {
    /// Runes and spells are in place: what "Setup applied" means.
    fn settled(&self, config: &Settings) -> bool {
        (self.runes || !config.runes) && (self.spells || !config.spells)
    }
    fn all(&self, config: &Settings) -> bool {
        self.settled(config) && (self.items || !config.items)
    }
}

/// Runs the champion-select loop and restarts it if it ever crashes, so the
/// companion cannot silently stop working while its tray icon looks fine.
pub fn start(settings: SharedSettings, status: SharedStatus, icons: Icons) {
    thread::spawn(move || loop {
        let (s, st, i) = (settings.clone(), status.clone(), icons.clone());
        let finished = thread::Builder::new()
            .name("champion-select".into())
            .spawn(move || run(s, st, i))
            .map(|h| h.join().is_ok())
            .unwrap_or(false);
        settings.clear_poison();
        status.clear_poison();
        icons.clear_poison();
        if finished || status.lock().unwrap().stop {
            break;
        }
        system::log("The background loop stopped unexpectedly and was restarted.");
        thread::sleep(Duration::from_secs(5));
    });
}

fn run(settings: SharedSettings, status: SharedStatus, icons: Icons) {
    let mut connection: Option<Connection> = None;
    let mut meta = GameData::default();
    let mut last_discovery = Instant::now() - Duration::from_secs(6);
    let mut active_key = String::new();
    let mut done = Done::default();
    let mut retry_at = Instant::now();
    let mut pause = Duration::from_millis(900);
    let mut in_select = false;
    // The spells as written, to notice the player swapping them afterwards.
    let mut applied_pair: Option<Value> = None;
    // OP.GG's answer for the current pick (key, lane, data), reused on retries.
    let mut cache: Option<(String, String, Value)> = None;
    let mut logged = String::new();
    // The Flash key at the previous champion select. A key the player picked
    // in the app since then wins over what the client pre-fills this time.
    let mut flash_before: Option<String> = None;
    let log_once = |logged: &mut String, line: String| {
        if *logged != line {
            system::log(&line);
            *logged = line;
        }
    };
    loop {
        thread::sleep(pause);
        pause = Duration::from_millis(900);
        if status.lock().unwrap().stop {
            break;
        }
        let mut config = settings.lock().unwrap().clone();
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
            if connection.is_some() {
                system::log("Found the League client.");
            }
        }
        let Some(api) = connection.as_ref() else {
            let mut s = status.lock().unwrap();
            s.phase = Phase::Offline;
            s.gameflow.clear();
            s.busy = false;
            s.title = "Waiting for League".into();
            s.detail = "Open the client. We'll take it from here.".into();
            s.activity = "Standing by".into();
            active_key.clear();
            in_select = false;
            continue;
        };
        let phase = match api.call("GET", "/lol-gameflow/v1/gameflow-phase", None) {
            Ok(r) if r.status == 200 => r.body.as_str().unwrap_or("").to_owned(),
            _ => {
                connection = None;
                meta = GameData::default();
                system::log("The League client closed.");
                let mut s = status.lock().unwrap();
                s.phase = Phase::Offline;
                s.gameflow.clear();
                s.error.clear();
                continue;
            }
        };
        if meta.is_empty() {
            meta = client::game_data(api);
            let keys = client::spell_keys(api);
            if let Some(flash) = meta.spells.get(&FLASH).cloned() {
                fetch_icons(api, &icons, &[&flash.icon]);
                status.lock().unwrap().flash_icon = Some(flash);
            }
            status.lock().unwrap().keys = keys;
        }
        status.lock().unwrap().gameflow = phase.clone();
        if phase != "ChampSelect" {
            active_key.clear();
            in_select = false;
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
        if !in_select {
            in_select = true;
            system::log("Champion select started.");
            // The client pre-fills the spells as the player left them last
            // game, before anything is written: that is where Flash belongs.
            let chosen_in_app = flash_before.as_ref().is_some_and(|k| *k != config.flash);
            if config.spells && !chosen_in_app {
                if let Some(key) = api
                    .call("GET", client::SPELLS, None)
                    .and_then(client::success)
                    .ok()
                    .as_ref()
                    .and_then(flash_key)
                {
                    learn_flash(&settings, &mut config, key, "where you left it last game");
                }
            }
            flash_before = Some(config.flash.clone());
            // Item sets switched off: take back the one this app made earlier.
            if !config.items {
                match client::apply_item_set(api, None) {
                    Ok(true) => system::log("Removed the companion's item set."),
                    Ok(false) => {}
                    Err(e) => system::log(&format!("Item set not removed: {e}")),
                }
            }
        }
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
            status.lock().unwrap().error = "Waiting for League to identify the game mode.".into();
            continue;
        }
        if client::is_tft(queue) {
            let mut s = status.lock().unwrap();
            s.title = "TFT".into();
            s.detail = "No runes in TFT. We'll sit this one out.".into();
            continue;
        }
        let map = client::game_mode(&flow);
        let map_id = flow.pointer("/map/id").and_then(Value::as_u64).unwrap_or(0);
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
            done = Done::default();
            applied_pair = None;
            retry_at = Instant::now();
            let mut s = status.lock().unwrap();
            s.error.clear();
            s.rune = "Waiting to apply".into();
            s.spell = "Waiting to apply".into();
            s.setup = None;
        }
        if done.all(&config) {
            watch_swap(api, &settings, &mut config, &status, &mut applied_pair);
            // Applied (and possibly moved by the player): the key in use now.
            flash_before = Some(config.flash.clone());
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
        // Only runes and spells show as work in progress; an item set that is
        // still waiting for OP.GG retries quietly.
        if !done.settled(&config) {
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
            && !done.spells
            && client::is_jade(queue, map)
            && *settings.lock().unwrap() == config
            && still_selected(api, champion, &assigned_lane, queue)
        {
            let pair = client::jade_spells(&assigned_lane, &config.flash);
            match client::apply_spells(api, &pair) {
                Ok(()) => {
                    done.spells = true;
                    setup.spells = spell_assets(&meta, &pair);
                    setup.spells_applied = true;
                    applied_pair = Some(pair);
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
        }
        if !default_lane.is_empty() {
            lane = default_lane.into();
        }
        let data = match &cache {
            Some((k, l, d)) if *k == active_key => {
                lane = l.clone();
                Ok(d.clone())
            }
            _ => {
                if lane.is_empty() || mode == "classic" {
                    let summary = client::remote(&format!(
                        "/api/GLOBAL/champions/{mode}/{provider_champion}/summaries?tier={tier}&version="
                    ));
                    // Without a usable summary, recommendations start from top lane.
                    lane = client::recommended_lane(summary.ok().as_ref(), &lane, mode);
                }
                let path = client::recommendation_path(mode, provider_champion, &lane, tier);
                let data = client::remote(&path);
                if let Ok(d) = &data {
                    cache = Some((active_key.clone(), lane.clone(), d.clone()));
                }
                data
            }
        };
        if let Err(e) = &data {
            log_once(&mut logged, format!("OP.GG: {e}"));
        }
        // A network response can arrive after the player changed champion or left select.
        if *settings.lock().unwrap() != config
            || !still_selected(api, champion, &assigned_lane, queue)
        {
            active_key.clear();
            continue;
        }
        // League's own recommendation, fetched only if OP.GG cannot help.
        let league = OnceCell::new();
        let league = || {
            league
                .get_or_init(|| {
                    client::league_recommendation(api, provider_champion, &assigned_lane, map_id)
                })
                .as_ref()
        };
        // With no assigned role, League's default position names the page.
        let league_lane = |rec: &Value| {
            if assigned_lane.is_empty() && mode_label(mode).is_empty() {
                client::position_lane(rec["position"].as_str().unwrap_or("")).to_owned()
            } else {
                lane.clone()
            }
        };
        if mode_label(mode).is_empty() {
            setup.lane = lane.clone();
        }
        let mut errors = Vec::new();
        if config.runes && !done.runes {
            let page = match data
                .as_ref()
                .map_err(String::clone)
                .and_then(|d| client::rune_page(d, &champion_name, &lane))
            {
                Ok(page) => Ok(page),
                Err(e) => match league().and_then(|r| {
                    let l = league_lane(r);
                    client::league_rune_page(r, &champion_name, &l).map(|p| (p, l))
                }) {
                    Some((page, l)) => {
                        setup.from_league = true;
                        if mode_label(mode).is_empty() {
                            setup.lane = l;
                        }
                        Ok(page)
                    }
                    None => Err(e),
                },
            };
            match page.and_then(|p| {
                client::apply_runes(api, &p)?;
                Ok(p)
            }) {
                Ok(page) => {
                    done.runes = true;
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
            && !done.spells
            && *settings.lock().unwrap() == config
            && still_selected(api, champion, &assigned_lane, queue)
        {
            let pair = match data
                .as_ref()
                .ok()
                .and_then(|d| client::recommended_spells(d, queue, map, &config.flash))
            {
                Some(pair) => Some(pair),
                None => {
                    let pair = league().and_then(|r| client::league_spells(r, &config.flash));
                    setup.from_league |= pair.is_some();
                    pair
                }
            };
            match pair
                .ok_or_else(|| "No complete spell recommendation is available.".to_string())
                .and_then(|p| {
                    client::apply_spells(api, &p)?;
                    Ok(p)
                }) {
                Ok(pair) => {
                    done.spells = true;
                    setup.spells = spell_assets(&meta, &pair);
                    setup.spells_applied = true;
                    applied_pair = Some(pair);
                    status.lock().unwrap().spell = "Applied and confirmed".into();
                }
                Err(e) => {
                    status.lock().unwrap().spell = "Could not apply".into();
                    errors.push(e);
                }
            }
        }
        // The item set needs OP.GG's build data; without it, try again later.
        if config.items && !done.items {
            if let Ok(d) = &data {
                setup.skills = client::skill_order(d);
                let label = match mode_label(mode) {
                    "" => client::champion_label(&champion_name, &setup.lane),
                    m => format!("{champion_name} · {m}"),
                };
                match client::item_set(d, champion, &label, map_id) {
                    None => done.items = true, // no build data for this pick
                    Some(set) => match client::apply_item_set(api, Some(&set)) {
                        Ok(_) => {
                            done.items = true;
                            setup.items_applied = true;
                        }
                        Err(e) => errors.push(e),
                    },
                }
            }
        }
        fetch_icons(api, &icons, &setup_icons(&setup));
        let mut s = status.lock().unwrap();
        s.busy = false;
        s.error = errors.join(" ");
        if !s.error.is_empty() {
            log_once(&mut logged, format!("Needs a hand: {}", s.error));
        }
        if done.settled(&config) {
            s.title = "Setup applied".into();
            s.detail = if setup.from_league {
                "League's own picks. OP.GG was out of reach.".into()
            } else {
                "That went reasonably well.".into()
            };
            s.activity = "Confirmed by the League client".into();
            s.last_applied = system::local_time();
            log_once(&mut logged, applied_line(&setup, &config));
        } else {
            s.title = "Needs a hand".into();
            s.detail = "See the note below. We'll retry on our own.".into();
        }
        s.setup = Some(setup);
    }
}

/// The player swapped the spells after they were applied: remember the new
/// Flash key for next time and show the swap, without writing anything.
fn watch_swap(
    api: &dyn Api,
    settings: &SharedSettings,
    config: &mut Settings,
    status: &SharedStatus,
    applied: &mut Option<Value>,
) {
    let Some(pair) = applied.as_ref() else { return };
    if !config.spells {
        return;
    }
    let Ok(now) = api
        .call("GET", client::SPELLS, None)
        .and_then(client::success)
    else {
        return;
    };
    if now["spell1Id"] != pair["spell2Id"] || now["spell2Id"] != pair["spell1Id"] {
        return;
    }
    if let Some(key) = flash_key(&now) {
        learn_flash(settings, config, key, "you moved it in champion select");
    }
    if let Some(setup) = status.lock().unwrap().setup.as_mut() {
        setup.spells.swap(0, 1);
    }
    *applied = Some(json!({"spell1Id": now["spell1Id"], "spell2Id": now["spell2Id"]}));
}

fn applied_line(setup: &Setup, config: &Settings) -> String {
    let name = |a: &Option<Asset>| a.as_ref().map(|a| a.name.clone()).unwrap_or_default();
    let mut parts = vec![format!(
        "Applied {}",
        client::champion_label(&setup.champion, &setup.lane)
    )];
    if config.runes {
        parts.push(name(&setup.keystone));
    }
    if config.spells {
        parts.push(format!(
            "{} + {}",
            name(&setup.spells[0]),
            name(&setup.spells[1])
        ));
    }
    if setup.items_applied {
        parts.push(format!("item set ({})", setup.skills));
    }
    parts.push(
        if setup.from_league {
            "League's picks"
        } else {
            "OP.GG"
        }
        .into(),
    );
    parts.join(" · ")
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
    #[test]
    fn flash_key_is_read_from_the_selection() {
        assert_eq!(flash_key(&json!({"spell1Id":4,"spell2Id":21})), Some("d"));
        assert_eq!(flash_key(&json!({"spell1Id":11,"spell2Id":4})), Some("f"));
        assert_eq!(flash_key(&json!({"spell1Id":74,"spell2Id":77})), Some("d"));
        // No Flash, or nothing chosen yet: nothing to learn.
        assert_eq!(flash_key(&json!({"spell1Id":32,"spell2Id":13})), None);
        assert_eq!(flash_key(&json!({"spell1Id":0,"spell2Id":0})), None);
        assert_eq!(flash_key(&Value::Null), None);
    }
    #[test]
    fn swap_after_apply_is_noticed_without_writing() {
        struct Swapped(RefCell<Vec<String>>);
        impl Api for Swapped {
            fn call(&self, method: &str, path: &str, _b: Option<&Value>) -> Result<Reply, String> {
                self.0.borrow_mut().push(format!("{method} {path}"));
                Ok(Reply {
                    status: 200,
                    body: json!({"spell1Id":11,"spell2Id":4}),
                })
            }
        }
        let api = Swapped(RefCell::new(vec![]));
        let settings: SharedSettings = Default::default();
        settings.lock().unwrap().spells = true;
        let status: SharedStatus = Default::default();
        status.lock().unwrap().setup = Some(Setup {
            spells: [
                Some(Asset {
                    name: "Flash".into(),
                    icon: String::new(),
                }),
                None,
            ],
            ..Default::default()
        });
        // Same key as learned: the setting is untouched (no save in tests).
        let mut config = Settings {
            flash: "f".into(),
            ..Default::default()
        };
        let mut applied = Some(json!({"spell1Id":4,"spell2Id":11}));
        watch_swap(&api, &settings, &mut config, &status, &mut applied);
        assert_eq!(applied, Some(json!({"spell1Id":11,"spell2Id":4})));
        assert!(status.lock().unwrap().setup.as_ref().unwrap().spells[1].is_some());
        assert!(api.0.borrow().iter().all(|c| c.starts_with("GET ")));
    }
    #[test]
    fn items_stay_off_unless_chosen() {
        assert!(!Settings::default().items);
        let config = Settings::default();
        let done = Done {
            runes: true,
            spells: true,
            items: false,
        };
        assert!(done.all(&config));
    }
}
