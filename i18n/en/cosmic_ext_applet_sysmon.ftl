app-title = System Monitor
app-comment = CPU, memory, GPU, disk and network at a glance in the COSMIC™ panel
app-keywords = COSMIC;System;Monitor;CPU;Memory;GPU;Network;

## Panel button

cpu = CPU
memory = RAM
gpu = GPU
disk = Disk
network = Network
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
section-cpu = CPU
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
# The way to the builder, and its title.
panel-button = Panel button
# Under "Panel button": "Stacked · 4 items".
setting-panel-button-description = { $layout } · { $items ->
    [one] { $items } item
   *[other] { $items } items
}
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

## Popup: the panel button's builder

builder-hint = Select an item to move it, remove it or choose what it shows. Add items with +.
builder-add = Add an item
builder-add-top = Add to the top row
builder-add-bottom = Add to the bottom row
builder-move-left = Move left
builder-move-right = Move right
builder-to-top-row = To top row
builder-to-bottom-row = To bottom row
builder-remove = Remove
# The caption over an item's choices of what it shows.
builder-target-mount = MOUNT POINT
builder-target-disk = DISK
builder-target-drive = DRIVE
builder-target-interface = INTERFACE
# The items, by their full names.
item-cpu = CPU usage
item-cpu-temp = CPU temperature
item-ram = RAM
item-swap = Swap
item-gpu = GPU usage
item-gpu-temp = GPU temperature
item-vram = VRAM
item-disk-usage = Disk usage
item-disk-read = Disk read
item-disk-write = Disk write
item-nvme-temp = NVMe temperature
item-download = Download
item-upload = Upload
# An item and what it shows: "GPU usage · RTX 4060".
item-with-target = { $kind } · { $target }
# What an item shows.
target-auto = Auto
target-all-disks = All disks
target-all-interfaces = All interfaces

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
