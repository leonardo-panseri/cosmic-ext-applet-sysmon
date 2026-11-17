// SPDX-License-Identifier: MPL-2.0

//! NVIDIA GPUs through the host's NVML, without ever waking a suspended card (D3, ADR 0003).
//!
//! The cards come from `/proc/driver/nvidia/gpus/<bus id>/`, which the driver answers without
//! waking them, and each one's power state from `/sys/bus/pci/devices/<bus id>/power/runtime_status`.
//! Nothing else of the PCI device is read: no config space, no vendor or device ids.
//!
//! `libnvidia-ml.so.1` is `dlopen`ed on the first request that wants NVML, and kept loaded;
//! loading it doesn't touch any GPU. `nvmlInit_v2` isn't per card: it wakes every suspended
//! one (spike S1). So immediately before it, the `runtime_status` of **every** card the driver
//! lists is read, and NVML is called only if all of them are awake; otherwise every card is
//! skipped. A sample is then one session: init → for each card, handle by bus id → queries →
//! shutdown, so nothing is held between samples and the probe never has NVML initialised when
//! it exits. Every NVML call wakes a suspended card and calls every few seconds keep an idle one
//! awake (S1), so *when* to ask is the applet's back-off, not decided here.

use std::ffi::{CStr, CString, c_char, c_int, c_uint};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sysmon_proto::{MAX_GPUS, NvidiaGpu, NvidiaState, NvmlSample};

use crate::power;

/// A sample slower than this probably raced the card's suspend; logged in debug mode.
pub const SLOW_SAMPLE: Duration = Duration::from_secs(1);
/// Board power above this is implausible when the card's limit is unknown, W.
pub const MAX_WATTS: f32 = 1000.0;
/// A GPU core temperature above this is implausible, °C.
const MAX_CELSIUS: u32 = 150;
/// Most entries looked at in the driver's directory (D8).
const MAX_DIR_ENTRIES: usize = 64;

/// The raw numbers of one NVML sample, before [`plausible`] filters them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Raw {
    pub uuid: Option<String>,
    pub name: Option<String>,
    /// `nvmlUtilization_t.gpu`, %.
    pub util: Option<u32>,
    /// °C.
    pub celsius: Option<u32>,
    /// Power draw and the enforced power limit, mW.
    pub power_mw: Option<u32>,
    pub limit_mw: Option<u32>,
    /// Bytes.
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
}

/// One NVML session per call.
pub trait Nvml: Send + std::fmt::Debug {
    /// Init, then the stats of each card in `bus_ids` in order, then shutdown. `None` if init
    /// fails; otherwise one entry per card, `None` for a card NVML doesn't know or fails on.
    fn session(&self, bus_ids: &[String]) -> Option<Vec<Option<Raw>>>;
}

/// Loads NVML; `None` without it.
pub type Loader = fn() -> Option<Box<dyn Nvml>>;

#[derive(Debug)]
enum Lazy {
    Untried(Loader),
    Loaded(Option<Box<dyn Nvml>>),
}

/// The host's NVIDIA GPUs.
#[derive(Debug)]
pub struct Gpus {
    /// `/proc/driver/nvidia/gpus`.
    procfs: PathBuf,
    /// `/sys/bus/pci/devices`.
    sysfs: PathBuf,
    nvml: Lazy,
    debug: bool,
}

impl Gpus {
    /// This host's GPUs, with the host's `libnvidia-ml.so.1`.
    #[must_use]
    pub fn new(debug: bool) -> Self {
        Self::with(
            Path::new("/proc/driver/nvidia/gpus"),
            Path::new("/sys/bus/pci/devices"),
            Library::load,
            debug,
        )
    }

    /// GPUs listed under `procfs`, with their power state under `sysfs`, sampled with what
    /// `loader` loads.
    #[must_use]
    pub fn with(procfs: &Path, sysfs: &Path, loader: Loader, debug: bool) -> Self {
        Self {
            procfs: procfs.into(),
            sysfs: sysfs.into(),
            nvml: Lazy::Untried(loader),
            debug,
        }
    }

