// Preferences (libadwaita) for AI Usage Bar.

import Adw from 'gi://Adw';
import Gtk from 'gi://Gtk';
import Gdk from 'gi://Gdk';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import {ExtensionPreferences, gettext as _} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

// ── Vendor login / config ────────────────────────────────────────────────
const VENDOR_AUTH = [
    {id: 'anthropic', name: 'Claude', kind: 'oauth', cli: 'claude', login: 'claude', pkg: '@anthropic-ai/claude-code'},
    {id: 'openai', name: 'Codex', kind: 'oauth', cli: 'codex', login: 'codex login', pkg: '@openai/codex'},
    // Local-server vendor: there is no separate credential or npm-installable
    // login helper. If `agy` exists we can open it; the app and IDE are equally
    // valid sources and are managed outside this extension.
    {id: 'antigravity', name: 'Google Antigravity', kind: 'local', cli: 'agy'},
    {id: 'zai', name: 'Z.AI (GLM)', kind: 'apikey', env: 'ZAI_API_KEY'},
    {id: 'openrouter', name: 'OpenRouter', kind: 'apikey', env: 'OPENROUTER_API_KEY'},
    {id: 'deepseek', name: 'DeepSeek', kind: 'apikey', env: 'DEEPSEEK_API_KEY'},
];

// The config file the Rust binary would actually read. It resolves the
// platform config dir, which honors $XDG_CONFIG_HOME — hard-coding
// ~/.config meant a user with XDG_CONFIG_HOME set saw "no key configured"
// for keys that were configured. Falls back to the legacy path so a config
// written before this is still found.
function configPath() {
    const xdg = `${GLib.get_user_config_dir()}/ai-usagebar/config.toml`;
    if (GLib.file_test(xdg, GLib.FileTest.EXISTS))
        return xdg;
    return `${GLib.get_home_dir()}/.config/ai-usagebar/config.toml`;
}

