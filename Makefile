# mbar: build, test and install helpers.
#
#   make release                       optimized build of the bar ($(TARGET_DIR)/release/mbar)
#   make install PREFIX=$HOME/.local   install mbar + `sketchybar` symlink (after `make release`)
#   make install-agent                 start mbar at login (LaunchAgent dev.rubeen.mbar)
#   make ui                            build the management app (crates/mbar-ui)
#
# Variables: PREFIX (default /usr/local), DESTDIR, CARGO, SKETCHYBAR_LINK (1/0).

CARGO           ?= cargo
PREFIX          ?= /usr/local
BINDIR          ?= $(PREFIX)/bin
DATADIR         ?= $(PREFIX)/share/mbar
SKETCHYBAR_LINK ?= 1

LABEL       := dev.rubeen.mbar
AGENT_DIR   := $(HOME)/Library/LaunchAgents
AGENT_PLIST := $(AGENT_DIR)/$(LABEL).plist
AGENT_PATH  := $(BINDIR):/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin
UI_DIR      := crates/mbar-ui
GUI_DOMAIN   = gui/$(shell id -u)

# cargo may build elsewhere (CARGO_TARGET_DIR, build.target-dir in ~/.cargo/config.toml).
TARGET_DIR    := $(shell $(CARGO) metadata --format-version 1 --no-deps 2>/dev/null | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
UI_TARGET_DIR := $(shell cd $(UI_DIR) && $(CARGO) metadata --format-version 1 --no-deps 2>/dev/null | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
# Fall back to cargo's default if `cargo metadata` is unavailable.
TARGET_DIR    := $(or $(TARGET_DIR),target)
UI_TARGET_DIR := $(or $(UI_TARGET_DIR),$(UI_DIR)/target)

.PHONY: all help build release test lint fmt ui ui-test install install-ui \
        install-agent uninstall-agent uninstall clean app verify-app dmg

all: build

help:
	@echo "Targets:"
	@echo "  build            debug build of the workspace"
	@echo "  release          release build of the mbar binary"
	@echo "  test             cargo test --workspace"
	@echo "  lint             rustfmt check + clippy -D warnings"
	@echo "  fmt              format all crates"
	@echo "  ui               release build of mbar-ui (separate workspace)"
	@echo "  ui-test          mbar-ui model tests (no GUI)"
	@echo "  install          install mbar to \$$(BINDIR) (PREFIX=$(PREFIX))"
	@echo "  install-ui       install mbar-ui to \$$(BINDIR)"
	@echo "  install-agent    install and load the LaunchAgent $(LABEL)"
	@echo "  uninstall-agent  unload and remove the LaunchAgent"
	@echo "  uninstall        uninstall-agent + remove installed files"
	@echo "  clean            cargo clean (workspace and mbar-ui)"
	@echo "  app              build dist/mbar.app (MBAR_SIGN_IDENTITY, MBAR_UNIVERSAL)"
	@echo "  verify-app       structural checks of dist/mbar.app"
	@echo "  dmg              notarized dist/mbar-<v>.dmg, update zip, appcast.xml (NOTARIZE, MBAR_SIGN_IDENTITY)"

build:
	$(CARGO) build --workspace

release:
	$(CARGO) build --release -p mbar

test:
	$(CARGO) test --workspace

lint:
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings

fmt:
	$(CARGO) fmt --all

ui:
	cd $(UI_DIR) && $(CARGO) build --release

ui-test:
	cd $(UI_DIR) && $(CARGO) test --no-default-features

install:
	@test -x "$(TARGET_DIR)/release/mbar" || { echo "$(TARGET_DIR)/release/mbar not found: run 'make release' first"; exit 1; }
	install -d "$(DESTDIR)$(BINDIR)" "$(DESTDIR)$(DATADIR)"
	install -m 755 "$(TARGET_DIR)/release/mbar" "$(DESTDIR)$(BINDIR)/mbar"
	install -m 644 lua/mbar.d.lua "$(DESTDIR)$(DATADIR)/mbar.d.lua"
ifeq ($(SKETCHYBAR_LINK),1)
	@if [ -e "$(DESTDIR)$(BINDIR)/sketchybar" ] && [ ! -L "$(DESTDIR)$(BINDIR)/sketchybar" ]; then \
		echo "warning: $(DESTDIR)$(BINDIR)/sketchybar exists and is not a symlink, leaving it alone"; \
	else \
		ln -sfn mbar "$(DESTDIR)$(BINDIR)/sketchybar"; \
		echo "linked $(DESTDIR)$(BINDIR)/sketchybar -> mbar"; \
	fi
endif

install-ui:
	@test -x "$(UI_TARGET_DIR)/release/mbar-ui" || { echo "run 'make ui' first"; exit 1; }
	install -d "$(DESTDIR)$(BINDIR)"
	install -m 755 "$(UI_TARGET_DIR)/release/mbar-ui" "$(DESTDIR)$(BINDIR)/mbar-ui"

install-agent:
	@test "$$(uname -s)" = Darwin || { echo "install-agent: macOS only"; exit 1; }
	@test -x "$(BINDIR)/mbar" || { echo "$(BINDIR)/mbar not found: run 'make install PREFIX=$(PREFIX)' first"; exit 1; }
	install -d "$(AGENT_DIR)" "$(HOME)/Library/Logs"
	sed -e 's|@MBAR_BIN@|$(BINDIR)/mbar|g' \
	    -e 's|@HOME@|$(HOME)|g' \
	    -e 's|@PATH@|$(AGENT_PATH)|g' \
	    packaging/$(LABEL).plist > "$(AGENT_PLIST)"
	plutil -lint "$(AGENT_PLIST)"
	-launchctl bootout $(GUI_DOMAIN)/$(LABEL) 2>/dev/null
	launchctl bootstrap $(GUI_DOMAIN) "$(AGENT_PLIST)"
	@echo "mbar runs as $(LABEL); logs: $(HOME)/Library/Logs/mbar.log"

uninstall-agent:
	-@[ "$$(uname -s)" = Darwin ] && launchctl bootout $(GUI_DOMAIN)/$(LABEL) 2>/dev/null; true
	rm -f "$(AGENT_PLIST)"

uninstall: uninstall-agent
	rm -f "$(DESTDIR)$(BINDIR)/mbar" "$(DESTDIR)$(BINDIR)/mbar-ui"
	@if [ -L "$(DESTDIR)$(BINDIR)/sketchybar" ] && [ "$$(readlink "$(DESTDIR)$(BINDIR)/sketchybar")" = mbar ]; then \
		rm -f "$(DESTDIR)$(BINDIR)/sketchybar"; echo "removed $(DESTDIR)$(BINDIR)/sketchybar"; \
	fi
	rm -f "$(DESTDIR)$(DATADIR)/mbar.d.lua"
	-rmdir "$(DESTDIR)$(DATADIR)" 2>/dev/null

app:
	packaging/macos/build-app.sh

verify-app:
	packaging/macos/verify-app.sh dist/mbar.app

dmg: app
	packaging/macos/make-dmg.sh

clean:
	$(CARGO) clean
	cd $(UI_DIR) && $(CARGO) clean
