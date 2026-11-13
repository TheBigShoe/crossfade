# ADTS / AAC fixtures

Made on macOS with Apple's encoder from the deterministic signals that `make_signals.py`
writes (`python3 make_signals.py <dir>`: 16-bit WAV, sines, a log sweep, decaying 3 kHz clicks
every 100 ms, seeded white noise, silence, a 5.1 tone per channel, a six-tone mix with noise).
Each is `afconvert -f adts -d <format> -b <bitrate> <in>.wav <out>.aac`:

| File | Input | `-d` | `-b` | Exercises |
|---|---|---|---|---|
| `tone_44k_stereo.aac` | `tone_44k_stereo.wav` (440/660 Hz, 1 s) | `aac@44100` | 128000 | long blocks, CPE |
| `sweep_48k_mono.aac` | `sweep_48k_mono.wav` (20 Hz-20 kHz, 1 s) | `aac@48000` | 96000 | SCE, every band |
| `clicks_44k_stereo.aac` | `clicks_44k_stereo.wav` (1 s) | `aac@44100` | 128000 | short blocks, block switching, TNS |
| `noise_44k_stereo.aac` | `noise_44k_stereo.wav` (0.5 s) | `aac@44100` | 192000 | escape codebook |
| `silence_44k_stereo.aac` | `silence_44k_stereo.wav` (0.5 s) | `aac@44100` | 64000 | empty frames |
| `tone_16k_mono.aac` | `tone_16k_mono.wav` (1 s) | `aac@16000` | 32000 | 16 kHz band tables |
| `tone_8k_mono.aac` | `tone_8k_mono.wav` (1 s) | `aac@8000` | 16000 | 8 kHz band tables |
| `mix_44k_stereo_64k.aac` | `mix_44k_stereo.wav` (1 s) | `aac@44100` | 64000 | low bitrate stereo tools |
| `mix_44k_stereo_he.aac` | `mix_44k_stereo.wav` | `aach` | 48000 | HE-AAC: SBR in FIL, played as its 22.05 kHz core |
| `tones_48k_51.aac` | `tones_48k_51.wav` (0.5 s) | `aac@48000`, `-l MPEG_5_1_A -l AAC_5_1` | default | 5.1, LFE |

Apple writes MPEG-2 ADTS (`FF F9`), no CRC, one raw data block per frame.
