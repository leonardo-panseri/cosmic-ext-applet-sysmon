// SPDX-License-Identifier: MPL-2.0

//! The sampler's I/O: the procfs and sysfs files of the closed tier (D2), kept open and reread
//! with `pread` at offset 0, so a tick makes no `open()` (§3.1). Devices are discovered at start,
//! every [`REDISCOVER_EVERY`], and after a read error.
//!
//! **D3.** Of a DRM card, only `device/power/runtime_status` is read until it says the card
//! [may be queried](RuntimeStatus::may_query). Its driver, its files and any hwmon chip under its
//! device are looked at only then, and the chip and files only on ticks where it still may be.
//! Nothing here opens `/dev`.
//!
//! What the sandbox can't see comes from the [`Probe`]: the host's mounts every [`MOUNTS_EVERY`]
//! in both tiers, the top processes on every open-tier tick (D2), and NVIDIA stats through NVML
//! when the back-off of [`NvmlSchedule`] says so (ADR 0003). Those calls block this thread for at
//! most the probe's timeouts.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sysmon_proto::{Mount, NvidiaState, NvmlSample};

use super::nvml_schedule::{IDLE_INTERVAL, NvmlSchedule, Tick};
use super::stats::{DiskLayout, DriveTemp, GpuSample, Processes, Sample};
use crate::config::VramCache;
use crate::probe_client::{self, Probe};
use crate::sources::block::BlockTopology;
use crate::sources::drm::{self, RuntimeStatus};
use crate::sources::hwmon::{self, ChipTemp};
use crate::sources::{
    MAX_DRIVES, MAX_GPUS, clip_name, discover, diskstats, gpu_name, loadavg, meminfo, net_dev,
    nvidia_procfs, nvme, proc_stat, uptime,
};

/// How often devices are looked for again: network interfaces, disks, GPUs, hwmon chips.
pub const REDISCOVER_EVERY: Duration = Duration::from_secs(60);
/// How often the probe is asked for the mounts and their space (D2).
pub const MOUNTS_EVERY: Duration = Duration::from_secs(30);
/// How often `nvme` drive temperatures are reread. Each read is a SMART command to the drive, which
/// blocks the sampler (one more wakeup) and wakes the drive's controller from its idle power
/// state; a drive heats up over minutes, so a warning 30 s late is still in time.
const DRIVE_TEMP_EVERY: Duration = Duration::from_secs(30);
/// Largest file read, so a misbehaving file can't grow the buffer without bound (D8).
const MAX_FILE_BYTES: usize = 1 << 20;
/// Most hwmon chips and DRM entries looked at (D8).
const MAX_DIR_ENTRIES: usize = 256;
/// How long an NVML sample is shown on the ticks the back-off skips: one backed-off interval,
/// plus a battery tick of slack.
const NVML_HOLD: Duration = IDLE_INTERVAL.saturating_add(Duration::from_secs(5));

/// What the current tier reads beyond the closed tier's files.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wants {
    /// The top processes: the popup is open.
    pub processes: bool,
    /// Which NVIDIA cards' stats (D2).
    pub nvml: Nvml,
}

/// Which NVIDIA cards someone looks at, so NVML is asked while one of them is awake, with the
/// back-off of ADR 0003. Whatever is wanted, a suspended card is never asked about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Nvml {
    /// None.
    #[default]
    Off,
    /// Any: the popup is open, an NVIDIA warning is on, or the panel button shows a GPU item
    /// whose GPU is Auto.
    Any,
    /// Only those at these PCI addresses: the panel button shows GPU items for them.
    Cards(Vec<String>),
}

impl Nvml {
    /// Whether `gpu`'s stats are wanted, if it is an NVIDIA card.
    #[must_use]
    pub fn covers(&self, gpu: &GpuSample) -> bool {
        match self {
            Self::Off => false,
            Self::Any => true,
            Self::Cards(cards) => gpu.pci.as_ref().is_some_and(|pci| cards.contains(pci)),
        }
    }
}

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

/// How [`Chip::open`] reads a chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    /// Every temperature file: an on-chip sensor answers from a register.
    All,
    /// The name and labels, then only the main sensor's limits; no `temp*_input`. Every other
    /// temperature file of an `nvme` chip is a command to the drive.
    Labels,
}

