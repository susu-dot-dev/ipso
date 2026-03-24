use anyhow::{bail, Result};
use diffy::{apply, create_patch, Patch};

/// Compute a unified diff between `original` and `patched`.
/// Returns `None` if the sources are identical.
pub fn compute_diff(original: &str, patched: &str) -> Option<String> {
    if original == patched {
        return None;
    }
    let patch = create_patch(original, patched);
    Some(patch.to_string())
}

/// Apply a diff string to the original source.
pub fn apply_diff(original: &str, diff: &str) -> Result<String> {
    let patch: Patch<str> = match Patch::from_str(diff) {
        Ok(p) => p,
        Err(e) => bail!("failed to parse diff: {}", e),
    };
    match apply(original, &patch) {
        Ok(result) => Ok(result),
        Err(e) => bail!("failed to apply diff: {}", e),
    }
}

/// Reconstruct the original source from a patched source and a diff.
/// The diff was computed as `original → patched`, so we reverse it.
pub fn reconstruct_original(patched: &str, diff: &str) -> Result<String> {
    // Parse the patch and reverse it: swap old/new hunks.
    let patch: Patch<str> = match Patch::from_str(diff) {
        Ok(p) => p,
        Err(e) => bail!("failed to parse diff for reconstruction: {}", e),
    };
    // Build reversed patch string by swapping --- / +++ and - / + lines.
    let fwd = patch.to_string();
    let reversed = reverse_patch_str(&fwd);
    let rev_patch: Patch<str> = match Patch::from_str(&reversed) {
        Ok(p) => p,
        Err(e) => bail!("failed to parse reversed diff: {}", e),
    };
    match apply(patched, &rev_patch) {
        Ok(result) => Ok(result),
        Err(e) => bail!("failed to apply reversed diff: {}", e),
    }
}

