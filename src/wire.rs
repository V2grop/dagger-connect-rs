//! Version 1 multiplexing frames, carried exclusively inside authenticated Noise records.
use anyhow::{Result, bail, ensure};

pub const DATA_SIZE: usize = 16 * 1024;
const TEXT_SIZE: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Open {
        id: u32,
        kind: StreamKind,
        target: String,
    },
    Opened {
        id: u32,
    },
    Data {
        id: u32,
        data: Vec<u8>,
    },
    Eof {
        id: u32,
    },
    Close {
        id: u32,
    },
    Error {
        id: u32,
        message: String,
    },
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
    Credit {
        id: u32,
        frames: u32,
    },
}

impl Frame {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        match self {
            Self::Open { id, kind, target } => {
                ensure!(
                    !target.is_empty() && target.len() <= TEXT_SIZE,
                    "invalid target length"
                );
                ensure!(!target.contains('\0'), "invalid target");
                header(&mut output, 1, *id)?;
                output.push(match kind {
                    StreamKind::Tcp => 1,
                    StreamKind::Udp => 2,
                });
                output.extend_from_slice(target.as_bytes());
            }
            Self::Opened { id } => header(&mut output, 2, *id)?,
            Self::Data { id, data } => {
                ensure!(data.len() <= DATA_SIZE, "data exceeds frame limit");
                header(&mut output, 3, *id)?;
                output.extend_from_slice(data);
            }
            Self::Eof { id } => header(&mut output, 4, *id)?,
            Self::Close { id } => header(&mut output, 5, *id)?,
            Self::Error { id, message } => {
                ensure!(message.len() <= TEXT_SIZE, "error exceeds frame limit");
                header(&mut output, 6, *id)?;
                output.extend_from_slice(message.as_bytes());
            }
            Self::Credit { id, frames } => {
                ensure!((1..=16).contains(frames), "invalid flow-control credit");
                header(&mut output, 9, *id)?;
                output.extend_from_slice(&frames.to_be_bytes());
            }
            Self::Ping { nonce } | Self::Pong { nonce } => {
                output.push(if matches!(self, Self::Ping { .. }) {
                    7
                } else {
                    8
                });
                output.extend_from_slice(&nonce.to_be_bytes());
            }
        }
        Ok(output)
    }

    pub fn decode(input: &[u8]) -> Result<Self> {
        ensure!(!input.is_empty(), "empty frame");
        let kind = input[0];
        if kind == 7 || kind == 8 {
            ensure!(input.len() == 9, "invalid heartbeat frame");
            let nonce = u64::from_be_bytes(input[1..9].try_into()?);
            return Ok(if kind == 7 {
                Self::Ping { nonce }
            } else {
                Self::Pong { nonce }
            });
        }
        ensure!(input.len() >= 5, "truncated frame");
        let id = u32::from_be_bytes(input[1..5].try_into()?);
        ensure!(id != 0, "stream zero is reserved");
        let tail = &input[5..];
        Ok(match kind {
            1 => {
                ensure!(
                    tail.len() >= 2 && tail.len() <= TEXT_SIZE + 1,
                    "invalid open frame"
                );
                let kind = match tail[0] {
                    1 => StreamKind::Tcp,
                    2 => StreamKind::Udp,
                    _ => bail!("unknown stream kind"),
                };
                let target = std::str::from_utf8(&tail[1..])?.to_owned();
                ensure!(!target.contains('\0'), "invalid target");
                Self::Open { id, kind, target }
            }
            2 | 4 | 5 => {
                ensure!(tail.is_empty(), "trailing frame bytes");
                match kind {
                    2 => Self::Opened { id },
                    4 => Self::Eof { id },
                    _ => Self::Close { id },
                }
            }
            3 => {
                ensure!(tail.len() <= DATA_SIZE, "oversized data frame");
                Self::Data {
                    id,
                    data: tail.to_vec(),
                }
            }
            6 => {
                ensure!(tail.len() <= TEXT_SIZE, "oversized error frame");
                Self::Error {
                    id,
                    message: std::str::from_utf8(tail)?.to_owned(),
                }
            }
            9 => {
                ensure!(tail.len() == 4, "invalid credit frame");
                let frames = u32::from_be_bytes(tail.try_into()?);
                ensure!((1..=16).contains(&frames), "invalid flow-control credit");
                Self::Credit { id, frames }
            }
            _ => bail!("unknown frame type"),
        })
    }
}

fn header(output: &mut Vec<u8>, tag: u8, id: u32) -> Result<()> {
    ensure!(id != 0, "stream zero is reserved");
    output.push(tag);
    output.extend_from_slice(&id.to_be_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_frame_types_roundtrip() -> Result<()> {
        let frames = [
            Frame::Open {
                id: 7,
                kind: StreamKind::Tcp,
                target: "localhost:80".into(),
            },
            Frame::Open {
                id: 8,
                kind: StreamKind::Udp,
                target: "[::1]:53".into(),
            },
            Frame::Opened { id: 7 },
            Frame::Data {
                id: 7,
                data: vec![0, 255, 3],
            },
            Frame::Data {
                id: 8,
                data: vec![],
            },
            Frame::Eof { id: 7 },
            Frame::Close { id: 7 },
            Frame::Error {
                id: 7,
                message: "denied".into(),
            },
            Frame::Ping { nonce: u64::MAX },
            Frame::Pong { nonce: 1 },
            Frame::Credit { id: 7, frames: 16 },
        ];
        for frame in frames {
            assert_eq!(Frame::decode(&frame.encode()?)?, frame);
        }
        Ok(())
    }

    #[test]
    fn malformed_frames_are_rejected_without_panics() {
        assert!(
            Frame::Open {
                id: 1,
                kind: StreamKind::Tcp,
                target: "host\0:80".into(),
            }
            .encode()
            .is_err()
        );
        for bad in [
            vec![],
            vec![1],
            vec![1, 0, 0, 0, 1],
            vec![2, 0, 0, 0, 0],
            vec![2, 0, 0, 0, 1, 0],
            vec![1, 0, 0, 0, 1, 8, 65],
            vec![7, 0],
            vec![6, 0, 0, 0, 1, 255],
            vec![255, 0, 0, 0, 1],
        ] {
            assert!(Frame::decode(&bad).is_err(), "{bad:?}");
        }
        for tag in 0..=255u8 {
            for len in 0..40 {
                let mut input = vec![tag; len];
                if len > 0 {
                    input[0] = tag;
                }
                let _ = Frame::decode(&input);
            }
        }
        assert!(
            Frame::Data {
                id: 1,
                data: vec![0; DATA_SIZE + 1]
            }
            .encode()
            .is_err()
        );
    }
}

// Dagger Rust rewrite by ir_spoof; https://t.me/ir_spoof
// Attribution copy: Dagger Rust rewrite by i​r⁠_​s⁠p​o⁠o​f
