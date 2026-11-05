PREFIX  ?= $(HOME)/.local
BINDIR  ?= $(PREFIX)/bin
DATADIR ?= $(PREFIX)/share

.PHONY: build test install uninstall default

build:
	cargo build --release --locked

test:
	cargo test --locked

# Installs the binary, launcher entry and icon. The launcher entry points at
# the installed binary by absolute path, so it works even when BINDIR is not
# on the session's PATH.
install: build
	install -Dm755 target/release/lightdown $(DESTDIR)$(BINDIR)/lightdown
	install -Dm644 assets/lightdown.svg $(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/lightdown.svg
	install -d $(DESTDIR)$(DATADIR)/applications
	sed 's|^Exec=lightdown|Exec=$(BINDIR)/lightdown|' assets/lightdown.desktop \
		> $(DESTDIR)$(DATADIR)/applications/lightdown.desktop
	-test -n "$(DESTDIR)" || update-desktop-database -q $(DATADIR)/applications

uninstall:
	rm -f $(DESTDIR)$(BINDIR)/lightdown
	rm -f $(DESTDIR)$(DATADIR)/applications/lightdown.desktop
	rm -f $(DESTDIR)$(DATADIR)/icons/hicolor/scalable/apps/lightdown.svg
	-test -n "$(DESTDIR)" || update-desktop-database -q $(DATADIR)/applications

# Makes Lightdown the default application for markdown files.
default:
	xdg-mime default lightdown.desktop text/markdown text/x-markdown