impl Chip {
    /// Reads the chip in `dir`, as `how` says, if it is one [`hwmon::classify`] knows.
    fn open(dir: &Path, how: Open) -> Option<Self> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).ok()?.take(MAX_DIR_ENTRIES).flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if !hwmon::is_wanted_file(&name) {
                continue;
            }
            let lazy = how == Open::Labels && name != "name" && !name.ends_with("_label");
            if lazy {
                // Only that the sensor exists, so it can be classified; never its value.
                if name.ends_with("_input") {
                    files.push((name, "0".to_owned()));
                }
            } else if let Ok(text) = std::fs::read_to_string(entry.path()) {
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
        if how == Open::Labels {
            for limit in [format!("{prefix}max"), format!("{prefix}crit")] {
                if let Ok(text) = std::fs::read_to_string(dir.join(&limit)) {
                    files.push((limit, text));
                }
            }
        }
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

/// What NVIDIA procfs says about a card, without waking it.
#[derive(Debug, Clone)]
struct NvidiaIds {
    /// The PCI address, as the probe reports it.
    bus_id: String,
    uuid: Option<String>,
}

/// A DRM card.
#[derive(Debug)]
struct Card {
    name: String,
    /// The canonical path of its device, to match hwmon chips to it.
    device: PathBuf,
    runtime_status: Option<Pread>,
    /// From NVIDIA procfs, which never wakes the card, or for AMD looked up when identified.
    model: Option<String>,
    /// Set for a card the NVIDIA driver lists.
    nvidia: Option<NvidiaIds>,
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
            nvidia: None,
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
            pci: self.pci(),
            nvidia: self.nvidia.is_some(),
            name: self.model.clone(),
            uuid: self.nvidia.as_ref().and_then(|n| n.uuid.clone()),
            status,
            ..GpuSample::default()
        };
        // An NVIDIA card's stats come from the probe (NVML), never from its PCI device.
        if !status.may_query() || self.nvidia.is_some() {
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
            gpu.name.clone_from(&self.model);
        }
        if self.amd.is_some()
            && self.chip.is_none()
            && let Some(dir) = self.chip_dir.take()
        {
            self.chip = Chip::open(&dir, Open::All);
        }
        if let Some(ChipTemp::AmdGpu(c)) = self.chip.as_mut().and_then(Chip::read) {
            gpu.celsius = Some(c);
        }
        gpu
    }

    /// The PCI address: the name of the device directory, which discovery already resolved, for
    /// a device on PCI. Nothing is read.
    fn pci(&self) -> Option<String> {
        if let Some(nvidia) = &self.nvidia {
            return Some(nvidia.bus_id.clone());
        }
        let name = self.device.file_name()?.to_string_lossy();
        sysmon_probe::nvidia::is_bus_id(&name).then(|| name.to_ascii_lowercase())
    }

    /// Looks at the driver, and names an AMD card. Only called while the card may be queried
    /// (D3), and never for an NVIDIA card.
    fn identify(&mut self) {
        self.identified = true;
        let driver = std::fs::read_link(self.device.join("driver")).ok();
        let driver = driver.as_deref().and_then(Path::file_name);
        if driver.is_some_and(|d| d == "amdgpu") {
            let file = |name: &str| Pread::open(&self.device.join(name));
            let vram_total = std::fs::read_to_string(self.device.join("mem_info_vram_total"))
                .ok()
                .and_then(|t| drm::parse_bytes(&t));
            self.amd = Some(AmdFiles {
                busy: file("gpu_busy_percent"),
                vram_used: file("mem_info_vram_used"),
                vram_total,
            });
            self.model = Some(amd_name(&self.device, vram_total));
        }
    }
}

/// An `amdgpu` card's name: `product_name`, else its `pci.ids` entry, else a generic one. Reads
/// its vendor and device ids, which is safe for an awake AMD card and never done for NVIDIA.
fn amd_name(device: &Path, vram_total: Option<u64>) -> String {
    let read = |name: &str| std::fs::read_to_string(device.join(name)).ok();
    if let Some(name) = read("product_name").map(|n| clip_name(&n))
        && !name.is_empty()
    {
        return name;
    }
    let ids = read("vendor")
        .and_then(|v| gpu_name::parse_id(&v))
        .filter(|&v| v == gpu_name::AMD_VENDOR)
        .zip(read("device").and_then(|d| gpu_name::parse_id(&d)));
    if let Some((vendor, device_id)) = ids {
        for path in gpu_name::PCI_IDS_PATHS {
            if let Ok(file) = File::open(path) {
                if let Some(name) = gpu_name::lookup(io::BufReader::new(file), vendor, device_id) {
                    return gpu_name::amd_display_name(&name);
                }
                break;
            }
        }
    }
    let boot_vga = read("boot_vga").is_some_and(|b| b.trim() == "1");
    gpu_name::amd_generic_name(boot_vga, vram_total).to_owned()
}

