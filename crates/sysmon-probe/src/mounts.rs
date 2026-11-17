// SPDX-License-Identifier: MPL-2.0

//! Mounted filesystems: real block-device filesystems only, deduplicated by device, at most
//! [`MAX_MOUNTS`] (D7, D8), each with its space from `statvfs`.

use std::path::Path;

use sysmon_proto::{MAX_MOUNTS, Mount, clip_name};

/// Filesystem types that are read-only images: always full, so never worth a warning.
const IMAGE_FS_TYPES: [&str; 5] = ["squashfs", "erofs", "iso9660", "udf", "overlay"];
/// Device names that are not physical storage.
const VIRTUAL_DEVICES: [&str; 3] = ["/dev/loop", "/dev/ram", "/dev/zram"];

/// One line of `/proc/<pid>/mounts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    pub device: String,
    pub mount_point: String,
    pub fs_type: String,
}

/// Parses `/proc/<pid>/mounts`, undoing the kernel's octal escapes (`\040` for a space).
#[must_use]
pub fn parse_mounts(text: &str) -> Vec<MountEntry> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_ascii_whitespace();
            Some(MountEntry {
                device: unescape(fields.next()?),
                mount_point: unescape(fields.next()?),
                fs_type: unescape(fields.next()?),
            })
        })
        .collect()
}

fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = bytes
            .get(i + 1..i + 4)
            .filter(|d| bytes[i] == b'\\' && d.iter().all(|b| (b'0'..=b'7').contains(b)));
        if let Some(d) = octal {
            let value = d.iter().fold(0u16, |v, b| v * 8 + u16::from(b - b'0'));
            out.push(u8::try_from(value).unwrap_or(b'?'));
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The entries that are real block-device filesystems: a `/dev/` source that isn't a loop, RAM
/// or zram device, and not a read-only image type. A device mounted more than once (btrfs
/// subvolumes, bind mounts) is kept once, at its shortest mount point. Sorted by mount point, so
/// `/` comes first, and at most [`MAX_MOUNTS`].
#[must_use]
pub fn real_mounts(entries: Vec<MountEntry>) -> Vec<MountEntry> {
    let mut kept: Vec<MountEntry> = Vec::new();
    for entry in entries {
        if !entry.device.starts_with("/dev/")
            || VIRTUAL_DEVICES.iter().any(|v| entry.device.starts_with(v))
            || IMAGE_FS_TYPES.contains(&entry.fs_type.as_str())
        {
            continue;
        }
        match kept.iter_mut().find(|k| k.device == entry.device) {
            Some(k) if shorter(&entry.mount_point, &k.mount_point) => *k = entry,
            Some(_) => {}
            None => kept.push(entry),
        }
    }
    kept.sort_by(|a, b| a.mount_point.cmp(&b.mount_point));
    kept.truncate(MAX_MOUNTS);
    kept
}

fn shorter(a: &str, b: &str) -> bool {
    (a.len(), a) < (b.len(), b)
}

/// The real mounts in `mounts_file` (`/proc/self/mounts`), with their space. A mount whose
/// `statvfs` fails is left out.
#[must_use]
pub fn read(mounts_file: &Path) -> Vec<Mount> {
    let Ok(text) = std::fs::read_to_string(mounts_file) else {
        return Vec::new();
    };
    real_mounts(parse_mounts(&text))
        .into_iter()
        .filter_map(|entry| {
            let s = rustix::fs::statvfs(entry.mount_point.as_str()).ok()?;
            Some(Mount {
                mount_point: clip_name(&entry.mount_point),
                device: clip_name(&entry.device),
                fs_type: clip_name(&entry.fs_type),
                total: s.f_blocks.saturating_mul(s.f_frsize),
                used: s
                    .f_blocks
                    .saturating_sub(s.f_bfree)
                    .saturating_mul(s.f_frsize),
                available: s.f_bavail.saturating_mul(s.f_frsize),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/proc/self/mounts` of the reference laptop (abridged), plus the cases it lacks.
    const MOUNTS: &str = "\
/dev/nvme0n1p2 / btrfs rw,noatime,compress=zstd:1,ssd,subvolid=256,subvol=/@ 0 0
devtmpfs /dev devtmpfs rw,nosuid,size=4096k,mode=755,inode64 0 0
tmpfs /dev/shm tmpfs rw,nosuid,nodev,inode64 0 0
proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0
sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0
tmpfs /tmp tmpfs rw,noatime,inode64 0 0
/dev/nvme0n1p2 /srv btrfs rw,noatime,subvolid=259,subvol=/@srv 0 0
/dev/nvme0n1p2 /home btrfs rw,noatime,subvolid=257,subvol=/@home 0 0
/dev/nvme0n1p2 /var/log btrfs rw,noatime,subvolid=262,subvol=/@log 0 0
/dev/nvme1n1p1 /mnt/storage ext4 rw,noatime 0 0
/dev/nvme0n1p1 /boot vfat rw,relatime,fmask=0077,dmask=0077 0 0
fuse-overlayfs /run/user/1000/psd/leo-firefox fuse.fuse-overlayfs rw,nosuid,nodev 0 0
portal /run/user/1000/doc fuse.portal rw,nosuid,nodev,relatime 0 0
/dev/loop3 /var/lib/snapd/snap/core22/1380 squashfs ro,nodev,relatime 0 0
/dev/sr0 /run/media/leo/UBUNTU iso9660 ro,nosuid,nodev,relatime 0 0
overlay /var/lib/docker/overlay2/x/merged overlay rw,relatime 0 0
/dev/sda1 /run/media/leo/My\\040Disk exfat rw,nosuid,nodev,relatime 0 0
";

    fn points(entries: &[MountEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.mount_point.as_str()).collect()
    }

    #[test]
    fn parse_mounts_reads_device_point_and_type() {
        let entries = parse_mounts(MOUNTS);
        assert_eq!(entries.len(), 17);
        assert_eq!(
            entries[0],
            MountEntry {
                device: "/dev/nvme0n1p2".into(),
                mount_point: "/".into(),
                fs_type: "btrfs".into(),
            }
        );
    }

    #[test]
    fn parse_mounts_undoes_octal_escapes() {
        let entries = parse_mounts("/dev/sda1 /media/a\\040b\\134c\\011d ext4 rw 0 0\n");
        assert_eq!(entries[0].mount_point, "/media/a b\\c\td");
        // Not an escape: kept as is.
        assert_eq!(
            parse_mounts("/dev/x /a\\9 ext4 rw 0 0")[0].mount_point,
            "/a\\9"
        );
    }

    #[test]
    fn real_mounts_keeps_block_devices_once_at_their_shortest_mount_point() {
        let real = real_mounts(parse_mounts(MOUNTS));
        assert_eq!(
            points(&real),
            ["/", "/boot", "/mnt/storage", "/run/media/leo/My Disk"]
        );
    }

    #[test]
    fn real_mounts_prefers_the_shortest_point_whatever_the_order() {
        let real = real_mounts(parse_mounts(
            "/dev/sda2 /home btrfs rw 0 0\n/dev/sda2 / btrfs rw 0 0\n",
        ));
        assert_eq!(points(&real), ["/"]);
    }

    #[test]
    fn real_mounts_keeps_at_most_16() {
        let lines: Vec<String> = (0..40)
            .map(|i| format!("/dev/sd{i} /mnt/{i:02} ext4 rw 0 0"))
            .collect();
        let text = lines.join("\n");
        assert_eq!(real_mounts(parse_mounts(&text)).len(), MAX_MOUNTS);
    }

    #[test]
    fn read_gives_this_machine_s_mounts_their_space() {
        // Wherever the tests run, `/proc/self/mounts` parses; the space is checked for what's
        // there (a container may have no block-device mount at all).
        for mount in read(Path::new("/proc/self/mounts")) {
            assert!(mount.device.starts_with("/dev/"));
            assert!(mount.used <= mount.total);
        }
    }
}
