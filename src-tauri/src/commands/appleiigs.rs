//! eXoAppleIIGS launch pipeline (§23). eXo picks GSplus, GSport or MAME per
//! game and steers disks, speed and config through batch files; the bundled
//! index (`metadata/appleiigs.json`) is that decision as data. Windows runs
//! eXo's own builds from utilAppleIIGS.zip, macOS and Linux run every game
//! under the GSplus 1.38 pack with eXo's config.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use super::win9x::{binary_exists_on_path, pack_candidate, EngineCmd};
use super::DbState;
use crate::db::queries;
use crate::models::Game;

pub(crate) const COLLECTION: &str = "eXoAppleIIGS";
/// Content pack carrying GSplus 1.38 on macOS and Linux.
pub(crate) const PACK_ID: &str = "gsplus";
/// Where utilAppleIIGS.zip puts eXo's emulators.
fn emulators(torrent_root: &Path) -> PathBuf {
    torrent_root.join("eXo").join("emulators")
}

/// A path as the emulator gets it: one separator style, never `a\b/c`.
fn native(p: &Path) -> String {
    p.components().collect::<PathBuf>().display().to_string()
}
/// `game_config` key holding the chosen menu option's label.
const OPTION_KEY: &str = "iigs_option";

/// The emulator eXo assigns a game, and what actually runs it here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IigsEngine {
    Gsplus,
    Gsport,
    Mame,
}

impl IigsEngine {
    /// eXo's Windows build under `eXo/emulators/`, in the zip's own case.
    fn exo_binary(self, torrent_root: &Path) -> PathBuf {
        let (dir, exe) = match self {
            IigsEngine::Gsplus => ("GSPlus", "gsplus.exe"),
            IigsEngine::Gsport => ("GSPort", "GSport.exe"),
            IigsEngine::Mame => ("MAME", "mame.exe"),
        };
        emulators(torrent_root).join(dir).join(exe)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct MenuOption {
    pub label: String,
    #[serde(default)]
    pub config: Option<String>,
    #[serde(default)]
    pub disks: Vec<String>,
    #[serde(default)]
    pub fast: bool,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Extra {
    pub name: String,
    pub size: u64,
    /// Fetched and kept under `content/extrascache` (filled per request).
    #[serde(default)]
    pub cached: bool,
    /// Already in the game tree, as in an imported eXo install.
    #[serde(default)]
    pub on_disk: bool,
}

/// One game's line of the launch index; see gen_iigs_assets.py.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Entry {
    pub engine: IigsEngine,
    /// MAME's DISK1..n, mounted as hard1, hard2, flop3.
    #[serde(default)]
    pub disks: Vec<String>,
    /// eXo's SPEED=TRUE: MAME's 7 MHz ZipGS.
    #[serde(default)]
    pub fast: bool,
    /// The GSplus config when it is not `config.txt`.
    #[serde(default)]
    pub config: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub capslock: bool,
    #[serde(default)]
    pub options: Vec<MenuOption>,
    /// The config enables GSplus 0.14's virtual printer.
    #[serde(default)]
    pub printing: bool,
    #[serde(default)]
    pub extras: Vec<Extra>,
    /// Playtest verdict under GSplus 1.38 for a game eXo runs elsewhere:
    /// "ok", "issues" or "broken"; absent means untested.
    #[serde(default)]
    pub gsplus: Option<String>,
    #[serde(default)]
    pub gsplus_note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Index {
    default_bram: Vec<String>,
    games: HashMap<String, Entry>,
}

fn index() -> Option<&'static Index> {
    static INDEX: OnceLock<Option<Index>> = OnceLock::new();
    INDEX
        .get_or_init(|| {
            let path = crate::commands::paths::bundled_metadata_dir()
                .map_err(|e| log::error!("appleiigs: {e}"))
                .ok()?
                .join("appleiigs.json");
            let text = std::fs::read_to_string(&path)
                .map_err(|e| log::error!("appleiigs: cannot read {}: {e}", path.display()))
                .ok()?;
            serde_json::from_str(&text)
                .map_err(|e| log::error!("appleiigs: {}: {e}", path.display()))
                .ok()
        })
        .as_ref()
}

pub(crate) fn entry(dir: &str) -> Option<&'static Entry> {
    index()?.games.get(dir)
}

// ── Engine ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineSource {
    /// eXo's own build, extracted from utilAppleIIGS.zip (Windows).
    Exo,
    /// The GSplus 1.38 content pack.
    Pack,
    /// A `gsplus` on PATH, where no pack exists for the platform.
    Path,
}

