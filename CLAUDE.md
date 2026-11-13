# Crossfade

An FFmpeg-class audio/video tool in pure Rust: one engine, a CLI (`crossfade`)
and a GPUI desktop app (`crossfade-gui`).

## Rules

1. **TDD, visibly.** No production code without a failing test that demanded
   it. Per behaviour: write the smallest test, run it, see it fail for the
   right reason, commit as `test: <what>` with the trimmed failing output in
   the body; then the least code that passes, committed as `feat:`/`fix:`.
   Refactors are `refactor:` with the suite green. A test commit may leave the
   branch red; the branch tip must be green. Scaffolding with no behaviour
   (manifests, licences, justfile) is `chore:`.
2. **Commits:** `<type>: <what changed>`, types `feat fix refactor docs test
   chore style`. Stage files by name (never `git add .` or `-A`). End every
   message with the `Co-Authored-By:` trailer.
3. **Pure Rust engine.** The engine crates (core, flac, alac, aac, mp3, vorbis, opus, loudness, resample, format, codec, dsp, graph)
   may not depend, under any feature, on anything that links a native library
   or compiles C/assembly. Enforced by `engine_is_pure_rust`. Every crate
   carries `#![forbid(unsafe_code)]`.
4. **Dependency direction:** core, flac, alac, loudness, resample, mp3 and the codec crates <- codec, dsp
   <- format <- graph <- app <- cli, ui. Standalone crates (`crossfade-flac` first, then
   `crossfade-loudness`, and the codec crates `crossfade-alac`, `crossfade-aac`, `crossfade-mp3`,
   `crossfade-vorbis` and `crossfade-opus`, the same way) depend on
   nothing in the workspace and not even on core; `codec` wraps them in thin
   adapters. `crossfade-resample` stands alone the same way (only `libm`). A demuxer needs a codec's frame-header parser to split packets (as
   FFmpeg's libavformat uses libavcodec's parsers), so format depends on the
   standalone codec crate directly, never on the engine's `codec`. `appconfig` stands alone and only ui uses it.
   `crossfade-testkit` depends only on core and is only a dev-dependency.
   Nothing points the other way. GPUI is imported only through
   `crossfade_ui::prelude`.
5. **Self-contained tests.** `cargo test --workspace` needs no network,
   FFmpeg, audio device or display. No wall-clock timing assertions: use the
   `Clock` trait with a manual clock.