/// An `nvme` drive's hwmon chip, opened once ([`Open::Labels`]) and kept while the chip is
/// there, so its limits are read only once.
#[derive(Debug)]
struct Drive {
    name: String,
    /// The chip's hwmon directory and the canonical path of its device: which chip it is.
    dir: PathBuf,
    device: Option<PathBuf>,
    chip: Chip,
    /// The disk it is ([`Sample::drive_disks`]), found at each discovery.
    disk: Option<String>,
}

/// A drive's temperature, and the disk it is ([`Sample::drive_disks`]).
type DriveTempOn = (DriveTemp, Option<String>);

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
    /// `/sys/class/hwmon`.
    hwmon: PathBuf,
    /// `/sys/block` and `/sys/class/block`.
    sys_block: PathBuf,
    class_block: PathBuf,
    /// The disks' models and how block devices stack, from the last discovery.
    disk_layout: Arc<DiskLayout>,
    probe: Box<dyn Probe>,
    /// When to ask the probe for NVIDIA stats (ADR 0003).
    nvml: NvmlSchedule,
    /// The last NVML sample of each card, by bus id, and when it was taken: shown on the ticks
    /// the back-off skips.
    nvml_samples: HashMap<String, (Instant, NvmlSample)>,
    vram: VramCache,
    /// The last mounts and when they were asked for.
    mounts: Option<(Instant, Vec<Mount>)>,
    /// The last drive temperatures, with each drive's disk, and when they were read.
    drive_temps: Option<(Instant, Vec<DriveTempOn>)>,
    discovered_at: Option<Instant>,
    /// A read failed: look for devices again on the next tick.
    failed: bool,
}

