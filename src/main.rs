mod ast;
mod formatter;
mod matcher;
mod sorter;

use clap::{Parser, Subcommand};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process::{self, Command};

#[derive(Parser)]
#[command(
    name = "codeowners-format",
    about = "Auto-formatter for GitHub CODEOWNERS files"
)]
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

        /// Sort rules within each group lexicographically (preserving override semantics).
        #[arg(long)]
        sort: bool,
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
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        // Default to format behavior when no subcommand (backwards compat with pipe usage).
        None => {
            let input = read_stdin();
            let file = ast::parse(&input);
            let formatted = formatter::format(&file);
            print!("{formatted}");
        }
        Some(Commands::Format {
            file,
            check,
            write_in_place,
            sort,
        }) => cmd_format(file, check, write_in_place, sort),
        Some(Commands::WhoOwns {
            path,
            codeowners,
            unowned,
        }) => cmd_whoowns(&path, codeowners.as_deref(), unowned),
    }
}

fn cmd_format(file: Option<PathBuf>, check: bool, write_in_place: bool, sort: bool) {
    let input = match &file {
        Some(path) => read_file(path),
        None => read_stdin(),
    };

    let mut parsed = ast::parse(&input);
    if sort {
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
    let rules = matcher::compile_rules(&parsed);

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
            Some(if rel.ends_with('/') { rel } else { format!("{rel}/") })
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

    let mut owned_count = 0;
    let mut unowned_count = 0;

    for file in &files {
        let owners = matcher::find_owners(file, &rules);
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
                eprintln!("error: not a git repository (or any parent): {}", from.display());
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

    eprintln!(
        "error: no CODEOWNERS file found in {}",
        repo_root.display()
    );
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
