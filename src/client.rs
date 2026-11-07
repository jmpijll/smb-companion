use serde_json::{json, Value};
use std::{collections::HashMap, fs, path::PathBuf, sync::OnceLock};

pub const PAGES: &str = "/lol-perks/v1/pages";
pub const SESSION: &str = "/lol-champ-select/v1/session";
pub const SPELLS: &str = "/lol-champ-select/v1/session/my-selection";
pub const PROVIDER: &str = "lol-api-champion.op.gg";
/// Rune pages this app owns start with this prefix. Pages named by the
/// earlier "Quiet Select" build are adopted instead of duplicated.
pub const OWN_PAGE: &str = "SMB //";
const LEGACY_PAGE: &str = "Quiet Select · ";

pub fn is_own_page(name: &str) -> bool {
    name.starts_with(&format!("{OWN_PAGE} ")) || name.starts_with(LEGACY_PAGE)
}

pub fn game_mode(flow: &Value) -> &str {
    flow.pointer("/map/gameMode")
        .and_then(Value::as_str)
        .unwrap_or("")
}

pub fn is_tft(queue: u64) -> bool {
    [
        1090, 1100, 1110, 1120, 1130, 1140, 1150, 1160, 1170, 1180, 1190, 1210, 6120, 6110,
    ]
    .contains(&queue)
}

// Swiftplay ("JADE") uses its own champion and spell IDs.
pub fn is_jade(queue: u64, map: &str) -> bool {
    queue == 4310 || map == "JADE"
}

pub fn recommendation_champion(champion: u64, queue: u64, map: &str) -> u64 {
    if is_jade(queue, map) && champion >= 60_000 {
        champion - 60_000
    } else {
        champion
    }
}

pub fn jade_spells(lane: &str, key: &str) -> Value {
    let second = if lane.eq_ignore_ascii_case("jungle") {
        711
    } else {
        77
    };
    if key == "d" {
        json!({"spell1Id":74,"spell2Id":second})
    } else {
        json!({"spell1Id":second,"spell2Id":74})
    }
}

pub fn recommended_spells(data: &Value, queue: u64, map: &str, key: &str) -> Option<Value> {
    let exclude_exhaust = map == "KIWI" || [2400, 2401, 2403, 2405].contains(&queue);
    let first = data["summoner_spells"].as_array()?.iter().find(|p| {
        !(exclude_exhaust && p["ids"].as_array().is_some_and(|v| v.contains(&json!(3))))
    })?;
    let ids = first["ids"]
        .as_array()?
        .iter()
        .map(Value::as_u64)
        .collect::<Option<Vec<_>>>()?;
    ordered_spells(&ids, key)
}

pub fn ordered_spells(ids: &[u64], key: &str) -> Option<Value> {
    if ids.len() != 2 || ids.contains(&0) || ids[0] == ids[1] {
        return None;
    }
    let mut pair = [ids[0], ids[1]];
    if pair.contains(&4) {
        let flash_index = if key == "d" { 0 } else { 1 };
        if pair[flash_index] != 4 {
            pair.swap(0, 1);
        }
    } else if key == "f" {
        pair.swap(0, 1);
    }
    Some(json!({"spell1Id":pair[0],"spell2Id":pair[1]}))
}

// Also used by the read-only live diagnostic, which reports selected champions.
#[allow(dead_code)]
pub fn selection(session: &Value) -> Option<(u64, String)> {
    selection_for_queue(session, 420)
}

// In draft/ranked queues only a locked-in pick counts; elsewhere the
// hovered pick intent is enough to prepare the setup.
pub fn selection_for_queue(session: &Value, queue: u64) -> Option<(u64, String)> {
    let cell = session.get("localPlayerCellId")?.as_i64()?;
    let player = session
        .get("myTeam")?
        .as_array()?
        .iter()
        .find(|p| p["cellId"].as_i64() == Some(cell))?;
    let mut champion = player.get("championId")?.as_u64()?;
    if champion == 0 {
        if ![420, 440].contains(&queue) {
            champion = player["championPickIntent"].as_u64().unwrap_or(0);
        }
        if champion == 0 {
            champion = session["actions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_array)
                .flatten()
                .find(|a| a["type"] == "pick" && a["actorCellId"].as_i64() == Some(cell))
                .filter(|a| ![420, 440].contains(&queue) || a["completed"] == true)
                .and_then(|a| a["championId"].as_u64())
                .unwrap_or(0);
        }
    }
    if champion == 0 {
        return None;
    }
    let lane = match player["assignedPosition"]
        .as_str()
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "bottom" | "adc" => "adc",
        "utility" | "support" => "support",
        "middle" | "mid" => "mid",
        "top" => "top",
        "jungle" => "jungle",
        _ => "",
    };
    Some((champion, lane.into()))
}

