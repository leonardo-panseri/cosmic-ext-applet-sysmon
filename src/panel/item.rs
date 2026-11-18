// SPDX-License-Identifier: MPL-2.0

//! The panel button's **items** and **rows** (`docs/panel-builder.md`): an [`Item`] is one value
//! with its icon, such as CPU usage or one GPU's temperature; [`Rows`] are the two rows of at
//! most [`MAX_ITEMS`] items each. An item names its target by something stable that needs no
//! query to resolve (a PCI address, a mount point, a disk's model and name, an interface name),
//! and finds it in each sample's [`Stats`]; a target that isn't there shows as missing.

use serde::{Deserialize, Serialize};

use super::format;
use crate::sampler::stats::{DiskStats, GpuSample, Stats, share};

/// The most items a row holds.
pub const MAX_ITEMS: usize = 4;

/// Which GPU a GPU item shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GpuRef {
    /// The first awake GPU, NVIDIA first: the old `gpu` row.
    Auto,
    /// The GPU at this PCI address, `0000:01:00.0`; for NVIDIA the procfs bus id, which is the
    /// same address.
    Pci(String),
}

/// A physical disk or drive: its model, and its kernel name (`nvme0n1`), which may change
/// between boots while the model doesn't.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiskId {
    pub model: String,
    pub name: String,
}

/// Which disks a disk rate shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiskRef {
    /// All physical disks together.
    All,
    Disk(DiskId),
}

/// Which interfaces a network rate shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NetRef {
    /// All physical interfaces together (D7).
    All,
    /// The interface of this name, `wlp4s0`.
    Interface(String),
}

/// One value on the panel button, with its icon. Stored in the config as, for example,
/// `GpuTemp(Pci("0000:01:00.0"))` or `DiskUsage("/mnt/storage")`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Item {
    /// CPU load, %.
    Cpu,
    /// CPU package temperature, °C.
    CpuTemp,
    /// Memory in use, %.
    Ram,
    /// Swap in use, %.
    Swap,
    /// A GPU's load, %.
    GpuUsage(GpuRef),
    /// A GPU's temperature, °C.
    GpuTemp(GpuRef),
    /// A GPU's VRAM in use, %.
    Vram(GpuRef),
    /// A filesystem's used space, %, by mount point.
    DiskUsage(String),
    /// Bytes read per second.
    DiskRead(DiskRef),
    /// Bytes written per second.
    DiskWrite(DiskRef),
    /// An `NVMe` drive's temperature, °C.
    NvmeTemp(DiskId),
    /// Bytes received per second.
    Download(NetRef),
    /// Bytes sent per second.
    Upload(NetRef),
}

/// What an item's value is, which says how it is written, how wide its cell is (D6) and how far
/// it must move before the closed tier redraws ([`super::hysteresis`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Percent,
    Celsius,
    Rate,
}

impl ValueKind {
    /// The widest text a value in this unit has, which sizes its cell (D6).
    #[must_use]
    pub fn widest(self) -> &'static str {
        match self {
            Self::Percent => format::WIDEST_PERCENT,
            Self::Celsius => format::WIDEST_CELSIUS,
            Self::Rate => format::WIDEST_RATE,
        }
    }
}

impl Item {
    /// The root filesystem's used space, the old `disk` row.
    #[must_use]
    pub fn root_usage() -> Self {
        Self::DiskUsage("/".into())
    }

    /// The unit of its value.
    #[must_use]
    pub fn value_kind(&self) -> ValueKind {
        match self {
            Self::Cpu | Self::Ram | Self::Swap | Self::GpuUsage(_) | Self::Vram(_) => {
                ValueKind::Percent
            }
            Self::DiskUsage(_) => ValueKind::Percent,
            Self::CpuTemp | Self::GpuTemp(_) | Self::NvmeTemp(_) => ValueKind::Celsius,
            Self::DiskRead(_) | Self::DiskWrite(_) | Self::Download(_) | Self::Upload(_) => {
                ValueKind::Rate
            }
        }
    }

    /// The GPU it shows, for a GPU item.
    #[must_use]
    pub fn gpu(&self) -> Option<&GpuRef> {
        match self {
            Self::GpuUsage(gpu) | Self::GpuTemp(gpu) | Self::Vram(gpu) => Some(gpu),
            _ => None,
        }
    }

