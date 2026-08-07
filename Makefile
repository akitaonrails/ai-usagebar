PREFIX ?= /usr/local

# The plugin Id is also the install directory name, so it must match
# KPlugin.Id in kde-plasmoid/package/metadata.json exactly.
PLASMOID_ID ?= io.github.akitaonrails.ai-usagebar
PLASMOID_DIR = $(DESTDIR)$(PREFIX)/share/plasma/plasmoids/$(PLASMOID_ID)

.PHONY: build install uninstall install-plasmoid uninstall-plasmoid \
	test desktop-test qml-lint qml-test mjs-probe smoke clippy fmt clean

build:
	cargo build --release

install: build
	install -Dm755 target/release/ai-usagebar     $(DESTDIR)$(PREFIX)/bin/ai-usagebar
	install -Dm755 target/release/ai-usagebar-tui $(DESTDIR)$(PREFIX)/bin/ai-usagebar-tui
	install -Dm644 config.example.toml            $(DESTDIR)$(PREFIX)/share/ai-usagebar/config.example.toml
	install -Dm644 README.md                      $(DESTDIR)$(PREFIX)/share/doc/ai-usagebar/README.md
	install -Dm644 LICENSE                        $(DESTDIR)$(PREFIX)/share/licenses/ai-usagebar/LICENSE

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/ai-usagebar
	rm -f $(DESTDIR)$(PREFIX)/bin/ai-usagebar-tui
	rm -rf $(DESTDIR)$(PREFIX)/share/ai-usagebar
	rm -rf $(DESTDIR)$(PREFIX)/share/doc/ai-usagebar
	rm -rf $(DESTDIR)$(PREFIX)/share/licenses/ai-usagebar

# Deliberately NOT part of `install`: that target is what a Sway or GNOME user
# runs to get the CLI, and dropping a plasmoid into /usr/share/plasma on a
# machine with no Plasma is rude. KDE users run `make install install-plasmoid`.
#
# NOTE: KPackage only scans $XDG_DATA_DIRS, which on a stock Plasma session does
# NOT include /usr/local/share — a system install needs PREFIX=/usr.
#
# A tree walk rather than the explicit `install -Dm644` lines used above: the
# CLI's five artifacts are stable, but a plasmoid grows, and a .qml missing from
# an explicit list fails at *runtime* in plasmashell, which no test here catches.
install-plasmoid:
	cd kde-plasmoid/package && find metadata.json contents -type f \
	  -exec install -Dm644 {} $(PLASMOID_DIR)/{} \;

uninstall-plasmoid:
	rm -rf $(PLASMOID_DIR)

test:
	cargo test
	$(MAKE) desktop-test

desktop-test:
	node gnome-extension/marker-logic.test.mjs
	node kde-plasmoid/plasmoid-logic.test.mjs

# Debian/Ubuntu put the Qt 6 tools in /usr/lib/qt6/bin, which is not on PATH.
QMLLINT ?= $(shell command -v qmllint 2>/dev/null || echo /usr/lib/qt6/bin/qmllint)

# Kept out of desktop-test on purpose: that gate runs on the Windows CI job too
# and must need nothing but node, whereas these need Qt (qt6-declarative-dev-tools).
# --unqualified disable: i18n/i18nc are injected into every applet by the Plasma
# runtime, so qmllint flags every translated string as an unqualified access.
# Known remaining false positive: it cannot resolve the list type of the
# Plasmoid.contextualActions attached property, though the syntax used matches
# org.kde.plasma.systemmonitor and org.kde.kupapplet verbatim.
qml-lint:
	$(QMLLINT) --unqualified disable kde-plasmoid/package/contents/ui/*.qml

QMLTESTRUNNER ?= $(shell command -v qmltestrunner 2>/dev/null || echo /usr/lib/qt6/bin/qmltestrunner)

# Instantiates the visual components for real and asserts what they paint —
# segment widths, positions and colours. Only possible for the components that
# never touch the Plasmoid attached property: main.qml genuinely cannot be
# tested this way, because the applet host injects that at runtime and KDE
# documents no way to mock it. UsageBar.qml does not, so it can.
#
# Offscreen so it needs no display, but it still needs Qt and Kirigami, which is
# why it stays out of desktop-test and out of CI (ubuntu-latest is 24.04 and
# ships Plasma 5, with no Plasma 6 QML modules at all).
qml-test:
	QT_QPA_PLATFORM=offscreen $(QMLTESTRUNNER) -input kde-plasmoid/qmltests

# Loads the package in a real Plasma applet host and runs the checks in
# contents/ui/main.qml. This is the only way to catch the V4-vs-V8 engine
# differences — Node accepts `catch {` and \p{...}, QML does not (the latter
# silently, which is why an automated check exists at all). Needs plasma-sdk
# and a running session; plasmoidviewer is a GUI app, hence the timeout.
mjs-probe:
	QT_LOGGING_RULES="qml.debug=true" timeout 15 plasmoidviewer \
	  -a kde-plasmoid/probe -l topedge -f horizontal 2>&1 \
	  | grep -E "^qml: (ok|FAIL)|MJS PROBE" || true

smoke:
	@echo "Running live API smoke tests (requires creds in shell env)..."
	cargo test --test live -- --ignored --nocapture

clippy:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt

clean:
	cargo clean