pub fn mode(queue: u64, map: &str) -> (&'static str, &'static str, &'static str) {
    if map == "KIWI_JADE" || queue == 2450 {
        ("aram_mayhem_classic", "all", "none")
    } else if map == "JADE" || [4300, 4310].contains(&queue) {
        ("classic", "all", "")
    } else if ["URF", "ARURF"].contains(&map) || [900, 1900].contains(&queue) {
        ("urf", "gold_plus", "none")
    } else if ["ARAM", "KIWI"].contains(&map)
        || [450, 720, 2400, 2401, 2403, 2405, 3240].contains(&queue)
    {
        ("aram", "emerald_plus", "none")
    } else if map == "CHERRY" || [1700, 1710, 1750].contains(&queue) {
        ("arena", "", "none")
    } else {
        ("ranked", "emerald_plus", "")
    }
}

pub fn recommended_lane(summary: Option<&Value>, assigned: &str, mode: &str) -> String {
    let positions = summary.and_then(|s| s["positions"].as_array());
    let first = positions
        .and_then(|p| p.first())
        .and_then(|p| p["name"].as_str())
        .map(str::to_lowercase);
    let mut lane = if assigned.is_empty() {
        if summary.is_some_and(|s| s["is_rip"] == true) {
            "top".into()
        } else {
            first.clone().unwrap_or_else(|| "top".into())
        }
    } else {
        assigned.to_lowercase()
    };
    if mode == "classic"
        && positions.is_some_and(|p| {
            !p.is_empty()
                && !p.iter().any(|p| {
                    p["name"]
                        .as_str()
                        .is_some_and(|n| n.eq_ignore_ascii_case(&lane))
                })
        })
    {
        lane = first.unwrap_or_else(|| "top".into());
    }
    lane
}