    /// Whether `other` is the same kind of item, whatever its target.
    #[must_use]
    pub fn same_kind(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    /// The GPU card it shows in `stats`, for a GPU item whose GPU is there.
    #[must_use]
    pub fn card<'a>(&self, stats: &'a Stats) -> Option<&'a GpuSample> {
        match self.gpu()? {
            GpuRef::Auto => stats.first_awake_gpu(),
            GpuRef::Pci(pci) => stats.gpus.iter().find(|g| g.pci.as_deref() == Some(pci)),
        }
    }

    /// Its value in `stats`, as the panel shows it; `None` when the target is missing or has no
    /// reading, such as a suspended GPU's load.
    #[must_use]
    pub fn value(&self, stats: &Stats) -> Option<String> {
        let rate = |rate: Option<f64>| rate.map(format::rate);
        match self {
            Self::Cpu => stats.cpu.map(format::percent),
            Self::CpuTemp => stats.cpu_temp.map(format::celsius),
            Self::Ram => stats.memory_percent().map(format::percent),
            Self::Swap => stats
                .mem
                .and_then(|m| share(m.swap_used(), m.swap_total))
                .map(format::percent),
            Self::GpuUsage(_) => self
                .card(stats)
                .and_then(|g| g.busy)
                .map(|b| format::percent(f32::from(b))),
            Self::GpuTemp(_) => self
                .card(stats)
                .and_then(|g| g.celsius)
                .map(format::celsius),
            Self::Vram(_) => self
                .card(stats)
                .and_then(|g| share(g.vram_used?, g.vram_total?))
                .map(format::percent),
            Self::DiskUsage(mount) => stats
                .mounts
                .iter()
                .find(|m| m.mount_point == *mount)
                .and_then(sysmon_proto::Mount::used_percent)
                .map(format::percent),
            Self::DiskRead(disk) => rate(disk_rate(stats, disk).map(|r| r.first)),
            Self::DiskWrite(disk) => rate(disk_rate(stats, disk).map(|r| r.second)),
            Self::NvmeTemp(id) => find_disk(&stats.disks, id)
                .and_then(|d| d.temp.as_ref())
                .map(|t| format::celsius(t.celsius)),
            Self::Download(net) => rate(net_rate(stats, net).map(|r| r.first)),
            Self::Upload(net) => rate(net_rate(stats, net).map(|r| r.second)),
        }
    }

    /// The name of the `NVMe` drive it shows in `stats`, as its warnings name it, for an
    /// `NVMe` item whose drive is there.
    #[must_use]
    pub fn drive<'a>(&self, stats: &'a Stats) -> Option<&'a str> {
        match self {
            Self::NvmeTemp(id) => find_disk(&stats.disks, id)
                .and_then(|d| d.temp.as_ref())
                .map(|t| t.name.as_str()),
            _ => None,
        }
    }
}

fn disk_rate(stats: &Stats, disk: &DiskRef) -> Option<crate::sampler::rates::PairRate> {
    match disk {
        DiskRef::All => stats.disk_total,
        DiskRef::Disk(id) => find_disk(&stats.disks, id).and_then(|d| d.rate),
    }
}

fn net_rate(stats: &Stats, net: &NetRef) -> Option<crate::sampler::rates::PairRate> {
    match net {
        NetRef::All => stats.net_total,
        NetRef::Interface(name) => stats.net.iter().find(|i| i.name == *name)?.rate,
    }
}

/// The disk `id` names: the one with its model and name, else the one with its model under
/// another name (the kernel named the disks in another order this boot).
#[must_use]
pub fn find_disk<'a>(disks: &'a [DiskStats], id: &DiskId) -> Option<&'a DiskStats> {
    disks
        .iter()
        .find(|d| d.model == id.model && d.name == id.name)
        .or_else(|| {
            disks
                .iter()
                .find(|d| d.model == id.model && d.model != d.name)
        })
}

impl DiskId {
    /// The id of `disk`.
    #[must_use]
    pub fn of(disk: &DiskStats) -> Self {
        Self {
            model: disk.model.clone(),
            name: disk.name.clone(),
        }
    }
}

/// One of the two rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Row {
    Top,
    Bottom,
}

impl Row {
    /// The other row.
    #[must_use]
    pub fn other(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
        }
    }
}

/// Where an item is: its row and its place in it, from the left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Slot {
    pub row: Row,
    pub index: usize,
}

