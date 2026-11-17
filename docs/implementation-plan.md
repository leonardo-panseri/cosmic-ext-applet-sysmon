# Implementation plan: system monitor applet for COSMIC

**Goal.** A COSMIC panel applet that shows CPU, memory, GPU, disk and network at a glance, with warnings,
2-minute history graphs, top processes and a shortcut to a full process monitor. Its features and UI/UX
follow the Omarchy plugin [navjottomer.sysmon](https://github.com/navjottomer/omarchy-sysmon) (MIT, commit
`bce68d4`). We copy its design, not its code. It ships as a Flatpak in the
[COSMIC Flatpak repository](https://github.com/pop-os/cosmic-flatpak), licensed MPL-2.0 like
`cosmic-ext-applet-codexbar`.

**The constraint that shapes everything:** it must cost close to nothing while the popup is closed, and it
must never wake a runtime-suspended discrete GPU.

**Reference hardware** (the developer's laptop): 16 threads, AMD iGPU (`card1`, `boot_vga=1`, `amdgpu`),
NVIDIA RTX 4060 Laptop dGPU (`card0`, `nvidia` 620.70.06, runtime D3 fine-grained, normally `suspended`).

---

## 1. Decisions

### D1 — Flatpak, with a long-lived host probe for what the sandbox can't see

Measured inside a Flatpak sandbox on the reference laptop (`devices=dri`, no network share):

| Source | In the sandbox |
|---|---|
| `/proc/stat`, `/proc/meminfo`, `/proc/loadavg`, `/proc/uptime`, `/proc/diskstats` | Host values (not namespaced) |
| `/sys/{block,bus,class,dev,devices}` | Visible: DRM cards, `power/runtime_status`, `gpu_busy_percent`, all hwmon chips (k10temp, amdgpu, nvme, BAT1…) |
| `/proc/driver/nvidia/gpus/*/{information,power}` | Visible, and reading it does not wake the GPU (`runtime_suspended_time` kept growing) |
| `/proc/net/dev` | Only `lo`; host interfaces need `--share=network` |
| `/proc/<pid>` | 4 PIDs (sandbox) vs 482 on the host: Flatpak always unshares the PID namespace |
| `/proc/self/mounts` | Describes the sandbox, not the host |

So the applet reads everything it can directly. A small binary, the **probe**, does what the sandbox
can't (§3.2):

- the process table
- host mounts and their `statvfs`
- NVIDIA stats through NVML, using the host's own `libnvidia-ml`, so we don't depend on the
  `org.freedesktop.Platform.GL.nvidia-*` extension matching the driver

The probe is started **once** with `flatpak-spawn --host` and talks over stdin/stdout. That needs
`--talk-name=org.freedesktop.Flatpak`, which 34 of the 69 apps in cosmic-flatpak already use, including
`minimon-applet` and `sysinfo-applet`.

**Rejected:** one `flatpak-spawn --host` per poll. `cosmic-ext-mini-taskmanager` does this (`src/process/host.rs`):
a D-Bus round trip plus a host shell reading every `/proc/<pid>/stat`, on every tick.

### D2 — Two sampling tiers

| | **Closed** (popup closed) | **Open** (popup open) |
|---|---|---|
| Interval | 2 s (4 s on battery) | 2 s |
| CPU total + per core, memory, load, uptime | ✓ (one read each) | ✓ |
| Net and disk I/O counters | ✓ (feed history) | ✓ |
| hwmon temps (CPU, AMD GPU) | ✓ (warnings need them) | ✓ |
| NVMe temps (each read is a SMART command) | every 30 s | every 30 s |
| AMD iGPU sysfs | ✓ | ✓ |
| dGPU `runtime_status` | ✓ (one small read) | ✓ |
| dGPU NVML (only when `active`; every 15 s after 3 idle samples, ADR 0003) | only if the panel button shows `gpu` | ✓ |
| Top processes | – | ✓ (probe) |
| Mounts + space | every 30 s (probe) | every 30 s (probe) |

History (60 samples = 2 min) is kept in both tiers, for CPU, each GPU and network, so the graphs are full as soon
as the popup opens. That's the same idea as the Omarchy plugin, which measures about 60 ms of CPU per
minute while closed. We aim for less.

On battery the closed-tier interval doubles. The battery state comes from UPower's `OnBattery` property, using signals
rather than polling (`--system-talk-name=org.freedesktop.UPower`). Both intervals are internal constants
until someone asks for a setting.

### D3 — Never wake a suspended GPU

Before touching any GPU beyond sysfs/procfs metadata, read `device/power/runtime_status`. If it's `suspended`,
show the card as suspended and skip it. Spike S1 found that every NVML call (including `nvmlInit` and
`nvmlShutdown`) wakes a suspended card, and that calling NVML every 5 s or more often keeps an idle card
awake, while merely holding it open does not. So the probe does init → query → shutdown per sample, only
when the card is `active`, and backs off to one sample every 15 s once the card's load has been 0 % for 3
samples ([ADR 0003](adr/0003-nvml-per-sample-with-idle-backoff.md)).

**Why this matters:** Kai-J-G/cosmic-ext-hardware-monitor, the closest existing applet, picks "discrete
first" at startup. On an NVIDIA machine it then runs `nvidia-smi` on every 2 s tick, popup open or not, and it does so
synchronously inside `update()` on the UI thread (`src/app.rs`, `Message::Tick => self.refresh()`). That
keeps the dGPU from ever suspending and blocks the UI while `nvidia-smi` runs. This is a likely cause of the
instability seen with it on the reference laptop. Not confirmed, but enough to avoid all three patterns.

### D4 — Show VRAM total while suspended, from a cache

The VRAM size can't be read without waking the card. `/proc/driver/nvidia/gpus/*/information` has the model
and UUID but not the memory size. PCI BAR1 is 8192 MiB here, but only because Resizable BAR is on. Without
ReBAR it's 256 MiB, and ReBAR sizes are powers of two, so a 12 GB card would show 16 GB. Rejected as a source.

Instead, the first time the card is seen awake, store `uuid → vram_total` in cosmic-config state. (In a Flatpak that's the app's own `~/.var/app/<id>/.local/state`, since `--filesystem` has no `xdg-state` location; no extra permission needed.) While suspended,
the GPU card shows:

```
▭ RTX 4060 Laptop GPU          Suspended
⛁ VRAM                  0 / 8G        0%
```

and only `Suspended` until the card has been seen awake once. The model name also comes from procfs, so no
`pci.ids` lookup is needed for NVIDIA.

### D5 — No cosmic-monitor code; launch it instead

cosmic-monitor is GPL-3.0-only and binary-only (no library, no D-Bus). We write our own collectors (MPL-2.0).
The popup's **Process monitor** button, and right-click on the panel button, launch `com.system76.CosmicMonitor`
on the host through `flatpak-spawn --host`. A setting to choose another app can come later.

### D6 — Redraw only when what's on screen changes

Every libcosmic message re-renders the view, and a panel redraw also costs the compositor. Therefore:

- The sampler runs on its own thread, never in `update()`.
- It sends the **closed-tier** state to the UI only when a shown value moved enough from the value last sent
  (2 points for a percentage, 1 °C, a 1.5× ratio for a network rate, held while both rates are below 16 KiB/s),
  when the warnings change or a value appears or disappears, or after 30 s if a held value drifted.
  History is appended on the sampler side and sent with the next update, or when the popup opens.
- The panel button gets a **fixed width** per layout, using tabular/monospace digits, so changing values
  never re-lay out the whole panel.
- While open, one message per tick redraws the popup.

### D7 — Count only physical devices

- **Network:** count only interfaces that have a `/sys/class/net/<if>/device` link. That skips lo, veth, bridges,
  docker, VPN/WireGuard and tun, so traffic isn't counted twice (the Omarchy plugin has the same rule).
  Show each interface separately when more than one is up.
- **Disk I/O:** count only whole disks (`/sys/block/*` with a `device` link), excluding loop, ram, zram and dm-*.
- **Mounts:** real block-device filesystems only, deduplicated by device, at most 16.

### D8 — Bounded everything

Lists have limits like the Omarchy plugin's: 16 mounts, 16 interfaces, 8 GPUs, 8 drives, top 5 processes per list,
and names clipped to 64 chars. Probe replies are capped in size. All system-supplied text is plain text.

---

## 2. UI

### 2.1 Panel button

Same three layouts as the reference:

- **Stacked** (default): two small rows in one panel slot. Each row is an icon and a value (CPU, RAM, GPU, Disk, CPU °C),
  with ↓/↑ network rates beside them if enabled.
- **One line:** both values and the rates on one line.
- **Compact:** only the top value.

On a vertical panel, Stacked and One line become Compact. **Warnings:** a row turns the theme's destructive
colour when its own value crosses a limit. If the problem is something not shown on the panel button, the whole button
turns that colour, and the tooltip says what's wrong. Left click toggles the popup. Right click opens the process monitor.

Warn when:
- CPU or GPU temperature ≥ `warn_temp` (85 °C)
- memory or any filesystem ≥ `warn_usage` (90 %)
- an NVMe drive ≥ its own rated limit (hwmon `temp*_crit`/`temp*_max`)

### 2.2 Popup (stats page)

Sections in the reference's order, inside a scrollable area (COSMIC popups have a maximum height):

1. **CPU header:** big total %, uptime, thread count, load average, package temp; 2-min graph;
   per-core bars (red above 85 %).
2. **Memory:** in use and swap: used / total, %.
3. **GPU:** one card per GPU (NVIDIA first). Each shows name, load · temp · power, a 2-min graph, and VRAM used / total.
   Suspended cards as in D4. The integrated GPU's "VRAM" is `mem_info_vram_total` (the carve-out).
4. **Network:** download/upload, a 2-min graph (download above the line, upload below), per interface if more than one.
5. **Disk:** read/write rates, space per filesystem, NVMe temperatures (model from `/sys/class/nvme/*/model`).
6. **Processes:** top 5 by CPU and top 5 by memory, side by side.
7. **Footer:** **Process monitor** and **Settings** buttons.

Graphs are our own small `canvas::Program`: an area fill in the accent colour on `bg_component`. Use COSMIC
symbolic icons, not Nerd Font glyphs.

### 2.3 Popup (settings page)

Inside the popup, like the reference and like codexbar's panel page:

- Layout: Stacked / One line / Compact
- Top row and bottom row: CPU / RAM / GPU / Disk / CPU °C
- Network speed: toggle
- Warn at or above: temperature (°C) and memory or disk (% full)
- Back

Changes save immediately to cosmic-config and the panel updates live. Defaults: `stacked`, top `cpu`, bottom `memory`,
network on, 85 °C, 90 %.

---

## 3. Architecture

### 3.1 Workspace

```
crates/
  sysmon-proto/   # serde types for probe requests/replies, protocol version
  sysmon-probe/   # host probe: dynamic glibc binary (ADR 0002), no libcosmic; lib reused in-process
src/              # the applet
  sampler/        # thread, tiers, rates, history
  sources/        # pure parsers: proc_stat, meminfo, net_dev, diskstats, hwmon, drm, nvidia_procfs
  probe_client/   # spawn, handshake, request/reply; in-process backend for non-Flatpak runs
  ui/             # panel button, popup pages, graph canvas
```

Parsers are pure functions over `&str`/`&[u8]`, tested against fixture files captured from real machines.
File handles stay open and are reread with `pread` at offset 0, so there is no `open()` per tick.

### 3.2 Probe

- **Launch:** the host path is `app-path` from `/.flatpak-info` plus `/bin/sysmon-probe`. Run
  `flatpak-spawn --host --watch-bus <path>`. `--watch-bus` kills it if the applet dies.
- **Lifecycle:** started on first need and kept until the applet exits. A process blocked on stdin causes
  no wakeups and costs only a few MB of RSS, so there's no reason to keep restarting it. It exits on EOF.
- **Protocol:** JSON lines with `{"v":1,...}` and a version handshake on start, so a probe from an older
  installation is caught. Requests are a closed enum: `Processes{top}`, `Mounts`, `Gpus{nvml:bool}`,
  `Release` (drop NVML handles). It accepts no paths, commands or arguments from the applet. It's an
  unsandboxed process, so its whole interface is read-only by construction.
- **Processes:** reads `/proc/<pid>/stat` and `statm` and computes CPU % from the difference against its previous
  scan. Only runs when asked, which is only while the popup is open.
- **Build:** an ordinary dynamically linked `x86_64-unknown-linux-gnu` (and aarch64) binary from the SDK. It
  runs with the host's loader and glibc, needs glibc ≥ 2.34 on the host (checked at build time), and can
  `dlopen` the host's `libnvidia-ml`. If it can't start, the applet uses sandbox-only data
  ([ADR 0002](adr/0002-dynamic-glibc-probe.md), spike S2).
- **Non-Flatpak** (development, distro packages): the same `Probe` trait has an in-process backend, so
  `cargo run` needs no probe process.

### 3.3 Flatpak finish-args

```
--socket=wayland
--share=ipc
--share=network                              # host netns for /proc/net/dev
--device=dri
--filesystem=xdg-config/cosmic:rw
--talk-name=com.system76.CosmicSettingsDaemon
--talk-name=org.freedesktop.Flatpak          # the probe; launching the process monitor
--system-talk-name=org.freedesktop.UPower    # on battery → slower closed tier
```

No `--device=all` and no `--filesystem=/sys/...`: the default `/sys` exposure was enough in the test.

---

## 4. Spikes (do before the phase that needs them)

- **S1 — NVML and runtime PM** (before P4). On the reference laptop: does `nvmlInit` wake a suspended card? Does
  holding the handle stop it from suspending? How long do `nvmlInit`/`nvmlShutdown` take? Pick one of:
  (a) init once while awake, shut down on popup close or on `Release`;
  (b) init/shutdown per sample;
  (c) skip NVML and read DRM fdinfo (`drm-engine-*`, `drm-memory-*`) from the probe's process scan, if
  the 620 open kernel module exposes it.
  Record the result as an ADR.
  **Result:** every NVML call wakes a suspended card; open handles don't pin it, but calls every ≤ 5 s do;
  no fdinfo on this driver. (b) with an idle back-off ([ADR 0003](adr/0003-nvml-per-sample-with-idle-backoff.md)).
- **S2 — static probe in flatpak-builder** (before P2). Is the musl target available in
  `org.freedesktop.Sdk.Extension.rust-stable`? If not, find the least bad alternative (e.g. build against an old glibc
  baseline).
  **Result:** no musl target, and static musl can't `dlopen` libnvidia-ml anyway. Dynamic glibc
  ([ADR 0002](adr/0002-dynamic-glibc-probe.md)).
- **S3 — `app-path` for user and system installs** (before P2). Check that the host can run the probe for
  `--user` and `--system` installs, and from `flatpak-builder --run` during development.
  **Result:** works for `--user` (`app-path` is the commit's own `files` dir, stable for the instance);
  `--system` checkouts are world-readable by the same evidence; `--watch-bus` does kill the host process.
  `flatpak-spawn` fails under `flatpak-builder --run`, so develop against an installed build (ADR 0002).
- **S4 — Panel right click and fixed width** (before P1 is done). Check that libcosmic's applet button can
  handle a right-click and a fixed-width label without breaking the panel's padding rules.

---

## 5. Phases

Each phase ends with something runnable and with its checks passing.

### P0 — Scaffold
- Copy codexbar's skeleton: justfile, `build.rs`/xdgen, i18n, metainfo, Flatpak manifest, rustfmt, AGENTS.md and
  `docs/agents/`. App id `io.github.leonardo_panseri.cosmic-ext-applet-sysmon`, MPL-2.0.
- `CONTEXT.md` with the glossary (§6). ADR 0001: host probe over a per-poll host spawn (D1).
- **Done when:** the applet shows static text in the panel, from `just run` and from a local Flatpak build.

### P1 — Sandbox sampler and panel button
- Parsers and fixtures for `stat`, `meminfo`, `loadavg`, `uptime`, `net/dev`, `diskstats`, hwmon, DRM (AMD), `runtime_status`,
  nvidia procfs.
- Sampler thread with tiers, rates from the measured monotonic elapsed time, history, change-only updates (D6).
- Panel button: the three layouts, configurable rows hard-coded for now, network rates, warning colours.
- **Done when:** with the panel visible for 10 min and the popup closed:
  - `perf stat -p` shows under 0.1 % CPU
  - `powertop` shows about 0.5 wakeups/s from the applet
  - `card0` stays `suspended` the whole time

### P2 — Probe
- `sysmon-proto` and `sysmon-probe` crates, handshake, `Processes` and `Mounts`, in-process backend.
- Spikes S2 and S3 first.
- **Done when:** in the Flatpak, the applet gets the host's top 5 processes and real mounts. Killing the applet kills the probe.
  The probe makes no wakeups while idle.

### P3 — Popup stats page
- All sections from §2.2 except NVIDIA details; scrolling; the graph canvas; Process monitor launch (D5).
- **Done when:** it matches the reference screenshot's layout and content on the reference laptop with the dGPU suspended.

### P4 — GPUs
- Spike S1, then NVIDIA via the probe, gated on `runtime_status`, with the idle back-off of ADR 0003. VRAM
  cache (D4). Multiple GPUs, NVIDIA first. `gpu` on the panel button means "first awake GPU, NVIDIA first".
- **Done when:**
  - running a GPU load with `prime-run` shows live NVIDIA stats
  - after the load ends, the card suspends again with the popup **open**: the back-off kicks in after 3 idle
    samples, then the card's own delay (S1 measured 20 s minimum awake, then 5–12 s after the last activity)
  - while suspended, it shows `0 / 8G`

### P5 — Settings page and warnings
- §2.3 page, cosmic-config schema, live panel update, tooltip with the active warnings, NVMe rated limits.
- **Done when:** every setting works live and survives a restart.

### P6 — Power check and release
- Repeat the P1 measurements with the popup closed, then measure with the popup open, both on battery. Write the
  numbers in the README.
- Metainfo screenshots (`just screenshots` as in codexbar), submission to cosmic-flatpak.

---

## 6. Glossary (seed for CONTEXT.md)

- **Panel button:** what sits in the panel: one to two rows of values, with optional network rates.
- **Popup:** opened from the panel button; has a **stats page** and a **settings page**.
- **Closed tier / open tier:** the two sampling sets (D2), selected by whether the popup is open.
- **Sample:** one tick's raw numbers. **History:** the last 60 samples kept for graphs.
- **Probe:** the host binary started with `flatpak-spawn --host` (D1, §3.2).
- **Suspended:** a GPU whose `runtime_status` is `suspended`; never queried beyond procfs/sysfs metadata.
- **Warning:** a value at or above its limit; colours a row or the whole panel button.

## 7. Out of scope for v1

Intel GPU load (no simple sysfs counter; fdinfo-based support could follow S1), keyboard navigation in
the popup, choosing the process monitor app, per-process GPU usage, battery/power draw display.
