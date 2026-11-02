"""Original optimisation experiments, applied only to disposable benchmark copies."""
from pathlib import Path


def apply(folder, variant):
    folder = Path(folder)
    scan = (folder / "src/scan.rs").read_text()
    evaluation = (folder / "src/eval.rs").read_text()
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
    if variant not in {"baseline", "nonzero", "sparse", "fused", "combined"}:
        raise ValueError(variant)
    (folder / "src/scan.rs").write_text(scan)
    (folder / "src/eval.rs").write_text(evaluation)