    /// The first [`MAX_GPUS`] NVIDIA GPUs; with `nvml`, sampled if every GPU is awake.
    pub fn answer(&mut self, nvml: bool) -> Vec<NvidiaGpu> {
        let all = self.bus_ids();
        let states = if nvml && !all.is_empty() {
            self.load();
            self.sample(&all)
        } else {
            vec![NvidiaState::Skipped; all.len()]
        };
        all.into_iter()
            .zip(states)
            .take(MAX_GPUS)
            .map(|(bus_id, state)| NvidiaGpu { bus_id, state })
            .collect()
    }

    fn load(&mut self) {
        if let Lazy::Untried(loader) = self.nvml {
            let nvml = loader();
            if nvml.is_none() && self.debug {
                eprintln!("probe: no NVML; no NVIDIA stats");
            }
            self.nvml = Lazy::Loaded(nvml);
        }
    }

    /// The states of the cards `all`, in order: the first [`MAX_GPUS`] sampled in one NVML
    /// session if every card is awake (D3), right after checking; all skipped otherwise.
    fn sample(&self, all: &[String]) -> Vec<NvidiaState> {
        let every = |state: NvidiaState| vec![state; all.len()];
        // Immediately before `nvmlInit_v2`, which would wake any of them.
        if !all.iter().all(|bus_id| self.awake(bus_id)) {
            return every(NvidiaState::Skipped);
        }
        let Lazy::Loaded(Some(nvml)) = &self.nvml else {
            return every(NvidiaState::Unavailable);
        };
        let shown = &all[..all.len().min(MAX_GPUS)];
        let start = Instant::now();
        let raws = nvml.session(shown);
        let took = start.elapsed();
        if took > SLOW_SAMPLE && self.debug {
            eprintln!("probe: NVML sample took {took:?}; it may have raced a suspend");
        }
        let Some(raws) = raws else {
            return every(NvidiaState::Unavailable);
        };
        raws.into_iter()
            .map(|raw| {
                raw.map_or(NvidiaState::Unavailable, |r| {
                    NvidiaState::Sampled(plausible(r))
                })
            })
            .chain(std::iter::repeat(NvidiaState::Unavailable))
            .take(all.len())
            .collect()
    }

    /// Whether the card is awake and stays so without our help ([`power::RuntimeStatus::may_query`]).
    fn awake(&self, bus_id: &str) -> bool {
        std::fs::read_to_string(self.sysfs.join(bus_id).join("power/runtime_status"))
            .is_ok_and(|s| power::parse_runtime_status(&s).may_query())
    }

    /// The bus ids the driver lists, sorted: every one, as each is checked before NVML starts,
    /// but at most [`MAX_DIR_ENTRIES`] (D8).
    fn bus_ids(&self) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(&self.procfs) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = entries
            .take(MAX_DIR_ENTRIES)
            .filter_map(Result::ok)
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|name| is_bus_id(name))
            .collect();
        ids.sort();
        ids
    }
}

/// Whether `name` is a PCI address such as `0000:01:00.0`, so it is safe in a path.
#[must_use]
pub fn is_bus_id(name: &str) -> bool {
    let hex = |s: &str, len: std::ops::RangeInclusive<usize>| {
        len.contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
    };
    let mut parts = name.split(':');
    let (Some(domain), Some(bus), Some(slot), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let Some((device, function)) = slot.split_once('.') else {
        return false;
    };
    hex(domain, 4..=8) && hex(bus, 2..=2) && hex(device, 2..=2) && hex(function, 1..=1)
}

/// `raw` without implausible readings: a load above 100 %, a temperature above 150 °C, power
/// above the card's limit (above 1 kW when it is unknown), which catches the bogus first
/// reading after a wake (S1 read 590 W), and VRAM used above the total.
#[must_use]
pub fn plausible(raw: Raw) -> NvmlSample {
    #[expect(clippy::cast_precision_loss, reason = "milliwatts, far below 2^24")]
    let watts = |mw: u32| mw as f32 / 1000.0;
    let limit = raw.limit_mw.filter(|&l| l > 0).map_or(MAX_WATTS, watts);
    let vram_total = raw.vram_total.filter(|&t| t > 0);
    NvmlSample {
        uuid: raw.uuid,
        name: raw.name,
        util: raw
            .util
            .filter(|&u| u <= 100)
            .and_then(|u| u8::try_from(u).ok()),
        #[expect(clippy::cast_precision_loss, reason = "a temperature")]
        celsius: raw.celsius.filter(|&c| c <= MAX_CELSIUS).map(|c| c as f32),
        watts: raw.power_mw.map(watts).filter(|&w| w <= limit),
        vram_used: raw
            .vram_used
            .filter(|&u| vram_total.is_some_and(|t| u <= t)),
        vram_total,
    }
}

// The parts of `nvml.h` used here.
type Return = c_int;
type Device = *mut std::ffi::c_void;
const SUCCESS: Return = 0;
const TEMPERATURE_GPU: c_int = 0;
/// `NVML_DEVICE_NAME_V2_BUFFER_SIZE` and `NVML_DEVICE_UUID_V2_BUFFER_SIZE`.
const TEXT_BUFFER: usize = 96;

#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: c_uint,
    memory: c_uint,
}

