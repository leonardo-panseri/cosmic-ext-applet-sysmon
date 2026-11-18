// SPDX-License-Identifier: MPL-2.0

//! What the panel button shows (plan §2.1, `docs/panel-builder.md`), as plain data: its
//! settings, its items' formatted values and the active warnings. The sampler builds a
//! [`PanelSummary`] every tick and, while the popup is closed, sends it only when it moved enough
//! from the last one sent ([`PanelHysteresis`], D6); the UI only lays it out.

pub mod format;
mod hysteresis;
mod item;
mod limits;

pub use hysteresis::PanelHysteresis;
pub use item::{
    DiskId, DiskRef, GpuRef, Group, Item, NetRef, Row, Rows, Slot, ValueKind, choices, find_disk,
};
pub use limits::{Limit, LimitKind, NVME_STEP_FROM, NvmeLimit, Unit};

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};
use serde::{Deserialize, Serialize};

use crate::sampler::stats::{DriveTemp, Stats, share};

/// How the panel button lays out its rows (`docs/panel-builder.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layout {
    /// The rows one above the other, in one panel slot; side by side on a vertical panel when
    /// both fit. The old `Compact` layout, which showed only the top row, reads as this.
    #[serde(alias = "Compact")]
    Stacked,
    /// Both rows on one line, the top row first; one column on a vertical panel.
    OneLine,
}

impl Layout {
    /// Every layout, in the builder's order.
    pub const ALL: [Self; 2] = [Self::Stacked, Self::OneLine];
}

/// How many columns the popup's stats page takes (§2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PopupLayout {
    /// Two columns when one would need scrolling on this screen.
    Auto,
    OneColumn,
    /// CPU, memory and GPU beside network, disk and processes.
    TwoColumns,
}

impl PopupLayout {
    /// Every popup layout, in the settings page's order.
    pub const ALL: [Self; 3] = [Self::Auto, Self::OneColumn, Self::TwoColumns];
}

/// The process monitor launched by default: COSMIC's System Monitor, by desktop id (D5).
pub const DEFAULT_PROCESS_MONITOR: &str = "com.system76.CosmicMonitor";

/// The applet's settings (§2.3), stored with cosmic-config as one key per field in
/// `~/.config/cosmic/<app id>/v1/`. Changed on the popup's settings page. Before the `warn_*`
/// limits there were only `warn_temp` and `warn_usage`; [`crate::config`] reads them as the
/// defaults of the limits they were split into.
#[derive(Debug, Clone, PartialEq, Eq, CosmicConfigEntry)]
#[version = 1]
pub struct PanelSettings {
    pub layout: Layout,
    /// The items, in two rows. Before them there were the `top` and `bottom` rows and the
    /// `network` switch; [`crate::config`] reads those as the rows' default.
    pub rows: Rows,
    /// The CPU package temperature, °C.
    pub warn_cpu_temp: Limit,
    /// Any GPU's temperature, °C.
    pub warn_gpu_temp: Limit,
    /// Memory in use, %.
    pub warn_memory: Limit,
    /// Swap in use, %.
    pub warn_swap: Limit,
    /// Any filesystem's used space, %.
    pub warn_disk: Limit,
    /// Any GPU's VRAM in use, %.
    pub warn_vram: Limit,
    /// Any `NVMe` drive's temperature, °C.
    pub warn_nvme: NvmeLimit,
    /// What right click and the popup's **Process monitor** button open: a desktop id or a
    /// command line ([`crate::launch`]).
    pub process_monitor: String,
    /// The popup's columns. Not a panel button setting, but stored with them.
    pub popup_layout: PopupLayout,
}

impl Default for PanelSettings {
    fn default() -> Self {
        Self {
            layout: Layout::Stacked,
            rows: Rows::default(),
            warn_cpu_temp: Limit::on(85),
            warn_gpu_temp: Limit::on(85),
            warn_memory: Limit::on(90),
            warn_swap: Limit::off(50),
            warn_disk: Limit::on(90),
            warn_vram: Limit::off(90),
            warn_nvme: NvmeLimit::default(),
            process_monitor: DEFAULT_PROCESS_MONITOR.to_owned(),
            popup_layout: PopupLayout::Auto,
        }
    }
}