fn reverse_patch_str(patch: &str) -> String {
    let lines: Vec<&str> = patch.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;

    // Find the --- / +++ header pair and swap them.
    // Standard unified diff always has --- before +++.
    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("--- ") {
            // Look ahead for the matching +++ line.
            if i + 1 < lines.len() && lines[i + 1].starts_with("+++ ") {
                // Output in reversed order: --- (was +++) then +++ (was ---).
                out.push(lines[i + 1].replacen("+++ ", "--- ", 1));
                out.push(line.replacen("--- ", "+++ ", 1));
                i += 2;
            } else {
                out.push(line.replacen("--- ", "+++ ", 1));
                i += 1;
            }
            break;
        } else {
            out.push(line.to_string());
            i += 1;
        }
    }

    // Process the hunk body.
    // Within each contiguous block of +/- lines the reversal must output
    // the new `-` lines (originally `+`) before the new `+` lines
    // (originally `-`), so that the reconstructed patch applies cleanly.
    while i < lines.len() {
        let line = lines[i];
        if line.starts_with("@@ ") {
            out.push(reverse_hunk_header(line));
            i += 1;
        } else if line.starts_with('-') || line.starts_with('+') {
            // Collect a contiguous change block, then emit in reversed polarity order.
            let mut now_minus: Vec<String> = Vec::new(); // from original `+`
            let mut now_plus: Vec<String> = Vec::new(); // from original `-`
            while i < lines.len() && (lines[i].starts_with('-') || lines[i].starts_with('+')) {
                let l = lines[i];
                if l.starts_with('-') && !l.starts_with("---") {
                    now_plus.push(format!("+{}", &l[1..]));
                } else if l.starts_with('+') && !l.starts_with("+++") {
                    now_minus.push(format!("-{}", &l[1..]));
                }
                i += 1;
            }
            // Emit `-` lines first, then `+` lines.
            out.extend(now_minus);
            out.extend(now_plus);
        } else {
            // Context line or other content — pass through unchanged.
            out.push(line.to_string());
            i += 1;
        }
    }
    out.join("\n") + if patch.ends_with('\n') { "\n" } else { "" }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- compute_diff ---

    #[test]
    fn compute_diff_identical_returns_none() {
        assert_eq!(compute_diff("hello\n", "hello\n"), None);
    }

    #[test]
    fn compute_diff_both_empty_returns_none() {
        assert_eq!(compute_diff("", ""), None);
    }

    #[test]
    fn compute_diff_different_returns_some() {
        assert!(compute_diff("x = 1\n", "x = 2\n").is_some());
    }

    // --- apply_diff round-trip ---

    #[test]
    fn apply_diff_round_trip_single_line() {
        let original = "x = 1\n";
        let patched = "x = 2\n";
        let diff = compute_diff(original, patched).unwrap();
        assert_eq!(apply_diff(original, &diff).unwrap(), patched);
    }

    #[test]
    fn apply_diff_round_trip_multi_line() {
        let original = "def foo():\n    return 1\n";
        let patched = "def foo():\n    x = 42\n    return x\n";
        let diff = compute_diff(original, patched).unwrap();
        assert_eq!(apply_diff(original, &diff).unwrap(), patched);
    }

    #[test]
    fn apply_diff_from_empty_original() {
        let original = "";
        let patched = "hello\n";
        let diff = compute_diff(original, patched).unwrap();
        assert_eq!(apply_diff(original, &diff).unwrap(), patched);
    }

    #[test]
    fn apply_diff_malformed_input_returns_original_unchanged() {
        // diffy is permissive: a string with no hunk markers is treated as a
        // zero-change patch and the original is returned unmodified.
        let result = apply_diff("hello\n", "this is not a diff").unwrap();
        assert_eq!(result, "hello\n");
    }

    // --- reconstruct_original round-trip ---

    #[test]
    fn reconstruct_original_single_line_change() {
        let original = "x = 1\n";
        let patched = "x = 99\n";
        let diff = compute_diff(original, patched).unwrap();
        assert_eq!(reconstruct_original(patched, &diff).unwrap(), original);
    }

    #[test]
    fn reconstruct_original_multi_line_change() {
        let original = "a\nb\nc\n";
        let patched = "a\nB\nc\n";
        let diff = compute_diff(original, patched).unwrap();
        assert_eq!(reconstruct_original(patched, &diff).unwrap(), original);
    }

    #[test]
    fn reconstruct_original_multi_hunk() {
        // Changes far apart produce multiple hunks.
        let original = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
        let patched = "a\nB\nc\nd\ne\nf\ng\nh\nI\nj\n";
        let diff = compute_diff(original, patched).unwrap();
        assert_eq!(reconstruct_original(patched, &diff).unwrap(), original);
    }

    #[test]
    fn reconstruct_original_malformed_diff_returns_original_unchanged() {
        // Same permissive behaviour as apply_diff.
        let result = reconstruct_original("hello\n", "not a diff").unwrap();
        assert_eq!(result, "hello\n");
    }

    // --- apply then reconstruct is identity ---

    #[test]
    fn apply_then_reconstruct_is_identity() {
        let original = "def foo(x):\n    return x + 1\n";
        let patched = "def foo(x):\n    result = x + 1\n    return result\n";
        let diff = compute_diff(original, patched).unwrap();
        let applied = apply_diff(original, &diff).unwrap();
        let recovered = reconstruct_original(&applied, &diff).unwrap();
        assert_eq!(applied, patched);
        assert_eq!(recovered, original);
    }
}

fn reverse_hunk_header(line: &str) -> String {
    // @@ -a,b +c,d @@  →  @@ -c,d +a,b @@
    let inner = line.trim_start_matches('@').trim_end_matches('@').trim();
    let parts: Vec<&str> = inner.split_whitespace().collect();
    if parts.len() >= 2 {
        let old = parts[0]; // -a,b
        let new = parts[1]; // +c,d
        let reversed_old = format!("-{}", &new[1..]); // drop leading +
        let reversed_new = format!("+{}", &old[1..]); // drop leading -
        let rest: String = if parts.len() > 2 {
            format!(" {}", parts[2..].join(" "))
        } else {
            String::new()
        };
        format!("@@ {} {}{} @@", reversed_old, reversed_new, rest)
    } else {
        line.to_string()
    }
}
