// Lueur — la lumière qui danse avec ta musique. Cœur en Rust (Tauri 2).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    fs,
    path::PathBuf,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::Engine;
use serde_json::{json, Value};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WindowEvent,
};


struct Shared {
    state: Mutex<Value>,
    art: Mutex<String>,
    settings: Mutex<Value>,
    settings_path: PathBuf,
    cmd: Mutex<mpsc::Sender<String>>,
    sources: Mutex<Value>,
    preferred: Mutex<String>,
}
type Sh<'a> = State<'a, Arc<Shared>>;

/// Dossier des réglages : %APPDATA%\Lueur (Windows), ~/Library/Application Support/Onda (Mac), ~/.config/Onda (Linux).
fn data_dir() -> PathBuf {
    if cfg!(windows) {
        return std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(".")).join("Lueur");
    }
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        return home.join("Library/Application Support/Onda");
    }
    std::env::var("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|_| home.join(".config")).join("Onda")
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn default_settings() -> Value {
    json!({ "light": true, "lyrics": true, "sparks": true, "glow": 70, "intensity": 70, "video": true })
}

fn update_settings(app: &AppHandle, sh: &Shared, patch: &Value) {
    let merged = {
        let mut s = sh.settings.lock().unwrap();
        if let (Some(obj), Some(p)) = (s.as_object_mut(), patch.as_object()) {
            for (k, v) in p {
                obj.insert(k.clone(), v.clone());
            }
        }
        s.clone()
    };
    let _ = fs::write(&sh.settings_path, merged.to_string());
    let _ = app.emit("settings", &merged);
}

// ---------- Commandes appelées par les écrans ----------

#[tauri::command]
fn init(sh: Sh) -> Value {
    json!({
        "settings": *sh.settings.lock().unwrap(),
        "state": *sh.state.lock().unwrap(),
        "art": *sh.art.lock().unwrap(),
        "sources": *sh.sources.lock().unwrap(),
        "source": *sh.preferred.lock().unwrap(),
        "demo": std::env::var("LUEUR_DEMO").unwrap_or_default(),
    })
}

#[tauri::command]
fn set_settings(app: AppHandle, sh: Sh, patch: Value) {
    update_settings(&app, &sh, &patch);
}

/// Choisit l'app de musique à suivre (vide = automatique).
#[tauri::command]
fn set_source(app: AppHandle, sh: Sh, id: String) {
    *sh.preferred.lock().unwrap() = id.clone();
    update_settings(&app, &sh, &json!({ "source": id }));
    let _ = sh.cmd.lock().unwrap().send("refresh".into());
}

#[tauri::command]
fn media_cmd(sh: Sh, name: String) {
    let _ = sh.cmd.lock().unwrap().send(name);
}

/// Requêtes web limitées aux deux services utilisés : paroles et albums.
#[tauri::command(async)]
fn http_get(url: String) -> Result<String, String> {
    if !(url.starts_with("https://lrclib.net/api/") || url.starts_with("https://itunes.apple.com/search?") || url.starts_with("https://api.deezer.com/")) {
        return Err("adresse refusée".into());
    }
    ureq::get(&url)
        .set("User-Agent", "Lueur/1.0")
        .timeout(Duration::from_secs(8))
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())
}

/// Cherche le clip du morceau et renvoie quelques identifiants de vidéos.
fn video_search_once(q: &str) -> Vec<Value> {
    let url = format!("https://www.youtube.com/results?search_query={}", enc(q));
    let res = ureq::get(&url)
        .set("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36")
        .set("Cookie", "SOCS=CAI; CONSENT=YES+1")
        .set("Accept-Language", "fr")
        .timeout(Duration::from_secs(8))
        .call();
    let Ok(resp) = res else { return vec![] };
    let Ok(html) = resp.into_string() else { return vec![] };
    let marker = "\"videoRenderer\":{\"videoId\":\"";
    let mut out: Vec<Value> = Vec::new();
    let mut rest = html.as_str();
    while let Some(i) = rest.find(marker) {
        rest = &rest[i + marker.len()..];
        let Some(id) = rest.get(..11) else { break };
        if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            continue;
        }
        let mut n = rest.len().min(6000);
        while !rest.is_char_boundary(n) {
            n -= 1;
        }
        let seg = &rest[..n];
        let title = json_str_after(seg, "\"title\":{\"runs\":[{\"text\":\"").unwrap_or_default();
        let secs = seg
            .find("\"lengthText\"")
            .and_then(|k| json_str_after(&seg[k..], "\"simpleText\":\""))
            .map(|t| t.split(':').fold(0u64, |a, p| a * 60 + p.trim().parse::<u64>().unwrap_or(0)))
            .unwrap_or(0);
        let views = seg
            .find("\"viewCountText\"")
            .and_then(|k| json_str_after(&seg[k..], "\"simpleText\":\""))
            .map(|t| t.chars().filter(|c| c.is_ascii_digit()).collect::<String>().parse::<u64>().unwrap_or(0))
            .unwrap_or(0);
        let channel = seg.find("\"ownerText\"").and_then(|k| json_str_after(&seg[k..], "\"text\":\"")).unwrap_or_default();
        if !out.iter().any(|x| x["id"] == id) {
            out.push(json!({ "id": id, "title": title, "secs": secs, "views": views, "channel": channel }));
        }
        if out.len() >= 10 {
            break;
        }
    }
    // YouTube affiche parfois ses résultats dans un autre format : on prend alors les vidéos citées dans la page, dans l'ordre.
    if out.is_empty() {
        let key = "\"videoId\":\"";
        let mut rest = html.as_str();
        while let Some(k) = rest.find(key) {
            rest = &rest[k + key.len()..];
            if let Some(id) = rest.get(..11) {
                if id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') && !out.iter().any(|x| x["id"] == id) {
                    out.push(json!({ "id": id, "title": "", "secs": 0 }));
                }
            }
            if out.len() >= 8 {
                break;
            }
        }
    }
    out
}

/// Cherche le clip d'un titre ; si YouTube renvoie une page vide, on réessaie une fois.
#[tauri::command(async)]
fn video_search(q: String) -> Vec<Value> {
    let first = video_search_once(&q);
    if !first.is_empty() {
        return first;
    }
    thread::sleep(Duration::from_millis(600));
    video_search_once(&q)
}

/// Lit la chaîne JSON qui suit `key` dans un extrait de page.
fn json_str_after(s: &str, key: &str) -> Option<String> {
    let start = s.find(key)? + key.len();
    let bytes = s.as_bytes();
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'"' {
            break;
        }
        i += 1;
    }
    let raw = s.get(start..i)?;
    serde_json::from_str::<String>(&format!("\"{raw}\"")).ok()
}

#[tauri::command]
fn open_url(url: String) {
    const APPS: [&str; 6] = [
        "spotify:", "https://music.apple.com/", "https://music.youtube.com/",
        "https://soundcloud.com/", "https://www.deezer.com/", "https://music.amazon.com/",
    ];
    let ok = APPS.contains(&url.as_str())
        || url.starts_with("spotify:search:")
        || url.strip_prefix("spotify:track:").is_some_and(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()))
        || (url.starts_with("music://music.apple.com/") && !url.contains(' '))
        || url.strip_prefix("deezer://www.deezer.com/track/").is_some_and(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
        || url.starts_with("https://music.youtube.com/search?q=")
        || url.starts_with("https://music.apple.com/");
    if ok {
        let _ = open::that(url);
    }
}

/// Le fond d'écran Windows, pour le voir derrière le verre.
#[tauri::command(async)]
fn wallpaper() -> String {
    let Ok(appdata) = std::env::var("APPDATA") else { return String::new() };
    let p = PathBuf::from(appdata).join("Microsoft/Windows/Themes/TranscodedWallpaper");
    match fs::read(p) {
        Ok(b) if !b.is_empty() => {
            let mime = if b[0] == 0x89 { "image/png" } else { "image/jpeg" };
            format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(b))
        }
        _ => String::new(),
    }
}

// ---------- Services de musique intégrés : leur page vit dans une fenêtre de l'app ----------

const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,HardwareMediaKeyHandling --ignore-gpu-blocklist --enable-gpu-rasterization --autoplay-policy=no-user-gesture-required";
#[allow(dead_code)]
const EDGE_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0";
static CUR_SERVICE: Mutex<String> = Mutex::new(String::new());

