use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::ast::{File, Line, Rule};

/// A compiled CODEOWNERS rule ready for matching.
pub struct CompiledRule {
    pub pattern: String,
    pub owners: Vec<String>,
    /// 1-based line number in the source file.
    pub line_number: usize,
}

/// Compile all rules from a CODEOWNERS AST into matchers.
pub fn compile_rules(file: &File) -> Vec<CompiledRule> {
    let mut line_number = 0usize;
    let mut rules = Vec::new();

    for group in &file.groups {
        for line in &group.lines {
            line_number += 1;
            if let Line::Rule(rule) = line
                && let Some(compiled) = compile_rule(rule, line_number)
            {
                rules.push(compiled);
            }
        }
        // Account for the blank line between groups.
        line_number += 1;
    }

    rules
}

fn compile_rule(rule: &Rule, line_number: usize) -> Option<CompiledRule> {
    // Validate that the pattern compiles as a glob.
    let glob_pattern = codeowners_to_glob(&rule.pattern);
    GlobBuilder::new(&glob_pattern)
        .literal_separator(true)
        .build()
        .ok()?;

    Some(CompiledRule {
        pattern: rule.pattern.clone(),
        owners: rule.owners.clone(),
        line_number,
    })
}

/// Convert a CODEOWNERS pattern to a globset-compatible pattern.
///
/// CODEOWNERS follows gitignore-style rules:
/// - Leading `/` anchors to the repo root (stripped for globset).
/// - Trailing `/` matches the directory and everything within.
/// - A pattern without `/` (or only trailing `/`) matches anywhere.
/// - A pattern with `/` in the beginning or middle is relative to root.
fn codeowners_to_glob(pattern: &str) -> String {
    let has_leading_slash = pattern.starts_with('/');
    let p = pattern.strip_prefix('/').unwrap_or(pattern);

    let base = p.strip_suffix('/').unwrap_or(p);
    let has_middle_slash = base.contains('/');
    let anchored = has_leading_slash || has_middle_slash;

    let mut result = String::new();
    if !anchored {
        result.push_str("**/");
    }
    result.push_str(p);
    if p.ends_with('/') {
        result.push_str("**");
    }
    result
}

/// A compiled set of all rules, using GlobSet for efficient batch matching.
pub struct RuleSet {
    pub rules: Vec<CompiledRule>,
    glob_set: GlobSet,
}

impl RuleSet {
    /// Build a RuleSet from a parsed CODEOWNERS file.
    pub fn new(file: &File) -> Self {
        let rules = compile_rules(file);
        let mut builder = GlobSetBuilder::new();
        for rule in &rules {
            // Re-build the glob for the set. We need to match the same
            // settings used in compile_rule.
            let glob_pattern = codeowners_to_glob(&rule.pattern);
            let glob = GlobBuilder::new(&glob_pattern)
                .literal_separator(true)
                .build()
                .unwrap();
            builder.add(glob);
        }
        let glob_set = builder.build().unwrap();
        Self { rules, glob_set }
    }

    /// Find all rules that match a given file path.
    /// Returns indices into the rules slice, sorted in order.
    pub fn find_all_matching(&self, path: &str) -> Vec<usize> {
        let mut indices = self.glob_set.matches(path);
        indices.sort_unstable();
        indices
    }

    /// Find the owners of a given file path using last-match-wins semantics.
    pub fn find_owners(&self, path: &str) -> &[String] {
        let indices = self.glob_set.matches(path);
        indices.iter().max().map_or(&[], |&i| &self.rules[i].owners)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glob_matches(pattern: &str, path: &str) -> bool {
        let glob_pattern = codeowners_to_glob(pattern);
        let glob = GlobBuilder::new(&glob_pattern)
            .literal_separator(true)
            .build()
            .unwrap()
            .compile_matcher();
        glob.is_match(path)
    }

    #[test]
    fn test_rooted_directory() {
        assert!(glob_matches("/src/sentry/api/", "src/sentry/api/foo.py"));
        assert!(glob_matches(
            "/src/sentry/api/",
            "src/sentry/api/sub/foo.py"
        ));
        assert!(!glob_matches("/src/sentry/api/", "src/sentry/utils/foo.py"));
    }

    #[test]
    fn test_rooted_file() {
        assert!(glob_matches("/src/foo.py", "src/foo.py"));
        assert!(!glob_matches("/src/foo.py", "other/src/foo.py"));
        assert!(!glob_matches("/src/foo.py", "src/bar.py"));
    }

    #[test]
    fn test_wildcard_extension() {
        assert!(glob_matches("*.js", "app.js"));
        assert!(glob_matches("*.js", "src/deep/nested/app.js"));
        assert!(!glob_matches("*.js", "app.py"));
    }

    #[test]
    fn test_wildcard_in_path() {
        assert!(glob_matches("/bin/mock*", "bin/mock-server"));
        assert!(glob_matches("/bin/mock*", "bin/mock_test"));
        assert!(!glob_matches("/bin/mock*", "bin/other"));
    }

    #[test]
    fn test_double_star() {
        assert!(glob_matches("**/logs", "logs"));
        assert!(glob_matches("**/logs", "src/logs"));
        assert!(glob_matches("**/logs", "deep/nested/logs"));
    }

    #[test]
    fn test_unanchored_with_slash() {
        // `docs/*` has a slash in the middle, so it's anchored to root
        assert!(glob_matches("docs/*", "docs/README.md"));
        assert!(!glob_matches("docs/*", "docs/sub/README.md"));
        assert!(!glob_matches("docs/*", "other/docs/README.md"));
    }

    #[test]
    fn test_catch_all() {
        assert!(glob_matches("*", "anything.txt"));
        assert!(glob_matches("*", "src/deep/file.py"));
    }

    #[test]
    fn test_last_match_wins() {
        let input = "* @default\n*.js @js-team\n/src/special.js @special\n";
        let file = crate::ast::parse(input);
        let rs = RuleSet::new(&file);

        assert_eq!(rs.find_owners("README.md"), &["@default"]);
        assert_eq!(rs.find_owners("app.js"), &["@js-team"]);
        assert_eq!(rs.find_owners("src/app.js"), &["@js-team"]);
        assert_eq!(rs.find_owners("src/special.js"), &["@special"]);
    }

    #[test]
    fn test_directory_override() {
        let input =
            "/src/sentry/snuba/ @snuba\n/src/sentry/snuba/metrics/query.py @snuba @telemetry\n";
        let file = crate::ast::parse(input);
        let rs = RuleSet::new(&file);

        assert_eq!(rs.find_owners("src/sentry/snuba/foo.py"), &["@snuba"]);
        assert_eq!(
            rs.find_owners("src/sentry/snuba/metrics/query.py"),
            &["@snuba", "@telemetry"]
        );
    }

    #[test]
    fn test_bare_filename_matches_anywhere() {
        assert!(glob_matches("Makefile", "Makefile"));
        assert!(glob_matches("Makefile", "src/Makefile"));
        assert!(glob_matches("Makefile", "src/deep/nested/Makefile"));
        assert!(!glob_matches("Makefile", "Makefile.bak"));
    }

    #[test]
    fn test_no_rules_means_unowned() {
        let file = crate::ast::parse("");
        let rs = RuleSet::new(&file);
        assert!(rs.find_owners("any/file.py").is_empty());
    }
}