// League ships champion metadata with the client, so names follow its installed
// roster and locale without a second internet provider or a hardcoded ID table.
pub fn champion_name(api: &dyn Api, champion: u64) -> String {
    api.call(
        "GET",
        &format!("/lol-game-data/assets/v1/champions/{champion}.json"),
        None,
    )
    .and_then(success)
    .ok()
    .and_then(|v| {
        v["name"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    })
    .unwrap_or_else(|| "Your champion".into())
}

pub fn champion_label(champion: &str, lane: &str) -> String {
    let role = match lane {
        "top" => "Top",
        "jungle" => "Jungle",
        "mid" => "Mid",
        "adc" => "Bot",
        "support" => "Support",
        _ => "",
    };
    if role.is_empty() {
        champion.into()
    } else {
        format!("{champion} · {role}")
    }
}

pub fn rune_page(data: &Value, champion: &str, lane: &str) -> Result<Value, String> {
    let b = data
        .get("rune_pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(5)
        .filter_map(|page| page["builds"].as_array())
        .flatten()
        .next()
        .ok_or("No rune recommendation is available for this champion.")?;
    let primary = b["primary_page_id"]
        .as_u64()
        .filter(|x| *x > 0)
        .ok_or("The rune recommendation is incomplete.")?;
    let secondary = b["secondary_page_id"]
        .as_u64()
        .filter(|x| *x > 0)
        .ok_or("The rune recommendation is incomplete.")?;
    let mut ids = vec![];
    for (key, count) in [
        ("primary_rune_ids", 4),
        ("secondary_rune_ids", 2),
        ("stat_mod_ids", 3),
    ] {
        let list = b[key]
            .as_array()
            .ok_or("The rune recommendation is incomplete.")?;
        if list.len() != count || list.iter().any(|x| x.as_u64().unwrap_or(0) == 0) {
            return Err("The rune recommendation is incomplete.".into());
        }
        ids.extend(list.iter().cloned());
    }
    Ok(
        json!({"name":format!("{OWN_PAGE} {}", champion_label(champion, lane)),"primaryStyleId":primary,"subStyleId":secondary,
        "selectedPerkIds":ids,"current":true,"isActive":true,"isDeletable":true,"isEditable":true,"isValid":true,
        "autoModifiedSelections":[0],"order":0,"quickPlayChampionIds":[]}),
    )
}

pub fn recommendation_path(mode: &str, champion: u64, lane: &str, tier: &str) -> String {
    let base = format!("/api/GLOBAL/champions/{mode}/{champion}");
    match mode {
        "arena" => format!("{base}/?tier={tier}&version="),
        "aram_mayhem_classic" => format!("{base}/none?tier={tier}&version=&is_more=true"),
        "aram" | "urf" => format!("{base}/none?tier={tier}&version="),
        _ => format!(
            "{base}/{lane}?tier={tier}&target_champion=&version=&role=&is_more=true{}",
            if mode == "classic" { "&hl=en_US" } else { "" }
        ),
    }
}

#[derive(Clone, Debug)]
pub struct Reply {
    pub status: u32,
    pub body: Value,
}
pub trait Api {
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Reply, String>;
}
pub fn success(reply: Reply) -> Result<Value, String> {
    if (200..300).contains(&reply.status) {
        Ok(reply.body)
    } else {
        Err(format!(
            "League returned HTTP {}. No success was recorded.",
            reply.status
        ))
    }
}
fn same_runes(a: &Value, b: &Value) -> bool {
    // Compare what we wrote; an absent quick-play binding equals an empty list.
    ["name", "primaryStyleId", "subStyleId", "selectedPerkIds"]
        .iter()
        .all(|k| a[*k] == b[*k])
        && a["quickPlayChampionIds"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
            == b["quickPlayChampionIds"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[])
}
pub fn apply_runes(api: &dyn Api, page: &Value) -> Result<(), String> {
    let pages = success(api.call("GET", PAGES, None)?)?;
    let all = pages
        .as_array()
        .ok_or("League returned an invalid rune page list.")?;
    // Reuse only this companion's page. Never remove the player's pages.
    let owned = all
        .iter()
        .find(|p| p["isEditable"] == true && is_own_page(p["name"].as_str().unwrap_or("")));
    let written = if let Some(old) = owned {
        let id = old["id"]
            .as_u64()
            .ok_or("The saved rune page has no valid ID.")?;
        let mut payload = page.clone();
        payload["id"] = json!(id);
        success(api.call("PUT", &format!("{PAGES}/{id}"), Some(&payload))?)?;
        id
    } else {
        let reply = api.call("POST", PAGES, Some(page))?;
        if reply.status == 400 {
            return Err("League could not create a rune page. Free one rune page slot, then retry; your pages were kept.".into());
        }
        success(reply)?["id"]
            .as_u64()
            .ok_or("League did not confirm the new rune page.")?
    };
    let check = success(api.call("GET", &format!("{PAGES}/{written}"), None)?)?;
    if !same_runes(&check, page) || check["current"] != true {
        return Err("League did not confirm the recommended rune page as active. Retry while in champion select.".into());
    }
    Ok(())
}
pub fn apply_spells(api: &dyn Api, spells: &Value) -> Result<(), String> {
    success(api.call("PATCH", SPELLS, Some(spells))?)?;
    let current = success(api.call("GET", SPELLS, None)?)?;
    if current["spell1Id"] != spells["spell1Id"] || current["spell2Id"] != spells["spell2Id"] {
        return Err(
            "League did not confirm the spell selection. Retry while in champion select.".into(),
        );
    }
    Ok(())
}

#[derive(Clone)]
pub struct Connection {
    port: u16,
    token: String,
}
impl Connection {
    pub fn from_lockfile(text: &str) -> Option<Self> {
        let p: Vec<_> = text.trim_matches(char::from(0)).trim().split(':').collect();
        if p.len() != 5 || p[4] != "https" || p[3].is_empty() {
            return None;
        }
        let port = p[2].parse::<u16>().ok().filter(|x| *x > 0)?;
        Some(Self {
            port,
            token: p[3].into(),
        })
    }
}
impl Api for Connection {
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Reply, String> {
        http(
            "127.0.0.1",
            self.port,
            method,
            path,
            Some(&self.token),
            body,
        )
    }
}
impl Connection {
    /// Raw bytes (icons) from the local client's own game data.
    pub fn bytes(&self, path: &str) -> Result<Vec<u8>, String> {
        if !path.starts_with("/lol-game-data/") {
            return Err("Only League game-data assets are requested.".into());
        }
        match request(
            "127.0.0.1",
            self.port,
            "GET",
            path,
            Some(&self.token),
            None,
            2 << 20,
        )? {
            (200, bytes) => Ok(bytes),
            (status, _) => Err(format!("League returned HTTP {status} for an icon.")),
        }
    }
}

/// Name and icon path of a rune, rune tree or summoner spell, in the
/// client's own language and art.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Asset {
    pub name: String,
    pub icon: String,
}

#[derive(Clone, Debug, Default)]
pub struct GameData {
    pub perks: HashMap<u64, Asset>,
    pub styles: HashMap<u64, Asset>,
    pub spells: HashMap<u64, Asset>,
}

impl GameData {
    pub fn is_empty(&self) -> bool {
        self.perks.is_empty() && self.styles.is_empty() && self.spells.is_empty()
    }
}

fn assets(list: Option<&Vec<Value>>) -> HashMap<u64, Asset> {
    list.into_iter()
        .flatten()
        .filter_map(|v| {
            let id = v["id"].as_u64()?;
            let icon = v["iconPath"].as_str()?.trim();
            if icon.is_empty() {
                return None;
            }
            Some((
                id,
                Asset {
                    name: v["name"].as_str().unwrap_or("").trim().into(),
                    icon: icon.into(),
                },
            ))
        })
        .collect()
}

/// Rune, rune-tree and spell metadata shipped with the installed client.
pub fn game_data(api: &dyn Api) -> GameData {
    let get = |path: &str| {
        api.call("GET", path, None)
            .and_then(success)
            .unwrap_or(Value::Null)
    };
    GameData {
        perks: assets(get("/lol-game-data/assets/v1/perks.json").as_array()),
        styles: assets(get("/lol-game-data/assets/v1/perkstyles.json")["styles"].as_array()),
        spells: assets(get("/lol-game-data/assets/v1/summoner-spells.json").as_array()),
    }
}

pub fn champion_icon(champion: u64) -> String {
    format!("/lol-game-data/assets/v1/champion-icons/{champion}.png")
}

pub fn remote(path: &str) -> Result<Value, String> {
    let r = http(PROVIDER, 443, "GET", path, None, None).map_err(|_| {
        "Cannot reach the recommendation service. Check your internet connection and retry."
            .to_string()
    })?;
    if r.status != 200 {
        return Err(format!(
            "The recommendation service returned HTTP {}. Your setup was not guessed.",
            r.status
        ));
    }
    r.body
        .get("data")
        .cloned()
        .ok_or("The recommendation service returned an unexpected response.".into())
}
pub fn discover(custom: &str) -> Option<Connection> {
    let mut paths = Vec::<PathBuf>::new();
    if !custom.trim().is_empty() {
        paths.push(PathBuf::from(custom).join("lockfile"));
    }
    for drive in ['C', 'D', 'E', 'F'] {
        paths.push(PathBuf::from(format!(
            "{drive}:/Riot Games/League of Legends/lockfile"
        )));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let riot = PathBuf::from(local).join("Riot Games/Riot Client/Config/lockfile");
        if let Some(c) = fs::read_to_string(riot)
            .ok()
            .and_then(|s| Connection::from_lockfile(&s))
        {
            if let Ok(r) = c.call("GET", "/product-session/v1/sessions", None) {
                if let Some(sessions) = r.body.as_object() {
                    for session in sessions
                        .values()
                        .filter(|s| s["productId"] == "league_of_legends")
                    {
                        if let Some(dir) = session
                            .pointer("/launchConfiguration/workingDirectory")
                            .and_then(Value::as_str)
                        {
                            paths.insert(0, PathBuf::from(dir).join("lockfile"));
                        }
                    }
                }
            }
        }
    }
    paths.into_iter().find_map(|p| {
        fs::read_to_string(p)
            .ok()
            .and_then(|s| Connection::from_lockfile(&s))
    })
}

fn agent(local: bool) -> &'static ureq::Agent {
    use std::{sync::Arc, time::Duration};
    static LOCAL: OnceLock<ureq::Agent> = OnceLock::new();
    static REMOTE: OnceLock<ureq::Agent> = OnceLock::new();
    let _ = rustls::crypto::ring::default_provider().install_default();
    let base = || {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_millis(3000))
            .timeout(Duration::from_millis(10000))
            .redirects(0)
            .user_agent(concat!("smb-lol/", env!("CARGO_PKG_VERSION")))
    };
    // Agents are reused so polling keeps one TLS connection alive instead of
    // handshaking every second.
    if local {
        LOCAL.get_or_init(|| {
            let config = rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(LoopbackCertificate))
                .with_no_client_auth();
            base()
                .try_proxy_from_env(false)
                .tls_config(Arc::new(config))
                .build()
        })
    } else {
        REMOTE.get_or_init(|| base().build())
    }
}

