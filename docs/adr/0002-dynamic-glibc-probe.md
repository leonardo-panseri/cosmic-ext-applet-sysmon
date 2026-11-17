---
status: accepted
---

# The probe is a dynamically linked glibc binary, not static musl

The plan first said to build the probe for `x86_64-unknown-linux-musl`, statically linked, so it would not
depend on the host's libc. Spike S2 showed that this is impossible and would be wrong anyway:

- `org.freedesktop.Sdk.Extension.rust-stable` for 25.08 ships std for `x86_64-unknown-linux-gnu` and
  `i686-unknown-linux-gnu` only (plus `aarch64-unknown-linux-gnu` on aarch64). There is no musl target and
  the SDK has no musl libc.
- In P4 the probe must `dlopen` the host's `libnvidia-ml.so.1`. A static musl binary can't `dlopen` at all,
  and glibc doesn't support `dlopen` from a static binary unless the same glibc is present at runtime (the
  SDK has 2.42; the reference host has 2.44). That would mean two libc copies in one threaded process.

So the probe is an ordinary dynamically linked `*-unknown-linux-gnu` binary built in the SDK, and it runs with
the host's own loader and glibc. The S2 test binary needed only `libc.so.6`, `libgcc_s.so.1` and the
loader, with no symbol newer than `GLIBC_2.34`. It ran on the host, read the host's `/proc` and loaded host
libraries. The host's `libnvidia-ml.so.1` needs only `GLIBC_2.7` and the basic libc libraries.

## Considered options

- **Static musl.** No target in the SDK, and it can't `dlopen` libnvidia-ml.
- **Static glibc (`+crt-static`).** Only links with a workaround (the SDK has no `libm.a`), and `dlopen`
  from it is unsupported.
- **Dynamic, always started through the runtime's `ld.so`** (`<runtime-path>/lib/x86_64-linux-gnu/ld-linux-x86-64.so.2
  --library-path … <probe>`). Tested working, and it removes the host glibc floor, but it adds a launch
  path that depends on `runtime-path` for no benefit on any host COSMIC supports.

## Consequences

- The host needs glibc ≥ 2.34 and the standard loader path (Ubuntu 22.04+, Debian 12+, Pop!_OS 22.04+,
  Fedora 35+, RHEL 9+). A build check keeps the probe's newest `GLIBC_` symbol version at or below 2.34.
- If the probe fails to start (old glibc, or NixOS without nix-ld), the applet keeps working with
  sandbox-only data: no process list and no host mounts or NVML stats. It doesn't retry in a loop.
- aarch64 uses `aarch64-unknown-linux-gnu`, built the same way.
- Spike S3 also found that `flatpak-spawn` doesn't work under `flatpak-builder --run` (the bus proxy can't
  reach the session bus). Test the probe from an installed build (`just flatpak-install`), or with the
  host's `flatpak build --talk-name=org.freedesktop.Flatpak <builddir> …` on a finished build dir.
