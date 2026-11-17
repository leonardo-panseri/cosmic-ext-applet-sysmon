// SPDX-License-Identifier: MPL-2.0

//! What the applet and its host **probe** say to each other (plan §3.2).
//!
//! JSON lines over the probe's stdin and stdout. Every message is one JSON object on one line and
//! carries `"v":1` ([`VERSION`]). On start the probe writes [`Reply::Hello`] before reading
//! anything, and the applet checks its version, so a probe from another installation is caught.
//! After that each [`Request`] gets exactly one [`Reply`], in order.
//!
//! Requests are a closed enum and carry no paths, commands or free text: the probe runs
//! unsandboxed, so its whole interface is read-only by construction.
//!
//! Replies are bounded (D8): [`Reply::bounded`] keeps at most [`MAX_TOP`] processes per list,
//! [`MAX_MOUNTS`] mounts, [`MAX_GPUS`] GPUs and [`MAX_NAME_CHARS`] characters per name. The probe builds its replies
//! within those bounds and [`decode_reply`] applies them again, so the applet never holds more,
//! whatever it is talking to. A line longer than [`MAX_LINE_BYTES`] is an error.

use std::io::{self, BufRead, Write};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The protocol version every message carries as `"v"`.
pub const VERSION: u32 = 1;
/// Most processes in each top list (D8).
pub const MAX_TOP: u8 = 5;
/// Most mounts in a reply (D8).
pub const MAX_MOUNTS: usize = 16;
/// Most GPUs in a reply (D8).
pub const MAX_GPUS: usize = 8;
/// Longest name kept from system-supplied text, in characters (D8).
pub const MAX_NAME_CHARS: usize = 64;
/// Longest line either side reads. A bounded reply is a few kilobytes.
pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// What the applet asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "req", rename_all = "snake_case")]
pub enum Request {
    /// The `top` processes by CPU and by memory, at most [`MAX_TOP`] each.
    Processes { top: u8 },
    /// Real block-device filesystems and their space.
    Mounts,
    /// The NVIDIA GPUs the driver lists; with `nvml`, each awake one's stats through the host's
    /// NVML (ADR 0003). Without it, or for a card that isn't awake, nothing touches the card.
    Gpus { nvml: bool },
    /// Drop any handles held for [`Request::Gpus`]. NVML is shut down after every sample
    /// (ADR 0003), so there is never anything to drop; kept for older applets.
    Release,
}

/// What the probe answers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    /// The first line the probe writes. `probe` is the probe's package version, for logs.
    Hello {
        probe: String,
    },
    Processes(TopProcesses),
    Mounts {
        mounts: Vec<Mount>,
    },
    /// The NVIDIA GPUs, in the driver's order; empty without the NVIDIA driver.
    Gpus {
        gpus: Vec<NvidiaGpu>,
    },
    /// The request is known but this probe doesn't answer it (yet).
    Unsupported,
    /// [`Request::Release`] is done.
    Released,
    /// The request couldn't be read: malformed, too long or another version.
    Error {
        message: String,
    },
}

/// The top processes, each list sorted from the highest value down.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TopProcesses {
    pub by_cpu: Vec<Process>,
    pub by_memory: Vec<Process>,
}

/// One process.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    /// CPU use since the previous scan, in percent of one core, as `top` shows it: a process
    /// keeping two cores busy is at 200 %.
    pub cpu: f32,
    /// Resident memory, bytes.
    pub rss: u64,
}

/// A mounted block-device filesystem and its space, in bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mount {
    pub mount_point: String,
    pub device: String,
    pub fs_type: String,
    pub total: u64,
    pub used: u64,
    /// Space unprivileged users may still use; less than `total - used` by the reserved blocks.
    pub available: u64,
}

impl Mount {
    /// Space used, in percent, as `df` computes it: of the space users can have (`used +
    /// available`), so a filesystem whose reserved blocks are all that is left shows 100 %.
    #[must_use]
    pub fn used_percent(&self) -> Option<f32> {
        let whole = self.used.saturating_add(self.available);
        // Percentages need no more than f32 precision.
        #[expect(clippy::cast_precision_loss, reason = "see above")]
        #[expect(clippy::cast_possible_truncation, reason = "see above")]
        (whole > 0).then(|| (self.used as f64 / whole as f64 * 100.0) as f32)
    }
}

/// An NVIDIA GPU, as the probe found it in `/proc/driver/nvidia/gpus/`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NvidiaGpu {
    /// The PCI address, e.g. `0000:01:00.0`.
    pub bus_id: String,
    #[serde(flatten)]
    pub state: NvidiaState,
}

