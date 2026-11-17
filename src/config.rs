// SPDX-License-Identifier: MPL-2.0

//! The stored settings (§2.3): [`PanelSettings`] in cosmic-config, one key per field in
//! `~/.config/cosmic/<app id>/v1/`. Anything the applet only caches, such as the VRAM sizes of
//! D4, belongs in cosmic-config *state* under its own type, not here.

use cosmic::cosmic_config::{self, CosmicConfigEntry};

use crate::panel::PanelSettings;

/// Opens the applet's config and reads it. Without it, changes last only until a restart.
pub fn load(app_id: &str) -> (Option<cosmic_config::Config>, PanelSettings) {
    match cosmic_config::Config::new(app_id, PanelSettings::VERSION) {
        Ok(handler) => {
            let settings = read(&handler);
            (Some(handler), settings)
        }
        Err(why) => {
            eprintln!("can't open the applet's config: {why}");
            (None, PanelSettings::default())
        }
    }
}

/// The stored settings, with defaults for anything missing or unreadable, and the limits
/// clamped to what the settings page offers.
pub fn read(handler: &cosmic_config::Config) -> PanelSettings {
    PanelSettings::get_entry(handler)
        .unwrap_or_else(|(errors, settings)| {
            for why in errors {
                eprintln!("can't read the applet's config: {why}");
            }
            settings
        })
        .clamped()
}

/// Writes the fields of `new` that differ from `stored`, one key each, and updates `stored`.
/// Other instances of the applet (another panel) see the change through their config watch.
pub fn store(handler: &cosmic_config::Config, stored: &mut PanelSettings, new: &PanelSettings) {
    let results = [
        stored.set_layout(handler, new.layout),
        stored.set_top(handler, new.top),
        stored.set_bottom(handler, new.bottom),
        stored.set_network(handler, new.network),
        stored.set_warn_temp(handler, new.warn_temp),
        stored.set_warn_usage(handler, new.warn_usage),
    ];
    for why in results.into_iter().filter_map(Result::err) {
        eprintln!("can't store a setting: {why}");
    }
}

/// With `SYSMON_DEBUG`, logs the settings as `what`: loaded, changed or stored.
pub fn log(what: &str, settings: &PanelSettings) {
    if crate::probe_client::debug() {
        eprintln!("config: {what} {settings:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel::{Layout, RowKind};

    const APP_ID: &str = "io.github.example.sysmon-test";

    fn open(dir: &tempfile::TempDir) -> cosmic_config::Config {
        cosmic_config::Config::with_custom_path(APP_ID, PanelSettings::VERSION, dir.path().into())
            .unwrap()
    }

    #[test]
    fn nothing_stored_reads_as_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(&open(&dir)), PanelSettings::default());
    }

    #[test]
    fn settings_survive_reopening_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let handler = open(&dir);
        let mut stored = read(&handler);
        let changed = PanelSettings {
            layout: Layout::OneLine,
            top: RowKind::Gpu,
            bottom: RowKind::CpuTemp,
            network: false,
            warn_temp: 78,
            warn_usage: 95,
        };
        store(&handler, &mut stored, &changed);
        assert_eq!(stored, changed);

        // One file per key, in version 1's folder.
        let folder = dir.path().join("cosmic").join(APP_ID).join("v1");
        for key in [
            "layout",
            "top",
            "bottom",
            "network",
            "warn_temp",
            "warn_usage",
        ] {
            assert!(folder.join(key).is_file(), "{key}");
        }
        assert_eq!(
            std::fs::read_to_string(folder.join("layout")).unwrap(),
            "OneLine"
        );

        assert_eq!(read(&open(&dir)), changed);
    }

    #[test]
    fn only_changed_keys_are_written() {
        let dir = tempfile::tempdir().unwrap();
        let handler = open(&dir);
        let mut stored = read(&handler);
        store(
            &handler,
            &mut stored,
            &PanelSettings {
                warn_temp: 90,
                ..PanelSettings::default()
            },
        );
        let folder = dir.path().join("cosmic").join(APP_ID).join("v1");
        assert!(folder.join("warn_temp").is_file());
        assert!(!folder.join("layout").exists());
    }

    #[test]
    fn hand_written_keys_are_read_and_bad_ones_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("cosmic").join(APP_ID).join("v1");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("layout"), "Compact").unwrap();
        std::fs::write(folder.join("warn_temp"), "300").unwrap();
        std::fs::write(folder.join("warn_usage"), "30").unwrap();
        std::fs::write(folder.join("top"), "Fan").unwrap();
        let settings = read(&open(&dir));
        assert_eq!(settings.layout, Layout::Compact);
        // Not a `u8`: the default. Out of the page's range: clamped.
        assert_eq!(settings.warn_temp, 85);
        assert_eq!(settings.warn_usage, 50);
        assert_eq!(settings.top, RowKind::Cpu);
    }
}