fn service_home(id: &str) -> Option<&'static str> {
    Some(match id {
        "spotify" => "https://open.spotify.com",
        "apple" => "https://music.apple.com",
        "ytmusic" => "https://music.youtube.com",
        "soundcloud" => "https://soundcloud.com",
        "deezer" => "https://www.deezer.com",
        "amazon" => "https://music.amazon.fr",
        _ => return None,
    })
}
fn service_login(id: &str) -> Option<&'static str> {
    Some(match id {
        "spotify" => "https://accounts.spotify.com/login?continue=https%3A%2F%2Fopen.spotify.com%2F",
        "deezer" => "https://www.deezer.com/login",
        "ytmusic" => "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fmusic.youtube.com%2F",
        "soundcloud" => "https://soundcloud.com/signin",
        "apple" => "https://music.apple.com/",
        "amazon" => "https://music.amazon.fr/",
        _ => return None,
    })
}
fn service_search(id: &str, q: &str) -> Option<String> {
    let e = enc(q);
    Some(match id {
        "spotify" | "deezer" | "amazon" => format!("/search/{e}"),
        "apple" => format!("/fr/search?term={e}"),
        "ytmusic" | "soundcloud" => format!("/search?q={e}"),
        _ => return None,
    })
}
fn place_player(app: &AppHandle, w: &tauri::WebviewWindow) {
    if let Some(scene) = app.get_webview_window("scene") {
        if let Ok(Some(m)) = scene.primary_monitor() {
            let (p, s) = (m.position(), m.size());
            // petite fenêtre centrée : elle ne sert qu'à se connecter
            let ww = ((s.width as f64 * 0.30) as u32).max(560).min(s.width);
            let hh = (s.height as f64 * 0.70) as u32;
            let _ = w.set_position(PhysicalPosition::new(p.x + ((s.width - ww) / 2) as i32, p.y + (s.height as f64 * 0.145) as i32));
            let _ = w.set_size(PhysicalSize::new(ww, hh));
        }
    }
}

