//! The inverse complex FFT of 480, 240, 120 or 60 points the IMDCT runs on (`celt/kiss_fft.c`,
//! `opus_ifft`, float build): mixed radix 4, 2, 3, 5 over the mode's twiddles, unscaled.

#[cfg(test)]
#[path = "fft_tests.rs"]
mod tests;
