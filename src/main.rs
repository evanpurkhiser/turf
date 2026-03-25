mod ast;
mod formatter;
mod matcher;
mod sorter;

use clap::{Parser, Subcommand};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process::{self, Command};

#[derive(Parser)]
#[command(name = "turf", about = "Toolkit for GitHub CODEOWNERS files")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Format a CODEOWNERS file.
    Format {
        /// Path to the CODEOWNERS file. Reads from stdin if not provided.
        file: Option<PathBuf>,

        /// Check if the file is already formatted (exit 1 if not).
        #[arg(long, conflicts_with = "write_in_place")]
        check: bool,

        /// Write the formatted output back to the file in-place.
        #[arg(short = 'w', long = "write")]
        write_in_place: bool,

        /// Skip sorting rules within groups.
        #[arg(long)]
        no_sort: bool,
    },

    /// Show who owns files in a repository.
    #[command(name = "who-owns")]
    WhoOwns {
        /// Path to a file or directory. Use `.` for the entire repo.
        path: PathBuf,

        /// Path to the CODEOWNERS file. Auto-detected if not provided.
        #[arg(long)]
        codeowners: Option<PathBuf>,

        /// Show unowned files too.
        #[arg(long)]
        unowned: bool,
    },

    /// Find rules that are overridden by later rules with different owners.
    Disputed {
        /// Path to a file or directory. Use `.` for the entire repo.
        path: PathBuf,

        /// Path to the CODEOWNERS file. Auto-detected if not provided.
        #[arg(long)]
        codeowners: Option<PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        // Default to format behavior when no subcommand (backwards compat with pipe usage).
        None => {
            let input = read_stdin();
            let mut file = ast::parse(&input);
            sorter::sort_groups(&mut file);
            let formatted = formatter::format(&file);
            print!("{formatted}");
        }
        Some(Commands::Format {
            file,
            check,
            write_in_place,
            no_sort,
        }) => cmd_format(file, check, write_in_place, no_sort),
        Some(Commands::WhoOwns {
            path,
            codeowners,
            unowned,
        }) => cmd_whoowns(&path, codeowners.as_deref(), unowned),
        Some(Commands::Disputed { path, codeowners }) => cmd_disputed(&path, codeowners.as_deref()),
    }
}

fn cmd_format(file: Option<PathBuf>, check: bool, write_in_place: bool, no_sort: bool) {
    let input = match &file {
        Some(path) => read_file(path),
        None => read_stdin(),
    };

    let mut parsed = ast::parse(&input);
    if !no_sort {
        sorter::sort_groups(&mut parsed);
    }
    let formatted = formatter::format(&parsed);

    if check {
        if formatted != input {
            if let Some(path) = &file {
                eprintln!("{} is not formatted", path.display());
            } else {
                eprintln!("input is not formatted");
            }
            process::exit(1);
        }
        process::exit(0);
    }

    if write_in_place {
        let path = file.as_ref().unwrap_or_else(|| {
            eprintln!("error: --write requires a file path");
            process::exit(2);
        });
        if let Err(e) = std::fs::write(path, &formatted) {
            eprintln!("error: could not write {}: {e}", path.display());
            process::exit(2);
        }
    } else {
        print!("{formatted}");
    }
}

fn cmd_whoowns(path: &Path, codeowners: Option<&Path>, unowned: bool) {
    let repo_root = find_repo_root(path);
    let codeowners_path = match codeowners {
        Some(p) => p.to_path_buf(),
        None => find_codeowners_file(&repo_root),
    };

    let input = read_file(&codeowners_path);
    let parsed = ast::parse(&input);
    let rule_set = matcher::RuleSet::new(&parsed);

    let scope_prefix = if path.is_file() {
        None
    } else {
        // Compute the relative prefix for scoping output to this directory.
        let abs = std::fs::canonicalize(path).unwrap_or_else(|e| {
            eprintln!("error: {}: {e}", path.display());
            process::exit(2);
        });
        if abs == repo_root {
            None
        } else {
            let rel = abs
                .strip_prefix(&repo_root)
                .unwrap_or(&abs)
                .to_string_lossy()
                .to_string();
            Some(if rel.ends_with('/') {
                rel
            } else {
                format!("{rel}/")
            })
        }
    };

    let files = if path.is_file() {
        let abs = std::fs::canonicalize(path).unwrap_or_else(|e| {
            eprintln!("error: {}: {e}", path.display());
            process::exit(2);
        });
        let rel = abs
            .strip_prefix(&repo_root)
            .unwrap_or(&abs)
            .to_string_lossy()
            .to_string();
        vec![rel]
    } else {
        let all_files = git_ls_files(&repo_root);
        match &scope_prefix {
            Some(prefix) => all_files
                .into_iter()
                .filter(|f| f.starts_with(prefix.as_str()))
                .collect(),
            None => all_files,
        }
    };

    // Match in parallel, then print in order.
    use rayon::prelude::*;
    let results: Vec<_> = files
        .par_iter()
        .map(|file| (file.as_str(), rule_set.find_owners(file).to_vec()))
        .collect();

    let mut owned_count = 0;
    let mut unowned_count = 0;

    for (file, owners) in &results {
        if owners.is_empty() {
            unowned_count += 1;
            if unowned {
                println!("{file} (unowned)");
            }
        } else {
            owned_count += 1;
            println!("{file} {}", owners.join(" "));
        }
    }

    eprintln!(
        "\n{owned_count} owned, {unowned_count} unowned, {} total files",
        files.len()
    );
}

