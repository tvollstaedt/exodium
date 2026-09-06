//! eXoScummVM launch pipeline.
//!
//! There is no per-game conf in this pack. eXo runs every title through ONE
//! bat (`launch_svm.bat`) that looks the game up in an index
//! (`util/scummvm.txt`: `<dir>;<engine:gameid | target>;<build>\scummvm.exe`)
//! and assembles a single ScummVM command line. This module does the same:
//! the bundled copy of that index (`metadata/scummvm.txt`) names the game id
//! and the ScummVM build the game is pinned to, the game's own directory tree
//! names the platform variant (`platform.txt`, `menu.txt`, `video.txt`,
//! `sound.txt`, ...), and the pinned build's `scummvm.ini` (bundled under
//! `metadata/scummvm_ini/`) carries the targets 44 games launch by name.
//!
//! eXo pins seven builds because games break across ScummVM releases (its
//! readme keeps "5 or 6 old versions" for exactly that). On Windows those
//! builds come out of the torrent's `utilSVM.zip`; on macOS/Linux each is a
//! content pack. Until a pack is installed, a `scummvm` on PATH (or the
//! Flatpak) runs the game with whatever version it is - the panel says so.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Serialize;
use tauri::{AppHandle, State};

use super::win9x::{binary_exists_on_path, pack_candidate, EngineCmd};
use super::DbState;
use crate::db::queries;
use crate::models::Game;

/// eXo's build directories under `eXo/emulators/scmvm/` and the ScummVM
/// version each holds. "default" is the root `scummvm.exe` (2.8.0 - its ini
/// says 2.6.1, the binary does not). The git snapshots carry their commit in
/// the version string, so a pack can be built from the exact revision.
pub(crate) const BUILDS: &[(&str, &str)] = &[
    ("default", "2.8.0"),
    ("2.5", "2.5.0"),
    ("2.9.0", "2.9.0"),
    ("2026.1.0", "2026.1.0"),
    ("svn2.3_18903", "2.3.0git18903-g313a824fb9"),
    ("svn2.8_9335", "2.8.0git9335-g00e72a17004"),
    ("svn3.0.0git19492", "3.0.0git20192-g3ca9da6a1c3"),
];

/// Flatpak id of the upstream ScummVM build (Linux fallback of last resort).
const FLATPAK_ID: &str = "org.scummvm.ScummVM";

pub(crate) fn build_version(slug: &str) -> &'static str {
    BUILDS
        .iter()
        .find(|(s, _)| *s == slug)
        .map(|(_, v)| *v)
        .unwrap_or("unknown")
}

/// One line of eXo's launch index.
#[derive(Debug, Clone)]
struct IndexEntry {
    /// The directory the game runs from - a zip stem for most entries, an
    /// inner variant directory for the 122 sub-entries.
    dir: String,
    /// `engine:gameid`, a bare gameid, or a target defined in the build's ini.
    game_id: String,
    /// Build slug (see `BUILDS`).
    build: String,
}

fn index() -> &'static [IndexEntry] {
    static INDEX: OnceLock<Vec<IndexEntry>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let Ok(dir) = crate::commands::setup::bundled_metadata_dir() else {
            log::error!("scummvm: no bundled metadata dir - launch index unavailable");
            return Vec::new();
        };
        let path = dir.join("scummvm.txt");
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                log::error!("scummvm: cannot read {}: {}", path.display(), e);
                return Vec::new();
            }
        };
        text.lines()
            .filter_map(|line| {
                let mut parts = line.trim_end_matches('\r').splitn(3, ';');
                let dir = parts.next()?.trim();
                let game_id = parts.next()?.trim();
                let exe = parts.next()?.trim();
                if dir.is_empty() || game_id.is_empty() {
                    return None;
                }
                // "2.9.0\scummvm.exe" -> "2.9.0"; a bare "scummvm.exe" is the root build.
                let build = exe
                    .rsplit_once('\\')
                    .map(|(d, _)| d.to_string())
                    .unwrap_or_else(|| "default".to_string());
                Some(IndexEntry { dir: dir.to_string(), game_id: game_id.to_string(), build })
            })
            .collect()
    })
}