#[repr(C)]
#[derive(Default)]
struct Memory {
    total: u64,
    free: u64,
    used: u64,
}

#[repr(C)]
#[derive(Default)]
struct MemoryV2 {
    version: c_uint,
    total: u64,
    reserved: u64,
    free: u64,
    used: u64,
}

/// `nvmlMemory_v2`: `NVML_STRUCT_VERSION(Memory, 2)`.
#[expect(clippy::cast_possible_truncation, reason = "a 40-byte struct")]
const MEMORY_V2: c_uint = std::mem::size_of::<MemoryV2>() as c_uint | (2 << 24);

type Init = unsafe extern "C" fn() -> Return;
type ByBusId = unsafe extern "C" fn(*const c_char, *mut Device) -> Return;
type GetUtilization = unsafe extern "C" fn(Device, *mut Utilization) -> Return;
type GetTemperature = unsafe extern "C" fn(Device, c_int, *mut c_uint) -> Return;
type GetUint = unsafe extern "C" fn(Device, *mut c_uint) -> Return;
type GetMemory = unsafe extern "C" fn(Device, *mut Memory) -> Return;
type GetMemoryV2 = unsafe extern "C" fn(Device, *mut MemoryV2) -> Return;
type GetText = unsafe extern "C" fn(Device, *mut c_char, c_uint) -> Return;

/// The host's `libnvidia-ml.so.1` and the functions used from it.
struct Library {
    init: Init,
    shutdown: Init,
    by_bus_id: ByBusId,
    utilization: Option<GetUtilization>,
    temperature: Option<GetTemperature>,
    power: Option<GetUint>,
    power_limit: Option<GetUint>,
    memory: Option<GetMemory>,
    memory_v2: Option<GetMemoryV2>,
    name: Option<GetText>,
    uuid: Option<GetText>,
    /// Keeps the function pointers above valid; never unloaded.
    _keep: libloading::Library,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("libnvidia-ml.so.1")
    }
}

impl Library {
    fn load() -> Option<Box<dyn Nvml>> {
        Self::open().map(|l| Box::new(l) as Box<dyn Nvml>)
    }

    fn open() -> Option<Self> {
        /// The symbol `name` as a `T`.
        ///
        /// # Safety
        /// `T` must be the symbol's type, and the result used only while `library` is loaded.
        unsafe fn get<T: Copy>(library: &libloading::Library, name: &[u8]) -> Option<T> {
            // SAFETY: see above.
            unsafe { library.get::<T>(name).ok().map(|s| *s) }
        }
        // SAFETY: NVML's initialisers only set up the library; it touches no device until
        // `nvmlInit`.
        let library = unsafe { libloading::Library::new("libnvidia-ml.so.1") }.ok()?;
        // SAFETY: each symbol is looked up with the signature `nvml.h` gives it, and the
        // pointers are kept with `library`, which is never unloaded.
        unsafe {
            Some(Self {
                init: get(&library, b"nvmlInit_v2\0")?,
                shutdown: get(&library, b"nvmlShutdown\0")?,
                by_bus_id: get(&library, b"nvmlDeviceGetHandleByPciBusId_v2\0")?,
                utilization: get(&library, b"nvmlDeviceGetUtilizationRates\0"),
                temperature: get(&library, b"nvmlDeviceGetTemperature\0"),
                power: get(&library, b"nvmlDeviceGetPowerUsage\0"),
                power_limit: get(&library, b"nvmlDeviceGetEnforcedPowerLimit\0"),
                memory: get(&library, b"nvmlDeviceGetMemoryInfo\0"),
                memory_v2: get(&library, b"nvmlDeviceGetMemoryInfo_v2\0"),
                name: get(&library, b"nvmlDeviceGetName\0"),
                uuid: get(&library, b"nvmlDeviceGetUUID\0"),
                _keep: library,
            })
        }
    }

