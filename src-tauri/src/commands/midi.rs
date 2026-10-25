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
    let port_device = match engine {
        DosboxEngine::Staging => "port",
        DosboxEngine::Ece => "win32",
        DosboxEngine::DosboxX => dosbox_x_port_device(),
    };
    let port = |name: &str, midi: &mut Vec<String>| {
        midi.push(format!("mididevice = {port_device}"));
        midi.push(format!("midiconfig = {}", port_config(name)));
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
