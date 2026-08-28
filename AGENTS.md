# AGENTS.md

Contributor guidance for `qir-qis`.

## Priority

- Do the main feature or bug-fix work first.
- While doing it, preserve the repo's existing correctness guarantees.
- Add the smallest effective coverage needed to protect the new or changed behavior.

## Core expectations

For validation, lowering, translation, and LLVM-facing code:

- malformed inputs must return `Err`, not panic or abort
- unsupported constructs must fail cleanly and explicitly
- successful translations must remain LLVM-verifiable
- behavior should stay deterministic for entry attributes, module flags, and output naming

If a change intentionally weakens one of these properties, the PR should say so.

## When to add tests

Add or update tests whenever a PR changes:

- validation rules in `src/lib.rs`
- lowering/output behavior in `src/convert.rs` or `src/decompose.rs`
- LLVM verification/optimization behavior in `src/llvm_verify.rs` or `src/opt.rs`
- WASM parsing behavior in `src/lib.rs` (via the `wasm` feature) or `fuzz/fuzz_targets/`
- fuzz, mutation-testing, or robustness workflow infrastructure

Rule of thumb:

- behavior change -> regression test
- boundary or invariant change -> property test where practical
- parser or contract expansion -> consider a fuzz target or fuzz-target extension

## Preferred testing style

- Prefer narrow regression tests over broad incidental churn.
- Prefer `proptest` for invariants, boundaries, and malformed-but-structured inputs.
- Prefer structure-aware fuzzing over arbitrary raw byte mutation when the goal is to exercise `qir-qis` logic rather than LLVM parser failure paths.
- If expensive fixture compilation is repeated across many tests, cache it with `LazyLock` or similar.
- Keep `make mutants` useful: kill meaningful mutants with tests, and keep `.cargo/mutants.toml` exclusions resilient to line movement.
- For external/runtime signature validation, prefer table-driven negative tests that cover return type, arity, and parameter kind/width; for numeric limits, cover both the largest accepted value and first rejected value.
- During validation-helper work, run a scoped mutation check such as `cargo mutants --package qir-qis --all-features --test-tool cargo --file src/lib.rs --re '<helper_name>'`.

## LLVM and platform guidance

- Prefer Inkwell wrappers over raw LLVM C APIs when a safe wrapper exists.
- Be cautious with raw LLVM message/string extraction APIs.
- Do not call low-level LLVM string APIs on arbitrary walked values unless the value kind is known to be safe.
- Prefer small helpers with narrow contracts for LLVM interaction.
- Assume Linux, macOS, and Windows all matter unless the code is explicitly platform-specific.
- If a strong assertion is unstable only on one platform, weaken the assertion rather than the product behavior.

## Workflow guidance

When touching `Makefile`, `.cargo/mutants.toml`, `fuzz/`, or `.github/workflows/`:

- keep local commands and CI entrypoints aligned
- keep PR smoke coverage practical
- reserve longer fuzzing and mutation campaigns for scheduled/manual runs unless there is a strong reason otherwise
- avoid adding unused tooling installs to CI jobs

When touching Clippy policy or lint-driven cleanup:

- keep package-wide lint policy in `Cargo.toml` under `[lints.clippy]` and `[lints.rust]`, not in `src/lib.rs`
- group manifest lint entries by intent, mirroring the categories that would otherwise live in crate attributes
- only add `clippy.toml` when a real Clippy config value is needed; do not add it just to repeat `msrv` that already matches `package.rust-version`
- when enabling stricter lints such as `indexing_slicing`, prefer removing panic-prone production indexing, but use narrow `#[allow(..., reason = "...")]` on tests or validated LLVM-layout helpers when fixed-position access is intentional and already guarded
- every surviving `#[allow(...)]` for Clippy lints should include a `reason = "..."`
- preserve the product guarantee that malformed inputs return `Err`; do not “fix” Clippy findings by introducing Rust panics

Before pushing changes:

- run `make lint` (runs Python linting via ruff/prek and type checking via ty; Rust clippy runs via the pre-commit hook)
- run `make test` (runs `cargo nextest` for Rust and `tests/test_main.py` for Python)

If local validation is running on a protected branch such as `main`, it is acceptable to skip only the `no-commit-to-branch` pre-commit hook for local lint runs; keep the hook enabled for normal developer protection.

## Suggested validation

For most semantic changes:

- `cargo test --all-features`

When touching fuzzing or robustness workflows, also run a focused subset such as:

- `make fuzz FUZZ_TARGET=<target> FUZZ_RUN_ARGS=-max_total_time=15`
- `cargo +nightly fuzz check --target $(rustc -vV | sed -n 's/^host: //p') <target>`

When touching mutation coverage or exclusions:

- `cargo mutants --package qir-qis --all-features --test-tool cargo --list`
- `make mutants` or a scoped shard during iteration

## PR guidance

- Keep the PR centered on its primary purpose.
- Add robustness work needed to make the change safe, but avoid unrelated cleanup.
- If testing uncovers a real production-code bug, call it out explicitly in the PR description.
- Do not describe workflow or harness bugs introduced and fixed within the same PR as pre-existing product bugs.
