PREFIX ?= $(HOME)/.local

.PHONY: build test install uninstall

build:
	cargo build --release

test:
	cargo test

# Installs the binary for the current user.
install: build
	install -Dm755 target/release/omacull $(PREFIX)/bin/omacull

uninstall:
	rm -f $(PREFIX)/bin/omacull