/// Find the index line for a game the way `launch_svm.bat` does: it looks the
/// innermost selected directory up first and keeps the previous hit when a
/// level has no line of its own (`findstr /b` - a PREFIX match, which is why
/// the exact pass runs before the prefix pass here).
fn lookup(names: &[&str]) -> Option<&'static IndexEntry> {
    let idx = index();
    names
        .iter()
        .find_map(|n| idx.iter().find(|e| e.dir.eq_ignore_ascii_case(n)))
        .or_else(|| {
            names.iter().find_map(|n| {
                idx.iter().find(|e| {
                    e.dir.len() >= n.len() && e.dir[..n.len()].eq_ignore_ascii_case(n)
                })
            })
        })
}

/// Where a ScummVM binary was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EngineSource {
    /// eXo's own build, extracted from utilSVM.zip (Windows).
    Exo,
    /// The downloaded content pack for exactly this build.
    Pack,
    /// A `scummvm` on PATH - some version, not necessarily the pinned one.
    Path,
    /// The upstream Flatpak (Linux).
    Flatpak,
}

pub(crate) struct Resolved {
    pub cmd: EngineCmd,
    pub source: EngineSource,
}

/// Locate a ScummVM able to run games pinned to `slug`, most faithful first:
/// eXo's build (Windows), the per-version pack (macOS/Linux), then whatever
/// the system offers. The last two ignore the pin - a game that needs 2.5.0
/// may misbehave under a 2026 release, which is what the packs are for.
pub(crate) fn resolve_scummvm(torrent_root: &Path, data_dir: &str, slug: &str) -> Option<Resolved> {
    if cfg!(windows) {
        let base = torrent_root.join("eXo/emulators/scmvm");
        let exe = if slug == "default" {
            base.join("scummvm.exe")
        } else {
            base.join(slug).join("scummvm.exe")
        };
        if exe.exists() {
            return Some(Resolved { cmd: EngineCmd::Direct(exe), source: EngineSource::Exo });
        }
    }
    let version = build_version(slug);
    let packed = if cfg!(target_os = "macos") {
        pack_candidate(data_dir, &format!("scummvm-{version}/ScummVM.app/Contents/MacOS/scummvm"))
    } else if cfg!(target_os = "linux") {
        pack_candidate(data_dir, &format!("scummvm-{version}/ScummVM.AppImage"))
    } else {
        None
    };
    if let Some(bin) = packed {
        return Some(Resolved { cmd: EngineCmd::Direct(bin), source: EngineSource::Pack });
    }
    if binary_exists_on_path("scummvm") {
        return Some(Resolved {
            cmd: EngineCmd::Direct(PathBuf::from("scummvm")),
            source: EngineSource::Path,
        });
    }
    if cfg!(target_os = "linux") && flatpak_available() {
        return Some(Resolved { cmd: EngineCmd::Flatpak(FLATPAK_ID), source: EngineSource::Flatpak });
    }
    None
}

