PREFIX ?= /usr/local

# The plugin Id is also the install directory name, so it must match
# KPlugin.Id in kde-plasmoid/package/metadata.json exactly.
PLASMOID_ID ?= io.github.akitaonrails.ai-usagebar
# abspath because install-plasmoid cd's into the package before walking it: a
# relative DESTDIR or PREFIX would otherwise resolve against that directory and
# quietly install the tree inside kde-plasmoid/package/ instead.
PLASMOID_DIR = $(abspath $(DESTDIR)$(PREFIX))/share/plasma/plasmoids/$(PLASMOID_ID)

.PHONY: build install uninstall install-plasmoid uninstall-plasmoid \
	test desktop-test plugin-test qml-lint qml-test mjs-probe smoke clippy fmt clean

build:
	cargo build --release

install: build
	install -Dm755 -- target/release/ai-usagebar     "$(DESTDIR)$(PREFIX)/bin/ai-usagebar"
	install -Dm755 -- target/release/ai-usagebar-tui "$(DESTDIR)$(PREFIX)/bin/ai-usagebar-tui"
	install -Dm644 -- config.example.toml            "$(DESTDIR)$(PREFIX)/share/ai-usagebar/config.example.toml"
	install -Dm644 -- README.md                      "$(DESTDIR)$(PREFIX)/share/doc/ai-usagebar/README.md"
	install -Dm644 -- LICENSE                        "$(DESTDIR)$(PREFIX)/share/licenses/ai-usagebar/LICENSE"

uninstall:
	rm -f -- "$(DESTDIR)$(PREFIX)/bin/ai-usagebar"
	rm -f -- "$(DESTDIR)$(PREFIX)/bin/ai-usagebar-tui"
	rm -rf -- "$(DESTDIR)$(PREFIX)/share/ai-usagebar"
	rm -rf -- "$(DESTDIR)$(PREFIX)/share/doc/ai-usagebar"
	rm -rf -- "$(DESTDIR)$(PREFIX)/share/licenses/ai-usagebar"

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
	  -exec install -Dm644 -- {} "$(PLASMOID_DIR)/{}" \;

uninstall-plasmoid:
	rm -rf -- "$(PLASMOID_DIR)"

test:
	cargo test
	$(MAKE) desktop-test
	$(MAKE) plugin-test

desktop-test:
	node gnome-extension/marker-logic.test.mjs
	node kde-plasmoid/plasmoid-logic.test.mjs
	node windows/popover/popover.test.mjs

plugin-test:
	node omarchy/model.test.mjs

# Prefer Qt 6-specific locations. Some distributions put Qt 5 binaries on PATH
# under the generic names while keeping Qt 6 under /usr/lib/qt6/bin.
QT6_BINDIR := $(shell command -v qtpaths6 >/dev/null 2>&1 && qtpaths6 --binaries-dir 2>/dev/null)
PATH_QMLLINT6 := $(shell p=$$(command -v qmllint 2>/dev/null); \
	[ -n "$$p" ] && "$$p" --version 2>&1 | grep -Eq '(^|[[:space:]])6\.' && printf '%s' "$$p")
QT6_TOOL_DIRS := $(QT6_BINDIR) /usr/lib/qt6/bin /usr/lib64/qt6/bin $(dir $(PATH_QMLLINT6))
QMLLINT ?= $(firstword $(foreach d,$(QT6_TOOL_DIRS),$(wildcard $(d)/qmllint)) \
	$(shell command -v qmllint6 2>/dev/null))

# Kept out of desktop-test on purpose: that gate runs on the Windows CI job too
# and must need nothing but node, whereas these need Qt (qt6-declarative-dev-tools).
# --unqualified disable: i18n/i18nc are injected into every applet by the Plasma
# runtime, so qmllint flags every translated string as an unqualified access.
# Known remaining false positive: it cannot resolve the list type of the
# Plasmoid.contextualActions attached property, though the syntax used matches
# org.kde.plasma.systemmonitor and org.kde.kupapplet verbatim.
qml-lint:
	@test -x "$(QMLLINT)" || { echo "Qt 6 qmllint not found" >&2; exit 1; }
	@"$(QMLLINT)" --version 2>&1 | grep -Eq '(^|[[:space:]])6\.' || \
	  { echo "$(QMLLINT) is not Qt 6 qmllint" >&2; exit 1; }
	"$(QMLLINT)" --unqualified disable \
	  kde-plasmoid/package/contents/config/*.qml \
	  kde-plasmoid/package/contents/ui/*.qml

QMLTESTRUNNER ?= $(firstword $(foreach d,$(QT6_TOOL_DIRS),$(wildcard $(d)/qmltestrunner)) \
	$(shell command -v qmltestrunner6 2>/dev/null))

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
	@test -x "$(QMLTESTRUNNER)" || { echo "Qt 6 qmltestrunner not found" >&2; exit 1; }
	QT_QPA_PLATFORM=offscreen "$(QMLTESTRUNNER)" -input kde-plasmoid/qmltests

# Loads the package in a real Plasma applet host and runs the checks in
# contents/ui/main.qml. This is the only way to catch the V4-vs-V8 engine
# differences — Node accepts `catch {` and \p{...}, QML does not (the latter
# silently, which is why an automated check exists at all). Needs plasma-sdk
# and a running session; plasmoidviewer is a GUI app, hence the timeout.
#
# This is a real gate, not a report: it used to end in `|| true`, so it printed
# FAIL and still exited 0. grep's empty-match exit and plasmoidviewer's timeout
# code are both meaningless here, so the verdict comes from the probe's own
# markers instead — BOTH must be present and no line may say FAIL.
mjs-probe:
	@out=$$(QT_LOGGING_RULES="qml.debug=true" timeout 15 plasmoidviewer \
	  -a kde-plasmoid/probe -l topedge -f horizontal 2>&1 \
	  | grep -E "^qml: (ok|FAIL)|MJS PROBE" || true); \
	printf '%s\n' "$$out"; \
	case "$$out" in *FAIL*) \
	  echo "✗ mjs-probe: a check failed under the applet host"; exit 1 ;; esac; \
	case "$$out" in *"MJS PROBE OK"*) ;; *) \
	  echo "✗ mjs-probe: no verdict — needs plasma-sdk and a running Plasma session"; \
	  exit 1 ;; esac; \
	case "$$out" in *"MJS PROBE EXEC DONE"*) ;; *) \
	  echo "✗ mjs-probe: the executable-engine check never reported back"; exit 1 ;; esac; \
	echo "✓ mjs-probe: module loads and the timeout wrapper executes under V4"

smoke:
	@echo "Running live API smoke tests (requires creds in shell env)..."
	cargo test --test live -- --ignored --nocapture

clippy:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt

clean:
	cargo clean
