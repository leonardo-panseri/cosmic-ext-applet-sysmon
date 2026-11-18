// SPDX-License-Identifier: MPL-2.0

//! The stored settings (§2.3): [`PanelSettings`] in cosmic-config, one key per field in
//! `~/.config/cosmic/<app id>/v1/`. What the applet only caches, the VRAM sizes of D4, is
//! cosmic-config *state* instead ([`VramCache`]).

use std::collections::BTreeMap;

use cosmic::cosmic_config::{self, ConfigGet, ConfigSet, CosmicConfigEntry};

use serde::Deserialize;

use crate::panel::{GpuRef, Item, Limit, LimitKind, NetRef, PanelSettings, Rows};
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
    let settings = PanelSettings::get_entry(handler).unwrap_or_else(|(errors, settings)| {
        for why in errors {
            eprintln!("can't read the applet's config: {why}");
        }
        settings
    });
    settle(handler, settings)
}

/// The limits that replaced each old single limit: `warn_temp` for the CPU and GPU temperatures,
/// `warn_usage` for memory and disk. Nothing else changed, so the config's version didn't.
const SPLIT_LIMITS: [(&str, [LimitKind; 2]); 2] = [
    ("warn_temp", [LimitKind::CpuTemp, LimitKind::GpuTemp]),
    ("warn_usage", [LimitKind::Memory, LimitKind::Disk]),
];

/// What a panel row showed before the rows held items: the old `top` and `bottom` keys.
#[derive(Debug, Clone, Copy, Deserialize)]
enum OldRow {
    Cpu,
    Memory,
    Gpu,
    Disk,
    CpuTemp,
}

impl OldRow {
    fn item(self) -> Item {
        match self {
            Self::Cpu => Item::Cpu,
            Self::Memory => Item::Ram,
            Self::Gpu => Item::GpuUsage(GpuRef::Auto),
            Self::Disk => Item::root_usage(),
            Self::CpuTemp => Item::CpuTemp,
        }
    }
}

/// The rows the old keys stand for: the `top` row and the download rate if `network` was on,
/// over the `bottom` row and the upload rate. Each key not stored, or unreadable, has its old
/// default (CPU, memory, on), so with none stored these are the default rows.
fn old_rows(handler: &cosmic_config::Config) -> Rows {
    let row = |key: &str, default: OldRow| handler.get_local::<OldRow>(key).unwrap_or(default);
    let network = handler.get_local::<bool>("network").unwrap_or(true);
    let mut top = vec![row("top", OldRow::Cpu).item()];
    let mut bottom = vec![row("bottom", OldRow::Memory).item()];
    if network {
        top.push(Item::Download(NetRef::All));
        bottom.push(Item::Upload(NetRef::All));
    }
    Rows::new(top, bottom)
}

/// `settings` as read from `handler`, by [`read`] or by the config watch, which knows only the
/// current keys: until `rows` is stored, the rows are the old `top`, `bottom` and `network` keys'
/// ([`old_rows`]); each limit not stored yet takes its old single limit, if one is stored; and the
/// limits are clamped to what the settings page offers. The old keys stay, unread once the keys
/// that replaced them are stored. (The old `Compact` layout reads as `Stacked`.)
pub fn settle(handler: &cosmic_config::Config, mut settings: PanelSettings) -> PanelSettings {
    if matches!(
        handler.get_local::<Rows>("rows"),
        Err(cosmic_config::Error::NotFound)
    ) {
        settings.rows = old_rows(handler);
    }
    for (old_key, kinds) in SPLIT_LIMITS {
        let Ok(old) = handler.get_local::<u8>(old_key) else {
            continue;
        };
        for kind in kinds {
            let stored = handler.get_local::<Limit>(key(kind));
            if matches!(stored, Err(cosmic_config::Error::NotFound))
                && let Some(limit) = settings.limit_mut(kind)
            {
                *limit = Limit::on(old);
            }
        }
    }
    settings.clamped()
}

/// The config key of the limit of `kind`.
fn key(kind: LimitKind) -> &'static str {
    match kind {
        LimitKind::CpuTemp => "warn_cpu_temp",
        LimitKind::GpuTemp => "warn_gpu_temp",
        LimitKind::Memory => "warn_memory",
        LimitKind::Swap => "warn_swap",
        LimitKind::Disk => "warn_disk",
        LimitKind::Vram => "warn_vram",
        LimitKind::Nvme => "warn_nvme",
    }
}

