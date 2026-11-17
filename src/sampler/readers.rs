// SPDX-License-Identifier: MPL-2.0

//! The sampler's I/O: the procfs and sysfs files of the closed tier (D2), kept open and reread
//! with `pread` at offset 0, so a tick makes no `open()` (§3.1). Devices are discovered at start,
//! every [`REDISCOVER_EVERY`], and after a read error.
//!
//! **D3.** Of a DRM card, only `device/power/runtime_status` is read until it says the card
//! [may be queried](RuntimeStatus::may_query). Its driver, its files and any hwmon chip under its
//! device are looked at only then, and the chip and files only on ticks where it still may be.
//! Nothing here opens `/dev`.

use std::collections::HashSet;
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::stats::{DriveTemp, FsUsage, GpuSample, Sample};
use crate::sources::drm::{self, RuntimeStatus};
use crate::sources::hwmon::{self, ChipTemp};
use crate::sources::{
    MAX_DRIVES, MAX_GPUS, clip_name, discover, diskstats, loadavg, meminfo, net_dev, nvidia_procfs,
    nvme, proc_stat, uptime,
};

/// How often devices are looked for again: network interfaces, disks, GPUs, hwmon chips.
pub const REDISCOVER_EVERY: Duration = Duration::from_secs(60);
/// How often the root filesystem's space is checked (D2: mounts every 30 s).
const ROOT_FS_EVERY: Duration = Duration::from_secs(30);
/// How often `nvme` drive temperatures are reread. Each read is a SMART command to the drive, which
/// blocks the sampler (one more wakeup) and wakes the drive's controller from its idle power
/// state; a drive heats up over minutes, so a warning 30 s late is still in time.
const DRIVE_TEMP_EVERY: Duration = Duration::from_secs(30);
/// Largest file read, so a misbehaving file can't grow the buffer without bound (D8).
const MAX_FILE_BYTES: usize = 1 << 20;
/// Most hwmon chips and DRM entries looked at (D8).
const MAX_DIR_ENTRIES: usize = 256;

/// A file kept open and reread from the start.
#[derive(Debug)]
struct Pread {
    file: File,
    buf: Vec<u8>,
}

impl Pread {
    fn open(path: &Path) -> Option<Self> {
        let file = File::open(path).ok()?;
        Some(Self {
            file,
            buf: Vec::new(),
        })
    }

    /// The file's whole current contents.
    fn read(&mut self) -> io::Result<&str> {
        let mut len = 0;
        loop {
            if len == self.buf.len() {
                if len >= MAX_FILE_BYTES {
                    return Err(io::Error::other("file too large"));
                }
                self.buf.resize((len * 2).max(4096), 0);
            }
            let n = self.file.read_at(&mut self.buf[len..], len as u64)?;
            if n == 0 {
                break;
            }
            len += n;
        }
        std::str::from_utf8(&self.buf[..len]).map_err(io::Error::other)
    }

    /// The contents parsed by `parse`; `None`, and `failed` set, on a read error.
    fn parse<T>(&mut self, failed: &mut bool, parse: impl FnOnce(&str) -> Option<T>) -> Option<T> {
        if let Ok(text) = self.read() {
            parse(text)
        } else {
            *failed = true;
            None
        }
    }
}

/// Opens `path` if `slot` holds nothing yet.
fn reopen(slot: &mut Option<Pread>, path: &str) {
    if slot.is_none() {
        *slot = Pread::open(Path::new(path));
    }
}

/// A hwmon chip the applet shows: the files of its main sensor ([`hwmon::classify_sensor`]),
/// the fixed ones read once and `temp<index>_input` kept open, so a tick reads one file.
#[derive(Debug)]
struct Chip {
    fixed: Vec<(String, String)>,
    input: (String, Pread),
}

impl Chip {
    /// Reads the chip in `dir` if it is one [`hwmon::classify`] knows.
    fn open(dir: &Path) -> Option<Self> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).ok()?.take(MAX_DIR_ENTRIES).flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if hwmon::is_wanted_file(&name)
                && let Ok(text) = std::fs::read_to_string(entry.path())
            {
                files.push((name, text));
            }
        }
        let index = hwmon::classify_sensor(&hwmon::parse_chip(files.iter().cloned())?)?
            .0
            .index;
        let prefix = format!("temp{index}_");
        let input_name = format!("{prefix}input");
        files.retain(|(name, _)| {
            name == "name" || (name.starts_with(&prefix) && *name != input_name)
        });
        let input = Pread::open(&dir.join(&input_name))?;
        Some(Self {
            fixed: files,
            input: (input_name, input),
        })
    }

    fn read(&mut self) -> Option<ChipTemp> {
        let (name, file) = &mut self.input;
        let input = file.read().ok()?;
        let files = self
            .fixed
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .chain([(name.as_str(), input)]);
        hwmon::classify(&hwmon::parse_chip(files)?)
    }
}

