---
status: accepted
---

# Keep the tiny-skia renderer

With the popup open, the applet used 5.47 % of one core in the real panel (stats page, 2 s ticks, no
input). A diagnosis found one update and one view per tick. libcosmic's autosize path produces 2 frames
per tick, and each frame costs 30–45 ms of UI-thread CPU. Nearly all of that is in `iced_tiny_skia`'s
present: for every damage region it clears and refills a full-window clip mask and redraws large quads.
Building the view, laying it out and drawing it take under 1 ms.

A spike compared libcosmic's `wgpu` renderer in the debug preview window (popup open, release build,
120 s, no input):

| | tiny-skia | wgpu (radv, AMD iGPU) |
|---|---|---|
| UI-thread CPU per tick, mean | 21.4 ms | 3.7 ms |
| Process CPU | 1.78 % | 0.86 % |
| Wakeups | 12.0/s | 12.0/s |
| RSS, host | 38 MB | 77 MB |
| RSS, Flatpak | 38 MB | 89 MB |

wgpu would also need our own guard against the NVIDIA card. iced sets `VK_LOADER_DRIVERS_DISABLE=nvidia*`
when the compositor's device isn't NVIDIA, but that misses Mesa's NVK in the Flatpak runtime, which
opened the NVIDIA render node (it didn't wake the card with driver 620.70). The spike's guard restricted
Vulkan to the iGPU's driver when an iGPU exists, and kept the card `suspended` throughout.

## Decision

Keep tiny-skia. The popup is open briefly and rarely, while wgpu's extra 40–51 MB of RSS would last for
the applet's whole life, popup open or not, and wgpu adds a GPU guard to maintain. With the popup closed
the renderer doesn't matter: hysteresis keeps panel redraws rare.

The `spike/wgpu` branch keeps the opt-in code (a `wgpu` feature and the `keep_wgpu_off_nvidia` guard).

## Consequences

- An open popup costs a few percent of one core. Most of it is in libcosmic: the duplicate autosize frame
  and tiny-skia's clip-mask refill per damage region. Both are worth reporting upstream, and fixing them
  would help every applet.
- If libcosmic's software renderer gets faster, this applet gets the benefit without changes.