// Does the config have an uncommented api_key in [section]?
function configHasApiKey(section) {
    const path = configPath();
    if (!GLib.file_test(path, GLib.FileTest.EXISTS))
        return false;
    try {
        const [ok, bytes] = GLib.file_get_contents(path);
        if (!ok)
            return false;
        let inSection = false;
        for (const raw of new TextDecoder().decode(bytes).split('\n')) {
            const t = raw.trim();
            if (t.startsWith('['))
                inSection = t === `[${section}]`;
            else if (inSection && !t.startsWith('#') && /^api_key\s*=\s*["']\S/.test(t))
                return true;
        }
    } catch (e) {}
    return false;
}

function vendorConfigured(v) {
    const home = GLib.get_home_dir();
    if (v.id === 'anthropic')
        return GLib.file_test(`${home}/.claude/.credentials.json`, GLib.FileTest.EXISTS);
    if (v.id === 'openai')
        return GLib.file_test(`${home}/.codex/auth.json`, GLib.FileTest.EXISTS);
    // Antigravity 2.0, the `agy` CLI and the IDE are separate products sharing
    // one quota, and any combination may be installed. Having any of their
    // state directories is enough — the binary reads usage from whichever
    // local server is running, so there is no credential file to look for.
    if (v.id === 'antigravity') {
        return ['antigravity', 'antigravity-cli', 'antigravity-ide']
            .some(d => GLib.file_test(`${home}/.gemini/${d}`, GLib.FileTest.IS_DIR));
    }
    return (GLib.getenv(v.env) || '').length > 0 || configHasApiKey(v.id);
}

// Open the user's terminal running `command` (login shell, so claude/codex are on PATH).
function spawnInTerminal(command) {
    for (const argv of [
        ['kgx', '--', 'bash', '-lc', command],
        ['gnome-terminal', '--', 'bash', '-lc', command],
        ['xterm', '-e', 'bash', '-lc', command],
    ]) {
        if (!GLib.find_program_in_path(argv[0]))
            continue;
        try {
            Gio.Subprocess.new(argv, Gio.SubprocessFlags.NONE);
            return true;
        } catch (e) {}
    }
    return false;
}

function spawnArgvInTerminal(commandArgv) {
    for (const argv of [
        ['kgx', '--', ...commandArgv],
        ['gnome-terminal', '--', ...commandArgv],
        ['xterm', '-e', ...commandArgv],
    ]) {
        if (!GLib.find_program_in_path(argv[0]))
            continue;
        try {
            Gio.Subprocess.new(argv, Gio.SubprocessFlags.NONE);
            return true;
        } catch (e) {}
    }
    return false;
}

// Is `cli` on the login-shell PATH? (npm-global / nvm bins often aren't on the
// prefs process PATH, so we ask a login shell.)
function checkCliInstalled(cli, callback) {
    try {
        const p = Gio.Subprocess.new(
            ['bash', '-lc', `command -v ${cli}`],
            Gio.SubprocessFlags.STDOUT_PIPE | Gio.SubprocessFlags.STDERR_SILENCE);
        p.communicate_utf8_async(null, null, (proc, res) => {
            try {
                const [, out] = proc.communicate_utf8_finish(res);
                callback((out || '').trim().length > 0);
            } catch (e) {
                callback(false);
            }
        });
    } catch (e) {
        callback(false);
    }
}

// Terminal script: if the CLI is missing, offer to npm-install it (with
// consent), then run the login. Runs in a login shell so PATH is complete.
function oauthCommand(v) {
    // Installs to ~/.local (already on PATH) so no sudo is needed even when the
    // system npm prefix is root-owned (/usr/lib/node_modules).
    return [
        `export PATH="$HOME/.local/bin:$PATH";`,
        `if command -v ${v.cli} >/dev/null 2>&1; then ${v.login};`,
        `else echo "⚠ ${v.cli} not found.";`,
        `echo "I can install it under ~/.local without sudo (npm --prefix). Package: ${v.pkg}"; echo;`,
        `read -p "Install now? [y/N] " a;`,
        `if [ "$a" = y ] || [ "$a" = Y ]; then npm i -g --prefix "$HOME/.local" ${v.pkg} && hash -r && ${v.login}; fi;`,
        `fi;`,
        `echo; read -p "Press Enter to close..."`,
    ].join(' ');
}

// Bind an Adw.ComboRow (index-based) to a string GSetting via a value table.
function bindCombo(settings, key, comboRow, values) {
    const sync = () => {
        const idx = values.indexOf(settings.get_string(key));
        comboRow.selected = idx < 0 ? 0 : idx;
    };
    sync();
    comboRow.connect('notify::selected', () => {
        const v = values[comboRow.selected];
        if (v !== undefined && v !== settings.get_string(key))
            settings.set_string(key, v);
    });
    settings.connect(`changed::${key}`, sync);
}

function rgbaToHex(rgba) {
    const h = v => Math.round(Math.max(0, Math.min(1, v)) * 255).toString(16).padStart(2, '0');
    return `#${h(rgba.red)}${h(rgba.green)}${h(rgba.blue)}`;
}

// A row with a GTK color picker bound to a hex-string GSetting.
function colorRow(settings, key, title) {
    const row = new Adw.ActionRow({title});
    const btn = new Gtk.ColorDialogButton({
        dialog: new Gtk.ColorDialog({with_alpha: false}),
        valign: Gtk.Align.CENTER,
    });
    let updating = false;
    const load = () => {
        const rgba = new Gdk.RGBA();
        if (rgba.parse(settings.get_string(key))) {
            updating = true;
            btn.set_rgba(rgba);
            updating = false;
        }
    };
    load();
    btn.connect('notify::rgba', () => {
        if (!updating)
            settings.set_string(key, rgbaToHex(btn.get_rgba()));
    });
    settings.connect(`changed::${key}`, load);
    row.add_suffix(btn);
    row.activatable_widget = btn;
    return row;
}

export default class AiUsageBarPrefs extends ExtensionPreferences {
    fillPreferencesWindow(window) {
        const settings = this.getSettings();
        const page = new Adw.PreferencesPage();
        window.add(page);

        // ── Display ──────────────────────────────────────────────────────
        const display = new Adw.PreferencesGroup({title: _('Display')});
        page.add(display);

        const showSession = new Adw.SwitchRow({title: _('Show 5h (session) bar')});
        settings.bind('show-session', showSession, 'active', Gio.SettingsBindFlags.DEFAULT);
        display.add(showSession);

        const showWeekly = new Adw.SwitchRow({title: _('Show weekly bar')});
        settings.bind('show-weekly', showWeekly, 'active', Gio.SettingsBindFlags.DEFAULT);
        display.add(showWeekly);

        const showExtra = new Adw.SwitchRow({
            title: _('Show extra usage bar (3rd)'),
            subtitle: _('o custo extra ($) como terceira barra'),
        });
        settings.bind('show-extra', showExtra, 'active', Gio.SettingsBindFlags.DEFAULT);
        display.add(showExtra);

        // Only Antigravity reports two independent pools today; for every other
        // vendor these rows are inert, which the subtitle spells out.
        const poolLabels = [_('Both'), _('First only'), _('Second only'), _('Automatic')];
        const poolValues = ['both', 'primary', 'secondary', 'auto'];
        const pools = new Adw.ComboRow({
            title: _('Pools no painel'),
            subtitle: _('for providers with two independent pools (e.g. Antigravity: Gemini and Claude & GPT OSS)'),
            model: Gtk.StringList.new(poolLabels),
        });
        bindCombo(settings, 'panel-pools', pools, poolValues);
        display.add(pools);

        const autoThreshold = new Adw.SpinRow({
            title: _('Automatic mode threshold (%)'),
            subtitle: _('switches to the other pool when the first passes this usage'),
            adjustment: new Gtk.Adjustment({lower: 50, upper: 100, step_increment: 1, page_increment: 5}),
        });
        settings.bind('panel-auto-threshold', autoThreshold, 'value', Gio.SettingsBindFlags.DEFAULT);
        display.add(autoThreshold);

        const syncThreshold = () =>
            autoThreshold.set_sensitive(settings.get_string('panel-pools') === 'auto');
        syncThreshold();
        settings.connect('changed::panel-pools', syncThreshold);

        const showPercent = new Adw.SwitchRow({title: _('Show percentage/value')});
        settings.bind('show-percent', showPercent, 'active', Gio.SettingsBindFlags.DEFAULT);
        display.add(showPercent);

        const showBars = new Adw.SwitchRow({
            title: _('Show bars'),
            subtitle: _('off = numbers only, no bars'),
        });
        settings.bind('show-bars', showBars, 'active', Gio.SettingsBindFlags.DEFAULT);
        display.add(showBars);

        const barWidth = new Adw.SpinRow({
            title: _('Width of each bar (cells)'),
            adjustment: new Gtk.Adjustment({lower: 4, upper: 20, step_increment: 1, page_increment: 2}),
        });
        settings.bind('bar-width', barWidth, 'value', Gio.SettingsBindFlags.DEFAULT);
        display.add(barWidth);

        // ── Colours ────────────────────────────────────────────────────────
        const colors = new Adw.PreferencesGroup({
            title: _('Colours'),
            description: _('Bar colour by usage band (One Dark by default).'),
        });
        page.add(colors);
        colors.add(colorRow(settings, 'color-low', _('Low (<50%)')));
        colors.add(colorRow(settings, 'color-mid', _('Medium (50–74%)')));
        colors.add(colorRow(settings, 'color-high', _('High (75–89%)')));
        colors.add(colorRow(settings, 'color-critical', _('Critical (≥90%)')));
        colors.add(colorRow(settings, 'color-empty', _('Empty (bar background)')));

        // ── Data ────────────────────────────────────────────────────────
        const data = new Adw.PreferencesGroup({title: _('Data')});
        page.add(data);

        const interval = new Adw.SpinRow({
            title: _('Refresh interval (s)'),
            adjustment: new Gtk.Adjustment({lower: 5, upper: 3600, step_increment: 5, page_increment: 30}),
        });
        settings.bind('refresh-interval', interval, 'value', Gio.SettingsBindFlags.DEFAULT);
        data.add(interval);

        const vendorList = ['anthropic', 'openai', 'zai', 'openrouter', 'deepseek', 'antigravity'];
        const vendor = new Adw.ComboRow({
            title: _('Vendor'),
            subtitle: _('anthropic and antigravity expose the 5h + weekly windows'),
            model: Gtk.StringList.new(vendorList),
        });
        bindCombo(settings, 'vendor', vendor, vendorList);
        data.add(vendor);

        const binPath = new Adw.EntryRow({title: _('Binary path (empty = auto)')});
        settings.bind('binary-path', binPath, 'text', Gio.SettingsBindFlags.DEFAULT);
        data.add(binPath);

        // ── Position ─────────────────────────────────────────────────────
        const pos = new Adw.PreferencesGroup({
            title: _('Panel position'),
            description: _('Changes apply immediately.'),
        });
        page.add(pos);

        const box = new Adw.ComboRow({
            title: _('Area'),
            subtitle: _('right = next to the network/clock'),
            model: Gtk.StringList.new(['left', 'center', 'right']),
        });
        bindCombo(settings, 'panel-box', box, ['left', 'center', 'right']);
        pos.add(box);

        const index = new Adw.SpinRow({
            title: _('Index within the area'),
            subtitle: _('0 = leftmost in the chosen area'),
            adjustment: new Gtk.Adjustment({lower: 0, upper: 20, step_increment: 1, page_increment: 1}),
        });
        settings.bind('panel-index', index, 'value', Gio.SettingsBindFlags.DEFAULT);
        pos.add(index);

        this._buildVendorsPage(window);
    }

    // A "Vendors" tab: per-vendor credential status + a login/config button.
    _buildVendorsPage(window) {
        const page = new Adw.PreferencesPage({
            title: _('Vendors'),
            icon_name: 'dialog-password-symbolic',
        });
        window.add(page);

        const group = new Adw.PreferencesGroup({
            title: _('Per-provider login / setup'),
            description: _('OAuth opens a terminal with the login command; API-key providers are configured in the TUI. Reopen this window to re-check status.'),
        });
        page.add(group);

        const updates = [];
        for (const v of VENDOR_AUTH) {
            const row = new Adw.ActionRow({title: v.name});
            const btn = new Gtk.Button({valign: Gtk.Align.CENTER});
            btn.add_css_class('flat');
            row.add_suffix(btn);

            const update = () => {
                if (v.kind === 'local') {
                    const productDetected = vendorConfigured(v);
                    row.subtitle = _('verificando…');
                    checkCliInstalled(v.cli, (installed) => {
                        if (productDetected) {
                            row.subtitle = _('✓ Antigravity detectado — mantenha o app, IDE ou agy aberto');
                        } else if (installed) {
                            row.subtitle = _('agy installed — open a session to make the quota available');
                        } else {
                            row.subtitle = _('open or install the app, the IDE or agy; there is no separate login');
                        }
                        btn.label = installed ? _('Open agy') : _('No separate login');
                        btn.sensitive = installed;
                    });
                    return;
                }
                btn.sensitive = true;
                if (v.kind !== 'oauth') {
                    const ok = vendorConfigured(v);
                    row.subtitle = ok ? _('✓ Configured') : `⚠ ${_('No API key')} — ${v.env}`;
                    btn.label = _('Configurar (TUI)');
                    return;
                }
                if (vendorConfigured(v)) {
                    row.subtitle = _('✓ Configured');
                    btn.label = _('Re-logar');
                    return;
                }
                row.subtitle = _('verificando…');
                checkCliInstalled(v.cli, (installed) => {
                    row.subtitle = installed
                        ? `⚠ ${_('Not signed in')} — \`${v.login}\``
                        : `⚠ ${v.cli} ${_('not installed')} (installs under ~/.local, no sudo)`;
                    btn.label = installed ? _('Sign in') : _('Install + sign in');
                });
            };
            update();
            updates.push(update);

            btn.connect('clicked', () => {
                if (v.kind === 'oauth') {
                    spawnInTerminal(oauthCommand(v));
                } else if (v.kind === 'local') {
                    spawnArgvInTerminal([v.cli]);
                } else {
                    const tui = GLib.find_program_in_path('ai-usagebar-tui') ||
                        `${GLib.get_home_dir()}/.cargo/bin/ai-usagebar-tui`;
                    spawnArgvInTerminal([tui]);
                }
                GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 4, () => {
                    update();
                    return GLib.SOURCE_REMOVE;
                });
            });

            group.add(row);
        }

        // Re-check when the window regains focus (e.g., after logging in via
        // the terminal) — fixes the "still shows not signed in" loop.
        window.connect('notify::is-active', () => {
            if (window.is_active)
                updates.forEach(u => u());
        });
    }
}
