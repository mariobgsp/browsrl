SHELL := /bin/bash

WEBKIT_PREFIX ?= /usr
WEBKIT_ROOT := $(patsubst %/usr,%,$(WEBKIT_PREFIX))
export PKG_CONFIG_PATH := $(WEBKIT_PREFIX)/lib/pkgconfig:$(PKG_CONFIG_PATH)
ifeq ($(WEBKIT_PREFIX),/usr)
else
export PKG_CONFIG_SYSROOT_DIR := $(WEBKIT_ROOT)
export LD_LIBRARY_PATH := $(WEBKIT_PREFIX)/lib:$(LD_LIBRARY_PATH)
endif

.PHONY: doctor build run check-core fmt-check clippy e2e perf diagrams diagrams-check gate verify

# The machine-checkable subset: everything that needs no renderer and no
# display. `gate` prints a test count so an automated gate can prove the
# checks actually ran rather than trusting an exit code.
GATE := fmt-check clippy check-core build e2e

doctor:
	@printf 'rustc: '; rustc --version
	@printf 'cargo: '; cargo --version
	@for package in gtk4 libadwaita-1 webkitgtk-6.0 sqlite3; do \
		printf '%s: ' "$$package"; pkg-config --modversion "$$package"; \
	done

build:
	cargo build

run:
	cargo run --

# The core (config, navigation, storage) must stay buildable without GTK or
# WebKit so head-less machines and CI can check the non-GUI contract.
check-core:
	cargo check --no-default-features

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets -- -D warnings

e2e: build
	python3 tests/e2e.py

perf: build
	python3 tests/perf.py

diagrams:
	PLANTUML_JAVA="$${PLANTUML_JAVA:-java}" PLANTUML_JAR="$${PLANTUML_JAR:-}" scripts/render-diagrams.sh

diagrams-check:
	PLANTUML_JAVA="$${PLANTUML_JAVA:-java}" PLANTUML_JAR="$${PLANTUML_JAR:-}" scripts/check-diagrams.sh

gate:
	@$(MAKE) --no-print-directory $(GATE)
	@printf 'gate: %s passed\n' '$(words $(GATE))'

verify: $(GATE) diagrams-check
