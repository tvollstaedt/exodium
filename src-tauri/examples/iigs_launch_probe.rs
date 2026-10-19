//! Starts eXoAppleIIGS games with the command Play would run, outside the
//! app: a Windows check of eXo's own builds without driving the UI (§23).
//!
//! Run from src-tauri/:
//!   cargo run --example iigs_launch_probe -- <game root> <data dir> <util zip|-> <shortcode>[::<option>]
//!
//! Unpacks the util zip like the support-file watcher (skip with `-`),
//! writes the launch config, spawns the emulator and prints `PID <pid>`.

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, data, util, spec] = args.as_slice() else {
        eprintln!("usage: iigs_launch_probe <game root> <data dir> <util zip|-> <shortcode>[::<option>]");
        std::process::exit(2);
    };
    let root = PathBuf::from(root);
    if util != "-" {
        match exodium_lib::appleiigs_probe_extract_support(&PathBuf::from(util), &root) {
            Ok(n) => println!("EXTRACTED {n}"),
            Err(e) => {
                eprintln!("ERR extract: {e}");
                std::process::exit(1);
            }
        }
    }
    let (code, option) = match spec.split_once("::") {
        Some((c, o)) => (c, Some(o)),
        None => (spec.as_str(), None),
    };
    let mut cmd = match exodium_lib::appleiigs_probe_command(code, &root, data, option) {
        Ok(cmd) => cmd,
        Err(e) => {
            eprintln!("ERR {spec}: {e}");
            std::process::exit(1);
        }
    };
    println!("CMD {:?} {:?} cwd={:?}", cmd.get_program(), cmd.get_args().collect::<Vec<_>>(), cmd.get_current_dir());
    match cmd.spawn() {
        Ok(child) => println!("PID {}", child.id()),
        Err(e) => {
            eprintln!("ERR spawn: {e}");
            std::process::exit(1);
        }
    }
}