fn request(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&Value>,
    limit: usize,
) -> Result<(u32, Vec<u8>), String> {
    use std::io::Read;
    let localhost = host == "127.0.0.1";
    if token.is_some() && !localhost {
        return Err("Client credentials can only be sent to the local League client.".into());
    }
    let url = format!("https://{host}:{port}{path}");
    let mut request = agent(localhost)
        .request(method, &url)
        .set("Accept", "application/json")
        .set("Content-Type", "application/json");
    if let Some(token) = token {
        request = request.set(
            "Authorization",
            &format!("Basic {}", base64(format!("riot:{token}").as_bytes())),
        );
    }
    let response = match if let Some(body) = body {
        request.send_string(&body.to_string())
    } else {
        request.call()
    } {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(_) => {
            return Err("The service did not respond. Check the connection and retry.".into())
        }
    };
    let status = response.status() as u32;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The response was interrupted.")?;
    if bytes.len() > limit {
        return Err("The response exceeded the expected size.".into());
    }
    Ok((status, bytes))
}

fn http(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> Result<Reply, String> {
    let (status, bytes) = request(host, port, method, path, token, body, 8 << 20)?;
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) if status >= 400 => Value::Null,
            Err(_) => return Err("The service returned an unreadable response.".into()),
        }
    };
    Ok(Reply { status, body })
}

