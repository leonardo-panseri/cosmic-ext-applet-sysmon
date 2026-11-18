// SPDX-License-Identifier: MPL-2.0

//! From samples to stats: percentages and rates between consecutive samples, and history.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::history::History;
use super::rates::{PairCounters, PairRate};
use crate::sources::block::BlockTopology;
use crate::sources::diskstats::{DiskCounters, SECTOR_BYTES};
use crate::sources::drm::RuntimeStatus;
use crate::sources::loadavg::LoadAvg;
use crate::sources::meminfo::MemInfo;
use crate::sources::net_dev::InterfaceCounters;
use crate::sources::proc_stat::{self, ProcStat};
use sysmon_proto::{Mount, TopProcesses};

/// One tick's raw numbers, as read. Anything that couldn't be read is `None` or missing.
#[derive(Debug, Clone, Default)]
pub struct Sample {
    pub stat: Option<ProcStat>,
    pub mem: Option<MemInfo>,
    pub load: Option<LoadAvg>,
    pub uptime: Option<Duration>,
    /// Physical interfaces only (D7).
    pub net: Vec<InterfaceCounters>,
    /// Whole physical disks only (D7).
    pub disks: Vec<DiskCounters>,
    /// CPU package temperature, °C.
    pub cpu_temp: Option<f32>,
    pub drives: Vec<DriveTemp>,
    /// The disk each of [`Self::drives`] is, in the same order: its namespace's kernel name
    /// (`nvme0n1`), when found.
    pub drive_disks: Vec<Option<String>>,
    pub gpus: Vec<GpuSample>,
    /// The host's real mounts, from the probe (D2: every 30 s); empty without a probe.
    pub mounts: Vec<Mount>,
    /// The top processes, from the probe; only in the open tier.
    pub processes: Option<TopProcesses>,
    /// What discovery found about the disks, to group the above by physical disk.
    pub disk_layout: Arc<DiskLayout>,
}

/// What discovery knows of the physical disks, shared by every sample until the next discovery.
#[derive(Debug, Clone, Default)]
pub struct DiskLayout {
    /// Each physical disk's model, by kernel name (`nvme0n1`).
    pub models: HashMap<String, String>,
    /// Which disk each block device is on.
    pub topology: BlockTopology,
}

/// An `nvme` drive's temperature.
#[derive(Debug, Clone, PartialEq)]
pub struct DriveTemp {
    /// The model, or the controller name (`nvme0`) without one.
    pub name: String,
    pub celsius: f32,
    /// The drive's rated limit (hwmon `max`, else `crit`), °C.
    pub limit: Option<f32>,
    /// Filled in by the [`Tracker`]: the temperature's extremes since start.
    pub extremes: Option<MinMax>,
}

/// One DRM card. Its stats are `None` unless the card [may be queried](RuntimeStatus::may_query)
/// (D3), except an NVIDIA card's VRAM while suspended: 0 used of the cached total (D4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuSample {
    /// The DRM card, e.g. `card1`.
    pub card: String,
    /// Its PCI address, `0000:01:00.0`, from the name of its `device` link's target: for
    /// NVIDIA also the procfs bus id. `None` for a card not on PCI.
    pub pci: Option<String>,
    /// Driven by the NVIDIA driver; listed first (§2.2).
    pub nvidia: bool,
    /// The model: from NVIDIA procfs, which never wakes the card, or for AMD from sysfs and
    /// `pci.ids`, looked up once.
    pub name: Option<String>,
    /// NVIDIA's `GPU-…` id, from procfs; keys the VRAM cache (D4).
    pub uuid: Option<String>,
    pub status: RuntimeStatus,
    /// Load in percent (amdgpu `gpu_busy_percent`, NVML utilisation).
    pub busy: Option<u8>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub celsius: Option<f32>,
    /// Board power draw, W (NVML).
    pub watts: Option<f32>,
    /// Filled in by the [`Tracker`]: the extremes since start.
    pub extremes: GpuExtremes,
}

impl GpuSample {
    /// Whether it shows as suspended: not awake, and so not queried.
    #[must_use]
    pub fn suspended(&self) -> bool {
        !self.status.may_query()
    }
}

/// `part` as a percentage of `whole`; `None` when `whole` is 0.
#[must_use]
pub fn share(part: u64, whole: u64) -> Option<f32> {
    // Percentages need no more than f32 precision.
    #[expect(clippy::cast_precision_loss, reason = "see above")]
    #[expect(clippy::cast_possible_truncation, reason = "see above")]
    (whole > 0).then(|| (part as f64 / whole as f64 * 100.0) as f32)
}

/// The lowest and highest value seen since the applet started; kept in memory only.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinMax {
    pub min: f32,
    pub max: f32,
}

