# System monitor applet for COSMIC

A COSMIC panel applet that shows CPU, memory, GPU, disk and network use at a glance, and costs close to
nothing while nobody is looking at it. See `docs/implementation-plan.md` and `docs/adr/`.

## Language

### What the user sees

**Panel button**:
What sits in the panel: two **rows** of **items**, Stacked or on One line (`docs/panel-builder.md`).
_Avoid_: bar, widget, indicator

**Item**:
One value on the panel button with its icon, such as CPU usage, a GPU's temperature or a mount's usage.
Some name a target: a GPU (Auto, or one by PCI address), a mount point, a disk, a drive or an interface.
_Avoid_: row (for a single value), cell, widget, metric

**Row**:
One of the panel button's two ordered lists of up to four items: the top row and the bottom row. On a
vertical panel a row may stand as a column; it is still a row.
_Avoid_: line, line of items (a **line** is both rows)

**Line**:
Both rows as one list, the top row first: what the One line layout shows, and what the builder edits in
that layout. An edit there folds the line back into two rows of about the same length.
_Avoid_: row (for both rows together)

**Builder**:
The settings page's sub-page that edits the panel button: the layout, and the rows (or, on One line, the
line) as chips. It has no preview: the panel button itself shows each change.
_Avoid_: editor, designer

**Popup**:
The window opened from the panel button. It has a **stats page** and a **settings page**.
_Avoid_: menu, dropdown, panel

**Popup layout**:
One column or two; **Auto** takes two when one would need scrolling on the panel's screen.
_Avoid_: mode, view, wide popup

**Warning**:
A value at or above its limit. It colours the items that own it (those of its device), or the whole panel
button when no item shown owns it.
_Avoid_: alert, alarm

### How values are gathered

**Closed tier**:
The small set of values sampled while the popup is closed.

**Open tier**:
The full set of values sampled while the popup is open.
_Avoid_: active mode, full mode

**Sample**:
One tick's raw numbers.
_Avoid_: reading, measurement

**Reading**:
One value from one sensor, counter or NVML query, such as a temperature or a power draw; a sample is
made of many. An implausible reading is dropped.
_Avoid_: sample (for a single value)

**History**:
The last 60 samples, kept for the graphs.
_Avoid_: buffer, log

**Probe**:
The small binary started once on the host with `flatpak-spawn --host`, for what the sandbox can't see.
_Avoid_: daemon, helper, agent

**Suspended**:
Said of a GPU whose runtime power state is `suspended`. It is never queried beyond metadata.
_Avoid_: asleep, off, idle

**Idle**:
Said of an awake GPU at 0 % load. Not suspended: it is still queried, but NVML backs off to every 15 s
so the card can suspend (ADR 0003).
_Avoid_: suspended, asleep, off
