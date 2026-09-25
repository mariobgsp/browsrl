SHELL := /bin/bash

WEBKIT_PREFIX ?= /usr
WEBKIT_ROOT := $(patsubst %/usr,%,$(WEBKIT_PREFIX))
export PKG_CONFIG_PATH := $(WEBKIT_PREFIX)/lib/pkgconfig:$(PKG_CONFIG_PATH)
ifeq ($(WEBKIT_PREFIX),/usr)
else
export PKG_CONFIG_SYSROOT_DIR := $(WEBKIT_ROOT)
export LD_LIBRARY_PATH := $(WEBKIT_PREFIX)/lib:$(LD_LIBRARY_PATH)
endif

# Bound the compiler's parallelism. rustc peaks at roughly a gigabyte per crate,
# so building gtk4, libadwaita and webkit6 side by side with anything else on
# the machine is what pushed this host into swap. Raise it where there is memory
# to spare: `make CARGO_BUILD_JOBS=8 build`.
CARGO_BUILD_JOBS ?= 2
export CARGO_BUILD_JOBS

PREFIX ?= /usr
DESTDIR ?=
BINDIR := $(DESTDIR)$(PREFIX)/bin
DATADIR := $(DESTDIR)$(PREFIX)/share
APP_ID := io.github.browsrl.Browsrl
ASSETS := assets

# The machine-checkable subset: everything that needs no renderer and no
# display. `gate` prints a test count so an automated gate can prove the
# checks actually ran rather than trusting an exit code.
GATE := fmt-check clippy check-core build e2e

.PHONY: doctor build build-release run check-core fmt-check clippy e2e e2e-gui perf \
        diagrams diagrams-check check-desktop packaging gate verify dist \
        install uninstall

doctor:
	@printf 'cargo jobs: %s (override with CARGO_BUILD_JOBS)\n' "$(CARGO_BUILD_JOBS)"
	@printf 'rustc: '; rustc --version
	@printf 'cargo: '; cargo --version
	@for package in gtk4 libadwaita-1 webkitgtk-6.0 sqlite3; do \
		printf '%s: ' "$$package"; pkg-config --modversion "$$package"; \
	done

build:
	cargo build

# Packaging installs the release binary; the gate keeps using the debug one
# because it is what the contracts exercise.
build-release:
	cargo build --release

run:
	cargo run

# The core (config, navigation, storage, bookmarks, history, downloads) must
# stay buildable without GTK or WebKit so head-less machines and CI can check
# the non-GUI contract.
check-core:
	cargo check --no-default-features

fmt-check:
	cargo fmt --all -- --check

clippy:
	cargo clippy --all-targets -- -D warnings

e2e: build
	python3 tests/e2e.py

# Drives the real window through its own GApplication actions over the session
# bus. Needs a display, so it is not part of `gate`.
e2e-gui: build
	python3 tests/gui.py

perf: build
	python3 tests/perf.py

diagrams:
	PLANTUML_JAVA="$${PLANTUML_JAVA:-java}" PLANTUML_JAR="$${PLANTUML_JAR:-}" scripts/render-diagrams.sh

diagrams-check:
	PLANTUML_JAVA="$${PLANTUML_JAVA:-java}" PLANTUML_JAR="$${PLANTUML_JAR:-}" scripts/check-diagrams.sh

# Desktop integration assets, validated so a typo cannot reach a distribution.
check-desktop:
	@desktop-file-validate $(ASSETS)/$(APP_ID).desktop && echo "desktop entry: valid"
	@python3 -c "import xml.etree.ElementTree as E; E.parse('$(ASSETS)/$(APP_ID).metainfo.xml'); print('metainfo xml: well formed')"
	@python3 -c "d=open('$(ASSETS)/browsrl-128.png','rb').read(); \
		assert d[:8]==b'\\x89PNG\\r\\n\\x1a\\n', 'not a png'; print('icon: valid png')"
	@# appstreamcli reports the missing project homepage as a warning. This
	@# repository has no public URL yet, so that warning is accepted and only a
	@# real error (an "E:" line) fails the check.
	@output=$$(appstreamcli validate --no-net --explain $(ASSETS)/$(APP_ID).metainfo.xml 2>&1 || true); \
		if echo "$$output" | grep -q "^E:"; then \
			echo "$$output"; echo "appstream metadata has errors"; exit 1; \
		fi; \
		echo "appstream metadata: no errors"

packaging: check-desktop

gate:
	@$(MAKE) --no-print-directory $(GATE)
	@printf 'gate: %s passed\n' '$(words $(GATE))'

verify: $(GATE) packaging diagrams-check

# A source archive, for handing the project to a machine without git. Built
# from the same inputs the gate checks, and it refuses to include a stale
# diagram or an unverified desktop entry by depending on the same targets.
DIST_NAME := browsrl-$(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
DIST_DIR := dist/$(DIST_NAME)

dist: verify build-release
	@rm -rf $(DIST_DIR)
	@mkdir -p $(DIST_DIR)
	@cp Cargo.toml Cargo.lock rust-toolchain.toml Makefile README.md .gitignore $(DIST_DIR)
	@cp -r src tests scripts diagrams assets $(DIST_DIR)
	@cp -r .pi/verify.json $(DIST_DIR)/verify.json
	@tar -czf dist/$(DIST_NAME).tar.gz -C dist $(DIST_NAME)
	@rm -rf $(DIST_DIR)
	@printf 'built %s\n' "dist/$(DIST_NAME).tar.gz"

install: build-release
	install -Dm755 target/release/browsrl $(BINDIR)/browsrl
	install -Dm644 $(ASSETS)/$(APP_ID).desktop \
		$(DATADIR)/applications/$(APP_ID).desktop
	install -Dm644 $(ASSETS)/$(APP_ID).metainfo.xml \
		$(DATADIR)/metainfo/$(APP_ID).metainfo.xml
	install -Dm644 $(ASSETS)/browsrl-128.png \
		$(DATADIR)/icons/hicolor/128x128/apps/browsrl.png
	@echo "installed to $(DESTDIR)$(PREFIX)"

uninstall:
	rm -f $(BINDIR)/browsrl
	rm -f $(DATADIR)/applications/$(APP_ID).desktop
	rm -f $(DATADIR)/metainfo/$(APP_ID).metainfo.xml
	rm -f $(DATADIR)/icons/hicolor/128x128/apps/browsrl.png
	@rmdir $(DATADIR)/icons/hicolor/128x128/apps $(DATADIR)/icons/hicolor/128x128 2>/dev/null || true
	@echo "removed from $(DESTDIR)$(PREFIX)"
