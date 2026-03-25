/// AST representation of a CODEOWNERS file.
///
/// The file is modeled as a sequence of groups separated by blank lines.
/// Each group contains lines, which can be comments or rules.
/// Comments preserve their exact text. Rules have a pattern and one or more owners,
/// plus an optional inline comment.

#[derive(Debug, Clone, PartialEq)]
pub struct File {
    /// Groups of lines, separated by blank lines in the source.
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    Comment(Comment),
    Rule(Rule),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    /// The full comment text including the leading `#`.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    /// The file pattern (e.g., `/src/sentry/api/` or `*.js`).
    pub pattern: String,
    /// One or more owners (e.g., `@org/team` or `user@example.com`).
    pub owners: Vec<String>,
    /// Optional inline comment (including the `#`).
    pub inline_comment: Option<String>,
}

/// Parse a CODEOWNERS file into an AST.
pub fn parse(input: &str) -> File {
    let mut groups: Vec<Group> = Vec::new();
    let mut current_lines: Vec<Line> = Vec::new();

    for raw_line in input.lines() {
        let trimmed = raw_line.trim();

        if trimmed.is_empty() {
            // Blank line: flush the current group if it has content.
            if !current_lines.is_empty() {
                groups.push(Group {
                    lines: std::mem::take(&mut current_lines),
                });
            }
            continue;
        }

        if trimmed.starts_with('#') {
            current_lines.push(Line::Comment(Comment {
                text: trimmed.to_string(),
            }));
            continue;
        }

        // It's a rule line: pattern followed by owners, with optional inline comment.
        current_lines.push(Line::Rule(parse_rule(trimmed)));
    }

    // Flush any remaining lines.
    if !current_lines.is_empty() {
        groups.push(Group {
            lines: current_lines,
        });
    }

    File { groups }
}

fn parse_rule(line: &str) -> Rule {
    // We need to split the line into tokens, but be careful about inline comments.
    // An inline comment starts with `#` that is not part of a pattern.
    // Since `#` cannot be escaped in patterns (per GitHub docs), any `#` after the
    // first token boundary is an inline comment.

    let mut tokens: Vec<String> = Vec::new();
    let mut inline_comment: Option<String> = None;

    let mut chars = line.chars().peekable();
    let mut current = String::new();

    // Skip leading whitespace.
    while chars.next_if(|&c| c == ' ' || c == '\t').is_some() {}

    // Parse the pattern (first token) - patterns can't contain # since escaping isn't supported.
    while let Some(&ch) = chars.peek() {
        if ch == ' ' || ch == '\t' {
            break;
        }
        current.push(ch);
        chars.next();
    }
    if !current.is_empty() {
        tokens.push(std::mem::take(&mut current));
    }

    // Parse remaining tokens (owners and possible inline comment).
    loop {
        while chars.next_if(|&c| c == ' ' || c == '\t').is_some() {}

        match chars.peek() {
            None => break,
            Some(&'#') => {
                inline_comment = Some(chars.collect());
                break;
            }
            Some(_) => {
                while let Some(&ch) = chars.peek() {
                    if ch == ' ' || ch == '\t' {
                        break;
                    }
                    current.push(ch);
                    chars.next();
                }
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
        }
    }

    let mut iter = tokens.into_iter();
    let pattern = iter.next().unwrap_or_default();
    let owners = iter.collect();

    Rule {
        pattern,
        owners,
        inline_comment,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_rule() {
        let rule = parse_rule("/src/foo/ @org/team");
        assert_eq!(rule.pattern, "/src/foo/");
        assert_eq!(rule.owners, vec!["@org/team"]);
        assert_eq!(rule.inline_comment, None);
    }

    #[test]
    fn test_parse_rule_multiple_owners() {
        let rule = parse_rule("/src/foo/ @org/team1 @org/team2");
        assert_eq!(rule.pattern, "/src/foo/");
        assert_eq!(rule.owners, vec!["@org/team1", "@org/team2"]);
    }

    #[test]
    fn test_parse_rule_with_inline_comment() {
        let rule = parse_rule("*.js @js-owner #This is a comment");
        assert_eq!(rule.pattern, "*.js");
        assert_eq!(rule.owners, vec!["@js-owner"]);
        assert_eq!(rule.inline_comment, Some("#This is a comment".to_string()));
    }

    #[test]
    fn test_parse_rule_no_owners() {
        let rule = parse_rule("/apps/github");
        assert_eq!(rule.pattern, "/apps/github");
        assert!(rule.owners.is_empty());
        assert_eq!(rule.inline_comment, None);
    }

    #[test]
    fn test_parse_file() {
        let input = "# Header comment\n\n/src/ @org/team1\n/lib/ @org/team2\n\n# Another section\n/docs/ @org/docs\n";
        let file = parse(input);
        assert_eq!(file.groups.len(), 3);

        // First group: just a comment
        assert_eq!(file.groups[0].lines.len(), 1);
        assert!(matches!(&file.groups[0].lines[0], Line::Comment(c) if c.text == "# Header comment"));

        // Second group: two rules
        assert_eq!(file.groups[1].lines.len(), 2);

        // Third group: comment + rule
        assert_eq!(file.groups[2].lines.len(), 2);
    }

    #[test]
    fn test_parse_multiple_blank_lines() {
        let input = "/src/ @team1\n\n\n\n/lib/ @team2\n";
        let file = parse(input);
        assert_eq!(file.groups.len(), 2);
    }

    #[test]
    fn test_parse_empty_input() {
        let file = parse("");
        assert!(file.groups.is_empty());
    }

    #[test]
    fn test_parse_rule_pattern_with_inline_comment_no_owners() {
        // A pattern followed directly by a comment, with no owners in between.
        // This matters because `/foo #comment` should NOT treat `#comment` as an owner.
        let rule = parse_rule("/foo #needs an owner");
        assert_eq!(rule.pattern, "/foo");
        assert!(rule.owners.is_empty());
        assert_eq!(rule.inline_comment, Some("#needs an owner".to_string()));
    }

    #[test]
    fn test_parse_no_trailing_newline() {
        // Files without a trailing newline should parse identically.
        let with = parse("/src/ @team\n");
        let without = parse("/src/ @team");
        assert_eq!(with, without);
    }
}