impl MinMax {
    /// A range of the one `value`.
    #[must_use]
    pub fn new(value: f32) -> Self {
        Self {
            min: value,
            max: value,
        }
    }

    /// Widens `range` to take in `value`, if there is one; a range starts with its first value.
    pub fn record(range: &mut Option<Self>, value: Option<f32>) {
        let Some(value) = value.filter(|v| v.is_finite()) else {
            return;
        };
        match range {
            Some(r) => {
                r.min = r.min.min(value);
                r.max = r.max.max(value);
            }
            None => *range = Some(Self::new(value)),
        }
    }
}

/// `value` bytes as an [`MinMax`] value; f32's 24 bits keep sizes to within a few KiB per GiB.
#[expect(clippy::cast_precision_loss, reason = "see above")]
fn bytes(value: u64) -> f32 {
    value as f32
}

/// The extremes the popup shows on hover, since the applet started.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Extremes {
    /// CPU total, %.
    pub cpu: Option<MinMax>,
    /// CPU package temperature, °C.
    pub cpu_temp: Option<MinMax>,
    /// Memory in use, bytes.
    pub memory: Option<MinMax>,
    /// Swap in use, bytes; none without any swap.
    pub swap: Option<MinMax>,
}

/// One GPU's extremes since the applet started, from the samples it was awake for.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GpuExtremes {
    /// Load, %.
    pub busy: Option<MinMax>,
    /// Temperature, °C.
    pub celsius: Option<MinMax>,
    /// VRAM in use, bytes.
    pub vram: Option<MinMax>,
}

impl GpuExtremes {
    /// Takes in `gpu`'s readings, unless it is suspended: its cached VRAM size (D4) is no reading.
    fn record(&mut self, gpu: &GpuSample) {
        if gpu.suspended() {
            return;
        }
        MinMax::record(&mut self.busy, gpu.busy.map(f32::from));
        MinMax::record(&mut self.celsius, gpu.celsius);
        MinMax::record(&mut self.vram, gpu.vram_used.map(bytes));
    }
}

/// A device's two rates, named.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceRate {
    pub name: String,
    /// Bytes per second: received and sent, or read and written. `None` on the first sample
    /// or after a counter reset.
    pub rate: Option<PairRate>,
}

/// A physical disk, as the popup's Disk section shows it: its own rates, temperature and
/// filesystems.
#[derive(Debug, Clone, PartialEq)]
pub struct DiskStats {
    /// The kernel name, `nvme0n1`.
    pub name: String,
    /// The model, else the kernel name.
    pub model: String,
    /// Bytes per second: `first` is read, `second` written.
    pub rate: Option<PairRate>,
    pub temp: Option<DriveTemp>,
    /// Its mounted filesystems, `/` first.
    pub mounts: Vec<Mount>,
}

/// The last [`super::history::HISTORY_LEN`] samples of what the popup graphs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Histories {
    /// CPU total, %.
    pub cpu: History,
    /// Memory in use and swap in use, % of their totals; swap is 0 without any.
    pub memory: History,
    pub swap: History,
    /// Each GPU's load, %, by card; 0 while suspended.
    pub gpus: Vec<(String, History)>,
    /// Network download and upload, bytes/s, all physical interfaces together.
    pub net_down: History,
    pub net_up: History,
    /// Each physical interface's download and upload, in [`Stats::net`] order.
    pub interfaces: Vec<InterfaceHistory>,
}

/// One network interface's history, bytes/s.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InterfaceHistory {
    pub name: String,
    pub down: History,
    pub up: History,
}

impl InterfaceHistory {
    /// Whether anything went through it in the last two minutes: the popup's sign that it is in
    /// use, since an interface that is down moves nothing.
    #[must_use]
    pub fn had_traffic(&self) -> bool {
        self.down.iter().chain(self.up.iter()).any(|v| v > 0.0)
    }
}

/// What one tick measured, ready to show.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    /// CPU total, %. `None` on the first sample.
    pub cpu: Option<f32>,
    /// Each online core, %, in `/proc/stat` order.
    pub cores: Vec<Option<f32>>,
    pub mem: Option<MemInfo>,
    pub load: Option<LoadAvg>,
    pub uptime: Option<Duration>,
    pub net: Vec<DeviceRate>,
    /// All of [`Self::net`] together: `first` is download, `second` upload.
    pub net_total: Option<PairRate>,
    /// All physical disks together, bytes/s: `first` is read, `second` written.
    pub disk_total: Option<PairRate>,
    pub cpu_temp: Option<f32>,
    pub drives: Vec<DriveTemp>,
    pub gpus: Vec<GpuSample>,
    /// Real mounts, `/` first when it is one (D7).
    pub mounts: Vec<Mount>,
    /// The physical disks, the one holding `/` first, then by name; a drive with a temperature
    /// but no counted disk last.
    pub disks: Vec<DiskStats>,
    /// The mounts of [`Self::mounts`] on no disk of [`Self::disks`].
    pub other_mounts: Vec<Mount>,
    /// `None` in the closed tier, or without a probe.
    pub processes: Option<TopProcesses>,
    pub history: Histories,
    /// The extremes since start; each GPU's are in [`GpuSample::extremes`].
    pub extremes: Extremes,
}

