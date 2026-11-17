# System monitor applet for COSMIC

A COSMIC panel applet that shows CPU, memory, GPU, disk and network use at a glance, and costs close to
nothing while nobody is looking at it. See `docs/implementation-plan.md` and `docs/adr/`.

## Language

### What the user sees

**Panel button**:
What sits in the panel: one or two rows of values, with optional network rates.
_Avoid_: bar, widget, indicator

**Popup**:
The window opened from the panel button. It has a **stats page** and a **settings page**.
_Avoid_: menu, dropdown, panel

**Warning**:
A value at or above its limit. It colours its own row, or the whole panel button when the value isn't shown there.
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

**History**:
The last 60 samples, kept for the graphs.
_Avoid_: buffer, log

**Probe**:
The helper started once on the host with `flatpak-spawn --host`, for what the sandbox can't see.
_Avoid_: daemon, helper, agent

**Suspended**:
Said of a GPU whose runtime power state is `suspended`. It is never queried beyond metadata.
_Avoid_: asleep, off, idle