impl PanelSettings {
    /// These settings with the limits inside the ranges the settings page offers, for values
    /// written to the config by hand.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        for kind in LimitKind::ALL {
            if let Some(limit) = self.limit_mut(kind) {
                limit.at = kind.clamp(limit.at);
            }
        }
        self.warn_nvme.at = self.warn_nvme.at.map(|at| LimitKind::Nvme.clamp(at));
        self
    }

    /// The limit of `kind`; `None` for [`LimitKind::Nvme`], which is [`Self::warn_nvme`].
    #[must_use]
    pub fn limit(&self, kind: LimitKind) -> Option<Limit> {
        match kind {
            LimitKind::CpuTemp => Some(self.warn_cpu_temp),
            LimitKind::GpuTemp => Some(self.warn_gpu_temp),
            LimitKind::Memory => Some(self.warn_memory),
            LimitKind::Swap => Some(self.warn_swap),
            LimitKind::Disk => Some(self.warn_disk),
            LimitKind::Vram => Some(self.warn_vram),
            LimitKind::Nvme => None,
        }
    }

    /// The limit of `kind`, to change it; `None` for [`LimitKind::Nvme`].
    pub fn limit_mut(&mut self, kind: LimitKind) -> Option<&mut Limit> {
        match kind {
            LimitKind::CpuTemp => Some(&mut self.warn_cpu_temp),
            LimitKind::GpuTemp => Some(&mut self.warn_gpu_temp),
            LimitKind::Memory => Some(&mut self.warn_memory),
            LimitKind::Swap => Some(&mut self.warn_swap),
            LimitKind::Disk => Some(&mut self.warn_disk),
            LimitKind::Vram => Some(&mut self.warn_vram),
            LimitKind::Nvme => None,
        }
    }

    /// Whether the limit of `kind` is on.
    #[must_use]
    pub fn is_on(&self, kind: LimitKind) -> bool {
        self.limit(kind).map_or(self.warn_nvme.on, |limit| limit.on)
    }

    /// Whether `value`, °C or %, warns under the limit of `kind`. Never for
    /// [`LimitKind::Nvme`], whose limit depends on the drive: see [`Self::nvme_hot`].
    #[must_use]
    pub fn warns(&self, kind: LimitKind, value: f32) -> bool {
        self.limit(kind)
            .and_then(Limit::active)
            .is_some_and(|at| value >= f32::from(at))
    }

    /// The limit `drive` warns at, °C: the override, else the drive's own rated limit; `None`
    /// while the `NVMe` limit is off, or without either.
    #[must_use]
    pub fn nvme_limit(&self, drive: &DriveTemp) -> Option<f32> {
        if !self.warn_nvme.on {
            return None;
        }
        self.warn_nvme.at.map(f32::from).or(drive.limit)
    }

    /// Whether `drive` is at or above [its limit](Self::nvme_limit).
    #[must_use]
    pub fn nvme_hot(&self, drive: &DriveTemp) -> bool {
        self.nvme_limit(drive)
            .is_some_and(|limit| drive.celsius >= limit)
    }
}

/// A value at or above its limit (§2.1), with both rounded to whole units for the tooltip.
/// Rounded, so a value moving within a degree or a percent doesn't make a new summary (D6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// The CPU package, °C.
    CpuTemp { celsius: i32, limit: i32 },
    /// A GPU, by DRM card, with its model when known; °C.
    GpuTemp {
        card: String,
        name: Option<String>,
        celsius: i32,
        limit: i32,
    },
    /// Memory in use, %.
    Memory { percent: i32, limit: i32 },
    /// Swap in use, %.
    Swap { percent: i32, limit: i32 },
    /// A filesystem, by mount point; % full.
    Filesystem {
        mount: String,
        percent: i32,
        limit: i32,
    },
    /// A GPU's VRAM in use, by DRM card, with its model when known; %.
    Vram {
        card: String,
        name: Option<String>,
        percent: i32,
        limit: i32,
    },
    /// An `nvme` drive, by name; °C. `rated` when the limit is the drive's own rated limit,
    /// not the settings' override.
    Nvme {
        drive: String,
        celsius: i32,
        limit: i32,
        rated: bool,
    },
}