/// Script chargé dans la page du service : il exécute la demande de lecture laissée par l'app (recherche + lancement), sans rien afficher.
const PLAYER_INIT: &str = r#"(function () {
  if (window.top !== window) return;
  // SoundCloud : garde l'identifiant que le site utilise pour ses propres requêtes
  if (location.hostname === 'soundcloud.com') {
    var grab = function (u) { try { var x = /[?&]client_id=([A-Za-z0-9]+)/.exec(String(u)); if (x) window.__scId = x[1]; } catch (e) {} };
    var of2 = window.fetch; window.fetch = function (input) { grab(input && input.url ? input.url : input); return of2.apply(this, arguments); };
    var oo = XMLHttpRequest.prototype.open; XMLHttpRequest.prototype.open = function (mth, u) { grab(u); return oo.apply(this, arguments); };
  }
  // Spotify : garde le jeton que le lecteur web utilise déjà pour lire le compte de la personne connectée.
  if (location.hostname === 'open.spotify.com') {
    var of = window.fetch;
    window.fetch = function (input, init) {
      try {
        var h = (init && init.headers) || (input && input.headers);
        var auth = h && (h.get ? h.get('authorization') : (h.authorization || h.Authorization));
        if (auth && /^Bearer /i.test(auth) && auth !== window.__spToken) {
          window.__spToken = auth; window.__spCached = false;
          try { localStorage.setItem('ondaTok', JSON.stringify({ t: auth, at: Date.now() })); } catch (e) {}
        }
      } catch (e) {}
      return of.apply(this, arguments);
    };
    try {
      var c = JSON.parse(localStorage.getItem('ondaTok') || 'null');
      if (c && c.t && Date.now() - c.at < 50 * 60 * 1000) { window.__spToken = c.t; window.__spCached = true; }
    } catch (e) {}
  }
  function act() {
    var n = window.name || '';
    if (n.indexOf('vesper:') !== 0) return;
    var a; try { a = JSON.parse(n.slice(7)); } catch (e) { return; }
    window.name = '';
    var h = location.hostname;
    var say = function (t) { if (!a.dbg) return; var d = document.getElementById('vesperDbg'); if (!d) { d = document.createElement('div'); d.id = 'vesperDbg'; d.style.cssText = 'position:fixed;left:0;top:0;right:0;z-index:999999;background:#b00;color:#fff;font:14px Consolas;padding:6px;white-space:pre-wrap'; document.documentElement.appendChild(d); } d.textContent += t + ' | '; };
    say('act ' + h);
    var until = function (fn, tries) { return new Promise(function (res) { var t = setInterval(function () { var v = null; try { v = fn(); } catch (e) {} if (v || --tries <= 0) { clearInterval(t); res(v); } }, 500); }); };
    if (h === 'music.apple.com') {
      until(function () { return window.MusicKit && MusicKit.getInstance && MusicKit.getInstance(); }, 40).then(async function (m) {
        say('musickit=' + !!m + ' global=' + (typeof window.MusicKit) + ' auth=' + (m && m.isAuthorized));
        if (!m) return;
        try {
        var r = await m.api.music('/v1/catalog/' + m.storefrontId + '/search', { term: a.q, types: 'songs', limit: 1 });
        var s = r && r.data && r.data.results && r.data.results.songs && r.data.results.songs.data[0];
        say('song=' + (s && s.id));
        if (!s) return;
        if (a.add) {   // ajoute le titre à la bibliothèque du compte, sans rien lire
          try { await m.api.music('/v1/me/library', { 'ids[songs]': s.id }, { fetchOptions: { method: 'POST' } }); say('ajout ok'); }
          catch (e) { say('ajout ERR ' + (e && (e.message || e))); }
          return;
        }
        try {
          await m.setQueue({ song: s.id }); say('file ok');
          await m.play(); say('play ok');
          await sleep(5000); say('etat=' + m.playbackState + ' position=' + Math.round(m.currentPlaybackTime || 0) + 's');
          await sleep(5000); say('etat=' + m.playbackState + ' position=' + Math.round(m.currentPlaybackTime || 0) + 's');
        } catch (e) { say('ERR ' + (e && (e.errorCode || e.message || e))); }
        say('queued state=' + m.playbackState);
        } catch (e) { say('ERR ' + (e && (e.message || e))); }
      });
    } else if (h === 'open.spotify.com') {
      until(function () { return document.querySelector('[data-testid="tracklist-row"]'); }, 40).then(function (row) { if (row) row.dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); });
    } else if (h === 'soundcloud.com') {
      until(function () { return document.querySelector('.searchList .sc-button-play, .soundList .sc-button-play'); }, 40).then(function (b) { if (b) b.click(); });
    } else if (h.indexOf('music.amazon') === 0) {
      until(function () { return document.querySelector('music-horizontal-item, music-image-row'); }, 40).then(function (b) { if (b) b.dispatchEvent(new MouseEvent('dblclick', { bubbles: true })); });
    }
  }
  var send = function (out) { location.hash = 'vesper=' + encodeURIComponent(JSON.stringify(out)); };
  var sleep = function (ms) { return new Promise(function (r) { setTimeout(r, ms); }); };
  // --- Spotify ---
  var spImg = function (imgs) { imgs = imgs || []; return (imgs[1] || imgs[0] || {}).url || ''; };
  var spTrack = function (t) { return t && t.name ? { id: t.id || '', ms: t.duration_ms || 0, n: t.name, a: (t.artists || []).map(function (x) { return x.name; }).join(', '), al: (t.album || {}).name || '', art: spImg((t.album || {}).images), t: 'songs' } : null; };
  var spGet = async function (path) {
    for (var i = 0; i < 200 && !window.__spToken; i++) await sleep(100);
    if (!window.__spToken) throw 'jeton';
    var r = await fetch('https://api.spotify.com' + path, { headers: { Authorization: window.__spToken } });
    if (r.status === 401 && window.__spCached) {   // jeton gardé périmé : on attend celui du lecteur web
      window.__spToken = null; window.__spCached = false; try { localStorage.removeItem('ondaTok'); } catch (e) {}
      return spGet(path);
    }
    if (r.status === 401 || r.status === 403) throw 'connexion';
    if (!r.ok) throw 'erreur ' + r.status;
    return r.json();
  };
  var full = { started: false, done: false, songs: [] };
  var spFetch = async function () {
    var out = { ok: false, added: [], played: [], songs: [], lists: [], why: '', partial: true };
    try {
      var r = await Promise.all([spGet('/v1/me/tracks?limit=50'), spGet('/v1/me/playlists?limit=50'), spGet('/v1/me/player/recently-played?limit=30').catch(function () { return { items: [] }; })]);
      var liked = r[0];
      out.songs = (liked.items || []).map(function (i) { return spTrack(i.track); }).filter(Boolean);
      out.added = out.songs.slice(0, 25);
      out.lists = (r[1].items || []).filter(Boolean).map(function (p) { return { id: p.id, n: p.name || '', a: (p.owner || {}).display_name || '', art: spImg(p.images), t: 'playlists' }; });
      out.played = (r[2].items || []).map(function (i) { return spTrack(i.track); }).filter(Boolean);
      out.ok = true;
      if (!full.started) {
        full.started = true;
        (async function () {
          var total = Math.min(liked.total || 0, 2000), offs = [];
          for (var o = 50; o < total; o += 50) offs.push(o);
          var all = out.songs.slice();
          for (var k = 0; k < offs.length; k += 5) {
            var pages = await Promise.all(offs.slice(k, k + 5).map(function (o) { return spGet('/v1/me/tracks?limit=50&offset=' + o).catch(function () { return { items: [] }; }); }));
            pages.forEach(function (pg) { all = all.concat((pg.items || []).map(function (i) { return spTrack(i.track); }).filter(Boolean)); });
          }
          full.songs = all; full.done = true;
        })();
      }
    } catch (e) { out.why = String(e); }
    send(out);
  };
  var spList = async function (id) {
    var out = { ok: false, tracks: [] };
    try { out.tracks = ((await spGet('/v1/playlists/' + id + '/tracks?limit=100')).items || []).map(function (i) { return spTrack(i.track); }).filter(Boolean); out.ok = true; } catch (e) {}
    send(out);
  };
  // --- Apple Music ---
  var amFetch = async function () {
    var m = window.MusicKit && MusicKit.getInstance && MusicKit.getInstance();
    var out = { ok: false, added: [], played: [], songs: [], lists: [], why: '', partial: true };
    if (!m) out.why = 'pas-pret';
    else if (!m.isAuthorized) out.why = 'connexion';
    else {
      var pick = function (r) { return ((r && r.data && r.data.data) || []).map(function (x) { var a = x.attributes || {}; return { id: x.id || '', ms: a.durationInMillis || 0, n: a.name || '', a: a.artistName || '', al: a.albumName || '', art: a.artwork && a.artwork.url ? a.artwork.url.replace('{w}', '300').replace('{h}', '300') : '', t: x.type || '' }; }).filter(function (x) { return x.n; }); };
      var get = async function (p, q) { try { return pick(await m.api.music(p, q)); } catch (e) { return []; } };
      var r = await Promise.all([get('/v1/me/library/recently-added', { limit: 25 }), get('/v1/me/recent/played/tracks', { limit: 20 }),
        get('/v1/me/library/songs', { limit: 100 }), get('/v1/me/library/playlists', { limit: 100 })]);
      out.added = r[0]; out.played = r[1]; out.songs = r[2]; out.lists = r[3]; out.ok = true;
      if (!full.started) {
        full.started = true;
        (async function () {
          var all = out.songs.slice();
          if (all.length >= 100) {
            for (var base = 100; base < 2000; base += 500) {
              var pages = await Promise.all([0, 100, 200, 300, 400].map(function (d) { return get('/v1/me/library/songs', { limit: 100, offset: base + d }); }));
              var got = 0; pages.forEach(function (pg) { all = all.concat(pg); got += pg.length; });
              if (got < 500) break;
            }
          }
          full.songs = all; full.done = true;
        })();
      }
    }
    send(out);
  };
  var amList = async function (id, kind) {
    var m = window.MusicKit && MusicKit.getInstance && MusicKit.getInstance();
    var out = { ok: false, tracks: [] };
    if (m && m.isAuthorized) {
      try {
        var r = await m.api.music('/v1/me/library/' + (kind === 'al' ? 'albums' : 'playlists') + '/' + id + '/tracks', { limit: 100 });
        out.tracks = ((r && r.data && r.data.data) || []).map(function (x) { var a = x.attributes || {}; return { ms: a.durationInMillis || 0, n: a.name || '', a: a.artistName || '', al: a.albumName || '', art: a.artwork && a.artwork.url ? a.artwork.url.replace('{w}', '300').replace('{h}', '300') : '' }; }).filter(function (x) { return x.n; });
        out.ok = true;
      } catch (e) {}
    }
    send(out);
  };
  // --- YouTube Music ---
  var ytHash = async function () {
    var m = document.cookie.match(/(?:^|; )(?:SAPISID|__Secure-3PAPISID)=([^;]+)/);
    if (!m) return '';
    var ts = Math.floor(Date.now() / 1000);
    var buf = await crypto.subtle.digest('SHA-1', new TextEncoder().encode(ts + ' ' + m[1] + ' ' + location.origin));
    return 'SAPISIDHASH ' + ts + '_' + Array.from(new Uint8Array(buf)).map(function (b) { return b.toString(16).padStart(2, '0'); }).join('');
  };
  var ytBrowse = async function (id) {
    for (var i = 0; i < 40 && !(window.ytcfg && ytcfg.get && ytcfg.get('INNERTUBE_CONTEXT')); i++) await sleep(500);
    if (!(window.ytcfg && ytcfg.get)) throw 'pas-pret';
    var auth = await ytHash(); if (!auth) throw 'connexion';
    var r = await fetch('/youtubei/v1/browse?prettyPrint=false', { method: 'POST', credentials: 'include',
      headers: { 'Content-Type': 'application/json', 'Authorization': auth, 'X-Origin': location.origin, 'X-Goog-AuthUser': '0' },
      body: JSON.stringify({ context: ytcfg.get('INNERTUBE_CONTEXT'), browseId: id }) });
    if (r.status === 401 || r.status === 403) throw 'connexion';
    if (!r.ok) throw 'erreur ' + r.status;
    return r.json();
  };
  var ytWalk = function (o, out) {
    if (!o || typeof o !== 'object') return;
    if (Array.isArray(o)) { for (var i = 0; i < o.length; i++) ytWalk(o[i], out); return; }
    if (o.musicResponsiveListItemRenderer) out.items.push(o.musicResponsiveListItemRenderer);
    if (o.musicTwoRowItemRenderer) out.cards.push(o.musicTwoRowItemRenderer);
    for (var k in o) if (k !== 'musicResponsiveListItemRenderer' && k !== 'musicTwoRowItemRenderer') ytWalk(o[k], out);
  };
  var ytText = function (c) { return ((((c || {}).musicResponsiveListItemFlexColumnRenderer || {}).text || {}).runs || []).map(function (r) { return r.text; }).join(''); };
  var ytThumb = function (t) { var th = ((((t || {}).musicThumbnailRenderer || {}).thumbnail || {}).thumbnails) || []; return (th[th.length - 1] || {}).url || ''; };
  var ytTrack = function (r) {
    var c = r.flexColumns || [], n = ytText(c[0]); if (!n) return null;
    var vid = (r.playlistItemData || {}).videoId || '';
    if (!vid) { try { vid = r.overlay.musicItemThumbnailOverlayRenderer.content.musicPlayButtonRenderer.playNavigationEndpoint.watchEndpoint.videoId; } catch (e) {} }
    return { id: vid, vid: vid, ms: 0, n: n, a: ytText(c[1]).split(' • ')[0], al: ytText(c[2]), art: ytThumb(r.thumbnail), t: 'songs' };
  };
  var ytCard = function (r) {
    var id = ((r.navigationEndpoint || {}).browseEndpoint || {}).browseId || '';
    var n = ((r.title || {}).runs || []).map(function (x) { return x.text; }).join('');
    return id.indexOf('VL') === 0 && n ? { id: id, n: n, a: '', art: ytThumb(r.thumbnailRenderer), t: 'playlists' } : null;
  };
  var ytGrab = async function (id) { var o = { items: [], cards: [] }; ytWalk(await ytBrowse(id), o); return o; };
  var ytFetch = async function () {
    var out = { ok: false, added: [], played: [], songs: [], lists: [], why: '' };
    try {
      out.songs = (await ytGrab('FEmusic_liked_videos')).items.map(ytTrack).filter(Boolean);
      out.added = out.songs.slice(0, 25);
      try { out.played = (await ytGrab('FEmusic_history')).items.map(ytTrack).filter(Boolean).slice(0, 30); } catch (e) {}
      out.lists = (await ytGrab('FEmusic_liked_playlists')).cards.map(ytCard).filter(Boolean);
      out.ok = true; full.started = true; full.done = true; full.songs = out.songs;
    } catch (e) { out.why = String(e); }
    send(out);
  };
  var ytList = async function (id) {
    var out = { ok: false, tracks: [] };
    try { out.tracks = (await ytGrab(id)).items.map(ytTrack).filter(Boolean); out.ok = true; } catch (e) {}
    send(out);
  };
  // L'app appelle ces deux fonctions ; la réponse est déposée dans l'adresse de la page, où l'app vient la lire.
  // mode « quick » : l'essentiel tout de suite ; mode « full » : le reste de la bibliothèque, quand il est prêt.
  window.__vesperFetch = function (mode) {
    if (mode === 'full') return send(full.done ? { ok: true, done: true, songs: full.songs } : { ok: true, done: false });
    if (location.hostname === 'open.spotify.com') return spFetch();
    if (location.hostname === 'music.youtube.com') return ytFetch();
    if (location.hostname === 'music.apple.com') return amFetch();
    send({ ok: false, why: 'non-pris-en-charge' });
  };
  // Lecture sur le compte Spotify : l'app Spotify ouverte sur le PC joue le titre (Premium nécessaire).
  window.__vesperSvc = async function (a) {
    var out = { ok: false, why: '' };
    try {
      if (location.hostname === 'soundcloud.com' && a.op === 'play') {   // trouve le titre : Onda le joue avec le lecteur officiel de SoundCloud
        for (var w = 0; w < 100 && !window.__scId; w++) await sleep(100);
        if (!window.__scId) throw 'jeton';
        var rq = await fetch('https://api-v2.soundcloud.com/search/tracks?limit=5&q=' + encodeURIComponent(a.q) + '&client_id=' + window.__scId);
        if (!rq.ok) throw 'erreur ' + rq.status;
        var hit = ((await rq.json()).collection || []).find(function (t) { return t.permalink_url && t.policy !== 'BLOCK' && t.policy !== 'SNIP'; });
        if (!hit) throw 'introuvable';
        out.url = hit.permalink_url; out.ms = hit.full_duration || hit.duration || 0; out.ok = true;
        return send(out);
      }
      // N'importe quel lecteur web (Deezer…) : ce qui joue, d'après la page elle-même
      var media = function () { return Array.prototype.slice.call(document.querySelectorAll('audio, video')).find(function (x) { return x.currentSrc || x.src; }); };
      if (a.op === 'mstate') {
        var el = media(), ms = navigator.mediaSession;
        out.playing = (ms && ms.playbackState === 'playing') || !!(el && !el.paused);
        out.title = (ms && ms.metadata && ms.metadata.title) || ''; out.pos = el ? Math.round(el.currentTime * 1000) : 0; out.ok = true;
        return send(out);
      }
      if (a.op === 'mtoggle') { var me = media(); if (me) { if (me.paused) me.play(); else me.pause(); } out.ok = !!me; return send(out); }
      // Apple Music joue DANS Onda : le lecteur web caché (MusicKit), là où la personne est connectée
      if (location.hostname === 'music.apple.com') {
        var mk = window.MusicKit && MusicKit.getInstance && MusicKit.getInstance();
        if (!mk) throw 'pas-pret';
        if (!mk.isAuthorized) throw 'jeton';
        if (a.op === 'state') { out.playing = !!mk.isPlaying; out.pos = Math.round((mk.currentPlaybackTime || 0) * 1000); out.ok = true; return send(out); }
        if (a.op === 'play') {
          var sf = mk.storefrontId || 'fr';
          var res = await mk.api.music('/v1/catalog/' + sf + '/search', { term: a.q, types: 'songs', limit: 1 });
          var song = (((res && res.data && res.data.results && res.data.results.songs) || {}).data || [])[0];
          if (!song) throw 'introuvable';
          await mk.setQueue({ song: song.id, startPlaying: true });
          try { await mk.play(); } catch (e) {}
          for (var q = 0; q < 50 && !mk.isPlaying; q++) await sleep(150);
          if (!mk.isPlaying) throw 'lecture';
          out.ok = true; return send(out);
        }
        if (a.op === 'pause') mk.pause();
        else if (a.op === 'resume') await mk.play();
        else if (a.op === 'seek') await mk.seekToTime((parseInt(a.q, 10) || 0) / 1000);
        out.ok = true; return send(out);
      }
      if (location.hostname !== 'open.spotify.com') throw 'non-pris-en-charge';
      if (a.op === 'state') {
        var rs = await fetch('https://api.spotify.com/v1/me/player', { headers: { Authorization: window.__spToken } });
        var js = rs.status === 200 ? await rs.json() : null;
        out.playing = !!(js && js.is_playing); out.pos = (js && js.progress_ms) || 0; out.id = (js && js.item && js.item.id) || ''; out.ok = true;
        return send(out);
      }
      // Spotify joue DANS Onda : le lecteur web caché (où la personne est connectée) est l'appareil de lecture
      var waitEl = async function (sel, ms) { for (var i = 0; i < ms / 100; i++) { var e = document.querySelector(sel); if (e) return e; await sleep(100); } return null; };
      var uiPlaying = function () { var b = document.querySelector('[data-testid="control-button-playpause"]'); return !!b && /pause/i.test(b.getAttribute('aria-label') || ''); };
      var ownDev = async function () {
        var devs = (await spGet('/v1/me/player/devices')).devices || [];
        return devs.find(function (d) { return /web player/i.test(d.name || ''); }) || null;
      };
      var call = async function (dev, m, path, body) {
        var r = await fetch('https://api.spotify.com/v1/me/player/' + path + (path.indexOf('?') < 0 ? '?' : '&') + 'device_id=' + encodeURIComponent(dev.id),
          { method: m, headers: { Authorization: window.__spToken, 'Content-Type': 'application/json' }, body: body ? JSON.stringify(body) : undefined });
        if (r.status === 403) throw 'premium';
        if (!r.ok && r.status !== 204) throw 'erreur ' + r.status;
      };
      var dev = null; try { dev = await ownDev(); } catch (e) {}
      if (a.op === 'play') {
        var found = (((await spGet('/v1/search?type=track&limit=1&q=' + encodeURIComponent(a.q))).tracks || {}).items || [])[0];
        if (!found) throw 'introuvable';
        out.id = found.id; out.uri = found.uri;
        if (dev) {   // Premium : on lance le titre sur le lecteur caché, sans rien toucher
          try { await call(dev, 'PUT', 'play', { uris: [found.uri] }); out.ok = true; out.how = 'api'; return send(out); }
          catch (e) { if (String(e) !== 'premium') out.apiErr = String(e); }
        }
        // Gratuit : on ouvre la page du titre dans le lecteur caché et on appuie sur lecture
        history.pushState({}, '', '/track/' + found.id);
        window.dispatchEvent(new PopStateEvent('popstate', { state: {} }));
        await sleep(400);
        var btn = await waitEl('[data-testid="action-bar-row"] [data-testid="play-button"]', 9000);
        if (!btn) throw 'page';
        btn.click();
        for (var k = 0; k < 50; k++) { if (uiPlaying()) { out.ok = true; out.how = 'page'; return send(out); } await sleep(150); }
        throw 'lecture';
      }
      if (a.op === 'pause' || a.op === 'resume') {
        try { if (!dev) throw 'premium'; await call(dev, 'PUT', a.op === 'pause' ? 'pause' : 'play'); }
        catch (e) {   // sans Premium : le bouton du lecteur caché
          var pp = document.querySelector('[data-testid="control-button-playpause"]');
          if (pp && (a.op === 'pause') === uiPlaying()) pp.click();
        }
      } else if (a.op === 'seek') {
        if (dev) await call(dev, 'PUT', 'seek?position_ms=' + (parseInt(a.q, 10) || 0));
      }
      out.ok = true;
    } catch (e) { out.why = String(e); }
    send(out);
  };
  window.__vesperFetchList = function (id, kind) {
    if (location.hostname === 'open.spotify.com') return spList(id);
    if (location.hostname === 'music.youtube.com') return ytList(id);
    return amList(id, kind);
  };
  window.__vesperAct = act;
  if (document.readyState === 'complete') act(); else window.addEventListener('load', act);
})();"#;

