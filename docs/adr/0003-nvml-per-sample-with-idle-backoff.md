---
status: accepted
---

# NVML once per sample, gated on `active`, with an idle back-off

Spike S1 measured NVML against runtime power management on the reference laptop (RTX 4060 Laptop,
620.70.06 open kernel module, fine-grained runtime D3, with a test program that `dlopen`s `libnvidia-ml.so.1`):

- **Calls wake the card; open handles don't.** `nvmlInit_v2` on a suspended card wakes it (about 1.85 s).
  So do every per-device call (`GetHandleByIndex`, `GetName`, `GetUUID`, temperature, utilisation,
  power, memory), `nvmlShutdown`, and a process exiting with NVML still initialised. `GetCount` does
  not. With NVML initialised and its `/dev/nvidia*` fds open but no calls, the card suspended normally,
  20 s after it woke. Plan D3's reason ("an open fd stops the driver from suspending the card") was wrong.
- **Frequent calls keep it awake.** After any wake the card stays `active` for at least about 20 s, then
  suspends 5–12 s after the last activity. NVML calls every 2 s or 5 s kept an idle card awake for as long as
  they ran, even when every call was gated on `runtime_status == active`. After a GPU load ended, 2 s gated
  sampling held the card awake 65 s, until the sampling stopped. Calls every 10 s let it suspend.
- **Costs:** on an awake card, init → handle → four queries → shutdown takes 10–16 ms.
- **No fdinfo alternative:** the 620 driver's `/dev/nvidia*` and `renderD*` fdinfo has no `drm-engine-*` or
  `drm-memory-*` keys, so plan option (c) is unavailable.
- The first `GetPowerUsage` after a wake read 590 W three times. `GetMemoryInfo` (v1) reports a
  driver reservation of about 331 MiB as used on an idle card; `GetMemoryInfo_v2` separates it.
  The total is 8188 MiB.

## Decision

Option (b), with a back-off:

- Before **every** NVML sample the probe reads the card's `runtime_status`, and calls nothing unless it is
  `active`. With several NVIDIA cards it reads every one's, since `nvmlInit_v2` isn't per card, and calls
  nothing unless all are awake. Each sample is one init → query each card → shutdown, so nothing is held
  between samples and there's never a shutdown or exit to do while a card is suspended.
- While the card's load is above 0 %, sample at the tier's normal rate (2 s). After 3 samples in a row at
  0 %, drop to one sample every 15 s, so an idle card can suspend. Go back to 2 s as soon as a sample
  shows load. The panel's `gpu` row follows the same rule.
- Drop implausible readings: power above the card's limit (or above 1 kW when the limit is unknown), and
  which catches the bogus first power reading after a wake.
- VRAM used comes from `GetMemoryInfo_v2` when available (otherwise v1). The VRAM total is cached by UUID
  (plan D4) and rounded, not truncated, when formatted (8188 MiB is "8G").

## Considered options

- **(a) Init once while awake, shut down on popup close or `Release`.** Holding NVML is harmless, but the
  shutdown itself wakes the card if it has suspended in the meantime, so closing the popup would often
  wake it.
- **(b) without the back-off.** Keeps an idle card awake indefinitely while the popup is open.
- **(c) DRM fdinfo.** Not exposed by this driver.
- **Stop sampling at idle and resume only after outside activity.** Fewer wakes, but it depends on a
  "still awake 30 s after our last call" heuristic and is slower to show a new load.
- **A fixed 15 s rate.** Never holds the card awake, but gives coarse numbers while it's busy.

## Consequences

- The plan's P4 check ("the card suspends again within its normal delay with the popup open") holds
  with the back-off; expect up to about 20 s at 2 s (to see 3 idle samples), then the card's own delay.
- While the card idles awake, its graph updates only every 15 s.
- A card whose `runtime_status` is `unsupported` (runtime PM off) never suspends, so it counts as awake.
- On 2 s ticks the 15 s idle interval works out to 16 s. A light vsync'd load can read 0 %, so the
  back-off may start while it still runs; that's the rule working as intended.
- The closed tier also polls an awake NVIDIA card, with this back-off, while its GPU temperature or VRAM
  warning is enabled (the temperature one is by default), so those warnings fire with the popup closed.
