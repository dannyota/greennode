.PHONY: help check fmt lint test deny lengths comments pins docs crate-age tools-outdated hooks-install reference

SHELL := bash
.SHELLFLAGS := -Eeuo pipefail -c
.DEFAULT_GOAL := help

TEST_HOME := $(CURDIR)/target/test-home
REAL_CARGO_HOME := $(or $(CARGO_HOME),$(HOME)/.cargo)
REAL_RUSTUP_HOME := $(or $(RUSTUP_HOME),$(HOME)/.rustup)
BIN := node_modules/.bin
BASELINE_REPO := https://github.com/dannyota/vngcloud
BASELINE_TAG := v1.0.1
BASELINE_COMMIT := d784efc9241adcc4ed31bcab21e80dfd4552aab5

help: ## Show the targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | sed -E 's/^([a-z-]+):.*## /\1\t/'

check: lengths comments pins docs fmt lint test ## Run every check that works offline

fmt: ## Check Rust formatting; fix with cargo fmt --all
	cargo fmt --all -- --check

lint: ## Run clippy with warnings denied
	cargo clippy --locked --all-targets -- -D warnings

test: ## Run tests with nextest and an empty home under target/
	rm -rf "$(TEST_HOME)"
	mkdir -p "$(TEST_HOME)"
	env CARGO_HOME="$(REAL_CARGO_HOME)" RUSTUP_HOME="$(REAL_RUSTUP_HOME)" \
		HOME="$(TEST_HOME)" XDG_CONFIG_HOME="$(TEST_HOME)" XDG_STATE_HOME="$(TEST_HOME)" XDG_DATA_HOME="$(TEST_HOME)" \
		bash -c 'if cargo nextest --version >/dev/null 2>&1; then cargo nextest run --locked; \
		else echo "cargo-nextest is missing, so tests run single-threaded and slowly; install it with: cargo install --locked cargo-nextest" >&2; \
		cargo test --locked -- --test-threads=1; fi'
	@if [[ -n "$$(ls -A "$(TEST_HOME)")" ]]; then echo "tests wrote under $(TEST_HOME):" >&2; find "$(TEST_HOME)" >&2; exit 1; fi

deny: ## Check advisories, licenses, bans, and sources
	cargo deny --locked check

lengths: ## Test and run the file length gate; set BASE_REF to forbid baseline growth
	bash scripts/test/check-lengths-test.sh
	bash scripts/check-lengths.sh $${BASE_REF:+--base-ref "$$BASE_REF"}

comments: ## Test and run the code comment gate
	python3 -I scripts/test/check-comments-test.py
	python3 -I scripts/check-comments.py

pins: ## Test and run the action pin gate and the tool pin consistency check
	python3 -I scripts/test/check-workflow-pins-test.py
	python3 -I scripts/check-workflow-pins.py

docs: ## Check Markdown with Prettier and markdownlint; needs npm ci --ignore-scripts
	@test -x $(BIN)/prettier -a -x $(BIN)/markdownlint-cli2 || { echo "run npm ci --ignore-scripts first" >&2; exit 1; }
	$(BIN)/prettier --check "**/*.md"
	$(BIN)/markdownlint-cli2 "**/*.md"

crate-age: ## Fail on crate versions newer than 24 hours since BASE_REF, default origin/master
	python3 -I scripts/test/check-crate-age-test.py
	python3 -I scripts/check-crate-age.py "$${BASE_REF:-origin/master}"

tools-outdated: ## Fail when a pinned tool or action is behind its latest release
	bash scripts/tools-outdated.sh

hooks-install: ## Use .githooks for this clone
	git config --local core.hooksPath .githooks

reference: ## Clone the vngcloud baseline tag into .reference/vngcloud
	@test -d .reference/vngcloud || git clone --quiet --depth 1 --branch "$(BASELINE_TAG)" "$(BASELINE_REPO)" .reference/vngcloud
	@test "$$(git -C .reference/vngcloud rev-parse HEAD)" = "$(BASELINE_COMMIT)" || { echo ".reference/vngcloud is not $(BASELINE_TAG)" >&2; exit 1; }
	@echo ".reference/vngcloud is $(BASELINE_TAG)"