/// An amdgpu card's files.
#[derive(Debug)]
struct AmdFiles {
    busy: Option<Pread>,
    vram_used: Option<Pread>,
    vram_total: Option<u64>,
}

/// A DRM card.
#[derive(Debug)]
struct Card {
    name: String,
    /// The canonical path of its device, to match hwmon chips to it.
    device: PathBuf,
    runtime_status: Option<Pread>,
    /// From NVIDIA procfs, which never wakes the card.
    model: Option<String>,
    /// Whether the driver was looked at; done once, the first time the card may be queried.
    identified: bool,
    amd: Option<AmdFiles>,
    /// Its hwmon chip, opened only while the card may be queried.
    chip: Option<Chip>,
    /// The hwmon directory of [`Self::chip`], found at discovery but not read yet.
    chip_dir: Option<PathBuf>,
}

impl Card {
    fn new(name: String, device: PathBuf) -> Self {
        let runtime_status = Pread::open(&device.join("power/runtime_status"));
        Self {
            name,
            device,
            runtime_status,
            model: None,
            identified: false,
            amd: None,
            chip: None,
            chip_dir: None,
        }
    }

    fn sample(&mut self, failed: &mut bool) -> GpuSample {
        let status = self
            .runtime_status
            .as_mut()
            .map_or(RuntimeStatus::Unknown, |f| {
                f.parse(failed, |t| Some(drm::parse_runtime_status(t)))
                    .unwrap_or(RuntimeStatus::Unknown)
            });
        let mut gpu = GpuSample {
            card: self.name.clone(),
            name: self.model.clone(),
            status,
            busy: None,
            vram_used: None,
            vram_total: None,
            celsius: None,
        };
        if !status.may_query() {
            return gpu;
        }
        if !self.identified {
            self.identify();
        }
        if let Some(amd) = &mut self.amd {
            gpu.busy = amd
                .busy
                .as_mut()
                .and_then(|f| f.read().ok().and_then(drm::parse_busy_percent));
            gpu.vram_used = amd
                .vram_used
                .as_mut()
                .and_then(|f| f.read().ok().and_then(drm::parse_bytes));
            gpu.vram_total = amd.vram_total;
        }
        // Other drivers' stats and hwmon come from the probe (P4).
        if self.amd.is_some()
            && self.chip.is_none()
            && let Some(dir) = self.chip_dir.take()
        {
            self.chip = Chip::open(&dir);
        }
        if let Some(ChipTemp::AmdGpu(c)) = self.chip.as_mut().and_then(Chip::read) {
            gpu.celsius = Some(c);
        }
        gpu
    }

    /// Looks at the driver. Only called while the card may be queried (D3).
    fn identify(&mut self) {
        self.identified = true;
        let driver = std::fs::read_link(self.device.join("driver")).ok();
        let driver = driver.as_deref().and_then(Path::file_name);
        if driver.is_some_and(|d| d == "amdgpu") {
            let file = |name: &str| Pread::open(&self.device.join(name));
            self.amd = Some(AmdFiles {
                busy: file("gpu_busy_percent"),
                vram_used: file("mem_info_vram_used"),
                vram_total: std::fs::read_to_string(self.device.join("mem_info_vram_total"))
                    .ok()
                    .and_then(|t| drm::parse_bytes(&t)),
            });
        }
    }
}

/// An `nvme` drive's hwmon chip.
#[derive(Debug)]
struct Drive {
    name: String,
    chip: Chip,
}

/// Every file the closed tier reads.
#[derive(Debug)]
pub struct Readers {
    stat: Option<Pread>,
    meminfo: Option<Pread>,
    loadavg: Option<Pread>,
    uptime: Option<Pread>,
    net_dev: Option<Pread>,
    diskstats: Option<Pread>,
    physical_net: HashSet<String>,
    physical_disks: HashSet<String>,
    cards: Vec<Card>,
    cpu_chip: Option<Chip>,
    drives: Vec<Drive>,
    /// Whether this runs in a Flatpak sandbox, where `/` is not the host's (the probe reads
    /// mounts in P2).
    in_flatpak: bool,
    root_fs: Option<(Instant, Option<FsUsage>)>,
    /// The last drive temperatures and when they were read.
    drive_temps: Option<(Instant, Vec<DriveTemp>)>,
    discovered_at: Option<Instant>,
    /// A read failed: look for devices again on the next tick.
    failed: bool,
}

