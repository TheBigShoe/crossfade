# The RFC 8251 test vectors, from their TOC bytes

The twelve `.bit` files of the Opus test vectors (corpus ids `opus/rfc8251/testvectorNN.bit`;
nothing from them is committed) mapped from each packet's TOC byte and framing alone, without
decoding. Regenerate with:

```sh
D=.corpus-cache/opus/rfc8251
cargo run -p crossfade-opus --example opus_vectors -- $D/testvector*.bit/testvector*.bit
```

Every `.dec` and `m.dec` file is raw 16-bit little-endian PCM, 48 kHz, stereo (the reference
decoder's output at `opus_demo -d 48000 2`, with nothing skipped): each holds exactly 4 bytes per
sample of the durations below (for example testvector01: 5,660,160 bytes = 1,415,040 samples =
29.48 s). `opus_compare` always reads the reference as stereo.

| Vector | Packets | Duration | Mode | Bandwidth | Frame | Mono/stereo | Code 0/1/2/3 | Switches (mode/stereo/bw) |
|---|---|---|---|---|---|---|---|---|
| testvector01 | 2147 | 29.48 s | CELT 2147 | FB 2147 | 10 ms 296, 2.5 ms 1154, 20 ms 148, 5 ms 549 | 0 / 2147 | 0 / 106 / 358 / 1683 | 0 / 0 / 0 |
| testvector02 | 1185 | 25.03 s | SILK 1185 | NB 1185 | 10 ms 607, 20 ms 314, 40 ms 158, 60 ms 106 | 602 / 583 | 1185 / 0 / 0 / 0 | 0 / 1 / 0 |
| testvector03 | 998 | 21.16 s | SILK 998 | MB 998 | 10 ms 508, 20 ms 266, 40 ms 134, 60 ms 90 | 510 / 488 | 998 / 0 / 0 / 0 | 0 / 1 / 0 |
| testvector04 | 1265 | 26.63 s | SILK 1265 | WB 1265 | 10 ms 651, 20 ms 334, 40 ms 168, 60 ms 112 | 640 / 625 | 1265 / 0 / 0 / 0 | 0 / 1 / 0 |
| testvector05 | 2037 | 27.17 s | Hybrid 2037 | SWB 2037 | 10 ms 1357, 20 ms 680 | 1020 / 1017 | 2037 / 0 / 0 / 0 | 0 / 1 / 0 |
| testvector06 | 1876 | 25.02 s | Hybrid 1876 | FB 1876 | 10 ms 1250, 20 ms 626 | 939 / 937 | 1876 / 0 / 0 / 0 | 0 / 1 / 0 |
| testvector07 | 4186 | 22.61 s | CELT 4186 | FB 1064, NB 994, SWB 1064, WB 1064 | 10 ms 568, 2.5 ms 2194, 20 ms 288, 5 ms 1136 | 2128 / 2058 | 4186 / 0 / 0 / 0 | 0 / 1 / 31 |
| testvector08 | 1247 | 27.30 s | CELT 1242, SILK 5 | FB 456, NB 152, SWB 339, WB 300 | 10 ms 173, 2.5 ms 174, 20 ms 586, 5 ms 314 | 361 / 886 | 846 / 16 / 194 / 191 | 1 / 32 / 29 |
| testvector09 | 1337 | 27.57 s | CELT 1332, SILK 5 | FB 1266, NB 20, SWB 35, WB 16 | 10 ms 192, 2.5 ms 274, 20 ms 629, 5 ms 242 | 634 / 703 | 971 / 130 / 43 / 193 | 1 / 43 / 7 |
| testvector10 | 1912 | 32.01 s | CELT 1598, Hybrid 314 | FB 1912 | 10 ms 412, 2.5 ms 778, 20 ms 340, 5 ms 382 | 925 / 987 | 756 / 47 / 325 / 784 | 16 / 64 / 0 |
| testvector11 | 553 | 30.02 s | CELT 553 | FB 553 | 20 ms 553 | 0 / 553 | 204 / 21 / 79 / 249 | 0 / 0 / 0 |
| testvector12 | 1332 | 26.64 s | Hybrid 264, SILK 1068 | MB 222, NB 352, SWB 264, WB 494 | 20 ms 1332 | 1332 / 0 | 1332 / 0 / 0 / 0 | 3 / 0 / 9 |

"Switches" count consecutive packets that differ in mode, in the stereo flag, or in bandwidth.
"Frame" is one frame's duration; a packet holds up to the "most frames" below.

| Vector | Lost (length 0) | Invalid | Most frames in a packet | Empty frames | 1-byte frames | Padded packets | Most padding | Final range 0 |
|---|---|---|---|---|---|---|---|---|
| testvector01 | 0 | 0 | 5 | 0 | 0 | 660 | 996 | 0 |
| testvector02 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 |
| testvector03 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 |
| testvector04 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 |
| testvector05 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 |
| testvector06 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 |
| testvector07 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 |
| testvector08 | 0 | 0 | 3 | 0 | 0 | 0 | 0 | 0 |
| testvector09 | 0 | 0 | 3 | 0 | 0 | 0 | 0 | 0 |
| testvector10 | 0 | 0 | 5 | 0 | 0 | 0 | 0 | 0 |
| testvector11 | 0 | 0 | 6 | 0 | 0 | 0 | 0 | 0 |
| testvector12 | 0 | 0 | 1 | 0 | 0 | 0 | 0 | 0 |

## What to prove a decoder on first

- **CELT alone:** testvector01 (FB stereo, every frame size, codes 1-3, 660 padded packets with
  up to 996 bytes of padding), testvector07 (every CELT bandwidth, every frame size, code 0,
  mono for packets 0-2127 then stereo, 31 bandwidth switches), testvector11 (FB stereo 20 ms,
  every code, up to 6 frames a packet).
- **SILK alone:** testvector02 (NB), testvector03 (MB), testvector04 (WB): every SILK frame size
  (10, 20, 40, 60 ms), code 0 only, mono first and stereo from the switch on (packet 602 in 02).
  These exercise the SILK-to-48 kHz resampler at each internal rate and SILK's own mono to
  stereo transition.
- **Hybrid alone** (needs both layers): testvector05 (SWB), testvector06 (FB), 10 and 20 ms,
  code 0, mono then stereo (from packet 1020 in 05).

## Mode switches (stage 3: redundancy and transitions)

- testvector08 and testvector09: five SILK packets (0-4, stereo), then CELT to the end; one
  SILK to CELT switch, then many stereo and bandwidth switches inside CELT. A CELT-only decoder
  meets SILK at packet 0, so these wait for both layers.
- testvector10: CELT and Hybrid, FB throughout, 16 mode switches and 64 stereo switches.
- testvector12: mono, 20 ms, SILK (0-385), Hybrid (386-607), SILK (608-1289), Hybrid (1290 on);
  bandwidths NB, MB, WB and SWB.

## Unusual

- No packet is lost (length 0), none is invalid, no frame is empty or one byte long: the
  vectors never take the decoder's packet loss concealment path (`opus_decode_frame` treats a
  frame of 0 or 1 bytes as lost), so every packet's final range is checked.
- No final range is 0 (`opus_demo` skips the check for 0).
- No packet holds more than 6 frames: the 48-frame limit is not exercised here (the packet
  tests cover it).
- Padding appears only in testvector01 (code 3 with the padding flag, up to 996 bytes).
