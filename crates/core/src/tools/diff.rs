//! Unified diffs of whole files, for previews and edit results.

/// Lines of context around each change, as in `diff -u`.
const CONTEXT: usize = 3;
/// Beyond this many line pairs in the changed region, the region is shown
/// as removed and added whole instead of searching for the smallest diff.
const MAX_COMPARED_CELLS: usize = 4_000_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Same,
    Removed,
    Added,
}

/// Counts of added and removed lines, and the diff text with `---`/`+++`
/// headers and `@@` hunks. `None` stands for a missing file.
pub(crate) struct FileDiff {
    pub added: usize,
    pub removed: usize,
    pub text: String,
}

pub(crate) fn unified(path: &str, before: Option<&str>, after: Option<&str>) -> FileDiff {
    let old = before.map(split).unwrap_or_default();
    let new = after.map(split).unwrap_or_default();
    let ops = diff_ops(&old, &new);
    let added = ops.iter().filter(|op| **op == Op::Added).count();
    let removed = ops.iter().filter(|op| **op == Op::Removed).count();
    let mut text = format!(
        "--- {}\n+++ {}\n",
        if before.is_some() { path } else { "/dev/null" },
        if after.is_some() { path } else { "/dev/null" }
    );
    // Line indices in old and new where each op starts.
    let mut positions = Vec::with_capacity(ops.len());
    let (mut at_old, mut at_new) = (0, 0);
    for op in &ops {
        positions.push((at_old, at_new));
        match op {
            Op::Same => {
                at_old += 1;
                at_new += 1;
            }
            Op::Removed => at_old += 1,
            Op::Added => at_new += 1,
        }
    }
    let changed = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| **op != Op::Same)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let mut index = 0;
    while index < changed.len() {
        let start = changed[index].saturating_sub(CONTEXT);
        let mut end = changed[index];
        // Changes closer than twice the context share a hunk.
        while index + 1 < changed.len() && changed[index + 1] <= end + 2 * CONTEXT + 1 {
            index += 1;
            end = changed[index];
        }
        let end = (end + CONTEXT + 1).min(ops.len());
        let (old_start, new_start) = positions[start];
        let old_count = ops[start..end]
            .iter()
            .filter(|op| **op != Op::Added)
            .count();
        let new_count = ops[start..end]
            .iter()
            .filter(|op| **op != Op::Removed)
            .count();
        text.push_str(&format!(
            "@@ -{} +{} @@\n",
            range(old_start, old_count),
            range(new_start, new_count)
        ));
        for (op, (old_line, new_line)) in ops[start..end].iter().zip(&positions[start..end]) {
            let (prefix, line) = match op {
                Op::Same => (' ', old[*old_line]),
                Op::Removed => ('-', old[*old_line]),
                Op::Added => ('+', new[*new_line]),
            };
            text.push(prefix);
            text.push_str(line.strip_suffix('\n').unwrap_or(line));
            text.push('\n');
            if !line.ends_with('\n') {
                text.push_str("\\ No newline at end of file\n");
            }
        }
        index += 1;
    }
    FileDiff {
        added,
        removed,
        text,
    }
}

/// `start,count` as `diff -u` writes it, 1-based; an empty range names the
/// line before it.
fn range(start: usize, count: usize) -> String {
    match count {
        0 => format!("{start},0"),
        1 => format!("{}", start + 1),
        _ => format!("{},{count}", start + 1),
    }
}

fn split(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

/// The edit script from `old` to `new`: equal ends are matched directly,
/// the middle by longest common subsequence.
fn diff_ops(old: &[&str], new: &[&str]) -> Vec<Op> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old_mid = &old[prefix..old.len() - suffix];
    let new_mid = &new[prefix..new.len() - suffix];
    let mut ops = vec![Op::Same; prefix];
    if old_mid.len().saturating_mul(new_mid.len()) > MAX_COMPARED_CELLS {
        ops.extend(std::iter::repeat_n(Op::Removed, old_mid.len()));
        ops.extend(std::iter::repeat_n(Op::Added, new_mid.len()));
    } else {
        ops.extend(lcs_ops(old_mid, new_mid));
    }
    ops.extend(std::iter::repeat_n(Op::Same, suffix));
    ops
}

fn lcs_ops(old: &[&str], new: &[&str]) -> Vec<Op> {
    let width = new.len() + 1;
    // lengths[i * width + j]: common subsequence length of old[i..], new[j..].
    let mut lengths = vec![0u32; (old.len() + 1) * width];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            lengths[i * width + j] = if old[i] == new[j] {
                lengths[(i + 1) * width + j + 1] + 1
            } else {
                lengths[(i + 1) * width + j].max(lengths[i * width + j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut ops = Vec::with_capacity(old.len() + new.len());
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            ops.push(Op::Same);
            i += 1;
            j += 1;
        } else if i < old.len()
            && (j == new.len() || lengths[(i + 1) * width + j] >= lengths[i * width + j + 1])
        {
            // Removed lines come before added ones, as in `diff -u`.
            ops.push(Op::Removed);
            i += 1;
        } else {
            ops.push(Op::Added);
            j += 1;
        }
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_changed_regions_appear_with_three_lines_of_context() {
        let before = (1..=40).map(|n| format!("{n}\n")).collect::<String>();
        let after = (1..=41)
            .filter(|n| *n != 30)
            .map(|n| match n {
                5 => "five\n".to_string(),
                n => format!("{n}\n"),
            })
            .collect::<String>();
        let diff = unified("f", Some(&before), Some(&after));
        assert_eq!((diff.added, diff.removed), (2, 2));
        assert_eq!(
            diff.text,
            "--- f\n+++ f\n@@ -2,7 +2,7 @@\n 2\n 3\n 4\n-5\n+five\n 6\n 7\n 8\n@@ -27,7 +27,6 @@\n 27\n 28\n 29\n-30\n 31\n 32\n 33\n@@ -38,3 +37,4 @@\n 38\n 39\n 40\n+41\n"
        );
    }

    #[test]
    fn new_and_deleted_files_and_missing_final_newlines_are_marked() {
        let added = unified("n", None, Some("a\nb"));
        assert_eq!(
            added.text,
            "--- /dev/null\n+++ n\n@@ -0,0 +1,2 @@\n+a\n+b\n\\ No newline at end of file\n"
        );
        let deleted = unified("d", Some("x\n"), None);
        assert_eq!(deleted.text, "--- d\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n");
        assert_eq!(
            unified("s", Some("same\n"), Some("same\n")).text,
            "--- s\n+++ s\n"
        );
    }
}
