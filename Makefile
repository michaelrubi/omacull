PREFIX ?= $(HOME)/.local

.PHONY: build test install uninstall

build:
	cargo build --release

test:
	cargo test

# Installs for the current user: binary, launcher entry and icon. Does not
# change which app opens folders or raws by default.
install: build
	install -Dm755 target/release/omacull $(PREFIX)/bin/omacull
	install -Dm644 assets/omacull.desktop $(PREFIX)/share/applications/omacull.desktop
	install -Dm644 assets/omacull.svg $(PREFIX)/share/icons/hicolor/scalable/apps/omacull.svg
	-update-desktop-database $(PREFIX)/share/applications 2>/dev/null

uninstall:
	rm -f $(PREFIX)/bin/omacull $(PREFIX)/share/applications/omacull.desktop \
		$(PREFIX)/share/icons/hicolor/scalable/apps/omacull.svg