pub(crate) struct Resolved {
    /// What runs the game: eXo's choice on Windows, GSplus elsewhere.
    pub engine: IigsEngine,
    pub cmd: EngineCmd,
    pub source: EngineSource,
}

fn pack_binary(data_dir: &str) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        pack_candidate(data_dir, "gsplus/GSplus.app/Contents/MacOS/GSplus")
    } else if cfg!(target_os = "linux") {
        pack_candidate(data_dir, "gsplus/GSplus.AppImage")
    } else {
        None
    }
}

/// The one place that decides which emulator runs a game. The panel, the
/// download queue and the launcher all ask here (§23).
pub(crate) fn resolve(torrent_root: &Path, data_dir: &str, exo: IigsEngine) -> Option<Resolved> {
    if cfg!(windows) {
        let exe = exo.exo_binary(torrent_root);
        return exe
            .exists()
            .then_some(Resolved { engine: exo, cmd: EngineCmd::Direct(exe), source: EngineSource::Exo });
    }
    if let Some(bin) = pack_binary(data_dir) {
        return Some(Resolved {
            engine: IigsEngine::Gsplus,
            cmd: EngineCmd::Direct(bin),
            source: EngineSource::Pack,
        });
    }
    // Where a pack exists it is fetched with the game; a system GSplus is
    // only the fallback for a platform without one.
    if crate::commands::content_packs::installable_pack(COLLECTION, PACK_ID).is_some() {
        return None;
    }
    binary_exists_on_path("gsplus").then(|| Resolved {
        engine: IigsEngine::Gsplus,
        cmd: EngineCmd::Direct(PathBuf::from("gsplus")),
        source: EngineSource::Path,
    })
}

fn rom(torrent_root: &Path, rom3: bool) -> PathBuf {
    emulators(torrent_root)
        .join("GSPlus")
        .join(if rom3 { "ROM3" } else { "ROM1" })
}

/// Everything utilAppleIIGS.zip provides on this platform is on disk: the
/// ROMs everywhere, eXo's three emulators on Windows.
pub(crate) fn support_ready(torrent_root: &Path) -> bool {
    rom(torrent_root, false).is_file()
        && rom(torrent_root, true).is_file()
        && (!cfg!(windows)
            || [IigsEngine::Gsplus, IigsEngine::Gsport, IigsEngine::Mame]
                .iter()
                .all(|e| e.exo_binary(torrent_root).is_file()))
}

// ── What a launch uses ──────────────────────────────────────────────────────

/// A game's settings with the stored menu choice applied.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Plan {
    pub option: Option<String>,
    pub config: Option<String>,
    pub disks: Vec<String>,
    pub fast: bool,
    pub note: Option<String>,
}

/// eXo asks at every launch; Exodium takes the stored label and falls back to
/// the first entry, which is the one eXo's menu lists first.
pub(crate) fn plan(entry: &Entry, cfg: &HashMap<String, String>) -> Plan {
    let chosen = cfg
        .get(OPTION_KEY)
        .and_then(|want| entry.options.iter().find(|o| &o.label == want))
        .or_else(|| entry.options.first());
    let note = [entry.note.as_deref(), chosen.and_then(|o| o.note.as_deref())]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
    Plan {
        option: chosen.map(|o| o.label.clone()),
        config: chosen.and_then(|o| o.config.clone()).or_else(|| entry.config.clone()),
        disks: chosen
            .filter(|o| !o.disks.is_empty())
            .map(|o| o.disks.clone())
            .unwrap_or_else(|| entry.disks.clone()),
        fast: entry.fast || chosen.is_some_and(|o| o.fast),
        note: (!note.is_empty()).then_some(note),
    }
}

