use globset::{GlobBuilder, GlobMatcher};

use crate::ast::{File, Line, Rule};

/// A compiled CODEOWNERS rule ready for matching.
pub struct CompiledRule {
    pub owners: Vec<String>,
    pub matcher: GlobMatcher,
}

/// Compile all rules from a CODEOWNERS AST into matchers.
pub fn compile_rules(file: &File) -> Vec<CompiledRule> {
    file.groups
        .iter()
        .flat_map(|group| &group.lines)
        .filter_map(|line| match line {
            Line::Rule(rule) => compile_rule(rule),
            _ => None,
        })
        .collect()
}

fn compile_rule(rule: &Rule) -> Option<CompiledRule> {
    let glob_pattern = codeowners_to_glob(&rule.pattern);
    let glob = GlobBuilder::new(&glob_pattern)
        .literal_separator(true)
        .build()
        .ok()?;

    Some(CompiledRule {
        owners: rule.owners.clone(),
        matcher: glob.compile_matcher(),
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

/// Find the owners of a given file path using last-match-wins semantics.
/// Returns the owners from the last matching rule, or an empty slice if unowned.
pub fn find_owners<'a>(path: &str, rules: &'a [CompiledRule]) -> &'a [String] {
    rules
        .iter()
        .rfind(|rule| rule.matcher.is_match(path))
        .map_or(&[], |rule| &rule.owners)
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
        let rules = compile_rules(&file);

        assert_eq!(find_owners("README.md", &rules), &["@default"]);
        assert_eq!(find_owners("app.js", &rules), &["@js-team"]);
        assert_eq!(find_owners("src/app.js", &rules), &["@js-team"]);
        assert_eq!(find_owners("src/special.js", &rules), &["@special"]);
    }

    #[test]
    fn test_directory_override() {
        let input =
            "/src/sentry/snuba/ @snuba\n/src/sentry/snuba/metrics/query.py @snuba @telemetry\n";
        let file = crate::ast::parse(input);
        let rules = compile_rules(&file);

        assert_eq!(find_owners("src/sentry/snuba/foo.py", &rules), &["@snuba"]);
        assert_eq!(
            find_owners("src/sentry/snuba/metrics/query.py", &rules),
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
        let rules: Vec<CompiledRule> = vec![];
        assert!(find_owners("any/file.py", &rules).is_empty());
    }
}
