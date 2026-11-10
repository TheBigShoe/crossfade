//! The end-to-end gate: the real runner, real threads and a real directory behind the same
//! window the other tests script. Nothing here asserts on how long anything took; the
//! waits poll a state with a count that is only a ceiling.

use std::time::Duration;

use crate::prelude::*;
use crate::test_support::{
    CONTAINER_LABELS, FORMAT_LABELS, Rig, TempDir, add_paths, click, draw, has, open_in,
    open_sheet, pick, sample_at, step, type_output,
};
use crate::workspace::{Workspace, WorkspaceOptions};
use crossfade_codec::decoder_for;
use crossfade_core::{AudioFrame, Limits, MediaSource, Packet, Samples};
use crossfade_format::{FormatId, open_demuxer};

/// Poll until `done` holds. Worker threads report through a channel that wakes the
/// foreground task, so parking is allowed; the bound only stops a hang.
fn wait_until(
    cx: &mut TestAppContext,
    workspace: &Entity<Workspace>,
    done: impl Fn(&Workspace) -> bool,
) {
    for _ in 0..6000 {
        cx.run_until_parked();
        if workspace.read_with(cx, |w, _| done(w)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("the queue did not reach the expected state");
}

fn data_chunk(bytes: &[u8]) -> &[u8] {
    assert_eq!(&bytes[..4], b"RIFF");
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().expect("4 bytes")) as usize;
        if &bytes[pos..pos + 4] == b"data" {
            return &bytes[pos + 8..pos + 8 + len];
        }
        pos += 8 + len + (len & 1);
    }
    panic!("no data chunk");
}

fn real_rig(cx: &mut TestAppContext) -> Rig {
    // Before any worker thread exists: the scheduler flags later wake-ups from other
    // threads as non-determinism unless parking was allowed first.
    cx.executor().allow_parking();
    let config = crate::config_test_support::TempDir::new("gate-config");
    let (window, workspace) = open_in(cx, WorkspaceOptions::real(), &config);
    // The scripted half of the rig is unused here; its runner never starts anything.
    let (script, _) = crate::scripted::Script::options();
    Rig {
        window,
        workspace,
        script,
        dir: TempDir::new("gate"),
        config,
    }
}

#[gpui_test]
fn a_real_conversion_runs_to_done_and_writes_the_expected_samples(cx: &mut TestAppContext) {
    let rig = real_rig(cx);
    let frames = 2_000;
    let input = rig.dir.write_wav("in.wav", 48_000, 2, frames);
    add_paths(cx, &rig, &[&input]);

    // 16-bit in, 24-bit out.
    open_sheet(cx, &rig);
    pick(cx, &rig, "sheet-format", &FORMAT_LABELS, 3);
    type_output(cx, &rig, &rig.dir.path("out24.wav"));
    click(cx, rig.window, "dialog-ok");
    step(cx, 3);
    wait_until(cx, &rig.workspace, |w| {
        w.queue_row_state(0).starts_with("Done")
    });
    draw(cx, rig.window);
    assert!(has(cx, rig.window, "queue-row-0"));
    assert_eq!(
        rig.workspace.read_with(cx, |w, _| w.queue_row_state(0)),
        "Done \u{b7} 2,000 frames"
    );

    let out = std::fs::read(rig.dir.path("out24.wav")).expect("the output exists");
    let data = data_chunk(&out);
    assert_eq!(data.len(), frames as usize * 2 * 3);
    for (i, sample) in data.as_chunks::<3>().0.iter().enumerate() {
        let got = i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) >> 8;
        let index = u32::try_from(i).expect("small");
        assert_eq!(got, i32::from(sample_at(index)) << 8, "sample {i}");
        // Widening 16 to 24 bits is exact: the low byte is zero.
        assert_eq!(sample[0], 0, "sample {i}");
    }
    assert_eq!(rig.dir.listing(), ["in.wav", "out24.wav"]);
}