impl Readers {
    #[must_use]
    pub fn new() -> Self {
        Self {
            stat: None,
            meminfo: None,
            loadavg: None,
            uptime: None,
            net_dev: None,
            diskstats: None,
            physical_net: HashSet::new(),
            physical_disks: HashSet::new(),
            cards: Vec::new(),
            cpu_chip: None,
            drives: Vec::new(),
            in_flatpak: Path::new("/.flatpak-info").exists(),
            root_fs: None,
            drive_temps: None,
            discovered_at: None,
            failed: false,
        }
    }

    /// Reads one sample at `now`.
    pub fn read(&mut self, now: Instant) -> Sample {
        let due = self
            .discovered_at
            .is_none_or(|at| now.duration_since(at) >= REDISCOVER_EVERY);
        if due || self.failed {
            self.discover();
            self.discovered_at = Some(now);
            self.failed = false;
        }
        let drives = self.drives(now);
        let root_fs = self.root_fs(now);
        let failed = &mut self.failed;
        let read = |slot: &mut Option<Pread>, failed: &mut bool| {
            slot.as_mut().and_then(|f| {
                let text = f.read().map(str::to_owned).ok();
                *failed |= text.is_none();
                text
            })
        };
        let net = read(&mut self.net_dev, failed).map_or_else(Vec::new, |t| {
            net_dev::physical(net_dev::parse(&t), &self.physical_net)
        });
        let disks = read(&mut self.diskstats, failed).map_or_else(Vec::new, |t| {
            diskstats::whole_disks(diskstats::parse(&t), &self.physical_disks)
        });
        Sample {
            stat: self
                .stat
                .as_mut()
                .and_then(|f| f.parse(failed, proc_stat::parse)),
            mem: self
                .meminfo
                .as_mut()
                .and_then(|f| f.parse(failed, meminfo::parse)),
            load: self
                .loadavg
                .as_mut()
                .and_then(|f| f.parse(failed, loadavg::parse)),
            uptime: self
                .uptime
                .as_mut()
                .and_then(|f| f.parse(failed, uptime::parse)),
            net,
            disks,
            cpu_temp: match self.cpu_chip.as_mut().and_then(Chip::read) {
                Some(ChipTemp::CpuPackage(c)) => Some(c),
                _ => None,
            },
            drives,
            gpus: self.cards.iter_mut().map(|c| c.sample(failed)).collect(),
            root_fs,
        }
    }

    /// The drives' temperatures, reread every [`DRIVE_TEMP_EVERY`].
    fn drives(&mut self, now: Instant) -> Vec<DriveTemp> {
        if let Some((at, temps)) = &self.drive_temps
            && now.duration_since(*at) < DRIVE_TEMP_EVERY
        {
            return temps.clone();
        }
        let temps: Vec<_> = self
            .drives
            .iter_mut()
            .filter_map(|d| match d.chip.read() {
                Some(ChipTemp::Nvme { celsius, limit }) => Some(DriveTemp {
                    name: d.name.clone(),
                    celsius,
                    limit,
                }),
                _ => None,
            })
            .collect();
        self.drive_temps = Some((now, temps.clone()));
        temps
    }

    fn root_fs(&mut self, now: Instant) -> Option<FsUsage> {
        if self.in_flatpak {
            return None;
        }
        match self.root_fs {
            Some((at, usage)) if now.duration_since(at) < ROOT_FS_EVERY => usage,
            _ => {
                let usage = rustix::fs::statvfs("/").ok().map(|s| FsUsage {
                    used: s
                        .f_blocks
                        .saturating_sub(s.f_bfree)
                        .saturating_mul(s.f_frsize),
                    total: s.f_blocks.saturating_mul(s.f_frsize),
                });
                self.root_fs = Some((now, usage));
                usage
            }
        }
    }

    fn discover(&mut self) {
        reopen(&mut self.stat, "/proc/stat");
        reopen(&mut self.meminfo, "/proc/meminfo");
        reopen(&mut self.loadavg, "/proc/loadavg");
        reopen(&mut self.uptime, "/proc/uptime");
        reopen(&mut self.net_dev, "/proc/net/dev");
        reopen(&mut self.diskstats, "/proc/diskstats");
        self.physical_net = discover::names_with_device_link(Path::new("/sys/class/net"));
        self.physical_disks = discover::names_with_device_link(Path::new("/sys/block"));
        self.discover_cards();
        self.discover_chips();
    }

