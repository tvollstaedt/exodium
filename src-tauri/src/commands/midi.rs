//! The user's two global MIDI outputs, General MIDI and MT-32 (#51). A DOS
//! game's own conf decides which one applies (`mididevice = mt32` or not),
//! and every launcher spells the answer in its engine's keys (§9).

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::db::queries;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GmOutput {
    /// Today's behaviour: the game conf's own device, nothing written.
    #[default]
    Default,
    FluidSynth,
    /// Nuked SC-55 via Staging's CLAP plugin; ECE and DOSBox-X have none.
    SoundCanvas,
    /// A host MIDI port, by name.
    Port(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Mt32Output {
    /// Munt with the collection's ROMs, as the conf says.
    #[default]
    Default,
    Port(String),
}

/// What the user's ROM folder holds, resolved once per launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MidiRoms {
    pub mt32: Option<PathBuf>,
    pub soundcanvas: Option<PathBuf>,
    pub soundfont: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MidiPrefs {
    pub gm: GmOutput,
    pub mt32: Mt32Output,
    pub roms: MidiRoms,
}

/// Whether Standard means FluidSynth with the collection's soundfont rather
/// than the conf's own device: off Windows `port` reaches no synth without
/// MIDI hardware, so eXo's General MIDI setups played no music there (§9).
pub fn standard_is_fluidsynth() -> bool {
    !cfg!(windows)
}

const PORT_PREFIX: &str = "port:";

pub fn parse_gm(value: &str) -> GmOutput {
    match value.trim() {
        "fluidsynth" => GmOutput::FluidSynth,
        "soundcanvas" => GmOutput::SoundCanvas,
        v => match v.strip_prefix(PORT_PREFIX).filter(|n| !n.is_empty()) {
            Some(name) => GmOutput::Port(name.to_string()),
            None => GmOutput::Default,
        },
    }
}

pub fn parse_mt32(value: &str) -> Mt32Output {
    match value.trim().strip_prefix(PORT_PREFIX).filter(|n| !n.is_empty()) {
        Some(name) => Mt32Output::Port(name.to_string()),
        None => Mt32Output::Default,
    }
}

/// Staging's own layout: `mt32-roms/`, `soundcanvas-roms/`, `soundfonts/`.
/// Only subfolders that exist count; the first `.sf2` by name is the font.
pub fn resolve_roms(rom_dir: &str) -> MidiRoms {
    let root = Path::new(rom_dir.trim());
    if rom_dir.trim().is_empty() || !root.is_dir() {
        return MidiRoms::default();
    }
    let dir = |leaf: &str| Some(root.join(leaf)).filter(|p| p.is_dir());
    let soundfont = dir("soundfonts").and_then(|d| {
        let mut fonts: Vec<PathBuf> = std::fs::read_dir(d)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("sf2")))
            .collect();
        fonts.sort();
        fonts.into_iter().next()
    });
    MidiRoms { mt32: dir("mt32-roms"), soundcanvas: dir("soundcanvas-roms"), soundfont }
}

/// The three config values as stored. Read under the DB lock; `resolve`
/// looks at the ROM folder, which may sit on a share, so it runs after the
/// lock is released (§22).
#[derive(Debug, Clone, Default)]
pub struct MidiConfig {
    gm: String,
    mt32: String,
    rom_dir: String,
}

impl MidiConfig {
    pub fn load(conn: &Connection) -> Result<Self, String> {
        let read = |key: &str| -> Result<String, String> {
            Ok(queries::get_config(conn, key).map_err(|e| e.to_string())?.unwrap_or_default())
        };
        Ok(Self {
            gm: read("midi_gm_output")?,
            mt32: read("midi_mt32_output")?,
            rom_dir: read("midi_rom_dir")?,
        })
    }