fn flatpak_available() -> bool {
    std::process::Command::new("flatpak")
        .args(["info", FLATPAK_ID])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The pinned build's ini, copied once into a writable place: ScummVM saves
/// its own settings back into the file it was started with, and the bundled
/// resources may be read-only (signed app bundle).
fn ensure_config(data_dir: &str, slug: &str) -> Result<PathBuf, String> {
    let dir = Path::new(data_dir).join("content/scummvm");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let ini = dir.join(format!("{slug}.ini"));
    if ini.exists() {
        return Ok(ini);
    }
    let bundled = crate::commands::setup::bundled_metadata_dir()?
        .join("scummvm_ini")
        .join(format!("{slug}.ini"));
    match std::fs::read(&bundled) {
        Ok(bytes) => std::fs::write(&ini, bytes).map_err(|e| e.to_string())?,
        Err(e) => {
            log::warn!("scummvm: no bundled ini for build {slug} ({e}); starting from defaults");
            std::fs::write(&ini, "[scummvm]\n").map_err(|e| e.to_string())?;
        }
    }
    Ok(ini)
}

/// Directories inside `dir`, in the order eXo's `dir /A /B` lists them
/// (alphabetical, case-insensitive); `!save`/dotfiles never count.
fn subdirs(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| !n.starts_with('.') && !n.starts_with('!'))
                .collect()
        })
        .unwrap_or_default();
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// First `:`-delimited field of a control file's first line, as eXo's
/// `for /f "tokens=1 delims=:"` reads it. None when the file is absent.
fn first_field(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let line = text.lines().next()?.trim_end_matches('\r');
    let field = line.split(':').next()?.trim();
    (!field.is_empty()).then(|| field.to_string())
}

/// The first control file found in `dirs`, probed in order.
fn probe(dirs: &[&Path], name: &str) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// The concrete directory a launch runs from plus the options its control
/// files request. eXo prompts for each level at launch; Exodium takes the
/// stored choice from `game_config` (`svm_variant`, `svm_sub`, `svm_sound`,
/// `svm_subtitles`) and falls back to the first entry, which is what eXo's
/// menu preselects.
pub(crate) struct Variant {
    pub run_dir: PathBuf,
    /// Directory names to look up in the index, innermost first.
    pub names: Vec<String>,
    pub platform: Option<String>,
    pub extra_args: Vec<String>,
    pub note: Option<String>,
}

