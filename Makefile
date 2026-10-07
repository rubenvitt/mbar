# mbar: build, test and install helpers.
#
#   make release                       optimized build of the bar (target/release/mbar)
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

.PHONY: all help build release test lint fmt ui ui-test install install-ui \
        install-agent uninstall-agent uninstall clean

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
	@test -x target/release/mbar || { echo "target/release/mbar not found: run 'make release' first"; exit 1; }
	install -d "$(DESTDIR)$(BINDIR)" "$(DESTDIR)$(DATADIR)"
	install -m 755 target/release/mbar "$(DESTDIR)$(BINDIR)/mbar"
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
	@test -x $(UI_DIR)/target/release/mbar-ui || { echo "run 'make ui' first"; exit 1; }
	install -d "$(DESTDIR)$(BINDIR)"
	install -m 755 $(UI_DIR)/target/release/mbar-ui "$(DESTDIR)$(BINDIR)/mbar-ui"

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

clean:
	$(CARGO) clean
	cd $(UI_DIR) && $(CARGO) clean
