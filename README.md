# turf

[![CI](https://github.com/evanpurkhiser/turf/actions/workflows/ci.yml/badge.svg)](https://github.com/evanpurkhiser/turf/actions/workflows/ci.yml)

A toolkit for GitHub CODEOWNERS files. Formats, sorts, and queries ownership.

## Commands

- **`format`** -- Format and sort a CODEOWNERS file. Aligns owner columns within each group, collapses extra blank lines, and sorts rules lexicographically while preserving last-match-wins semantics. Can read from a file or stdin.

  ```
  turf format <FILE>              # format and sort, print to stdout
  turf format <FILE> -w           # format and sort in-place
  turf format <FILE> --check      # exit 1 if not formatted (for CI)
  turf format <FILE> --no-sort    # format only, skip sorting
  cat CODEOWNERS | turf           # stdin/stdout pipe
  ```

- **`who-owns`** -- Show which teams own which files by evaluating the CODEOWNERS rules against actual files in the repo. Auto-detects the CODEOWNERS file location.

  ```
  turf who-owns .                 # show owners for all files in repo
  turf who-owns src/api/          # show owners scoped to a directory
  turf who-owns src/foo.py        # show owner of a specific file
  turf who-owns . --unowned       # include unowned files in output
  ```

- **`disputed`** -- Find dead rules where a specific pattern appears before a more general one, making it completely ineffective. A rule is flagged only when it is overridden on every single file it matches. Exits 1 if any dead rules are found.

  ```
  turf disputed .                 # find dead rules in the repo
  turf disputed src/api/          # scoped to a directory
  ```

## Sorting algorithm

By default, `turf format` sorts rules within each group lexicographically by file pattern. The sorting algorithm is designed to never change which team owns which file.

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
turf who-owns . | sort > before.txt

# Format the file
turf format CODEOWNERS -w

# Capture ownership after
turf who-owns . | sort > after.txt

# Verify identical
diff before.txt after.txt
```
