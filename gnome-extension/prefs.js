// Preferences (libadwaita) for AI Usage Bar.

import Adw from 'gi://Adw';
import Gtk from 'gi://Gtk';
import Gdk from 'gi://Gdk';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import {ExtensionPreferences, gettext as _} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

// ── Vendor login / config ────────────────────────────────────────────────
const VENDOR_AUTH = [
    {id: 'anthropic', name: 'Anthropic (Claude)', kind: 'oauth', cli: 'claude', login: 'claude', pkg: '@anthropic-ai/claude-code'},
    {id: 'openai', name: 'OpenAI (Codex)', kind: 'oauth', cli: 'codex', login: 'codex login', pkg: '@openai/codex'},
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
        `else echo "⚠ ${v.cli} nao encontrado.";`,
        `echo "Instalo em ~/.local sem sudo (npm --prefix). Pacote: ${v.pkg}"; echo;`,
        `read -p "Instalar agora? [y/N] " a;`,
        `if [ "$a" = y ] || [ "$a" = Y ]; then npm i -g --prefix "$HOME/.local" ${v.pkg} && hash -r && ${v.login}; fi;`,
        `fi;`,
        `echo; read -p "Enter para fechar..."`,
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
        const display = new Adw.PreferencesGroup({title: _('Exibição')});
        page.add(display);

        const showSession = new Adw.SwitchRow({title: _('Mostrar barra de 5h (sessão)')});
        settings.bind('show-session', showSession, 'active', Gio.SettingsBindFlags.DEFAULT);
        display.add(showSession);

        const showWeekly = new Adw.SwitchRow({title: _('Mostrar barra semanal')});
        settings.bind('show-weekly', showWeekly, 'active', Gio.SettingsBindFlags.DEFAULT);
        display.add(showWeekly);

        const showExtra = new Adw.SwitchRow({
            title: _('Mostrar barra de uso extra (3ª)'),
            subtitle: _('o custo extra ($) como terceira barra'),
        });
        settings.bind('show-extra', showExtra, 'active', Gio.SettingsBindFlags.DEFAULT);
        display.add(showExtra);

        // Only Antigravity reports two independent pools today; for every other
        // vendor these rows are inert, which the subtitle spells out.
        const poolLabels = [_('Ambos'), _('Só o primeiro'), _('Só o segundo'), _('Automático')];
        const poolValues = ['both', 'primary', 'secondary', 'auto'];
        const pools = new Adw.ComboRow({
            title: _('Pools no painel'),
            subtitle: _('para vendors com dois pools independentes (ex.: Antigravity: Gemini e Claude & GPT OSS)'),
            model: Gtk.StringList.new(poolLabels),
        });
        bindCombo(settings, 'panel-pools', pools, poolValues);
        display.add(pools);

        const autoThreshold = new Adw.SpinRow({
            title: _('Limiar do modo automático (%)'),
            subtitle: _('troca para o outro pool quando o primeiro passa deste uso'),
            adjustment: new Gtk.Adjustment({lower: 50, upper: 100, step_increment: 1, page_increment: 5}),
        });
        settings.bind('panel-auto-threshold', autoThreshold, 'value', Gio.SettingsBindFlags.DEFAULT);
        display.add(autoThreshold);

        const syncThreshold = () =>
            autoThreshold.set_sensitive(settings.get_string('panel-pools') === 'auto');
        syncThreshold();
        settings.connect('changed::panel-pools', syncThreshold);

        const showPercent = new Adw.SwitchRow({title: _('Mostrar porcentagem/valor')});
        settings.bind('show-percent', showPercent, 'active', Gio.SettingsBindFlags.DEFAULT);
        display.add(showPercent);

        const showBars = new Adw.SwitchRow({
            title: _('Mostrar barras'),
            subtitle: _('desligado = só os números, sem as barras'),
        });
        settings.bind('show-bars', showBars, 'active', Gio.SettingsBindFlags.DEFAULT);
        display.add(showBars);

        const barWidth = new Adw.SpinRow({
            title: _('Largura de cada barra (células)'),
            adjustment: new Gtk.Adjustment({lower: 4, upper: 20, step_increment: 1, page_increment: 2}),
        });
        settings.bind('bar-width', barWidth, 'value', Gio.SettingsBindFlags.DEFAULT);
        display.add(barWidth);

        // ── Cores ────────────────────────────────────────────────────────
        const colors = new Adw.PreferencesGroup({
            title: _('Cores'),
            description: _('Cor da barra por faixa de uso (One Dark por padrão).'),
        });
        page.add(colors);
        colors.add(colorRow(settings, 'color-low', _('Baixo (<50%)')));
        colors.add(colorRow(settings, 'color-mid', _('Médio (50–74%)')));
        colors.add(colorRow(settings, 'color-high', _('Alto (75–89%)')));
        colors.add(colorRow(settings, 'color-critical', _('Crítico (≥90%)')));
        colors.add(colorRow(settings, 'color-empty', _('Vazio (fundo da barra)')));

        // ── Dados ────────────────────────────────────────────────────────
        const data = new Adw.PreferencesGroup({title: _('Dados')});
        page.add(data);

        const interval = new Adw.SpinRow({
            title: _('Intervalo de atualização (s)'),
            adjustment: new Gtk.Adjustment({lower: 5, upper: 3600, step_increment: 5, page_increment: 30}),
        });
        settings.bind('refresh-interval', interval, 'value', Gio.SettingsBindFlags.DEFAULT);
        data.add(interval);

        const vendorList = ['anthropic', 'openai', 'zai', 'openrouter', 'deepseek', 'antigravity'];
        const vendor = new Adw.ComboRow({
            title: _('Vendor'),
            subtitle: _('anthropic e antigravity expõem as janelas de 5h + semanal'),
            model: Gtk.StringList.new(vendorList),
        });
        bindCombo(settings, 'vendor', vendor, vendorList);
        data.add(vendor);

        const binPath = new Adw.EntryRow({title: _('Caminho do binário (vazio = auto)')});
        settings.bind('binary-path', binPath, 'text', Gio.SettingsBindFlags.DEFAULT);
        data.add(binPath);

        // ── Position ─────────────────────────────────────────────────────
        const pos = new Adw.PreferencesGroup({
            title: _('Posição no painel'),
            description: _('Mudanças aplicam na hora.'),
        });
        page.add(pos);

        const box = new Adw.ComboRow({
            title: _('Área'),
            subtitle: _('right = ao lado da rede/relógio'),
            model: Gtk.StringList.new(['left', 'center', 'right']),
        });
        bindCombo(settings, 'panel-box', box, ['left', 'center', 'right']);
        pos.add(box);

        const index = new Adw.SpinRow({
            title: _('Índice na área'),
            subtitle: _('0 = mais à esquerda da área escolhida'),
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
            title: _('Login / configuração por vendor'),
            description: _('OAuth abre um terminal com o comando de login; vendors de API key são configurados no TUI. Reabra esta janela para reavaliar o status.'),
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
                            row.subtitle = _('agy instalado — abra uma sessão para disponibilizar a quota');
                        } else {
                            row.subtitle = _('abra ou instale o app, a IDE ou o agy; não há login separado');
                        }
                        btn.label = installed ? _('Abrir agy') : _('Sem login separado');
                        btn.sensitive = installed;
                    });
                    return;
                }
                btn.sensitive = true;
                if (v.kind !== 'oauth') {
                    const ok = vendorConfigured(v);
                    row.subtitle = ok ? _('✓ Configurado') : `⚠ ${_('Sem API key')} — ${v.env}`;
                    btn.label = _('Configurar (TUI)');
                    return;
                }
                if (vendorConfigured(v)) {
                    row.subtitle = _('✓ Configurado');
                    btn.label = _('Re-logar');
                    return;
                }
                row.subtitle = _('verificando…');
                checkCliInstalled(v.cli, (installed) => {
                    row.subtitle = installed
                        ? `⚠ ${_('Não logado')} — \`${v.login}\``
                        : `⚠ ${v.cli} ${_('não instalado')} (instala em ~/.local, sem sudo)`;
                    btn.label = installed ? _('Logar') : _('Instalar + logar');
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
        // the terminal) — fixes the "still shows não logado" loop.
        window.connect('notify::is-active', () => {
            if (window.is_active)
                updates.forEach(u => u());
        });
    }
}