impl Stats {
    /// Memory in use, % of the total.
    #[must_use]
    pub fn memory_percent(&self) -> Option<f32> {
        self.mem.and_then(|m| share(m.mem_used(), m.mem_total))
    }

    /// The root filesystem's used space, %: what the panel's Disk row shows.
    #[must_use]
    pub fn root_fs_percent(&self) -> Option<f32> {
        self.mounts
            .iter()
            .find(|m| m.mount_point == "/")
            .and_then(Mount::used_percent)
    }

    /// The GPUs, NVIDIA cards first (§2.2), otherwise in card order.
    pub fn gpus_nvidia_first(&self) -> impl Iterator<Item = &GpuSample> {
        let nvidia = self.gpus.iter().filter(|g| g.nvidia);
        nvidia.chain(self.gpus.iter().filter(|g| !g.nvidia))
    }

    /// The first GPU, NVIDIA first, that may be queried and reports a load: the one `gpu` on the
    /// panel shows. A suspended NVIDIA card is skipped for the integrated GPU.
    #[must_use]
    pub fn first_awake_gpu(&self) -> Option<&GpuSample> {
        self.gpus_nvidia_first()
            .find(|g| g.status.may_query() && g.busy.is_some())
    }
}

/// Turns consecutive samples into [`Stats`], keeping what it needs from the previous sample.
#[derive(Debug, Default)]
pub struct Tracker {
    prev: Option<(Instant, Option<ProcStat>)>,
    net: PairCounters,
    disks: PairCounters,
    history: Histories,
    extremes: Extremes,
    /// Each GPU's extremes, by card. Grows when a card appears, so a tick allocates nothing.
    gpu_extremes: Vec<(String, GpuExtremes)>,
    /// Each NVMe drive's temperature extremes, by its disk, else its name.
    drive_extremes: Vec<(String, Option<MinMax>)>,
}

impl Tracker {
    /// Computes the stats of `sample`, taken at `at`, against the previous one, and appends
    /// to the history.
    pub fn ingest(&mut self, sample: Sample, at: Instant) -> Stats {
        let (elapsed, prev_stat) = match self.prev.take() {
            Some((prev_at, stat)) => (at.checked_duration_since(prev_at), stat),
            None => (None, None),
        };
        let (cpu, cores) = cpu_busy(prev_stat.as_ref(), sample.stat.as_ref());
        self.prev = Some((at, sample.stat));

        let net = device_rates(
            self.net.update(
                sample
                    .net
                    .iter()
                    .map(|i| (i.name.as_str(), i.rx_bytes, i.tx_bytes)),
                elapsed,
            ),
            1,
        );
        let disks = device_rates(
            self.disks.update(
                sample
                    .disks
                    .iter()
                    .map(|d| (d.name.as_str(), d.sectors_read, d.sectors_written)),
                elapsed,
            ),
            SECTOR_BYTES,
        );
        let net_total = total(&net);
        let disk_total = total(&disks);

        if let Some(cpu) = cpu {
            self.history.cpu.push(cpu);
        }
        if let Some(mem) = sample.mem {
            self.history
                .memory
                .push(share(mem.mem_used(), mem.mem_total).unwrap_or(0.0));
            self.history
                .swap
                .push(share(mem.swap_used(), mem.swap_total).unwrap_or(0.0));
        }
        if elapsed.is_some() {
            let total = net_total.unwrap_or_default();
            #[expect(clippy::cast_possible_truncation, reason = "graph values")]
            {
                self.history.net_down.push(total.first as f32);
                self.history.net_up.push(total.second as f32);
            }
            self.push_interface_history(&net);
        }
        self.push_gpu_history(&sample.gpus);
        let mut drives = sample.drives;
        self.record_drive_extremes(&mut drives, &sample.drive_disks);
        let (disk_stats, other_mounts) = group_disks(
            &disks,
            &drives,
            &sample.drive_disks,
            &sample.mounts,
            &sample.disk_layout,
        );
        let mut gpus = sample.gpus;
        self.record_extremes(cpu, sample.mem, sample.cpu_temp, &mut gpus);

        Stats {
            cpu,
            cores,
            mem: sample.mem,
            load: sample.load,
            uptime: sample.uptime,
            net,
            net_total,
            disk_total,
            cpu_temp: sample.cpu_temp,
            drives,
            gpus,
            mounts: sample.mounts,
            disks: disk_stats,
            other_mounts,
            processes: sample.processes,
            history: self.history.clone(),
            extremes: self.extremes,
        }
    }