6. Short comments: why, not what. No banners, no restating the code.
7. **Threads never change output.** A thread count is an execution setting
   (`Limits::max_threads`, `--threads`), never part of a `Job`: the bytes, and
   the errors, are the same at every count, and tests compare 1 thread with
   several (the conformance tier runs both). Threads come from `std` only
   (`crossfade_flac`'s `pool`), and `1` starts none.

## Crates

| Crate | Role |
|---|---|
| `crossfade-core` | Time types, audio description (named layouts: `ChannelLayout::from_name`), codec ids (`CodecId`: PCM, FLAC, ALAC and the lossy Vorbis, Opus, MP3 and AAC, `is_lossy`), `Limits`, the engine contract (`Packet`, `AudioFrame`, `Demuxer`, `Muxer`, `Decoder`, `Encoder`, `Filter`) and the I/O seams (`MediaSource`, `MediaSink`, `JobIo`, `Clock`) |
| `crossfade-format` | Containers (`FormatId`): WAV, FLAC and AIFF/AIFF-C readers and writers, an Ogg reader (a mapping per codec: FLAC and Vorbis (`ogg/vorbis.rs`: granule timing, priming and padding, seek with a page and a long half-block of pre-roll, first link of a chain only) and Opus (`ogg/opus.rs`: RFC 7845, `OpusHead` as extradata, packets timed by their TOC from the first page's granule, pts `-pre_skip`, a cut front starting at 0, end trimming from the last granule, seek 1 s early, first link only)) and writer (FLAC inside), an MP4/M4A reader (`mp4`: the first sound track, ALAC, AAC (its config through `crossfade_aac::asc`) and MP3 entries, sample tables, gapless from the edit list then `iTunSMPB`, iTunes tags, seeking), an ADTS reader (`adts`: confirmed by a second header, resync, CRC-protected frames split per raw data block, a coarse seek index), an MP3 reader (Layer III; framing with resync, Xing/LAME/VBRI info frame, gapless timeline, ID3v2/v1 tags, exact seeking with reservoir pre-roll), tags (`vorbis_comment`, WAV `LIST/INFO`, AIFF text chunks); `open_demuxer` sniffs without seeking (through ID3v2 tags, whose bytes the MP3 reader receives), `create_muxer` writes |
| `crossfade-flac` | Standalone FLAC, no dependencies at all: `bits`, `crc`, `md5`, `frame` and `streaminfo` parsing, the verifying `Decoder` (frame in, raw `Block` out; `push_frame` decodes on threads), the `Encoder` (levels 0-8, 4-32 bit, never worse than raw, deterministic at any thread count), the `pool` its threads run on, its own `Error`. Other projects can use it alone |
| `crossfade-alac` | Standalone Apple Lossless decoder, no dependencies at all: `Config` (the ALACSpecificConfig), `Decoder` (packet in, interleaved `i32` out in ALAC's channel order, right-justified), `wave_layout` (ALAC's layout per channel count as WAVE positions), its own `Error`. Other projects can use it alone |
| `crossfade-aac` | Standalone AAC, only `libm`: the ADTS header (`adts`, with the raw data blocks of a frame) and AudioSpecificConfig / program config element (`asc`) parsers, always built; behind its off-by-default feature `decoder` (AAC is patent-encumbered), the AAC-LC `Decoder` (all window sequences and shapes, M/S, intensity, PNS, TNS, pulses; refuses Main, LTP, SSR, coupling and 960-sample frames by name), its own `Error`. Other projects can use it alone |
| `crossfade-mp3` | Standalone MPEG audio Layer III, only `libm`: `header::FrameHeader` (MPEG-1, 2, 2.5, layers I-III parsed), `info::InfoFrame` (Xing/Info with LAME's delay and padding, VBRI), the `Decoder` (one whole frame in, interleaved `f32` out, always a frame's worth: silence and an error the caller may ignore for a frame without its reservoir; full accuracy on the ISO conformance streams; bounded output for any input), its own `Error`. Other projects can use it alone |
| `crossfade-vorbis` | Standalone Vorbis I decoding, only `libm` (the same bits on every platform): `Ident`, `Comments` and `Setup` parsing (every length and index checked, degenerate values refused, allocations charged to a caller's budget first), `Modes` (a packet's block size from its first bits) and `packet_frames`, the `Decoder` (packet in, interleaved `f32` out, `reset`), `speakers` (the channel table), `pack_headers`/`unpack_headers` (Xiph lacing), its own `Error`; a hidden `testing` module (bitstream writer, Ogg files) for tests. Other projects can use it alone |
| `crossfade-opus` | Standalone Opus (RFC 6716 as RFC 8251 updates it), only `libm`: the range decoder (`range`: every primitive, bit-exact `tell`/`tell_frac`, the final range), packet parsing (`packet`: the TOC, codes 0-3 with rules R1-R7, the self-delimited framing, `packet_duration` from the TOC alone), `OpusHead`/`OpusTags` as data (`head`), the `Decoder` (a port of `opus_decode_frame`: SILK, CELT and hybrid through the crate-internal `celt` and `silk` layers, redundancy and mode transitions, concealment of lost packets (`decode(None)`, `conceal`), `decode_fec`, `set_gain`, `final_range`; packet in, 48 kHz interleaved `f32` out) and `MultistreamDecoder` (families 0, 1 and 255; ambisonics refused). A hidden `testing` module (range encoder, packet writer, Ogg Opus files in code, the vectors' readers, an `opus_compare` port) serves the tier-3 `conformance_opus`; `tests/VECTORS.md` maps the vectors. Its own `Error`. Other projects can use it alone |
| `crossfade-loudness` | Standalone loudness and peak meter, only `libm` (so results are the same bits on every platform): BS.1770-4 integrated, momentary and short-term loudness, EBU Tech 3342 loudness range, sample and true peak, any rate 8 kHz-1,048,575 Hz, a normalising-gain helper, its own `Error`. Other projects can use it alone |
| `crossfade-resample` | Standalone sample-rate conversion, only `libm`: windowed-sinc polyphase `Resampler` (any rates 1 to 1,048,575 Hz, `Fast`/`Balanced`/`Best`, streaming, time-aligned, the same bits on every platform and for any chunking), its own `Error`. Other projects can use it alone |
| `crossfade-codec` | The engine's codecs: PCM (little- and big-endian), thin adapters from `crossfade-flac`'s decoder and encoder, `crossfade-alac`'s decoder (`alac`: ALAC order to WAVE order, 20-bit left-justified in `S24`, independent packets decoded in batches on up to four scoped threads), `crossfade-mp3`'s decoder (`mp3`: `F32`, silence without error for frames whose main data precedes the first packet) and, with the off-by-default feature `aac`, `crossfade-aac`'s decoder (`aac`: interleaved `F32`; without the feature AAC is refused by name) to the engine's `Decoder` and `Encoder` traits (FLAC takes 8-32 bit integer containers and a true depth of 4 up to the container's, 1-8 channels, up to 1,048,575 Hz), from `crossfade-vorbis`'s decoder (`vorbis`: headers from `extradata` in Xiph lacing, `F32` out in `ChannelLayout::vorbis_order`'s order, undecodable packets skipped) and from `crossfade-opus`'s (`opus`: `OpusHead` from `extradata`, its output gain applied, `F32` out in the layout's order, a refused packet concealed for its TOC duration); `decoder_for` / `encoder_for` |
| `crossfade-dsp` | Sample-format conversion (its module doc is the project's rounding rule), the filter `Chain`, the filters (one module each, with their `FilterPlan`s; `resample` wraps `crossfade-resample`), `dither` (TPDF, clip count), `level` (dB through `libm`) |
| `crossfade-graph` | `Job` (plain serialisable data), `filter` (the user's filter text and JSON, units, and `plan`, the one mapping to the dsp's plans), `describe`/`validate` and `describe_concat`/`validate_concat` (the I/O-free plan and check `Pipeline::open` shares), `processing`, `Timeline` (the gapless trim after every decoder), `Pipeline::step`/`step_with` with transcode (one input or a join) and stream-copy engines, `run` with events and cancel, `probe`, `DecodeSession` (one input decoded frame by frame on its timeline, with exact `seek`), `decode_frames` (a decode-only pass for analysis, read through a session) |
| `crossfade-testkit` | Signal generators, PCM oracle, metrics, faulty I/O, manual clock (dev-only) |
| `crossfade-app` | What every front end shares, GUI-free: `FsIo` (file I/O never lives in the engine), `Request`, command-line rendering, `ThreadRunner`, and the screen models (`Inputs`, `inspector`, `ConvertSheet`, `Queue`, and the waveform strip's `WaveView` and `Strip`), plus the background `Analyzer` that feeds the waveform and loudness readout, and the preview `player` (`Player` over an `AudioSink`: a decode thread writes converted `f32` ahead into an `rtrb` ring per load or seek, `Feed::fill` is the wait-free device side, the position counts what was pulled; `VirtualSink` for tests; `Follow` for the playhead) |
| `crossfade-cli` | The `crossfade` binary and a small library (`request_from_args`) the parity tests call: `probe`, `convert`, `concat` |
| `crossfade-ui` | The desktop app: GPUI views over the `crossfade-app` models (the waveform strip's transport too), `audio_out` (the default output device through `cpal`), the `crossfade-gui` binary, headless UI tests, `examples/screenshots.rs` |
| `appconfig` | App-agnostic config layer extracted from Serialist (no GPUI, no `crossfade-*` deps): config paths, layered JSONC settings, comment-preserving editor, Zed-format keymaps and themes, watcher |
| `crossfade-corpus` | Conformance corpus tool: `corpus.toml` manifest (single files and `[[archive]]` members; `expect = "data"` for reference files), fetch/seed/verify into `.corpus-cache` (`--only <id prefix>`, repeatable), `require(id)` and `require_all(prefix)` for tier-3 tests (not an engine crate) |
| `crossfade-audit` | Workspace policy checks, e.g. the purity gate (test-only, unpublished) |

Format rules worth knowing before touching a codec or container: decoders
emit samples left-justified in the container sample format and record the
true depth in `StreamInfo::bits_per_raw_sample`, which every writer honours (the one
rule for it is `crossfade-graph`'s `output_depth`); a speaker layout is a
`ChannelLayout` that converts to and from the WAVE channel mask, in storage order; 8-bit signed audio is carried
as `U8` with a +128 offset; a reader never seeks unless the source says it
can, and never allocates a length it read from the file without checking
`Limits`. The reference `flac` tool and macOS `afconvert`/`afinfo` are
development oracles only; committed tests never call an external tool.
Small reference-encoder fixtures live under `crates/*/tests/fixtures/` with a
README giving the command that made each (5 MB budget for the repository).

## Lossy codecs, gapless and seeking

- **Timeline.** A stream's timeline starts at pts 0, the first sample frame meant to be heard.
  `Packet::pts` is the position of the first frame decoding the packet yields, NEGATIVE for an
  encoder's priming frames; a packet that yields nothing carries the pts of the next frame.
  A decoder stamps each frame with its packet's pts (later frames of the packet count on) and
  emits everything, priming and padding included. `StreamInfo::frames`, when known, counts
  only the frames meant to be heard.
- **Gapless trim in one place.** `crossfade_graph::Timeline`, right after every decoder (the
  transcode, `decode_frames`, the measuring pass, each input of a join), drops what lies
  before pts 0 and cuts at `frames`. A frame wholly on the timeline is not touched. No reader,
  decoder or front end trims on its own.
- **Seeking.** `Demuxer::seek(pts)` (stream 0's time base) lands early enough to cover the
  codec's pre-roll and returns the pts of the next packet (at most `pts`; before the start
  seeks to the start; at or past the end leaves the reader at its end); the default and a
  source that cannot seek give `Unsupported`. After a seek the caller calls
  `Decoder::reset`, which every decoder must implement (no default): it forgets all stream
  state, as if new. Frames decoded before `pts` are the caller's to discard. A reset FLAC
  decoder no longer checks the whole-stream MD5.
- **Seeking in Ogg and through the engine.** An Ogg mapping seeks with
  `PacketReader::seek_granule(target, floor)`: it lands on the packet after the last page of its
  stream (from `floor`, the mapping's first audio packet, kept from `packet_start()`) whose
  granule is at most `target`, returns that granule (`None`: back at `floor`), bisects in
  O(log n) page reads, and checks every page found by scanning by its CRC; `granule()` says
  which granule applies to the packet just read. A mapping asks for a target early enough for
  its pre-roll and reads on from the landing. `crossfade_graph::DecodeSession` is the one exact seek for a front end: it seeks the
  reader, resets the decoder and drops what lies before the target; seek-then-decode equals
  decode-then-slice (the tests, and the tier-3 `conformance_seek`). Opus alone is not exact:
  its decoder restarts 1 s early (RFC 7845 4.6 asks for at least 80 ms) and is then within
  1e-3 of a decode from the start, not bit-identical (measured on the RFC 8251 vectors in
  `conformance_opus`).
- **Lossy output default.** A lossy codec (`CodecId::is_lossy`) has no bit depth; its decoder
  emits interleaved `F32`. With no `--sample-format`, `output_sample_format` writes it as `S16`
  to every container, with the note "<codec> has no bit depth: writing 16-bit; choose
  --sample-format to change", and it is dithered like any float to integer. Float PCM to
  FLAC is still refused.
- **One standalone crate per codec**, `crossfade-<codec>`, depending on nothing in the
  workspace and on nothing else but `libm`, with its own `#[non_exhaustive]` `Error`;
  `crossfade-codec` wraps each in a thin adapter. A container reader that needs a codec's
  header parser depends on the standalone crate, never on `crossfade-codec`.
- **Bit-identical everywhere.** Tables and maths use `libm` only (no `std` `sin`, `cos`,
  `exp`, `ln`, `powf`, no `mul_add`), so a decoder gives the same bits on every platform.
- **Hostile input** never panics and never allocates a length read from the file without a
  check against `Limits`. Every reader and decoder gets a fuzz target.
- **Clean room.** Never read, copy or port GPL/LGPL code (FFmpeg, mpg123, libmad, faad2,
  LAME). Use the specifications and BSD/MIT/Apache/public-domain implementations only, and
  keep their notices where code is ported.
- **`crossfade-opus` alone is BSD-3-Clause** (not MIT OR Apache-2.0): it is a port of the RFC 6716
  reference decoder and libopus, whose licence it keeps; its `COPYING` carries their notice and
  the pointer to Opus's royalty-free patent licences.
- **AAC is patent-encumbered**: its decoder is compiled only with the off-by-default cargo
  feature `aac`; without it an AAC stream is refused with a message that says so.

## Processing (DSP) rules

- **Working format.** Filters process interleaved `f64` frames. The `Chain`
  (in `crossfade-dsp`) converts its input to `f64` with `convert`'s rule, runs
  the filters in order, then quantises to the output format.
- **Bit-exact when nothing is asked.** A job with no filters, no rate or
  channel change and no loss of depth has no chain (`processing` returns
  `None`) and takes the unprocessed path byte for byte; only a chain converts
  to `f64`. Losing depth (`loses_depth`, beside `output_depth`): an integer
  output narrower than the input's true depth, or float to integer; it gets a
  quantise-only chain when the job dithers (`--dither none` keeps the old
  truncation and rounding).
- **Joins** (`crossfade concat`): one input open at a time; each input that
  differs from the joined rate and layout (the first input's, or the output
  settings') gets its own conversion, drained at its end, so seams are
  sample-exact; one main chain runs over the joined stream; inputs alike with
  no processing join bit for bit. `Pipeline::step_with(io)` opens the next
  input.
- **Stopping early.** `Filter::ignores_further_input` (default `false`): once
  any stage of the chain says so, the pipeline stops reading and decoding,
  drains, and reports the whole input as done.
- **Dither and clipping.** After a chain, integer output gets TPDF dither
  (`--dither none` turns it off) from SplitMix64 with a fixed seed, one draw
  per sample in stream order: the same bytes on every platform, at every
  thread count and however frames are split. Integer output saturates; the
  count (signal past full scale, not dither alone) reaches the front end as
  `Event::Clipped`. Float output is never dithered or clipped. Processed
  output's true depth is its format's full width (overriding `output_depth`).
- **Determinism.** Engine DSP uses no `std` transcendental functions (`sin`,
  `cos`, `exp`, `ln`, `log10`, `powf`) and no `mul_add`: `libm` only, so a
  gain is the same bits everywhere. Plain `+ - * /`, `round`, `sqrt` and
  integer powers are exact IEEE operations and fine.
- **Where things live.** The user's words (units, seconds, dB, the text form,
  serde) are `crossfade-graph`'s `filter::FilterSpec`; what runs (frames at
  each filter's own input rate, linear gains) is `crossfade-dsp`'s
  `FilterPlan`. `crossfade_graph::filter::plan` is the one mapping, and it
  appends the output's rate and channel conversions as ordinary filters.
  `crossfade_dsp::filters::build` is the one place a plan becomes a running
  filter.
- **Timing.** A filter's output pts count output frames at its output rate,
  without gaps, starting at its first input pts rescaled (see `Filter`).
- **Adding a filter**: its text, JSON and rules in `crossfade-graph`'s `filter`
  module, its plan and `Filter` in its own module under
  `crates/crossfade-dsp/src/filters/`, one arm in `build_threads`, one arm in
  `filter::plan`'s mapping; test it in `crates/crossfade-dsp/tests/` and
  through the engine, add a row to the plans snapshot and the README's table.
- **Normalising** runs twice (`run`): a measuring pass (the chain up to the
  `normalize` stage, into a meter standing in for the encoder; nothing is
  written) and the conversion, with the stage replaced by the gain the
  measurement gives (`ChainPlan::with_measured_gain`). Events `Measuring`, then
  `Normalized`, then `Started`; each pass is half the progress. The input is
  opened twice through `JobIo`.
- **Notes.** A plan's `notes` say what the job assumes (a downmix of unlabelled
  channels takes FLAC's default layout for their count); the CLI prints them.
- **Threads in DSP.** The resampler converts segments of the stream on up to
  `Limits::max_threads` threads (`std::thread::scope`), bit-identical to one
  thread: segments start on the ratio's input period and each has a pre- and
  post-roll as long as the filter's reach. `1` starts no thread, nor does a
  stream under two segments.
- **Stream copy** refuses filters and any rate or channel change.

The purity gate lives in its own crate so its `serde_json` dependency and
`cargo metadata` call never touch an engine crate's dependency graph.

## Gate

```sh
just ci        # fmt-check + clippy -D warnings + cargo test --workspace, the same lint and tests with the aac feature (lint-aac, test-aac), then just conformance
just corpus    # once per clone: fetch every conformance file, FLAC/Opus/Vorbis/MP3/AAC (~550 MB; needs curl and the network); one codec: crossfade-corpus fetch --only flac/
just conformance  # tier 3: FLAC, seeking, MP3, AAC (ISO streams), Vorbis (Xiph files) and Opus (RFC 8251 vectors) vs the frozen results (needs the corpus, ~60 s in release)
just mobile    # engine crates build for aarch64-apple-ios and aarch64-linux-android
just deny      # licences, advisories, sources (needs the network)
just coverage  # line floors: 87% workspace, 90% crossfade-core
just fuzz-smoke  # 30 s per target; needs cargo-fuzz (nightly optional, see the justfile)
just lint-aac  # clippy and tests of what only the `aac` feature compiles (part of `just ci`;
just test-aac  # `just coverage` builds the workspace with it, so the decoder is counted)
just bench     # criterion; not part of the gate
just run       # the desktop app
just screenshots [names]  # offscreen PNGs in target/screenshots (macOS only)
```

`just ci` must pass before a branch is handed over. Work happens on a task
branch (`m<N>/<topic>`); the lead reviews and merges it into `main` with
`--no-ff`, so `main`'s first-parent history is always green.

`just ci` runs tier 3, so a fresh clone needs `just corpus` once. The tier FAILS,
never skips, when a corpus file is missing (the error says to run `just corpus`);
`CROSSFADE_CORPUS_DIR` points it at a shared cache. Plain `cargo test --workspace`
never needs the corpus: the conformance tests are `#[ignore]`d.

Speed claims are made whole process against whole process:
`scripts/compare-with-flac.py` times `crossfade convert` against the reference
`flac`, and `scripts/compare-decoders.py` times MP3, AAC and ALAC decoding
against `mpg123` and `afconvert`. `docs/performance.md` is the short report
(results on one page, the reference tools and how to get them, how to repeat);
`docs/performance-details.md` holds the full tables and the history. Update
both when the numbers move. In-process timings alone once hid 20 ms of pipeline overhead.
Check a hot-loop change under `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1` too:
auto-vectorisation must not depend on how the crate is split.

`fuzz/` is its own workspace (libFuzzer is C++) with its own lockfile. Without
nightly, a target still runs unsanitised on the pinned toolchain:
`RUSTC_BOOTSTRAP=1 cargo fuzz run --sanitizer none <target>`.

UI work: views stay thin and render a `crossfade-app` model; test the model
first, then the view with `#[gpui_test]` (headless, no display). A convert
built in the GUI must equal `crossfade_cli::request_from_args` for the same
intent (the parity tests). Review `just screenshots` output before handing over
UI changes. Audio output (`cpal`) lives only in `crossfade-ui` and is opened only by
`open_main_window`; tests and screenshots play through `VirtualSink`, and nothing on the
device's callback path blocks, locks or allocates. Tests and screenshots never read or write the real config
directory. The first GPUI build takes several minutes; a `target/` cloned from
another checkout (`cp -Rc`) needs `cargo clean -p <each workspace crate>` first
or stale artifacts are reused.

The purity test calls `cargo metadata`, which may fetch crate manifests for
other platforms on its very first run.

## Test tiers

Run on every `cargo test`:

- 0: unit and property tests (`proptest`)
- 1: round trips (encode then decode, mux then demux)
- 2: snapshots (`insta`; review `.snap` files by hand, `cargo-insta` is not
  assumed)

Beyond the plain `cargo test`:

- 3: conformance against reference vectors (`just conformance`, part of `just ci`: the
  FLAC corpus, seeking on every codec, the MP3 and AAC ISO streams, Xiph's Vorbis files and
  the RFC 8251 Opus vectors (final range per packet, `opus_compare`), each against frozen
  results in `crates/crossfade-format/tests/conformance/`, rewritten only on purpose, with
  `CROSSFADE_BLESS=1`; ~60 s with the corpus cached)
- 4: UI (`#[gpui_test]` runs on every `cargo test`; screenshots are a review
  aid, not a gate)
- 5: fuzzing (`cargo-fuzz`)
- 6: differential against FFmpeg (optional, never a gate for tier 0-2)
- 7: benchmarks
