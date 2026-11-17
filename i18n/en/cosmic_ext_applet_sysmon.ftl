app-title = System Monitor
app-comment = CPU, memory, GPU, disk and network at a glance in the COSMIC™ panel
app-keywords = COSMIC;System;Monitor;CPU;Memory;GPU;Network;

## Panel button

cpu = CPU
memory = RAM
gpu = GPU
disk = Disk
temperature = Temp
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
# A GPU whose model isn't known without waking it or a lookup table: "GPU card1".
gpu-unnamed = GPU { $card }
download-label = Download
upload-label = Upload
read = Read
write = Write
# A rate: "1.2M/s".
per-second = { $rate }/s
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
setting-warn-temp = Temperature
setting-warn-temp-description = CPU or GPU, in °C
setting-warn-usage = Memory or disk
setting-warn-usage-description = Percent full

## Panel button: the tooltip, one line per warning

warning-cpu-temp = CPU at { $celsius }°C, limit { $limit }°C
# $gpu is the model, or "GPU card1" (gpu-unnamed).
warning-gpu-temp = { $gpu } at { $celsius }°C, limit { $limit }°C
warning-memory = Memory { $percent }% full, limit { $limit }%
warning-filesystem = { $mount } { $percent }% full, limit { $limit }%
# An NVMe drive, against the limit its maker rates it for.
warning-nvme = { $drive } at { $celsius }°C, its limit { $limit }°C