    /// Widens the extremes with this sample's values, and copies each GPU's into `gpus`.
    fn record_extremes(
        &mut self,
        cpu: Option<f32>,
        mem: Option<MemInfo>,
        cpu_temp: Option<f32>,
        gpus: &mut [GpuSample],
    ) {
        MinMax::record(&mut self.extremes.cpu, cpu);
        MinMax::record(&mut self.extremes.cpu_temp, cpu_temp);
        MinMax::record(&mut self.extremes.memory, mem.map(|m| bytes(m.mem_used())));
        MinMax::record(
            &mut self.extremes.swap,
            mem.filter(|m| m.swap_total > 0)
                .map(|m| bytes(m.swap_used())),
        );
        for gpu in gpus {
            let i = if let Some(i) = self.gpu_extremes.iter().position(|(c, _)| *c == gpu.card) {
                i
            } else {
                self.gpu_extremes
                    .push((gpu.card.clone(), GpuExtremes::default()));
                self.gpu_extremes.len() - 1
            };
            let extremes = &mut self.gpu_extremes[i].1;
            extremes.record(gpu);
            gpu.extremes = *extremes;
        }
    }

    /// Widens each drive's temperature extremes, and copies them into `drives`. `drive_disks`
    /// says which disk each drive is.
    fn record_drive_extremes(&mut self, drives: &mut [DriveTemp], drive_disks: &[Option<String>]) {
        for (i, drive) in drives.iter_mut().enumerate() {
            let key = drive_disks
                .get(i)
                .and_then(Option::as_deref)
                .unwrap_or(&drive.name);
            let i = if let Some(i) = self.drive_extremes.iter().position(|(k, _)| k == key) {
                i
            } else {
                self.drive_extremes.push((key.to_owned(), None));
                self.drive_extremes.len() - 1
            };
            let extremes = &mut self.drive_extremes[i].1;
            MinMax::record(extremes, Some(drive.celsius));
            drive.extremes = *extremes;
        }
    }

    fn push_interface_history(&mut self, net: &[DeviceRate]) {
        let mut old: HashMap<_, _> = std::mem::take(&mut self.history.interfaces)
            .into_iter()
            .map(|h| (h.name.clone(), h))
            .collect();
        self.history.interfaces = net
            .iter()
            .map(|device| {
                let mut history = old
                    .remove(&device.name)
                    .unwrap_or_else(|| InterfaceHistory {
                        name: device.name.clone(),
                        ..InterfaceHistory::default()
                    });
                let rate = device.rate.unwrap_or_default();
                #[expect(clippy::cast_possible_truncation, reason = "graph values")]
                {
                    history.down.push(rate.first as f32);
                    history.up.push(rate.second as f32);
                }
                history
            })
            .collect();
    }

    fn push_gpu_history(&mut self, gpus: &[GpuSample]) {
        let mut old: HashMap<_, _> = std::mem::take(&mut self.history.gpus).into_iter().collect();
        self.history.gpus = gpus
            .iter()
            .map(|gpu| {
                let mut history = old.remove(&gpu.card).unwrap_or_default();
                history.push(gpu.busy.map_or(0.0, f32::from));
                (gpu.card.clone(), history)
            })
            .collect();
    }
}

/// The disks of `rates` (D7), each with its temperature from `drives` and its filesystems from
/// `mounts`, and the mounts on none of them. See [`Stats::disks`] for the order.
/// `drive_disks` says which disk each of `drives` is.
fn group_disks(
    rates: &[DeviceRate],
    drives: &[DriveTemp],
    drive_disks: &[Option<String>],
    mounts: &[Mount],
    layout: &DiskLayout,
) -> (Vec<DiskStats>, Vec<Mount>) {
    let drives = || {
        drives
            .iter()
            .zip(drive_disks.iter().chain(std::iter::repeat(&None)))
    };
    let mut disks: Vec<DiskStats> = rates
        .iter()
        .map(|d| DiskStats {
            name: d.name.clone(),
            model: layout
                .models
                .get(&d.name)
                .cloned()
                .unwrap_or_else(|| d.name.clone()),
            rate: d.rate,
            temp: drives()
                .find(|(_, disk)| disk.as_ref() == Some(&d.name))
                .map(|(t, _)| t.clone()),
            mounts: Vec::new(),
        })
        .collect();
    let mut other = Vec::new();
    for mount in mounts {
        let disk = layout.topology.whole_disk(&mount.device);
        match disks.iter_mut().find(|d| Some(d.name.as_str()) == disk) {
            Some(disk) => disk.mounts.push(mount.clone()),
            None => other.push(mount.clone()),
        }
    }
    let holds_root = |d: &DiskStats| d.mounts.iter().any(|m| m.mount_point == "/");
    disks.sort_by(|a, b| {
        holds_root(b)
            .cmp(&holds_root(a))
            .then_with(|| (a.name.len(), &a.name).cmp(&(b.name.len(), &b.name)))
    });
    disks.extend(
        drives()
            .filter(|(_, disk)| {
                !disk
                    .as_ref()
                    .is_some_and(|n| rates.iter().any(|d| d.name == *n))
            })
            .map(|(t, disk)| DiskStats {
                name: disk.clone().unwrap_or_default(),
                model: t.name.clone(),
                rate: None,
                temp: Some(t.clone()),
                mounts: Vec::new(),
            }),
    );
    (disks, other)
}