pub(crate) fn select_variant(
    game_dir: &Path,
    torrent_root: &Path,
    cfg: &HashMap<String, String>,
) -> Result<Variant, String> {
    let level1 = subdirs(game_dir);
    if level1.is_empty() {
        return Err(format!(
            "{} holds no game folder - the archive may be incomplete.",
            game_dir.display()
        ));
    }
    let wanted = cfg.get("svm_variant").map(String::as_str);
    let pick1 = wanted
        .filter(|w| level1.iter().any(|n| n == w))
        .map(str::to_string)
        // "Other Languages" is a menu of its own in eXo; the default is the
        // first real version.
        .or_else(|| level1.iter().find(|n| *n != "Other Languages").cloned())
        .unwrap_or_else(|| level1[0].clone());
    let dir1 = game_dir.join(&pick1);
    let mut names = vec![pick1.clone()];
    let run_dir = if dir1.join("menu.txt").is_file() {
        let level2 = subdirs(&dir1);
        if level2.is_empty() {
            return Err(format!("{} lists a sub-menu but holds no versions.", dir1.display()));
        }
        let pick2 = cfg
            .get("svm_sub")
            .filter(|w| level2.iter().any(|n| n == *w))
            .cloned()
            .unwrap_or_else(|| level2[0].clone());
        names.insert(0, pick2.clone());
        dir1.join(pick2)
    } else {
        dir1.clone()
    };

    let dirs: [&Path; 2] = [&run_dir, &dir1];
    let platform = probe(&dirs, "platform.txt").and_then(|p| first_field(&p));
    let mut extra: Vec<String> = Vec::new();

    // Sound: the menu eXo shows at launch. Its answer picks a music driver and
    // a `<prefix>audio.txt` with per-choice extras; without the menu the base
    // flags plus a plain audio.txt apply.
    let base = ["--opl-driver=nuked", "--output-rate=44100"];
    let mt32_dir = torrent_root.join("eXo/mt32");
    let sound_menu = probe(&dirs, "sound.txt")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| {
            t.lines()
                .next()
                .unwrap_or("")
                .split(':')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut audio_prefix = String::new();
    if sound_menu.is_empty() {
        extra.extend(base.iter().map(|s| s.to_string()));
    } else {
        let choice = cfg
            .get("svm_sound")
            .filter(|w| sound_menu.iter().any(|s| s == *w))
            .cloned()
            .unwrap_or_else(|| sound_menu[0].clone());
        let rev0 = probe(&dirs, "mt32_rev0.txt").is_some();
        let (prefix, flags): (&str, Vec<String>) = match choice.as_str() {
            "Tandy" => ("T_", vec!["--opl-driver=nuked".into(), "-epcjr".into(), "--output-rate=44100".into()]),
            "Creative Music System (CMS)" => ("CMS_", vec!["--opl-driver=nuked".into(), "-ecms".into(), "--output-rate=44100".into()]),
            "Roland MT32" if mt32_dir.is_dir() => {
                let roms = if rev0 { mt32_dir.join("rev0") } else { mt32_dir.clone() };
                ("mt32_", vec![
                    "--opl-driver=nuked".into(),
                    "-emt32".into(),
                    "--multi-midi".into(),
                    format!("--extrapath={}", roms.display()),
                    "--output-rate=44100".into(),
                ])
            }
            "Sound Canvas" if mt32_dir.join("SoundCanvas.sf2").is_file() => ("sc_", vec![
                "--opl-driver=nuked".into(),
                "-efluidsynth".into(),
                "--multi-midi".into(),
                format!("--soundfont={}", mt32_dir.join("SoundCanvas.sf2").display()),
                "--output-rate=44100".into(),
            ]),
            "Roland MT32" | "Sound Canvas" => {
                log::warn!(
                    "scummvm: '{}' needs the MT-32 ROMs / soundfont under {}, which are not \
                     installed - falling back to Sound Blaster",
                    choice,
                    mt32_dir.display()
                );
                ("sb_", base.iter().map(|s| s.to_string()).collect())
            }
            "James Woodcock's Enhanced Music" => ("eh_", base.iter().map(|s| s.to_string()).collect()),
            _ => ("sb_", base.iter().map(|s| s.to_string()).collect()),
        };
        audio_prefix = prefix.to_string();
        extra.extend(flags);
    }
    let audio_file = if audio_prefix.is_empty() {
        probe(&dirs, "audio.txt")
    } else {
        probe(&dirs, &format!("{audio_prefix}audio.txt")).or_else(|| probe(&dirs, "audio.txt"))
    };
    if let Some(args) = audio_file.and_then(|p| first_field(&p)) {
        extra.extend(args.split_whitespace().map(str::to_string));
    }
    if let Some(args) = probe(&dirs, "video.txt").and_then(|p| first_field(&p)) {
        extra.extend(args.split_whitespace().map(str::to_string));
    }
    if cfg.get("svm_subtitles").map(String::as_str) == Some("true") {
        if let Some(args) = probe(&dirs, "subtitle.txt").and_then(|p| first_field(&p)) {
            extra.extend(args.split_whitespace().map(str::to_string));
        }
    }
    let note = std::fs::read_to_string(game_dir.join("note.txt"))
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());

    Ok(Variant { run_dir, names, platform, extra_args: extra, note })
}

#[cfg(windows)]
fn hide_console(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}
#[cfg(not(windows))]
fn hide_console(_cmd: &mut std::process::Command) {}

