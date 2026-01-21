PYTHON = uv run --no-sync -- python

.PHONY: compile
# Usage:
# make compile FILE=tests/data/adaptive.ll
compile:
	cargo run -- $(FILE)

.PHONY: lint
lint:
	uvx prek run --all-files
	uvx ty check
	cargo clippy --all-targets --all-features -- -W clippy::pedantic -W clippy::nursery

.PHONY: test
test:
	cargo nextest run --all-targets --all-features

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
	make lint; uvx ruff check . --fix

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
