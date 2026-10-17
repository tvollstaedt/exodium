//! Writes the GSplus config the launcher would use for each named eXoAppleIIGS
//! game, for the playtest sweep (§23): the boot screenshots then come from the
//! app's own config derivation, not a copy of it.
//!
//! Run from src-tauri/:
//!   cargo run --example iigs_sweep -- <game root> <out dir> <shortcode>[::<option>]...
//!
//! `<game root>` holds `eXo/eXoAppleIIGS/<shortcode>/` (unpacked games),
//! `eXo/eXoAppleIIGS/!appleiigs/` (AppleIIGS_configs.zip) and the ROMs under
//! `eXo/emulators/GSPlus/`. Prints `CFG <shortcode> <path>` per config.

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(root), Some(out)) = (args.next(), args.next()) else {
        eprintln!("usage: iigs_sweep <game root> <out dir> <shortcode>[::<option>]...");
        std::process::exit(2);
    };
    let root = PathBuf::from(root);
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out).expect("cannot create the out dir");
    let mut failed = 0;
    for spec in args {
        let (code, option) = match spec.split_once("::") {
            Some((c, o)) => (c.to_string(), Some(o.to_string())),
            None => (spec.clone(), None),
        };
        match exodium_lib::appleiigs_sweep_config(&code, &root, option.as_deref()) {
            Ok(text) => {
                let name = spec.replace("::", " -- ").replace('/', "_");
                let path = out.join(format!("{name}.cfg"));
                std::fs::write(&path, text).expect("cannot write the config");
                println!("CFG {spec}\t{}", path.display());
            }
            Err(e) => {
                eprintln!("ERR {spec}: {e}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        std::process::exit(1);
    }
}