/// Ask ScummVM whether it recognises the game data before launching it. A
/// launch with unrecognised data does NOT fail: ScummVM logs a warning and
/// opens its own launcher GUI, which from Exodium's side looks like a game
/// that started - so the check has to happen up front. Placeholders and
/// piece fragments (see `extract_before_launch`) land here too.
fn detect(resolved: &Resolved, ini: &Path, run_dir: &Path, game_id: &str, grant: &Path) -> Result<(), String> {
    let (mut cmd, _) = resolved.cmd.command(grant);
    hide_console(&mut cmd);
    cmd.arg(format!("--config={}", ini.display()))
        .arg("--detect")
        .arg(format!("--path={}", run_dir.display()));
    // Same posix_spawn EBADF as the emulator spawn (see
    // spawn_emulator_and_track): a Tauri GUI build on macOS gets "Bad file
    // descriptor" from `output()` unless the child goes through fork+exec.
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| Ok(()));
        }
    }
    let out = cmd.output().map_err(|e| format!("could not run ScummVM: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Output is a table: header, separator, one row per detected game whose
    // first column is the engine:gameid. A named target cannot be matched
    // by id (the id lives in the ini), so any row counts for those.
    let rows: Vec<&str> = stdout
        .lines()
        .skip_while(|l| !l.starts_with("---"))
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .collect();
    let ok = if game_id.contains(':') {
        rows.iter().any(|r| r.split_whitespace().next() == Some(game_id))
    } else {
        !rows.is_empty()
    };
    if ok {
        return Ok(());
    }
    log::warn!(
        "scummvm --detect found nothing for {} in {} (exit {:?}):\n{}\n{}",
        game_id,
        run_dir.display(),
        out.status.code(),
        stdout.trim(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Err(format!(
        "ScummVM does not recognise the game data in {} (expected {}). \
         The download may be incomplete - try re-downloading the game.",
        run_dir.display(),
        game_id
    ))
}

pub(crate) async fn launch_scummvm_game(
    app: &AppHandle,
    game: Game,
    id: i64,
    data_dir: &str,
    fullscreen: bool,
    per_game_config: &HashMap<String, String>,
) -> Result<String, String> {
    let title = game.title.clone();
    launch_inner(app, game, id, data_dir, fullscreen, per_game_config)
        .await
        .inspect_err(|e| log::error!("launch_scummvm_game({}): {}", title, e))
}

async fn launch_inner(
    app: &AppHandle,
    game: Game,
    id: i64,
    data_dir: &str,
    fullscreen: bool,
    per_game_config: &HashMap<String, String>,
) -> Result<String, String> {
    let source = game.torrent_source.as_deref().unwrap_or("eXoScummVM");
    let torrent_root = crate::commands::setup::game_root(data_dir);
    let game_dir = super::games::extract_before_launch(app, &game, id, source, &torrent_root).await?;
    let variant = select_variant(&game_dir, &torrent_root, per_game_config)?;
    if let Some(note) = &variant.note {
        log::info!("eXo note for {}: {}", game.title, note);
    }

    let zip_stem = game.shortcode.clone().unwrap_or_default();
    let mut names: Vec<&str> = variant.names.iter().map(String::as_str).collect();
    names.push(&zip_stem);
    let entry = lookup(&names).ok_or_else(|| {
        format!("'{}' is not in eXo's ScummVM launch index ({}).", game.title, names.join(" / "))
    })?;

    let resolved = resolve_scummvm(&torrent_root, data_dir, &entry.build).ok_or_else(|| {
        format!(
            "ScummVM was not found. Install ScummVM from scummvm.org{} and try again.",
            if cfg!(target_os = "linux") { " (or the Flatpak org.scummvm.ScummVM)" } else { "" }
        )
    })?;
    if resolved.source != EngineSource::Exo && resolved.source != EngineSource::Pack {
        log::info!(
            "scummvm: '{}' is pinned to ScummVM {} by eXo; running the system copy instead",
            game.title,
            build_version(&entry.build)
        );
    }
    let ini = ensure_config(data_dir, &entry.build)?;
    // Saves inside the game dir, so uninstall's rename-backup and Reset
    // (§5) carry them; ScummVM's default would be a per-user folder outside
    // the library.
    let savepath = game_dir.join("!saves");
    std::fs::create_dir_all(&savepath).map_err(|e| format!("cannot create {}: {e}", savepath.display()))?;

    detect(&resolved, &ini, &variant.run_dir, &entry.game_id, &torrent_root)?;

    let fullscreen = match per_game_config.get("fullscreen").map(String::as_str) {
        Some("true") => true,
        Some("false") => false,
        _ => fullscreen,
    };
    let (mut cmd, bin) = resolved.cmd.command(&torrent_root);
    // eXo's line, minus `--no-console` (a Windows-only option that the macOS
    // and Linux binaries reject outright).
    if cfg!(windows) {
        cmd.arg("--no-console");
    }
    cmd.arg(if fullscreen { "-f" } else { "-F" });
    if per_game_config.get("svm_aspect").map(String::as_str) != Some("false") {
        cmd.arg("--aspect-ratio");
    }
    cmd.arg("--stretch-mode=pixel-perfect");
    cmd.arg(format!("--config={}", ini.display()));
    cmd.args(&variant.extra_args);
    if let Some(p) = &variant.platform {
        cmd.arg(format!("--platform={p}"));
    }
    cmd.arg(format!("--savepath={}", savepath.display()));
    cmd.arg(format!("--path={}", variant.run_dir.display()));
    cmd.arg(&entry.game_id);
    cmd.current_dir(&variant.run_dir);
    log::info!(
        "Launching {} via ScummVM {} ({:?}): {} in {}",
        game.title,
        build_version(&entry.build),
        resolved.source,
        entry.game_id,
        variant.run_dir.display()
    );
    super::games::spawn_emulator_and_track(app, cmd, &bin, &game, id)
}

#[derive(Debug, Clone, Serialize)]
pub struct ScummVmEngineInfo {
    pub available: bool,
    /// The ScummVM version eXo pins this game to.
    pub pinned_version: String,
    /// Where the binary that would run comes from; None when nothing resolves.
    pub source: Option<EngineSource>,
}

/// What the panel shows next to Play: answered by the launcher's own resolver
/// so the note can never disagree with an actual launch.
#[tauri::command]
pub async fn scummvm_engine_info(
    db_state: State<'_, DbState>,
    id: i64,
) -> Result<ScummVmEngineInfo, String> {
    let (variant, data_dir) = {
        let conn = db_state.lock()?;
        let game = queries::fetch_game_by_id(&conn, id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("Game with id {} not found", id))?;
        let data_dir = queries::get_config(&conn, "data_dir").map_err(|e| e.to_string())?;
        (game.dosbox_variant.unwrap_or_else(|| "default".to_string()), data_dir)
    };
    let Some(data_dir) = data_dir else {
        return Ok(ScummVmEngineInfo {
            available: false,
            pinned_version: build_version(&variant).to_string(),
            source: None,
        });
    };
    let torrent_root = crate::commands::setup::game_root(&data_dir);
    let resolved = resolve_scummvm(&torrent_root, &data_dir, &variant);
    Ok(ScummVmEngineInfo {
        available: resolved.is_some(),
        pinned_version: build_version(&variant).to_string(),
        source: resolved.map(|r| r.source),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    /// Maniac Mansion's tree: nine platform folders, DOS v1 with a sub-menu
    /// of render modes. Defaults pick the first entry at every level, like
    /// eXo's menu; stored choices override.
    #[test]
    fn variant_selection_walks_menu_levels_and_control_files() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("Maniac Mansion (Multi-Platform)");
        let root = tmp.path().join("root");
        write(&game.join("Maniac Mansion (Amiga)/platform.txt"), "amiga\n");
        write(&game.join("Maniac Mansion (DOS v1)/menu.txt"), "");
        write(&game.join("Maniac Mansion (DOS v1)/Maniac Mansion CGA/video.txt"), "--render-mode=CGA\n");
        write(&game.join("Maniac Mansion (DOS v1)/Maniac Mansion EGA/.keep"), "");
        write(&game.join("Maniac Mansion (NES, English, USA)/platform.txt"), "nes\n");
        write(&game.join("note.txt"), "Audio is off in this port.\n");

        let v = select_variant(&game, &root, &HashMap::new()).unwrap();
        assert_eq!(v.run_dir, game.join("Maniac Mansion (Amiga)"));
        assert_eq!(v.platform.as_deref(), Some("amiga"));
        assert_eq!(v.names, vec!["Maniac Mansion (Amiga)".to_string()]);
        assert_eq!(v.note.as_deref(), Some("Audio is off in this port."));
        assert!(v.extra_args.contains(&"--opl-driver=nuked".to_string()));

        let mut cfg = HashMap::new();
        cfg.insert("svm_variant".to_string(), "Maniac Mansion (DOS v1)".to_string());
        let v = select_variant(&game, &root, &cfg).unwrap();
        assert_eq!(v.run_dir, game.join("Maniac Mansion (DOS v1)/Maniac Mansion CGA"));
        assert_eq!(v.names[0], "Maniac Mansion CGA");
        assert_eq!(v.names[1], "Maniac Mansion (DOS v1)");
        assert!(v.extra_args.contains(&"--render-mode=CGA".to_string()));
        assert!(v.platform.is_none());

        cfg.insert("svm_sub".to_string(), "Maniac Mansion EGA".to_string());
        let v = select_variant(&game, &root, &cfg).unwrap();
        assert_eq!(v.run_dir, game.join("Maniac Mansion (DOS v1)/Maniac Mansion EGA"));
        assert!(!v.extra_args.iter().any(|a| a.starts_with("--render-mode")));
    }

    /// The sound menu maps onto music-driver flags; MT-32 needs the ROMs and
    /// falls back to the base flags without them rather than failing.
    #[test]
    fn sound_menu_picks_driver_flags() {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("Out of this World (Multi-Platform)");
        let root = tmp.path().join("root");
        write(&game.join("Out of this World (DOS)/sound.txt"), "Sound Blaster:Roland MT32\n");
        write(&game.join("Out of this World (DOS)/mt32_audio.txt"), "--midi-gain=50\n");

        let v = select_variant(&game, &root, &HashMap::new()).unwrap();
        assert!(!v.extra_args.contains(&"-emt32".to_string()));

        let mut cfg = HashMap::new();
        cfg.insert("svm_sound".to_string(), "Roland MT32".to_string());
        let v = select_variant(&game, &root, &cfg).unwrap();
        assert!(!v.extra_args.contains(&"-emt32".to_string()), "no ROMs -> no MT-32");

        std::fs::create_dir_all(root.join("eXo/mt32")).unwrap();
        let v = select_variant(&game, &root, &cfg).unwrap();
        assert!(v.extra_args.contains(&"-emt32".to_string()));
        assert!(v.extra_args.iter().any(|a| a.starts_with("--extrapath=")));
        assert!(v.extra_args.contains(&"--midi-gain=50".to_string()));
    }

    /// `findstr /b` semantics: exact directory first, then the first line
    /// whose directory starts with the name.
    #[test]
    fn index_lookup_prefers_exact_over_prefix() {
        let idx = index();
        if idx.is_empty() {
            eprintln!("Skipping: bundled scummvm.txt not found");
            return;
        }
        let e = lookup(&["Maniac Mansion EGA", "Maniac Mansion (DOS v1)", "Maniac Mansion (Multi-Platform)"]).unwrap();
        assert_eq!(e.game_id, "scumm:maniac");
        assert_eq!(e.build, "2.9.0");
        let e = lookup(&["Maniac Mansion"]).unwrap();
        assert!(e.dir.starts_with("Maniac Mansion"));
        let e = lookup(&["Arthur - The Quest for Excalibur (DOS)"]).unwrap();
        assert_eq!(e.build, "svn2.8_9335");
        assert_eq!(build_version(&e.build), "2.8.0git9335-g00e72a17004");
    }
}