/// The panel button's two rows, each of at most [`MAX_ITEMS`] items, in order. Stored as a pair
/// of lists, `([Cpu, Download(All)], [Ram, Upload(All)])`; extra items in a hand-written config
/// are dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "(Vec<Item>, Vec<Item>)", into = "(Vec<Item>, Vec<Item>)")]
pub struct Rows {
    top: Vec<Item>,
    bottom: Vec<Item>,
}

impl Default for Rows {
    /// Today's look: CPU usage and the download rate over RAM and the upload rate.
    fn default() -> Self {
        Self::new(
            vec![Item::Cpu, Item::Download(NetRef::All)],
            vec![Item::Ram, Item::Upload(NetRef::All)],
        )
    }
}

impl From<(Vec<Item>, Vec<Item>)> for Rows {
    fn from((top, bottom): (Vec<Item>, Vec<Item>)) -> Self {
        Self::new(top, bottom)
    }
}

impl From<Rows> for (Vec<Item>, Vec<Item>) {
    fn from(rows: Rows) -> Self {
        (rows.top, rows.bottom)
    }
}

impl Rows {
    /// The rows `top` and `bottom`, each cut to [`MAX_ITEMS`].
    #[must_use]
    pub fn new(mut top: Vec<Item>, mut bottom: Vec<Item>) -> Self {
        top.truncate(MAX_ITEMS);
        bottom.truncate(MAX_ITEMS);
        Self { top, bottom }
    }

    /// The items of `row`, from the left.
    #[must_use]
    pub fn row(&self, row: Row) -> &[Item] {
        match row {
            Row::Top => &self.top,
            Row::Bottom => &self.bottom,
        }
    }

    fn row_mut(&mut self, row: Row) -> &mut Vec<Item> {
        match row {
            Row::Top => &mut self.top,
            Row::Bottom => &mut self.bottom,
        }
    }

    /// Every item, the top row first.
    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.top.iter().chain(&self.bottom)
    }

    /// The item at `slot`, if there is one.
    #[must_use]
    pub fn get(&self, slot: Slot) -> Option<&Item> {
        self.row(slot.row).get(slot.index)
    }

    /// Whether `row` has room for another item.
    #[must_use]
    pub fn has_room(&self, row: Row) -> bool {
        self.row(row).len() < MAX_ITEMS
    }

    /// Appends `item` to `row`, if it has room. Returns where it went.
    pub fn push(&mut self, row: Row, item: Item) -> Option<Slot> {
        if !self.has_room(row) {
            return None;
        }
        let items = self.row_mut(row);
        items.push(item);
        Some(Slot {
            row,
            index: items.len() - 1,
        })
    }

    /// Removes the item at `slot`, if there is one.
    pub fn remove(&mut self, slot: Slot) -> Option<Item> {
        let items = self.row_mut(slot.row);
        (slot.index < items.len()).then(|| items.remove(slot.index))
    }

    /// Replaces the item at `slot` with `item`, if there is one there.
    pub fn set(&mut self, slot: Slot, item: Item) {
        if let Some(old) = self.row_mut(slot.row).get_mut(slot.index) {
            *old = item;
        }
    }

    /// Swaps the item at `slot` with its neighbour on the left, or on the right if `right`.
    /// Returns where it went; `None` at the row's end, where it stays.
    pub fn shift(&mut self, slot: Slot, right: bool) -> Option<Slot> {
        let items = self.row_mut(slot.row);
        let to = if right {
            slot.index.checked_add(1).filter(|&to| to < items.len())?
        } else {
            slot.index.checked_sub(1)?
        };
        if slot.index >= items.len() {
            return None;
        }
        items.swap(slot.index, to);
        Some(Slot { index: to, ..slot })
    }

    /// Moves the item at `slot` to the end of the other row, if that has room. Returns where it
    /// went.
    pub fn move_to_other_row(&mut self, slot: Slot) -> Option<Slot> {
        let other = slot.row.other();
        if !self.has_room(other) {
            return None;
        }
        let item = self.remove(slot)?;
        self.push(other, item)
    }
}

/// The groups of the picker, in its order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Cpu,
    Memory,
    Gpu,
    Disk,
    Network,
}

impl Group {
    pub const ALL: [Self; 5] = [
        Self::Cpu,
        Self::Memory,
        Self::Gpu,
        Self::Disk,
        Self::Network,
    ];
}

