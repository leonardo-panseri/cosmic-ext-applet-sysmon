// SPDX-License-Identifier: MPL-2.0

//! `/sys/class/block`: which whole disk a mounted filesystem lives on.
//!
//! [`BlockTopology::read`] is **I/O**, like [`super::discover`]: it walks the class directory
//! once per discovery, reading only links, directory names and the short `dm/name` files.
//! [`BlockTopology::whole_disk`] then answers from memory: a partition maps to its disk (its
//! parent directory in `/sys/devices`), a stacked device (LUKS and LVM through device-mapper,
//! RAID through md) to the one disk all its `slaves/` lead to.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use super::clip_name;

/// Most block devices looked at, and most slaves per device (D8).
const MAX_ENTRIES: usize = 256;
/// How deep stacked devices are followed: LVM on LUKS on a partition is three.
const MAX_DEPTH: usize = 8;

/// One `/sys/class/block` entry, as far as mapping goes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockEntry {
    /// The kernel name: `nvme0n1p2`, `dm-0`.
    pub name: String,
    /// A partition's disk.
    pub parent: Option<String>,
    /// The devices a stacked device is built on.
    pub slaves: Vec<String>,
    /// A device-mapper device's name, as `/dev/mapper/` shows it.
    pub dm_name: Option<String>,
}

/// Every block device and how they stack, found at discovery.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockTopology {
    entries: HashMap<String, BlockEntry>,
    /// Device-mapper names to kernel names.
    dm_names: HashMap<String, String>,
}