    /// The card's stats; NVML must be initialised.
    ///
    /// # Safety
    /// Between a successful `nvmlInit_v2` and its `nvmlShutdown`.
    unsafe fn query(&self, bus_id: &CStr) -> Option<Raw> {
        let mut device: Device = std::ptr::null_mut();
        // SAFETY: NVML is initialised; every out-pointer is to a live local of the type
        // `nvml.h` gives, and text buffers are passed with their size.
        unsafe {
            if (self.by_bus_id)(bus_id.as_ptr(), &raw mut device) != SUCCESS {
                return None;
            }
            let text = |f: Option<GetText>| {
                let f = f?;
                let mut buf = [0 as c_char; TEXT_BUFFER];
                #[expect(clippy::cast_possible_truncation, reason = "96")]
                let ok = f(device, buf.as_mut_ptr(), TEXT_BUFFER as c_uint) == SUCCESS;
                ok.then(|| CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
            };
            let uint = |f: Option<GetUint>| {
                let mut value: c_uint = 0;
                (f?(device, &raw mut value) == SUCCESS).then_some(value)
            };
            let util = self.utilization.and_then(|f| {
                let mut u = Utilization::default();
                (f(device, &raw mut u) == SUCCESS).then_some(u.gpu)
            });
            let celsius = self.temperature.and_then(|f| {
                let mut t: c_uint = 0;
                (f(device, TEMPERATURE_GPU, &raw mut t) == SUCCESS).then_some(t)
            });
            // v2 counts the driver's reservation apart from what is used (S1).
            let memory = self
                .memory_v2
                .and_then(|f| {
                    let mut m = MemoryV2 {
                        version: MEMORY_V2,
                        ..MemoryV2::default()
                    };
                    (f(device, &raw mut m) == SUCCESS).then_some((m.used, m.total))
                })
                .or_else(|| {
                    let f = self.memory?;
                    let mut m = Memory::default();
                    (f(device, &raw mut m) == SUCCESS).then_some((m.used, m.total))
                });
            Some(Raw {
                uuid: text(self.uuid),
                name: text(self.name),
                util,
                celsius,
                power_mw: uint(self.power),
                limit_mw: uint(self.power_limit),
                vram_used: memory.map(|m| m.0),
                vram_total: memory.map(|m| m.1),
            })
        }
    }
}

impl Nvml for Library {
    fn session(&self, bus_ids: &[String]) -> Option<Vec<Option<Raw>>> {
        let bus_ids: Vec<_> = bus_ids
            .iter()
            .map(|b| CString::new(b.as_str()).ok())
            .collect();
        // SAFETY: init and shutdown are paired, and `query` runs between them.
        unsafe {
            if (self.init)() != SUCCESS {
                return None;
            }
            let raws = bus_ids
                .iter()
                .map(|b| b.as_deref().and_then(|b| self.query(b)))
                .collect();
            (self.shutdown)();
            Some(raws)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::Mutex;

    use super::*;

    /// A fresh directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("sysmon-nvidia-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A host with these cards and power states, under `dir`.
    fn host(dir: &TempDir, cards: &[(&str, &str)]) -> (PathBuf, PathBuf) {
        let procfs = dir.0.join("proc");
        let sysfs = dir.0.join("sys");
        std::fs::create_dir_all(&procfs).unwrap();
        for (bus, status) in cards {
            std::fs::create_dir_all(procfs.join(bus)).unwrap();
            let power = sysfs.join(bus).join("power");
            std::fs::create_dir_all(&power).unwrap();
            std::fs::write(power.join("runtime_status"), format!("{status}\n")).unwrap();
        }
        (procfs, sysfs)
    }

    thread_local! {
        /// The fake NVML's sessions, each with the bus ids it was asked about, in order.
        static SESSIONS: RefCell<Vec<Vec<String>>> = const { RefCell::new(Vec::new()) };
    }
    /// Loads of the fake NVML.
    static LOADS: Mutex<u32> = Mutex::new(0);

    #[derive(Debug)]
    struct Fake;

    impl Nvml for Fake {
        fn session(&self, bus_ids: &[String]) -> Option<Vec<Option<Raw>>> {
            SESSIONS.with(|s| s.borrow_mut().push(bus_ids.to_vec()));
            let raw = Raw {
                util: Some(40),
                power_mw: Some(590_000),
                limit_mw: Some(80_000),
                ..Raw::default()
            };
            Some(vec![Some(raw); bus_ids.len()])
        }
    }

    #[expect(clippy::unnecessary_wraps, reason = "a `Loader`")]
    fn fake() -> Option<Box<dyn Nvml>> {
        *LOADS.lock().unwrap() += 1;
        Some(Box::new(Fake))
    }

    fn missing() -> Option<Box<dyn Nvml>> {
        None
    }

    /// The fake NVML's sessions since the last call: one `nvmlInit_v2` each.
    fn sessions() -> Vec<Vec<String>> {
        SESSIONS.with(|s| s.borrow_mut().drain(..).collect())
    }

    #[test]
    fn awake_cards_are_sampled_in_one_session() {
        let dir = TempDir::new("awake");
        let (procfs, sysfs) = host(
            &dir,
            &[("0000:02:00.0", "active"), ("0000:04:00.0", "unsupported")],
        );
        let mut gpus = Gpus::with(&procfs, &sysfs, fake, false);
        let sampled: Vec<_> = gpus
            .answer(true)
            .into_iter()
            .map(|g| matches!(g.state, NvidiaState::Sampled(_)))
            .collect();
        assert_eq!(sampled, [true, true]);
        assert_eq!(sessions(), [["0000:02:00.0", "0000:04:00.0"]]);
    }

    #[test]
    fn any_card_not_awake_keeps_nvml_from_starting() {
        for status in ["suspended", "suspending", "resuming", "error"] {
            let dir = TempDir::new(status);
            let (procfs, sysfs) = host(
                &dir,
                &[("0000:01:00.0", "active"), ("0000:02:00.0", status)],
            );
            let states: Vec<_> = Gpus::with(&procfs, &sysfs, fake, false)
                .answer(true)
                .into_iter()
                .map(|g| g.state)
                .collect();
            assert_eq!(
                states,
                [NvidiaState::Skipped, NvidiaState::Skipped],
                "{status}"
            );
            assert_eq!(sessions(), Vec::<Vec<String>>::new(), "{status}");
        }
    }

    #[test]
    fn one_suspended_card_keeps_nvml_from_starting_for_any() {
        let dir = TempDir::new("one-suspended");
        let (procfs, sysfs) = host(
            &dir,
            &[("0000:01:00.0", "active"), ("0000:02:00.0", "suspended")],
        );
        let states: Vec<_> = Gpus::with(&procfs, &sysfs, fake, false)
            .answer(true)
            .into_iter()
            .map(|g| g.state)
            .collect();
        // `nvmlInit_v2` isn't per card: it would wake the suspended one too (S1).
        assert_eq!(sessions(), Vec::<Vec<String>>::new());
        assert_eq!(states, [NvidiaState::Skipped, NvidiaState::Skipped]);
    }

    #[test]
    fn without_nvml_nothing_is_touched_or_loaded() {
        let dir = TempDir::new("no-nvml");
        let (procfs, sysfs) = host(&dir, &[("0000:01:00.0", "active")]);
        let loads = *LOADS.lock().unwrap();
        let mut gpus = Gpus::with(&procfs, &sysfs, fake, false);
        let reply = gpus.answer(false);
        assert_eq!(reply.len(), 1);
        assert_eq!(reply[0].state, NvidiaState::Skipped);
        assert_eq!(sessions(), Vec::<Vec<String>>::new());
        assert!(matches!(gpus.nvml, Lazy::Untried(_)));
        // Loaded once, on the first request that wants it.
        gpus.answer(true);
        gpus.answer(true);
        assert!(*LOADS.lock().unwrap() > loads);
        assert!(matches!(gpus.nvml, Lazy::Loaded(Some(_))));
    }

    #[test]
    fn a_missing_library_is_no_data_not_an_error() {
        let dir = TempDir::new("missing");
        let (procfs, sysfs) = host(
            &dir,
            &[("0000:01:00.0", "active"), ("0000:02:00.0", "unsupported")],
        );
        let states: Vec<_> = Gpus::with(&procfs, &sysfs, missing, false)
            .answer(true)
            .into_iter()
            .map(|g| g.state)
            .collect();
        assert_eq!(states, [NvidiaState::Unavailable, NvidiaState::Unavailable]);
    }

    #[test]
    fn no_driver_no_gpus() {
        let dir = TempDir::new("none");
        let mut gpus = Gpus::with(&dir.0.join("nope"), &dir.0, fake, false);
        assert_eq!(gpus.answer(true), []);
        assert!(matches!(gpus.nvml, Lazy::Untried(_)));
    }

    #[test]
    fn only_pci_addresses_are_used_as_paths() {
        assert!(is_bus_id("0000:01:00.0"));
        assert!(is_bus_id("10000:e1:00.7"));
        for bad in [
            "..",
            "0000:01:00",
            "0000:01:00.0/../..",
            "../01:00.0",
            "0000:01:00.00",
            "0000:1:00.0",
            "zzzz:01:00.0",
            "0000:01:00.0:1",
            "",
        ] {
            assert!(!is_bus_id(bad), "{bad}");
        }
        let dir = TempDir::new("paths");
        let (procfs, sysfs) = host(&dir, &[("0000:01:00.0", "active")]);
        std::fs::create_dir_all(procfs.join("not-a-gpu")).unwrap();
        let ids: Vec<_> = Gpus::with(&procfs, &sysfs, fake, false)
            .answer(false)
            .into_iter()
            .map(|g| g.bus_id)
            .collect();
        assert_eq!(ids, ["0000:01:00.0"]);
    }

    #[test]
    fn implausible_readings_are_dropped() {
        // The first power reading after a wake, above the card's limit (S1).
        let sample = plausible(Raw {
            util: Some(40),
            celsius: Some(52),
            power_mw: Some(590_000),
            limit_mw: Some(80_000),
            vram_used: Some(9 << 30),
            vram_total: Some(8188 << 20),
            ..Raw::default()
        });
        assert_eq!(
            (sample.util, sample.celsius, sample.watts, sample.vram_used),
            (Some(40), Some(52.0), None, None)
        );
        assert_eq!(sample.vram_total, Some(8188 << 20));

        let sample = plausible(Raw {
            util: Some(101),
            celsius: Some(400),
            power_mw: Some(31_500),
            limit_mw: Some(80_000),
            vram_used: Some(512 << 20),
            vram_total: Some(8188 << 20),
            ..Raw::default()
        });
        assert_eq!(
            (sample.util, sample.celsius, sample.watts, sample.vram_used),
            (None, None, Some(31.5), Some(512 << 20))
        );

        // Without a limit, 1 kW.
        let watts = |mw| {
            plausible(Raw {
                power_mw: Some(mw),
                ..Raw::default()
            })
            .watts
        };
        assert_eq!(watts(999_000), Some(999.0));
        assert_eq!(watts(1_000_001), None);
        // At the limit is fine.
        let at_limit = plausible(Raw {
            power_mw: Some(80_000),
            limit_mw: Some(80_000),
            ..Raw::default()
        });
        assert_eq!(at_limit.watts, Some(80.0));
    }

    #[test]
    fn memory_v2_has_the_header_s_version() {
        assert_eq!(std::mem::size_of::<MemoryV2>(), 40);
        assert_eq!(MEMORY_V2, 0x0200_0028);
    }
}