fn build_player(app: &AppHandle, url: tauri::WebviewUrl) -> Result<tauri::WebviewWindow, String> {
    let scene = app.get_webview_window("scene").ok_or("scène absente")?;
    let w = tauri::WebviewWindowBuilder::new(app, "player", url)
        .title("Onda – musique")
        .decorations(false)
        .shadow(true)
        .skip_taskbar(true)
        .visible(false)
        .initialization_script(PLAYER_INIT)
        .additional_browser_args(BROWSER_ARGS);
    #[cfg(target_os = "linux")]
    let w = w.user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36");
    // Mac : la fenêtre se présente comme Safari (FairPlay y est disponible) pour que Spotify et Apple Music l'acceptent
    #[cfg(target_os = "macos")]
    let w = w.user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15");
    let w = w
        .parent(&scene)
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?;
    place_player(app, &w);
    Ok(w)
}
fn go_to(w: &tauri::WebviewWindow, url: &str) {
    let _ = w.eval(&format!("location.href = {}", serde_json::to_string(url).unwrap_or_default()));
}

/// Ouvre (ou change) le service de musique dans la fenêtre intégrée.
#[tauri::command]
async fn open_service(app: AppHandle, id: String, show: bool, login: Option<bool>) -> Result<(), String> {
    let login = login.unwrap_or(false);
    let same = *CUR_SERVICE.lock().unwrap() == id;
    let w = match app.get_webview_window("player") {
        Some(w) => {
            if login {
                if let Some(u) = service_login(&id) {
                    go_to(&w, u);
                }
            } else if !same {
                if let Some(home) = service_home(&id) {
                    go_to(&w, &format!("{home}/"));
                }
            }
            w
        }
        None => {
            let url = if id == "test" {
                tauri::WebviewUrl::App("test.html".into())
            } else {
                let home = service_home(&id).ok_or("service inconnu")?;
                let first = if login { service_login(&id).unwrap_or(home).to_string() } else { format!("{home}/") };
                tauri::WebviewUrl::External(first.parse().map_err(|_| "adresse invalide".to_string())?)
            };
            build_player(&app, url)?
        }
    };
    *CUR_SERVICE.lock().unwrap() = id;
    if show {
        place_player(&app, &w);
        let _ = w.show();
        let _ = w.set_focus();
    }
    Ok(())
}

