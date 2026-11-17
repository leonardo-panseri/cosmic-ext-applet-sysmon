// SPDX-License-Identifier: MPL-2.0

//! Readers for the kernel's procfs and sysfs files (plan §3.1).
//!
//! Every module except [`discover`] is a set of **pure parsers**: functions over the text of one
//! file, returning typed values. They do no I/O, never panic on malformed input, and are tested
//! against fixtures captured from real machines in `tests/fixtures/`. The sampler owns the file
//! handles and rereads them; [`discover`] holds the few directory walks it needs at startup.
//!
//! Bounds (D8) live here too: [`clip_name`] and the `MAX_*` limits.

pub mod discover;
pub mod diskstats;
pub mod drm;
pub mod hwmon;
pub mod loadavg;
pub mod meminfo;
pub mod net_dev;
pub mod nvidia_procfs;
pub mod nvme;
pub mod proc_stat;
pub mod uptime;

/// Longest name kept from system-supplied text, in characters (D8).
pub const MAX_NAME_CHARS: usize = 64;
/// Most network interfaces shown (D8).
pub const MAX_INTERFACES: usize = 16;
/// Most drives shown (D8).
pub const MAX_DRIVES: usize = 8;
/// Most GPUs shown (D8).
pub const MAX_GPUS: usize = 8;

/// Trims `text` and keeps at most [`MAX_NAME_CHARS`] characters of it.
#[must_use]
pub fn clip_name(text: &str) -> String {
    text.trim().chars().take(MAX_NAME_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_name_trims_and_keeps_64_characters_on_char_boundaries() {
        assert_eq!(clip_name("  eno1\n"), "eno1");
        let long = "é".repeat(100);
        assert_eq!(clip_name(&long).chars().count(), MAX_NAME_CHARS);
    }
}

#[cfg(test)]
pub(crate) mod test_fixtures {
    use std::path::PathBuf;

    /// Reads `tests/fixtures/<rel>` as text.
    pub fn read(rel: &str) -> String {
        let path = dir(rel);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    pub fn dir(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(rel)
    }

    /// Every file in `tests/fixtures/<rel>/` as (file name, contents), the way the sampler hands
    /// a sysfs directory to a parser.
    pub fn files(rel: &str) -> Vec<(String, String)> {
        let mut files: Vec<_> = std::fs::read_dir(dir(rel))
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let name = e.file_name().into_string().unwrap();
                (name, std::fs::read_to_string(e.path()).unwrap())
            })
            .collect();
        files.sort();
        files
    }
}
