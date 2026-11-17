// SPDX-License-Identifier: MPL-2.0

//! **I/O**, unlike the rest of [`super`]: directory walks the sampler runs when it (re)discovers
//! devices, not on every tick.

use std::collections::HashSet;
use std::path::Path;

/// Most directory entries looked at, so a host with thousands of veth interfaces stays cheap (D8).
const MAX_ENTRIES: usize = 4096;

/// Names of the entries of `class_dir` (`/sys/class/net`, `/sys/block`) that have a `device`
/// link, i.e. are backed by hardware (D7). The link is only `lstat`ed, never followed.
///
/// A missing or unreadable directory yields an empty set.
#[must_use]
pub fn names_with_device_link(class_dir: &Path) -> HashSet<String> {
    let Ok(entries) = std::fs::read_dir(class_dir) else {
        return HashSet::new();
    };
    entries
        .take(MAX_ENTRIES)
        .filter_map(Result::ok)
        .filter(|e| {
            std::fs::symlink_metadata(e.path().join("device"))
                .is_ok_and(|m| m.file_type().is_symlink())
        })
        .filter_map(|e| e.file_name().into_string().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    /// A fresh directory under the system temp dir, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("sysmon-discover-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn finds_entries_with_a_device_link_even_a_dangling_one() {
        // Shaped like /sys/class/net: hardware interfaces have a `device` symlink, virtual ones don't.
        let root = TempDir::new("net");
        let class = root.0.join("class");
        let devices = root.0.join("devices");
        std::fs::create_dir_all(devices.join("0000:03:00.0")).unwrap();
        for name in ["eno1", "wlp4s0", "lo", "docker0"] {
            std::fs::create_dir_all(class.join(name)).unwrap();
        }
        symlink(devices.join("0000:03:00.0"), class.join("eno1/device")).unwrap();
        // Never followed, so a target that doesn't exist still counts.
        symlink(devices.join("missing"), class.join("wlp4s0/device")).unwrap();
        std::fs::write(class.join("stray-file"), "").unwrap();

        let mut found: Vec<_> = names_with_device_link(&class).into_iter().collect();
        found.sort();
        assert_eq!(found, ["eno1", "wlp4s0"]);
    }

    #[test]
    fn a_missing_directory_has_no_devices() {
        let root = TempDir::new("missing");
        assert!(names_with_device_link(&root.0.join("nope")).is_empty());
    }
}