/// Envoie le lecteur intégré (caché) sur une page du service : c'est ainsi que l'app lance un titre.
#[tauri::command]
async fn player_open(app: AppHandle, id: String, path: String) -> Result<(), String> {
    if !path.starts_with('/') {
        return Err("chemin invalide".into());
    }
    let home = service_home(&id).ok_or("service inconnu")?;
    let url = format!("{home}{path}");
    match app.get_webview_window("player") {
        Some(w) => go_to(&w, &url),
        None => {
            build_player(&app, tauri::WebviewUrl::External(url.parse().map_err(|_| "adresse invalide".to_string())?))?;
        }
    }
    *CUR_SERVICE.lock().unwrap() = id;
    Ok(())
}

/// L'app choisit et lance elle-même le titre dans le lecteur caché du service.
#[tauri::command]
async fn player_play(app: AppHandle, id: String, q: String, add: Option<bool>) -> Result<(), String> {
    let home = service_home(&id).ok_or("service inconnu")?;
    let name = serde_json::to_string(&format!("vesper:{}", json!({ "q": q, "add": add.unwrap_or(false), "dbg": std::env::var("LUEUR_DEMO").is_ok() }))).unwrap_or_default();
    let target = match id.as_str() {
        "spotify" => format!("{home}/search/{}/tracks", enc(&q)),
        "soundcloud" => format!("{home}/search/sounds?q={}", enc(&q)),
        "amazon" => format!("{home}/search/{}", enc(&q)),
        _ => format!("{home}/"),
    };
    let w = match app.get_webview_window("player") {
        Some(w) => w,
        None => build_player(&app, tauri::WebviewUrl::External(format!("{home}/").parse().map_err(|_| "adresse invalide".to_string())?))?,
    };
    let js = if id == "apple" {
        // déjà sur la page : on lance directement ; sinon on y retourne et le script s'exécute au chargement
        format!("window.name = {name}; if (location.hostname === 'music.apple.com' && window.__vesperAct) window.__vesperAct(); else location.href = {};", serde_json::to_string(&target).unwrap_or_default())
    } else {
        format!("window.name = {name}; location.href = {};", serde_json::to_string(&target).unwrap_or_default())
    };
    let _ = w.eval(&js);
    *CUR_SERVICE.lock().unwrap() = id;
    Ok(())
}

/// Demande à la page du service la musique du compte, puis lit la réponse.
#[tauri::command]
async fn player_data(app: AppHandle, list: Option<String>, kind: Option<String>, mode: Option<String>) -> Result<String, String> {
    let mode = if mode.as_deref() == Some("full") { "full" } else { "quick" };
    let k = if kind.as_deref() == Some("al") { "al" } else { "pl" };
    let w = app.get_webview_window("player").ok_or("lecteur absent")?;
    let js = match &list {
        Some(id) if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) => format!("window.__vesperFetchList && window.__vesperFetchList('{id}', '{k}')"),
        _ => format!("window.__vesperFetch && window.__vesperFetch('{mode}')"),
    };
    let _ = w.eval(&js);
    for _ in 0..240 {
        thread::sleep(Duration::from_millis(120));
        if let Ok(u) = w.url() {
            if let Some(d) = u.fragment().and_then(|f| f.strip_prefix("vesper=")) {
                let out = dec(d);
                let _ = w.eval("history.replaceState(null, '', location.pathname + location.search)");
                return Ok(out);
            }
        }
    }
    Err("pas de réponse du compte".into())
}

/// Cherche une version plus récente d'Onda (fichier signé publié sur GitHub).
#[tauri::command]
async fn update_check(app: AppHandle) -> Result<Option<Value>, String> {
    use tauri_plugin_updater::UpdaterExt;
    let updater = app.updater().map_err(|e| e.to_string())?;
    match updater.check().await.map_err(|e| e.to_string())? {
        Some(u) => Ok(Some(json!({ "version": u.version, "notes": u.body.clone().unwrap_or_default() }))),
        None => Ok(None),
    }
}

/// Télécharge la nouvelle version, vérifie sa signature, l'installe et relance Onda.
#[tauri::command]
async fn update_install(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    let update = app.updater().map_err(|e| e.to_string())?.check().await.map_err(|e| e.to_string())?.ok_or("aucune mise à jour")?;
    update.download_and_install(|_, _| {}, || {}).await.map_err(|e| e.to_string())?;
    app.restart();
}

/// Commande de lecture envoyée au service connecté (lecture sur le compte de la personne).
#[tauri::command]
async fn svc_cmd(app: AppHandle, op: String, q: Option<String>) -> Result<String, String> {
    if !["play", "pause", "resume", "seek", "state", "mstate", "mtoggle"].contains(&op.as_str()) {
        return Err("commande inconnue".into());
    }
    let w = app.get_webview_window("player").ok_or("lecteur absent")?;
    let arg = json!({ "op": op, "q": q.unwrap_or_default() }).to_string();
    let _ = w.eval(&format!("window.__vesperSvc && window.__vesperSvc({arg})"));
    for _ in 0..120 {
        thread::sleep(Duration::from_millis(100));
        if let Ok(u) = w.url() {
            if let Some(d) = u.fragment().and_then(|f| f.strip_prefix("vesper=")) {
                let out = dec(d);
                let _ = w.eval("history.replaceState(null, '', location.pathname + location.search)");
                return Ok(out);
            }
        }
    }
    Err("pas de réponse".into())
}

#[cfg(not(windows))]
const UI_PORT: u16 = 47231;

/// L'app d'un service est-elle installée (son type de lien est connu de Windows) ? Évite la fenêtre « Choisir une application ».
#[tauri::command]
fn has_app(scheme: String) -> bool {
    if scheme.is_empty() || !scheme.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let reg = std::process::Command::new("reg")
            .args(["query", &format!("HKCR\\{scheme}"), "/ve"])
            .creation_flags(0x0800_0000)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if reg {
            return true;
        }
        // apps du Microsoft Store : leurs liens ne sont pas toujours déclarés au même endroit
        let pkg = match scheme.as_str() {
            "music" => "AppleInc.AppleMusicWin",
            "spotify" => "SpotifyAB.SpotifyMusic",
            "deezer" => "Deezer.",
            _ => return false,
        };
        let dir = std::env::var("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Packages")).ok();
        return dir
            .and_then(|d| fs::read_dir(d).ok())
            .map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().starts_with(pkg)))
            .unwrap_or(false);
    }
    #[allow(unreachable_code)]
    true
}

// Mac : le son d'Apple Music passe par l'app Musique (titres de ta bibliothèque)
#[tauri::command]
fn mac_music(op: String, q: Option<String>, artist: Option<String>) -> Result<String, String> {
    if !cfg!(target_os = "macos") {
        return Ok(json!({ "ok": false, "why": "systeme" }).to_string());
    }
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let q = q.unwrap_or_default();
    let body = match op.as_str() {
        "play" => format!(
            "set r to (every track of library playlist 1 whose name is \"{}\" and artist contains \"{}\")\nif r is {{}} then set r to (every track of library playlist 1 whose name contains \"{}\" and artist contains \"{}\")\nif r is {{}} then return \"absent\"\nplay item 1 of r\nreturn \"ok\"",
            esc(&q), esc(&artist.clone().unwrap_or_default()), esc(&q), esc(&artist.unwrap_or_default())
        ),
        "state" => "return \"st|\" & (player position as text) & \"|\" & (player state as text)".into(),
        "pause" => "pause\nreturn \"ok\"".into(),
        "resume" => "play\nreturn \"ok\"".into(),
        "seek" => format!("set player position to {}\nreturn \"ok\"", q.parse::<f64>().unwrap_or(0.0) / 1000.0),
        _ => return Err("commande inconnue".into()),
    };
    let script = format!("tell application \"Music\"\n{body}\nend tell");
    let out = std::process::Command::new("osascript").arg("-e").arg(script).output().map_err(|e| e.to_string())?;
    let txt = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if let Some(rest) = txt.strip_prefix("st|") {
        let mut it = rest.split('|');
        let pos = it.next().unwrap_or("0").replace(',', ".").parse::<f64>().unwrap_or(0.0);
        let playing = it.next().unwrap_or("") == "playing";
        return Ok(json!({ "ok": true, "pos": (pos * 1000.0) as i64, "playing": playing }).to_string());
    }
    Ok(if txt == "ok" { json!({ "ok": true }) } else { json!({ "ok": false, "why": if txt.is_empty() { "refus" } else { txt.as_str() } }) }.to_string())
}

