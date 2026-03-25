use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::ast::{File, Group, Line, Rule};

/// Sort rules within each group lexicographically by pattern, preserving
/// semantics by keeping conflicting rules (different owners + overlapping
/// patterns) in their original relative order.
pub fn sort_groups(file: &mut File) {
    for group in &mut file.groups {
        sort_group(group);
    }
}

/// An entry is a block of leading comment lines followed by a rule.
/// Comments move with their associated rule when sorting.
struct Entry {
    /// Comment lines that precede this rule.
    leading_comments: Vec<Line>,
    /// The rule line.
    rule: Rule,
}

/// Sort a single group's rules, keeping comments attached to their rules.
fn sort_group(group: &mut Group) {
    // Split lines into:
    // - header_comments: comments at the very top of the group (before any rule)
    // - entries: (leading comments + rule) pairs, where leading comments are
    //   comments between the previous rule and this one
    // - trailing_comments: comments after the last rule
    let mut header_comments: Vec<Line> = Vec::new();
    let mut entries: Vec<Entry> = Vec::new();
    let mut pending_comments: Vec<Line> = Vec::new();
    let mut seen_rule = false;

    for line in group.lines.drain(..) {
        match line {
            Line::Comment(_) => {
                if !seen_rule {
                    header_comments.push(line);
                } else {
                    pending_comments.push(line);
                }
            }
            Line::Rule(rule) => {
                seen_rule = true;
                entries.push(Entry {
                    leading_comments: std::mem::take(&mut pending_comments),
                    rule,
                });
            }
        }
    }

    // Any remaining comments have no rule after them (trailing comments).
    let trailing_comments = pending_comments;

    if entries.len() <= 1 {
        // Nothing to sort — reconstruct and return.
        reconstruct_with_header(group, header_comments, entries, trailing_comments);
        return;
    }

    // Build conflict graph: edge from i → j means i must come before j.
    // This happens when i was originally before j, they have different owners,
    // and their patterns may overlap.
    let n = entries.len();
    let mut in_degree = vec![0usize; n];
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];

    for i in 0..n {
        for j in (i + 1)..n {
            if entries[i].rule.owners != entries[j].rule.owners
                && may_overlap(&entries[i].rule.pattern, &entries[j].rule.pattern)
            {
                adj[i].push(j);
                in_degree[j] += 1;
            }
        }
    }

    // Sort key: (is_anchored, pattern, index).
    // Unanchored patterns (false) sort before anchored (true),
    // then lexicographically by pattern within each group.
    let sort_key = |i: usize| -> (bool, &str, usize) {
        let pattern = entries[i].rule.pattern.as_str();
        (!is_unanchored(pattern), pattern, i)
    };

    // Kahn's algorithm with a min-heap to get the lexicographically
    // smallest valid ordering that respects conflict constraints.
    let mut heap: BinaryHeap<Reverse<(bool, &str, usize)>> = BinaryHeap::new();
    for (i, &deg) in in_degree.iter().enumerate() {
        if deg == 0 {
            heap.push(Reverse(sort_key(i)));
        }
    }

    let mut sorted_indices: Vec<usize> = Vec::with_capacity(n);
    while let Some(Reverse((_, _, idx))) = heap.pop() {
        sorted_indices.push(idx);
        for &next in &adj[idx] {
            in_degree[next] -= 1;
            if in_degree[next] == 0 {
                heap.push(Reverse(sort_key(next)));
            }
        }
    }

    // Reorder entries according to sorted_indices.
    // We need to move entries out since they're not Copy.
    let mut entry_slots: Vec<Option<Entry>> = entries.into_iter().map(Some).collect();
    let mut sorted_entries: Vec<Entry> = Vec::with_capacity(n);
    for idx in sorted_indices {
        sorted_entries.push(entry_slots[idx].take().unwrap());
    }

    reconstruct_with_header(group, header_comments, sorted_entries, trailing_comments);
}

fn reconstruct_with_header(
    group: &mut Group,
    header_comments: Vec<Line>,
    entries: Vec<Entry>,
    trailing_comments: Vec<Line>,
) {
    group.lines.extend(header_comments);
    for entry in entries {
        group.lines.extend(entry.leading_comments);
        group.lines.push(Line::Rule(entry.rule));
    }
    group.lines.extend(trailing_comments);
}