// This verifier is constructed only for literal 127.0.0.1, with redirects and proxies disabled.
// League's rotating local endpoint uses a self-signed certificate.
// Handshake signatures are still verified; remote recommendation TLS uses normal roots.
#[derive(Debug)]
struct LoopbackCertificate;
impl rustls::client::danger::ServerCertVerifier for LoopbackCertificate {
    fn verify_server_cert(
        &self,
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if name.to_str() != "127.0.0.1" {
            return Err(rustls::Error::General(
                "Loopback certificate verifier used for a non-local host".into(),
            ));
        }
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for b in bytes.chunks(3) {
        let n = ((b[0] as u32) << 16)
            | ((b.get(1).copied().unwrap_or(0) as u32) << 8)
            | b.get(2).copied().unwrap_or(0) as u32;
        out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        out.push(if b.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if b.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    struct Fake {
        replies: RefCell<VecDeque<Reply>>,
        calls: RefCell<Vec<(String, String, Option<Value>)>>,
    }
    impl Fake {
        fn new(items: Vec<(u32, Value)>) -> Self {
            Self {
                replies: RefCell::new(
                    items
                        .into_iter()
                        .map(|(status, body)| Reply { status, body })
                        .collect(),
                ),
                calls: RefCell::new(vec![]),
            }
        }
    }
    impl Api for Fake {
        fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Reply, String> {
            self.calls
                .borrow_mut()
                .push((method.into(), path.into(), body.cloned()));
            self.replies
                .borrow_mut()
                .pop_front()
                .ok_or("Unexpected extra client request".into())
        }
    }
    #[test]
    fn creates_and_confirms_only_its_own_page() {
        let page = json!({"primaryStyleId":8000,"subStyleId":8100,"selectedPerkIds":[1,2,3],"current":true});
        let api = Fake::new(vec![
            (
                200,
                json!([{"id":5,"name":"My personal page","isEditable":true}]),
            ),
            (200, json!({"id":12})),
            (200, page.clone()),
        ]);
        apply_runes(&api, &page).unwrap();
        assert_eq!(
            api.calls
                .borrow()
                .iter()
                .map(|c| c.0.as_str())
                .collect::<Vec<_>>(),
            vec!["GET", "POST", "GET"]
        );
        assert_eq!(api.calls.borrow()[2].1, "/lol-perks/v1/pages/12");
    }
    #[test]
    fn full_rune_inventory_preserves_personal_pages() {
        let api = Fake::new(vec![
            (200, json!([{"id":5,"name":"Personal","isEditable":true}])),
            (400, json!({"message":"maximum pages"})),
        ]);
        assert!(apply_runes(&api, &json!({}))
            .unwrap_err()
            .contains("Free one rune page slot"));
        assert!(api.calls.borrow().iter().all(|c| c.0 != "DELETE"));
    }
    #[test]
    fn updates_the_companion_page_without_creating_duplicates() {
        let page = json!({"primaryStyleId":8000,"subStyleId":8100,"selectedPerkIds":[1,2,3],"current":true});
        let api = Fake::new(vec![
            (
                200,
                json!([{"id":7,"name":"Quiet Select · 64 jungle","isEditable":true}]),
            ),
            (200, Value::Null),
            (200, page.clone()),
        ]);
        apply_runes(&api, &page).unwrap();
        assert_eq!(api.calls.borrow()[1].0, "PUT");
        assert_eq!(api.calls.borrow()[1].1, "/lol-perks/v1/pages/7");
    }
    #[test]
    fn accepted_write_is_not_success_if_readback_differs() {
        let api = Fake::new(vec![
            (204, Value::Null),
            (200, json!({"spell1Id":4,"spell2Id":7})),
        ]);
        assert!(apply_spells(&api, &json!({"spell1Id":4,"spell2Id":14})).is_err());
    }
    #[test]
    fn rune_readback_requires_the_requested_name_and_quick_play_bindings() {
        let page = json!({"name":"SMB // Lee Sin · Jungle","primaryStyleId":8000,
            "subStyleId":8100,"selectedPerkIds":[8005,9111,9104,8014,8139,8135,5005,5008,5002],
            "quickPlayChampionIds":[],"current":true});
        for (field, wrong) in [
            ("name", json!("Old numeric page")),
            ("quickPlayChampionIds", json!([99])),
        ] {
            let mut readback = page.clone();
            readback[field] = wrong;
            let api = Fake::new(vec![
                (200, json!([])),
                (200, json!({"id":12})),
                (200, readback),
            ]);
            assert!(
                apply_runes(&api, &page).is_err(),
                "Accepted mismatched {field}"
            );
        }
        // An omitted quickPlayChampionIds is the same as [].
        let mut readback = page.clone();
        readback
            .as_object_mut()
            .unwrap()
            .remove("quickPlayChampionIds");
        let api = Fake::new(vec![
            (200, json!([])),
            (200, json!({"id":12})),
            (200, readback),
        ]);
        apply_runes(&api, &page).unwrap();
    }
    #[test]
    fn confirms_spell_slots_with_readback() {
        let spells = json!({"spell1Id":14,"spell2Id":4});
        let api = Fake::new(vec![(204, Value::Null), (200, spells.clone())]);
        apply_spells(&api, &spells).unwrap();
        assert_eq!(api.calls.borrow()[0].0, "PATCH");
        assert_eq!(api.calls.borrow()[1].0, "GET");
    }
    #[test]
    fn rejected_spell_write_cannot_report_success() {
        let api = Fake::new(vec![(403, json!({}))]);
        assert!(apply_spells(&api, &json!({})).is_err());
        assert_eq!(api.calls.borrow().len(), 1);
    }
    #[test]
    fn strict_local_lockfile_parsing_and_basic_auth_encoding() {
        assert!(Connection::from_lockfile("LeagueClient:123:50001:dummy-token:https").is_some());
        for bad in [
            "LeagueClient:123:0:dummy:https",
            "LeagueClient:123:70000:dummy:https",
            "LeagueClient:123:12::https",
            "LeagueClient:123:12:dummy:http",
        ] {
            assert!(Connection::from_lockfile(bad).is_none());
        }
        assert_eq!(base64(b"riot:dummy"), "cmlvdDpkdW1teQ==");
    }
    #[test]
    fn flash_follows_preferred_key() {
        assert_eq!(
            ordered_spells(&[14, 4], "d"),
            Some(json!({"spell1Id":4,"spell2Id":14}))
        );
        assert_eq!(
            ordered_spells(&[4, 14], "f"),
            Some(json!({"spell1Id":14,"spell2Id":4}))
        );
    }
    #[test]
    fn no_flash_keeps_first_spell_preference() {
        assert_eq!(
            ordered_spells(&[6, 7], "d"),
            Some(json!({"spell1Id":6,"spell2Id":7}))
        );
        assert!(ordered_spells(&[4], "f").is_none());
        assert_eq!(
            ordered_spells(&[6, 7], "f"),
            Some(json!({"spell1Id":7,"spell2Id":6}))
        );
    }
    #[test]
    fn use_local_cell_and_selected_champion() {
        let v = json!({"localPlayerCellId":2,"myTeam":[{"cellId":1,"championId":99},{"cellId":2,"championId":64,"assignedPosition":"jungle"}]});
        assert_eq!(selection(&v), Some((64, "jungle".into())));
        assert!(selection(&json!({"localPlayerCellId":2,"myTeam":[{"cellId":2,"championId":0,"championPickIntent":99}]})).is_none());
    }
    #[test]
    fn rune_order_includes_shards() {
        let d = json!({"rune_pages":[{"builds":[{"primary_page_id":8000,"secondary_page_id":8100,"primary_rune_ids":[8005,9111,9104,8014],"secondary_rune_ids":[8139,8135],"stat_mod_ids":[5005,5008,5002]}]}]});
        let api = Fake::new(vec![(
            200,
            json!({"id":64,"name":"Lee Sin","alias":"LeeSin"}),
        )]);
        let name = champion_name(&api, 64);
        let p = rune_page(&d, &name, "jungle").unwrap();
        assert_eq!(
            p["selectedPerkIds"],
            json!([8005, 9111, 9104, 8014, 8139, 8135, 5005, 5008, 5002])
        );
        assert_eq!(p["primaryStyleId"], 8000);
        assert_eq!(p["name"], "SMB // Lee Sin · Jungle");
    }
    #[test]
    fn no_invented_builds_when_data_missing() {
        assert!(rune_page(&json!({}), "Lux", "mid").is_err());
    }
    #[test]
    fn names_use_client_locale_and_hide_missing_metadata() {
        let api = Fake::new(vec![(
            200,
            json!({"id":62,"name":" Wukong ","alias":"MonkeyKing"}),
        )]);
        assert_eq!(champion_name(&api, 62), "Wukong");
        assert_eq!(
            api.calls.borrow()[0].1,
            "/lol-game-data/assets/v1/champions/62.json"
        );
        for (status, body) in [
            (404, json!({})),
            (200, json!({"name":" "})),
            (200, json!({"alias":"MonkeyKing"})),
        ] {
            assert_eq!(
                champion_name(&Fake::new(vec![(status, body)]), 62),
                "Your champion"
            );
        }
        assert_eq!(champion_label("Kai'Sa", "none"), "Kai'Sa");
        assert_eq!(champion_label("Kai'Sa", "adc"), "Kai'Sa · Bot");
    }
    #[test]
    fn adopts_legacy_pages_but_never_personal_ones() {
        assert!(is_own_page("SMB // Lee Sin · Jungle"));
        assert!(is_own_page("Quiet Select · Lee Sin · Jungle"));
        for personal in ["SMB", "SMB//Lee", "My SMB page", "Quiet Select", ""] {
            assert!(!is_own_page(personal), "{personal}");
        }
    }
    #[test]
    fn game_data_reads_client_metadata() {
        let api = Fake::new(vec![
            (
                200,
                json!([{"id":8005,"name":"Press the Attack","iconPath":"/lol-game-data/assets/v1/perk-images/pta.png"},{"id":1,"name":"No icon","iconPath":""}]),
            ),
            (
                200,
                json!({"styles":[{"id":8000,"name":"Precision","iconPath":"/lol-game-data/assets/v1/perk-images/Styles/7201_Precision.png"}]}),
            ),
            (404, json!({})),
        ]);
        let data = game_data(&api);
        assert_eq!(data.perks[&8005].name, "Press the Attack");
        assert!(!data.perks.contains_key(&1));
        assert_eq!(data.styles[&8000].name, "Precision");
        assert!(data.spells.is_empty());
        assert_eq!(
            champion_icon(64),
            "/lol-game-data/assets/v1/champion-icons/64.png"
        );
    }
    #[test]
    fn modes_follow_queue_rules() {
        assert_eq!(mode(450, ""), ("aram", "emerald_plus", "none"));
        assert_eq!(mode(1900, ""), ("urf", "gold_plus", "none"));
        assert_eq!(mode(420, ""), ("ranked", "emerald_plus", ""));
    }
    #[test]
    fn jade_uses_mode_specific_spells_and_champion_ids() {
        assert_eq!(recommendation_champion(60099, 4310, ""), 99);
        assert_eq!(recommendation_champion(60099, 4300, ""), 60099);
        assert_eq!(recommendation_champion(60099, 4300, "JADE"), 99);
        assert_eq!(
            jade_spells("JUNGLE", "d"),
            json!({"spell1Id":74,"spell2Id":711})
        );
        assert_eq!(
            jade_spells("mid", "f"),
            json!({"spell1Id":77,"spell2Id":74})
        );
    }
    #[test]
    fn mode_is_read_from_the_map_field() {
        assert_eq!(
            game_mode(&json!({"map":{"gameMode":"KIWI"},"gameData":{"gameMode":"CLASSIC"}})),
            "KIWI"
        );
        assert!(is_tft(6120));
        assert!(!is_tft(420));
    }
    #[test]
    fn kiwi_excludes_exhaust_for_map_and_queue() {
        let data = json!({"summoner_spells":[{"ids":[3,4]},{"ids":[4,7]}]});
        let expected = Some(json!({"spell1Id":4,"spell2Id":7}));
        assert_eq!(recommended_spells(&data, 450, "KIWI", "d"), expected);
        assert_eq!(recommended_spells(&data, 2400, "", "d"), expected);
        assert_eq!(
            recommended_spells(&data, 450, "ARAM", "d"),
            Some(json!({"spell1Id":4,"spell2Id":3}))
        );
    }
    #[test]
    fn intent_and_actions_follow_queue_lock_rules() {
        let mut session = json!({"localPlayerCellId":2,"myTeam":[{"cellId":2,"championId":0,"championPickIntent":99}],"actions":[[{"type":"pick","actorCellId":2,"championId":64,"completed":false}]]});
        assert_eq!(selection_for_queue(&session, 490), Some((99, "".into())));
        assert_eq!(selection_for_queue(&session, 420), None);
        session["actions"][0][0]["completed"] = json!(true);
        assert_eq!(selection_for_queue(&session, 420), Some((64, "".into())));
        session["myTeam"][0]["championId"] = json!(1);
        assert_eq!(selection_for_queue(&session, 420), Some((1, "".into())));
    }
    #[test]
    fn rune_builds_are_flattened_before_first_selection() {
        let data = json!({"rune_pages":[{"builds":[]},{"builds":[{"primary_page_id":8000,"secondary_page_id":8100,"primary_rune_ids":[8005,9111,9104,8014],"secondary_rune_ids":[8139,8135],"stat_mod_ids":[5005,5008,5002]}]}]});
        assert_eq!(
            rune_page(&data, "Lux", "mid").unwrap()["primaryStyleId"],
            8000
        );
    }
    #[test]
    fn summaries_use_top_fallback_and_validate_classic_role() {
        let summary = json!({"positions":[{"name":"MID"},{"name":"TOP"}]});
        assert_eq!(recommended_lane(None, "", "ranked"), "top");
        assert_eq!(recommended_lane(Some(&summary), "", "ranked"), "mid");
        assert_eq!(recommended_lane(Some(&summary), "jungle", "classic"), "mid");
        assert_eq!(recommended_lane(Some(&summary), "top", "classic"), "top");
        assert_eq!(
            recommended_lane(
                Some(&json!({"is_rip":true,"positions":[{"name":"MID"}]})),
                "",
                "ranked"
            ),
            "top"
        );
    }
    #[test]
    fn recommendation_requests_use_mode_specific_routes() {
        assert_eq!(
            recommendation_path("arena", 99, "none", ""),
            "/api/GLOBAL/champions/arena/99/?tier=&version="
        );
        assert_eq!(
            recommendation_path("aram", 99, "none", "emerald_plus"),
            "/api/GLOBAL/champions/aram/99/none?tier=emerald_plus&version="
        );
        assert!(recommendation_path("classic", 99, "mid", "all").ends_with("&hl=en_US"));
    }
}