#[tauri::command]
fn player_show(app: AppHandle, show: bool) {
    if let Some(w) = app.get_webview_window("player") {
        if show {
            place_player(&app, &w);
            let _ = w.show();
        } else {
            let _ = w.hide();
        }
    }
}

#[tauri::command]
fn player_nav(app: AppHandle, action: String) {
    if let Some(w) = app.get_webview_window("player") {
        let _ = match action.as_str() {
            "back" => w.eval("history.back()"),
            "reload" => w.eval("location.reload()"),
            // écran de cookies : choisit le refus des cookies non essentiels
            "consent_reject" => w.eval("(function(){const b=[...document.querySelectorAll('button')].find(x=>/refuser|reject/i.test((x.getAttribute('aria-label')||'')+' '+x.textContent));if(b)b.click();})()"),
            _ => Ok(()),
        };
    }
}

/// Lance une recherche dans le service intégré. `soft` évite de recharger la page pour ne pas couper la musique.
#[tauri::command]
fn player_search(app: AppHandle, q: String, soft: bool) {
    let id = CUR_SERVICE.lock().unwrap().clone();
    let (Some(home), Some(path), Some(w)) = (service_home(&id), service_search(&id, &q), app.get_webview_window("player")) else { return };
    let js = if soft {
        format!("history.pushState({{}}, '', {});window.dispatchEvent(new PopStateEvent('popstate', {{ state: {{}} }}));", serde_json::to_string(&path).unwrap_or_default())
    } else {
        format!("location.href = {}", serde_json::to_string(&format!("{home}{path}")).unwrap_or_default())
    };
    let _ = w.eval(&js);
}

#[tauri::command]
fn close_service(app: AppHandle) {
    if let Some(w) = app.get_webview_window("player") {
        let _ = w.destroy();
    }
    CUR_SERVICE.lock().unwrap().clear();
}