    pub fn resolve(&self) -> MidiPrefs {
        MidiPrefs {
            gm: parse_gm(&self.gm),
            mt32: parse_mt32(&self.mt32),
            roms: resolve_roms(&self.rom_dir),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DosboxEngine {
    Staging,
    Ece,
    DosboxX,
}

/// The conf's `[midi] mididevice`, lowercased. `mt32` means the game's own
/// SETUP chose MT-32 patches, so the user's MT-32 output applies; anything
/// else is General MIDI.
pub fn conf_midi_device(conf: &str) -> Option<String> {
    let mut in_midi = false;
    let mut device: Option<String> = None;
    for line in conf.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_midi = trimmed.eq_ignore_ascii_case("[midi]");
            continue;
        }
        if !in_midi || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = trimmed.split_once('=') {
            if key.trim().eq_ignore_ascii_case("mididevice") {
                device = Some(value.trim().to_ascii_lowercase());
            }
        }
    }
    device
}

/// Devices that send General MIDI to the host's port - the ones Standard may
/// replace. A conf that names a synth (eXo's FluidSynth confs carry their
/// own soundfont) keeps it.
fn routes_to_port(device: &str) -> bool {
    matches!(device, "" | "auto" | "default" | "port" | "coremidi" | "alsa" | "win32" | "oss")
}

/// The `midiconfig` value naming a stored port for this engine. Staging and
/// DOSBox-X on Linux take ALSA's numeric `client:port`, resolved from the
/// live listing because the number changes with every replug; elsewhere
/// the name (matched as a substring) is the stable handle.
pub fn port_config(name: &str) -> String {
    if cfg!(target_os = "linux") {
        return alsa_address(name).unwrap_or_else(|| name.to_string());
    }
    name.to_string()
}

#[cfg(target_os = "linux")]
fn alsa_address(name: &str) -> Option<String> {
    let live = list_outputs();
    let wanted = strip_alsa_address(name);
    live.iter()
        .find(|p| p.as_str() == name)
        .or_else(|| live.iter().find(|p| strip_alsa_address(p) == wanted))
        .and_then(|p| trailing_alsa_address(p))
        .or_else(|| trailing_alsa_address(name))
        .map(str::to_string)
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
fn alsa_address(_name: &str) -> Option<String> {
    None
}

/// midir spells an ALSA port `<client>:<port name> <client#>:<port#>`.
fn trailing_alsa_address(port_name: &str) -> Option<&str> {
    let tail = port_name.rsplit(' ').next()?;
    let (client, port) = tail.split_once(':')?;
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (numeric(client) && numeric(port)).then_some(tail)
}

fn strip_alsa_address(port_name: &str) -> &str {
    match trailing_alsa_address(port_name) {
        Some(tail) => port_name[..port_name.len() - tail.len()].trim_end(),
        None => port_name,
    }
}

/// The ALSA client name, which is what ScummVM lists a device under.
fn alsa_client_name(port_name: &str) -> &str {
    strip_alsa_address(port_name).split(':').next().unwrap_or(port_name)
}

/// ScummVM's `--music-driver` id for a stored port: `<driver>_<name>`, the
/// driver id joined to the name its backend reports (CoreMIDI display
/// name, ALSA client name, WinMM product name).
pub fn scummvm_device_id(name: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("coremidi_{name}")
    } else if cfg!(target_os = "linux") {
        format!("alsa_{}", alsa_client_name(name))
    } else {
        format!("windows_{name}")
    }
}

fn dosbox_x_port_device() -> &'static str {
    if cfg!(target_os = "macos") {
        "coremidi"
    } else if cfg!(target_os = "linux") {
        "alsa"
    } else {
        "win32"
    }
}

/// `(mididevice, midiconfig)` that send to a stored port under `engine`.
fn port_settings(engine: DosboxEngine, name: &str) -> (&'static str, String) {
    let device = match engine {
        DosboxEngine::Staging => "port",
        DosboxEngine::Ece => "win32",
        DosboxEngine::DosboxX => dosbox_x_port_device(),
    };
    (device, port_config(name))
}

