//! Frames into one packet, as libopus's `src/repacketizer.c` writes them
//! (`opus_repacketizer_out_range_impl` without extensions, and `opus_packet_pad`): code 0, 1,
//! 2 or 3 by the frames' count and sizes, the self-delimited framing of a multistream packet's
//! inner streams (RFC 6716 Appendix B), and padding up to a size (zeros).

use crate::error::{Error, Result};
use crate::packet::Packet;

/// A frame length in one or two bytes (`encode_size`).
fn push_size(out: &mut Vec<u8>, size: usize) {
    if size < 252 {
        out.push(size as u8);
    } else {
        let first = 252 + (size & 3);
        out.push(first as u8);
        out.push(((size - first) >> 2) as u8);
    }
}

fn size_bytes(size: usize) -> usize {
    1 + usize::from(size >= 252)
}

/// One packet of `frames` (each at most 1275 bytes) with the configuration and stereo flag of
/// `toc`, in at most `maxlen` bytes; `pad` fills it to exactly `maxlen`.
pub(crate) fn assemble(
    toc: u8,
    frames: &[&[u8]],
    maxlen: usize,
    self_delimited: bool,
    pad: bool,
) -> Result<Vec<u8>> {
    let count = frames.len();
    if count == 0 || count > 48 {
        return Err(Error::BadArgument("a packet has 1 to 48 frames"));
    }
    let too_small = || Error::BadArgument("the packet does not fit");
    let toc = toc & 0xFC;
    let last = frames[count - 1].len();
    let delim = if self_delimited { size_bytes(last) } else { 0 };
    let mut out = Vec::with_capacity(maxlen);
    let mut tot_size = delim;
    if count == 1 {
        tot_size += frames[0].len() + 1;
        out.push(toc);
    } else if count == 2 {
        if frames[1].len() == frames[0].len() {
            tot_size += 2 * frames[0].len() + 1;
            out.push(toc | 1);
        } else {
            tot_size += frames[0].len() + frames[1].len() + 1 + size_bytes(frames[0].len());
            out.push(toc | 2);
            push_size(&mut out, frames[0].len());
        }
    }
    if tot_size > maxlen {
        return Err(too_small());
    }
    if count > 2 || (pad && tot_size < maxlen) {
        // Code 3, from the start.
        out.clear();
        tot_size = delim;
        let vbr = frames.iter().any(|f| f.len() != frames[0].len());
        if vbr {
            tot_size += 2;
            for f in &frames[..count - 1] {
                tot_size += size_bytes(f.len()) + f.len();
            }
            tot_size += last;
        } else {
            tot_size += count * frames[0].len() + 2;
        }
        if tot_size > maxlen {
            return Err(too_small());
        }
        out.push(toc | 3);
        out.push(count as u8 | if vbr { 0x80 } else { 0 });
        let pad_amount = if pad { maxlen - tot_size } else { 0 };
        if pad_amount != 0 {
            out[1] |= 0x40;
            let nb_255s = (pad_amount - 1) / 255;
            out.extend(std::iter::repeat_n(255, nb_255s));
            out.push((pad_amount - 255 * nb_255s - 1) as u8);
        }
        if vbr {
            for f in &frames[..count - 1] {
                push_size(&mut out, f.len());
            }
        }
    }
    if self_delimited {
        push_size(&mut out, last);
    }
    for f in frames {
        out.extend_from_slice(f);
    }
    if pad {
        out.resize(maxlen, 0);
    }
    Ok(out)
}

/// `opus_packet_pad`: `packet` grown to `new_len` bytes with code 3 padding; unchanged when it
/// is that long already.
pub(crate) fn pad(packet: &mut Vec<u8>, new_len: usize) -> Result<()> {
    if packet.len() >= new_len {
        return if packet.len() == new_len {
            Ok(())
        } else {
            Err(Error::BadArgument("a packet longer than its padded size"))
        };
    }
    let parsed = Packet::parse(packet)?;
    let out = assemble(packet[0], parsed.frames(), new_len, false, true)?;
    *packet = out;
    Ok(())
}

/// A packet's frames framed again (self-delimited for a multistream packet's inner streams),
/// padded to `maxlen` with `pad`; its own padding dropped.
pub(crate) fn reframe(
    packet: &[u8],
    maxlen: usize,
    self_delimited: bool,
    pad: bool,
) -> Result<Vec<u8>> {
    let parsed = Packet::parse(packet)?;
    assemble(packet[0], parsed.frames(), maxlen, self_delimited, pad)
}

#[cfg(test)]
#[path = "repacketizer_tests.rs"]
mod tests;