/// `rel` under `base`, each component matched without regard to case: eXo's
/// configs say `game.po` for a `Game.po`, and Windows never noticed. Falls
/// back to the literal join when nothing matches.
pub(crate) fn resolve_case_insensitive(base: &Path, rel: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    for part in rel.split(['\\', '/']).filter(|p| !p.is_empty() && *p != ".") {
        if part == ".." {
            path.pop();
            continue;
        }
        // The listing, not `exists()`: a case-insensitive filesystem would
        // answer yes for the wrong spelling and hide the real name.
        let names: Vec<std::ffi::OsString> = std::fs::read_dir(&path)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name()).collect())
            .unwrap_or_default();
        let found = names
            .iter()
            .find(|n| n.to_string_lossy() == part)
            .or_else(|| names.iter().find(|n| n.to_string_lossy().eq_ignore_ascii_case(part)));
        path = match found {
            Some(name) => path.join(name),
            None => path.join(part),
        };
    }
    path
}

/// The GSplus config a launch runs with: eXo's own with absolute paths, or -
/// for a game eXo runs under MAME - one built from its disks. KEGS changes
/// into the config's directory, so a relative path would follow the copy.
pub(crate) fn derive_config(
    template: Option<&str>,
    game_dir: &Path,
    torrent_root: &Path,
    plan: &Plan,
    default_bram: &[String],
    hide_status: bool,
) -> String {
    let mut out: Vec<String> = Vec::new();
    match template {
        Some(text) => {
            let mut rom_set = false;
            for line in text.lines().map(|l| l.trim_end_matches('\r')) {
                let (key, value) = match line.split_once('=') {
                    Some((k, v)) => (k.trim(), v.trim()),
                    None => {
                        out.push(line.to_string());
                        continue;
                    }
                };
                if key == "g_cfg_rom_path" {
                    let rom3 = value.to_ascii_uppercase().ends_with("ROM3");
                    out.push(format!("g_cfg_rom_path = {}", native(&rom(torrent_root, rom3))));
                    rom_set = true;
                } else if is_slot_key(key) && !value.is_empty() {
                    out.push(format!("{key} = {}", native(&resolve_case_insensitive(game_dir, value))));
                } else if hide_status && key == "g_status_enable" {
                    continue;
                } else {
                    out.push(line.to_string());
                }
            }
            if !rom_set {
                out.push(format!("g_cfg_rom_path = {}", native(&rom(torrent_root, false))));
            }
        }
        None => {
            // MAME's machine: ROM 03, 8 MB, the CFFA2 card's two drives in
            // slot 7 and its third disk in the 3.5" drive.
            out.push("# GSplus configuration file version 0.14".to_string());
            for (slot, disk) in ["s7d1", "s7d2", "s5d1"].iter().zip(&plan.disks) {
                out.push(format!("{slot} = {}", native(&resolve_case_insensitive(game_dir, disk))));
            }
            out.push(format!("g_cfg_rom_path = {}", native(&rom(torrent_root, true))));
            out.push(format!("g_limit_speed = {}", if plan.fast { 3 } else { 2 }));
            if plan.fast {
                out.push("g_zip_speed_mhz = 8".to_string());
            }
            out.extend(default_bram.iter().cloned());
        }
    }
    if hide_status {
        out.push("g_status_enable = 0".to_string());
    }
    out.join("\n") + "\n"
}

fn is_slot_key(key: &str) -> bool {
    let b = key.as_bytes();
    b.len() >= 4 && b[0] == b's' && b[1].is_ascii_digit() && b[2] == b'd' && b[3..].iter().all(u8::is_ascii_digit)
}

/// eXo's MAME command line (Launch_IIGS.bat), with absolute paths.
fn mame_args(torrent_root: &Path, game_dir: &Path, plan: &Plan, fullscreen: bool) -> Vec<String> {
    let mame = emulators(torrent_root).join("MAME");
    let cfg_dir = if plan.fast { mame.join("fast") } else { mame.clone() };
    let mut args: Vec<String> = vec!["apple2gs".into(), "-noka".into()];
    let dir = |p: &Path| format!("{}{}", native(p), std::path::MAIN_SEPARATOR);
    args.extend(["-rp".into(), dir(&mame.join("roms"))]);
    args.extend(["-inipath".into(), dir(&mame)]);
    args.extend(["-cfg_directory".into(), dir(&cfg_dir)]);
    args.extend(["-nvram_directory".into(), dir(&mame)]);
    if fullscreen {
        args.push("-nowindow".into());
    } else {
        args.extend(["-window".into(), "-resolution0".into(), "1400x920".into()]);
    }
    args.extend(
        ["-v", "-sl7", "cffa2", "-noautosave", "-gameio", "joy", "-ramsize", "8M", "-keyboardprovider", "dinput"]
            .map(String::from),
    );
    for (flag, disk) in ["-hard1", "-hard2", "-flop3"].iter().zip(&plan.disks) {
        args.push((*flag).into());
        args.push(native(&resolve_case_insensitive(game_dir, disk)));
    }
    args
}

