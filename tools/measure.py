#!/usr/bin/env python3
"""Measure what a running applet costs: CPU time and wakeups, and whether card0 stays suspended.

Stands in for `perf stat -p` and `powertop` (plan §5, P1 and P6), by the developer's decision:
both need privileges or extra packages, while this reads only procfs and sysfs. Every second, for
the whole run, it reads:

- `/proc/<pid>/stat`: `utime + stime` of the whole process (dead threads included), in clock
  ticks (`SC_CLK_TCK`). Reported as a percentage of the wall time: one core fully busy is 100 %.
- `/proc/<pid>/task/*/status`: `voluntary_ctxt_switches + nonvoluntary_ctxt_switches` of each
  thread. Reported per second, summed over the union of the threads seen. Each switch away
  from a thread that then runs again is one wakeup, so this is a **proxy for powertop's wakeups**,
  not the same count: powertop also attributes timer and interrupt wakeups, and does not count a
  thread that yields while still runnable. Threads that start during the run count from zero;
  threads that exit count up to their last reading (switches in their last second are missed).
- `/sys/class/drm/card0/device/power/runtime_status`, reported if it is ever not `suspended`.

Usage (stdlib only, Python 3.11+; run with `python3 -I`):

    python3 -I tools/measure.py [PID] [SECONDS]

PID defaults to the running `cosmic-ext-applet-sysmon` process, SECONDS to 600.
"""

import argparse
import os
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

BINARY = "cosmic-ext-applet-sysmon"
RUNTIME_STATUS = Path("/sys/class/drm/card0/device/power/runtime_status")


def find_applet() -> int:
    """The PID of the running applet. `comm` is truncated to 15 characters, so a match on it is
    confirmed against the executable's full name in `cmdline`."""
    found = []
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            if (proc / "comm").read_text().strip() != BINARY[:15]:
                continue
            argv0 = (proc / "cmdline").read_bytes().split(b"\0")[0].decode(errors="replace")
        except OSError:
            continue
        if os.path.basename(argv0) == BINARY:
            found.append(int(proc.name))
    if not found:
        sys.exit(f"no running {BINARY} process; pass a PID")
    if len(found) > 1:
        sys.exit(f"several {BINARY} processes ({', '.join(map(str, found))}); pass a PID")
    return found[0]


def process_cpu_ticks(pid: int) -> int:
    """utime + stime of the whole process, in clock ticks."""
    stat = Path(f"/proc/{pid}/stat").read_text()
    # comm may hold spaces and parentheses: the fields start after the last ')'.
    fields = stat[stat.rindex(")") + 2:].split()
    # Fields 14 and 15 of proc(5); fields[0] here is field 3.
    return int(fields[11]) + int(fields[12])


def thread_cpu_ticks(task: Path) -> int:
    stat = (task / "stat").read_text()
    fields = stat[stat.rindex(")") + 2:].split()
    return int(fields[11]) + int(fields[12])


def context_switches(task: Path) -> int:
    total = 0
    for line in (task / "status").read_text().splitlines():
        if line.startswith(("voluntary_ctxt_switches:", "nonvoluntary_ctxt_switches:")):
            total += int(line.split(":")[1])
    return total


@dataclass
class Thread:
    comm: str
    first_switches: int
    last_switches: int
    first_ticks: int
    last_ticks: int


@dataclass
class Run:
    pid: int
    threads: dict[int, Thread] = field(default_factory=dict)
    statuses: dict[str, int] = field(default_factory=dict)
    not_suspended: list[tuple[float, str]] = field(default_factory=list)

    def sample(self, started: bool, elapsed: float) -> None:
        for task in Path(f"/proc/{self.pid}/task").iterdir():
            try:
                tid = int(task.name)
                switches = context_switches(task)
                ticks = thread_cpu_ticks(task)
                comm = (task / "comm").read_text().strip()
            except (OSError, ValueError):
                continue  # The thread exited between the listing and the read.
            thread = self.threads.get(tid)
            if thread is None:
                # A thread born during the run counts from zero; one there at the start, from now.
                base_switches, base_ticks = (switches, ticks) if not started else (0, 0)
                self.threads[tid] = Thread(comm, base_switches, switches, base_ticks, ticks)
            else:
                thread.comm, thread.last_switches, thread.last_ticks = comm, switches, ticks
        try:
            status = RUNTIME_STATUS.read_text().strip()
        except OSError as error:
            status = f"unreadable ({error.strerror})"
        self.statuses[status] = self.statuses.get(status, 0) + 1
        if status != "suspended":
            self.not_suspended.append((elapsed, status))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("pid", nargs="?", type=int, help=f"default: the running {BINARY}")
    parser.add_argument("seconds", nargs="?", type=int, default=600, help="default: 600")
    args = parser.parse_args()
    pid = args.pid or find_applet()
    clk_tck = os.sysconf("SC_CLK_TCK")

    run = Run(pid)
    print(f"measuring PID {pid} for {args.seconds} s…", file=sys.stderr)
    start = time.monotonic()
    start_ticks = end_ticks = process_cpu_ticks(pid)
    run.sample(started=False, elapsed=0.0)
    try:
        for second in range(1, args.seconds + 1):
            # Sleep to whole seconds after the start, so the run lasts what was asked.
            time.sleep(max(0.0, start + second - time.monotonic()))
            run.sample(started=True, elapsed=time.monotonic() - start)
            end_ticks = process_cpu_ticks(pid)
    except FileNotFoundError:
        print("the process exited; reporting up to its last reading", file=sys.stderr)
    except KeyboardInterrupt:
        print("interrupted; reporting so far", file=sys.stderr)
    wall = time.monotonic() - start

    cpu_seconds = (end_ticks - start_ticks) / clk_tck
    rows = sorted(
        run.threads.items(),
        key=lambda item: item[1].last_switches - item[1].first_switches,
        reverse=True,
    )
    total_switches = sum(t.last_switches - t.first_switches for _, t in rows)
    print(f"PID {pid}, {wall:.0f} s wall, SC_CLK_TCK {clk_tck}")
    print(f"CPU:      {cpu_seconds:.2f} s = {cpu_seconds / wall * 100:.3f} % of one core")
    print(f"Wakeups:  {total_switches} context switches = {total_switches / wall:.2f}/s "
          f"(over {len(rows)} threads seen)")
    print()
    print(f"{'TID':>8}  {'thread':<16} {'switches':>9} {'/s':>7} {'CPU s':>7}")
    for tid, t in rows:
        switches = t.last_switches - t.first_switches
        ticks = t.last_ticks - t.first_ticks
        print(f"{tid:>8}  {t.comm:<16} {switches:>9} {switches / wall:>7.2f} {ticks / clk_tck:>7.2f}")
    print()
    readings = ", ".join(f"{status} ×{count}" for status, count in run.statuses.items())
    print(f"card0 runtime_status: {readings}")
    for elapsed, status in run.not_suspended:
        print(f"  not suspended at {elapsed:.0f} s: {status}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
