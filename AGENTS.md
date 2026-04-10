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
- WASM parsing behavior in `src/utils.rs`
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