/// The template for this launch: the option's or eXo's config from the
/// game's `!appleiigs` folder, else the `config.txt` a GSport game carries in
/// its own zip. None for MAME's games, which have no config at all.
fn template(config_dir: &Path, game_dir: &Path, plan: &Plan, exo: IigsEngine) -> Option<String> {
    let name = plan.config.as_deref().unwrap_or("config.txt");
    let candidates = [resolve_case_insensitive(config_dir, name), resolve_case_insensitive(game_dir, "config.txt")];
    let found = candidates.iter().find(|p| p.is_file());
    match (found, exo) {
        (Some(p), _) => std::fs::read(p).ok().map(|b| String::from_utf8_lossy(&b).into_owned()),
        (None, IigsEngine::Mame) => None,
        (None, _) => {
            log::warn!("appleiigs: no config for {} - building one from its disks", game_dir.display());
            None
        }
    }
}

/// The GSplus config one launch of `shortcode` uses: its template from the
/// game's `!appleiigs` folder or zip, with this machine's absolute paths.
fn launch_config(shortcode: &str, entry: &Entry, plan: &Plan, torrent_root: &Path, game_dir: &Path, modern: bool) -> String {
    let config_dir = torrent_root
        .join(crate::commands::collections::collection_game_prefix(COLLECTION))
        .join("!appleiigs")
        .join(shortcode);
    let bram = index().map(|i| i.default_bram.as_slice()).unwrap_or(&[]);
    let tpl = template(&config_dir, game_dir, plan, entry.engine);
    derive_config(tpl.as_deref(), game_dir, torrent_root, plan, bram, modern)
}

/// The config GSplus 1.38 would get for a game, for the playtest sweep
/// (`examples/iigs_sweep.rs`): `option` picks a menu entry by label.
#[doc(hidden)]
pub fn sweep_config(shortcode: &str, torrent_root: &Path, option: Option<&str>) -> Result<String, String> {
    let entry = entry(shortcode).ok_or_else(|| format!("{shortcode} is not in the launch index"))?;
    let mut cfg = HashMap::new();
    if let Some(o) = option {
        cfg.insert(OPTION_KEY.to_string(), o.to_string());
    }
    let plan = plan(entry, &cfg);
    let game_dir = torrent_root
        .join(crate::commands::collections::collection_game_prefix(COLLECTION))
        .join(shortcode);
    Ok(launch_config(shortcode, entry, &plan, torrent_root, &game_dir, true))
}

/// Per-launch configs. Not `content/appleiigs`: the torrent carries an empty
/// FILE `Content/AppleIIGS`, and where the game root is the data dir (§2) a
/// case-insensitive filesystem makes the two one path.
fn run_dir(data_dir: &str, shortcode: &str) -> PathBuf {
    Path::new(data_dir).join("content").join("iigs-run").join(shortcode)
}

pub(crate) async fn launch_appleiigs_game(
    app: &AppHandle,
    game: Game,
    id: i64,
    data_dir: &str,
    fullscreen: bool,
    crt: bool,
    per_game_config: &HashMap<String, String>,
) -> Result<String, String> {
    let title = game.title.clone();
    launch_inner(app, game, id, data_dir, fullscreen, crt, per_game_config)
        .await
        .inspect_err(|e| log::error!("launch_appleiigs_game({}): {}", title, e))
}