impl Item {
    /// The picker's group of this item.
    #[must_use]
    pub fn group(&self) -> Group {
        match self {
            Self::Cpu | Self::CpuTemp => Group::Cpu,
            Self::Ram | Self::Swap => Group::Memory,
            Self::GpuUsage(_) | Self::GpuTemp(_) | Self::Vram(_) => Group::Gpu,
            Self::DiskUsage(_) | Self::DiskRead(_) | Self::DiskWrite(_) | Self::NvmeTemp(_) => {
                Group::Disk
            }
            Self::Download(_) | Self::Upload(_) => Group::Network,
        }
    }

    /// The same item for each target `stats` has: every GPU (with Auto first), mount, disk
    /// (with all first), drive or interface (with all first); for an item without a target,
    /// itself. The item's own target is kept, missing or not, in its place.
    #[must_use]
    pub fn targets(&self, stats: &Stats) -> Vec<Self> {
        let gpus = |make: fn(GpuRef) -> Self| {
            let mut items = vec![make(GpuRef::Auto)];
            items.extend(
                stats
                    .gpus_nvidia_first()
                    .filter_map(|g| g.pci.clone())
                    .map(|pci| make(GpuRef::Pci(pci))),
            );
            items
        };
        let disks = |make: fn(DiskRef) -> Self| {
            let mut items = vec![make(DiskRef::All)];
            items.extend(
                stats
                    .disks
                    .iter()
                    .filter(|d| d.rate.is_some())
                    .map(|d| make(DiskRef::Disk(DiskId::of(d)))),
            );
            items
        };
        let nets = |make: fn(NetRef) -> Self| {
            let mut items = vec![make(NetRef::All)];
            items.extend(
                stats
                    .net
                    .iter()
                    .map(|i| make(NetRef::Interface(i.name.clone()))),
            );
            items
        };
        let mut targets = match self {
            Self::Cpu | Self::CpuTemp | Self::Ram | Self::Swap => vec![self.clone()],
            Self::GpuUsage(_) => gpus(Self::GpuUsage),
            Self::GpuTemp(_) => gpus(Self::GpuTemp),
            Self::Vram(_) => gpus(Self::Vram),
            Self::DiskUsage(_) => stats
                .mounts
                .iter()
                .map(|m| Self::DiskUsage(m.mount_point.clone()))
                .collect(),
            Self::DiskRead(_) => disks(Self::DiskRead),
            Self::DiskWrite(_) => disks(Self::DiskWrite),
            Self::NvmeTemp(_) => stats
                .disks
                .iter()
                .filter(|d| d.temp.is_some())
                .map(|d| Self::NvmeTemp(DiskId::of(d)))
                .collect(),
            Self::Download(_) => nets(Self::Download),
            Self::Upload(_) => nets(Self::Upload),
        };
        if !targets.contains(self) {
            targets.push(self.clone());
        }
        targets
    }

    /// Whether it has a target to choose: a GPU, mount, disk, drive or interface.
    #[must_use]
    pub fn has_target(&self) -> bool {
        !matches!(self, Self::Cpu | Self::CpuTemp | Self::Ram | Self::Swap)
    }
}