fn cpu_busy(prev: Option<&ProcStat>, cur: Option<&ProcStat>) -> (Option<f32>, Vec<Option<f32>>) {
    let (Some(prev), Some(cur)) = (prev, cur) else {
        let cores = cur.map_or(0, |c| c.cores.len());
        return (None, vec![None; cores]);
    };
    let total = proc_stat::busy_percent(&prev.total, &cur.total);
    let cores = if prev.cores.len() == cur.cores.len() {
        prev.cores
            .iter()
            .zip(&cur.cores)
            .map(|(p, c)| proc_stat::busy_percent(p, c))
            .collect()
    } else {
        // A core went on- or offline, so positions don't match.
        vec![None; cur.cores.len()]
    };
    (total, cores)
}

#[expect(clippy::cast_precision_loss, reason = "a unit size")]
fn device_rates(rates: Vec<(String, Option<PairRate>)>, unit: u64) -> Vec<DeviceRate> {
    rates
        .into_iter()
        .map(|(name, rate)| DeviceRate {
            name,
            rate: rate.map(|r| PairRate {
                first: r.first * unit as f64,
                second: r.second * unit as f64,
            }),
        })
        .collect()
}

/// The sum of the devices that have a rate; `None` when none has.
fn total(devices: &[DeviceRate]) -> Option<PairRate> {
    devices
        .iter()
        .filter_map(|d| d.rate)
        .reduce(|a, b| PairRate {
            first: a.first + b.first,
            second: a.second + b.second,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::block::BlockEntry;
    use crate::sources::proc_stat::CpuTimes;

    fn stat(busy: u64, idle: u64) -> ProcStat {
        let times = CpuTimes {
            user: busy,
            idle,
            ..CpuTimes::default()
        };
        ProcStat {
            total: times,
            cores: vec![times, times],
        }
    }

    fn iface(name: &str, rx: u64, tx: u64) -> InterfaceCounters {
        InterfaceCounters {
            name: name.into(),
            rx_bytes: rx,
            tx_bytes: tx,
        }
    }

    fn gpu(card: &str, status: RuntimeStatus, busy: Option<u8>) -> GpuSample {
        GpuSample {
            card: card.into(),
            status,
            busy,
            ..GpuSample::default()
        }
    }

    #[test]
    fn cpu_load_needs_two_samples() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        let first = tracker.ingest(
            Sample {
                stat: Some(stat(100, 900)),
                ..Sample::default()
            },
            t0,
        );
        assert_eq!(first.cpu, None);
        assert_eq!(first.cores, [None, None]);
        let second = tracker.ingest(
            Sample {
                stat: Some(stat(125, 975)),
                ..Sample::default()
            },
            t0 + Duration::from_secs(2),
        );
        assert_eq!(second.cpu, Some(25.0));
        assert_eq!(second.cores, [Some(25.0), Some(25.0)]);
        assert_eq!(second.history.cpu.iter().collect::<Vec<_>>(), [25.0]);
    }

    #[test]
    fn network_total_sums_the_interfaces_over_the_measured_time() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        tracker.ingest(
            Sample {
                net: vec![iface("eth0", 0, 0), iface("wlan0", 0, 0)],
                ..Sample::default()
            },
            t0,
        );
        let stats = tracker.ingest(
            Sample {
                net: vec![iface("eth0", 4000, 400), iface("wlan0", 1000, 100)],
                ..Sample::default()
            },
            t0 + Duration::from_millis(2500),
        );
        let total = stats.net_total.unwrap();
        assert_eq!((total.first, total.second), (2000.0, 200.0));
        assert_eq!(stats.history.net_down.latest(), Some(2000.0));
        assert_eq!(stats.history.net_up.latest(), Some(200.0));
        let interfaces: Vec<_> = stats
            .history
            .interfaces
            .iter()
            .map(|h| (h.name.as_str(), h.down.latest(), h.up.latest()))
            .collect();
        assert_eq!(
            interfaces,
            [
                ("eth0", Some(1600.0), Some(160.0)),
                ("wlan0", Some(400.0), Some(40.0))
            ]
        );
    }

    #[test]
    fn an_interface_without_traffic_is_seen_as_unused() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        tracker.ingest(
            Sample {
                net: vec![iface("eth0", 0, 0), iface("wlan0", 0, 0)],
                ..Sample::default()
            },
            t0,
        );
        let stats = tracker.ingest(
            Sample {
                net: vec![iface("eth0", 0, 0), iface("wlan0", 500, 0)],
                ..Sample::default()
            },
            t0 + Duration::from_secs(2),
        );
        let used: Vec<_> = stats
            .history
            .interfaces
            .iter()
            .map(|h| (h.name.as_str(), h.had_traffic()))
            .collect();
        assert_eq!(used, [("eth0", false), ("wlan0", true)]);
    }

    #[test]
    fn disk_rates_are_in_bytes() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        let disk = |read, written| DiskCounters {
            name: "nvme0n1".into(),
            sectors_read: read,
            sectors_written: written,
        };
        tracker.ingest(
            Sample {
                disks: vec![disk(0, 0)],
                ..Sample::default()
            },
            t0,
        );
        let stats = tracker.ingest(
            Sample {
                disks: vec![disk(4, 2)],
                ..Sample::default()
            },
            t0 + Duration::from_secs(1),
        );
        let total = stats.disk_total.unwrap();
        assert_eq!((total.first, total.second), (2048.0, 1024.0));
    }

    #[test]
    fn gpu_history_follows_each_card_and_is_zero_while_suspended() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        tracker.ingest(
            Sample {
                gpus: vec![
                    gpu("card0", RuntimeStatus::Suspended, None),
                    gpu("card1", RuntimeStatus::Active, Some(30)),
                ],
                ..Sample::default()
            },
            t0,
        );
        let stats = tracker.ingest(
            Sample {
                gpus: vec![
                    gpu("card0", RuntimeStatus::Suspended, None),
                    gpu("card1", RuntimeStatus::Active, Some(40)),
                ],
                ..Sample::default()
            },
            t0 + Duration::from_secs(2),
        );
        let histories: Vec<_> = stats
            .history
            .gpus
            .iter()
            .map(|(card, h)| (card.as_str(), h.iter().collect::<Vec<_>>()))
            .collect();
        assert_eq!(
            histories,
            [("card0", vec![0.0, 0.0]), ("card1", vec![30.0, 40.0])]
        );
        assert_eq!(stats.first_awake_gpu().unwrap().card, "card1");
    }

    #[test]
    fn the_panel_s_gpu_is_the_first_awake_one_nvidia_first() {
        let nvidia = |status, busy| GpuSample {
            nvidia: true,
            ..gpu("card1", status, busy)
        };
        let amd = gpu("card0", RuntimeStatus::Active, Some(12));
        let stats = |gpus| Stats {
            gpus,
            ..Stats::default()
        };
        let awake = stats(vec![amd.clone(), nvidia(RuntimeStatus::Active, Some(60))]);
        assert_eq!(awake.first_awake_gpu().unwrap().card, "card1");
        let order: Vec<_> = awake.gpus_nvidia_first().map(|g| g.card.as_str()).collect();
        assert_eq!(order, ["card1", "card0"]);
        // Suspended, or awake without stats (not asked yet): the iGPU.
        for status in [RuntimeStatus::Suspended, RuntimeStatus::Active] {
            let stats = stats(vec![amd.clone(), nvidia(status, None)]);
            assert_eq!(stats.first_awake_gpu().unwrap().card, "card0");
        }
    }

    fn memory(used: u64, swap_used: u64) -> Sample {
        Sample {
            mem: Some(MemInfo {
                mem_total: 1000,
                mem_available: 1000 - used,
                swap_total: 400,
                swap_free: 400 - swap_used,
            }),
            ..Sample::default()
        }
    }

    #[test]
    fn memory_and_swap_history_in_percent_from_the_first_sample() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        tracker.ingest(memory(250, 0), t0);
        let stats = tracker.ingest(memory(500, 100), t0 + Duration::from_secs(2));
        assert_eq!(
            stats.history.memory.iter().collect::<Vec<_>>(),
            [25.0, 50.0]
        );
        assert_eq!(stats.history.swap.iter().collect::<Vec<_>>(), [0.0, 25.0]);
        // No swap at all reads 0 %; no meminfo adds nothing.
        let mut no_swap = memory(100, 0);
        no_swap.mem.as_mut().unwrap().swap_total = 0;
        no_swap.mem.as_mut().unwrap().swap_free = 0;
        let mut tracker_without_swap = Tracker::default();
        let without = tracker_without_swap.ingest(no_swap.clone(), t0);
        assert_eq!(without.extremes.swap, None);
        let stats = tracker.ingest(no_swap, t0 + Duration::from_secs(4));
        assert_eq!(stats.history.swap.latest(), Some(0.0));
        let stats = tracker.ingest(Sample::default(), t0 + Duration::from_secs(6));
        assert_eq!(stats.history.memory.iter().len(), 3);
    }

    #[test]
    fn cpu_memory_and_temperature_keep_their_extremes_since_start() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        let at = |i: u32| t0 + Duration::from_secs(2) * i;
        let sample = |busy, idle, used, temp| Sample {
            stat: Some(stat(busy, idle)),
            cpu_temp: temp,
            ..memory(used, used / 4)
        };
        let first = tracker.ingest(sample(0, 0, 300, Some(50.0)), at(0));
        // No CPU load before two samples, so no range either.
        assert_eq!(first.extremes.cpu, None);
        assert_eq!(first.extremes.memory, Some(MinMax::new(300.0)));
        // 25 % busy, then 75 %, then 50 %: the range keeps 25–75.
        tracker.ingest(sample(25, 75, 600, Some(70.0)), at(1));
        tracker.ingest(sample(100, 100, 200, None), at(2));
        let stats = tracker.ingest(sample(150, 150, 400, Some(55.0)), at(3));
        assert_eq!(
            stats.extremes.cpu,
            Some(MinMax {
                min: 25.0,
                max: 75.0
            })
        );
        assert_eq!(
            stats.extremes.memory,
            Some(MinMax {
                min: 200.0,
                max: 600.0
            })
        );
        assert_eq!(
            stats.extremes.swap,
            Some(MinMax {
                min: 50.0,
                max: 150.0
            })
        );
        assert_eq!(
            stats.extremes.cpu_temp,
            Some(MinMax {
                min: 50.0,
                max: 70.0
            })
        );
    }

    #[test]
    fn each_gpu_keeps_its_extremes_and_none_while_suspended() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        let card = |status, busy: Option<u8>, celsius, used| GpuSample {
            celsius,
            vram_used: used,
            vram_total: Some(1000),
            ..gpu("card0", status, busy)
        };
        tracker.ingest(
            Sample {
                gpus: vec![card(RuntimeStatus::Active, Some(10), Some(45.0), Some(100))],
                ..Sample::default()
            },
            t0,
        );
        // Suspended: the cached `0 / 8G` (D4) isn't a reading.
        tracker.ingest(
            Sample {
                gpus: vec![card(RuntimeStatus::Suspended, None, None, Some(0))],
                ..Sample::default()
            },
            t0 + Duration::from_secs(2),
        );
        let stats = tracker.ingest(
            Sample {
                gpus: vec![card(RuntimeStatus::Active, Some(90), Some(60.0), Some(800))],
                ..Sample::default()
            },
            t0 + Duration::from_secs(4),
        );
        let gpu = &stats.gpus[0];
        let range = |min, max| Some(MinMax { min, max });
        assert_eq!(
            (gpu.extremes.busy, gpu.extremes.celsius, gpu.extremes.vram),
            (range(10.0, 90.0), range(45.0, 60.0), range(100.0, 800.0))
        );
    }

    #[test]
    fn each_drive_keeps_its_temperature_extremes() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        let drive = |name: &str, celsius| DriveTemp {
            name: name.into(),
            celsius,
            limit: None,
            extremes: None,
        };
        // Two drives of one model, told apart by their disks.
        let sample = |a, b| Sample {
            drives: vec![drive("Same model", a), drive("Same model", b)],
            drive_disks: vec![Some("nvme0n1".into()), Some("nvme1n1".into())],
            ..Sample::default()
        };
        tracker.ingest(sample(40.0, 30.0), t0);
        tracker.ingest(sample(55.0, 35.0), t0 + Duration::from_secs(2));
        let stats = tracker.ingest(sample(45.0, 33.0), t0 + Duration::from_secs(4));
        let range = |min, max| Some(MinMax { min, max });
        let extremes: Vec<_> = stats.drives.iter().map(|d| d.extremes).collect();
        assert_eq!(extremes, [range(40.0, 55.0), range(30.0, 35.0)]);
        let on_disks: Vec<_> = stats
            .disks
            .iter()
            .map(|d| d.temp.as_ref().and_then(|t| t.extremes))
            .collect();
        assert_eq!(on_disks, [range(40.0, 55.0), range(30.0, 35.0)]);
    }

    fn mount(mount_point: &str, device: &str) -> Mount {
        Mount {
            mount_point: mount_point.into(),
            device: device.into(),
            fs_type: "ext4".into(),
            total: 100,
            used: 40,
            available: 60,
        }
    }

    /// The reference laptop's two drives, the second with a LUKS partition, and a USB stick
    /// whose disk isn't counted.
    fn laptop_disks() -> Arc<DiskLayout> {
        let entry = |name: &str, parent: Option<&str>| BlockEntry {
            name: name.into(),
            parent: parent.map(Into::into),
            ..BlockEntry::default()
        };
        let topology = BlockTopology::from_entries([
            entry("nvme0n1", None),
            entry("nvme0n1p1", Some("nvme0n1")),
            entry("nvme0n1p2", Some("nvme0n1")),
            entry("nvme1n1", None),
            entry("nvme1n1p1", Some("nvme1n1")),
            entry("nvme1n1p2", Some("nvme1n1")),
            BlockEntry {
                name: "dm-0".into(),
                slaves: vec!["nvme1n1p2".into()],
                dm_name: Some("luks-1".into()),
                ..BlockEntry::default()
            },
            entry("sdz", None),
            entry("sdz1", Some("sdz")),
        ]);
        Arc::new(DiskLayout {
            models: [
                ("nvme0n1".into(), "Micron_2400".into()),
                ("nvme1n1".into(), "WD_BLACK SN850 1TB".into()),
            ]
            .into(),
            topology,
        })
    }

    #[test]
    fn disks_group_their_rates_temperature_and_mounts() {
        let mut tracker = Tracker::default();
        let t0 = Instant::now();
        let counters = |name: &str, read, written| DiskCounters {
            name: name.into(),
            sectors_read: read,
            sectors_written: written,
        };
        let temp = |name: &str| DriveTemp {
            name: name.into(),
            celsius: 40.0,
            limit: Some(80.0),
            extremes: None,
        };
        let sample = |read| Sample {
            // In /proc/diskstats order.
            disks: vec![counters("nvme1n1", read, 0), counters("nvme0n1", 0, read)],
            drives: vec![
                temp("WD_BLACK SN850 1TB"),
                temp("Micron_2400"),
                temp("Spare"),
            ],
            // A drive whose namespace isn't a counted disk still shows.
            drive_disks: vec![Some("nvme1n1".into()), Some("nvme0n1".into()), None],
            mounts: vec![
                mount("/", "/dev/nvme0n1p2"),
                mount("/boot", "/dev/nvme0n1p1"),
                mount("/data", "/dev/mapper/luks-1"),
                mount("/media/usb", "/dev/sdz1"),
                mount("/mnt/storage", "/dev/nvme1n1p1"),
            ],
            disk_layout: laptop_disks(),
            ..Sample::default()
        };
        tracker.ingest(sample(0), t0);
        let stats = tracker.ingest(sample(4), t0 + Duration::from_secs(1));

        let disks: Vec<_> = stats
            .disks
            .iter()
            .map(|d| {
                (
                    d.model.as_str(),
                    d.rate.map(|r| (r.first, r.second)),
                    d.temp.as_ref().map(|t| t.name.as_str()),
                    d.mounts
                        .iter()
                        .map(|m| m.mount_point.as_str())
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        assert_eq!(
            disks,
            [
                // The disk holding `/` first.
                (
                    "Micron_2400",
                    Some((0.0, 2048.0)),
                    Some("Micron_2400"),
                    vec!["/", "/boot"]
                ),
                (
                    "WD_BLACK SN850 1TB",
                    Some((2048.0, 0.0)),
                    Some("WD_BLACK SN850 1TB"),
                    vec!["/data", "/mnt/storage"]
                ),
                ("Spare", None, Some("Spare"), vec![]),
            ]
        );
        let other: Vec<_> = stats
            .other_mounts
            .iter()
            .map(|m| m.mount_point.as_str())
            .collect();
        assert_eq!(other, ["/media/usb"]);
    }

    #[test]
    fn a_disk_without_a_model_is_named_after_its_device() {
        let mut tracker = Tracker::default();
        let stats = tracker.ingest(
            Sample {
                disks: vec![DiskCounters {
                    name: "sda".into(),
                    ..DiskCounters::default()
                }],
                ..Sample::default()
            },
            Instant::now(),
        );
        assert_eq!(stats.disks[0].model, "sda");
        assert_eq!(stats.disks[0].rate, None);
    }

    #[test]
    fn memory_percent_is_used_over_total() {
        let stats = Stats {
            mem: Some(MemInfo {
                mem_total: 1000,
                mem_available: 250,
                ..MemInfo::default()
            }),
            ..Stats::default()
        };
        assert_eq!(stats.memory_percent(), Some(75.0));
    }
}