impl Readers {
    /// Readers that ask `probe` for what the sandbox can't see, remembering VRAM sizes in `vram`.
    #[must_use]
    pub fn new(probe: Box<dyn Probe>, vram: VramCache) -> Self {
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
            hwmon: PathBuf::from("/sys/class/hwmon"),
            sys_block: PathBuf::from("/sys/block"),
            class_block: PathBuf::from("/sys/class/block"),
            disk_layout: Arc::default(),
            probe,
            nvml: NvmlSchedule::default(),
            nvml_samples: HashMap::new(),
            vram,
            mounts: None,
            drive_temps: None,
            discovered_at: None,
            failed: false,
        }
    }

    /// Reads one sample at `now`, with what the tier `wants`.
    pub fn read(&mut self, now: Instant, wants: &Wants) -> Sample {
        let due = self
            .discovered_at
            .is_none_or(|at| now.duration_since(at) >= REDISCOVER_EVERY);
        if due || self.failed {
            self.discover();
            self.discovered_at = Some(now);
            self.failed = false;
        }
        let (drives, drive_disks) = self.drives(now).into_iter().unzip();
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
        let mut sample = Sample {
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
            drive_disks,
            gpus: self.cards.iter_mut().map(|c| c.sample(failed)).collect(),
            mounts: Vec::new(),
            processes: Processes::Pending,
            disk_layout: Arc::clone(&self.disk_layout),
        };
        // After the counters, so a slow probe doesn't skew the rates measured from `now`.
        sample.mounts = self.mounts(now);
        if wants.processes {
            sample.processes = self
                .probe
                .processes()
                .map_or(Processes::Unavailable, Processes::Top);
        }
        self.nvidia(now, &wants.nvml, &mut sample.gpus);
        sample
    }

    /// Fills in the NVIDIA cards of `gpus`: asks the probe when the back-off says so, shows the
    /// last sample on the ticks in between, and the cached VRAM size while suspended (D4).
    /// Only a wanted card being awake lets the probe be asked; the probe itself also asks
    /// nothing unless every NVIDIA card is awake (ADR 0003).
    fn nvidia(&mut self, now: Instant, wanted: &Nvml, gpus: &mut [GpuSample]) {
        let tick = Tick {
            now,
            awake: gpus
                .iter()
                .any(|g| g.nvidia && g.status.may_query() && wanted.covers(g)),
            wanted: *wanted != Nvml::Off,
        };
        if self.nvml.due(tick) {
            let backed_off = self.nvml.backed_off();
            let reply = self.probe.nvidia().unwrap_or_default();
            let mut load = None;
            for gpu in reply {
                if let NvidiaState::Sampled(sample) = gpu.state {
                    load = load.max(sample.util);
                    if let Some(total) = sample.vram_total
                        && let Some(uuid) = sample.uuid.as_deref().or_else(|| {
                            self.cards
                                .iter()
                                .filter_map(|c| c.nvidia.as_ref())
                                .find(|n| n.bus_id == gpu.bus_id)
                                .and_then(|n| n.uuid.as_deref())
                        })
                    {
                        self.vram.remember(uuid, total);
                    }
                    self.nvml_samples.insert(gpu.bus_id, (now, sample));
                } else {
                    self.nvml_samples.remove(&gpu.bus_id);
                }
            }
            self.nvml.record(now, load);
            if probe_client::debug() {
                let schedule = match (backed_off, self.nvml.backed_off()) {
                    (false, true) => "backing off to every 15 s",
                    (true, false) => "load: back to every tick",
                    (true, true) => "backed off",
                    (false, false) => "every tick",
                };
                eprintln!(
                    "[{}] nvml: asked, load {load:?}; {schedule}",
                    probe_client::stamp()
                );
            }
        }
        for (card, gpu) in self.cards.iter().zip(gpus.iter_mut()) {
            let Some(ids) = &card.nvidia else {
                continue;
            };
            if gpu.suspended() {
                self.nvml_samples.remove(&ids.bus_id);
                if let Some(total) = ids.uuid.as_deref().and_then(|u| self.vram.get(u)) {
                    gpu.vram_used = Some(0);
                    gpu.vram_total = Some(total);
                }
                continue;
            }
            let Some((_, sample)) = self
                .nvml_samples
                .get(&ids.bus_id)
                .filter(|(at, _)| now.saturating_duration_since(*at) <= NVML_HOLD)
            else {
                continue;
            };
            if gpu.name.is_none() {
                gpu.name.clone_from(&sample.name);
            }
            gpu.busy = sample.util;
            gpu.celsius = sample.celsius;
            gpu.watts = sample.watts;
            gpu.vram_used = sample.vram_used;
            gpu.vram_total = sample.vram_total;
        }
    }

    /// The drives' temperatures, each with its disk, reread every [`DRIVE_TEMP_EVERY`].
    fn drives(&mut self, now: Instant) -> Vec<DriveTempOn> {
        if let Some((at, temps)) = &self.drive_temps
            && now.duration_since(*at) < DRIVE_TEMP_EVERY
        {
            return temps.clone();
        }
        let temps: Vec<_> = self
            .drives
            .iter_mut()
            .filter_map(|d| match d.chip.read() {
                Some(ChipTemp::Nvme { celsius, limit }) => Some((
                    DriveTemp {
                        name: d.name.clone(),
                        celsius,
                        limit,
                        extremes: None,
                    },
                    d.disk.clone(),
                )),
                _ => None,
            })
            .collect();
        self.drive_temps = Some((now, temps.clone()));
        temps
    }

    /// With `SYSMON_DEBUG`, asks for the processes once, so the probe's first process reply is
    /// logged without opening the popup.
    pub fn debug_probe(&mut self) {
        if crate::probe_client::debug() {
            self.probe.processes();
        }
    }

    /// The mounts, asked for again every [`MOUNTS_EVERY`]; empty without a probe.
    fn mounts(&mut self, now: Instant) -> Vec<Mount> {
        if let Some((at, mounts)) = &self.mounts
            && now.duration_since(*at) < MOUNTS_EVERY
        {
            return mounts.clone();
        }
        let mounts = self.probe.mounts().unwrap_or_default();
        self.mounts = Some((now, mounts.clone()));
        mounts
    }

    fn discover(&mut self) {
        reopen(&mut self.stat, "/proc/stat");
        reopen(&mut self.meminfo, "/proc/meminfo");
        reopen(&mut self.loadavg, "/proc/loadavg");
        reopen(&mut self.uptime, "/proc/uptime");
        reopen(&mut self.net_dev, "/proc/net/dev");
        reopen(&mut self.diskstats, "/proc/diskstats");
        self.physical_net = discover::names_with_device_link(Path::new("/sys/class/net"));
        self.physical_disks = discover::names_with_device_link(&self.sys_block);
        self.discover_cards();
        self.discover_chips();
        self.discover_disks();
    }

    /// Names the physical disks, maps block devices to them, and says which disk each drive's
    /// chip is: only links, directory names and `model` files, never a drive's sensors. Block
    /// devices never live under a GPU's PCI device, so nothing here touches a card (D3).
    fn discover_disks(&mut self) {
        let mut paths = Vec::new();
        let mut models = HashMap::new();
        for name in &self.physical_disks {
            if diskstats::is_virtual(name) {
                continue;
            }
            let dir = self.sys_block.join(name);
            // An NVMe namespace's `device` is its controller, which has the model too.
            if let Some(model) = std::fs::read_to_string(dir.join("device/model"))
                .ok()
                .and_then(|m| nvme::parse_model(&m))
            {
                models.insert(name.clone(), model);
            }
            if let Ok(path) = std::fs::canonicalize(&dir) {
                paths.push((name.clone(), path));
            }
        }
        for drive in &mut self.drives {
            // An `nvme` chip's device is the controller; its namespaces are the disks under it.
            drive.disk = drive.device.as_ref().and_then(|controller| {
                paths
                    .iter()
                    .filter(|(_, path)| path.starts_with(controller))
                    .map(|(name, _)| name.clone())
                    .min()
            });
        }
        if let Some((_, temps)) = &mut self.drive_temps {
            for (temp, disk) in temps {
                if let Some(drive) = self.drives.iter().find(|d| d.name == temp.name) {
                    disk.clone_from(&drive.disk);
                }
            }
        }
        self.disk_layout = Arc::new(DiskLayout {
            models,
            topology: BlockTopology::read(&self.class_block),
        });
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
                card.nvidia = Some(NvidiaIds {
                    bus_id: bus.to_ascii_lowercase(),
                    uuid: info.uuid,
                });
            }
        }
    }

    /// Looks for the CPU's and the drives' chips again. A drive's chip already open is kept, and
    /// so is its last temperature: rediscovery reads nothing of a drive but its `name` file
    /// ([`DRIVE_TEMP_EVERY`]).
    fn discover_chips(&mut self) {
        self.cpu_chip = None;
        let mut old_drives = std::mem::take(&mut self.drives);
        let root = self.hwmon.clone();
        for entry in sorted_entries(&root) {
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
                    self.cpu_chip = Chip::open(&dir, Open::All);
                }
                "nvme" if self.drives.len() < MAX_DRIVES => {
                    if let Some(i) = old_drives
                        .iter()
                        .position(|d| d.dir == dir && d.device == device)
                    {
                        self.drives.push(old_drives.swap_remove(i));
                        continue;
                    }
                    let Some(chip) = Chip::open(&dir, Open::Labels) else {
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
                    self.drives.push(Drive {
                        name,
                        dir,
                        device,
                        chip,
                        disk: None,
                    });
                }
                _ => {}
            }
        }
        if let Some((_, temps)) = &mut self.drive_temps {
            temps.retain(|(t, _)| self.drives.iter().any(|d| d.name == t.name));
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
    use std::sync::{Arc, Mutex};

    use sysmon_proto::{Reply, Request, TopProcesses};

    use super::*;
    use crate::probe_client::Unavailable;

    /// A probe that records what it is asked and answers with nothing.
    #[derive(Debug, Default, Clone)]
    struct Recorder(Arc<Mutex<Vec<Request>>>);

    impl Probe for Recorder {
        fn ask(&mut self, request: Request) -> Option<Reply> {
            self.0.lock().unwrap().push(request);
            Some(match request {
                Request::Processes { .. } => Reply::Processes(TopProcesses::default()),
                Request::Mounts => Reply::Mounts { mounts: vec![] },
                Request::Gpus { .. } => Reply::Gpus { gpus: vec![] },
                Request::Release => Reply::Released,
            })
        }
    }

    /// The open tier without NVML, so these tests never ask about this machine's GPU.
    const OPEN: Wants = Wants {
        processes: true,
        nvml: Nvml::Off,
    };

    /// A probe whose one NVIDIA card at `0000:01:00.0` answers with `util`; counts the asks.
    #[derive(Debug, Clone, Default)]
    struct FakeNvidia {
        util: Arc<Mutex<u8>>,
        asked: Arc<Mutex<u32>>,
    }

    impl Probe for FakeNvidia {
        fn ask(&mut self, request: Request) -> Option<Reply> {
            assert_eq!(request, Request::Gpus { nvml: true });
            *self.asked.lock().unwrap() += 1;
            Some(Reply::Gpus {
                gpus: vec![sysmon_proto::NvidiaGpu {
                    bus_id: "0000:01:00.0".into(),
                    state: NvidiaState::Sampled(NvmlSample {
                        uuid: Some("GPU-1".into()),
                        name: Some("NVML name".into()),
                        util: Some(*self.util.lock().unwrap()),
                        celsius: Some(52.0),
                        watts: Some(31.5),
                        vram_used: Some(512 << 20),
                        vram_total: Some(8188 << 20),
                    }),
                }],
            })
        }
    }

    #[test]
    fn nvidia_stats_come_from_the_probe_and_vram_from_the_cache_while_suspended() {
        let dir = std::env::temp_dir().join(format!("sysmon-readers-nv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("power")).unwrap();
        let status = dir.join("power/runtime_status");
        std::fs::write(&status, "suspended\n").unwrap();

        let probe = FakeNvidia::default();
        let mut readers = Readers::new(Box::new(probe.clone()), VramCache::default());
        let mut card = Card::new("card0".into(), dir.clone());
        card.model = Some("NVIDIA GeForce RTX 4060 Laptop GPU".into());
        card.nvidia = Some(NvidiaIds {
            bus_id: "0000:01:00.0".into(),
            uuid: Some("GPU-1".into()),
        });
        readers.cards.push(card);
        let tick = |readers: &mut Readers, at: Instant, wanted: &Nvml| {
            let mut failed = false;
            let mut gpus: Vec<_> = readers
                .cards
                .iter_mut()
                .map(|c| c.sample(&mut failed))
                .collect();
            readers.nvidia(at, wanted, &mut gpus);
            gpus.remove(0)
        };
        let t0 = Instant::now();
        let s = Duration::from_secs;

        // Suspended and never seen awake: only `Suspended`, and nothing asked.
        let gpu = tick(&mut readers, t0, &Nvml::Any);
        assert!(gpu.suspended() && gpu.nvidia);
        assert_eq!((gpu.vram_used, gpu.vram_total), (None, None));
        assert_eq!(*probe.asked.lock().unwrap(), 0);

        // Awake but nobody looking: nothing asked either.
        std::fs::write(&status, "active\n").unwrap();
        let gpu = tick(&mut readers, t0 + s(2), &Nvml::Off);
        assert_eq!(gpu.busy, None);
        assert_eq!(*probe.asked.lock().unwrap(), 0);

        // Awake and looked at: NVML's numbers, the procfs name.
        *probe.util.lock().unwrap() = 40;
        let gpu = tick(&mut readers, t0 + s(4), &Nvml::Any);
        assert_eq!(
            (
                gpu.busy,
                gpu.celsius,
                gpu.watts,
                gpu.vram_used,
                gpu.vram_total
            ),
            (
                Some(40),
                Some(52.0),
                Some(31.5),
                Some(512 << 20),
                Some(8188 << 20)
            )
        );
        assert_eq!(
            gpu.name.as_deref(),
            Some("NVIDIA GeForce RTX 4060 Laptop GPU")
        );

        // Idle: three samples, then the last one is shown on the skipped ticks.
        *probe.util.lock().unwrap() = 0;
        for i in 0..4 {
            tick(&mut readers, t0 + s(6 + 2 * i), &Nvml::Any);
        }
        assert_eq!(*probe.asked.lock().unwrap(), 4);
        let gpu = tick(&mut readers, t0 + s(14), &Nvml::Any);
        assert_eq!(gpu.busy, Some(0));
        assert_eq!(*probe.asked.lock().unwrap(), 4);

        // Suspended: `0 / 8G` from the cache, and the held sample is dropped.
        std::fs::write(&status, "suspended\n").unwrap();
        let gpu = tick(&mut readers, t0 + s(16), &Nvml::Any);
        assert_eq!(
            (gpu.busy, gpu.vram_used, gpu.vram_total),
            (None, Some(0), Some(8188 << 20))
        );
        assert_eq!(*probe.asked.lock().unwrap(), 4);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_gpu_item_for_one_card_asks_nvml_only_while_that_card_is_awake() {
        let dir = std::env::temp_dir().join(format!("sysmon-readers-one-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("power")).unwrap();
        let status = dir.join("power/runtime_status");
        std::fs::write(&status, "suspended\n").unwrap();
        let probe = FakeNvidia::default();
        let mut readers = Readers::new(Box::new(probe.clone()), VramCache::default());
        let mut card = Card::new("card0".into(), dir.clone());
        card.nvidia = Some(NvidiaIds {
            bus_id: "0000:01:00.0".into(),
            uuid: None,
        });
        readers.cards.push(card);
        let tick = |readers: &mut Readers, at: Instant, wanted: &Nvml| {
            let mut failed = false;
            let mut gpus: Vec<_> = readers
                .cards
                .iter_mut()
                .map(|c| c.sample(&mut failed))
                .collect();
            readers.nvidia(at, wanted, &mut gpus);
            gpus.remove(0)
        };
        let t0 = Instant::now();
        let this = Nvml::Cards(vec!["0000:01:00.0".into()]);
        let other = Nvml::Cards(vec!["0000:05:00.0".into()]);
        // Suspended: nothing asked, whatever is wanted. The PCI address needs no read.
        let gpu = tick(&mut readers, t0, &this);
        assert_eq!(gpu.pci.as_deref(), Some("0000:01:00.0"));
        assert_eq!(*probe.asked.lock().unwrap(), 0);
        std::fs::write(&status, "active\n").unwrap();
        // Awake, but only another card is on the panel button.
        tick(&mut readers, t0 + Duration::from_secs(2), &other);
        assert_eq!(*probe.asked.lock().unwrap(), 0);
        let gpu = tick(&mut readers, t0 + Duration::from_secs(4), &this);
        assert_eq!(*probe.asked.lock().unwrap(), 1);
        assert_eq!(gpu.busy, Some(0));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A hwmon root with one `nvme` chip, `hwmon0`, its `Composite` sensor at `input` with a
    /// `max` of `max`, removed on drop.
    struct FakeHwmon(PathBuf);

    impl FakeHwmon {
        fn new(tag: &str, input: &str, max: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("sysmon-hwmon-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let chip = root.join("hwmon0");
            std::fs::create_dir_all(&chip).unwrap();
            std::fs::write(chip.join("name"), "nvme\n").unwrap();
            std::fs::write(chip.join("temp1_label"), "Composite\n").unwrap();
            let hwmon = Self(root);
            hwmon.set("temp1_input", input);
            hwmon.set("temp1_max", max);
            hwmon
        }

        /// Rewrites `file` of the chip in place, as the kernel answers a new read.
        fn set(&self, file: &str, text: &str) {
            std::fs::write(self.0.join("hwmon0").join(file), format!("{text}\n")).unwrap();
        }

        fn readers(&self) -> Readers {
            let mut readers = Readers::new(Box::new(Unavailable), VramCache::default());
            readers.hwmon.clone_from(&self.0);
            readers
        }
    }

    impl Drop for FakeHwmon {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn drive(celsius: f32, limit: f32) -> DriveTemp {
        DriveTemp {
            name: "hwmon0".into(),
            celsius,
            limit: Some(limit),
            extremes: None,
        }
    }

    #[test]
    fn nvme_drives_are_read_every_30_s_whatever_rediscovery_does() {
        let hwmon = FakeHwmon::new("cadence", "40000", "70000");
        let mut readers = hwmon.readers();
        let t0 = Instant::now();
        let s = Duration::from_secs;
        assert_eq!(
            readers.read(t0, &Wants::default()).drives,
            [drive(40.0, 70.0)]
        );

        hwmon.set("temp1_input", "50000");
        hwmon.set("temp1_max", "80000");
        // A read error elsewhere looks for devices again on the next tick, without the drive.
        readers.failed = true;
        assert_eq!(
            readers.read(t0 + s(2), &Wants::default()).drives,
            [drive(40.0, 70.0)]
        );
        assert_eq!(
            readers.read(t0 + s(29), &Wants::default()).drives,
            [drive(40.0, 70.0)]
        );
        // 30 s on: the temperature again, and the limit read once at first discovery.
        assert_eq!(
            readers.read(t0 + s(30), &Wants::default()).drives,
            [drive(50.0, 70.0)]
        );
        hwmon.set("temp1_input", "55000");
        assert_eq!(
            readers.read(t0 + s(60), &Wants::default()).drives,
            [drive(55.0, 70.0)]
        );
        // Nor does the minute's rediscovery read it.
        hwmon.set("temp1_input", "60000");
        assert_eq!(
            readers.read(t0 + s(62), &Wants::default()).drives,
            [drive(55.0, 70.0)]
        );
    }

    #[test]
    fn an_nvme_drive_is_found_without_reading_its_temperature() {
        // Discovery doesn't read `temp1_input`: a bad value then doesn't hide the drive.
        let hwmon = FakeHwmon::new("discovery", "garbage", "70000");
        let mut readers = hwmon.readers();
        let t0 = Instant::now();
        assert_eq!(readers.read(t0, &Wants::default()).drives, []);
        hwmon.set("temp1_input", "40000");
        let later = t0 + Duration::from_secs(30);
        assert_eq!(
            readers.read(later, &Wants::default()).drives,
            [drive(40.0, 70.0)]
        );
    }

    #[test]
    fn discovery_names_each_disk_and_finds_the_disk_of_each_nvme_chip() {
        use std::os::unix::fs::symlink;
        // Shaped like the reference laptop: /sys/block/<disk> links into the controller's
        // device directory, and the controller's hwmon chip links to the controller.
        let hwmon = FakeHwmon::new("disks", "40000", "70000");
        let sys = hwmon.0.join("sys");
        let controller = sys.join("devices/pci0000:00/0000:00:02.4/0000:05:00.0/nvme/nvme0");
        let namespace = controller.join("nvme0n1");
        std::fs::create_dir_all(namespace.join("nvme0n1p1")).unwrap();
        std::fs::write(namespace.join("nvme0n1p1/partition"), "1\n").unwrap();
        std::fs::write(controller.join("model"), "Micron_2400_MTFDKBA512QFM   \n").unwrap();
        symlink(&controller, namespace.join("device")).unwrap();
        symlink(&controller, hwmon.0.join("hwmon0/device")).unwrap();
        // A SATA disk without a model file is named after its device.
        let sata = sys.join("devices/ata1/host0/target0:0:0/0:0:0:0");
        std::fs::create_dir_all(sata.join("block/sda")).unwrap();
        symlink(&sata, sata.join("block/sda/device")).unwrap();
        let class = sys.join("class/block");
        std::fs::create_dir_all(&class).unwrap();
        symlink(&namespace, class.join("nvme0n1")).unwrap();
        symlink(namespace.join("nvme0n1p1"), class.join("nvme0n1p1")).unwrap();
        symlink(sata.join("block/sda"), class.join("sda")).unwrap();

        let mut readers = hwmon.readers();
        readers.sys_block.clone_from(&class);
        readers.class_block.clone_from(&class);
        let sample = readers.read(Instant::now(), &Wants::default());
        let names: Vec<_> = sample.drives.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Micron_2400_MTFDKBA512QFM"]);
        assert_eq!(sample.drive_disks, [Some("nvme0n1".to_owned())]);
        let layout = &sample.disk_layout;
        assert_eq!(
            layout.models.get("nvme0n1").map(String::as_str),
            Some("Micron_2400_MTFDKBA512QFM")
        );
        assert_eq!(layout.models.get("sda"), None);
        assert_eq!(
            layout.topology.whole_disk("/dev/nvme0n1p1"),
            Some("nvme0n1")
        );
    }

    #[test]
    fn mounts_every_30_s_in_both_tiers_and_processes_only_while_open() {
        let recorder = Recorder::default();
        let mut readers = Readers::new(Box::new(recorder.clone()), VramCache::default());
        let t0 = Instant::now();
        let s = Duration::from_secs;
        let closed = readers.read(t0, &Wants::default());
        assert_eq!(closed.processes, Processes::Pending);
        readers.read(t0 + s(2), &Wants::default());
        let open = readers.read(t0 + s(4), &OPEN);
        assert!(matches!(open.processes, Processes::Top(_)));
        readers.read(t0 + s(30), &Wants::default());
        assert_eq!(
            *recorder.0.lock().unwrap(),
            [
                Request::Mounts,
                Request::Processes { top: 5 },
                Request::Mounts
            ]
        );
    }

    #[test]
    fn without_a_probe_there_are_no_mounts_or_processes() {
        let mut readers = Readers::new(Box::new(Unavailable), VramCache::default());
        let sample = readers.read(Instant::now(), &OPEN);
        assert_eq!(sample.mounts, []);
        assert_eq!(sample.processes, Processes::Unavailable);
    }

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
        let mut readers = Readers::new(Box::new(Unavailable), VramCache::default());
        let sample = readers.read(Instant::now(), &Wants::default());
        assert!(sample.stat.is_some_and(|s| !s.cores.is_empty()));
        assert!(sample.mem.is_some());
        assert!(sample.load.is_some());
        assert!(sample.uptime.is_some());
    }
}