impl Warning {
    /// Whether `item`, showing `stats`, owns this warning, so it takes the warning's colour.
    /// An item owns the warnings of its own device, as the rows did in the reference: both CPU
    /// items the CPU temperature; every item of a GPU that card's temperature and VRAM; RAM
    /// memory and swap, Swap swap; a mount's usage that filesystem; an `NVMe` item its drive.
    #[must_use]
    pub fn owned_by(&self, item: &Item, stats: &Stats) -> bool {
        match self {
            Self::CpuTemp { .. } => matches!(item, Item::Cpu | Item::CpuTemp),
            Self::GpuTemp { card, .. } | Self::Vram { card, .. } => {
                item.card(stats).is_some_and(|g| g.card == *card)
            }
            Self::Memory { .. } => *item == Item::Ram,
            Self::Swap { .. } => matches!(item, Item::Ram | Item::Swap),
            Self::Filesystem { mount, .. } => {
                matches!(item, Item::DiskUsage(shown) if shown == mount)
            }
            Self::Nvme { drive, .. } => item.drive(stats) == Some(drive.as_str()),
        }
    }
}

/// `value` rounded to a whole number, as the panel and the tooltip show it.
fn whole(value: f32) -> i32 {
    // Temperatures and percentages, far inside i32.
    #[expect(clippy::cast_possible_truncation, reason = "see above")]
    let whole = value.round() as i32;
    whole
}

/// Every warning `stats` raises under `settings`' limits.
#[must_use]
pub fn warnings(stats: &Stats, settings: &PanelSettings) -> Vec<Warning> {
    // The value, if it warns under the limit of `kind`, with the limit; both whole.
    let over = |kind: LimitKind, value: Option<f32>| {
        let value = value.filter(|&v| settings.warns(kind, v))?;
        let limit = settings.limit(kind).map_or(0, |l| i32::from(l.at));
        Some((whole(value), limit))
    };
    let mut warnings = Vec::new();
    if let Some((celsius, limit)) = over(LimitKind::CpuTemp, stats.cpu_temp) {
        warnings.push(Warning::CpuTemp { celsius, limit });
    }
    warnings.extend(stats.gpus.iter().filter_map(|g| {
        let (celsius, limit) = over(LimitKind::GpuTemp, g.celsius)?;
        Some(Warning::GpuTemp {
            card: g.card.clone(),
            name: g.name.clone(),
            celsius,
            limit,
        })
    }));
    warnings.extend(stats.gpus.iter().filter_map(|g| {
        let used = g.vram_used.zip(g.vram_total);
        let (percent, limit) = over(LimitKind::Vram, used.and_then(|(u, t)| share(u, t)))?;
        Some(Warning::Vram {
            card: g.card.clone(),
            name: g.name.clone(),
            percent,
            limit,
        })
    }));
    if let Some((percent, limit)) = over(LimitKind::Memory, stats.memory_percent()) {
        warnings.push(Warning::Memory { percent, limit });
    }
    let swap = stats.mem.and_then(|m| share(m.swap_used(), m.swap_total));
    if let Some((percent, limit)) = over(LimitKind::Swap, swap) {
        warnings.push(Warning::Swap { percent, limit });
    }
    warnings.extend(stats.mounts.iter().filter_map(|m| {
        let (percent, limit) = over(LimitKind::Disk, m.used_percent())?;
        Some(Warning::Filesystem {
            mount: m.mount_point.clone(),
            percent,
            limit,
        })
    }));
    warnings.extend(stats.drives.iter().filter_map(|d| {
        let limit = settings.nvme_limit(d).filter(|&l| d.celsius >= l)?;
        Some(Warning::Nvme {
            drive: d.name.clone(),
            celsius: whole(d.celsius),
            limit: whole(limit),
            rated: settings.warn_nvme.at.is_none(),
        })
    }));
    warnings
}

