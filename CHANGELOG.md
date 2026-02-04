# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.2](https://github.com/Quantinuum/qir-qis/compare/v0.1.1...v0.1.2) (2026-02-04)


### Bug Fixes

* **rust API:** update deps ([#20](https://github.com/Quantinuum/qir-qis/issues/20)) ([c74f764](https://github.com/Quantinuum/qir-qis/commit/c74f764fd20c6b729ff08f3cdd01f1e3be84c240)), closes [#18](https://github.com/Quantinuum/qir-qis/issues/18)

## [0.1.1](https://github.com/Quantinuum/qir-qis/compare/v0.1.0...v0.1.1) (2026-01-23)


### Bug Fixes

* remove `_core` from Rust API ([#14](https://github.com/Quantinuum/qir-qis/issues/14)) ([c9f3b16](https://github.com/Quantinuum/qir-qis/commit/c9f3b160f052539e169523625ce89f5d090fb1e3))

## [0.1.0] - 2026-01-22

### Added

* Initial release of QIR-QIS compiler
* QIR validation for LLVM bitcode
* Translation from QIR to Quantinuum QIS instruction set
* Configurable LLVM optimization levels (0-3)
* Support for multiple target architectures (aarch64, x86-64, native)
* Command-line interface for QIR compilation
* Python bindings with full API:
  * `qir_ll_to_bc()` - Convert LLVM IR text to bitcode
  * `validate_qir()` - Validate QIR bitcode
  * `qir_to_qis()` - Compile QIR to QIS
  * `get_entry_attributes()` - Extract entry point metadata
* Rust library API for embedding in other projects
* Python package distribution via PyPI
* Rust crate distribution via crates.io
* Comprehensive test suite with snapshot testing
* Integration with Selene quantum simulator for testing
* CI/CD pipeline with GitHub Actions
* Documentation and examples

[0.1.0]: https://github.com/quantinuum/qir-qis/releases/tag/v0.1.0
<!-- markdownlint-disable-file MD012 -->
