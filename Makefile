PYTHON = uv run -- python
RUST_HOST_TARGET ?= $(shell rustc -vV | sed -n 's/^host: //p')
FUZZ_TARGET ?= validate_qir
FUZZ_RUN_ARGS ?= -max_total_time=30
FUZZ_ALL_TARGETS := $(basename $(notdir $(wildcard fuzz/fuzz_targets/*.rs)))

.PHONY: compile
# Usage:
# make compile FILE=tests/data/adaptive.ll
compile:
	cargo run -- $(FILE)

.PHONY: lint
lint:
	uvx prek run --all-files
	uvx ty check

.PHONY: test
test:
	cargo nextest run --all-targets --all-features

.PHONY: mutants
mutants:
	cargo mutants --package qir-qis --all-features --test-tool cargo

.PHONY: fuzz
fuzz:
	cargo +nightly fuzz run --target $(RUST_HOST_TARGET) $(FUZZ_TARGET) -- $(FUZZ_RUN_ARGS)

.PHONY: fuzz-all
fuzz-all:
	@for target in $(FUZZ_ALL_TARGETS); do \
		cargo +nightly fuzz check --target $(RUST_HOST_TARGET) "$$target" || exit 1; \
		cargo +nightly fuzz run --target $(RUST_HOST_TARGET) "$$target" -- -max_total_time=15 || exit 1; \
	done

.PHONY: sim
# make sim FILE=tests/data/adaptive.ll
sim:
	$(PYTHON) main.py $(FILE)

.PHONY: dis
dis:
	llvm-dis $(FILE)

.PHONY: stubs
stubs:
	cargo run --bin stub_gen
	uvx prek run --files qir_qis.pyi || true
	uvx ruff check qir_qis.pyi --fix

find_files = \
	@find tests/data/ \
		\( -path 'tests/data/bad' \
		\) -prune -false \
		-o -name '*.ll' -print

.PHONY: all_files
all_files:
	$(find_files) | \
	while read file; do \
			$(MAKE) $(TARGET) FILE="$$file" || exit 1; \
	done


.PHONY: allsim
allsim:
	uv sync
	rm -f tests/data/**/*.qis.ll
	$(MAKE) TARGET=sim all_files

.PHONY: allcompile
allcompile:
	rm -f tests/data/**/*.qis.ll
	$(MAKE) TARGET=compile all_files
