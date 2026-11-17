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
settings-placeholder = Settings arrive in a later version.
back = Back
