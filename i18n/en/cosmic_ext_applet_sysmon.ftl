app-title = System Monitor
app-comment = CPU, memory, GPU, disk and network at a glance in the COSMIC™ panel
app-keywords = COSMIC;System;Monitor;CPU;Memory;GPU;Network;

## Panel button

cpu = CPU
memory = RAM
gpu = GPU
disk = Disk
download = ↓
upload = ↑
no-value = –

## Popup: stats page

placeholder = Statistics will appear here.
# The CPU header: "CPU · 16 threads".
cpu-threads = CPU · { $threads ->
    [one] { $threads } thread
   *[other] { $threads } threads
}
uptime-days = up { $days }d { $hours }h
uptime-hours = up { $hours }h { $minutes }m
uptime-minutes = up { $minutes }m
# Section headers, shown as written.
section-memory = MEMORY
section-gpu = GPU
section-network = NETWORK
section-disk = DISK
section-processes = PROCESSES
in-use = In use
swap = Swap
vram = VRAM
# A GPU's stats, joined: "33% · 51°C · 64W".
stat-separator = {" · "}
# Used of total: "11G / 30G".
used-of-total = { $used } / { $total }
gpu-suspended = Suspended
# A GPU whose model isn't known (not NVIDIA or AMD): "GPU card1".
gpu-unnamed = GPU { $card }
download-label = Download
upload-label = Upload
read = Read
write = Write
# A rate: "1.2M/s".
per-second = { $rate }/s
# The block of filesystems not on any disk shown above.
disk-other = Other
# The tooltip over a value: its lowest and highest since the applet started.
since-start = Since start: min { $min } · max { $max }
processes-cpu = CPU
processes-memory = Memory
no-processes = Not available

## Popup: footer and settings page

process-monitor = Process monitor
settings = Settings
back = Back
# Section headers, shown as written.
setting-layout = LAYOUT
setting-top-row = TOP ROW
setting-bottom-row = BOTTOM ROW
setting-warn = WARN AT OR ABOVE
layout-stacked = Stacked
layout-one-line = One line
layout-compact = Compact
# The CPU temperature, as a choice for a panel row.
row-cpu-temp = CPU °C
setting-network = Network speed
setting-network-description = Download and upload
setting-process-monitor = Process monitor
setting-process-monitor-hint = Desktop ID or command
setting-process-monitor-reset = Reset to default
setting-popup-layout = POPUP LAYOUT
# Auto: two columns when one would need scrolling.
popup-layout-auto = Auto
popup-layout-one-column = One column
popup-layout-two-columns = Two columns
# The warnings, each with its limit and a switch.
setting-warn-cpu-temp = CPU temperature
setting-warn-gpu-temp = GPU temperature
setting-warn-memory = Memory
setting-warn-swap = Swap
setting-warn-disk = Disk
setting-warn-disk-description = Any filesystem
setting-warn-vram = VRAM
setting-warn-nvme = NVMe temperature
setting-warn-nvme-description = Empty: rated limit
# The NVMe limit's field while it is each drive's own rated limit.
setting-limit-rated = Rated

## Panel button: the tooltip, one line per warning

warning-cpu-temp = CPU at { $celsius }°C, limit { $limit }°C
# $gpu is the model, or "GPU card1" (gpu-unnamed).
warning-gpu-temp = { $gpu } at { $celsius }°C, limit { $limit }°C
warning-memory = Memory { $percent }% full, limit { $limit }%
warning-swap = Swap { $percent }% full, limit { $limit }%
warning-filesystem = { $mount } { $percent }% full, limit { $limit }%
# $gpu is the model, or "GPU card1" (gpu-unnamed).
warning-vram = { $gpu } VRAM { $percent }% full, limit { $limit }%
# An NVMe drive, against the limit its maker rates it for.
warning-nvme = { $drive } at { $celsius }°C, its limit { $limit }°C
# An NVMe drive, against the limit set in the settings instead.
warning-nvme-override = { $drive } at { $celsius }°C, limit { $limit }°C
