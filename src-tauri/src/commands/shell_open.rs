//! Handing a path to the desktop's default application. Every child an
//! AppImage build spawns goes through `sanitize_appimage_env`, and on Linux
//! the launcher's exit code is read rather than thrown away (§20).

use std::path::Path;
use std::process::Command;

use tauri::AppHandle;

/// AppImage/linuxdeploy variables that point only into `$APPDIR`. Dropped for
/// children; unset, each falls back to its default.
#[cfg(target_os = "linux")]
pub(crate) const APPIMAGE_ONLY_VARS: &[&str] = &[
    // AppImage runtime / Tauri's AppRun
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "APPDIR",
    "APPIMAGE",
    "OWD",
    "ARGV0",
    "PYTHONHOME",
    // linuxdeploy-plugin-gtk.sh
    "GDK_BACKEND",
    "GTK_DATA_PREFIX",
    "GTK_THEME",
    "GTK_EXE_PREFIX",
    "GTK_PATH",
    "GTK_IM_MODULE_FILE",
    "GDK_PIXBUF_MODULE_FILE",
    "GIO_EXTRA_MODULES",
    "GSETTINGS_SCHEMA_DIR",
    // linuxdeploy-plugin-gstreamer.sh + AppRun
    "GST_REGISTRY_REUSE_PLUGIN_SCANNER",
    "GST_PLUGIN_SYSTEM_PATH",
    "GST_PLUGIN_SYSTEM_PATH_1_0",
    "GST_PLUGIN_PATH_1_0",
    "GST_PLUGIN_SCANNER_1_0",
    "GST_PTP_HELPER_1_0",
];

/// Variables the AppRun PREPENDS `$APPDIR` entries to, keeping the host value
/// behind them. Removing these outright would take the host's own entries with
/// them (`PATH` most obviously), so only the `$APPDIR` entries are stripped.
#[cfg(target_os = "linux")]
pub(crate) const APPIMAGE_PREFIXED_PATH_VARS: &[&str] = &[
    "PATH",
    "XDG_DATA_DIRS",
    "PERLLIB",
    "PYTHONPATH",
    "QT_PLUGIN_PATH",
];

/// Drop the `$APPDIR`-rooted entries from a colon-separated path list.
/// `None` means nothing but AppImage entries were left.
#[cfg(target_os = "linux")]
pub(crate) fn strip_appdir_entries(value: &str, appdir: &str) -> Option<String> {
    let kept: Vec<&str> = value
        .split(':')
        .filter(|e| !e.is_empty() && !Path::new(e).starts_with(appdir))
        .collect();
    if kept.is_empty() {
        None
    } else {
        Some(kept.join(":"))
    }
}

