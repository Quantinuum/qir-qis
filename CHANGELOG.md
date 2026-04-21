# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.5](https://github.com/Quantinuum/qir-qis/compare/v0.1.4...v0.1.5) (2026-04-21)


### Features

* add mz leaked support ([#70](https://github.com/Quantinuum/qir-qis/issues/70)) ([c9116a1](https://github.com/Quantinuum/qir-qis/commit/c9116a1fe07a6a9558a7abcb31b3a8edafe9d24e))


### Bug Fixes

* add robustness coverage and cross-platform guardrails ([#59](https://github.com/Quantinuum/qir-qis/issues/59)) ([72ccb62](https://github.com/Quantinuum/qir-qis/commit/72ccb62f430f4a6ea49db195a2f76130947f7f16))
* add Windows guardrails for optimized conversion ([#51](https://github.com/Quantinuum/qir-qis/issues/51)) ([0ac66a1](https://github.com/Quantinuum/qir-qis/commit/0ac66a189ea3e905713a0789ea2058e0e6021e63))
* read module flags from named metadata ([#47](https://github.com/Quantinuum/qir-qis/issues/47)) ([4db37b5](https://github.com/Quantinuum/qir-qis/commit/4db37b5777147432bc591b0f7b9586a361c1c898))
* restore selene-compatible public bitcode bytes ([#72](https://github.com/Quantinuum/qir-qis/issues/72)) ([ad27b4e](https://github.com/Quantinuum/qir-qis/commit/ad27b4e3fe4de0958f59e68302cae28e466d7df8))
* support inkwell 0.9 upgrade ([#69](https://github.com/Quantinuum/qir-qis/issues/69)) ([3d13edd](https://github.com/Quantinuum/qir-qis/commit/3d13edd89ffd4c4c6a6c65f883a05d6ae3cc4ac2))

## [0.1.4](https://github.com/Quantinuum/qir-qis/compare/v0.1.3...v0.1.4) (2026-03-17)


### Features

* support QIR 2.0 conversion with LLVM 21 ([#34](https://github.com/Quantinuum/qir-qis/issues/34)) ([5d71a0e](https://github.com/Quantinuum/qir-qis/commit/5d71a0ed1b60b69e8bd365fd32134ad48ab53375))


### Bug Fixes

* preserve entry metadata without Windows CI crashes ([#38](https://github.com/Quantinuum/qir-qis/issues/38)) ([41a275e](https://github.com/Quantinuum/qir-qis/commit/41a275edb59a0c5ac3270830bd884f0d28fd92d2))

## [0.1.3](https://github.com/Quantinuum/qir-qis/compare/v0.1.2...v0.1.3) (2026-02-24)


### Features

* add support for barrier instructions ([#27](https://github.com/Quantinuum/qir-qis/issues/27)) ([95d5ec2](https://github.com/Quantinuum/qir-qis/commit/95d5ec267c163a646e090bf9565af29714222df7))

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
