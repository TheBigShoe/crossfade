//! Scalefactor band offsets (ISO/IEC 14496-3 Tables 4.129 to 4.141, 1024/128 columns) and
//! TNS limits (Table 4.156) by sampling index. Values as in the standard; cross-checked against
//! the MIT-licensed crate `oxideav-aac` 0.1.7 (`swb_offset.rs`, `tns_max.rs`), whose notice is
//! in `codebooks.rs`.

const LONG_96: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64, 72, 80, 88, 96, 108, 120, 132,
    144, 156, 172, 188, 212, 240, 276, 320, 384, 448, 512, 576, 640, 704, 768, 832, 896, 960, 1024,
];
const LONG_64: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 64, 72, 80, 88, 100, 112, 124, 140,
    156, 172, 192, 216, 240, 268, 304, 344, 384, 424, 464, 504, 544, 584, 624, 664, 704, 744, 784,
    824, 864, 904, 944, 984, 1024,
];
const LONG_48: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 48, 56, 64, 72, 80, 88, 96, 108, 120, 132, 144, 160,
    176, 196, 216, 240, 264, 292, 320, 352, 384, 416, 448, 480, 512, 544, 576, 608, 640, 672, 704,
    736, 768, 800, 832, 864, 896, 928, 1024,
];
const LONG_32: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 48, 56, 64, 72, 80, 88, 96, 108, 120, 132, 144, 160,
    176, 196, 216, 240, 264, 292, 320, 352, 384, 416, 448, 480, 512, 544, 576, 608, 640, 672, 704,
    736, 768, 800, 832, 864, 896, 928, 960, 992, 1024,
];
const LONG_24: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 52, 60, 68, 76, 84, 92, 100, 108, 116, 124, 136,
    148, 160, 172, 188, 204, 220, 240, 260, 284, 308, 336, 364, 396, 432, 468, 508, 552, 600, 652,
    704, 768, 832, 896, 960, 1024,
];
const LONG_16: &[u16] = &[
    0, 8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 88, 100, 112, 124, 136, 148, 160, 172, 184, 196, 212,
    228, 244, 260, 280, 300, 320, 344, 368, 396, 424, 456, 492, 532, 572, 616, 664, 716, 772, 832,
    896, 960, 1024,
];
const LONG_8: &[u16] = &[
    0, 12, 24, 36, 48, 60, 72, 84, 96, 108, 120, 132, 144, 156, 172, 188, 204, 220, 236, 252, 268,
    288, 308, 328, 348, 372, 396, 420, 448, 476, 508, 544, 580, 620, 664, 712, 764, 820, 880, 944,
    1024,
];

const SHORT_96: &[u16] = &[0, 4, 8, 12, 16, 20, 24, 32, 40, 48, 64, 92, 128];
const SHORT_48: &[u16] = &[0, 4, 8, 12, 16, 20, 28, 36, 44, 56, 68, 80, 96, 112, 128];
const SHORT_24: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 36, 44, 52, 64, 76, 92, 108, 128,
];
const SHORT_16: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 32, 40, 48, 60, 72, 88, 108, 128,
];
const SHORT_8: &[u16] = &[
    0, 4, 8, 12, 16, 20, 24, 28, 36, 44, 52, 60, 72, 88, 108, 128,
];

/// Long-window band offsets by sampling index 0 to 11, with the closing 1024.
pub(crate) const SWB_LONG: [&[u16]; 12] = [
    LONG_96, LONG_96, LONG_64, LONG_48, LONG_48, LONG_32, LONG_24, LONG_24, LONG_16, LONG_16,
    LONG_16, LONG_8,
];

/// Short-window band offsets by sampling index 0 to 11, with the closing 128.
pub(crate) const SWB_SHORT: [&[u16]; 12] = [
    SHORT_96, SHORT_96, SHORT_96, SHORT_48, SHORT_48, SHORT_48, SHORT_24, SHORT_24, SHORT_16,
    SHORT_16, SHORT_16, SHORT_8,
];

/// TNS_MAX_BANDS for Main and LC profiles, long and short windows.
pub(crate) const TNS_MAX_BANDS_LONG: [u8; 12] = [31, 31, 34, 40, 42, 51, 46, 46, 42, 42, 42, 39];
pub(crate) const TNS_MAX_BANDS_SHORT: [u8; 12] = [9, 9, 10, 14, 14, 14, 14, 14, 14, 14, 14, 14];
/// TNS_MAX_ORDER for LC.
pub(crate) const TNS_MAX_ORDER_LONG: usize = 12;
pub(crate) const TNS_MAX_ORDER_SHORT: usize = 7;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_tables_rise_and_close_the_spectrum() {
        for (long, short) in SWB_LONG.iter().zip(SWB_SHORT) {
            assert_eq!((long[0], *long.last().unwrap()), (0, 1024));
            assert_eq!((short[0], *short.last().unwrap()), (0, 128));
            for t in [*long, short] {
                // Every band is a whole number of 4-line Huffman quads.
                assert!(t.windows(2).all(|w| w[0] < w[1] && (w[1] - w[0]) % 4 == 0));
            }
        }
        let bands: Vec<_> = SWB_LONG.iter().map(|t| t.len() - 1).collect();
        assert_eq!(bands, [41, 41, 47, 49, 49, 51, 47, 47, 43, 43, 43, 40]);
        let bands: Vec<_> = SWB_SHORT.iter().map(|t| t.len() - 1).collect();
        assert_eq!(bands, [12, 12, 12, 14, 14, 14, 15, 15, 15, 15, 15, 15]);
    }
}
