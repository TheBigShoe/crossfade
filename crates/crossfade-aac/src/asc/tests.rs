use super::*;

#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    used: u32,
}

impl Bits {
    fn put(mut self, n: u32, v: u32) -> Self {
        for i in (0..n).rev() {
            if self.used.is_multiple_of(8) {
                self.out.push(0);
            }
            let bit = (v >> i) & 1;
            *self.out.last_mut().unwrap() |= (bit as u8) << (7 - self.used % 8);
            self.used += 1;
        }
        self
    }
    fn align(mut self) -> Self {
        self.used = self.used.div_ceil(8) * 8;
        self
    }
    fn bytes(self) -> Vec<u8> {
        self.out
    }
}

/// object type, index, channels, then GASpecificConfig with no flags.
fn ga(aot: u32, index: u32, channels: u32) -> Bits {
    Bits::default()
        .put(5, aot)
        .put(4, index)
        .put(4, channels)
        .put(3, 0)
}

#[test]
fn the_two_byte_config_of_lc_stereo_at_44100() {
    let c = AudioSpecificConfig::parse(&[0x12, 0x10]).unwrap();
    assert_eq!(
        c,
        AudioSpecificConfig {
            object_type: 2,
            sample_rate: 44100,
            sampling_index: 4,
            channel_configuration: 2,
            program_config: None,
            frame_length_flag: false,
            depends_on_core_coder: false,
            extension_flag: false,
            sbr: None,
        }
    );
    assert_eq!(c.channels(), 2);
}

#[test]
fn channel_configurations_one_to_seven_give_their_channel_counts() {
    for (config, channels) in [(1, 1), (2, 2), (3, 3), (4, 4), (5, 5), (6, 6), (7, 8)] {
        let c = AudioSpecificConfig::parse(&ga(2, 3, config).bytes()).unwrap();
        assert_eq!(c.channels(), channels, "{config}");
    }
    for config in 8..16 {
        assert_eq!(
            AudioSpecificConfig::parse(&ga(2, 3, config).bytes()),
            Err(Error::Unsupported(format!(
                "aac: channel configuration {config} is not supported"
            )))
        );
    }
}

#[test]
fn an_explicit_rate_takes_the_table_index_the_standard_gives_it() {
    for (rate, index) in [(44100, 4), (46008, 4), (46009, 3), (7350, 11), (100_000, 0)] {
        let bytes = Bits::default()
            .put(5, 2)
            .put(4, 15)
            .put(24, rate)
            .put(4, 1)
            .put(3, 0)
            .bytes();
        let c = AudioSpecificConfig::parse(&bytes).unwrap();
        assert_eq!((c.sample_rate, c.sampling_index), (rate, index), "{rate}");
    }
}

#[test]
fn the_flags_and_the_core_coder_delay_are_read() {
    let bytes = Bits::default()
        .put(5, 2)
        .put(4, 4)
        .put(4, 2)
        .put(1, 1) // frameLengthFlag
        .put(1, 1) // dependsOnCoreCoder
        .put(14, 0x1234)
        .put(1, 0)
        .bytes();
    let c = AudioSpecificConfig::parse(&bytes).unwrap();
    assert!(c.frame_length_flag && c.depends_on_core_coder && !c.extension_flag);
}

#[test]
fn explicit_sbr_and_ps_are_unwrapped_to_their_core() {
    for (aot, ps) in [(5, false), (29, true)] {
        let bytes = Bits::default()
            .put(5, aot)
            .put(4, 6) // core 24 kHz
            .put(4, if ps { 1 } else { 2 })
            .put(4, 3) // SBR at 48 kHz
            .put(5, 2)
            .put(3, 0)
            .bytes();
        let c = AudioSpecificConfig::parse(&bytes).unwrap();
        assert_eq!((c.object_type, c.sample_rate), (2, 24000));
        assert_eq!(
            c.sbr,
            Some(Sbr {
                explicit: true,
                ps,
                sample_rate: 48000
            })
        );
    }
}

#[test]
fn backward_compatible_sbr_and_ps_after_the_core_are_reported() {
    let base = || ga(2, 6, 1).put(11, 0x2B7).put(5, 5).put(1, 1).put(4, 3);
    let c = AudioSpecificConfig::parse(&base().bytes()).unwrap();
    assert_eq!(
        c.sbr,
        Some(Sbr {
            explicit: false,
            ps: false,
            sample_rate: 48000
        })
    );
    let c = AudioSpecificConfig::parse(&base().put(11, 0x548).put(1, 1).bytes()).unwrap();
    assert!(c.sbr.unwrap().ps);
    // The sync word with SBR not present.
    let off = ga(2, 6, 1).put(11, 0x2B7).put(5, 5).put(1, 0).bytes();
    assert_eq!(AudioSpecificConfig::parse(&off).unwrap().sbr, None);
    // Trailing bits that are not the sync word are ignored.
    let junk = ga(2, 4, 2).put(16, 0xFFFF).bytes();
    assert_eq!(AudioSpecificConfig::parse(&junk).unwrap().sbr, None);
}

