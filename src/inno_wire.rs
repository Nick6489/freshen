//! Fixed-size, little-endian telemetry protocol. No control commands.
#![allow(dead_code)] // Encoding is DLL-side; decoding is helper-side.
pub const SIZE: usize = 52;
pub const VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message {
    pub token: [u8; 32],
    /// 1 preparing, 2 installing, 3 finalizing, 4 progress, 5 diagnostic code.
    pub kind: u32,
    pub current: u32,
    pub total: u32,
}
impl Message {
    pub fn encode(self) -> [u8; SIZE] {
        let mut bytes = [0; SIZE];
        bytes[..4].copy_from_slice(b"FINO");
        bytes[4..8].copy_from_slice(&VERSION.to_le_bytes());
        bytes[8..40].copy_from_slice(&self.token);
        bytes[40..44].copy_from_slice(&self.kind.to_le_bytes());
        bytes[44..48].copy_from_slice(&self.current.to_le_bytes());
        bytes[48..52].copy_from_slice(&self.total.to_le_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != SIZE
            || &bytes[..4] != b"FINO"
            || u32::from_le_bytes(bytes[4..8].try_into().ok()?) != VERSION
        {
            return None;
        }
        let message = Self {
            token: bytes[8..40].try_into().ok()?,
            kind: u32::from_le_bytes(bytes[40..44].try_into().ok()?),
            current: u32::from_le_bytes(bytes[44..48].try_into().ok()?),
            total: u32::from_le_bytes(bytes[48..52].try_into().ok()?),
        };
        if !(1..=5).contains(&message.kind)
            || (message.kind == 4 && (message.total == 0 || message.current > message.total))
        {
            return None;
        }
        Some(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_and_reject_invalid_frames() {
        let message = Message {
            token: [7; 32],
            kind: 4,
            current: 2,
            total: 3,
        };
        let encoded = message.encode();
        assert_eq!(Message::decode(&encoded), Some(message));
        for length in 0..SIZE {
            assert!(Message::decode(&encoded[..length]).is_none());
        }
        let mut bad = encoded;
        bad[4] = 2;
        assert!(Message::decode(&bad).is_none());
        for (kind, current, total) in [(0, 0, 0), (6, 0, 0), (4, 1, 0), (4, 4, 3)] {
            assert!(
                Message::decode(
                    &Message {
                        kind,
                        current,
                        total,
                        ..message
                    }
                    .encode()
                )
                .is_none()
            );
        }
    }
}
