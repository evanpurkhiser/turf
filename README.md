# codeowners-format

A formatter for GitHub CODEOWNERS files. Aligns columns, normalizes whitespace, and optionally sorts rules within groups while preserving override semantics.

## Usage

```
codeowners-format format <FILE>              # print formatted output to stdout
codeowners-format format <FILE> -w           # format in-place
codeowners-format format <FILE> --check      # exit 1 if not formatted (for CI)
codeowners-format format <FILE> --sort       # sort rules within groups
codeowners-format format <FILE> --sort -w    # sort and write in-place
cat CODEOWNERS | codeowners-format           # stdin/stdout pipe

codeowners-format who-owns .                  # show owners for all files in repo
codeowners-format who-owns src/api/           # show owners scoped to a directory
codeowners-format who-owns src/foo.py         # show owner of a specific file
codeowners-format who-owns . --unowned        # include unowned files in output
```

## Formatting rules

- Within each group (separated by blank lines), the owner column is aligned to one space after the longest pattern in that group.
- Multiple consecutive blank lines between groups are collapsed to a single blank line.
- Comments and inline comments are preserved exactly as written.
- A trailing newline is ensured.

## Sorting algorithm

When `--sort` is passed, rules within each group are sorted lexicographically by file pattern. The sorting algorithm is designed to never change which team owns which file.

CODEOWNERS uses last-match-wins semantics: if multiple rules match a file, the last one in the file determines ownership. This means carelessly reordering rules could silently reassign ownership.

The sorter handles this by identifying which pairs of rules could conflict -- meaning they have different owner sets and their file patterns could potentially match the same file. These conflicting pairs are constrained to keep their original relative order. All other rules are free to sort.

Specifically, two rules are considered potentially overlapping (and therefore pinned) when:

- Either pattern uses `**` (matches at any depth)
- Either pattern is unanchored (e.g., `*.js` or `Makefile` -- matches anywhere in the tree)
- Both patterns are anchored and one's directory prefix is a parent of the other's (e.g., `/src/foo/` and `/src/foo/bar.py`)

This check is conservative: it may prevent sorting rules that could technically be safely reordered, but it will never allow a reorder that changes ownership. Rules with the same owner set are always safe to sort freely since reordering them cannot change who owns any file.

The implementation uses a topological sort (Kahn's algorithm) where conflicting pairs form directed edges preserving original order, and lexicographic pattern order is used as the tiebreaker. The result is the lexicographically smallest ordering that preserves all ownership semantics.

You can verify that sorting is safe for your file using `who-owns`:

```
# Capture ownership before
codeowners-format who-owns . | sort > before.txt

# Sort the file
codeowners-format format CODEOWNERS --sort -w

# Capture ownership after
codeowners-format who-owns . | sort > after.txt

# Verify identical
diff before.txt after.txt
```
