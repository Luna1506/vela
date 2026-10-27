//! Length-prefixed framing on the TCP connection: a 4-byte big-endian length
//! followed by that many bytes.

use std::io::{self, Read, Write};

/// Largest frame accepted from the peer.
pub const MAX_FRAME: usize = 5 * 1024 * 1024;

/// Reads one frame. A length above [`MAX_FRAME`] is `InvalidData`; a short
/// read is `UnexpectedEof`.
pub fn read_frame(r: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame of {len} bytes exceeds the {MAX_FRAME} byte limit"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// Writes one frame (length prefix plus `msg`).
pub fn write_frame(w: &mut impl Write, msg: &[u8]) -> io::Result<()> {
    let len = u32::try_from(msg.len())
        .ok()
        .filter(|_| msg.len() <= MAX_FRAME)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    w.write_all(&len.to_be_bytes())?;
    w.write_all(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quickshare::proto::connections::{KeepAliveFrame, OfflineFrame, V1Frame, offline_frame, v1_frame};
    use prost::Message;

    #[test]
    fn frame_roundtrip() {
        let mut b = Vec::new();
        write_frame(&mut b, b"hello").unwrap();
        assert_eq!(&b[..4], &[0, 0, 0, 5]);
        assert_eq!(read_frame(&mut &b[..]).unwrap(), b"hello");
    }

    #[test]
    fn frame_rejects_oversize() {
        let b = ((MAX_FRAME as u32) + 1).to_be_bytes();
        assert_eq!(read_frame(&mut &b[..]).unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn frame_eof_is_unexpected_eof() {
        assert_eq!(read_frame(&mut &[0u8, 0][..]).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn frame_truncated_body_is_unexpected_eof() {
        let b = [0u8, 0, 0, 5, b'h', b'i'];
        assert_eq!(read_frame(&mut &b[..]).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn proto_offline_frame_roundtrips() {
        let frame = OfflineFrame {
            version: Some(offline_frame::Version::V1 as i32),
            v1: Some(V1Frame {
                r#type: Some(v1_frame::FrameType::KeepAlive as i32),
                keep_alive: Some(KeepAliveFrame {
                    ack: Some(true),
                    seq_num: None,
                }),
                ..Default::default()
            }),
        };
        let decoded = OfflineFrame::decode(frame.encode_to_vec().as_slice()).unwrap();
        assert_eq!(decoded, frame);
    }
}