/// What became of an NVIDIA GPU in a [`Request::Gpus`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum NvidiaState {
    /// Not asked: NVML wasn't requested, or the card wasn't awake (`runtime_status` other than
    /// `active`), so nothing touched it.
    Skipped,
    /// Awake, but NVML isn't installed or failed.
    Unavailable,
    /// Awake, and NVML answered. Implausible readings are `None`.
    Sampled(NvmlSample),
}

/// One NVML sample of a GPU. Each value is `None` when NVML didn't give a plausible one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NvmlSample {
    /// `GPU-…`, as in `/proc/driver/nvidia/gpus/*/information`.
    pub uuid: Option<String>,
    pub name: Option<String>,
    /// GPU load, % (0–100).
    pub util: Option<u8>,
    /// GPU core temperature, °C.
    pub celsius: Option<f32>,
    /// Board power draw, W.
    pub watts: Option<f32>,
    /// Video memory in use and in all, bytes.
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
}

impl Reply {
    /// This reply within the bounds of D8.
    #[must_use]
    pub fn bounded(self) -> Self {
        match self {
            Self::Hello { probe } => Self::Hello {
                probe: clip_name(&probe),
            },
            Self::Processes(top) => Self::Processes(TopProcesses {
                by_cpu: bound_processes(top.by_cpu),
                by_memory: bound_processes(top.by_memory),
            }),
            Self::Mounts { mut mounts } => {
                mounts.truncate(MAX_MOUNTS);
                for m in &mut mounts {
                    m.mount_point = clip_name(&m.mount_point);
                    m.device = clip_name(&m.device);
                    m.fs_type = clip_name(&m.fs_type);
                }
                Self::Mounts { mounts }
            }
            Self::Gpus { mut gpus } => {
                gpus.truncate(MAX_GPUS);
                for gpu in &mut gpus {
                    gpu.bus_id = clip_name(&gpu.bus_id);
                    if let NvidiaState::Sampled(sample) = &mut gpu.state {
                        sample.uuid = sample.uuid.as_deref().map(clip_name);
                        sample.name = sample.name.as_deref().map(clip_name);
                        sample.util = sample.util.filter(|u| *u <= 100);
                    }
                }
                Self::Gpus { gpus }
            }
            Self::Error { message } => Self::Error {
                message: clip_name(&message),
            },
            other @ (Self::Unsupported | Self::Released) => other,
        }
    }
}

fn bound_processes(mut list: Vec<Process>) -> Vec<Process> {
    list.truncate(usize::from(MAX_TOP));
    for p in &mut list {
        p.name = clip_name(&p.name);
    }
    list
}

/// Trims `text` and keeps at most [`MAX_NAME_CHARS`] characters of it.
#[must_use]
pub fn clip_name(text: &str) -> String {
    text.trim().chars().take(MAX_NAME_CHARS).collect()
}

