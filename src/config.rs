// SPDX-License-Identifier: MPL-2.0

//! The stored settings (§2.3): [`PanelSettings`] in cosmic-config, one key per field in
//! `~/.config/cosmic/<app id>/v1/`. What the applet only caches, the VRAM sizes of D4, is
//! cosmic-config *state* instead ([`VramCache`]).

use std::collections::BTreeMap;

use cosmic::cosmic_config::{self, ConfigGet, ConfigSet, CosmicConfigEntry};

use crate::panel::PanelSettings;
use crate::sources::MAX_GPUS;

/// The version of the state's layout.
pub const STATE_VERSION: u64 = 1;
/// The state key of the VRAM sizes: a map from GPU UUID to bytes.
pub const VRAM_KEY: &str = "vram_total";
/// Most cards remembered (D8); a card seen later than that is shown without its size.
const MAX_REMEMBERED: usize = 2 * MAX_GPUS;

/// Each NVIDIA card's VRAM size, by UUID, as seen the first time it was awake, so a suspended
/// card can show it without being woken (D4). Written only when a size changes.
#[derive(Debug, Default)]
pub struct VramCache {
    /// `None` without a state directory, or in tests: kept in memory only.
    state: Option<cosmic_config::Config>,
    totals: BTreeMap<String, u64>,
}

impl VramCache {
    /// The applet's cache in cosmic-config state: `~/.local/state/cosmic/<app id>/v1/`, or in
    /// a Flatpak the app's own `$XDG_STATE_HOME` (`~/.var/app/<app id>/.local/state/cosmic/…`).
    /// `Config::new_state` would use the host's `~/.local/state` there, which the sandbox can't
    /// write without a permission (`xdg-state` isn't one), so it would land in a tmpfs. The
    /// cache is the applet's alone, so its private state directory is enough.
    pub fn open(app_id: &str) -> Self {
        let sandbox_state = std::env::var_os("FLATPAK_ID")
            .and(std::env::var_os("XDG_STATE_HOME"))
            .filter(|dir| !dir.is_empty());
        let state = match sandbox_state {
            Some(dir) => cosmic_config::Config::with_custom_path(app_id, STATE_VERSION, dir.into()),
            None => cosmic_config::Config::new_state(app_id, STATE_VERSION),
        };
        match state {
            Ok(state) => Self::with_state(state),
            Err(why) => {
                eprintln!("can't open the applet's state: {why}; VRAM sizes aren't remembered");
                Self::default()
            }
        }
    }

    /// The cache stored in `state`.
    pub fn with_state(state: cosmic_config::Config) -> Self {
        let totals = state
            .get::<BTreeMap<String, u64>>(VRAM_KEY)
            .unwrap_or_else(|why| {
                // `NotFound` until the first card is seen awake.
                if why.is_err() {
                    eprintln!("can't read the cached VRAM sizes: {why}");
                }
                BTreeMap::new()
            })
            .into_iter()
            .take(MAX_REMEMBERED)
            .collect();
        Self {
            state: Some(state),
            totals,
        }
    }

    /// The VRAM size of the card with `uuid`, bytes, if it was ever seen awake.
    #[must_use]
    pub fn get(&self, uuid: &str) -> Option<u64> {
        self.totals.get(uuid).copied()
    }

    /// Remembers that the card with `uuid` has `total` bytes of VRAM. Writes the state only when
    /// that is news.
    pub fn remember(&mut self, uuid: &str, total: u64) {
        if self.get(uuid) == Some(total)
            || (self.totals.len() >= MAX_REMEMBERED && !self.totals.contains_key(uuid))
        {
            return;
        }
        self.totals.insert(uuid.to_owned(), total);
        if crate::probe_client::debug() {
            eprintln!("config: remembered VRAM {uuid} = {total} bytes");
        }
        if let Some(state) = &self.state
            && let Err(why) = state.set(VRAM_KEY, &self.totals)
        {
            eprintln!("can't store the VRAM size: {why}");
        }
    }
}

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

    const UUID: &str = "GPU-6e9f6c15-581b-4f11-c568-db1698fe7502";

    #[test]
    fn vram_sizes_survive_a_restart_and_are_written_only_when_new() {
        let dir = tempfile::tempdir().unwrap();
        let state = || {
            cosmic_config::Config::with_custom_path(APP_ID, STATE_VERSION, dir.path().into())
                .unwrap()
        };
        let mut cache = VramCache::with_state(state());
        assert_eq!(cache.get(UUID), None);
        cache.remember(UUID, 8188 << 20);
        let file = dir
            .path()
            .join("cosmic")
            .join(APP_ID)
            .join("v1")
            .join(VRAM_KEY);
        assert!(file.is_file());
        // The same size again: no write. Any write would bring the file back.
        std::fs::remove_file(&file).unwrap();
        cache.remember(UUID, 8188 << 20);
        assert!(!file.exists());
        cache.remember(UUID, 4096 << 20);
        assert!(file.is_file());

        let reopened = VramCache::with_state(state());
        assert_eq!(reopened.get(UUID), Some(4096 << 20));
        assert_eq!(reopened.get("GPU-other"), None);
    }

    #[test]
    fn the_vram_cache_is_bounded() {
        let mut cache = VramCache::default();
        for i in 0..40 {
            cache.remember(&format!("GPU-{i}"), 1);
        }
        assert_eq!(cache.totals.len(), MAX_REMEMBERED);
        // Known cards still update.
        cache.remember("GPU-0", 2);
        assert_eq!(cache.get("GPU-0"), Some(2));
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