#[tauri::command]
fn win(app: AppHandle, action: String) {
    match action.as_str() {
        "hide" => {
            if let Some(p) = app.get_webview_window("player") {
                let _ = p.hide();
            }
            if let Some(w) = app.get_webview_window("scene") {
                let _ = w.hide();
            }
        }
        "show" => show_scene(&app),
        "fullscreen" => {
            let sh = app.state::<Arc<Shared>>();
            let windowed = !sh.settings.lock().unwrap()["windowed"].as_bool().unwrap_or(false);
            update_settings(&app, &sh, &json!({ "windowed": windowed }));
            if let Some(w) = app.get_webview_window("scene") {
                apply_mode(&w, windowed);
            }
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

fn apply_mode(w: &tauri::WebviewWindow, windowed: bool) {
    if windowed {
        let _ = w.set_fullscreen(false);
        let _ = w.set_decorations(true);
        let _ = w.set_size(tauri::LogicalSize::new(1280.0, 800.0));
        let _ = w.center();
    } else {
        let _ = w.set_decorations(false);
        let _ = w.set_fullscreen(true);
    }
}

fn show_scene(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("scene") {
        let windowed = app.state::<Arc<Shared>>().settings.lock().unwrap()["windowed"].as_bool().unwrap_or(false);
        let _ = w.show();
        apply_mode(&w, windowed);
        let _ = w.set_focus();
    }
}

// ---------- Compte Spotify (connexion OAuth PKCE : aucun mot de passe ne passe par Lueur) ----------

struct Spot {
    client_id: String,
    refresh: String,
    access: String,
    expires: Option<Instant>,
}
static SPOT: Mutex<Spot> = Mutex::new(Spot { client_id: String::new(), refresh: String::new(), access: String::new(), expires: None });
const REDIRECT: &str = "http://127.0.0.1:8898/callback";
const SCOPES: &str = "user-read-playback-state user-modify-playback-state user-read-currently-playing";

fn spot_path() -> PathBuf {
    data_dir().join("spotify.json")
}
fn spot_load() {
    if let Ok(txt) = fs::read_to_string(spot_path()) {
        if let Ok(v) = serde_json::from_str::<Value>(&txt) {
            let mut s = SPOT.lock().unwrap();
            s.client_id = v["client_id"].as_str().unwrap_or("").to_string();
            s.refresh = v["refresh_token"].as_str().unwrap_or("").to_string();
        }
    }
}
fn spot_save(s: &Spot) {
    let _ = fs::write(spot_path(), json!({ "client_id": s.client_id, "refresh_token": s.refresh }).to_string());
}
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}
fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
fn random_token() -> String {
    let mut b = [0u8; 48];
    let _ = getrandom::getrandom(&mut b);
    b64url(&b)
}
fn token_request(form: &[(&str, &str)]) -> Result<Value, String> {
    match ureq::post("https://accounts.spotify.com/api/token").timeout(Duration::from_secs(10)).send_form(form) {
        Ok(r) => serde_json::from_str(&r.into_string().map_err(|e| e.to_string())?).map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, r)) => Err(format!("Spotify a refusé la connexion ({code}) : {}", r.into_string().unwrap_or_default())),
        Err(e) => Err(e.to_string()),
    }
}
fn store_tokens(v: &Value) {
    let mut s = SPOT.lock().unwrap();
    s.access = v["access_token"].as_str().unwrap_or("").to_string();
    let secs = v["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60);
    s.expires = Some(Instant::now() + Duration::from_secs(secs));
    if let Some(r) = v["refresh_token"].as_str() {
        s.refresh = r.to_string();
    }
    spot_save(&s);
}
fn access_token() -> Result<String, String> {
    let (cid, refresh, access, valid) = {
        let s = SPOT.lock().unwrap();
        (s.client_id.clone(), s.refresh.clone(), s.access.clone(), s.expires.map_or(false, |e| Instant::now() < e))
    };
    if refresh.is_empty() {
        return Err("Spotify n'est pas connecté".into());
    }
    if valid && !access.is_empty() {
        return Ok(access);
    }
    let v = token_request(&[("grant_type", "refresh_token"), ("refresh_token", &refresh), ("client_id", &cid)]).map_err(|_| "401".to_string())?;
    store_tokens(&v);
    Ok(v["access_token"].as_str().unwrap_or("").to_string())
}

#[tauri::command]
fn spotify_status() -> Value {
    let s = SPOT.lock().unwrap();
    json!({ "connected": !s.refresh.is_empty(), "client_id": s.client_id })
}

#[tauri::command]
fn spotify_disconnect() {
    let mut s = SPOT.lock().unwrap();
    s.refresh.clear();
    s.access.clear();
    s.expires = None;
    let _ = fs::remove_file(spot_path());
}

/// Ouvre la page d'autorisation Spotify dans le navigateur et attend le retour sur 127.0.0.1.
#[tauri::command(async)]
fn spotify_connect(client_id: String) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};

    let client_id = client_id.trim().to_string();
    if client_id.len() < 20 || client_id.len() > 64 || !client_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err("Ce Client ID n'a pas la bonne forme (32 lettres et chiffres).".into());
    }
    let verifier = random_token();
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    let state = random_token()[..20].to_string();
    let listener = std::net::TcpListener::bind("127.0.0.1:8898").map_err(|_| "Le port 8898 est déjà utilisé sur ce PC.".to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let url = format!(
        "https://accounts.spotify.com/authorize?response_type=code&client_id={}&scope={}&redirect_uri={}&state={}&code_challenge_method=S256&code_challenge={}",
        enc(&client_id), enc(SCOPES), enc(REDIRECT), enc(&state), enc(&challenge)
    );
    open::that(&url).map_err(|e| e.to_string())?;

    let deadline = Instant::now() + Duration::from_secs(180);
    let code = loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                let path = req.split_whitespace().nth(1).unwrap_or("");
                let reply = |stream: &mut std::net::TcpStream, msg: &str| {
                    let body = format!("<!doctype html><meta charset=utf-8><title>Lueur</title><body style=\"font:18px Segoe UI,sans-serif;background:#0b0b12;color:#fff;display:grid;place-items:center;height:100vh;margin:0\"><p>{msg}</p>");
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                };
                if !path.starts_with("/callback") {
                    reply(&mut stream, "Lueur");
                    continue;
                }
                let query = path.split_once('?').map(|x| x.1).unwrap_or("");
                let get = |k: &str| query.split('&').find_map(|kv| kv.split_once('=').filter(|x| x.0 == k).map(|x| dec(x.1)));
                if get("state").as_deref() != Some(state.as_str()) {
                    reply(&mut stream, "Retour inattendu : recommence la connexion depuis Lueur.");
                    return Err("Le retour de Spotify ne correspond pas à la demande.".into());
                }
                match get("code") {
                    Some(c) => {
                        reply(&mut stream, "Lueur est connectée à Spotify. Tu peux fermer cet onglet.");
                        break c;
                    }
                    None => {
                        reply(&mut stream, "Connexion annulée.");
                        return Err("Connexion à Spotify annulée.".into());
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    return Err("Temps écoulé : aucune réponse de Spotify.".into());
                }
                thread::sleep(Duration::from_millis(150));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let v = token_request(&[
        ("grant_type", "authorization_code"),
        ("code", &code),
        ("redirect_uri", REDIRECT),
        ("client_id", &client_id),
        ("code_verifier", &verifier),
    ])?;
    SPOT.lock().unwrap().client_id = client_id;
    store_tokens(&v);
    Ok(())
}

/// Appels à Spotify limités au lecteur et à la recherche. En cas d'échec, renvoie le code HTTP.
#[tauri::command(async)]
fn spotify_api(method: String, path: String, body: Option<String>) -> Result<String, String> {
    if !(path.starts_with("/v1/me/player") || path.starts_with("/v1/search?")) || !["GET", "PUT", "POST"].contains(&method.as_str()) {
        return Err("appel refusé".into());
    }
    let token = access_token()?;
    let req = ureq::request(&method, &format!("https://api.spotify.com{path}"))
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(8));
    let res = if method == "GET" {
        req.call()
    } else {
        req.set("Content-Type", "application/json").send_string(body.as_deref().unwrap_or(""))
    };
    match res {
        Ok(r) => Ok(r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(code, _)) => Err(code.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

// ---------- Musique en cours (Windows) ----------

#[cfg(windows)]
mod winmedia {
    use super::*;
    use windows::{
        Media::Control::{
            GlobalSystemMediaTransportControlsSession as Session,
            GlobalSystemMediaTransportControlsSessionManager as SessionManager,
            GlobalSystemMediaTransportControlsSessionMediaProperties as MediaProps,
            GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
        },
        Storage::Streams::DataReader,
    };

    fn is_self(_s: &Session) -> bool {
        false // le lecteur intégré à l'app est une source comme les autres
    }
    
    fn pick_session(mgr: &SessionManager, preferred: &str) -> Option<Session> {
        if !preferred.is_empty() {
            if let Ok(list) = mgr.GetSessions() {
                for s in list {
                    if s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default() == preferred {
                        return Some(s);
                    }
                }
            }
        }
        if let Ok(list) = mgr.GetSessions() {
            for s in list {
                if is_self(&s) {
                    continue;
                }
                let playing = s
                    .GetPlaybackInfo()
                    .and_then(|i| i.PlaybackStatus())
                    .map(|p| p == PlaybackStatus::Playing)
                    .unwrap_or(false);
                if playing {
                    return Some(s);
                }
            }
        }
        mgr.GetCurrentSession().ok().filter(|s| !is_self(s))
    }
    
    struct Snap {
        title: String,
        artist: String,
        album: String,
        app: String,
        playing: bool,
        pos: i64,
        dur: i64,
        props: MediaProps,
    }
    
    fn snapshot(s: &Session) -> windows::core::Result<Snap> {
        let props = s.TryGetMediaPropertiesAsync()?.get()?;
        let playing = s.GetPlaybackInfo()?.PlaybackStatus()? == PlaybackStatus::Playing;
        let tl = s.GetTimelineProperties()?;
        let dur = tl.EndTime()?.Duration / 10_000;
        let mut pos = tl.Position()?.Duration / 10_000;
        let updated = tl.LastUpdatedTime()?.UniversalTime;
        if playing && updated > 0 {
            let now_ticks = now_ms() * 10_000 + 116_444_736_000_000_000;
            let d = (now_ticks - updated) / 10_000;
            if d > 0 && d < 86_400_000 {
                pos += d;
            }
        }
        if dur > 0 && pos > dur {
            pos = dur;
        }
        Ok(Snap {
            title: props.Title()?.to_string(),
            artist: props.Artist()?.to_string(),
            album: props.AlbumTitle()?.to_string(),
            app: s.SourceAppUserModelId()?.to_string(),
            playing,
            pos,
            dur,
            props,
        })
    }
    
    fn thumbnail(props: &MediaProps) -> windows::core::Result<Vec<u8>> {
        let stream = props.Thumbnail()?.OpenReadAsync()?.get()?;
        let size = stream.Size()? as u32;
        let reader = DataReader::CreateDataReader(&stream)?;
        reader.LoadAsync(size)?.get()?;
        let mut buf = vec![0u8; size as usize];
        reader.ReadBytes(&mut buf)?;
        Ok(buf)
    }
    
    pub fn media_thread(app: AppHandle, sh: Arc<Shared>, rx: mpsc::Receiver<String>) {
        unsafe {
            let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
        }
        let mgr = loop {
            match SessionManager::RequestAsync().and_then(|op| op.get()) {
                Ok(m) => break m,
                Err(_) => thread::sleep(Duration::from_secs(2)),
            }
        };
        let mut last_key = String::from("\u{0}");
        let mut track_start = Instant::now();
        let mut thumb_tries = 0u32;
        let mut last_thumb_len = usize::MAX;
    
        loop {
            // Commandes (pause, suivant…) ou attente d'une demi-seconde
            if let Ok(c) = rx.recv_timeout(Duration::from_millis(500)) {
                let pref = sh.preferred.lock().unwrap().clone();
                if let Some(s) = pick_session(&mgr, &pref) {
                    let _ = match c.as_str() {
                        "toggle" => s.TryTogglePlayPauseAsync().and_then(|o| o.get()),
                        "next" => s.TrySkipNextAsync().and_then(|o| o.get()),
                        "prev" => s.TrySkipPreviousAsync().and_then(|o| o.get()),
                        _ => Ok(false),
                    };
                }
                thread::sleep(Duration::from_millis(150));
            }
    
            // Liste des apps de musique ouvertes, pour le sélecteur de source
            let mut list = Vec::new();
            if let Ok(sessions) = mgr.GetSessions() {
                for s in sessions {
                    if is_self(&s) {
                        continue;
                    }
                    let id = s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default();
                    let playing = s
                        .GetPlaybackInfo()
                        .and_then(|i| i.PlaybackStatus())
                        .map(|p| p == PlaybackStatus::Playing)
                        .unwrap_or(false);
                    list.push(json!({ "id": id, "playing": playing }));
                }
            }
            let list = Value::Array(list);
            {
                let mut cur = sh.sources.lock().unwrap();
                if *cur != list {
                    *cur = list.clone();
                    let _ = app.emit("sources", &list);
                }
            }
    
            let pref = sh.preferred.lock().unwrap().clone();
            let snap = pick_session(&mgr, &pref).and_then(|s| snapshot(&s).ok());
            let Some(sn) = snap else {
                if !last_key.is_empty() {
                    last_key.clear();
                    let st = json!({ "title": "", "artist": "", "album": "", "app": "", "playing": false, "pos": 0, "dur": 0, "at": now_ms() });
                    *sh.state.lock().unwrap() = st.clone();
                    *sh.art.lock().unwrap() = String::new();
                    let _ = app.emit("state", &st);
                    let _ = app.emit("art", "");
                }
                continue;
            };
    
            let key = format!("{}|{}", sn.title, sn.artist);
            if key != last_key {
                last_key = key;
                track_start = Instant::now();
                thumb_tries = 10;
                last_thumb_len = usize::MAX;
                *sh.art.lock().unwrap() = String::new();
                let _ = app.emit("art", "");
            }
            // Certaines sources ne donnent pas la position : on compte depuis le début du titre
            let pos = if sn.dur > 0 { sn.pos } else { track_start.elapsed().as_millis() as i64 };
            let st = json!({
                "title": sn.title, "artist": sn.artist, "album": sn.album, "app": sn.app,
                "playing": sn.playing, "pos": pos, "dur": sn.dur, "at": now_ms(),
            });
            *sh.state.lock().unwrap() = st.clone();
            let _ = app.emit("state", &st);
    
            if thumb_tries > 0 {
                thumb_tries -= 1;
                if let Ok(bytes) = thumbnail(&sn.props) {
                    if !bytes.is_empty() && bytes.len() != last_thumb_len {
                        last_thumb_len = bytes.len();
                        let mime = if bytes[0] == 0x89 { "image/png" } else { "image/jpeg" };
                        let url = format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes));
                        *sh.art.lock().unwrap() = url.clone();
                        let _ = app.emit("art", url);
                    }
                }
            }
        }
    }
}
#[cfg(windows)]
use winmedia::media_thread;

/// Hors Windows : pas de lecteur système à suivre, l'app lit l'état de son propre lecteur.
#[cfg(not(windows))]
fn media_thread(_app: AppHandle, _sh: Arc<Shared>, rx: mpsc::Receiver<String>) {
    while rx.recv().is_ok() {}
}

// ---------- Son du PC (sortie audio, jamais le micro) ----------

fn audio_thread(app: AppHandle) {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let Some(dev) = host.default_output_device() else { return };
    let Ok(cfg) = dev.default_output_config() else { return };
    if cfg.sample_format() != cpal::SampleFormat::F32 {
        return;
    }
    let ch = cfg.channels().max(1) as usize;
    let sr = cfg.sample_rate().0 as f32;
    let alpha = 1.0 - (-2.0 * std::f32::consts::PI * 150.0 / sr).exp(); // passe-bas ~150 Hz : les basses

    // (énergie des basses, énergie totale, nombre d'échantillons, dernier son reçu)
    let acc = Arc::new(Mutex::new((0f32, 0f32, 0u32, Instant::now())));
    let acc2 = acc.clone();
    let mut lp = 0f32;
    let stream = dev.build_input_stream(
        &cfg.config(),
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            let (mut b, mut f, mut n) = (0f32, 0f32, 0u32);
            for frame in data.chunks(ch) {
                let m = frame.iter().sum::<f32>() / ch as f32;
                lp += alpha * (m - lp);
                b += lp * lp;
                f += m * m;
                n += 1;
            }
            let mut g = acc2.lock().unwrap();
            g.0 += b;
            g.1 += f;
            g.2 += n;
            g.3 = Instant::now();
        },
        |_err| {},
        None,
    );
    let Ok(stream) = stream else { return };
    if stream.play().is_err() {
        return;
    }

    let mut silent = true;
    loop {
        thread::sleep(Duration::from_millis(33));
        let (b, f, n, last) = {
            let mut g = acc.lock().unwrap();
            let v = (g.0, g.1, g.2, g.3);
            g.0 = 0.0;
            g.1 = 0.0;
            g.2 = 0;
            v
        };
        let (bass, level) = if n > 0 {
            ((b / n as f32).sqrt(), (f / n as f32).sqrt())
        } else if last.elapsed() < Duration::from_millis(120) {
            continue;
        } else {
            (0.0, 0.0)
        };
        if bass == 0.0 && level == 0.0 {
            if silent {
                continue;
            }
            silent = true;
        } else {
            silent = false;
        }
        let _ = app.emit("audio", json!({ "b": bass, "l": level }));
    }
}

