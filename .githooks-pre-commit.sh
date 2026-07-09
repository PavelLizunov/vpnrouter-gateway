#!/bin/sh
# Blocking pre-commit gate: static checks + tests (see CLAUDE.md, Workflow).
# Installed as .git/hooks/pre-commit; this tracked copy survives re-clones.
set -e
export PATH="$HOME/.cargo/bin:$PATH"
echo "pre-commit: cargo fmt --check"
cargo fmt --check
echo "pre-commit: cargo clippy -D warnings"
cargo clippy --all-targets -- -D warnings
echo "pre-commit: cargo test"
cargo test --quiet
echo "pre-commit: OK"
