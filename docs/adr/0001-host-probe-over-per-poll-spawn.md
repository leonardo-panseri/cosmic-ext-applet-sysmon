---
status: accepted
---

# A long-lived host probe, not one host spawn per poll

The applet ships as a Flatpak and reads everything it can from inside the sandbox: `/proc/stat`,
`/proc/meminfo`, `/proc/diskstats`, `/sys` (DRM, hwmon, `runtime_status`) and, with `--share=network`,
`/proc/net/dev` all show host values. The sandbox can't see the host's process table (Flatpak always unshares
the PID namespace), the host's mounts, or NVIDIA stats through the host's `libnvidia-ml`. For those, the
applet starts a small helper, the **probe**, **once** with `flatpak-spawn --host --watch-bus` and talks to it
over stdin/stdout. The probe answers a closed set of read-only requests and exits on EOF or when the applet's
bus name goes away.

This needs `--talk-name=org.freedesktop.Flatpak`, which 34 of the 69 apps in pop-os/cosmic-flatpak already
use, including `minimon-applet` and `sysinfo-applet`. A process blocked on stdin costs a few MB and no
wakeups, so there is no reason to restart it.

## Considered options

- **One `flatpak-spawn --host` per poll.** Rejected: `cosmic-ext-mini-taskmanager` does this
  (`src/process/host.rs`), paying a D-Bus round trip plus a host shell that reads every `/proc/<pid>/stat` on
  every tick. That is the opposite of the applet's goal of costing close to nothing while the popup is closed.

## Consequences

- The probe runs unsandboxed, so its interface accepts no paths, commands or arguments from the applet.
- The probe is a separate binary that runs against the host's libc, not the runtime's. It is dynamically
  linked so it can load the host's libnvidia-ml ([ADR 0002](0002-dynamic-glibc-probe.md)).
- The applet and probe check a protocol version on start, so a probe from another installation is caught.
- Outside a Flatpak (`cargo run`, distro packages) the same interface has an in-process backend, so no helper
  is needed there.