#[test]
fn object_types_without_a_general_audio_config_are_refused_by_name() {
    for (aot, name) in [
        (8, "CELP"),
        (24, "ER CELP"),
        (39, "ER AAC ELD"),
        (42, "USAC"),
    ] {
        let b = if aot >= 32 {
            Bits::default().put(5, 31).put(6, aot - 32)
        } else {
            Bits::default().put(5, aot)
        };
        let bytes = b.put(4, 4).put(4, 2).put(8, 0).bytes();
        assert_eq!(
            AudioSpecificConfig::parse(&bytes),
            Err(Error::Unsupported(format!(
                "aac: audio object type {aot} ({name}) is not supported"
            )))
        );
    }
    // Main profile has a general audio config: parsing is not refusing.
    assert_eq!(
        AudioSpecificConfig::parse(&ga(1, 4, 2).bytes())
            .unwrap()
            .object_type,
        1
    );
}

#[test]
fn a_config_cut_short_is_an_unexpected_end() {
    let bytes = [0x12, 0x10];
    assert_eq!(AudioSpecificConfig::parse(&[]), Err(Error::UnexpectedEnd));
    assert_eq!(
        AudioSpecificConfig::parse(&bytes[..1]),
        Err(Error::UnexpectedEnd)
    );
    let explicit = Bits::default().put(5, 2).put(4, 15).put(8, 1).bytes();
    assert_eq!(
        AudioSpecificConfig::parse(&explicit),
        Err(Error::UnexpectedEnd)
    );
}

#[test]
fn rate_index_13_and_14_are_reserved() {
    for index in [13, 14] {
        assert_eq!(
            AudioSpecificConfig::parse(&ga(2, index, 2).bytes()),
            Err(Error::Malformed(format!(
                "aac: reserved sampling index {index}"
            )))
        );
    }
}

/// A 5.1 program: front SCE + CPE, back CPE, one LFE, a coupling element, a comment.
fn pce_body(b: Bits) -> Bits {
    b.put(4, 3) // instance tag
        .put(2, 1) // LC
        .put(4, 4) // 44.1 kHz
        .put(4, 2) // front
        .put(4, 0) // side
        .put(4, 1) // back
        .put(2, 1) // lfe
        .put(3, 0) // assoc
        .put(4, 1) // cc
        .put(1, 0)
        .put(1, 0)
        .put(1, 0)
        .put(1, 0)
        .put(4, 0) // front SCE 0
        .put(1, 1)
        .put(4, 1) // front CPE 1
        .put(1, 1)
        .put(4, 2) // back CPE 2
        .put(4, 0) // lfe 0
        .put(1, 1)
        .put(4, 5) // cc, independently switched
        .align()
        .put(8, 2)
        .put(8, u32::from(b'h'))
        .put(8, u32::from(b'i'))
}

fn five_one() -> ProgramConfig {
    ProgramConfig {
        element_instance_tag: 3,
        profile: 1,
        sampling_index: 4,
        front: vec![
            Element {
                is_cpe: false,
                tag: 0,
            },
            Element {
                is_cpe: true,
                tag: 1,
            },
        ],
        side: vec![],
        back: vec![Element {
            is_cpe: true,
            tag: 2,
        }],
        lfe: vec![0],
        assoc_data: vec![],
        coupling: vec![Coupling {
            independently_switched: true,
            tag: 5,
        }],
        comment: b"hi".to_vec(),
    }
}

#[test]
fn channel_configuration_zero_reads_the_program_config_element() {
    let bytes = pce_body(ga(2, 4, 0)).bytes();
    let c = AudioSpecificConfig::parse(&bytes).unwrap();
    assert_eq!(c.program_config, Some(five_one()));
    assert_eq!(c.channels(), 6);
}

#[test]
fn a_raw_data_block_that_opens_with_a_pce_names_its_program() {
    let block = pce_body(Bits::default().put(3, 5))
        .put(3, 7)
        .align()
        .bytes();
    assert_eq!(
        ProgramConfig::from_raw_data_block(&block),
        Ok(Some(five_one()))
    );
    // A block that opens with another element has none to give.
    assert_eq!(ProgramConfig::from_raw_data_block(&[0x20, 0]), Ok(None));
    assert_eq!(
        ProgramConfig::from_raw_data_block(&block[..4]),
        Err(Error::UnexpectedEnd)
    );
}