/// The `-conf` fragment carrying the user's MIDI choice for one DOS launch.
/// Empty when the chosen output is the game conf's own; ECE and DOSBox-X
/// have no Sound Canvas, so that choice writes nothing there too.
pub fn dosbox_frag(
    engine: DosboxEngine,
    conf_device: Option<&str>,
    prefs: &MidiPrefs,
    exo_soundfont: Option<&Path>,
) -> String {
    let mut midi: Vec<String> = Vec::new();
    let mut sections: Vec<String> = Vec::new();
    let staging = engine == DosboxEngine::Staging;
    let wants_mt32 = conf_device == Some("mt32");
    let port = |name: &str, midi: &mut Vec<String>| {
        let (device, config) = port_settings(engine, name);
        midi.push(format!("mididevice = {device}"));
        midi.push(format!("midiconfig = {config}"));
    };
    let fluidsynth = |sf: &Path, midi: &mut Vec<String>, sections: &mut Vec<String>| {
        midi.push("mididevice = fluidsynth".to_string());
        if staging {
            sections.push(format!("[fluidsynth]\nsoundfont = {}\n", sf.display()));
        } else {
            midi.push(format!("fluid.soundfont = {}", sf.display()));
        }
    };
    let soundfont = prefs.roms.soundfont.as_deref().or(exo_soundfont);

    if wants_mt32 {
        match &prefs.mt32 {
            Mt32Output::Port(name) => port(name, &mut midi),
            Mt32Output::Default => {
                if let Some(dir) = &prefs.roms.mt32 {
                    if staging {
                        sections.push(format!("[mt32]\nromdir = {}\n", dir.display()));
                    } else {
                        midi.push(format!("mt32.romdir = {}", dir.display()));
                    }
                }
            }
        }
    } else {
        match &prefs.gm {
            GmOutput::Port(name) => port(name, &mut midi),
            // Without a soundfont Staging falls back to `port` on its own,
            // so the game's own device is no worse.
            GmOutput::FluidSynth => match soundfont {
                Some(sf) => fluidsynth(sf, &mut midi, &mut sections),
                None => log::info!("FluidSynth output without a soundfont on disk; keeping the game's own MIDI device"),
            },
            GmOutput::SoundCanvas if staging => {
                midi.push("mididevice = soundcanvas".to_string());
                if let Some(dir) = &prefs.roms.soundcanvas {
                    sections.push(format!("[soundcanvas]\nsoundcanvas_rom_dir = {}\n", dir.display()));
                }
            }
            GmOutput::SoundCanvas => {
                log::info!("Sound Canvas output needs DOSBox Staging; keeping the game's own MIDI device");
            }
            GmOutput::Default => {
                let replaceable = conf_device.is_none_or(routes_to_port);
                if let Some(sf) = soundfont.filter(|_| staging && standard_is_fluidsynth() && replaceable) {
                    fluidsynth(sf, &mut midi, &mut sections);
                }
            }
        }
    }

    let mut frag = String::new();
    if !midi.is_empty() {
        frag.push_str("[midi]\n");
        for line in midi {
            frag.push_str(&line);
            frag.push('\n');
        }
    }
    for section in sections {
        frag.push_str(&section);
    }
    frag
}