    fn discover_cards(&mut self) {
        let mut names: Vec<String> = sorted_entries(Path::new("/sys/class/drm"))
            .into_iter()
            .filter(|n| {
                n.strip_prefix("card")
                    .is_some_and(|i| !i.is_empty() && i.bytes().all(|b| b.is_ascii_digit()))
            })
            .collect();
        // card10 after card9.
        names.sort_by_key(|n| (n.len(), n.clone()));
        let mut old = std::mem::take(&mut self.cards);
        for name in names.into_iter().take(MAX_GPUS) {
            // The class link and the `device` link are only resolved, nothing under it is read.
            let Ok(device) = std::fs::canonicalize(format!("/sys/class/drm/{name}/device")) else {
                continue;
            };
            let card = match old
                .iter()
                .position(|c| c.name == name && c.device == device)
            {
                Some(i) => {
                    let mut card = old.swap_remove(i);
                    if card.runtime_status.is_none() {
                        card.runtime_status = Pread::open(&device.join("power/runtime_status"));
                    }
                    card
                }
                None => Card::new(name, device),
            };
            self.cards.push(card);
        }
        self.name_nvidia_cards();
    }

    /// Names NVIDIA cards from `/proc/driver/nvidia/gpus/*/information`, which the driver
    /// answers without waking the card (D1).
    fn name_nvidia_cards(&mut self) {
        let root = Path::new("/proc/driver/nvidia/gpus");
        for bus in sorted_entries(root).into_iter().take(MAX_GPUS) {
            let Some(info) = std::fs::read_to_string(root.join(&bus).join("information"))
                .ok()
                .and_then(|t| nvidia_procfs::parse_information(&t))
            else {
                continue;
            };
            let bus = info.bus_location.unwrap_or(bus);
            if let Some(card) = self.cards.iter_mut().find(|c| {
                c.device
                    .file_name()
                    .is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case(&bus))
            }) {
                card.model = Some(info.model);
            }
        }
    }

    fn discover_chips(&mut self) {
        self.cpu_chip = None;
        self.drives.clear();
        self.drive_temps = None;
        let root = Path::new("/sys/class/hwmon");
        for entry in sorted_entries(root) {
            let dir = root.join(&entry);
            let device = std::fs::canonicalize(dir.join("device")).ok();
            // A GPU's chip: only its card may say when it can be read (D3).
            if let Some(device) = &device
                && let Some(card) = self
                    .cards
                    .iter_mut()
                    .find(|c| device.starts_with(&c.device))
            {
                if card.chip.is_none() {
                    card.chip_dir = Some(dir);
                }
                continue;
            }
            let Some(name) = std::fs::read_to_string(dir.join("name")).ok() else {
                continue;
            };
            match name.trim() {
                "k10temp" | "zenpower" | "coretemp" if self.cpu_chip.is_none() => {
                    self.cpu_chip = Chip::open(&dir);
                }
                "nvme" if self.drives.len() < MAX_DRIVES => {
                    let Some(chip) = Chip::open(&dir) else {
                        continue;
                    };
                    let name = device
                        .as_deref()
                        .and_then(|d| std::fs::read_to_string(d.join("model")).ok())
                        .and_then(|m| nvme::parse_model(&m))
                        .or_else(|| {
                            device
                                .as_deref()
                                .and_then(Path::file_name)
                                .map(|f| clip_name(&f.to_string_lossy()))
                        })
                        .unwrap_or_else(|| clip_name(&entry));
                    self.drives.push(Drive { name, chip });
                }
                _ => {}
            }
        }
    }
}

/// The names in `dir`, sorted, at most [`MAX_DIR_ENTRIES`].
fn sorted_entries(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .take(MAX_DIR_ENTRIES)
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pread_rereads_a_file_from_the_start() {
        let path = std::env::temp_dir().join(format!("sysmon-pread-{}", std::process::id()));
        std::fs::write(&path, "first").unwrap();
        let mut file = Pread::open(&path).unwrap();
        assert_eq!(file.read().unwrap(), "first");
        assert_eq!(file.read().unwrap(), "first");
        // Rewritten in place, as sysfs and procfs regenerate a file on each read.
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .write_all_at(b"again", 0)
            .unwrap();
        assert_eq!(file.read().unwrap(), "again");
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn pread_reads_files_larger_than_one_buffer() {
        let path = std::env::temp_dir().join(format!("sysmon-pread-big-{}", std::process::id()));
        let text = "x".repeat(10_000);
        std::fs::write(&path, &text).unwrap();
        assert_eq!(Pread::open(&path).unwrap().read().unwrap(), text);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn reads_this_machine_s_procfs() {
        let mut readers = Readers::new();
        let sample = readers.read(Instant::now());
        assert!(sample.stat.is_some_and(|s| !s.cores.is_empty()));
        assert!(sample.mem.is_some());
        assert!(sample.load.is_some());
        assert!(sample.uptime.is_some());
    }
}
