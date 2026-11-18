# Spec: a configurable panel button

Replaces the panel button's fixed rows (plan §2.1, the Layout / Top row / Bottom row controls of §2.3).
Agreed with the user on 2026-11-18.

## Model

- **Two rows of items.** Each row holds 0–4 items, in order. An item is one value with its icon.
- **Two layouts:** **Stacked** shows the rows one above the other. **One line** joins them into one line,
  the top row first, with a thin separator between the rows. Switching layouts keeps the rows. Compact is
  removed.
- **Items** (each an icon and a fixed-width value, D6):

  | Item | Value | Parameter |
  |---|---|---|
  | CPU usage | % | – |
  | CPU temperature | °C | – |
  | RAM | % | – |
  | Swap | % | – |
  | GPU usage | % | GPU: Auto (first awake, NVIDIA first) or a specific GPU |
  | GPU temperature | °C | GPU, as above |
  | VRAM | % | GPU, as above |
  | Disk usage | % | mount point (default `/`) |
  | Disk read | rate | disk (default: all) |
  | Disk write | rate | disk (default: all) |
  | NVMe temperature | °C | drive |
  | Download | rate | interface (default: all physical ones) |
  | Upload | rate | interface (default: all physical ones) |

  A specific GPU is identified by something stable: the PCI address, or for NVIDIA the procfs bus id
  (never anything that needs a query to resolve). A disk or drive is identified by its model plus device
  name, and an interface by its name. An item whose target is missing shows "–".
- **Defaults** match today's look: the top row holds CPU usage and Download; the bottom row holds RAM and
  Upload. A user's old settings (`layout`, `top`, `bottom`, `network`) migrate to rows:
  - top → `[top item, Download if network]`
  - bottom → `[bottom item, Upload if network]`
  - Compact → Stacked with the same rows
- **Warnings:** each item owns the warnings about its own value, for example GPU temperature for its GPU,
  Disk usage for its mount, NVMe temperature for its drive, and RAM for memory. A warning that no shown
  item owns colours the whole button, as today. The tooltip lists every active warning.
- **Sampling:**
  - The closed tier reads what the items need. A GPU item counts as "GPU on the panel button" for NVML
    (D2, ADR 0003): only while that card is awake, with the back-off.
  - A Disk read/write item for one disk needs that disk's counters, which are already sampled.
  - Hysteresis (D6) applies per item, using the item kind's rule.

## Vertical panels

- **One line:** a single column, the items one below the other. Each item shows its icon above its value,
  centred, at a fixed height, with the value sized to the panel's width ("1.4M" is the widest value).
- **Stacked:** two columns, **the top row on the left** and the bottom row on the right. This applies only
  when both columns fit at a readable font size (about 9 px or more). Otherwise it falls back to the
  single column, top row first. The user's rows never change; only the arrangement does.

## The builder (settings page)

The settings page gets a "Panel button" entry that opens a sub-page:

```
┌ Panel button ─────────────────────────────────┐
│  [ live preview of the button, real values ]  │
│  Layout   ( Stacked | One line )              │
│                                               │
│  TOP ROW                                      │
│  [▣ CPU 12% ✕] [🌡 CPU 54°C ✕] [↓ 1.2M ✕]   [+]│
│  BOTTOM ROW                                   │
│  [▤ RAM 37% ✕] [▭ RTX 4060 33% ✕]           [+]│
│                                               │
│  Selected: CPU °C   ◀ Move  Move ▶  ⇅ Row      │
└───────────────────────────────────────────────┘
```

- **Live preview:** the top shows the real panel button widget with live values, in the chosen layout.
- **Chips:** each row is a wrapping row of chips. A chip shows the item's icon, short name and live value.
  - Clicking a chip selects it (highlighted).
  - The bar below the rows then offers **Move left**, **Move right**, **Move to the other row** and
    **Remove**, and shows the item's parameter picker if it has one (GPU, mount, disk, drive or interface).
  - ✕ on a chip removes it directly.
  - There is no drag and drop.
- **Adding:** **+** at the end of a row opens a picker list grouped under CPU, Memory, GPU, Disk and
  Network.
  - Each entry shows its icon, name and current value.
  - Parameterised items list one entry per target, e.g. "GPU usage · RTX 4060", "GPU usage · Radeon 680M"
    and "GPU usage · Auto".
  - Picking an entry appends it to that row.
  - **+** is disabled when the row has 4 items.
- **Saving:** changes save immediately to cosmic-config and apply live, as the other settings do.
- **Back:** returns to the settings page.

## Config

A new key `rows: ([Item; ≤4], [Item; ≤4])`, plus `layout: Stacked | OneLine`. `Item` is a serde enum with
its parameter, for example `GpuTemp(GpuRef::Auto)`, `GpuTemp(GpuRef::Pci("0000:01:00.0"))` or
`DiskUsage("/mnt/storage")`. Migrate from the old keys as described above, and keep reading them until
`rows` is stored.