/// The user's ports as a game's own sound menu reaches them; `None` keeps
/// the menu's device. Needed because a launcher's `CONFIG -set` switches the
/// device at runtime, after the conf fragment applied (§9).
#[derive(Debug, Default)]
pub struct MenuPorts {
    mt32: Option<(&'static str, String)>,
    gm: Option<(&'static str, String)>,
}

impl MenuPorts {
    pub fn new(engine: DosboxEngine, prefs: &MidiPrefs) -> Self {
        let side = |name: Option<&String>| {
            let (device, config) = port_settings(engine, name?);
            if config.chars().all(batch_safe) {
                Some((device, config))
            } else {
                log::warn!("MIDI port {config:?} cannot be written into a batch file; game sound menus keep their own device");
                None
            }
        };
        let mt32 = match &prefs.mt32 {
            Mt32Output::Port(name) => Some(name),
            Mt32Output::Default => None,
        };
        let gm = match &prefs.gm {
            GmOutput::Port(name) => Some(name),
            _ => None,
        };
        Self { mt32: side(mt32), gm: side(gm) }
    }

    fn for_device(&self, device: &str) -> Option<&(&'static str, String)> {
        match device {
            "mt32" => self.mt32.as_ref(),
            "fluidsynth" | "soundcanvas" => self.gm.as_ref(),
            _ => None,
        }
    }
}

/// ASCII only, since the shell reads the bat as code-page text; `%` would be
/// expanded, `"` would end the argument, `<>|` would redirect.
fn batch_safe(c: char) -> bool {
    (c == ' ' || c.is_ascii_graphic()) && !matches!(c, '"' | '%' | '<' | '>' | '|')
}

const MENU_MARK: &str = "@REM exodium-midi: ";
const MENU_MARK_END: &str = "@REM exodium-midi end";

/// Point a launcher's sound-menu lines (`CONFIG -set "mididevice=mt32"`) at
/// the user's ports. Blocks from an earlier launch are restored to eXo's line
/// first, so the result depends only on eXo's text and `ports`. `text` holds
/// one char per byte, which keeps a CP437 menu intact.
pub fn redirect_menu_devices(text: &str, ports: &MenuPorts, bat: &str) -> String {
    let restored = restore_menu_devices(text);
    if ports.mt32.is_none() && ports.gm.is_none() {
        return restored;
    }
    let fallback_eol = if restored.contains("\r\n") { "\r\n" } else { "\n" };
    let mut out = String::with_capacity(restored.len() + 256);
    for line in restored.split_inclusive('\n') {
        let (body, eol) = split_eol(line);
        // DOSBox's shell redirects even on a REM line, so the original could
        // not be kept in the marker.
        let redirect = body.find(['<', '>', '|']);
        let target = menu_device(&body[..redirect.unwrap_or(body.len())])
            .and_then(|(head_len, device)| ports.for_device(&device).map(|port| (head_len, port)));
        match target {
            Some(_) if redirect.is_some() => {
                log::warn!("Leaving MIDI line in {bat} unchanged, it redirects: {}", body.trim());
                out.push_str(line);
            }
            Some((head_len, (device, config))) => {
                let head = &body[..head_len];
                let inner_eol = if eol.is_empty() { fallback_eol } else { eol };
                out.push_str(&format!("{MENU_MARK}{body}{inner_eol}"));
                out.push_str(&format!("{head}\"midiconfig={config}\"{inner_eol}"));
                out.push_str(&format!("{head}\"mididevice={device}\"{inner_eol}"));
                out.push_str(&format!("{MENU_MARK_END}{eol}"));
            }
            None => out.push_str(line),
        }
    }
    out
}

/// Every complete marker block back to the original line it carries, with
/// the line ending the end marker kept for it.
fn restore_menu_devices(text: &str) -> String {
    if !text.contains(MENU_MARK) {
        return text.to_string();
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < lines.len() {
        if let Some(orig) = split_eol(lines[i]).0.strip_prefix(MENU_MARK) {
            let next_mark = lines[i + 1..].iter().position(|l| {
                let body = split_eol(l).0;
                body == MENU_MARK_END || body.starts_with(MENU_MARK)
            });
            if let Some(end) = next_mark.map(|k| i + 1 + k) {
                let (end_body, end_eol) = split_eol(lines[end]);
                if end_body == MENU_MARK_END {
                    out.push_str(orig);
                    out.push_str(end_eol);
                    i = end + 1;
                    continue;
                }
            }
        }
        out.push_str(lines[i]);
        i += 1;
    }
    out
}

fn split_eol(line: &str) -> (&str, &str) {
    let body = match line.strip_suffix('\n') {
        Some(l) => l.strip_suffix('\r').unwrap_or(l),
        None => line,
    };
    (body, &line[body.len()..])
}

/// `(length of the text before the argument, device)` for a line that sets
/// `mididevice` through `CONFIG -set`. Only `@` and `if [not] exist` may
/// precede it - the shapes eXo writes, and a file test holds across the
/// repeated lines.
fn menu_device(body: &str) -> Option<(usize, String)> {
    let mut rest = body.trim_start_matches(is_sep);
    rest = rest.strip_prefix('@').unwrap_or(rest);
    loop {
        let (word, after) = first_word(rest);
        if !word.eq_ignore_ascii_case("if") {
            break;
        }
        let (mut word, mut after) = first_word(after);
        if word.eq_ignore_ascii_case("not") {
            (word, after) = first_word(after);
        }
        let (target, after) = first_word(after);
        if !word.eq_ignore_ascii_case("exist") || target.is_empty() {
            return None;
        }
        rest = after;
    }
    let (config, after) = first_word(rest);
    let (set, after) = first_word(after);
    if !config.eq_ignore_ascii_case("config") || !set.eq_ignore_ascii_case("-set") {
        return None;
    }
    let payload = after.trim_start_matches(is_sep);
    let head_len = body.len() - payload.len();
    let arg = payload.trim_end_matches(is_sep);
    let arg = match arg.strip_prefix('"') {
        Some(quoted) => quoted.strip_suffix('"')?,
        None => arg,
    };
    let (key, value) = arg.split_once('=')?;
    let value = value.trim_matches(is_sep);
    let plain = !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric());
    (key.trim_matches(is_sep).eq_ignore_ascii_case("mididevice") && plain)
        .then(|| (head_len, value.to_ascii_lowercase()))
}

/// The shell's separators. Not `char::is_whitespace`: as one char per byte,
/// CP437 letters 0x85 and 0xA0 read as Unicode spaces.
fn is_sep(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn first_word(s: &str) -> (&str, &str) {
    let s = s.trim_start_matches(is_sep);
    let end = s.find(is_sep).unwrap_or(s.len());
    (&s[..end], &s[end..])
}

/// The host's MIDI output ports by name. Names, never indices: an index
/// moves with every replug, and the engines match on the name.
pub fn list_outputs() -> Vec<String> {
    let Ok(out) = midir::MidiOutput::new("Exodium") else {
        return Vec::new();
    };
    out.ports().iter().filter_map(|p| out.port_name(p).ok()).collect()
}

#[tauri::command]
pub async fn list_midi_outputs() -> Result<Vec<String>, String> {
    tokio::task::spawn_blocking(list_outputs)
        .await
        .map_err(|e| format!("MIDI port listing failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(gm: GmOutput, mt32: Mt32Output) -> MidiPrefs {
        MidiPrefs { gm, mt32, roms: MidiRoms::default() }
    }

    #[test]
    fn config_values_parse_to_outputs() {
        assert_eq!(parse_gm(""), GmOutput::Default);
        assert_eq!(parse_gm("fluidsynth"), GmOutput::FluidSynth);
        assert_eq!(parse_gm("soundcanvas"), GmOutput::SoundCanvas);
        assert_eq!(parse_gm("port:loopMIDI Port A"), GmOutput::Port("loopMIDI Port A".into()));
        assert_eq!(parse_gm("port:"), GmOutput::Default);
        assert_eq!(parse_mt32("port:Roland UM-ONE"), Mt32Output::Port("Roland UM-ONE".into()));
        assert_eq!(parse_mt32("fluidsynth"), Mt32Output::Default);
    }

    #[test]
    fn conf_device_comes_from_the_midi_section_only() {
        assert_eq!(conf_midi_device("[midi]\nmididevice = mt32\nmpu401 = intelligent\n").as_deref(), Some("mt32"));
        assert_eq!(conf_midi_device("[sdl]\nfullscreen=false\n[MIDI]\nMIDIDEVICE=MT32\n").as_deref(), Some("mt32"));
        assert_eq!(conf_midi_device("[midi]\nmididevice = auto\n[mt32]\nromdir = /x\n").as_deref(), Some("auto"));
        assert_eq!(conf_midi_device("[midi]\n# mididevice = mt32\nmididevice = fluidsynth\n").as_deref(), Some("fluidsynth"));
        assert_eq!(conf_midi_device("[autoexec]\nmididevice = mt32\n"), None);
        assert_eq!(conf_midi_device(""), None);
    }

    /// eXo's FluidSynth confs pick their own soundfont; Standard leaves them.
    #[test]
    fn standard_keeps_a_conf_that_names_a_synth() {
        let p = MidiPrefs::default();
        let sf = Some(Path::new("/exo/mt32/SoundCanvas.sf2"));
        assert_eq!(dosbox_frag(DosboxEngine::Staging, Some("fluidsynth"), &p, sf), "");
        let auto = dosbox_frag(DosboxEngine::Staging, Some("auto"), &p, sf);
        assert_eq!(auto.is_empty(), cfg!(windows), "{auto}");
    }

    /// Standard keeps the conf's device, except General MIDI under Staging off
    /// Windows, which gets FluidSynth once a soundfont is on disk.
    #[test]
    fn standard_is_the_confs_own_device_or_fluidsynth_off_windows() {
        let p = MidiPrefs::default();
        let sf = Some(Path::new("/exo/mt32/SoundCanvas.sf2"));
        for engine in [DosboxEngine::Ece, DosboxEngine::DosboxX] {
            for device in [None, Some("mt32")] {
                assert_eq!(dosbox_frag(engine, device, &p, sf), "");
            }
        }
        assert_eq!(dosbox_frag(DosboxEngine::Staging, Some("mt32"), &p, sf), "");
        assert_eq!(dosbox_frag(DosboxEngine::Staging, None, &p, None), "");
        let gm = dosbox_frag(DosboxEngine::Staging, None, &p, sf);
        if cfg!(windows) {
            assert_eq!(gm, "");
        } else {
            assert_eq!(gm, "[midi]\nmididevice = fluidsynth\n[fluidsynth]\nsoundfont = /exo/mt32/SoundCanvas.sf2\n");
        }
    }

    #[test]
    fn the_conf_picks_which_output_applies() {
        let p = prefs(GmOutput::FluidSynth, Mt32Output::Port("UM-ONE".into()));
        let gm = dosbox_frag(DosboxEngine::Staging, None, &p, Some(Path::new("/sf.sf2")));
        assert_eq!(gm, "[midi]\nmididevice = fluidsynth\n[fluidsynth]\nsoundfont = /sf.sf2\n");
        assert_eq!(dosbox_frag(DosboxEngine::Staging, None, &p, None), "", "no soundfont, no FluidSynth");
        let mt32 = dosbox_frag(DosboxEngine::Staging, Some("mt32"), &p, None);
        assert_eq!(mt32, "[midi]\nmididevice = port\nmidiconfig = UM-ONE\n");
    }

    #[test]
    fn staging_spells_sections_and_the_port_device() {
        let p = MidiPrefs {
            gm: GmOutput::FluidSynth,
            mt32: Mt32Output::Default,
            roms: MidiRoms {
                mt32: Some("/roms/mt32-roms".into()),
                soundcanvas: Some("/roms/soundcanvas-roms".into()),
                soundfont: Some("/roms/soundfonts/a.sf2".into()),
            },
        };
        assert_eq!(
            dosbox_frag(DosboxEngine::Staging, None, &p, Some(Path::new("/exo/SoundCanvas.sf2"))),
            "[midi]\nmididevice = fluidsynth\n[fluidsynth]\nsoundfont = /roms/soundfonts/a.sf2\n"
        );
        assert_eq!(
            dosbox_frag(DosboxEngine::Staging, Some("mt32"), &p, None),
            "[mt32]\nromdir = /roms/mt32-roms\n"
        );
        let sc = MidiPrefs { gm: GmOutput::SoundCanvas, ..p.clone() };
        assert_eq!(
            dosbox_frag(DosboxEngine::Staging, None, &sc, None),
            "[midi]\nmididevice = soundcanvas\n[soundcanvas]\nsoundcanvas_rom_dir = /roms/soundcanvas-roms\n"
        );
        let port = prefs(GmOutput::Port("loopMIDI Port A".into()), Mt32Output::Default);
        let frag = dosbox_frag(DosboxEngine::Staging, None, &port, None);
        assert!(frag.starts_with("[midi]\nmididevice = port\nmidiconfig = "), "{frag}");
    }

    #[test]
    fn fluidsynth_falls_back_to_the_collections_soundfont() {
        let p = prefs(GmOutput::FluidSynth, Mt32Output::Default);
        let frag = dosbox_frag(DosboxEngine::Staging, None, &p, Some(Path::new("/exo/mt32/SoundCanvas.sf2")));
        assert!(frag.contains("[fluidsynth]\nsoundfont = /exo/mt32/SoundCanvas.sf2\n"), "{frag}");
    }

    /// ECE knows only the classic dotted keys, win32 ports and no Sound Canvas.
    #[test]
    fn ece_spells_classic_keys_and_skips_soundcanvas() {
        let roms = MidiRoms { mt32: Some("/roms/mt32-roms".into()), soundcanvas: None, soundfont: None };
        let p = MidiPrefs { gm: GmOutput::FluidSynth, mt32: Mt32Output::Default, roms };
        assert_eq!(
            dosbox_frag(DosboxEngine::Ece, None, &p, Some(Path::new("C:/exo/mt32/SoundCanvas.sf2"))),
            "[midi]\nmididevice = fluidsynth\nfluid.soundfont = C:/exo/mt32/SoundCanvas.sf2\n"
        );
        assert_eq!(dosbox_frag(DosboxEngine::Ece, Some("mt32"), &p, None), "[midi]\nmt32.romdir = /roms/mt32-roms\n");
        let sc = MidiPrefs { gm: GmOutput::SoundCanvas, ..p.clone() };
        assert_eq!(dosbox_frag(DosboxEngine::Ece, None, &sc, None), "");
        let port = prefs(GmOutput::Default, Mt32Output::Port("Roland UM-ONE".into()));
        let frag = dosbox_frag(DosboxEngine::Ece, Some("mt32"), &port, None);
        assert!(frag.starts_with("[midi]\nmididevice = win32\nmidiconfig = "), "{frag}");
        assert!(!frag.contains("[mt32]"));
    }

    #[test]
    fn dosbox_x_names_the_platform_device() {
        let p = prefs(GmOutput::Port("loopMIDI Port A".into()), Mt32Output::Default);
        let frag = dosbox_frag(DosboxEngine::DosboxX, None, &p, None);
        let expected = format!("[midi]\nmididevice = {}\nmidiconfig = ", dosbox_x_port_device());
        assert!(frag.starts_with(&expected), "{frag}");
        assert!(["coremidi", "alsa", "win32"].contains(&dosbox_x_port_device()));
        let sc = prefs(GmOutput::SoundCanvas, Mt32Output::Default);
        assert_eq!(dosbox_frag(DosboxEngine::DosboxX, None, &sc, None), "");
    }

    fn menu_ports(engine: DosboxEngine, gm: Option<&str>, mt32: Option<&str>) -> MenuPorts {
        let gm = gm.map_or(GmOutput::Default, |n| GmOutput::Port(n.into()));
        let mt32 = mt32.map_or(Mt32Output::Default, |n| Mt32Output::Port(n.into()));
        MenuPorts::new(engine, &prefs(gm, mt32))
    }

    fn latin1(bytes: &[u8]) -> String {
        bytes.iter().map(|&b| b as char).collect()
    }

    /// SQ5's menu shape, CRLF, with CP437 box drawing and umlauts around it.
    fn sq5_menu() -> String {
        let mut bytes = b"@echo off\r\necho \xC9\xCD\xBB Men\x81 \x85 \xA0\r\n:EMT32\r\n".to_vec();
        bytes.extend_from_slice(b"CONFIG -set \"mididevice=mt32\"\r\n:ESC55\r\nCONFIG -set \"mididevice=fluidsynth\"\r\n");
        bytes.extend_from_slice(b":ESB16\r\nCONFIG -set \"mididevice=default\"\r\n@SIERRA\r\n");
        latin1(&bytes)
    }

    #[test]
    fn an_mt32_menu_line_becomes_a_block_for_the_mt32_port() {
        let ports = menu_ports(DosboxEngine::Staging, None, Some("UM-ONE"));
        let out = redirect_menu_devices("CONFIG -set \"mididevice=mt32\"\r\n:next\r\n", &ports, "run.bat");
        assert_eq!(
            out,
            "@REM exodium-midi: CONFIG -set \"mididevice=mt32\"\r\n\
             CONFIG -set \"midiconfig=UM-ONE\"\r\n\
             CONFIG -set \"mididevice=port\"\r\n\
             @REM exodium-midi end\r\n:next\r\n"
        );
    }

    #[test]
    fn a_guarded_fluidsynth_line_repeats_its_guard() {
        let ports = menu_ports(DosboxEngine::Staging, Some("IAC Driver Bus 1"), None);
        let line = "if exist SC55.SEL CONFIG -set \"mididevice=fluidsynth\"\n";
        assert_eq!(
            redirect_menu_devices(line, &ports, "run.bat"),
            "@REM exodium-midi: if exist SC55.SEL CONFIG -set \"mididevice=fluidsynth\"\n\
             if exist SC55.SEL CONFIG -set \"midiconfig=IAC Driver Bus 1\"\n\
             if exist SC55.SEL CONFIG -set \"mididevice=port\"\n\
             @REM exodium-midi end\n"
        );
        let soundcanvas = "@ CONFIG -set \"mididevice=soundcanvas\"\n";
        let out = redirect_menu_devices(soundcanvas, &ports, "run.bat");
        assert!(out.contains("\n@ CONFIG -set \"midiconfig=IAC Driver Bus 1\"\n@ CONFIG -set \"mididevice=port\"\n"), "{out}");
    }

    #[test]
    fn each_port_touches_only_its_own_lines() {
        let text = sq5_menu();
        let gm = redirect_menu_devices(&text, &menu_ports(DosboxEngine::Staging, Some("GM"), None), "run.bat");
        assert!(gm.contains("\r\nCONFIG -set \"mididevice=mt32\"\r\n"), "{gm}");
        assert!(gm.contains("@REM exodium-midi: CONFIG -set \"mididevice=fluidsynth\"\r\n"));
        assert!(gm.contains("CONFIG -set \"midiconfig=GM\"\r\n"));
        let mt32 = redirect_menu_devices(&text, &menu_ports(DosboxEngine::Staging, None, Some("MT")), "run.bat");
        assert!(mt32.contains("\r\nCONFIG -set \"mididevice=fluidsynth\"\r\n"), "{mt32}");
        assert!(mt32.contains("@REM exodium-midi: CONFIG -set \"mididevice=mt32\"\r\n"));
        assert!(mt32.contains("CONFIG -set \"midiconfig=MT\"\r\n"));
        for out in [&gm, &mt32] {
            assert_eq!(out.matches("@REM exodium-midi end").count(), 1);
            assert!(out.contains("\r\nCONFIG -set \"mididevice=default\"\r\n"));
        }
    }

    #[test]
    fn standard_outputs_restore_exos_text_byte_for_byte() {
        let mut text = sq5_menu();
        text.push_str("if exist SC55.SEL CONFIG -set \"mididevice=fluidsynth\"\n\t@config -SET mididevice=MT32");
        let both = menu_ports(DosboxEngine::Staging, Some("GM"), Some("MT"));
        let patched = redirect_menu_devices(&text, &both, "run.bat");
        assert_eq!(patched.matches("@REM exodium-midi end").count(), 4, "{patched}");
        assert!(patched.ends_with("\t@config -SET \"mididevice=port\"\r\n@REM exodium-midi end"), "{patched}");
        let standard = redirect_menu_devices(&patched, &MenuPorts::default(), "run.bat");
        assert_eq!(standard, text);
        let bytes: Vec<u8> = standard.chars().map(|c| c as u8).collect();
        assert!(bytes.windows(3).any(|w| w == b"\xC9\xCD\xBB"));
    }

    #[test]
    fn patching_again_or_with_other_ports_starts_from_exos_text() {
        let text = sq5_menu();
        let a = menu_ports(DosboxEngine::Staging, Some("GM A"), Some("MT A"));
        let b = menu_ports(DosboxEngine::Staging, Some("GM B"), None);
        let once = redirect_menu_devices(&text, &a, "run.bat");
        assert_eq!(redirect_menu_devices(&once, &a, "run.bat"), once);
        let switched = redirect_menu_devices(&once, &b, "run.bat");
        assert_eq!(switched, redirect_menu_devices(&text, &b, "run.bat"));
        assert!(!switched.contains("MT A") && !switched.contains("GM A"));
    }

    #[test]
    fn redirections_and_unknown_shapes_stay_as_they_are() {
        let ports = menu_ports(DosboxEngine::Staging, Some("GM"), Some("MT"));
        for line in [
            "CONFIG -set \"mididevice=mt32\" > NUL\r\n",
            "if exist MT32.SEL CONFIG -set mididevice=mt32|more\r\n",
            "if errorlevel 1 CONFIG -set \"mididevice=mt32\"\r\n",
            "REM CONFIG -set \"mididevice=mt32\"\r\n",
            "echo CONFIG -set \"mididevice=fluidsynth\"\r\n",
            "CONFIG -set \"midi mididevice=mt32\"\r\n",
            "CONFIG -get \"mididevice=mt32\"\r\n",
            "CONFIG -set \"mididevice=mt32 fluidsynth\"\r\n",
        ] {
            assert_eq!(redirect_menu_devices(line, &ports, "run.bat"), line);
        }
    }

    #[test]
    fn a_port_name_the_shell_would_mangle_is_not_written() {
        for name in ["50% Synth", "Port \"A\"", "A|B", "Gr\u{e4}t"] {
            let ports = menu_ports(DosboxEngine::Staging, Some(name), Some(name));
            let line = "CONFIG -set \"mididevice=mt32\"\r\nCONFIG -set \"mididevice=fluidsynth\"\r\n";
            assert_eq!(redirect_menu_devices(line, &ports, "run.bat"), line, "{name}");
        }
    }

    #[test]
    fn an_unfinished_block_is_left_alone() {
        let text = "@REM exodium-midi: CONFIG -set \"mididevice=mt32\"\r\nCONFIG -set \"mididevice=port\"\r\n";
        assert_eq!(redirect_menu_devices(text, &MenuPorts::default(), "run.bat"), text);
    }

    #[test]
    fn ece_menus_reach_the_port_through_win32() {
        let ports = menu_ports(DosboxEngine::Ece, None, Some("Roland UM-ONE"));
        let out = redirect_menu_devices("CONFIG -set \"mididevice=mt32\"\r\n", &ports, "run.bat");
        assert!(out.contains("CONFIG -set \"mididevice=win32\"\r\n@REM exodium-midi end\r\n"), "{out}");
        assert!(out.contains("CONFIG -set \"midiconfig="), "{out}");
    }

    #[test]
    fn alsa_port_names_split_into_client_and_address() {
        let name = "Midi Through:Midi Through Port-0 14:0";
        assert_eq!(trailing_alsa_address(name), Some("14:0"));
        assert_eq!(strip_alsa_address(name), "Midi Through:Midi Through Port-0");
        assert_eq!(alsa_client_name(name), "Midi Through");
        assert_eq!(trailing_alsa_address("loopMIDI Port A"), None);
        assert_eq!(alsa_client_name("UM-ONE"), "UM-ONE");
    }

    #[test]
    fn scummvm_id_joins_driver_and_backend_name() {
        let id = scummvm_device_id("Midi Through:Midi Through Port-0 14:0");
        let expected = if cfg!(target_os = "macos") {
            "coremidi_Midi Through:Midi Through Port-0 14:0"
        } else if cfg!(target_os = "linux") {
            "alsa_Midi Through"
        } else {
            "windows_Midi Through:Midi Through Port-0 14:0"
        };
        assert_eq!(id, expected);
    }

    #[test]
    fn rom_dir_resolves_only_existing_staging_subfolders() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(resolve_roms(""), MidiRoms::default());
        assert_eq!(resolve_roms(tmp.path().join("missing").to_str().unwrap()), MidiRoms::default());
        std::fs::create_dir(tmp.path().join("mt32-roms")).unwrap();
        std::fs::create_dir(tmp.path().join("soundfonts")).unwrap();
        std::fs::write(tmp.path().join("soundfonts/b.SF2"), b"").unwrap();
        std::fs::write(tmp.path().join("soundfonts/a.sf2"), b"").unwrap();
        std::fs::write(tmp.path().join("soundfonts/readme.txt"), b"").unwrap();
        let roms = resolve_roms(tmp.path().to_str().unwrap());
        assert_eq!(roms.mt32, Some(tmp.path().join("mt32-roms")));
        assert_eq!(roms.soundcanvas, None);
        assert_eq!(roms.soundfont, Some(tmp.path().join("soundfonts/a.sf2")));
    }
}