fn cmd_disputed(path: &Path, codeowners: Option<&Path>) {
    use rayon::prelude::*;
    use std::collections::BTreeMap;

    let repo_root = find_repo_root(path);
    let codeowners_path = match codeowners {
        Some(p) => p.to_path_buf(),
        None => find_codeowners_file(&repo_root),
    };

    let input = read_file(&codeowners_path);
    let parsed = ast::parse(&input);
    let rule_set = matcher::RuleSet::new(&parsed);
    let rules = &rule_set.rules;
    let files = git_ls_files(&repo_root);

    // For each rule, track:
    // - total files it matches
    // - files where it's overridden (a later rule with different owners wins)
    // - which later rules override it and how many files
    //
    // A rule is "dead" (disputed) when it's overridden on ALL files it matches,
    // meaning it can never actually determine ownership. This catches the case
    // where a specific rule appears before a general one, making it useless.
    let n = rules.len();

    // Parallel: for each file, collect (rule_idx, is_winner, winner_idx) tuples.
    let per_file: Vec<Vec<(usize, Option<usize>)>> = files
        .par_iter()
        .filter_map(|file| {
            let matching = rule_set.find_all_matching(file);
            if matching.is_empty() {
                return None;
            }
            let winner = *matching.last().unwrap();
            let winner_owners = &rules[winner].owners;
            let result: Vec<(usize, Option<usize>)> = matching
                .iter()
                .map(|&idx| {
                    if idx == winner {
                        (idx, None) // this rule wins
                    } else if rules[idx].owners != *winner_owners {
                        (idx, Some(winner)) // overridden by a different owner
                    } else {
                        (idx, None) // overridden but same owners, doesn't matter
                    }
                })
                .collect();
            Some(result)
        })
        .collect();

    // Reduce: count total matches and overrides per rule.
    let mut total_matches = vec![0usize; n];
    let mut overridden_matches = vec![0usize; n];
    let mut override_by: BTreeMap<(usize, usize), usize> = BTreeMap::new();

    for file_results in per_file {
        for (rule_idx, override_winner) in file_results {
            total_matches[rule_idx] += 1;
            if let Some(winner) = override_winner {
                overridden_matches[rule_idx] += 1;
                *override_by.entry((rule_idx, winner)).or_insert(0) += 1;
            }
        }
    }

    // A rule is dead if it matches at least one file and is overridden on all of them.
    let dead_rules: Vec<usize> = (0..n)
        .filter(|&i| total_matches[i] > 0 && overridden_matches[i] == total_matches[i])
        .collect();

    if dead_rules.is_empty() {
        eprintln!("No disputed rules found.");
        return;
    }

    for &rule_idx in &dead_rules {
        let rule = &rules[rule_idx];
        println!(
            "line {}: {} {} is fully overridden ({} files):",
            rule.line_number,
            rule.pattern,
            rule.owners.join(" "),
            total_matches[rule_idx],
        );
        for (&(_, winner_idx), &count) in override_by.range((rule_idx, 0)..=(rule_idx, n)) {
            let winner = &rules[winner_idx];
            println!(
                "  line {}: {} {} ({count} files)",
                winner.line_number,
                winner.pattern,
                winner.owners.join(" "),
            );
        }
        println!();
    }

    eprintln!("{} dead rules found.", dead_rules.len());
    process::exit(1);
}

fn find_repo_root(from: &Path) -> PathBuf {
    let start = if from.is_file() {
        from.parent().unwrap_or(from)
    } else {
        from
    };

    let abs = std::fs::canonicalize(start).unwrap_or_else(|e| {
        eprintln!("error: {}: {e}", start.display());
        process::exit(2);
    });

    let mut dir = abs.as_path();
    loop {
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => {
                eprintln!(
                    "error: not a git repository (or any parent): {}",
                    from.display()
                );
                process::exit(2);
            }
        }
    }
}

fn find_codeowners_file(repo_root: &Path) -> PathBuf {
    let candidates = [
        repo_root.join("CODEOWNERS"),
        repo_root.join(".github/CODEOWNERS"),
        repo_root.join("docs/CODEOWNERS"),
    ];

    for candidate in candidates {
        if candidate.exists() {
            return candidate;
        }
    }

    eprintln!("error: no CODEOWNERS file found in {}", repo_root.display());
    process::exit(2);
}

fn git_ls_files(repo_root: &Path) -> Vec<String> {
    let output = Command::new("git")
        .arg("ls-files")
        .current_dir(repo_root)
        .output()
        .unwrap_or_else(|e| {
            eprintln!("error: failed to run git ls-files: {e}");
            process::exit(2);
        });

    if !output.status.success() {
        eprintln!(
            "error: git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        process::exit(2);
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.to_string())
        .collect()
}

fn read_file(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("error: could not read {}: {e}", path.display());
        process::exit(2);
    })
}

fn read_stdin() -> String {
    let mut buf = String::new();
    io::stdin().read_to_string(&mut buf).unwrap_or_else(|e| {
        eprintln!("error: could not read stdin: {e}");
        process::exit(2);
    });
    buf
}