async fn launch_inner(
    app: &AppHandle,
    game: Game,
    id: i64,
    data_dir: &str,
    fullscreen: bool,
    crt: bool,
    per_game_config: &HashMap<String, String>,
) -> Result<String, String> {
    let source = game.torrent_source.as_deref().unwrap_or(COLLECTION);
    let torrent_root = crate::commands::paths::game_root(data_dir);
    let shortcode = game.shortcode.clone().unwrap_or_default();
    let entry = entry(&shortcode)
        .ok_or_else(|| format!("'{}' is not in eXo's Apple IIGS launch index.", game.title))?;
    if !support_ready(&torrent_root) {
        // Play is the remedy: queue the util zip here too, so a game adopted
        // from disk (no download_game) is not stuck without its ROMs.
        let manager = app.state::<super::TorrentState>().0.read().await.get(source).cloned();
        return Err(match manager {
            Some(mgr) => {
                crate::support_files::ensure_queued(&crate::support_files::APPLEIIGS_SUPPORT, &mgr).await;
                "Downloading the Apple IIGS support files (ROMs) - press Play again once they are in."
            }
            None => "The Apple IIGS support files (ROMs) are not on disk, and they download only \
                     online. Go online in Settings → Network, then press Play again.",
        }
        .to_string());
    }
    let resolved = resolve(&torrent_root, data_dir, entry.engine).ok_or_else(|| {
        if cfg!(windows) {
            "eXo's Apple IIGS emulators were not found in eXo\\emulators inside your library folder."
                .to_string()
        } else {
            "GSplus was not found. Download it from the game's panel, or install GSplus 1.38 \
             (github.com/digarok/gsplus) so that `gsplus` is on your PATH."
                .to_string()
        }
    })?;
    let game_dir = super::install::extract_before_launch(app, &game, id, source, &torrent_root).await?;
    let plan = plan(entry, per_game_config);
    let fullscreen = match per_game_config.get("fullscreen").map(String::as_str) {
        Some("true") => true,
        Some("false") => false,
        _ => fullscreen,
    };
    let run = run_dir(data_dir, &shortcode);
    std::fs::create_dir_all(&run).map_err(|e| format!("cannot create {}: {e}", run.display()))?;

    let (mut cmd, bin) = resolved.cmd.command(&torrent_root);
    let modern = resolved.source != EngineSource::Exo;
    match resolved.engine {
        IigsEngine::Mame => {
            cmd.args(mame_args(&torrent_root, &game_dir, &plan, fullscreen));
            cmd.current_dir(torrent_root.join("eXo"));
        }
        IigsEngine::Gsport => {
            // GSport reads `config.txt` from its working directory and takes
            // no options; the window opens small and F11 goes fullscreen.
            let text = launch_config(&shortcode, entry, &plan, &torrent_root, &game_dir, false);
            std::fs::write(run.join("config.txt"), text).map_err(|e| e.to_string())?;
            cmd.current_dir(&run);
        }
        IigsEngine::Gsplus => {
            let text = launch_config(&shortcode, entry, &plan, &torrent_root, &game_dir, modern);
            let cfg_path = run.join("gsplus.cfg");
            std::fs::write(&cfg_path, text).map_err(|e| e.to_string())?;
            if modern {
                cmd.arg("-cfg").arg(&cfg_path);
                cmd.args(["-fullscreen", if fullscreen { "1" } else { "0" }]);
                cmd.args(["-crt", if crt { "1" } else { "0" }]);
            } else {
                cmd.arg("-config").arg(&cfg_path);
                if fullscreen {
                    cmd.arg("-fullscreen");
                } else {
                    cmd.args(["-sw", "1400", "-sh", "920"]);
                }
            }
            // eXo starts it from the game folder; GSplus 0.14 prints there.
            cmd.current_dir(&game_dir);
        }
    }
    log::info!(
        "Launching {} via {:?} ({:?}, eXo: {:?}){}",
        game.title,
        resolved.engine,
        resolved.source,
        entry.engine,
        plan.option.as_deref().map(|o| format!(", option '{o}'")).unwrap_or_default()
    );
    super::games::spawn_emulator_and_track(
        app,
        cmd,
        &bin,
        &super::games::running_game_key(&game),
        &game.title,
        id,
    )
}

