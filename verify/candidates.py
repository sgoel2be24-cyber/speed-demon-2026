"""Original optimisation experiments, applied only to disposable benchmark copies."""
from pathlib import Path


def apply(folder, variant):
    folder = Path(folder)
    scan = (folder / "src/scan.rs").read_text()
    evaluation = (folder / "src/eval.rs").read_text()
    if variant in {"phase", "phase4", "phase32"}:
        main = (folder / "src/main.rs").read_text()
        begin = main.index("    let totals: Vec<_> = thread::scope")
        main = main[:begin] + '    fs::write(out.join("results.txt"), phase::run(&plan, docs, threads))\n}\n'
        main = main.replace("mod classify;", "mod phase;\nmod classify;")
        (folder / "src/main.rs").write_text(main)
        phase_source = Path(__file__).with_name("phase.rs").read_text()
        size = {"phase": 16384, "phase4": 4096, "phase32": 32768}[variant]
        phase_source = phase_source.replace("const PHASE: usize = 16384;", f"const PHASE: usize = {size};")
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
    if variant not in {"baseline", "nonzero", "sparse", "fused", "combined", "phase", "phase4", "phase32"}:
        raise ValueError(variant)
    (folder / "src/scan.rs").write_text(scan)
    (folder / "src/eval.rs").write_text(evaluation)
