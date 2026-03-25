use crate::ast::{File, Group, Line};

/// Format a CODEOWNERS AST back into a string.
///
/// Formatting rules:
/// - Within each group, rule lines are column-aligned: the owner column starts
///   at the same position (1 space after the longest pattern in the group).
/// - Groups are separated by exactly one blank line.
/// - Comments are preserved as-is.
/// - Inline comments are separated from the last owner by one space.
/// - Multiple consecutive blank lines in the source are collapsed to one.
pub fn format(file: &File) -> String {
    let mut output = String::new();

    for (i, group) in file.groups.iter().enumerate() {
        if i > 0 {
            output.push('\n');
        }
        format_group(group, &mut output);
    }

    // Ensure file ends with a newline.
    if !output.ends_with('\n') {
        output.push('\n');
    }

    output
}

fn format_group(group: &Group, output: &mut String) {
    // First pass: find the longest pattern in this group's rules.
    let max_pattern_len = group
        .lines
        .iter()
        .filter_map(|line| match line {
            Line::Rule(rule) if !rule.owners.is_empty() => Some(rule.pattern.len()),
            _ => None,
        })
        .max()
        .unwrap_or(0);

    // The owner column starts at max_pattern_len + padding.
    // Use at least 1 space of padding, but pad to a consistent column.
    let owner_column = max_pattern_len + 1;

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
        let input = "/src/ @team1\n/src/very/long/path/ @team2\n/x/ @team3\n";
        let file = parse(input);
        let output = format(&file);
        // All owner columns should align.
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
        assert_eq!(output, "/src/ @team1\n\n/lib/ @team2\n");
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
        let input = "# Group 1\n/src/ @team1\n\n# Group 2\n/lib/ @team2\n";
        let file = parse(input);
        let output = format(&file);
        assert_eq!(
            output,
            "# Group 1\n/src/ @team1\n\n# Group 2\n/lib/ @team2\n"
        );
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
        // A rule with no owners should NOT affect the alignment column for other rules.
        let input = "/apps/github\n/src/very/long/path/ @team1\n/x/ @team2\n";
        let file = parse(input);
        let output = format(&file);
        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(lines[0], "/apps/github");
        assert_eq!(lines[1], "/src/very/long/path/ @team1");
        assert_eq!(lines[2], "/x/                  @team2");
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