// ── Panel ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct AppleIigsInfo {
    /// The emulator eXo runs the game with.
    pub exo_engine: IigsEngine,
    /// The emulator that runs it here; None when nothing resolves.
    pub engine: Option<IigsEngine>,
    pub source: Option<EngineSource>,
    /// The pack that would supply GSplus here; None on Windows.
    pub pack_id: Option<String>,
    /// eXo's notes for the game and the selected menu option.
    pub note: Option<String>,
    pub options: Vec<String>,
    pub selected: Option<String>,
    /// Playtest verdict when the game runs under another emulator than
    /// eXo's: "untested", "ok", "issues" or "broken".
    pub substitute: Option<String>,
    pub substitute_note: Option<String>,
    /// The game prints through GSplus 0.14's virtual printer.
    pub printing: bool,
    /// ...which only Windows runs.
    pub printing_unavailable: bool,
    pub capslock: bool,
    pub extras: Vec<Extra>,
}

pub(crate) fn info_for(entry: &Entry, resolved: Option<&Resolved>, cfg: &HashMap<String, String>) -> AppleIigsInfo {
    let plan = plan(entry, cfg);
    // Absent a resolver answer, the platform decides what would run.
    let runs = resolved.map(|r| r.engine).unwrap_or(if cfg!(windows) { entry.engine } else { IigsEngine::Gsplus });
    let substitute = (runs != entry.engine).then(|| entry.gsplus.clone().unwrap_or_else(|| "untested".into()));
    let exo_gsplus = runs == IigsEngine::Gsplus && resolved.is_none_or(|r| r.source == EngineSource::Exo) && cfg!(windows);
    AppleIigsInfo {
        exo_engine: entry.engine,
        engine: resolved.map(|r| r.engine),
        source: resolved.map(|r| r.source),
        pack_id: (!cfg!(windows)).then(|| PACK_ID.to_string()),
        note: plan.note,
        options: entry.options.iter().map(|o| o.label.clone()).collect(),
        selected: plan.option,
        substitute_note: substitute.as_ref().and(entry.gsplus_note.clone()),
        substitute,
        printing: entry.printing,
        printing_unavailable: entry.printing && !exo_gsplus,
        capslock: entry.capslock,
        extras: entry.extras.clone(),
    }
}

/// What the panel shows for an eXoAppleIIGS game, answered by the launcher's
/// own resolver; None for any other collection.
#[tauri::command]
pub async fn appleiigs_info(db_state: State<'_, DbState>, id: i64) -> Result<Option<AppleIigsInfo>, String> {
    let (game, data_dir, cfg) = {
        let conn = db_state.lock()?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game with id {} not found", id))?;
        let data_dir = queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?;
        let cfg = queries::get_all_game_config(&conn, id).unwrap_or_default();
        (game, data_dir, cfg)
    };
    if game.torrent_source.as_deref() != Some(COLLECTION) {
        return Ok(None);
    }
    let Some(entry) = game.shortcode.as_deref().and_then(entry) else {
        return Ok(None);
    };
    let resolved = data_dir
        .as_deref()
        .and_then(|d| resolve(&crate::commands::paths::game_root(d), d, entry.engine));
    let mut info = info_for(entry, resolved.as_ref(), &cfg);
    if let (Some(d), Some(code)) = (data_dir.as_deref(), game.shortcode.as_deref()) {
        let root = crate::commands::paths::game_root(d);
        for extra in &mut info.extras {
            extra.cached = crate::commands::reading::cached_extra(d, code, &extra.name).is_some();
            extra.on_disk = root.join(extra_entry(code, &extra.name)).is_file();
        }
    }
    Ok(Some(info))
}

/// Remember which program of a multi-program disk Play starts; None clears.
#[tauri::command]
pub async fn set_appleiigs_option(
    db_state: State<'_, DbState>,
    id: i64,
    option: Option<String>,
) -> Result<(), String> {
    let conn = db_state.lock()?;
    match option.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => queries::set_game_config(&conn, id, OPTION_KEY, v).map_err(|e| e.to_string()),
        None => queries::delete_game_config(&conn, id, OPTION_KEY).map_err(|e| e.to_string()),
    }
}

/// Where utilAppleIIGS.zip stands: the ROMs everywhere, eXo's emulators on
/// Windows.
#[tauri::command]
pub async fn get_appleiigs_support_status(
    torrent_state: State<'_, super::TorrentState>,
) -> Result<crate::support_files::SupportStatus, String> {
    use crate::support_files::{status_for, APPLEIIGS_SUPPORT};
    Ok(status_for(&torrent_state, &APPLEIIGS_SUPPORT, support_ready).await)
}

