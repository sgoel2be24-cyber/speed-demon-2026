"""Original optimisation experiments, applied only to disposable benchmark copies."""
from pathlib import Path


def apply(folder, variant):
    folder = Path(folder)
    scan = (folder / "src/scan.rs").read_text()
    evaluation = (folder / "src/eval.rs").read_text()
    if variant in {"shape", "staged-shape", "columns-hash-shape"}:
        queries = folder / "src/queries.rs"
        source = queries.read_text()
        source = source.replace("Vec<(bool, u32, Reverse<u64>)>", "Vec<(u8, u32, Reverse<u64>)>")
        source = source.replace("form.ops\n                    .iter()", "form.ops[..form.len.saturating_sub(1) as usize]\n                    .iter()")
        source = source.replace("""            } else {
                0
            };
            (!is_folded[line], shape, Reverse(evaluations[line]))""",
        """            } else if query.is_phrase {
                0
            } else {
                query.ops[..query.len as usize - 1].iter()
                    .fold(query.len as u32, |shape, &op| shape * 3 + op as u32)
            };
            (u8::from(!is_folded[line]) + u8::from(query.is_phrase), shape, Reverse(evaluations[line]))""")
        queries.write_text(source)
    if variant in {"staged", "staged-shape", "swiss-staged"}:
        terms = folder / "src/terms.rs"
        terms.write_text(terms.read_text().replace("#[derive(Clone, Copy)]\npub struct Found", "#[derive(Clone, Copy, Default)]\npub struct Found"))
        scan = scan.replace("use crate::terms::Key;", "use crate::terms::{Found, Key};")
        marker = "/// Turns tokens into term ids"
        helper = """#[inline(never)]
fn lookup_tokens(plan: &Compiled, shard: &[u8], tokens: &[[u32; 2]], keys: &[(usize, Key)], found: &mut [Found]) {
    for (entry, (&[start, end], &(home, key))) in found.iter_mut().zip(tokens.iter().zip(keys)) {
        *entry = plan.table.lookup(home, key, shard, start as usize, end as usize);
    }
}

"""
        scan = scan.replace(marker, helper + marker)
        scan = scan.replace("""    shard: &[u8],
    tokens: &[[u32; 2]],
    keys: &[(usize, Key)],
    ids: &mut [u32],""", """    found: &[Found],
    ids: &mut [u32],""")
        scan = scan.replace("let (table, n_dense, n_bool_terms) = (&plan.table, plan.n_dense, plan.n_bool_terms);", "let (n_dense, n_bool_terms) = (plan.n_dense, plan.n_bool_terms);")
        old = """    for (position, ((id, &[start, end]), &(home, key))) in
        ids.iter_mut().zip(tokens).zip(keys).enumerate()
    {
        let found = table.lookup(home, key, shard, start as usize, end as usize);"""
        assert old in scan
        scan = scan.replace(old, """    for (position, (id, &found)) in ids.iter_mut().zip(found).enumerate() {""")
        scan = scan.replace("    let mut waiting = vec![];", "    let mut waiting = vec![];\n    let mut found = vec![];")
        scan = scan.replace("            keys.resize(tokens.len(), (0, Key::default()));", "            keys.resize(tokens.len(), (0, Key::default()));\n            found.resize(tokens.len(), Found::default());")
        scan = scan.replace("        hash_tokens(plan, shard, tokens, &mut keys);", "        hash_tokens(plan, shard, tokens, &mut keys);\n        lookup_tokens(plan, shard, tokens, &keys, &mut found);")
        scan = scan.replace("""            plan,
            shard,
            tokens,
            &keys,
            ids,""", """            plan,
            &found[..tokens.len()],
            ids,""")
    if variant in {"columns", "columns-hash", "columns-hash-shape"}:
        evaluation = evaluation.replace("pub first: Vec<[u16; MAX_IDS]>", "pub first: Vec<u16>")
        evaluation = evaluation.replace("first: vec![[0; MAX_IDS]; n_queries]", "first: vec![0; MAX_IDS * n_queries]")
        evaluation = evaluation.replace("let mut slots = self.first[query][seen..].iter_mut();", "let mut seen = seen;")
        evaluation = evaluation.replace("let Some(slot) = slots.next() else { return };\n                *slot = (block_start + i as u32 * 64 + rest.trailing_zeros()) as u16;",
            """if seen == MAX_IDS { return }
                self.first[seen * self.count.len() + query] = (block_start + i as u32 * 64 + rest.trailing_zeros()) as u16;
                seen += 1;""")
        evaluation = evaluation.replace("self.first[query][seen] = doc;", "self.first[seen * self.count.len() + query] = doc;")
        output = folder / "src/output.rs"
        output.write_text(output.read_text().replace("for &doc in &shard.first[q][..take] {",
            "for rank in 0..take {\n                let doc = shard.first[rank * shard.count.len() + q];"))
    if variant in {"hash64", "columns-hash", "columns-hash-shape"}:
        terms = folder / "src/terms.rs"
        terms.write_text(terms.read_text().replace(
            "self.0[0].wrapping_mul(A) ^ self.0[1].wrapping_mul(B)", "self.0[0].wrapping_mul(A)"))
    if variant in {"phase", "phase4", "phase32", "phasefast", "phasequery", "phasequery4", "phasequery8", "phasequery-swiss"}:
        main = (folder / "src/main.rs").read_text()
        begin = main.index("    let totals: Vec<_> = thread::scope")
        main = main[:begin] + '    fs::write(out.join("results.txt"), phase::run(&plan, docs, threads))\n}\n'
        main = main.replace("mod classify;", "mod phase;\nmod classify;")
        (folder / "src/main.rs").write_text(main)
        phase_source = Path(__file__).with_name("phase.rs").read_text()
        size = {"phase": 16384, "phase4": 4096, "phase32": 32768, "phasefast": 16384, "phasequery": 16384, "phasequery4": 4096, "phasequery8": 8192, "phasequery-swiss": 16384}[variant]
        phase_source = phase_source.replace("const PHASE: usize = 16384;", f"const PHASE: usize = {size};")
        if variant in {"phasefast", "phasequery", "phasequery4", "phasequery8", "phasequery-swiss"}:
            phase_source = phase_source.replace(
                "    for phase in (0..documents).step_by(PHASE) {",
                """    let dense_storage = crate::huge_slice(plan.n_dense as usize * (PHASE / 64), 0u64);
    for phase in (0..documents).step_by(PHASE) {""")
            phase_source = phase_source.replace(
                """        let stride = count.div_ceil(64);
        let workers = threads.max(1).min(stride);
        let mut dense = vec![0u64; plan.n_dense as usize * stride];""",
                """        let stride = PHASE / 64;
        let groups = count.div_ceil(512);
        let workers = threads.max(1).min(groups);
        let dense = &mut *dense_storage;
        dense.fill(0);""")
            phase_source = phase_source.replace("stride * worker / workers * 64", "groups * worker / workers * 512")
            phase_source = phase_source.replace("stride * (worker + 1) / workers * 64", "groups * (worker + 1) / workers * 512")
            begin = phase_source.index("fn stats(")
            end = phase_source.index("fn evaluate(", begin)
            stats = phase_source[begin:end]
            stats = stats.replace("Vec<u32>", "[u32; MAX_IDS]")
            stats = stats.replace("let mut first = Vec::with_capacity(MAX_IDS);", "let mut first = [0; MAX_IDS];\n    let mut seen = 0;")
            stats = stats.replace("first.len()", "seen")
            stats = stats.replace("first.push(base + i as u32 * 256 + j as u32 * 64 + rest.trailing_zeros());",
                                  "first[seen] = base + i as u32 * 256 + j as u32 * 64 + rest.trailing_zeros();\n                    seen += 1;")
            phase_source = phase_source[:begin] + stats + phase_source[end:]
        if variant in {"phasequery", "phasequery4", "phasequery8", "phasequery-swiss"}:
            old = """        let present = term_present(query.terms[i], doc, plan, dense, stride, offsets, postings);
        result = match query.ops[i - 1] {
            Op::And => result && present,
            Op::Or => result || present,
            Op::Not => result && !present,
        };"""
            assert old in phase_source
            phase_source = phase_source.replace(old, """        let present = || term_present(query.terms[i], doc, plan, dense, stride, offsets, postings);
        result = match query.ops[i - 1] {
            Op::And => result && present(),
            Op::Or => result || present(),
            Op::Not => result && !present(),
        };""")
            old = """        candidates.clear();
        for &term in &query.terms[..query.len as usize] {"""
            assert old in phase_source
            phase_source = phase_source.replace(old, """        candidates.clear();
        let mut cover = [0u32; 6];
        cover[0] = query.terms[0];
        let mut length = 1;
        let cost = |term: u32| {
            if term < plan.n_dense { usize::MAX / 8 }
            else { let t = (term - plan.n_dense) as usize; offsets[t + 1] - offsets[t] }
        };
        if q >= plan.forms.len() {
            for i in 1..query.len as usize {
                let term = query.terms[i];
                match query.ops[i - 1] {
                    Op::And => {
                        let current: usize = cover[..length].iter().map(|&t| cost(t)).sum();
                        if cost(term) < current { cover[0] = term; length = 1; }
                    }
                    Op::Or => { cover[length] = term; length += 1; }
                    Op::Not => {}
                }
            }
        }
        let watched = if q < plan.forms.len() || cover[..length].iter().any(|&t| t < plan.n_dense) {
            &query.terms[..query.len as usize]
        } else { &cover[..length] };
        for &term in watched {""")
        (folder / "src/phase.rs").write_text(phase_source)
        scan = scan.replace("fn find_token_edges(", "pub(crate) fn find_token_edges(")
        scan = scan.replace("fn hash_tokens(", "pub(crate) fn hash_tokens(")
        evaluation = evaluation.replace("fn tally(", "pub(crate) fn tally(")
        output = folder / "src/output.rs"
        output.write_text(output.read_text().replace("fn push_number(", "pub(crate) fn push_number("))
    if variant in {"nonzero", "combined"}:
        old = "totals.add_block(q, &evaluate_form(form, &self.words), start);"
        assert old in scan
        scan = scan.replace(old, """let result = evaluate_form(form, &self.words);
                if result != [0; 4] {
                    totals.add_block(q, &result, start);
                }""")
    if variant in {"sparse", "combined"}:
        old = "fn tally(word: &Word) -> (u32, u32) {\n    use std::arch::x86_64::*;"
        assert old in evaluation
        evaluation = evaluation.replace(old, """fn tally(word: &Word) -> (u32, u32) {
    let count: u32 = word.iter().map(|part| part.count_ones()).sum();
    if count <= 8 {
        let mut sum = 0;
        for (lane, &part) in word.iter().enumerate() {
            let mut rest = part;
            while rest != 0 {
                sum += lane as u32 * 64 + rest.trailing_zeros();
                rest &= rest - 1;
            }
        }
        return (count, sum);
    }
    use std::arch::x86_64::*;""")
    if variant in {"fused", "combined"}:
        scan = scan.replace(
            "fn find_token_edges(bytes: &[u8], mut at: usize, edges: &mut Vec<u32>) -> usize {\n    edges.clear();",
            """fn find_token_edges(plan: &Compiled, bytes: &[u8], mut at: usize,
                    edges: &mut Vec<u32>, keys: &mut Vec<(usize, Key)>) -> usize {
    edges.clear();
    keys.clear();""")
        old = "edges.push(at as u32 + changes.trailing_zeros());"
        assert old in scan
        scan = scan.replace(old, """edges.push(at as u32 + changes.trailing_zeros());
            if edges.len().is_multiple_of(2) {
                let start = edges[edges.len() - 2] as usize;
                let end = edges[edges.len() - 1] as usize;
                let key = Key::at(bytes, start, end - start);
                keys.push((plan.table.prefetch(key), key));
            }""")
        scan = scan.replace("find_token_edges(shard, tab + 1, &mut edges)",
                            "find_token_edges(plan, shard, tab + 1, &mut edges, &mut keys)")
        scan = scan.replace("            keys.resize(tokens.len(), (0, Key::default()));\n", "")
        scan = scan.replace("        hash_tokens(plan, shard, tokens, &mut keys);\n", "")
        begin = scan.index("/// Finds each token's home")
        end = scan.index("/// Turns tokens into term ids", begin)
        scan = scan[:begin] + scan[end:]
    if variant in {"swiss", "swiss-staged", "phasequery-swiss"}:
        terms = folder / "src/terms.rs"
        source = terms.read_text().split("/// Open-addressing hash table")[0]
        source = source.replace("#[derive(Clone, Copy, Default, PartialEq)]", "#[derive(Clone, Copy, Default, PartialEq, Eq)]")
        terms.write_text(source + Path(__file__).with_name("swiss.rs").read_text())
    if variant not in {"baseline", "nonzero", "sparse", "fused", "combined", "phase", "phase4", "phase32", "phasefast", "columns", "hash64", "columns-hash", "columns-hash-shape", "staged", "shape", "staged-shape", "swiss", "swiss-staged", "phasequery", "phasequery4", "phasequery8", "phasequery-swiss"}:
        raise ValueError(variant)
    (folder / "src/scan.rs").write_text(scan)
    (folder / "src/eval.rs").write_text(evaluation)
