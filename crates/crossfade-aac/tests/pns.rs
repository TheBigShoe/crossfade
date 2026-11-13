//! Perceptual noise substitution is a pure function of the packet: the noise a block gets
//! does not depend on what was decoded before it, so seeking (a reset, then any packet) gives
//! the same samples as a straight run. The streams are written here: mono, long windows, the
//! low bands silent and the upper ones noise, a different energy per frame.
#![cfg(feature = "decoder")]

use crossfade_aac::Decoder;
use crossfade_aac::asc::AudioSpecificConfig;

/// MSB-first bit writer.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    used: u32,
}

impl Bits {
    fn put(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            if self.used.is_multiple_of(8) {
                self.bytes.push(0);
            }
            let bit = (value >> i) as u8 & 1;
            *self.bytes.last_mut().unwrap() |= bit << (7 - self.used % 8);
            self.used += 1;
        }
    }
}

/// One raw data block: an SCE with bands 0..20 zero and 20..40 noise, the first noise
/// energy `energy` (0..512) and each later one a step up or down, then END.
fn pns_block(energy: u32) -> Vec<u8> {
    let mut b = Bits::default();
    b.put(0, 3); // ID_SCE
    b.put(0, 4); // tag
    b.put(100, 8); // global_gain
    b.put(0, 1); // ics_reserved_bit
    b.put(0, 2); // ONLY_LONG
    b.put(0, 1); // sine window
    b.put(40, 6); // max_sfb
    b.put(0, 1); // predictor_data_present
    b.put(0, 4); // section: zero book,
    b.put(20, 5); // 20 bands
    b.put(13, 4); // section: noise book,
    b.put(20, 5); // 20 bands
    b.put(energy, 9); // the first noise energy, raw
    for band in 1..20 {
        // Scalefactor codes for deltas 0, +1, -1.
        match band % 3 {
            0 => b.put(0x0, 1),
            1 => b.put(0xa, 4),
            _ => b.put(0x4, 3),
        }
    }
    b.put(0, 1); // pulse_data_present
    b.put(0, 1); // tns_data_present
    b.put(0, 1); // gain_control_data_present
    b.put(7, 3); // ID_END
    b.bytes
}

fn decoder() -> Decoder {
    // AAC LC, 44.1 kHz, mono.
    let config = AudioSpecificConfig::parse(&[0x12, 0x08]).unwrap();
    Decoder::new(&config).unwrap()
}

fn decode(d: &mut Decoder, block: &[u8]) -> Vec<f32> {
    let mut out = Vec::new();
    assert_eq!(d.decode(block, &mut out).unwrap(), 1024);
    out
}

#[test]
fn the_blocks_are_noise() {
    let mut d = decoder();
    decode(&mut d, &pns_block(300));
    let out = decode(&mut d, &pns_block(300));
    let energy: f64 = out.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    assert!(energy > 1e-6, "{energy}");
    assert!(out.iter().all(|x| x.abs() < 1.0));
}

#[test]
fn a_blocks_noise_does_not_depend_on_what_came_before() {
    let (a, b, c) = (pns_block(300), pns_block(310), pns_block(290));
    // b's output depends on b and on a (through the overlap), not on anything earlier.
    let mut straight = decoder();
    decode(&mut straight, &a);
    let want = decode(&mut straight, &b);
    let mut after = decoder();
    decode(&mut after, &c);
    decode(&mut after, &c);
    decode(&mut after, &a);
    let got = decode(&mut after, &b);
    assert!(
        got.iter()
            .zip(&want)
            .all(|(x, y)| x.to_bits() == y.to_bits()),
        "the noise depends on the blocks decoded before"
    );
    // And a reset, as a seek does it, changes nothing either.
    after.reset();
    decode(&mut after, &a);
    let got = decode(&mut after, &b);
    assert!(
        got.iter()
            .zip(&want)
            .all(|(x, y)| x.to_bits() == y.to_bits())
    );
}

#[test]
fn different_packets_get_different_noise() {
    // The same energies in two packets that differ only in a byte after END: the noise is
    // seeded from the bytes, so it differs (and still has the same energy per band).
    let a = pns_block(300);
    let mut b = a.clone();
    b.push(0x5A);
    let (x, y) = (decode(&mut decoder(), &a), decode(&mut decoder(), &b));
    assert!(x != y, "two packets got the same noise");
    let energy = |v: &[f32]| v.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>();
    assert!((energy(&x) / energy(&y) - 1.0).abs() < 0.5);
}