#[gpui_test]
fn cancelling_a_running_conversion_leaves_no_output_and_no_temp_file(cx: &mut TestAppContext) {
    let rig = real_rig(cx);
    // A sparse half-gigabyte file: far more work than the cancel needs to interrupt, and
    // almost no disk.
    let input = rig.dir.write_sparse_wav("big.wav", 48_000, 256 << 20);
    add_paths(cx, &rig, &[&input]);
    open_sheet(cx, &rig);
    type_output(cx, &rig, &rig.dir.path("never.wav"));
    click(cx, rig.window, "dialog-ok");
    step(cx, 3);
    draw(cx, rig.window);
    click(cx, rig.window, "queue-cancel-0");
    wait_until(cx, &rig.workspace, |w| w.queue_row_state(0) == "Cancelled");
    assert_eq!(rig.dir.listing(), ["big.wav"]);
}

/// A FLAC file's packets and, decoded by the engine, its 16-bit samples.
fn flac_audio(path: &str) -> (Vec<Vec<u8>>, Vec<i16>, Option<u64>) {
    let bytes = std::fs::read(path).expect("the file exists");
    let source: Box<dyn MediaSource> = Box::new(std::io::Cursor::new(bytes));
    let (format, mut demuxer) = open_demuxer(source, &Limits::default()).expect("a readable file");
    assert_eq!(format, FormatId::Flac);
    let stream = demuxer.streams()[0].clone();
    let mut decoder = decoder_for(&stream).expect("a flac decoder");
    let (mut packets, mut samples) = (Vec::new(), Vec::new());
    let mut frame = AudioFrame::new(&stream.audio);
    let mut packet = Packet::default();
    let mut take = |decoder: &mut dyn crossfade_core::Decoder, samples: &mut Vec<i16>| {
        while decoder.receive_frame(&mut frame).expect("decodes") {
            let Samples::S16(chunk) = &frame.samples else {
                panic!("expected 16-bit samples");
            };
            samples.extend_from_slice(chunk);
        }
    };
    while demuxer.read_packet(&mut packet).expect("a packet") {
        packets.push(packet.data.clone());
        decoder.send_packet(&packet).expect("decodes");
        take(&mut *decoder, &mut samples);
    }
    decoder.drain();
    take(&mut *decoder, &mut samples);
    (packets, samples, stream.frames)
}

#[gpui_test]
fn a_wav_converted_to_flac_decodes_bit_exact_and_copies_without_touching_the_audio(
    cx: &mut TestAppContext,
) {
    let rig = real_rig(cx);
    let frames = 2_000;
    let input = rig.dir.write_wav("in.wav", 48_000, 2, frames);
    add_paths(cx, &rig, &[&input]);

    open_sheet(cx, &rig);
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 1);
    click(cx, rig.window, "dialog-ok");
    step(cx, 3);
    wait_until(cx, &rig.workspace, |w| {
        w.queue_row_state(0).starts_with("Done")
    });
    draw(cx, rig.window);

    let flac = rig.dir.path("in.flac");
    let (packets, samples, total) = flac_audio(&flac);
    assert_eq!(total, Some(u64::from(frames)));
    let want: Vec<i16> = (0..frames * 2).map(sample_at).collect();
    assert_eq!(samples, want, "the FLAC decodes to the source samples");

    // Stream copy that FLAC into a second one.
    add_paths(cx, &rig, &[&flac]);
    open_sheet(cx, &rig);
    pick(cx, &rig, "sheet-container", &CONTAINER_LABELS, 1);
    click(cx, rig.window, "sheet-copy");
    draw(cx, rig.window);
    click(cx, rig.window, "dialog-ok");
    step(cx, 3);
    wait_until(cx, &rig.workspace, |w| {
        w.queue_row_state(1).starts_with("Done")
    });
    let (copied_packets, copied_samples, copied_total) = flac_audio(&rig.dir.path("in-copy.flac"));
    assert_eq!(
        copied_packets, packets,
        "the audio frames are byte for byte the same"
    );
    assert_eq!(copied_samples, want);
    assert_eq!(copied_total, total);
    assert_eq!(rig.dir.listing(), ["in-copy.flac", "in.flac", "in.wav"]);
}
