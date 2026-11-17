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
    pub root_fs: Option<FsUsage>,
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

/// One DRM card. Everything but `status` is `None` unless the card [may be
/// queried](RuntimeStatus::may_query) (D3).
#[derive(Debug, Clone, PartialEq)]
pub struct GpuSample {
    /// The DRM card, e.g. `card1`.
    pub card: String,
    /// The model, when the driver says it without waking the card (NVIDIA procfs).
    pub name: Option<String>,
    pub status: RuntimeStatus,
    /// Load in percent (amdgpu `gpu_busy_percent`).
    pub busy: Option<u8>,
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    pub celsius: Option<f32>,
}

/// Space on a filesystem, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FsUsage {
    pub used: u64,
    pub total: u64,
}

impl FsUsage {
    /// Space used, in percent of the total.
    #[must_use]
    pub fn percent(&self) -> Option<f32> {
        share(self.used, self.total)
    }
}

/// `part` as a percentage of `whole`.
fn share(part: u64, whole: u64) -> Option<f32> {
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
    pub disks: Vec<DeviceRate>,
    /// All of [`Self::disks`] together: `first` is read, `second` written.
    pub disk_total: Option<PairRate>,
    pub cpu_temp: Option<f32>,
    pub drives: Vec<DriveTemp>,
    pub gpus: Vec<GpuSample>,
    pub root_fs: Option<FsUsage>,
    pub history: Histories,
}

impl Stats {
    /// Memory in use, % of the total.
    #[must_use]
    pub fn memory_percent(&self) -> Option<f32> {
        self.mem.and_then(|m| share(m.mem_used(), m.mem_total))
    }

    /// The first GPU that may be queried and reports a load: the one `gpu` on the panel shows.
    #[must_use]
    pub fn first_awake_gpu(&self) -> Option<&GpuSample> {
        self.gpus
            .iter()
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
            let net = net_total.unwrap_or_default();
            #[expect(clippy::cast_possible_truncation, reason = "graph values")]
            {
                self.history.net_down.push(net.first as f32);
                self.history.net_up.push(net.second as f32);
            }
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
            disks,
            disk_total,
            cpu_temp: sample.cpu_temp,
            drives: sample.drives,
            gpus: sample.gpus,
            root_fs: sample.root_fs,
            history: self.history.clone(),
        }
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
            name: None,
            status,
            busy,
            vram_used: None,
            vram_total: None,
            celsius: None,
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
