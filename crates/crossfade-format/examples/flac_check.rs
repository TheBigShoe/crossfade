//! Decodes FLAC files with the native reader and decoder and prints one line per file:
//! `cargo run --release -p crossfade-format --example flac_check -- [--raw OUT] [--modes] FILE...`
//!
//! `--raw OUT` writes the decoded samples (the left-justification undone, little-endian signed,
//! `ceil(bits/8)` bytes each; unsigned 8-bit is not used) for comparison with
//! `flac -d --force-raw-format --endian=little --sign=signed`. `--modes` also counts the
//! channel assignments of the frames.

use crossfade_codec::decoder_for;
use crossfade_core::{AudioFrame, Error, Limits, Packet, Samples};
use crossfade_flac::frame::{self, ChannelAssignment};
use std::io::Write;
use std::time::Instant;

fn bits_of(extradata: &[u8]) -> u32 {
    (u32::from(extradata[12] & 1) << 4 | u32::from(extradata[13] >> 4)) + 1
}

fn write_raw(out: &mut impl Write, frame: &AudioFrame, bits: u32) -> std::io::Result<()> {
    let width = bits.div_ceil(8) as usize;
    let mut put = |v: i64| out.write_all(&v.to_le_bytes()[..width]);
    match &frame.samples {
        Samples::U8(v) => v
            .iter()
            .try_for_each(|&s| put((i64::from(s) - 128) >> (8 - bits))),
        Samples::S16(v) => v.iter().try_for_each(|&s| put(i64::from(s) >> (16 - bits))),
        Samples::S24(v) | Samples::S32(v) => {
            let container = if matches!(frame.samples, Samples::S24(_)) {
                24
            } else {
                32
            };
            v.iter()
                .try_for_each(|&s| put(i64::from(s) >> (container - bits)))
        }
        _ => Ok(()),
    }
}

struct Outcome {
    samples: u64,
    md5_checked: bool,
    modes: [u64; 4],
}

fn check(path: &str, raw: Option<&str>) -> (Result<(), Error>, Outcome) {
    let mut o = Outcome {
        samples: 0,
        md5_checked: false,
        modes: [0; 4],
    };
    let r = (|| {
        let file = std::fs::File::open(path)?;
        let mut demux = crossfade_format::flac::open(Box::new(file), &Limits::default())?;
        let stream = demux.streams()[0].clone();
        let bits = bits_of(&stream.extradata);
        o.md5_checked = stream.extradata[18..34].iter().any(|&b| b != 0);
        let mut decoder = decoder_for(&stream)?;
        let mut raw_out = raw
            .map(std::fs::File::create)
            .transpose()?
            .map(std::io::BufWriter::new);
        let (mut packet, mut frame) = (Packet::default(), AudioFrame::new(&stream.audio));
        while demux.read_packet(&mut packet)? {
            if let Ok(h) = frame::parse_header(&packet.data) {
                let i = match h.channels {
                    ChannelAssignment::Independent(_) => 0,
                    ChannelAssignment::LeftSide => 1,
                    ChannelAssignment::SideRight => 2,
                    ChannelAssignment::MidSide => 3,
                };
                o.modes[i] += 1;
            }
            decoder.send_packet(&packet)?;
            while decoder.receive_frame(&mut frame)? {
                o.samples += frame.frames() as u64;
                if let Some(w) = raw_out.as_mut() {
                    write_raw(w, &frame, bits)?;
                }
            }
        }
        decoder.drain();
        while decoder.receive_frame(&mut frame)? {}
        Ok(())
    })();
    (r, o)
}

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    let (mut raw, mut modes) = (None, false);
    let mut files = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--raw" => raw = args.next(),
            "--modes" => modes = true,
            _ => files.push(a),
        }
    }
    for path in files {
        let name = path.rsplit('/').next().unwrap_or(&path);
        let start = Instant::now();
        let (result, o) = check(&path, raw.as_deref());
        let secs = start.elapsed().as_secs_f64();
        let verdict = match &result {
            Ok(()) if o.md5_checked => "ok md5-ok".to_owned(),
            Ok(()) => "ok no-md5".to_owned(),
            Err(e) => format!("ERROR {e}"),
        };
        let extra = if modes {
            format!(" indep/LS/RS/MS={:?}", o.modes)
        } else {
            String::new()
        };
        println!(
            "{name}: {verdict} samples={} secs={secs:.3}{extra}",
            o.samples
        );
    }
}
