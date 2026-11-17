// SPDX-License-Identifier: MPL-2.0

//! From samples to stats: percentages and rates between consecutive samples, and history.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::history::History;
use super::rates::{PairCounters, PairRate};
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
    pub gpus: Vec<GpuSample>,
    /// The host's real mounts, from the probe (D2: every 30 s); empty without a probe.
    pub mounts: Vec<Mount>,
    /// The top processes, from the probe; only in the open tier.
    pub processes: Option<TopProcesses>,
}

/// An `nvme` drive's temperature.
#[derive(Debug, Clone, PartialEq)]
pub struct DriveTemp {
    /// The model, or the controller name (`nvme0`) without one.
    pub name: String,
    pub celsius: f32,
    /// The drive's rated limit (hwmon `max`, else `crit`), °C.
    pub limit: Option<f32>,
}

impl DriveTemp {
    /// Whether the drive is at or above its own rated limit; never without one.
    #[must_use]
    pub fn hot(&self) -> bool {
        self.limit.is_some_and(|limit| self.celsius >= limit)
    }
}

/// One DRM card. Its stats are `None` unless the card [may be queried](RuntimeStatus::may_query)
/// (D3), except an NVIDIA card's VRAM while suspended: 0 used of the cached total (D4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuSample {
    /// The DRM card, e.g. `card1`.
    pub card: String,
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

/// A device's two rates, named.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceRate {
    pub name: String,
    /// Bytes per second: received and sent, or read and written. `None` on the first sample
    /// or after a counter reset.
    pub rate: Option<PairRate>,
}

/// The last [`super::history::HISTORY_LEN`] samples of what the popup graphs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Histories {
    /// CPU total, %.
    pub cpu: History,
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
    /// `None` in the closed tier, or without a probe.
    pub processes: Option<TopProcesses>,
    pub history: Histories,
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
            drives: sample.drives,
            gpus: sample.gpus,
            mounts: sample.mounts,
            processes: sample.processes,
            history: self.history.clone(),
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