/// Conservatively determine if two CODEOWNERS patterns could match the same file.
///
/// Returns true if the patterns might overlap (erring on the side of caution).
/// False positives (claiming overlap when there is none) are safe — they just
/// prevent sorting. False negatives would be a bug.
fn may_overlap(a: &str, b: &str) -> bool {
    // If either uses **, it can match at any depth — assume overlap.
    if a.contains("**") || b.contains("**") {
        return true;
    }

    // If both are unanchored, check if they could match the same file.
    if is_unanchored(a) && is_unanchored(b) {
        // Two bare literal filenames (no wildcards, no trailing slash) can only
        // match the same file if they're literally identical.
        // e.g., `Makefile` and `.git-blame-ignore-revs` can never overlap.
        let a_is_literal = !a.contains(['*', '?', '[']) && !a.ends_with('/');
        let b_is_literal = !b.contains(['*', '?', '[']) && !b.ends_with('/');
        if a_is_literal && b_is_literal {
            return a == b;
        }
        // Otherwise (wildcards or directories), be conservative.
        return true;
    }

    // If only one is unanchored, it could match anywhere — assume overlap.
    if is_unanchored(a) || is_unanchored(b) {
        return true;
    }

    // Both patterns are anchored. Check if their directory trees overlap
    // by comparing the non-wildcard prefixes.
    let a_prefix = anchor_prefix(a);
    let b_prefix = anchor_prefix(b);

    a_prefix.starts_with(b_prefix) || b_prefix.starts_with(a_prefix)
}

/// A pattern is "unanchored" if it has no `/` in the beginning or middle.
/// Per gitignore rules, such patterns match files anywhere in the tree.
fn is_unanchored(pattern: &str) -> bool {
    if pattern.starts_with('/') {
        return false;
    }
    let base = pattern.strip_suffix('/').unwrap_or(pattern);
    !base.contains('/')
}

