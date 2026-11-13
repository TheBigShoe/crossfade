//! The test vectors' files: `.bit` streams in `opus_demo`'s framing and `.dec` raw PCM.

/// One packet of a `.bit` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitPacket<'a> {
    /// The packet; empty means lost (`opus_demo` decodes it with packet loss concealment).
    pub data: &'a [u8],
    /// The encoder's final range after the packet; 0 means not recorded.
    pub final_range: u32,
}

/// Splits a `.bit` file: per packet a 4-byte big-endian length, the encoder's 4-byte
/// big-endian final range, then the packet. A length over 1500 bytes (`opus_demo`'s
/// `max_payload_bytes`) or a packet cut short is an error.
pub fn read_bit_file(bytes: &[u8]) -> Result<Vec<BitPacket<'_>>, String> {
    let mut rest = bytes;
    let mut packets = Vec::new();
    while !rest.is_empty() {
        let at = bytes.len() - rest.len();
        let Some((head, after)) = rest.split_first_chunk::<8>() else {
            return Err(format!("a packet header is cut short at byte {at}"));
        };
        let len = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as usize;
        let final_range = u32::from_be_bytes([head[4], head[5], head[6], head[7]]);
        if len > 1500 {
            return Err(format!("packet {} claims {len} bytes", packets.len()));
        }
        let data = after
            .get(..len)
            .ok_or_else(|| format!("packet {} is cut short", packets.len()))?;
        rest = &after[len..];
        packets.push(BitPacket { data, final_range });
    }
    Ok(packets)
}

/// Raw 16-bit little-endian PCM (an odd trailing byte is ignored, as `fread` would).
pub fn read_pcm16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| i16::from_le_bytes(b))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(data: &[u8], rng: u32) -> Vec<u8> {
        let mut v = (data.len() as u32).to_be_bytes().to_vec();
        v.extend(rng.to_be_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn reads_packets_and_final_ranges() {
        let mut file = packet(&[0xFC, 1, 2], 0x1234_5678);
        file.extend(packet(&[], 0));
        file.extend(packet(&[7; 1500], 0xFFFF_FFFF));
        let p = read_bit_file(&file).unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].data, &[0xFC, 1, 2]);
        assert_eq!(p[0].final_range, 0x1234_5678);
        assert!(p[1].data.is_empty());
        assert_eq!(p[2].data.len(), 1500);
        assert_eq!(read_bit_file(&[]).unwrap(), vec![]);
    }

    #[test]
    fn refuses_cut_and_oversized_packets() {
        let file = packet(&[1, 2, 3], 9);
        for cut in 1..file.len() {
            assert!(read_bit_file(&file[..cut]).is_err(), "cut at {cut}");
        }
        assert!(read_bit_file(&packet(&[0; 1501], 0)).is_err());
    }

    #[test]
    fn reads_little_endian_pcm() {
        assert_eq!(
            read_pcm16(&[0x01, 0x80, 0xFF, 0x7F, 9]),
            vec![-32767, 32767]
        );
    }
}