/// Writes the fields of `new` that differ from `stored`, one key each, and updates `stored`.
/// Other instances of the applet (another panel) see the change through their config watch.
pub fn store(handler: &cosmic_config::Config, stored: &mut PanelSettings, new: &PanelSettings) {
    let results = [
        stored.set_layout(handler, new.layout),
        stored.set_rows(handler, new.rows.clone()),
        stored.set_warn_cpu_temp(handler, new.warn_cpu_temp),
        stored.set_warn_gpu_temp(handler, new.warn_gpu_temp),
        stored.set_warn_memory(handler, new.warn_memory),
        stored.set_warn_swap(handler, new.warn_swap),
        stored.set_warn_disk(handler, new.warn_disk),
        stored.set_warn_vram(handler, new.warn_vram),
        stored.set_warn_nvme(handler, new.warn_nvme),
        stored.set_process_monitor(handler, new.process_monitor.clone()),
        stored.set_popup_layout(handler, new.popup_layout),
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
    use crate::panel::{DiskRef, Layout, Limit, NvmeLimit, PopupLayout};

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
            rows: Rows::new(
                vec![
                    Item::GpuTemp(GpuRef::Pci("0000:01:00.0".into())),
                    Item::DiskRead(DiskRef::All),
                ],
                vec![Item::DiskUsage("/mnt/storage".into())],
            ),
            warn_cpu_temp: Limit::on(78),
            warn_gpu_temp: Limit::off(80),
            warn_memory: Limit::on(95),
            warn_swap: Limit::on(30),
            warn_disk: Limit::on(85),
            warn_vram: Limit::on(97),
            warn_nvme: NvmeLimit {
                on: true,
                at: Some(70),
            },
            process_monitor: "kitty -e btop".into(),
            popup_layout: PopupLayout::TwoColumns,
        };
        store(&handler, &mut stored, &changed);
        assert_eq!(stored, changed);

        // One file per key, in version 1's folder.
        let folder = dir.path().join("cosmic").join(APP_ID).join("v1");
        for key in [
            "layout",
            "rows",
            "warn_cpu_temp",
            "warn_gpu_temp",
            "warn_memory",
            "warn_swap",
            "warn_disk",
            "warn_vram",
            "warn_nvme",
            "process_monitor",
            "popup_layout",
        ] {
            assert!(folder.join(key).is_file(), "{key}");
        }
        assert_eq!(
            std::fs::read_to_string(folder.join("layout")).unwrap(),
            "OneLine"
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("popup_layout")).unwrap(),
            "TwoColumns"
        );
        assert_eq!(
            std::fs::read_to_string(folder.join("rows")).unwrap(),
            "([\n    GpuTemp(Pci(\"0000:01:00.0\")),\n    DiskRead(All),\n], [\n    \
             DiskUsage(\"/mnt/storage\"),\n])"
        );

        assert_eq!(read(&open(&dir)), changed);
    }

    /// Writes the key `key` as `value`, by hand, into version 1's folder.
    fn write_key(dir: &tempfile::TempDir, key: &str, value: &str) {
        let folder = dir.path().join("cosmic").join(APP_ID).join("v1");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(key), value).unwrap();
    }

    #[test]
    fn the_old_single_limits_become_the_defaults_of_the_split_ones() {
        let dir = tempfile::tempdir().unwrap();
        write_key(&dir, "warn_temp", "78");
        write_key(&dir, "warn_usage", "95");
        let settings = read(&open(&dir));
        assert_eq!(settings.warn_cpu_temp, Limit::on(78));
        assert_eq!(settings.warn_gpu_temp, Limit::on(78));
        assert_eq!(settings.warn_memory, Limit::on(95));
        assert_eq!(settings.warn_disk, Limit::on(95));
        // The new limits keep their own defaults.
        let defaults = PanelSettings::default();
        assert_eq!(settings.warn_swap, defaults.warn_swap);
        assert_eq!(settings.warn_vram, defaults.warn_vram);
    }

    #[test]
    fn a_new_limit_once_stored_wins_over_the_old_one() {
        let dir = tempfile::tempdir().unwrap();
        write_key(&dir, "warn_temp", "78");
        let handler = open(&dir);
        let mut stored = read(&handler);
        let changed = PanelSettings {
            warn_cpu_temp: Limit::off(90),
            ..stored.clone()
        };
        store(&handler, &mut stored, &changed);
        let settings = read(&open(&dir));
        assert_eq!(settings.warn_cpu_temp, Limit::off(90));
        // Not stored yet: still from the old key.
        assert_eq!(settings.warn_gpu_temp, Limit::on(78));
    }

    #[test]
    fn a_change_seen_through_the_config_watch_keeps_the_old_limits() {
        let dir = tempfile::tempdir().unwrap();
        write_key(&dir, "warn_usage", "60");
        let handler = open(&dir);
        // What the watch reads: the derived entry, without the old keys.
        let watched = PanelSettings::get_entry(&handler).unwrap_or_else(|(_, s)| s);
        assert_eq!(watched.warn_memory, Limit::on(90));
        let settled = settle(&handler, watched);
        assert_eq!(settled.warn_memory, Limit::on(60));
        assert_eq!(settled.warn_disk, Limit::on(60));
    }

    #[test]
    fn bad_old_limits_are_ignored_and_wild_ones_clamped() {
        let dir = tempfile::tempdir().unwrap();
        write_key(&dir, "warn_temp", "\"hot\"");
        write_key(&dir, "warn_usage", "20");
        let settings = read(&open(&dir));
        assert_eq!(settings.warn_cpu_temp, Limit::on(85));
        assert_eq!(settings.warn_memory, Limit::on(50));
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
                warn_swap: Limit::on(40),
                ..PanelSettings::default()
            },
        );
        let folder = dir.path().join("cosmic").join(APP_ID).join("v1");
        assert_eq!(
            std::fs::read_to_string(folder.join("warn_swap")).unwrap(),
            "(\n    on: true,\n    at: 40,\n)"
        );
        assert!(!folder.join("layout").exists());
        assert!(!folder.join("warn_memory").exists());
    }

    /// The rows of the settings read with the old keys `keys` stored.
    fn migrated(keys: &[(&str, &str)]) -> PanelSettings {
        let dir = tempfile::tempdir().unwrap();
        for (key, value) in keys {
            write_key(&dir, key, value);
        }
        read(&open(&dir))
    }

    #[test]
    fn the_old_rows_become_items_with_the_rates_beside_them() {
        let settings = migrated(&[("top", "Gpu"), ("bottom", "Disk")]);
        assert_eq!(
            settings.rows,
            Rows::new(
                vec![Item::GpuUsage(GpuRef::Auto), Item::Download(NetRef::All)],
                vec![Item::root_usage(), Item::Upload(NetRef::All)],
            )
        );
        assert_eq!(settings.layout, Layout::Stacked);
    }

    #[test]
    fn the_old_rows_without_network_rates_hold_one_item_each() {
        let settings = migrated(&[
            ("layout", "OneLine"),
            ("top", "CpuTemp"),
            ("network", "false"),
        ]);
        assert_eq!(
            settings.rows,
            Rows::new(vec![Item::CpuTemp], vec![Item::Ram])
        );
        assert_eq!(settings.layout, Layout::OneLine);
    }

    #[test]
    fn compact_becomes_stacked_with_the_same_rows() {
        let settings = migrated(&[("layout", "Compact"), ("bottom", "Cpu")]);
        assert_eq!(settings.layout, Layout::Stacked);
        assert_eq!(
            settings.rows,
            Rows::new(
                vec![Item::Cpu, Item::Download(NetRef::All)],
                vec![Item::Cpu, Item::Upload(NetRef::All)],
            )
        );
    }

    #[test]
    fn an_unreadable_old_row_keeps_its_old_default() {
        let settings = migrated(&[("top", "Fan"), ("network", "false")]);
        assert_eq!(settings.rows, Rows::new(vec![Item::Cpu], vec![Item::Ram]));
    }

    #[test]
    fn stored_rows_win_over_the_old_keys() {
        let dir = tempfile::tempdir().unwrap();
        write_key(&dir, "top", "Gpu");
        write_key(&dir, "network", "false");
        let handler = open(&dir);
        let mut stored = read(&handler);
        assert_eq!(
            stored.rows,
            Rows::new(vec![Item::GpuUsage(GpuRef::Auto)], vec![Item::Ram])
        );
        let changed = PanelSettings {
            rows: Rows::new(vec![Item::Swap], vec![]),
            ..stored.clone()
        };
        store(&handler, &mut stored, &changed);
        assert_eq!(read(&open(&dir)).rows, Rows::new(vec![Item::Swap], vec![]));
        // Through the config watch too.
        let watched = PanelSettings::get_entry(&handler).unwrap_or_else(|(_, s)| s);
        assert_eq!(settle(&handler, watched).rows, changed.rows);
    }

    #[test]
    fn the_config_watch_reads_the_old_rows_until_rows_are_stored() {
        let dir = tempfile::tempdir().unwrap();
        write_key(&dir, "bottom", "CpuTemp");
        let handler = open(&dir);
        let watched = PanelSettings::get_entry(&handler).unwrap_or_else(|(_, s)| s);
        assert_eq!(watched.rows, Rows::default());
        assert_eq!(
            settle(&handler, watched)
                .rows
                .row(crate::panel::Row::Bottom),
            [Item::CpuTemp, Item::Upload(NetRef::All)]
        );
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
        std::fs::write(folder.join("layout"), "OneLine").unwrap();
        std::fs::write(folder.join("warn_cpu_temp"), "(on: true, at: 200)").unwrap();
        std::fs::write(folder.join("warn_memory"), "(on: false, at: 30)").unwrap();
        std::fs::write(folder.join("warn_vram"), "(on: true)").unwrap();
        std::fs::write(folder.join("warn_nvme"), "(on: true, at: Some(72))").unwrap();
        std::fs::write(folder.join("rows"), "([Cpu, Fan], [])").unwrap();
        std::fs::write(folder.join("popup_layout"), "Wide").unwrap();
        let settings = read(&open(&dir));
        assert_eq!(settings.layout, Layout::OneLine);
        // Out of the page's range: clamped. Not a `Limit`: the default.
        assert_eq!(settings.warn_cpu_temp, Limit::on(110));
        assert_eq!(settings.warn_memory, Limit::off(50));
        // A field missing: the default.
        assert_eq!(settings.warn_vram, PanelSettings::default().warn_vram);
        assert_eq!(settings.warn_nvme.at, Some(72));
        // Rows stored but unreadable: the default rows, not the old keys'.
        assert_eq!(settings.rows, Rows::default());
        assert_eq!(settings.popup_layout, PopupLayout::Auto);
    }
}
