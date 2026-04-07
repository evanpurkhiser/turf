use crate::ast::{File, Group, Line};

/// Maximum global alignment column. The global column is the largest natural
/// column across all groups, but capped at this value. Groups that exceed it
/// fall back to their own natural alignment.
const MAX_OWNER_COLUMN: usize = 90;

/// Format a CODEOWNERS AST back into a string.
///
/// Formatting rules:
/// - A global alignment column is computed as the largest natural column across
///   all groups, capped at 90. Groups that fit within this use the global column.
/// - Groups whose longest pattern exceeds the global column align to their own
///   natural column (longest pattern + 1 space) instead.
/// - Groups are separated by exactly one blank line.
/// - Comments are preserved as-is.
/// - Inline comments are separated from the last owner by one space.
/// - Multiple consecutive blank lines in the source are collapsed to one.
pub fn format(file: &File) -> String {
    // Find the global column: max natural column across all groups, capped.
    let global_column = file
        .groups
        .iter()
        .filter_map(natural_owner_column)
        .filter(|&col| col <= MAX_OWNER_COLUMN)
        .max()
        // When no group fits under the cap (all oversized, or no owned rules),
        // each group will fall back to its own natural column in format_group.
        .unwrap_or(0);

    let mut output = String::new();

    for (i, group) in file.groups.iter().enumerate() {
        if i > 0 {
            output.push('\n');
        }
        format_group(group, global_column, &mut output);
    }

    // Ensure file ends with a newline.
    if !output.ends_with('\n') {
        output.push('\n');
    }

    output
}

/// Compute the natural owner column for a group (longest pattern + 1 space).
/// Returns `None` if the group has no rules with owners.
fn natural_owner_column(group: &Group) -> Option<usize> {
    group
        .lines
        .iter()
        .filter_map(|line| match line {
            Line::Rule(rule) if !rule.owners.is_empty() => Some(rule.pattern.len()),
            _ => None,
        })
        .max()
        .map(|len| len + 1)
}

