//! True bit depth and speaker layout through the real binary. The sources are the committed
//! reference-encoder fixtures; every result is judged by reading header bytes here, not by the
//! engine's own readers.

use std::path::PathBuf;
use std::process::{Command, Output};

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../crossfade-format/tests/fixtures/flac"
);

struct Dir(PathBuf);

impl Dir {
    fn new(test: &str) -> Self {
        let p = std::env::temp_dir().join(format!("crossfade-fid-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }

    fn fixture(&self, name: &str) {
        std::fs::copy(format!("{FIXTURES}/{name}"), self.0.join(name)).unwrap();
    }

    fn read(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.0.join(name)).unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_crossfade"))
            .args(args)
            .current_dir(&self.0)
            .output()
            .unwrap()
    }

    fn convert(&self, input: &str, output: &str, extra: &[&str]) {
        let mut args = vec!["convert", input, "-o", output, "--force"];
        args.extend_from_slice(extra);
        let out = self.run(&args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// STREAMINFO's depth and MD5 of a FLAC file.
fn depth_and_md5(flac: &[u8]) -> (u8, Vec<u8>) {
    assert_eq!(&flac[..4], b"fLaC");
    let info = &flac[8..42];
    (
        ((info[12] & 1) << 4 | info[13] >> 4) + 1,
        info[18..34].to_vec(),
    )
}

#[test]
fn reference_fixtures_keep_their_depth_and_md5_through_flac_wav_and_aiff() {
    let d = Dir::new("depth");
    for (name, depth) in [
        ("stereo-12bit.flac", 12),
        ("stereo-20bit.flac", 20),
        ("stereo-24bit.flac", 24),
    ] {
        d.fixture(name);
        let want = depth_and_md5(&d.read(name));
        assert_eq!(want.0, depth, "{name}");
        d.convert(name, "same.flac", &[]);
        assert_eq!(depth_and_md5(&d.read("same.flac")), want, "{name}");
        for ext in ["wav", "aiff"] {
            let mid = format!("mid.{ext}");
            d.convert(name, &mid, &[]);
            d.convert(&mid, "back.flac", &[]);
            assert_eq!(
                depth_and_md5(&d.read("back.flac")),
                want,
                "{name} via {ext}"
            );
        }
    }
}

#[test]
fn a_20_bit_flac_makes_a_wav_with_twenty_valid_bits_in_a_24_bit_container() {
    let d = Dir::new("wav20");
    d.fixture("stereo-20bit.flac");
    d.convert("stereo-20bit.flac", "out.wav", &[]);
    let wav = d.read("out.wav");
    let u16_at = |i: usize| u16::from_le_bytes([wav[i], wav[i + 1]]);
    assert_eq!(u16_at(20), 0xFFFE);
    assert_eq!(u16_at(34), 24);
    assert_eq!(u16_at(38), 20);
    let aiff = {
        d.convert("stereo-20bit.flac", "out.aiff", &[]);
        d.read("out.aiff")
    };
    let at = aiff.windows(4).position(|w| w == b"COMM").unwrap() + 8;
    assert_eq!(u16::from_be_bytes([aiff[at + 6], aiff[at + 7]]), 20);
}

#[test]
fn asking_for_another_sample_format_writes_that_full_width() {
    let d = Dir::new("widen");
    d.fixture("stereo-16bit.flac");
    d.convert("stereo-16bit.flac", "in.wav", &[]);
    d.convert("in.wav", "wide.flac", &["--sample-format", "s24"]);
    assert_eq!(depth_and_md5(&d.read("wide.flac")).0, 24);
    d.fixture("stereo-20bit.flac");
    d.convert(
        "stereo-20bit.flac",
        "narrow.flac",
        &["--sample-format", "s16"],
    );
    assert_eq!(depth_and_md5(&d.read("narrow.flac")).0, 16);
    d.convert("stereo-20bit.flac", "copy.flac", &["--codec", "copy"]);
    assert_eq!(depth_and_md5(&d.read("copy.flac")).0, 20);
}

#[test]
fn an_untagged_5_1_flac_gets_the_mask_the_reference_tool_writes() {
    let d = Dir::new("mask");
    d.fixture("surround-5.1.flac");
    d.convert("surround-5.1.flac", "out.wav", &[]);
    let wav = d.read("out.wav");
    assert_eq!(u16::from_le_bytes([wav[20], wav[21]]), 0xFFFE);
    // `flac -d` writes 0x060F (front, LFE and side pair) for six untagged channels.
    assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 0x060F);
}