/// Every item the picker offers for `stats`, one per target, in the picker's order: by
/// [`Group`], then by kind, then by target. A GPU's items are offered only with a GPU, and an
/// `NVMe` drive's only with a drive.
#[must_use]
pub fn choices(stats: &Stats) -> Vec<Item> {
    let kinds = [
        Item::Cpu,
        Item::CpuTemp,
        Item::Ram,
        Item::Swap,
        Item::GpuUsage(GpuRef::Auto),
        Item::GpuTemp(GpuRef::Auto),
        Item::Vram(GpuRef::Auto),
        Item::root_usage(),
        Item::DiskRead(DiskRef::All),
        Item::DiskWrite(DiskRef::All),
        Item::Download(NetRef::All),
        Item::Upload(NetRef::All),
    ];
    let mut items = Vec::new();
    for kind in kinds {
        if kind.gpu().is_some() && stats.gpus.is_empty() {
            continue;
        }
        if matches!(kind, Item::DiskUsage(_)) {
            // Only the mounts there are; `/` first, as the stats list it.
            items.extend(
                stats
                    .mounts
                    .iter()
                    .map(|m| Item::DiskUsage(m.mount_point.clone())),
            );
            continue;
        }
        items.extend(kind.targets(stats));
        if matches!(kind, Item::DiskWrite(_)) {
            items.extend(
                stats
                    .disks
                    .iter()
                    .filter(|d| d.temp.is_some())
                    .map(|d| Item::NvmeTemp(DiskId::of(d))),
            );
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler::rates::PairRate;
    use crate::sampler::stats::{DeviceRate, DriveTemp};
    use crate::sources::drm::RuntimeStatus;
    use crate::sources::meminfo::MemInfo;

    fn rate(first: f64, second: f64) -> PairRate {
        PairRate { first, second }
    }

    fn gpu(card: &str, pci: &str, nvidia: bool, busy: Option<u8>) -> GpuSample {
        GpuSample {
            card: card.into(),
            pci: Some(pci.into()),
            nvidia,
            status: if busy.is_some() {
                RuntimeStatus::Active
            } else {
                RuntimeStatus::Suspended
            },
            busy,
            celsius: busy.map(|_| 60.0),
            ..GpuSample::default()
        }
    }

    const NVIDIA: &str = "0000:01:00.0";
    const AMD: &str = "0000:05:00.0";

    fn disk(name: &str, model: &str, read: f64, celsius: Option<f32>) -> DiskStats {
        DiskStats {
            name: name.into(),
            model: model.into(),
            rate: Some(rate(read, 0.0)),
            temp: celsius.map(|celsius| DriveTemp {
                name: model.into(),
                celsius,
                limit: None,
            }),
            mounts: Vec::new(),
        }
    }

    fn stats() -> Stats {
        Stats {
            cpu: Some(12.4),
            cpu_temp: Some(54.0),
            mem: Some(MemInfo {
                mem_total: 100,
                mem_available: 63,
                swap_total: 100,
                swap_free: 92,
            }),
            gpus: vec![
                gpu("card0", NVIDIA, true, Some(40)),
                gpu("card1", AMD, false, Some(33)),
            ],
            mounts: vec![sysmon_proto::Mount {
                mount_point: "/mnt/storage".into(),
                device: "/dev/sdb1".into(),
                fs_type: "ext4".into(),
                total: 100,
                used: 92,
                available: 8,
            }],
            disks: vec![
                disk("nvme0n1", "Samsung SSD", 2048.0, Some(41.0)),
                disk("sda", "WD Blue", 0.0, None),
            ],
            disk_total: Some(rate(3.0 * 1024.0 * 1024.0, 512.0)),
            net: vec![
                DeviceRate {
                    name: "wlp4s0".into(),
                    rate: Some(rate(1229.0, 20.0)),
                },
                DeviceRate {
                    name: "enp5s0".into(),
                    rate: None,
                },
            ],
            net_total: Some(rate(340.0 * 1024.0, 1229.0)),
            ..Stats::default()
        }
    }

    fn value(item: impl std::borrow::Borrow<Item>) -> Option<String> {
        item.borrow().value(&stats())
    }

    #[test]
    fn items_without_a_target_show_their_value() {
        assert_eq!(value(Item::Cpu).as_deref(), Some("12%"));
        assert_eq!(value(Item::CpuTemp).as_deref(), Some("54°C"));
        assert_eq!(value(Item::Ram).as_deref(), Some("37%"));
        assert_eq!(value(Item::Swap).as_deref(), Some("8%"));
    }

    #[test]
    fn a_gpu_item_shows_the_auto_gpu_or_the_one_at_its_address() {
        let pci = |p: &str| GpuRef::Pci(p.into());
        assert_eq!(value(Item::GpuUsage(GpuRef::Auto)).as_deref(), Some("40%"));
        assert_eq!(value(Item::GpuUsage(pci(AMD))).as_deref(), Some("33%"));
        assert_eq!(value(Item::GpuTemp(pci(NVIDIA))).as_deref(), Some("60°C"));
        // Not there: missing.
        assert_eq!(value(Item::GpuUsage(pci("0000:09:00.0"))), None);
    }

    #[test]
    fn a_suspended_gpu_has_no_load_and_auto_takes_the_next() {
        let mut stats = stats();
        stats.gpus[0] = gpu("card0", NVIDIA, true, None);
        let pci = GpuRef::Pci(NVIDIA.into());
        assert_eq!(Item::GpuUsage(pci).value(&stats), None);
        assert_eq!(
            Item::GpuUsage(GpuRef::Auto).value(&stats).as_deref(),
            Some("33%")
        );
        assert_eq!(
            Item::GpuUsage(GpuRef::Auto)
                .card(&stats)
                .map(|g| g.card.as_str()),
            Some("card1")
        );
    }

    #[test]
    fn disk_items_show_their_mount_disk_or_drive() {
        let samsung = DiskId {
            model: "Samsung SSD".into(),
            name: "nvme0n1".into(),
        };
        assert_eq!(
            value(Item::DiskUsage("/mnt/storage".into())).as_deref(),
            Some("92%")
        );
        assert_eq!(value(Item::root_usage()), None);
        assert_eq!(value(Item::DiskRead(DiskRef::All)).as_deref(), Some("3.0M"));
        assert_eq!(
            value(Item::DiskWrite(DiskRef::All)).as_deref(),
            Some("512B")
        );
        assert_eq!(
            value(Item::DiskRead(DiskRef::Disk(samsung.clone()))).as_deref(),
            Some("2.0K")
        );
        assert_eq!(value(Item::NvmeTemp(samsung)).as_deref(), Some("41°C"));
    }

    #[test]
    fn a_disk_renamed_since_is_found_by_its_model() {
        let renamed = DiskId {
            model: "Samsung SSD".into(),
            name: "nvme1n1".into(),
        };
        assert_eq!(value(Item::NvmeTemp(renamed)).as_deref(), Some("41°C"));
        let gone = DiskId {
            model: "Crucial".into(),
            name: "nvme0n1".into(),
        };
        assert_eq!(value(Item::NvmeTemp(gone)), None);
    }

    #[test]
    fn network_items_show_all_interfaces_or_one() {
        assert_eq!(value(Item::Download(NetRef::All)).as_deref(), Some("340K"));
        assert_eq!(value(Item::Upload(NetRef::All)).as_deref(), Some("1.2K"));
        let wifi = NetRef::Interface("wlp4s0".into());
        assert_eq!(value(Item::Download(wifi)).as_deref(), Some("1.2K"));
        let first_sample = NetRef::Interface("enp5s0".into());
        assert_eq!(value(Item::Upload(first_sample)), None);
    }

    #[test]
    fn the_default_rows_are_today_s_look() {
        let rows = Rows::default();
        assert_eq!(rows.row(Row::Top), [Item::Cpu, Item::Download(NetRef::All)]);
        assert_eq!(
            rows.row(Row::Bottom),
            [Item::Ram, Item::Upload(NetRef::All)]
        );
    }

    #[test]
    fn rows_are_stored_as_a_pair_of_lists() {
        let rows = Rows::new(
            vec![
                Item::GpuTemp(GpuRef::Pci(NVIDIA.into())),
                Item::DiskUsage("/mnt/storage".into()),
            ],
            vec![Item::GpuTemp(GpuRef::Auto)],
        );
        let text = ron::to_string(&rows).unwrap();
        assert_eq!(
            text,
            r#"([GpuTemp(Pci("0000:01:00.0")),DiskUsage("/mnt/storage")],[GpuTemp(Auto)])"#
        );
        assert_eq!(ron::from_str::<Rows>(&text).unwrap(), rows);
    }

    #[test]
    fn a_row_holds_at_most_four_items() {
        let five = vec![Item::Cpu; 5];
        let rows: Rows =
            ron::from_str(&ron::to_string(&(five, Vec::<Item>::new())).unwrap()).unwrap();
        assert_eq!(rows.row(Row::Top).len(), MAX_ITEMS);
        let mut rows = rows;
        assert!(!rows.has_room(Row::Top));
        assert_eq!(rows.push(Row::Top, Item::Ram), None);
        assert_eq!(
            rows.push(Row::Bottom, Item::Ram),
            Some(Slot {
                row: Row::Bottom,
                index: 0
            })
        );
    }

    fn top(index: usize) -> Slot {
        Slot {
            row: Row::Top,
            index,
        }
    }

    #[test]
    fn an_item_moves_within_its_row_and_stops_at_the_ends() {
        let mut rows = Rows::new(vec![Item::Cpu, Item::CpuTemp, Item::Ram], vec![]);
        assert_eq!(rows.shift(top(0), true), Some(top(1)));
        assert_eq!(rows.row(Row::Top), [Item::CpuTemp, Item::Cpu, Item::Ram]);
        assert_eq!(rows.shift(top(1), false), Some(top(0)));
        assert_eq!(rows.shift(top(0), false), None);
        assert_eq!(rows.shift(top(2), true), None);
        assert_eq!(rows.shift(top(7), false), None);
        assert_eq!(rows.row(Row::Top), [Item::Cpu, Item::CpuTemp, Item::Ram]);
    }

    #[test]
    fn an_item_moves_to_the_end_of_the_other_row_if_it_has_room() {
        let mut rows = Rows::default();
        assert_eq!(
            rows.move_to_other_row(top(0)),
            Some(Slot {
                row: Row::Bottom,
                index: 2
            })
        );
        assert_eq!(rows.row(Row::Top), [Item::Download(NetRef::All)]);
        assert_eq!(
            rows.row(Row::Bottom),
            [Item::Ram, Item::Upload(NetRef::All), Item::Cpu]
        );
        rows.push(Row::Bottom, Item::Swap);
        assert_eq!(rows.move_to_other_row(top(0)), None);
        assert_eq!(rows.row(Row::Top).len(), 1);
    }

    #[test]
    fn items_are_removed_and_replaced_in_place() {
        let mut rows = Rows::default();
        rows.set(top(0), Item::CpuTemp);
        assert_eq!(rows.remove(top(1)), Some(Item::Download(NetRef::All)));
        assert_eq!(rows.remove(top(1)), None);
        assert_eq!(rows.row(Row::Top), [Item::CpuTemp]);
    }

    #[test]
    fn the_picker_offers_one_entry_per_target_in_group_order() {
        let samsung = DiskId {
            model: "Samsung SSD".into(),
            name: "nvme0n1".into(),
        };
        let wd = DiskId {
            model: "WD Blue".into(),
            name: "sda".into(),
        };
        let pci = |p: &str| GpuRef::Pci(p.into());
        let wifi = || NetRef::Interface("wlp4s0".into());
        let wired = || NetRef::Interface("enp5s0".into());
        assert_eq!(
            choices(&stats()),
            [
                Item::Cpu,
                Item::CpuTemp,
                Item::Ram,
                Item::Swap,
                Item::GpuUsage(GpuRef::Auto),
                Item::GpuUsage(pci(NVIDIA)),
                Item::GpuUsage(pci(AMD)),
                Item::GpuTemp(GpuRef::Auto),
                Item::GpuTemp(pci(NVIDIA)),
                Item::GpuTemp(pci(AMD)),
                Item::Vram(GpuRef::Auto),
                Item::Vram(pci(NVIDIA)),
                Item::Vram(pci(AMD)),
                Item::DiskUsage("/mnt/storage".into()),
                Item::DiskRead(DiskRef::All),
                Item::DiskRead(DiskRef::Disk(samsung.clone())),
                Item::DiskRead(DiskRef::Disk(wd.clone())),
                Item::DiskWrite(DiskRef::All),
                Item::DiskWrite(DiskRef::Disk(samsung.clone())),
                Item::DiskWrite(DiskRef::Disk(wd)),
                Item::NvmeTemp(samsung),
                Item::Download(NetRef::All),
                Item::Download(wifi()),
                Item::Download(wired()),
                Item::Upload(NetRef::All),
                Item::Upload(wifi()),
                Item::Upload(wired()),
            ]
        );
        let groups: Vec<Group> = choices(&stats()).iter().map(Item::group).collect();
        assert!(groups.is_sorted_by_key(|g| Group::ALL.iter().position(|a| a == g)));
    }

    #[test]
    fn without_a_gpu_the_picker_offers_no_gpu_items() {
        let stats = Stats {
            gpus: Vec::new(),
            ..stats()
        };
        assert!(choices(&stats).iter().all(|i| i.gpu().is_none()));
    }

    #[test]
    fn an_item_s_targets_keep_its_own_even_when_missing() {
        let gone = Item::DiskUsage("/mnt/usb".into());
        assert_eq!(
            gone.targets(&stats()),
            [Item::DiskUsage("/mnt/storage".into()), gone.clone()]
        );
        assert_eq!(Item::Cpu.targets(&stats()), [Item::Cpu]);
    }
}