/// Torrent path of the archive the Extras live in, and an extra's entry.
pub(crate) const EXTRAS_ARCHIVE: &str = "Content/!AppleIIGSmetadata.zip";

pub(crate) fn extra_entry(shortcode: &str, name: &str) -> String {
    format!("eXo/eXoAppleIIGS/!appleiigs/{shortcode}/Extras/{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry_json(json: &str) -> Entry {
        serde_json::from_str(json).unwrap()
    }

    /// eXo's configs name `game.po`; the zip holds `Game.po`. On a
    /// case-sensitive filesystem only the real name opens.
    #[test]
    fn disk_names_resolve_regardless_of_case() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("eXoAppleIIGS/Arkanoid (1988)");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("Game.po"), b"x").unwrap();
        let other = tmp.path().join("eXoAppleIIGS/Bard's Tale (1987)");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("Characters.2mg"), b"x").unwrap();

        assert_eq!(resolve_case_insensitive(&game, "game.po"), game.join("Game.po"));
        assert_eq!(
            resolve_case_insensitive(&game, "../bard's tale (1987)/characters.2mg"),
            other.join("Characters.2mg")
        );
        assert_eq!(resolve_case_insensitive(&game, "missing.po"), game.join("missing.po"));
    }

    #[test]
    fn exo_config_keeps_its_settings_with_absolute_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let game = root.join("eXo/eXoAppleIIGS/Arkanoid (1988)");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("Game.po"), b"x").unwrap();
        let template = "# GSplus configuration file version 0.14\r\n\r\ns5d1 = \r\ns7d1 = game.po\r\n\r\n\
                        g_cfg_rom_path = ..\\..\\..\\GSPlus\\ROM1\r\ng_limit_speed = 2\r\nbram1[00] = 00 01\r\n";
        let plan = Plan { option: None, config: None, disks: vec![], fast: false, note: None };

        let out = derive_config(Some(template), &game, root, &plan, &[], true);

        assert!(out.contains(&format!("s7d1 = {}", native(&game.join("Game.po")))));
        assert!(out.contains("s5d1 = \n"), "an empty slot stays empty");
        assert!(out.contains(&format!("g_cfg_rom_path = {}", native(&root.join("eXo/emulators/GSPlus/ROM1")))));
        assert!(out.contains("g_limit_speed = 2\n") && out.contains("bram1[00] = 00 01\n"));
        assert!(out.ends_with("g_status_enable = 0\n"));
        assert!(!out.contains('\r'));
    }

    /// A MAME game has no config: its disks go where MAME's CFFA2 card and
    /// 3.5" drive put them, on MAME's ROM 03 machine.
    #[test]
    fn a_mame_game_gets_a_config_built_from_its_disks() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("Spirit of Excalibur (1991)");
        std::fs::create_dir_all(&game).unwrap();
        let plan = Plan {
            option: None,
            config: None,
            disks: vec!["disk1.2mg".into(), "disk3.2mg".into(), "disk2.2mg".into()],
            fast: true,
            note: None,
        };
        let out = derive_config(None, &game, tmp.path(), &plan, &["bram3[00] = 01".to_string()], false);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.iter().any(|l| l.ends_with("disk1.2mg") && l.starts_with("s7d1 = ")));
        assert!(lines.iter().any(|l| l.ends_with("disk3.2mg") && l.starts_with("s7d2 = ")));
        assert!(lines.iter().any(|l| l.ends_with("disk2.2mg") && l.starts_with("s5d1 = ")));
        assert!(lines.iter().any(|l| l.starts_with("g_cfg_rom_path = ") && l.ends_with("ROM3")));
        assert!(lines.contains(&"g_limit_speed = 3") && lines.contains(&"g_zip_speed_mhz = 8"));
        assert!(lines.contains(&"bram3[00] = 01"));
        assert!(!out.contains("g_status_enable"));
    }

    #[test]
    fn the_stored_option_picks_config_disks_and_note() {
        let e = entry_json(
            r#"{"engine":"mame","note":"Add-On 1 is X.","options":[
                {"label":"Add-Ons 1 and 2","disks":["Game.po"],"fast":true},
                {"label":"Add-Ons 3 and 4","disks":["Game2.po"],"fast":true,"note":"Second set."}]}"#,
        );
        let p = plan(&e, &HashMap::new());
        assert_eq!(p.option.as_deref(), Some("Add-Ons 1 and 2"));
        assert_eq!(p.disks, ["Game.po"]);
        assert!(p.fast);
        assert_eq!(p.note.as_deref(), Some("Add-On 1 is X."));

        let cfg = HashMap::from([(OPTION_KEY.to_string(), "Add-Ons 3 and 4".to_string())]);
        let p = plan(&e, &cfg);
        assert_eq!(p.disks, ["Game2.po"]);
        assert_eq!(p.note.as_deref(), Some("Add-On 1 is X.\nSecond set."));

        // A label the index no longer has falls back to the first entry.
        let cfg = HashMap::from([(OPTION_KEY.to_string(), "gone".to_string())]);
        assert_eq!(plan(&e, &cfg).option.as_deref(), Some("Add-Ons 1 and 2"));

        let g = entry_json(r#"{"engine":"gsplus","options":[{"label":"Cribbage King","config":"crib.txt"}]}"#);
        assert_eq!(plan(&g, &HashMap::new()).config.as_deref(), Some("crib.txt"));
    }

    #[test]
    fn mame_runs_exo_command_line_with_absolute_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let game = root.join("eXo/eXoAppleIIGS/Rocket Ranger (1989)");
        let plan = Plan {
            option: None,
            config: None,
            disks: vec!["disk1.2mg".into(), "disk2.2mg".into()],
            fast: false,
            note: None,
        };
        let args = mame_args(root, &game, &plan, false);
        assert_eq!(&args[..2], ["apple2gs", "-noka"]);
        let after = |flag: &str| args.iter().position(|a| a == flag).map(|i| args[i + 1].clone());
        assert_eq!(after("-hard1"), Some(native(&game.join("disk1.2mg"))));
        assert_eq!(after("-hard2"), Some(native(&game.join("disk2.2mg"))));
        assert!(after("-flop3").is_none());
        assert!(after("-cfg_directory").unwrap().starts_with(&native(&root.join("eXo/emulators/MAME"))));
        assert!(!after("-cfg_directory").unwrap().contains("fast"));
        assert!(args.contains(&"-window".to_string()));
        let fast = Plan { fast: true, ..plan };
        assert!(mame_args(root, &game, &fast, true).iter().any(|a| a.contains("fast")));
    }

    /// The panel's verdict: a game eXo runs under GSplus is no substitute; a
    /// MAME game under GSplus is, and says how it fared in the playtest.
    #[test]
    fn info_marks_substitutes_and_their_playtest() {
        let mame = entry_json(r#"{"engine":"mame","disks":["Game.po"],"gsplus":"issues","gsplus_note":"No sound."}"#);
        let gsplus = Resolved {
            engine: IigsEngine::Gsplus,
            cmd: EngineCmd::Direct(PathBuf::from("gsplus")),
            source: EngineSource::Pack,
        };
        let info = info_for(&mame, Some(&gsplus), &HashMap::new());
        assert_eq!(info.substitute.as_deref(), Some("issues"));
        assert_eq!(info.substitute_note.as_deref(), Some("No sound."));

        let untested = entry_json(r#"{"engine":"gsport"}"#);
        assert_eq!(info_for(&untested, Some(&gsplus), &HashMap::new()).substitute.as_deref(), Some("untested"));

        let native = entry_json(r#"{"engine":"gsplus","printing":true}"#);
        let info = info_for(&native, Some(&gsplus), &HashMap::new());
        assert!(info.substitute.is_none());
        assert!(info.printing_unavailable, "GSplus 1.38 has no printer");
    }

    #[test]
    fn slot_keys_are_recognised() {
        assert!(is_slot_key("s7d1") && is_slot_key("s5d2") && is_slot_key("s7d12"));
        assert!(!is_slot_key("g_cfg_rom_path") && !is_slot_key("sd1") && !is_slot_key("s7x1"));
    }
}