/// Why a line couldn't be decoded.
#[derive(Debug)]
pub enum DecodeError {
    /// The line isn't a JSON object of this protocol.
    Malformed(serde_json::Error),
    /// The line has another `"v"`, or none (`None`).
    Version(Option<u64>),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(e) => write!(f, "malformed message: {e}"),
            Self::Version(Some(v)) => write!(f, "protocol version {v}, expected {VERSION}"),
            Self::Version(None) => write!(f, "message without a protocol version"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// `message` as one line, with `"v"` and the trailing newline.
///
/// # Panics
/// Never: the messages of this crate always serialize to JSON objects.
#[must_use]
pub fn encode<T: Serialize>(message: &T) -> String {
    let mut value = serde_json::to_value(message).expect("messages serialize to JSON");
    let object = value.as_object_mut().expect("messages are JSON objects");
    object.insert("v".into(), VERSION.into());
    let mut line = value.to_string();
    line.push('\n');
    line
}

/// Writes `message` as one line and flushes it.
///
/// # Errors
/// If `out` fails.
pub fn write_message<T: Serialize>(out: &mut impl Write, message: &T) -> io::Result<()> {
    out.write_all(encode(message).as_bytes())?;
    out.flush()
}

/// Decodes one line, checking its version first.
///
/// # Errors
/// If the line isn't a message of this protocol version.
pub fn decode<T: DeserializeOwned>(line: &str) -> Result<T, DecodeError> {
    let mut value: serde_json::Value =
        serde_json::from_str(line).map_err(DecodeError::Malformed)?;
    let version = value
        .as_object_mut()
        .and_then(|o| o.remove("v"))
        .and_then(|v| v.as_u64());
    if version != Some(u64::from(VERSION)) {
        return Err(DecodeError::Version(version));
    }
    serde_json::from_value(value).map_err(DecodeError::Malformed)
}

/// Decodes a reply and [bounds](Reply::bounded) it.
///
/// # Errors
/// As [`decode`].
pub fn decode_reply(line: &str) -> Result<Reply, DecodeError> {
    decode::<Reply>(line).map(Reply::bounded)
}

/// Reads one line into `line`, without its newline. `Ok(false)` at the end of the input.
///
/// # Errors
/// If `input` fails, the line isn't UTF-8, or it is longer than [`MAX_LINE_BYTES`].
pub fn read_line(input: &mut impl BufRead, line: &mut String) -> io::Result<bool> {
    line.clear();
    let limit = MAX_LINE_BYTES as u64 + 1;
    let n = io::Read::take(&mut *input, limit).read_line(line)?;
    if n == 0 {
        return Ok(false);
    }
    if !line.ends_with('\n') && n as u64 >= limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "line longer than the protocol allows",
        ));
    }
    if line.ends_with('\n') {
        line.pop();
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, name: &str) -> Process {
        Process {
            pid,
            name: name.into(),
            cpu: 1.5,
            rss: 4096,
        }
    }

    fn mount(point: &str) -> Mount {
        Mount {
            mount_point: point.into(),
            device: "/dev/nvme0n1p2".into(),
            fs_type: "btrfs".into(),
            total: 100,
            used: 40,
            available: 50,
        }
    }

    fn sample() -> NvmlSample {
        NvmlSample {
            uuid: Some("GPU-6e9f6c15-581b-4f11-c568-db1698fe7502".into()),
            name: Some("NVIDIA GeForce RTX 4060 Laptop GPU".into()),
            util: Some(37),
            celsius: Some(52.0),
            watts: Some(31.5),
            vram_used: Some(512 << 20),
            vram_total: Some(8188 << 20),
        }
    }

    /// `line` as JSON, after checking it is one line. Key order depends on `serde_json`'s
    /// features, which the workspace may unify, so tests compare values.
    fn json(line: &str) -> serde_json::Value {
        let body = line.strip_suffix('\n').expect("ends with a newline");
        assert!(!body.contains('\n'));
        serde_json::from_str(body).unwrap()
    }

    #[test]
    fn requests_are_one_line_with_the_version() {
        use serde_json::json;
        assert_eq!(
            json(&encode(&Request::Processes { top: 5 })),
            json!({"v": 1, "req": "processes", "top": 5})
        );
        assert_eq!(
            json(&encode(&Request::Mounts)),
            json!({"v": 1, "req": "mounts"})
        );
        assert_eq!(
            json(&encode(&Request::Gpus { nvml: false })),
            json!({"v": 1, "req": "gpus", "nvml": false})
        );
        assert_eq!(
            json(&encode(&Request::Release)),
            json!({"v": 1, "req": "release"})
        );
    }

    #[test]
    fn every_message_round_trips() {
        for request in [
            Request::Processes { top: 3 },
            Request::Mounts,
            Request::Gpus { nvml: true },
            Request::Release,
        ] {
            assert_eq!(decode::<Request>(&encode(&request)).unwrap(), request);
        }
        for reply in [
            Reply::Hello {
                probe: "0.1.0".into(),
            },
            Reply::Processes(TopProcesses {
                by_cpu: vec![process(1, "init")],
                by_memory: vec![process(2, "firefox")],
            }),
            Reply::Mounts {
                mounts: vec![mount("/")],
            },
            Reply::Gpus {
                gpus: vec![
                    NvidiaGpu {
                        bus_id: "0000:01:00.0".into(),
                        state: NvidiaState::Sampled(sample()),
                    },
                    NvidiaGpu {
                        bus_id: "0000:02:00.0".into(),
                        state: NvidiaState::Skipped,
                    },
                    NvidiaGpu {
                        bus_id: "0000:03:00.0".into(),
                        state: NvidiaState::Unavailable,
                    },
                ],
            },
            Reply::Unsupported,
            Reply::Released,
            Reply::Error {
                message: "nope".into(),
            },
        ] {
            assert_eq!(decode_reply(&encode(&reply)).unwrap(), reply);
        }
    }

    #[test]
    fn hello_is_checked_against_the_version() {
        assert!(matches!(
            decode_reply("{\"reply\":\"hello\",\"probe\":\"9\",\"v\":2}"),
            Err(DecodeError::Version(Some(2)))
        ));
        assert!(matches!(
            decode_reply("{\"reply\":\"hello\",\"probe\":\"9\"}"),
            Err(DecodeError::Version(None))
        ));
        assert!(matches!(
            decode_reply("{\"reply\":\"hello\",\"probe\":\"9\",\"v\":1}"),
            Ok(Reply::Hello { .. })
        ));
    }

    #[test]
    fn requests_outside_the_closed_set_are_rejected() {
        for line in [
            "{\"req\":\"exec\",\"cmd\":\"rm -rf /\",\"v\":1}",
            "{\"req\":\"processes\",\"top\":\"all\",\"v\":1}",
            "{\"req\":\"processes\",\"top\":300,\"v\":1}",
            "[\"mounts\"]",
            "not json",
        ] {
            assert!(
                matches!(
                    decode::<Request>(line),
                    Err(DecodeError::Malformed(_) | DecodeError::Version(_))
                ),
                "{line}"
            );
        }
    }

    #[test]
    fn decoded_replies_are_bounded() {
        let long = "x".repeat(200);
        let reply = Reply::Processes(TopProcesses {
            by_cpu: (0..20).map(|pid| process(pid, &long)).collect(),
            by_memory: vec![],
        });
        let Reply::Processes(top) = decode_reply(&encode(&reply)).unwrap() else {
            panic!()
        };
        assert_eq!(top.by_cpu.len(), 5);
        assert!(top.by_cpu.iter().all(|p| p.name.chars().count() == 64));

        let reply = Reply::Mounts {
            mounts: (0..40).map(|i| mount(&format!("/mnt/{i}"))).collect(),
        };
        let Reply::Mounts { mounts } = decode_reply(&encode(&reply)).unwrap() else {
            panic!()
        };
        assert_eq!(mounts.len(), 16);
    }

    #[test]
    fn gpu_replies_are_bounded() {
        let long = "x".repeat(200);
        let gpu = NvidiaGpu {
            bus_id: long.clone(),
            state: NvidiaState::Sampled(NvmlSample {
                name: Some(long),
                util: Some(250),
                ..sample()
            }),
        };
        let reply = Reply::Gpus {
            gpus: vec![gpu; 20],
        };
        let Reply::Gpus { gpus } = decode_reply(&encode(&reply)).unwrap() else {
            panic!()
        };
        assert_eq!(gpus.len(), MAX_GPUS);
        assert_eq!(gpus[0].bus_id.len(), 64);
        let NvidiaState::Sampled(sample) = &gpus[0].state else {
            panic!()
        };
        assert_eq!(sample.name.as_deref().map(str::len), Some(64));
        assert_eq!(sample.util, None);
    }

    #[test]
    fn a_skipped_gpu_is_one_small_object() {
        let line = encode(&Reply::Gpus {
            gpus: vec![NvidiaGpu {
                bus_id: "0000:01:00.0".into(),
                state: NvidiaState::Skipped,
            }],
        });
        assert_eq!(
            json(&line),
            serde_json::json!({"v": 1, "reply": "gpus", "gpus": [{"bus_id": "0000:01:00.0", "state": "skipped"}]})
        );
    }

    #[test]
    fn used_percent_is_of_the_space_users_can_have() {
        // 40 used, 50 available, 10 reserved: 40 / 90.
        assert_eq!(mount("/").used_percent(), Some(44.444_443));
        let empty = Mount {
            total: 0,
            used: 0,
            available: 0,
            ..mount("/")
        };
        assert_eq!(empty.used_percent(), None);
    }

    #[test]
    fn read_line_splits_lines_and_stops_at_the_end() {
        let mut input = io::Cursor::new("one\ntwo");
        let mut line = String::new();
        assert!(read_line(&mut input, &mut line).unwrap());
        assert_eq!(line, "one");
        assert!(read_line(&mut input, &mut line).unwrap());
        assert_eq!(line, "two");
        assert!(!read_line(&mut input, &mut line).unwrap());
    }

    #[test]
    fn read_line_refuses_overlong_lines() {
        let mut input = io::Cursor::new("x".repeat(MAX_LINE_BYTES + 10));
        let mut line = String::new();
        assert!(read_line(&mut input, &mut line).is_err());
        // Exactly the limit, with its newline, is fine.
        let mut input = io::Cursor::new(format!("{}\n", "x".repeat(MAX_LINE_BYTES)));
        assert!(read_line(&mut input, &mut line).unwrap());
    }

    #[test]
    fn write_message_writes_one_line() {
        let mut out = Vec::new();
        write_message(&mut out, &Reply::Released).unwrap();
        assert_eq!(
            json(std::str::from_utf8(&out).unwrap()),
            serde_json::json!({"v": 1, "reply": "released"})
        );
    }
}