fn format_group(group: &Group, global_column: usize, output: &mut String) {
    // Use the global column, unless this group's longest pattern exceeds it.
    let owner_column = natural_owner_column(group)
        .filter(|&col| col > global_column)
        .unwrap_or(global_column);

    for line in &group.lines {
        match line {
            Line::Comment(comment) => {
                output.push_str(&comment.text);
                output.push('\n');
            }
            Line::Rule(rule) => {
                output.push_str(&rule.pattern);

                if !rule.owners.is_empty() {
                    // Pad to the owner column.
                    let padding = owner_column.saturating_sub(rule.pattern.len()).max(1);
                    output.extend(std::iter::repeat_n(' ', padding));
                    output.push_str(&rule.owners.join(" "));
                }

                if let Some(comment) = &rule.inline_comment {
                    output.push(' ');
                    output.push_str(comment);
                }

                output.push('\n');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::parse;

    #[test]
    fn test_format_alignment() {
        // Global column = max natural column = 21 (from /src/very/long/path/ + 1).
        let input = "/src/ @team1\n/src/very/long/path/ @team2\n/x/ @team3\n";
        let file = parse(input);
        let output = format(&file);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "/src/                @team1");
        assert_eq!(lines[1], "/src/very/long/path/ @team2");
        assert_eq!(lines[2], "/x/                  @team3");
    }

    #[test]
    fn test_format_preserves_comments() {
        let input = "# Section header\n## Sub-header\n/src/ @team1\n";
        let file = parse(input);
        let output = format(&file);
        assert!(output.starts_with("# Section header\n## Sub-header\n"));
    }

    #[test]
    fn test_format_collapses_blank_lines() {
        let input = "/src/ @team1\n\n\n\n/lib/ @team2\n";
        let file = parse(input);
        let output = format(&file);
        let lines: Vec<&str> = output.lines().collect();
        // Both groups share the same global column, separated by one blank line.
        assert_eq!(lines[0].find('@'), lines[2].find('@'));
        assert_eq!(lines[1], "");
    }

    #[test]
    fn test_format_inline_comment() {
        let input = "*.js @owner #This is a comment\n";
        let file = parse(input);
        let output = format(&file);
        assert_eq!(output, "*.js @owner #This is a comment\n");
    }

    #[test]
    fn test_format_no_owners() {
        let input = "/apps/github\n";
        let file = parse(input);
        let output = format(&file);
        assert_eq!(output, "/apps/github\n");
    }

    #[test]
    fn test_format_multiple_owners_aligned() {
        let input = "/short @team1 @team2\n/a/very/long/path/here @team3\n";
        let file = parse(input);
        let output = format(&file);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "/short                 @team1 @team2");
        assert_eq!(lines[1], "/a/very/long/path/here @team3");
    }

    #[test]
    fn test_format_separate_groups() {
        // Two groups with short patterns share the same global column.
        let input = "# Group 1\n/src/ @team1\n\n# Group 2\n/lib/ @team2\n";
        let file = parse(input);
        let output = format(&file);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "# Group 1");
        assert_eq!(lines[2], "");
        assert_eq!(lines[3], "# Group 2");
        // Both rules align at the same column.
        assert_eq!(lines[1].find('@'), lines[4].find('@'));
    }

    #[test]
    fn test_roundtrip_already_formatted() {
        let input = "# Header\n/src/ @team1\n/lib/ @team2\n";
        let file = parse(input);
        let output = format(&file);
        let file2 = parse(&output);
        let output2 = format(&file2);
        assert_eq!(output, output2, "formatting should be idempotent");
    }

    #[test]
    fn test_format_alignment_with_ownerless_rule_in_group() {
        let input = "/apps/github\n/src/very/long/path/ @team1\n/x/ @team2\n";
        let file = parse(input);
        let output = format(&file);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "/apps/github");
        assert_eq!(lines[1], "/src/very/long/path/ @team1");
        assert_eq!(lines[2], "/x/                  @team2");
    }

    #[test]
    fn test_format_global_column_capped_at_max() {
        // With a large file the global column should cap at MAX_OWNER_COLUMN.
        // Group 1 has a long (but under 90) pattern, group 2 has short patterns.
        let pat = "/".to_string() + &"a".repeat(85) + "/";
        let input = format!("{} @team1\n\n/short/ @team2\n", pat);
        let file = parse(&input);
        let output = format(&file);
        let lines: Vec<&str> = output.lines().collect();
        // Global column = 88 (87-char pattern + 1), both groups use it.
        let col = lines[0].find('@').unwrap();
        assert_eq!(col, 88);
        assert_eq!(lines[2].find('@').unwrap(), 88);
    }

    #[test]
    fn test_format_oversized_group_aligns_naturally() {
        // When a group's longest pattern exceeds MAX_OWNER_COLUMN, that group
        // falls back to natural alignment while others use the global column.
        let long_pattern = "/".to_string() + &"a".repeat(95);
        let input = format!("{} @team1\n\n/short/ @team2\n", long_pattern);
        let file = parse(&input);
        let output = format(&file);
        let lines: Vec<&str> = output.lines().collect();
        // Oversized group aligns naturally.
        let col = lines[0].find('@').unwrap();
        assert_eq!(col, 97); // 96-char pattern + 1 space
        // Short group uses the global column (just /short/ = 8).
        let short_col = lines[2].find('@').unwrap();
        assert_eq!(short_col, 8);
    }

    #[test]
    fn test_format_all_groups_oversized() {
        // When every group exceeds MAX_OWNER_COLUMN, each aligns naturally.
        let pat1 = "/".to_string() + &"a".repeat(95);
        let pat2 = "/".to_string() + &"b".repeat(100);
        let input = format!("{} @team1\n\n{} @team2\n/short/ @team3\n", pat1, pat2);
        let file = parse(&input);
        let output = format(&file);
        let lines: Vec<&str> = output.lines().collect();
        // Group 1 aligns at 97.
        assert_eq!(lines[0].find('@').unwrap(), 97);
        // Group 2's longest is pat2 (102 chars), so aligns at 102.
        assert_eq!(lines[2].find('@').unwrap(), 102);
        assert_eq!(lines[3].find('@').unwrap(), 102);
    }

    #[test]
    fn test_format_comment_only_group() {
        let input = "# Just a comment\n\n/src/ @team1\n";
        let file = parse(input);
        let output = format(&file);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "# Just a comment");
        assert!(lines[2].contains("@team1"));
    }

    #[test]
    fn test_format_empty_file() {
        let input = "";
        let file = parse(input);
        let output = format(&file);
        assert_eq!(output, "\n");
    }

    #[test]
    fn test_format_sort_then_format_idempotent() {
        // Full pipeline: parse -> sort -> format should be idempotent on second pass.
        let input = "/src/z/ @team\n/src/a/ @team\n/lib/ @other\n";
        let mut file = parse(input);
        crate::sorter::sort_groups(&mut file);
        let output = format(&file);

        let mut file2 = parse(&output);
        crate::sorter::sort_groups(&mut file2);
        let output2 = format(&file2);
        assert_eq!(output, output2, "sort + format should be idempotent");
    }
}
