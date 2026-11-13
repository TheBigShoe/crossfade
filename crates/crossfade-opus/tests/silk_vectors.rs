//! SILK's own tier-3 check, stricter than `opus_compare`: the SILK-only RFC 8251 vectors (02
//! NB, 03 MB, 04 WB; 10 to 60 ms frames, mono then stereo) decode with every packet's final
//! range matching and every 16-bit sample EQUAL to the reference `.dec`, since SILK and its
//! resampler are integer arithmetic. `#[ignore]`d: it needs the corpus
//! (`CROSSFADE_CORPUS_DIR`, default `.corpus-cache` at the workspace root; `just corpus`).

use crossfade_opus::testing::bitfile::read_pcm16;
use crossfade_opus::testing::compare::compare_pcm16;
use crossfade_opus::testing::vectors::decode_vector;
use std::path::PathBuf;

fn vector(name: &str) -> Vec<u8> {
    let dir = std::env::var_os("CROSSFADE_CORPUS_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.corpus-cache"),
        PathBuf::from,
    );
    let path = dir.join("opus/rfc8251").join(name).join(name);
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!("{}: {e} (run `just corpus`, or set CROSSFADE_CORPUS_DIR)", path.display())
    })
}

#[test]
#[ignore = "tier 3: needs the corpus (`just corpus`)"]
fn silk_only_vectors_are_sample_exact() {
    for n in ["02", "03", "04"] {
        let bit = vector(&format!("testvector{n}.bit"));
        let ours = decode_vector(&bit, 2, |_| {}).unwrap_or_else(|e| panic!("vector {n}: {e}"));
        let reference = read_pcm16(&vector(&format!("testvector{n}.dec")));
        assert_eq!(ours.len(), reference.len(), "vector {n}: length");
        let max_diff = ours
            .iter()
            .zip(&reference)
            .map(|(&a, &b)| (i32::from(a) - i32::from(b)).abs())
            .max()
            .unwrap_or(0);
        let q = compare_pcm16(&reference, &ours, true, 48000).unwrap();
        println!(
            "testvector{n}: {} samples, max difference {max_diff}, Q {:.1}",
            ours.len(),
            q.q
        );
        assert!(q.passes(), "vector {n}: Q {}", q.q);
        assert_eq!(max_diff, 0, "vector {n}: not sample-exact");
    }
}