/// One item as the panel button shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShownItem {
    pub item: Item,
    /// Its value as shown; `None` shows as a dash.
    pub value: Option<String>,
    /// It owns an active warning ([`Warning::owned_by`]), so it takes the warning colour.
    pub warns: bool,
    /// The DRM card a GPU item shows, which [`GpuRef::Auto`] picks anew each sample.
    pub card: Option<String>,
}

/// Everything the panel button shows, formatted, and nothing it doesn't, so two summaries
/// differ only when the button would look different (D6). Both layouts show the same items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelSummary {
    pub top: Vec<ShownItem>,
    pub bottom: Vec<ShownItem>,
    /// Some warning is owned by no item shown, so the whole button takes the warning colour.
    pub button_warns: bool,
    /// Every warning, shown or not: listed in the panel button's tooltip.
    pub warnings: Vec<Warning>,
}

impl PanelSummary {
    /// What the panel button shows of `stats` with `settings`.
    #[must_use]
    pub fn new(stats: &Stats, settings: &PanelSettings) -> Self {
        let warnings = warnings(stats, settings);
        let shown = |row: Row| -> Vec<ShownItem> {
            settings
                .rows
                .row(row)
                .iter()
                .map(|item| ShownItem {
                    item: item.clone(),
                    value: item.value(stats),
                    warns: warnings.iter().any(|w| w.owned_by(item, stats)),
                    card: item.card(stats).map(|g| g.card.clone()),
                })
                .collect()
        };
        let button_warns = warnings
            .iter()
            .any(|w| !settings.rows.items().any(|item| w.owned_by(item, stats)));
        Self {
            top: shown(Row::Top),
            bottom: shown(Row::Bottom),
            button_warns,
            warnings,
        }
    }

    /// The items of `row`, from the left.
    #[must_use]
    pub fn row(&self, row: Row) -> &[ShownItem] {
        match row {
            Row::Top => &self.top,
            Row::Bottom => &self.bottom,
        }
    }