impl BlockTopology {
    /// Reads `class_dir` (`/sys/class/block`). A missing or unreadable directory maps nothing.
    #[must_use]
    pub fn read(class_dir: &Path) -> Self {
        let Ok(dir) = std::fs::read_dir(class_dir) else {
            return Self::default();
        };
        let entries = dir
            .take(MAX_ENTRIES)
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let path = e.path();
                // The class entry links to the device; a partition's parent directory is its disk.
                let parent = path
                    .join("partition")
                    .exists()
                    .then(|| std::fs::read_link(&path).ok())
                    .flatten()
                    .and_then(|target| {
                        let parent = target.parent()?.file_name()?;
                        Some(clip_name(&parent.to_string_lossy()))
                    });
                let slaves = std::fs::read_dir(path.join("slaves"))
                    .map(|d| {
                        d.take(MAX_ENTRIES)
                            .filter_map(Result::ok)
                            .filter_map(|s| s.file_name().into_string().ok())
                            .collect()
                    })
                    .unwrap_or_default();
                let dm_name = std::fs::read_to_string(path.join("dm/name"))
                    .ok()
                    .map(|n| clip_name(&n))
                    .filter(|n| !n.is_empty());
                Some(BlockEntry {
                    name,
                    parent,
                    slaves,
                    dm_name,
                })
            });
        Self::from_entries(entries)
    }

    /// A topology of `entries`.
    #[must_use]
    pub fn from_entries(entries: impl IntoIterator<Item = BlockEntry>) -> Self {
        let mut topology = Self::default();
        for entry in entries {
            if let Some(dm) = &entry.dm_name {
                topology.dm_names.insert(dm.clone(), entry.name.clone());
            }
            topology.entries.insert(entry.name.clone(), entry);
        }
        topology
    }

    /// The whole disk the filesystem on `source` (a mount's `/dev/…` device) lives on: its
    /// kernel name, such as `nvme0n1`. `None` for a source that isn't a known block device, or
    /// that spans more than one disk.
    #[must_use]
    pub fn whole_disk(&self, source: &str) -> Option<&str> {
        let name = self.kernel_name(source.strip_prefix("/dev/")?)?;
        let mut disks = HashSet::new();
        if !self.collect_disks(name, 0, &mut disks) {
            return None;
        }
        let mut disks = disks.into_iter();
        match (disks.next(), disks.next()) {
            (Some(disk), None) => Some(disk),
            _ => None,
        }
    }

    /// The kernel name of `/dev/<path>`: `nvme0n1p2`, `dm-0`, `mapper/<dm name>`, or LVM's
    /// `<vg>/<lv>`.
    fn kernel_name(&self, path: &str) -> Option<&str> {
        if let Some(entry) = self.entries.get(path) {
            return Some(&entry.name);
        }
        if let Some(dm) = path.strip_prefix("mapper/") {
            return self.dm_names.get(dm).map(String::as_str);
        }
        // LVM's /dev/<vg>/<lv> is the dm device `<vg>-<lv>`, dashes in either doubled.
        let (vg, lv) = path.split_once('/')?;
        let dm = format!("{}-{}", vg.replace('-', "--"), lv.replace('-', "--"));
        self.dm_names.get(&dm).map(String::as_str)
    }

    /// Adds the disks under `name` to `disks`. `false` if the stack is deeper than
    /// [`MAX_DEPTH`], which only a loop would be: no answer then rather than a wrong one.
    fn collect_disks<'a>(
        &'a self,
        name: &'a str,
        depth: usize,
        disks: &mut HashSet<&'a str>,
    ) -> bool {
        let Some(entry) = self.entries.get(name) else {
            return true;
        };
        if depth > MAX_DEPTH {
            return false;
        }
        if let Some(parent) = &entry.parent {
            self.collect_disks(parent, depth + 1, disks)
        } else if entry.slaves.is_empty() {
            disks.insert(&entry.name);
            true
        } else {
            entry
                .slaves
                .iter()
                .all(|slave| self.collect_disks(slave, depth + 1, disks))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::path::{Path, PathBuf};

    /// A fake `/sys` under the temp dir, removed on drop.
    struct FakeSys(PathBuf);

    impl FakeSys {
        fn new(tag: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("sysmon-block-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("class/block")).unwrap();
            Self(root)
        }

        fn class(&self) -> PathBuf {
            self.0.join("class/block")
        }

        /// A device at `devices/<path>`, linked from `class/block/<its name>` as the kernel
        /// links it, with `files` in its directory.
        fn device(&self, path: &str, files: &[(&str, &str)]) -> PathBuf {
            let dir = self.0.join("devices").join(path);
            std::fs::create_dir_all(&dir).unwrap();
            for (name, text) in files {
                let file = dir.join(name);
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(file, text).unwrap();
            }
            let name = Path::new(path).file_name().unwrap();
            symlink(
                Path::new("../../devices").join(path),
                self.class().join(name),
            )
            .unwrap();
            dir
        }

        /// Puts `under` in `stacked`'s `slaves/` directory.
        fn slave(&self, stacked: &Path, under: &str) {
            std::fs::create_dir_all(stacked.join("slaves")).unwrap();
            symlink(self.class().join(under), stacked.join("slaves").join(under)).unwrap();
        }
    }

    impl Drop for FakeSys {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The reference laptop's two drives, plus a LUKS partition with LVM on it and a RAID 1.
    fn laptop(sys: &FakeSys) {
        let nvme0 = "pci0000:00/0000:00:02.4/0000:05:00.0/nvme/nvme0/nvme0n1";
        sys.device(nvme0, &[]);
        for p in ["1", "2", "3"] {
            sys.device(&format!("{nvme0}/nvme0n1p{p}"), &[("partition", p)]);
        }
        let nvme1 = "pci0000:00/0000:00:01.2/0000:02:00.0/nvme/nvme1/nvme1n1";
        sys.device(nvme1, &[]);
        sys.device(&format!("{nvme1}/nvme1n1p1"), &[("partition", "1")]);
        for disk in ["sda", "sdb"] {
            let path = format!("pci0000:00/ata1/host0/target0:0:0/0:0:0:0/block/{disk}");
            sys.device(&path, &[]);
            sys.device(&format!("{path}/{disk}1"), &[("partition", "1")]);
        }
        let crypt = sys.device("virtual/block/dm-0", &[("dm/name", "luks-1234\n")]);
        sys.slave(&crypt, "nvme0n1p3");
        let lvm = sys.device("virtual/block/dm-1", &[("dm/name", "vg--data-home\n")]);
        sys.slave(&lvm, "dm-0");
        let raid = sys.device("virtual/block/md0", &[]);
        sys.slave(&raid, "sda1");
        sys.slave(&raid, "sdb1");
        sys.device("virtual/block/zram0", &[]);
    }

    #[test]
    fn partitions_map_to_their_disk() {
        let sys = FakeSys::new("parts");
        laptop(&sys);
        let topology = BlockTopology::read(&sys.class());
        assert_eq!(topology.whole_disk("/dev/nvme0n1p2"), Some("nvme0n1"));
        assert_eq!(topology.whole_disk("/dev/nvme0n1p1"), Some("nvme0n1"));
        assert_eq!(topology.whole_disk("/dev/nvme1n1p1"), Some("nvme1n1"));
        // A filesystem on the whole disk.
        assert_eq!(topology.whole_disk("/dev/nvme1n1"), Some("nvme1n1"));
    }

    #[test]
    fn device_mapper_sources_map_through_their_slaves() {
        let sys = FakeSys::new("dm");
        laptop(&sys);
        let topology = BlockTopology::read(&sys.class());
        // LUKS, by its mapper name and by its kernel name.
        assert_eq!(
            topology.whole_disk("/dev/mapper/luks-1234"),
            Some("nvme0n1")
        );
        assert_eq!(topology.whole_disk("/dev/dm-0"), Some("nvme0n1"));
        // LVM on LUKS: two levels down. The mapper name doubles the dashes inside names.
        assert_eq!(
            topology.whole_disk("/dev/mapper/vg--data-home"),
            Some("nvme0n1")
        );
        assert_eq!(topology.whole_disk("/dev/vg-data/home"), Some("nvme0n1"));
    }

    #[test]
    fn sources_on_several_disks_or_unknown_map_to_none() {
        let sys = FakeSys::new("none");
        laptop(&sys);
        let topology = BlockTopology::read(&sys.class());
        // RAID 1 over two disks: neither one is its disk.
        assert_eq!(topology.whole_disk("/dev/md0"), None);
        for source in [
            "/dev/mapper/missing",
            "/dev/nvme9n1p1",
            "/dev/disk/by-uuid/1234",
            "nvme0n1p2",
            "server:/export",
            "",
        ] {
            assert_eq!(topology.whole_disk(source), None, "{source}");
        }
    }

    #[test]
    fn a_missing_directory_maps_nothing() {
        let sys = FakeSys::new("missing");
        let topology = BlockTopology::read(&sys.0.join("nope"));
        assert_eq!(topology.whole_disk("/dev/nvme0n1p2"), None);
    }

    #[test]
    fn a_slave_loop_ends() {
        // Not something the kernel makes, but a stacked device must not resolve forever.
        let topology = BlockTopology::from_entries([
            BlockEntry {
                name: "dm-0".into(),
                slaves: vec!["dm-1".into()],
                ..BlockEntry::default()
            },
            BlockEntry {
                name: "dm-1".into(),
                slaves: vec!["dm-0".into()],
                ..BlockEntry::default()
            },
        ]);
        assert_eq!(topology.whole_disk("/dev/dm-0"), None);
    }
}