// ---------- Démarrage ----------

fn main() {
    spot_load();
    // L'état partagé existe avant les fenêtres, pour qu'elles puissent le lire dès leur chargement
    let dir = data_dir();
    let _ = fs::create_dir_all(&dir);
    let settings_path = dir.join("settings.json");
    let mut settings = default_settings();
    if let Ok(txt) = fs::read_to_string(&settings_path) {
        if let Ok(Value::Object(saved)) = serde_json::from_str::<Value>(&txt) {
            for (k, v) in saved {
                settings[k] = v;
            }
        }
    }
    let demo = matches!(std::env::var("LUEUR_DEMO").as_deref(), Ok("1") | Ok("amb") | Ok("lib"));
    let state = if demo {
        json!({ "title": "One More Time", "artist": "Daft Punk", "album": "Discovery", "app": "Spotify.exe",
                "playing": true, "pos": 5000, "dur": 320000, "at": now_ms() })
    } else {
        json!({ "title": "", "artist": "", "album": "", "app": "", "playing": false, "pos": 0, "dur": 0, "at": now_ms() })
    };
    let preferred = settings["source"].as_str().unwrap_or("").to_string();
    let sources = if demo {
        json!([{ "id": "Spotify.exe", "playing": true }, { "id": "chrome.exe", "playing": false }])
    } else {
        json!([])
    };
    let (tx, rx) = mpsc::channel::<String>();
    let shared = Arc::new(Shared {
        state: Mutex::new(state),
        art: Mutex::new(String::new()),
        settings: Mutex::new(settings),
        settings_path,
        cmd: Mutex::new(tx),
        sources: Mutex::new(sources),
        preferred: Mutex::new(preferred),
    });

    // Linux et Mac : l'écran est servi depuis http://localhost, sinon YouTube refuse de lire les clips (origine tauri://)
    let builder = tauri::Builder::default();
    #[cfg(not(windows))]
    let builder = builder.plugin(tauri_plugin_localhost::Builder::new(UI_PORT).build());
    builder
        .manage(shared.clone())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    use tauri_plugin_global_shortcut::{Code, ShortcutState};
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let name = match shortcut.key {
                        Code::MediaPlayPause | Code::MediaStop => "toggle",
                        Code::MediaTrackNext => "next",
                        Code::MediaTrackPrevious => "prev",
                        _ => return,
                    };
                    let _ = app.emit("media-key", name);
                })
                .build(),
        )
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_scene(app)))
        .invoke_handler(tauri::generate_handler![init, set_settings, set_source, spotify_status, spotify_connect, spotify_disconnect, spotify_api, video_search, open_service, player_open, player_play, player_data, update_check, update_install, svc_cmd, mac_music, has_app, player_show, player_nav, player_search, close_service, media_cmd, http_get, open_url, wallpaper, win])
        .on_window_event(|w, e| {
            if let WindowEvent::CloseRequested { api, .. } = e {
                if w.label() == "scene" {
                    api.prevent_close();
                    let _ = w.emit("app-hidden", ());
                    let _ = w.hide();
                }
            }
        })
        .setup(move |app| {
            let handle = app.handle().clone();
            // Fenêtre de lumière : transparente, par-dessus tout, traversée par les clics
            if let Some(ov) = app.get_webview_window("overlay") {
                if let Ok(Some(m)) = ov.primary_monitor() {
                    let (p, s) = (m.position(), m.size());
                    let _ = ov.set_position(PhysicalPosition::new(p.x, p.y));
                    let _ = ov.set_size(PhysicalSize::new(s.width, s.height.saturating_sub(1)));
                }
                let _ = ov.set_ignore_cursor_events(true);
                let _ = ov.show();
            }
            #[cfg(not(windows))]
            if let Some(w) = app.get_webview_window("scene") {
                if let Ok(u) = tauri::Url::parse(&format!("http://localhost:{UI_PORT}/scene.html")) {
                    let _ = w.navigate(u);
                }
            }
            show_scene(&handle);
            {
                use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut};
                for c in [Code::MediaPlayPause, Code::MediaTrackNext, Code::MediaTrackPrevious, Code::MediaStop] {
                    let _ = app.global_shortcut().register(Shortcut::new(None, c));
                }
            }

            // Icône près de l'horloge
            let open_i = MenuItem::with_id(app, "open", "Ouvrir Onda", true, None::<&str>)?;
            let light_i = MenuItem::with_id(app, "light", "Lumière : activer / couper", true, None::<&str>)?;
            let lyrics_i = MenuItem::with_id(app, "lyrics", "Paroles : activer / couper", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quitter Onda", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open_i, &light_i, &lyrics_i, &quit_i])?;
            let mut tray = TrayIconBuilder::new()
                .tooltip("Onda")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| match e.id.as_ref() {
                    "open" => show_scene(app),
                    "quit" => app.exit(0),
                    key @ ("light" | "lyrics") => {
                        let sh = app.state::<Arc<Shared>>();
                        let cur = sh.settings.lock().unwrap()[key].as_bool().unwrap_or(true);
                        update_settings(app, &sh, &json!({ key: !cur }));
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                        show_scene(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            if !demo {
                let (h, s) = (handle.clone(), shared.clone());
                thread::spawn(move || media_thread(h, s, rx));
            }
            thread::spawn(move || audio_thread(handle));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("erreur au lancement de Onda");
}