/// Strip the AppImage's environment from a child: with the bundled
/// `LD_LIBRARY_PATH` inherited, a host binary mixes bundled and host
/// libraries - an emulator hangs in teardown on window close. Gated on
/// `APPIMAGE`.
#[cfg(target_os = "linux")]
pub(crate) fn sanitize_appimage_env(cmd: &mut Command) {
    if std::env::var_os("APPIMAGE").is_none() {
        return;
    }
    let appdir = std::env::var("APPDIR").ok();
    for var in APPIMAGE_ONLY_VARS {
        cmd.env_remove(var);
    }
    if let Some(appdir) = appdir.as_deref().filter(|d| !d.is_empty()) {
        for var in APPIMAGE_PREFIXED_PATH_VARS {
            if let Ok(value) = std::env::var(var) {
                match strip_appdir_entries(&value, appdir) {
                    Some(kept) => cmd.env(var, kept),
                    None => cmd.env_remove(var),
                };
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn sanitize_appimage_env(_cmd: &mut Command) {}

/// Open `path` with the desktop's default application. On Linux the
/// launcher's exit code is read; elsewhere the opener plugin's answer is
/// the only signal there is.
pub(crate) async fn open_with_default_app(app: &AppHandle, path: &Path) -> Result<(), String> {
    log::info!("Opening {} with the system handler", path.display());
    let result = open_impl(app, path).await;
    if let Err(msg) = &result {
        log::warn!("Opening {} failed: {msg}", path.display());
    }
    result
}

#[cfg(target_os = "linux")]
async fn open_impl(_app: &AppHandle, path: &Path) -> Result<(), String> {
    let path = path.to_path_buf();
    tauri::async_runtime::spawn_blocking(move || linux::run_xdg_open(&path))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(not(target_os = "linux"))]
async fn open_impl(app: &AppHandle, path: &Path) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;
    use std::process::{Command, ExitStatus, Stdio};
    use std::time::{Duration, Instant};

    /// xdg-open may return once the handler is dispatched (KDE) or only when
    /// the viewer closes (the generic path), so an exit inside this window is
    /// the launch failing and anything longer counts as started.
    const EARLY_EXIT_WINDOW: Duration = Duration::from_secs(2);

    pub(super) fn run_xdg_open(path: &Path) -> Result<(), String> {
        let mut cmd = Command::new("xdg-open");
        cmd.arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        super::sanitize_appimage_env(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("xdg-open could not be started: {e}"))?;
        let started = Instant::now();
        while started.elapsed() < EARLY_EXIT_WINDOW {
            match child.try_wait() {
                Ok(Some(status)) => return exit_result(status),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(format!("xdg-open could not be waited on: {e}")),
            }
        }
        let shown = path.display().to_string();
        std::thread::spawn(move || {
            if let Ok(status) = child.wait() {
                log::info!("xdg-open for {shown} ended after {:?}: {status}", started.elapsed());
            }
        });
        Ok(())
    }

    /// The exit codes xdg-open(1) documents.
    pub(super) fn exit_result(status: ExitStatus) -> Result<(), String> {
        if status.success() {
            return Ok(());
        }
        let reason = match status.code() {
            Some(1) => "error in the command line",
            Some(2) => "the file does not exist",
            Some(3) => "no application is registered for this file type",
            Some(4) => "the registered application could not be started",
            _ => "it was terminated",
        };
        Err(format!("xdg-open failed ({status}): {reason}"))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::process::ExitStatus;

    // The AppImage's LD_LIBRARY_PATH is what makes an emulator hang on window
    // close, and PATH must survive the cleanup or nothing launches at all.
    #[test]
    fn appimage_cleanup_drops_the_library_overrides_but_keeps_path() {
        assert!(APPIMAGE_ONLY_VARS.contains(&"LD_LIBRARY_PATH"));
        assert!(APPIMAGE_ONLY_VARS.contains(&"GIO_EXTRA_MODULES"));
        assert!(APPIMAGE_ONLY_VARS.contains(&"GST_PLUGIN_SYSTEM_PATH_1_0"));
        assert!(!APPIMAGE_ONLY_VARS.contains(&"PATH"));
        assert!(APPIMAGE_PREFIXED_PATH_VARS.contains(&"PATH"));
    }

    #[test]
    fn strip_appdir_entries_keeps_the_host_half() {
        assert_eq!(
            strip_appdir_entries("/tmp/.mount_x/usr/share/:/usr/share:/usr/local/share", "/tmp/.mount_x"),
            Some("/usr/share:/usr/local/share".to_string())
        );
        // A path that merely starts with the same characters is not inside it.
        assert_eq!(
            strip_appdir_entries("/tmp/.mount_xy/usr/bin:/usr/bin", "/tmp/.mount_x"),
            Some("/tmp/.mount_xy/usr/bin:/usr/bin".to_string())
        );
        // Nothing but AppImage entries left → the child gets no variable at all.
        assert_eq!(strip_appdir_entries("/tmp/.mount_x/usr/bin:", "/tmp/.mount_x"), None);
    }

    #[test]
    fn xdg_open_exit_codes_are_named() {
        use std::os::unix::process::ExitStatusExt;
        assert!(linux::exit_result(ExitStatus::from_raw(0)).is_ok());
        let no_handler = linux::exit_result(ExitStatus::from_raw(3 << 8)).unwrap_err();
        assert!(no_handler.contains("no application is registered"), "{no_handler}");
        let killed = linux::exit_result(ExitStatus::from_raw(9)).unwrap_err();
        assert!(killed.contains("terminated"), "{killed}");
    }
}
