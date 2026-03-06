# Development Notes

## Updating LLVM

LLVM version changes are mostly controlled from three places:

1. Workflow LLVM version variables:
   - `.github/workflows/CI.yml`
   - `.github/workflows/rust-release.yml`
   - `.github/workflows/wheels-release.yml`
2. The versioned `llvm-sys` prefix env var name passed through `.github/actions/setup-llvm/action.yml`.
3. Rust crate bindings in `Cargo.toml`:
   - `inkwell` feature (for example `llvm21-1`)
   - `llvm-sys` crate version (for example `211.0.0`)

For an LLVM major-version bump, update these in order:

1. Change `LLVM_VERSION` in the workflows to the desired full release, such as `22.0.0`.
2. Change the versioned env var name, such as `LLVM_SYS_211_PREFIX` to `LLVM_SYS_220_PREFIX`, in the workflow env blocks and any packaging config that still needs the explicit `llvm-sys` name.
3. Update `inkwell` and `llvm-sys` in `Cargo.toml`, then refresh `Cargo.lock`.
4. Check `pyproject.toml` for remaining version-specific wheel settings:
   - macOS Homebrew package name, such as `llvm@21`
   - versioned `LLVM_SYS_*` env var names on macOS and Windows
5. Check `README.md` for install examples that mention the old LLVM major.

The manylinux images intentionally install LLVM into `/opt/llvm`, so they should not need path changes for a version bump. Those paths are defined in:

- `.github/manylinux/Dockerfile.manylinux-2_34-x86_64`
- `.github/manylinux/Dockerfile.manylinux-2_34-aarch64`
- `.github/manylinux/llvm-config-wrapper.sh`

Recommended verification after a bump:

1. Run `cargo test --lib --all-features`.
2. Run `cargo run --example rust_api --all-features --profile test`.
3. Run `make lint`.
4. Run the CI matrix and wheel builds before cutting a release.
