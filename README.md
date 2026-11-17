# System monitor applet for the COSMIC™ desktop

A third-party panel applet for the COSMIC™ desktop that will show CPU, memory, GPU, disk and network use at a
glance, with a popup for details, while costing close to nothing when the popup is closed. Its design follows
the Omarchy plugin [navjottomer.sysmon][omarchy-sysmon]. See [the implementation plan](docs/implementation-plan.md).

**Status:** scaffold (plan phase P0). The panel button shows placeholder text and the popup is empty.

## Install

The applet isn't published yet, so build the Flatpak locally. You need `flatpak`, [just][just], and:

```sh
flatpak install --user flathub org.flatpak.Builder \
    org.freedesktop.Sdk//25.08 org.freedesktop.Sdk.Extension.rust-stable//25.08 \
    com.system76.Cosmic.BaseApp//stable
```

Then, from this repository:

```sh
just flatpak-build     # builds into build-flatpak/repo (offline, from flatpak/cargo-sources.json)
just flatpak-install   # adds build-flatpak/repo as the `sysmon-applet-local` remote and installs from it
```

Open **Settings → Desktop → Panel → Configure panel applets**, press **Add applet** and choose
**System Monitor**.

## Development

A [justfile](./justfile) is included for the [casey/just][just] command runner.

- `just run` builds and runs the applet outside the panel
- `cargo test` runs the unit tests
- `just check --all-targets` runs clippy with pedantic lints; CI also runs `cargo fmt --check` and denies warnings
- `just flatpak-build` builds the working tree as a Flatpak; builder state stays in `.flatpak-builder/` and
  `build-flatpak/`, since `org.flatpak.Builder` has a private `/tmp`
- `just flatpak-sources` regenerates `flatpak/cargo-sources.json`; run it whenever `Cargo.lock` changes
- `just install` installs a host build under `/usr` (`rootdir=` and `prefix=` change where)

Developers should install [rustup][rustup] and configure their editor to use [rust-analyzer][rust-analyzer].

## Translators

[Fluent][fluent] is used for localization of the software. Fluent's translation files are found in the [i18n directory](./i18n). New translations may copy the [English (en) localization](./i18n/en) of the project, rename `en` to the desired [ISO 639-1 language code][iso-codes], and then translations can be provided for each [message identifier][fluent-guide]. If no translation is necessary, the message may be omitted.

## Licence

MPL-2.0.

[fluent]: https://projectfluent.org/
[fluent-guide]: https://projectfluent.org/fluent/guide/hello.html
[iso-codes]: https://en.wikipedia.org/wiki/List_of_ISO_639-1_codes
[just]: https://github.com/casey/just
[rustup]: https://rustup.rs/
[rust-analyzer]: https://rust-analyzer.github.io/
[omarchy-sysmon]: https://github.com/navjottomer/omarchy-sysmon
