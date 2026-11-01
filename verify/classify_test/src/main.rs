//! Proves `classify` right for every input. Each byte is classified on its own, so trying
//! all 256 values in all 32 positions covers everything. This file sits outside track-b/ so
//! it adds nothing to the entry's source size.

#[path = "../../../track-b/src/classify.rs"]
mod classify;

use classify::{CHUNK, classify};

fn main() {
    let simd = cfg!(target_feature = "avx2");
    for filler in [b' ', b'a', b'\n', 0, 0x80, 0xFF] {
        for lane in 0..CHUNK {
            for value in 0..=255u8 {
                let mut chunk = [filler; CHUNK];
                chunk[lane] = value;
                let (tokens, line_ends) = classify(&chunk);
                for (i, &byte) in chunk.iter().enumerate() {
                    assert_eq!(tokens >> i & 1 == 1, byte.is_ascii_alphanumeric(), "token, byte {byte:#x} at {i}");
                    assert_eq!(line_ends >> i & 1 == 1, byte == b'\n', "LF, byte {byte:#x} at {i}");
                }
            }
        }
    }
    println!("classify is correct for every byte in every position (AVX2 build: {simd})");
}
