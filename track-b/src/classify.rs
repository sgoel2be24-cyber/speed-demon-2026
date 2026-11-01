//! Sorts 32 bytes at a time into token bytes and line ends.

/// How many bytes `classify` looks at in one go.
pub const CHUNK: usize = 32;

/// Returns two masks for `chunk`. Bit `i` of the first says byte `i` belongs to a token, which
/// only ASCII letters and digits do: every byte of 0x80 or above separates, even inside a
/// letter. Bit `i` of the second says byte `i` is an LF.
#[cfg(target_feature = "avx2")]
#[inline]
pub fn classify(chunk: &[u8; CHUNK]) -> (u32, u32) {
    use std::arch::x86_64::*;
    // SAFETY: the cfg above means this build only runs on CPUs with AVX2, and the load is
    // an unaligned read of the 32 bytes of `chunk`.
    unsafe {
        /// True for bytes from `low` to `high`. The comparison is unsigned, unlike AVX2's
        /// `cmpgt`, which would read every byte of 0x80 or above as a negative number.
        #[inline]
        unsafe fn in_range(bytes: __m256i, low: u8, high: u8) -> __m256i {
            unsafe {
                let offset = _mm256_sub_epi8(bytes, _mm256_set1_epi8(low as i8));
                let width = _mm256_set1_epi8((high - low) as i8);
                _mm256_cmpeq_epi8(_mm256_min_epu8(offset, width), offset)
            }
        }
        let bytes = _mm256_loadu_si256(chunk.as_ptr().cast());
        // Setting bit 0x20 folds case, and no other byte lands in a..z. Digits are tested on
        // the raw bytes, because this folding would also move control bytes onto them.
        let folded = _mm256_or_si256(bytes, _mm256_set1_epi8(0x20));
        let letters = in_range(folded, b'a', b'z');
        let digits = in_range(bytes, b'0', b'9');
        let tokens = _mm256_movemask_epi8(_mm256_or_si256(letters, digits));
        let line_ends =
            _mm256_movemask_epi8(_mm256_cmpeq_epi8(bytes, _mm256_set1_epi8(b'\n' as i8)));
        (tokens as u32, line_ends as u32)
    }
}

/// The same masks without AVX2, for building and testing on other machines.
#[cfg(not(target_feature = "avx2"))]
#[inline]
pub fn classify(chunk: &[u8; CHUNK]) -> (u32, u32) {
    let (mut tokens, mut line_ends) = (0, 0);
    for (i, &byte) in chunk.iter().enumerate() {
        tokens |= (byte.is_ascii_alphanumeric() as u32) << i;
        line_ends |= ((byte == b'\n') as u32) << i;
    }
    (tokens, line_ends)
}