    /// Every item, the top row first.
    #[cfg(test)]
    pub fn items(&self) -> impl Iterator<Item = &ShownItem> {
        self.top.iter().chain(&self.bottom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler::rates::PairRate;
    use crate::sampler::stats::{DiskStats, DriveTemp, GpuSample};
    use crate::sources::drm::RuntimeStatus;
    use crate::sources::meminfo::MemInfo;
    use sysmon_proto::Mount;

    /// A filesystem at `mount_point`, `used` % full (no reserved blocks).
    fn mount(mount_point: &str, used: u64) -> Mount {
        Mount {
            mount_point: mount_point.into(),
            device: "/dev/sda1".into(),
            fs_type: "ext4".into(),
            total: 100,
            used,
            available: 100 - used,
        }
    }

    fn memory(percent: u64) -> MemInfo {
        MemInfo {
            mem_total: 100,
            mem_available: 100 - percent,
            ..MemInfo::default()
        }
    }

    const AMD: &str = "0000:05:00.0";
    const NVIDIA: &str = "0000:01:00.0";

    fn amd_gpu(busy: u8, celsius: f32) -> GpuSample {
        GpuSample {
            card: "card1".into(),
            pci: Some(AMD.into()),
            status: RuntimeStatus::Active,
            busy: Some(busy),
            celsius: Some(celsius),
            ..GpuSample::default()
        }
    }

    fn nvidia_gpu(status: RuntimeStatus, busy: Option<u8>, celsius: Option<f32>) -> GpuSample {
        GpuSample {
            card: "card0".into(),
            pci: Some(NVIDIA.into()),
            nvidia: true,
            name: Some("RTX 4060".into()),
            status,
            busy,
            celsius,
            ..GpuSample::default()
        }
    }

    fn with_rows(top: Vec<Item>, bottom: Vec<Item>) -> PanelSettings {
        PanelSettings {
            rows: Rows::new(top, bottom),
            ..PanelSettings::default()
        }
    }

    fn values(summary: &PanelSummary, row: Row) -> Vec<Option<&str>> {
        summary
            .row(row)
            .iter()
            .map(|s| s.value.as_deref())
            .collect()
    }

    #[test]
    fn the_default_rows_show_cpu_and_download_over_ram_and_upload() {
        let stats = Stats {
            cpu: Some(12.4),
            mem: Some(memory(45)),
            net_total: Some(PairRate {
                first: 1229.0,
                second: 20.0,
            }),
            ..Stats::default()
        };
        let summary = PanelSummary::new(&stats, &PanelSettings::default());
        assert_eq!(values(&summary, Row::Top), [Some("12%"), Some("1.2K")]);
        assert_eq!(values(&summary, Row::Bottom), [Some("45%"), Some("20B")]);
        assert!(!summary.button_warns);
        assert_eq!(summary.warnings, []);
    }

    #[test]
    fn missing_values_are_none_and_empty_rows_show_nothing() {
        let settings = with_rows(vec![Item::Cpu, Item::root_usage()], vec![]);
        let summary = PanelSummary::new(&Stats::default(), &settings);
        assert_eq!(values(&summary, Row::Top), [None, None]);
        assert_eq!(summary.bottom, []);
    }

    #[test]
    fn warnings_start_at_the_limits() {
        let stats = Stats {
            cpu_temp: Some(85.0),
            gpus: vec![amd_gpu(10, 84.9)],
            mem: Some(memory(90)),
            mounts: vec![mount("/", 89), mount("/mnt/storage", 90)],
            drives: vec![
                DriveTemp {
                    name: "Fast SSD".into(),
                    celsius: 70.0,
                    limit: Some(70.0),
                },
                DriveTemp {
                    name: "No limit".into(),
                    celsius: 99.0,
                    limit: None,
                },
            ],
            ..Stats::default()
        };
        assert_eq!(
            warnings(&stats, &PanelSettings::default()),
            [
                Warning::CpuTemp {
                    celsius: 85,
                    limit: 85
                },
                Warning::Memory {
                    percent: 90,
                    limit: 90
                },
                Warning::Filesystem {
                    mount: "/mnt/storage".into(),
                    percent: 90,
                    limit: 90
                },
                Warning::Nvme {
                    drive: "Fast SSD".into(),
                    celsius: 70,
                    limit: 70,
                    rated: true,
                },
            ]
        );
    }

    #[test]
    fn a_gpu_warning_names_the_card_and_its_model() {
        let stats = Stats {
            gpus: vec![nvidia_gpu(RuntimeStatus::Active, Some(10), Some(91.4))],
            ..Stats::default()
        };
        assert_eq!(
            warnings(&stats, &PanelSettings::default()),
            [Warning::GpuTemp {
                card: "card0".into(),
                name: Some("RTX 4060".into()),
                celsius: 91,
                limit: 85
            }]
        );
    }

    #[test]
    fn the_limits_come_from_the_settings() {
        let stats = Stats {
            cpu_temp: Some(72.0),
            mem: Some(memory(60)),
            ..Stats::default()
        };
        assert_eq!(warnings(&stats, &PanelSettings::default()), Vec::new());
        let strict = PanelSettings {
            warn_cpu_temp: Limit::on(70),
            warn_memory: Limit::on(60),
            ..PanelSettings::default()
        };
        assert_eq!(
            warnings(&stats, &strict),
            [
                Warning::CpuTemp {
                    celsius: 72,
                    limit: 70
                },
                Warning::Memory {
                    percent: 60,
                    limit: 60
                },
            ]
        );
    }

    #[test]
    fn cpu_and_gpu_temperatures_have_their_own_limits() {
        let stats = Stats {
            cpu_temp: Some(72.0),
            gpus: vec![amd_gpu(10, 72.0)],
            ..Stats::default()
        };
        let settings = PanelSettings {
            warn_cpu_temp: Limit::on(70),
            warn_gpu_temp: Limit::on(75),
            ..PanelSettings::default()
        };
        assert_eq!(
            warnings(&stats, &settings),
            [Warning::CpuTemp {
                celsius: 72,
                limit: 70
            }]
        );
        let settings = PanelSettings {
            warn_cpu_temp: Limit { on: false, at: 70 },
            warn_gpu_temp: Limit::on(72),
            ..PanelSettings::default()
        };
        assert_eq!(
            warnings(&stats, &settings),
            [Warning::GpuTemp {
                card: "card1".into(),
                name: None,
                celsius: 72,
                limit: 72
            }]
        );
    }

    #[test]
    fn values_moving_within_a_degree_keep_the_same_summary() {
        let settings = with_rows(vec![Item::CpuTemp], vec![]);
        let at = |celsius| {
            PanelSummary::new(
                &Stats {
                    cpu_temp: Some(celsius),
                    ..Stats::default()
                },
                &settings,
            )
        };
        assert_eq!(at(90.1), at(90.4));
        assert_ne!(at(90.4), at(90.6));
    }

    /// Whether each item of `settings`' rows warns, top row first, and whether the whole button
    /// does.
    fn colours(stats: &Stats, settings: &PanelSettings) -> (Vec<bool>, bool) {
        let summary = PanelSummary::new(stats, settings);
        (
            summary.items().map(|i| i.warns).collect(),
            summary.button_warns,
        )
    }

    #[test]
    fn memory_colours_the_ram_item_or_else_the_whole_button() {
        let stats = Stats {
            mem: Some(memory(95)),
            ..Stats::default()
        };
        assert_eq!(
            colours(&stats, &PanelSettings::default()),
            (vec![false, false, true, false], false)
        );
        assert_eq!(
            colours(&stats, &with_rows(vec![Item::Cpu], vec![Item::Swap])),
            (vec![false, false], true)
        );
    }

    #[test]
    fn cpu_temperature_colours_both_cpu_items() {
        let stats = Stats {
            cpu_temp: Some(90.0),
            ..Stats::default()
        };
        // As in the reference's warning example: the CPU item turns red, not the whole button.
        assert_eq!(
            colours(
                &stats,
                &with_rows(vec![Item::Cpu, Item::CpuTemp], vec![Item::Ram])
            ),
            (vec![true, true, false], false)
        );
        // Neither CPU item on the panel button: the whole button.
        assert_eq!(
            colours(
                &stats,
                &with_rows(vec![Item::Ram], vec![Item::root_usage()])
            ),
            (vec![false, false], true)
        );
    }

    #[test]
    fn a_disk_usage_item_owns_only_its_own_filesystem() {
        let stats = Stats {
            mounts: vec![mount("/", 40), mount("/mnt/storage", 97)],
            ..Stats::default()
        };
        let storage = Item::DiskUsage("/mnt/storage".into());
        assert_eq!(
            colours(&stats, &with_rows(vec![Item::root_usage()], vec![])),
            (vec![false], true)
        );
        assert_eq!(
            colours(&stats, &with_rows(vec![Item::root_usage()], vec![storage])),
            (vec![false, true], false)
        );
    }

    #[test]
    fn every_item_of_a_gpu_owns_its_temperature_and_vram() {
        let settings = PanelSettings {
            warn_vram: Limit::on(90),
            ..with_rows(
                vec![Item::GpuUsage(GpuRef::Auto)],
                vec![Item::Vram(GpuRef::Pci(AMD.into()))],
            )
        };
        let hot_amd = Stats {
            gpus: vec![amd_gpu(20, 95.0)],
            ..Stats::default()
        };
        assert_eq!(colours(&hot_amd, &settings), (vec![true, true], false));
        // Auto shows the awake NVIDIA card; the AMD item still owns the iGPU's warning.
        let both = Stats {
            gpus: vec![
                amd_gpu(20, 95.0),
                nvidia_gpu(RuntimeStatus::Active, Some(50), Some(70.0)),
            ],
            ..Stats::default()
        };
        assert_eq!(colours(&both, &settings), (vec![false, true], false));
        let summary = PanelSummary::new(&both, &settings);
        assert_eq!(summary.top[0].card.as_deref(), Some("card0"));
        assert_eq!(summary.bottom[0].card.as_deref(), Some("card1"));
        // Only the NVIDIA card's temperature on the panel button: the whole button.
        let nvidia_only = with_rows(vec![Item::GpuTemp(GpuRef::Pci(NVIDIA.into()))], vec![]);
        assert_eq!(colours(&both, &nvidia_only), (vec![false], true));
    }

    #[test]
    fn a_suspended_nvidia_card_leaves_auto_to_the_igpu() {
        let stats = Stats {
            gpus: vec![
                nvidia_gpu(RuntimeStatus::Suspended, None, None),
                amd_gpu(7, 45.0),
            ],
            ..Stats::default()
        };
        let settings = with_rows(vec![Item::GpuUsage(GpuRef::Auto)], vec![]);
        let summary = PanelSummary::new(&stats, &settings);
        assert_eq!(values(&summary, Row::Top), [Some("7%")]);
        assert_eq!(summary.top[0].card.as_deref(), Some("card1"));
    }

    fn nvme_stats() -> Stats {
        let drive = DriveTemp {
            name: "Samsung SSD 980 PRO".into(),
            celsius: 74.0,
            limit: Some(70.0),
        };
        Stats {
            drives: vec![drive.clone()],
            disks: vec![DiskStats {
                name: "nvme0n1".into(),
                model: drive.name.clone(),
                rate: None,
                temp: Some(drive),
                mounts: Vec::new(),
            }],
            ..Stats::default()
        }
    }

    #[test]
    fn an_nvme_item_owns_its_drive_s_temperature() {
        let stats = nvme_stats();
        let drive = Item::NvmeTemp(DiskId {
            model: "Samsung SSD 980 PRO".into(),
            name: "nvme0n1".into(),
        });
        assert_eq!(
            colours(&stats, &with_rows(vec![Item::Cpu, drive], vec![])),
            (vec![false, true], false)
        );
        assert_eq!(
            colours(&stats, &PanelSettings::default()),
            (vec![false; 4], true)
        );
    }

    #[test]
    fn no_warnings_colour_nothing() {
        let stats = Stats {
            cpu_temp: Some(50.0),
            mem: Some(memory(40)),
            ..Stats::default()
        };
        assert_eq!(
            colours(&stats, &PanelSettings::default()),
            (vec![false; 4], false)
        );
    }

    #[test]
    fn hand_written_limits_are_clamped_to_the_settings_page_ranges() {
        let wild = PanelSettings {
            warn_cpu_temp: Limit::on(200),
            warn_memory: Limit::off(10),
            warn_swap: Limit::on(5),
            warn_nvme: NvmeLimit {
                on: true,
                at: Some(250),
            },
            ..PanelSettings::default()
        };
        let clamped = wild.clamped();
        assert_eq!(clamped.warn_cpu_temp, Limit::on(110));
        assert_eq!(clamped.warn_memory, Limit::off(50));
        assert_eq!(clamped.warn_swap, Limit::on(10));
        assert_eq!(clamped.warn_nvme.at, Some(100));
        assert_eq!(PanelSettings::default().clamped(), PanelSettings::default());
    }

    #[test]
    fn swap_and_vram_warn_only_once_turned_on() {
        let stats = Stats {
            mem: Some(MemInfo {
                mem_total: 100,
                mem_available: 50,
                swap_total: 100,
                swap_free: 40,
            }),
            gpus: vec![GpuSample {
                vram_used: Some(95),
                vram_total: Some(100),
                ..nvidia_gpu(RuntimeStatus::Active, Some(10), Some(50.0))
            }],
            ..Stats::default()
        };
        assert_eq!(warnings(&stats, &PanelSettings::default()), Vec::new());
        let on = PanelSettings {
            warn_swap: Limit::on(50),
            warn_vram: Limit::on(90),
            ..PanelSettings::default()
        };
        assert_eq!(
            warnings(&stats, &on),
            [
                Warning::Vram {
                    card: "card0".into(),
                    name: Some("RTX 4060".into()),
                    percent: 95,
                    limit: 90
                },
                Warning::Swap {
                    percent: 60,
                    limit: 50
                },
            ]
        );
    }

    #[test]
    fn no_swap_and_a_suspended_card_never_warn() {
        let stats = Stats {
            mem: Some(memory(10)),
            gpus: vec![GpuSample {
                vram_used: Some(0),
                vram_total: Some(8 << 30),
                ..nvidia_gpu(RuntimeStatus::Suspended, None, None)
            }],
            ..Stats::default()
        };
        let strict = PanelSettings {
            warn_swap: Limit::on(10),
            warn_vram: Limit::on(50),
            ..PanelSettings::default()
        };
        assert_eq!(warnings(&stats, &strict), Vec::new());
    }

    #[test]
    fn memory_and_disk_have_their_own_limits() {
        let stats = Stats {
            mem: Some(memory(80)),
            mounts: vec![mount("/", 80)],
            ..Stats::default()
        };
        let settings = PanelSettings {
            warn_memory: Limit::off(60),
            warn_disk: Limit::on(75),
            ..PanelSettings::default()
        };
        assert_eq!(
            warnings(&stats, &settings),
            [Warning::Filesystem {
                mount: "/".into(),
                percent: 80,
                limit: 75
            }]
        );
    }

    fn drive(name: &str, celsius: f32, rated: Option<f32>) -> DriveTemp {
        DriveTemp {
            name: name.into(),
            celsius,
            limit: rated,
        }
    }

    #[test]
    fn an_nvme_override_replaces_every_drive_s_rated_limit() {
        let stats = Stats {
            drives: vec![
                drive("Rated", 72.0, Some(80.0)),
                drive("Unrated", 66.0, None),
            ],
            ..Stats::default()
        };
        assert_eq!(warnings(&stats, &PanelSettings::default()), Vec::new());
        let lower = PanelSettings {
            warn_nvme: NvmeLimit {
                on: true,
                at: Some(65),
            },
            ..PanelSettings::default()
        };
        let nvme = |name: &str, celsius| Warning::Nvme {
            drive: name.into(),
            celsius,
            limit: 65,
            rated: false,
        };
        assert_eq!(
            warnings(&stats, &lower),
            [nvme("Rated", 72), nvme("Unrated", 66)]
        );
        let off = PanelSettings {
            warn_nvme: NvmeLimit {
                on: false,
                at: Some(65),
            },
            ..PanelSettings::default()
        };
        assert_eq!(warnings(&stats, &off), Vec::new());
        let rated_off = PanelSettings {
            warn_nvme: NvmeLimit {
                on: false,
                at: None,
            },
            ..PanelSettings::default()
        };
        let hot = Stats {
            drives: vec![drive("Rated", 85.0, Some(80.0))],
            ..Stats::default()
        };
        assert_eq!(warnings(&hot, &rated_off), Vec::new());
    }

    #[test]
    fn swap_colours_the_ram_and_swap_items() {
        let settings = PanelSettings {
            warn_swap: Limit::on(50),
            ..PanelSettings::default()
        };
        let stats = Stats {
            mem: Some(MemInfo {
                mem_total: 100,
                mem_available: 50,
                swap_total: 100,
                swap_free: 10,
            }),
            ..Stats::default()
        };
        assert_eq!(
            colours(&stats, &settings),
            (vec![false, false, true, false], false)
        );
        let swap_only = PanelSettings {
            rows: Rows::new(vec![Item::Swap], vec![]),
            ..settings
        };
        assert_eq!(colours(&stats, &swap_only), (vec![true], false));
    }
}