/// Get the directory prefix of an anchored pattern, up to the first wildcard.
/// This is used to compare whether two patterns operate in the same directory tree.
///
/// Examples:
///   `/src/sentry/api/` → `src/sentry/api/`
///   `/src/sentry/api/endpoints/relay/` → `src/sentry/api/endpoints/relay/`
///   `/bin/mock*` → `bin/`
///   `/src/sentry/snuba/metrics/query.py` → `src/sentry/snuba/metrics/`
fn anchor_prefix(pattern: &str) -> &str {
    // Strip leading `/`.
    let p = pattern.strip_prefix('/').unwrap_or(pattern);

    // Find the first wildcard character.
    let wildcard_pos = p.find(['*', '?', '[']);
    let before_wildcard = match wildcard_pos {
        Some(pos) => &p[..pos],
        None => p,
    };

    // Return up to and including the last `/` before the wildcard (the directory part).
    match before_wildcard.rfind('/') {
        Some(pos) => &p[..=pos],
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast;

    #[test]
    fn test_is_unanchored() {
        assert!(is_unanchored("*.js"));
        assert!(is_unanchored("Makefile"));
        assert!(!is_unanchored("/src/foo/"));
        assert!(!is_unanchored("src/foo/bar.py"));
        assert!(!is_unanchored("/Makefile"));
    }

    #[test]
    fn test_anchor_prefix() {
        assert_eq!(anchor_prefix("/src/sentry/api/"), "src/sentry/api/");
        assert_eq!(anchor_prefix("/bin/mock*"), "bin/");
        assert_eq!(
            anchor_prefix("/src/sentry/snuba/metrics/query.py"),
            "src/sentry/snuba/metrics/"
        );
        assert_eq!(anchor_prefix("*.js"), "");
        assert_eq!(anchor_prefix("/Makefile"), "");
    }

    #[test]
    fn test_may_overlap_same_tree() {
        // Same directory tree → overlap
        assert!(may_overlap(
            "/src/sentry/api/",
            "/src/sentry/api/endpoints/relay/"
        ));
        // Different trees → no overlap
        assert!(!may_overlap("/src/sentry/api/", "/tests/sentry/relay/"));
    }

    #[test]
    fn test_may_overlap_wildcards() {
        // ** always overlaps
        assert!(may_overlap("**/logs", "/src/foo/"));
        // Unanchored glob overlaps with everything
        assert!(may_overlap("*.js", "/src/foo.py"));
    }

    #[test]
    fn test_may_overlap_unanchored_literals() {
        // Two different bare filenames can never match the same file.
        assert!(!may_overlap("Makefile", ".git-blame-ignore-revs"));
        assert!(!may_overlap("Makefile", ".envrc"));
        // Same bare filename does overlap (trivially).
        assert!(may_overlap("Makefile", "Makefile"));
        // Wildcard unanchored still overlaps conservatively.
        assert!(may_overlap("*.js", "Makefile"));
        // Unanchored directory patterns overlap conservatively.
        assert!(may_overlap("docs/", "logs/"));
    }

    #[test]
    fn test_may_overlap_disjoint_anchored() {
        assert!(!may_overlap("/src/sentry/api/", "/src/sentry/relay/"));
        assert!(!may_overlap("/tests/foo/", "/src/foo/"));
    }

    #[test]
    fn test_sort_same_owners() {
        let input = "/src/z/ @team\n/src/a/ @team\n/src/m/ @team\n";
        let mut file = ast::parse(input);
        sort_groups(&mut file);

        let rules: Vec<&str> = file.groups[0]
            .lines
            .iter()
            .filter_map(|l| match l {
                Line::Rule(r) => Some(r.pattern.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(rules, vec!["/src/a/", "/src/m/", "/src/z/"]);
    }

    #[test]
    fn test_sort_different_owners_no_overlap() {
        // Different owners but different directory trees → safe to sort.
        let input = "/tests/z/ @team-test\n/src/a/ @team-src\n";
        let mut file = ast::parse(input);
        sort_groups(&mut file);

        let rules: Vec<&str> = file.groups[0]
            .lines
            .iter()
            .filter_map(|l| match l {
                Line::Rule(r) => Some(r.pattern.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(rules, vec!["/src/a/", "/tests/z/"]);
    }

    #[test]
    fn test_sort_preserves_override_order() {
        // /src/foo/ comes before /src/foo/bar.py with different owners.
        // The specific rule overrides the general one — order must be preserved.
        let input = "/src/foo/ @team-general\n/src/foo/bar.py @team-specific\n";
        let mut file = ast::parse(input);
        sort_groups(&mut file);

        let rules: Vec<&str> = file.groups[0]
            .lines
            .iter()
            .filter_map(|l| match l {
                Line::Rule(r) => Some(r.pattern.as_str()),
                _ => None,
            })
            .collect();
        // Must stay in original order since they overlap and have different owners.
        assert_eq!(rules, vec!["/src/foo/", "/src/foo/bar.py"]);
    }

    #[test]
    fn test_sort_mixed_overlap_and_independent() {
        // Three rules: two overlap (must keep order), one is independent (can sort).
        let input = "/src/z/ @team-a\n/src/z/specific.py @team-b\n/lib/a/ @team-c\n";
        let mut file = ast::parse(input);
        sort_groups(&mut file);

        let rules: Vec<&str> = file.groups[0]
            .lines
            .iter()
            .filter_map(|l| match l {
                Line::Rule(r) => Some(r.pattern.as_str()),
                _ => None,
            })
            .collect();
        // /lib/a/ can move to front (independent, sorts first).
        // /src/z/ must stay before /src/z/specific.py (overlap + different owners).
        assert_eq!(rules, vec!["/lib/a/", "/src/z/", "/src/z/specific.py"]);
    }

    #[test]
    fn test_sort_preserves_comments() {
        let input = "## Header\n/src/z/ @team\n# About a\n/src/a/ @team\n";
        let mut file = ast::parse(input);
        sort_groups(&mut file);

        let lines: Vec<String> = file.groups[0]
            .lines
            .iter()
            .map(|l| match l {
                Line::Comment(c) => c.text.clone(),
                Line::Rule(r) => r.pattern.clone(),
            })
            .collect();
        // "## Header" stays pinned at the top (it's before any rule).
        // "# About a" is attached to /src/a/ and moves with it.
        assert_eq!(lines, vec!["## Header", "# About a", "/src/a/", "/src/z/"]);
    }

    #[test]
    fn test_sort_trailing_comments() {
        let input = "/src/z/ @team\n/src/a/ @team\n# trailing\n";
        let mut file = ast::parse(input);
        sort_groups(&mut file);

        let lines: Vec<String> = file.groups[0]
            .lines
            .iter()
            .map(|l| match l {
                Line::Comment(c) => c.text.clone(),
                Line::Rule(r) => r.pattern.clone(),
            })
            .collect();
        assert_eq!(lines, vec!["/src/a/", "/src/z/", "# trailing"]);
    }

    #[test]
    fn test_sort_unanchored_before_anchored() {
        // Unanchored patterns (no leading /) should sort before anchored ones.
        let input = "/src/z/ @team\n*.js @team\nMakefile @team\n/src/a/ @team\n";
        let mut file = ast::parse(input);
        sort_groups(&mut file);

        let rules: Vec<&str> = file.groups[0]
            .lines
            .iter()
            .filter_map(|l| match l {
                Line::Rule(r) => Some(r.pattern.as_str()),
                _ => None,
            })
            .collect();
        // Unanchored first (sorted among themselves), then anchored (sorted).
        assert_eq!(rules, vec!["*.js", "Makefile", "/src/a/", "/src/z/"]);
    }

    #[test]
    fn test_sort_unanchored_before_anchored_mixed_owners() {
        // *.py was originally after /src/b/ with different owners and overlapping
        // patterns, so it must stay after /src/b/ (moving it before would change
        // who owns src/b/*.py files). /src/a/ and /src/b/ are disjoint and free to sort.
        let input = "/src/b/ @team-b\n*.py @team-a\n/src/a/ @team-a\n";
        let mut file = ast::parse(input);
        sort_groups(&mut file);

        let rules: Vec<&str> = file.groups[0]
            .lines
            .iter()
            .filter_map(|l| match l {
                Line::Rule(r) => Some(r.pattern.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(rules, vec!["/src/a/", "/src/b/", "*.py"]);
    }
}
