//! Tests the actual entry modules without adding tests to the scored directory.
//! The allocator shim uses ordinary boxed slices; mmap/huge-page behaviour is
//! covered by the executable and Docker tests, not by this crate.
#![allow(dead_code)]

#[path = "../../../track-b/src/eval.rs"]
mod eval;
#[path = "../../../track-b/src/queries.rs"]
mod queries;
#[path = "../../../track-b/src/terms.rs"]
mod terms;

pub fn huge_slice<T: Copy + 'static>(len: usize, value: T) -> &'static mut [T] {
    Box::leak(vec![value; len].into_boxed_slice())
}

pub fn free_huge<T>(slice: &mut [T]) {
    // SAFETY: the entry only calls this with a complete allocation from huge_slice,
    // and replaces the old reference immediately without reading it again.
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            slice.as_mut_ptr(),
            slice.len(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scalar(word: &eval::Word) -> (u32, u32) {
        let mut count = 0;
        let mut sum = 0;
        for bit in 0..256 {
            if word[bit / 64] >> (bit % 64) & 1 != 0 {
                count += 1;
                sum += bit as u32;
            }
        }
        (count, sum)
    }

    fn check(totals: &mut eval::Totals, word: eval::Word) {
        totals.count[0] = 0;
        totals.sum[0] = 0;
        totals.add_block(0, &word, 0);
        assert_eq!(
            (totals.count[0] as u32, totals.sum[0]),
            scalar(&word),
            "{word:x?}"
        );
    }

    #[test]
    fn build_features() {
        let avx2 = cfg!(target_feature = "avx2");
        println!("kernel tests AVX2 build: {avx2}");
        if std::env::var_os("VERIFY_REQUIRE_AVX2").is_some() {
            assert!(avx2, "CI must exercise the AVX2 entry paths");
        }
    }

    #[test]
    fn tally_all_byte_values_in_every_position() {
        let mut totals = eval::Totals::new(0, 1);
        for fill in [0, u64::MAX, 0xaaaa_aaaa_aaaa_aaaa, 0x5555_5555_5555_5555] {
            for byte in 0..32 {
                for value in 0..=255u64 {
                    let shift = byte % 8 * 8;
                    let mut word = [fill; 4];
                    word[byte / 8] = (word[byte / 8] & !(255 << shift)) | value << shift;
                    check(&mut totals, word);
                }
            }
        }
    }

    #[test]
    fn tally_all_sixteen_bit_windows_and_complements() {
        let mut totals = eval::Totals::new(0, 1);
        for start in (0..256).step_by(16) {
            for value in 0..=65535u64 {
                let mut word = [0; 4];
                word[start / 64] = value << (start % 64);
                check(&mut totals, word);
                check(&mut totals, word.map(|part| !part));
            }
        }
    }

    #[test]
    fn tally_mixed_words_and_local_id_limit() {
        let mut totals = eval::Totals::new(0, 1);
        let mut state = 0x1234_5678_9abc_def0u64;
        for _ in 0..100_000 {
            let word = std::array::from_fn(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            });
            check(&mut totals, word);
        }
        totals = eval::Totals::new(0, 1);
        for start in (0..65280).step_by(256) {
            totals.add_block(0, &[u64::MAX; 4], start);
        }
        totals.add_block(0, &[u64::MAX, u64::MAX, u64::MAX, u64::MAX >> 1], 65280);
        assert_eq!(totals.count[0], 65535);
        assert_eq!(totals.sum[0], 65535 * 65534 / 2);
        assert_eq!(totals.first[0], [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }

    #[test]
    fn dense_term_cap_keeps_ids_exact() {
        let mut text = String::new();
        // More than u16::MAX dense candidates; the rest must become rare terms.
        for term in 0..65540 {
            for _ in 0..4 {
                text.push_str(&format!("BOOL t{term}\n"));
            }
        }
        let plan = queries::compile(text.as_bytes());
        assert_eq!(plan.n_dense, 65535);
        assert_eq!(plan.n_bool_terms, 65540);
        for (line, &position) in plan.position_of_line.iter().enumerate() {
            let query = &plan.queries[position as usize];
            assert_eq!(
                plan.table.term(query.terms[0]),
                format!("t{}", line / 4).as_bytes()
            );
            if (position as usize) < plan.forms.len() {
                assert_eq!(
                    plan.forms[position as usize].terms[0] as u32,
                    query.terms[0]
                );
            } else {
                assert!(plan.triggers.get(query.terms[0]).contains(&position));
            }
        }
    }

    #[test]
    fn every_dense_rare_pattern_and_operator_sequence() {
        let mut text = String::new();
        let mut line = 0;
        for len in 1..=6usize {
            for dense_mask in 0..1usize << len {
                for operators in 0..3usize.pow((len - 1) as u32) {
                    text.push_str("BOOL ");
                    let mut ops = operators;
                    for at in 0..len {
                        if at > 0 {
                            text.push_str([" AND ", " OR ", " NOT "][ops % 3]);
                            ops /= 3;
                        }
                        if dense_mask >> at & 1 != 0 {
                            text.push_str(if at % 2 == 0 { "a" } else { "b" });
                        } else {
                            text.push_str(&format!("r{line}p{at}"));
                        }
                    }
                    text.push('\n');
                    line += 1;
                }
            }
        }
        let plan = queries::compile(text.as_bytes());
        assert_eq!(plan.n_dense, 2);
        let mut words = vec![[0; 4]; plan.n_bool_terms as usize];
        let slots: Vec<u32> = (0..plan.n_bool_terms).collect();
        for (q, query) in plan.queries.iter().enumerate() {
            let mut terms = query.terms[..query.len as usize].to_vec();
            terms.sort_unstable();
            terms.dedup();
            for (at, &term) in terms.iter().enumerate() {
                let mask = (0..1usize << terms.len())
                    .filter(|&assignment| assignment >> at & 1 != 0)
                    .fold(0, |mask, assignment| mask | 1u64 << assignment);
                words[term as usize] = [mask, 0, 0, 0];
            }
            let actual = eval::evaluate(query, &words, &slots);
            let mut expected = 0;
            for assignment in 0..1usize << terms.len() {
                let present =
                    |term| assignment >> terms.iter().position(|&t| t == term).unwrap() & 1 != 0;
                let mut result = present(query.terms[0]);
                for at in 1..query.len as usize {
                    let value = present(query.terms[at]);
                    result = match query.ops[at - 1] {
                        queries::Op::And => result && value,
                        queries::Op::Or => result || value,
                        queries::Op::Not => result && !value,
                    };
                }
                expected |= u64::from(result) << assignment;
            }
            assert_eq!(actual, [expected, 0, 0, 0]);
            let triggered = terms
                .iter()
                .filter(|&&t| plan.triggers.get(t).contains(&(q as u32)))
                .fold(0, |bits, &t| bits | words[t as usize][0]);
            let folded = plan
                .forms
                .get(q)
                .map_or([0; 4], |form| eval::evaluate_form(form, &words));
            assert_eq!(
                (actual[0] ^ folded[0]) & !triggered,
                0,
                "a query must use its exact form or be triggered: {q}"
            );
            for &term in &terms {
                words[term as usize] = [0; 4];
            }
        }
        println!("checked {line} dense/rare and operator patterns, all truth assignments");
    }
}
