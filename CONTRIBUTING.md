# Contributing to QIR-QIS

Thank you for your interest in contributing! We welcome contributions from the community.

## Getting Started

See the [Development section](README.md#development) in the README for setup instructions, testing, and build processes.

## How to Contribute

### Reporting Issues

Before creating an issue, check existing ones to avoid duplicates. Include:

- Clear title and description
- Steps to reproduce (with example QIR files if applicable)
- Expected vs actual behavior
- Version information (Rust, Python, LLVM, OS)
- Error messages and stack traces

### Pull Requests

1. Fork and create a branch from `main`
2. Make your changes following our standards (see below)
3. Add tests for new functionality
4. Run `make test` and `make lint` - both must pass
5. Submit PR with clear description and reference relevant issues

**PR Checklist:**

- [ ] Tests pass (`make test`)
- [ ] Linters pass (`make lint`)
- [ ] Documentation updated (README, API docs, qtm-qir-reference.md if needed)
- [ ] Python stubs regenerated if API changed (`make stubs`)

## Coding Standards

### Rust

- Use `cargo fmt` for formatting
- Fix all `clippy` warnings: `cargo clippy --all-targets --all-features -- -D warnings`
- Document public APIs with doc comments
- Avoid panics in library code - use `Result` types
- Write unit tests for new functionality

### Python

- Follow PEP 8
- Use type hints
- Update `.pyi` stubs: `make stubs`
- Test with Python 3.10-3.14

### Commit Messages

Use [conventional commits](https://www.conventionalcommits.org/en/v1.0.0/):

```text
type(scope): brief description

[optional body]
```

**Types:** `feat`, `fix`, `docs`, `style`, `refactor`, `test`, `chore`

**Examples:**

- `feat(decompose): add CY gate support`
- `fix(validate): correct qubit count validation`
- `docs: update LLVM installation instructions`

## Testing

See [README Testing section](README.md#testing) for details on:

- Running the test suite
- Testing individual files
- Simulation testing with Selene

Always add tests for new features and ensure all tests pass before submitting a PR.

## Documentation

- **User-facing changes**: Update [README.md](README.md)
- **QIR feature support**: Update [qtm-qir-reference.md](qtm-qir-reference.md)
- **Code comments**: Document complex logic
- **API docs**: Generate with `cargo doc --no-deps --open`

## Release Process

(For maintainers)

Releases are automated using [Release Please](https://github.com/googleapis/release-please):

1. **Make changes** following [Conventional Commits](https://www.conventionalcommits.org/) format.

2. **Merge PRs to main** - Release Please will automatically:
   - Create/update a release PR with:
     - Updated `CHANGELOG.md`
     - Updated `Cargo.toml` version
     - Updated `.release-please-manifest.json`

3. **Review and merge the release PR** - This triggers:
   - GitHub release creation with tag `vX.Y.Z`
   - Automatic publishing to [crates.io](https://crates.io/crates/qir-qis)
   - Automatic publishing to [PyPI](https://pypi.org/project/qir-qis/)

No manual version bumping or tagging needed!

## Code of Conduct

Be respectful and professional. Report unacceptable behavior to project maintainers.

## Questions?

Open an issue with the "question" label or start a GitHub Discussion.

## License

By contributing, you agree that your contributions will be licensed under Apache-2.0.
