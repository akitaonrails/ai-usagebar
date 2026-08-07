// Pure helpers for the KDE Plasma 6 plasmoid.
//
// Nothing here may import Qt or QML: the file has to stay runnable under plain
// Node so `plasmoid-logic.test.mjs` can cover it. Anything that needs a live
// applet (the injected `Plasmoid` attached property) belongs in the .qml files
// instead — `qmltestrunner` cannot mock that context, so QML is exercised by
// hand via plasmoidviewer, and everything else lives here.
//
// marker-logic.mjs next to this file is a byte-identical copy of the canonical
// gnome-extension/marker-logic.js. Never edit it here.

import {
    FIELD, field, integer, markerElapsed, hasUsageWindows, isGrouped,
    splitFormatOutput, plainTextFromPango, disambiguateTags, selectPools,
} from './marker-logic.mjs';

export const DEFAULT_BINARY = 'ai-usagebar';
export const DEFAULT_VENDOR = 'anthropic';

// The vendors whose usage windows the shared --format contract covers, i.e. the
// same set the GNOME extension supports. Used as the floor for the config
// picker so a vendor that is switched off in config.toml (Antigravity is not
// enabled by default) can still be ticked.
export const KNOWN_VENDORS = [
    'anthropic', 'openai', 'zai', 'openrouter', 'deepseek', 'antigravity',
];

// ---------------------------------------------------------------------------
// Vendor login / configuration.
//
// A deliberate port of the GNOME prefs "Vendors" page (gnome-extension/prefs.js
// VENDOR_AUTH, vendorConfigured, checkCliInstalled, oauthCommand). Table, status
// strings and button labels are copied verbatim so the two settings pages say
// the same thing about the same vendor; only the plumbing differs, because QML
// has no filesystem API and must ask a shell instead.
// ---------------------------------------------------------------------------
export const VENDOR_AUTH = [
    {id: 'anthropic', name: 'Anthropic (Claude)', kind: 'oauth', cli: 'claude', login: 'claude', pkg: '@anthropic-ai/claude-code'},
    {id: 'openai', name: 'OpenAI (Codex)', kind: 'oauth', cli: 'codex', login: 'codex login', pkg: '@openai/codex'},
    // Local-server vendor: there is no separate credential or npm-installable
    // login helper. If `agy` exists we can open it; the app and IDE are equally
    // valid sources and are managed outside this widget.
    {id: 'antigravity', name: 'Google Antigravity', kind: 'local', cli: 'agy'},
    {id: 'zai', name: 'Z.AI (GLM)', kind: 'apikey', env: 'ZAI_API_KEY'},
    {id: 'openrouter', name: 'OpenRouter', kind: 'apikey', env: 'OPENROUTER_API_KEY'},
    {id: 'deepseek', name: 'DeepSeek', kind: 'apikey', env: 'DEEPSEEK_API_KEY'},
];

// Display names come from the table above so the two settings pages name the
// same vendor the same way — a raw slug like "zai" is not what a user
// recognises. Derived rather than kept as a second literal map: the Vendors
// page already renders these names, and a duplicate table let the two pages
// disagree after a rename with nothing to catch it.
//
// Declared *after* VENDOR_AUTH on purpose. Reading a const from a function
// hoisted above it works, but V4 logs `usedbeforedeclared` for it on every
// load, which is noise in the user's journal.
export function vendorLabel(slug) {
    const id = String(slug ?? '').trim();
    const entry = VENDOR_AUTH.find(v => v.id === id);
    return entry ? entry.name : id;
}

// One shell round trip for the whole table: GNOME can call GLib.file_test and
// GLib.getenv directly, QML cannot. Emits `id:configured:cli` per vendor and
// NEVER echoes a key — the api_key test is evaluated in awk and only its
// boolean result crosses the boundary.
export function buildVendorProbeCommand() {
    const lines = ['cfg="${XDG_CONFIG_HOME:-$HOME/.config}/ai-usagebar/config.toml"',
                   '[ -f "$cfg" ] || cfg="$HOME/.config/ai-usagebar/config.toml"',
                   // Mirrors configHasApiKey(): track the section, skip comments,
                   // require api_key = "<non-space>.
                   // The awk program must contain NO literal single quote: it is
                   // itself single-quoted for the shell, so one would close the
                   // block. printf '\047' supplies it through -v instead, and the
                   // regex is built as a dynamic string.
                   'has_key() { [ -f "$cfg" ] || { echo 0; return; }; ' +
                   'awk -v sec="[$1]" -v q="$(printf \'\\047\')" \'{ ' +
                   't=$0; sub(/^[ \\t]+/,"",t); sub(/[ \\t]+$/,"",t); ' +
                   'if (substr(t,1,1)=="[") { ins=(t==sec) } ' +
                   'else if (ins && substr(t,1,1)!="#" && ' +
                   't ~ "^api_key[ \\t]*=[ \\t]*[\\"" q "][^ \\t]") { print 1; found=1; exit } } ' +
                   'END { if (!found) print 0 }\' "$cfg"; }',
                   'cli() { command -v "$1" >/dev/null 2>&1 && echo 1 || echo 0; }'];

    for (const v of VENDOR_AUTH) {
        if (v.id === 'anthropic')
            lines.push('c=$([ -f "$HOME/.claude/.credentials.json" ] && echo 1 || echo 0)');
        else if (v.id === 'openai')
            lines.push('c=$([ -f "$HOME/.codex/auth.json" ] && echo 1 || echo 0)');
        else if (v.id === 'antigravity')
            // Antigravity 2.0, the agy CLI and the IDE are separate products
            // sharing one quota; any of their state dirs is enough.
            lines.push('c=0; for d in antigravity antigravity-cli antigravity-ide; do ' +
                       '[ -d "$HOME/.gemini/$d" ] && c=1; done');
        else
            lines.push(`c=0; [ -n "\${${v.env}:-}" ] && c=1; [ "$c" = 0 ] && c=$(has_key ${v.id})`);
        lines.push(`echo "${v.id}:$c:$(${v.cli ? `cli ${v.cli}` : 'echo 0'})"`);
    }
    // Login shell: npm-global / nvm bins are often absent from plasmashell's PATH.
    return `bash -lc ${shellQuote(lines.join('; '))}`;
}

export function parseVendorProbe(stdout) {
    const out = {};
    for (const line of String(stdout ?? '').split('\n')) {
        const parts = line.trim().split(':');
        if (parts.length < 3)
            continue;
        out[parts[0]] = {configured: parts[1] === '1', cliInstalled: parts[2] === '1'};
    }
    return out;
}

// Status subtitle + button label per vendor. Every string here is copied from
// gnome-extension/prefs.js so both settings pages read identically.
export function vendorAuthRows(probe, table) {
    const p = probe ?? {};
    return (table ?? VENDOR_AUTH).map(v => {
        const known = Object.prototype.hasOwnProperty.call(p, v.id);
        const st = p[v.id] ?? {configured: false, cliInstalled: false};
        if (!known)
            return {id: v.id, name: v.name, kind: v.kind, subtitle: 'verificando…', button: '', enabled: false};

        if (v.kind === 'local') {
            const subtitle = st.configured
                ? '✓ Antigravity detectado — mantenha o app, IDE ou agy aberto'
                : st.cliInstalled
                    ? 'agy instalado — abra uma sessão para disponibilizar a quota'
                    : 'abra ou instale o app, a IDE ou o agy; não há login separado';
            return {id: v.id, name: v.name, kind: v.kind, subtitle,
                    button: st.cliInstalled ? 'Abrir agy' : 'Sem login separado',
                    enabled: st.cliInstalled};
        }

        if (v.kind !== 'oauth') {
            return {
                id: v.id, name: v.name, kind: v.kind,
                subtitle: st.configured ? '✓ Configurado' : `⚠ Sem API key — ${v.env}`,
                button: 'Configurar (TUI)', enabled: true,
            };
        }

        if (st.configured)
            return {id: v.id, name: v.name, kind: v.kind, subtitle: '✓ Configurado', button: 'Re-logar', enabled: true};

        return {
            id: v.id, name: v.name, kind: v.kind,
            subtitle: st.cliInstalled
                ? `⚠ Não logado — \`${v.login}\``
                : `⚠ ${v.cli} não instalado (instala em ~/.local, sem sudo)`,
            button: st.cliInstalled ? 'Logar' : 'Instalar + logar',
            enabled: true,
        };
    });
}

// Verbatim port of oauthCommand(): installs to ~/.local (already on PATH) so no
// sudo is needed even when the system npm prefix is root-owned.
export function oauthCommand(v) {
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

// What clicking a vendor's button runs, as a shell string for the executable
// engine. GNOME walks kgx/gnome-terminal/xterm; on KDE konsole comes first, and
// the user's configured terminal wins over both.
export function vendorActionCommand(v, terminalCommand, tuiPath) {
    if (v.kind === 'oauth')
        return terminalRun(terminalCommand, `bash -lc ${shellQuote(oauthCommand(v))}`);
    if (v.kind === 'local')
        return terminalRun(terminalCommand, shellQuote(v.cli));
    // Same fallback GNOME uses: PATH first, then ~/.cargo/bin. Resolved in the
    // shell because plasmashell's PATH is not the login shell's.
    const explicit = String(tuiPath ?? '').trim();
    if (explicit)
        return terminalRun(terminalCommand, shellQuote(explicit));
    return 'tui="$(command -v ai-usagebar-tui || echo "$HOME/.cargo/bin/ai-usagebar-tui")"; ' +
        terminalRun(terminalCommand, '"$tui"');
}

function terminalRun(terminalCommand, quotedProgram) {
    const custom = String(terminalCommand ?? '').trim();
    if (custom)
        return `${custom} ${quotedProgram}`;
    return [
        `if command -v konsole >/dev/null 2>&1; then exec konsole -e ${quotedProgram};`,
        `elif command -v x-terminal-emulator >/dev/null 2>&1; then exec x-terminal-emulator -e ${quotedProgram};`,
        `elif command -v xterm >/dev/null 2>&1; then exec xterm -e ${quotedProgram};`,
        `else exit 127; fi`,
    ].join(' ');
}

// `ai-usagebar usage --json` reports one entry per CONFIGURED entry, each with
// either a plan or an error — which is exactly what the config page needs to
// show why a vendor would fail before you put it in the scroll ring.
export function parseUsageReport(stdout) {
    const raw = String(stdout ?? '').trim();
    if (!raw)
        return {ok: false, entries: []};
    let data;
    try {
        data = JSON.parse(raw);
    } catch (e) {
        return {ok: false, entries: []};
    }
    if (!data || !data.entries || typeof data.entries.length !== 'number')
        return {ok: false, entries: []};
    return {ok: true, entries: Array.from(data.entries)};
}

// Merge what the binary reports with the known set, deduped, report order
// first. Named Anthropic accounts arrive as "anthropic@label" but --vendor
// takes the bare slug, so the account suffix is stripped and collapsed.
export function vendorChoices(entries, known) {
    const base = known ?? KNOWN_VENDORS;
    const seen = new Map();

    for (const entry of Array.from(entries ?? [])) {
        const id = String(entry?.id ?? '');
        const slug = id.split('@')[0].trim();
        if (!slug || seen.has(slug))
            continue;
        const error = entry?.error ? String(entry.error) : '';
        seen.set(slug, {
            id: slug,
            label: vendorLabel(slug),
            configured: true,
            status: error ? 'error' : 'ok',
            // One line: the config page is a form, not a log viewer.
            detail: error ? error.split('\n')[0] : String(entry?.plan ?? ''),
        });
    }

    for (const slug of base) {
        if (!seen.has(slug)) {
            seen.set(slug, {id: slug, label: vendorLabel(slug),
                            configured: false, status: 'unknown', detail: ''});
        }
    }

    return Array.from(seen.values());
}

// A KConfigXT StringList arrives in QML as an array-LIKE object, not a real
// Array: `typeof` is "object", `length` is right, the contents are right, but
// `Array.isArray()` returns false. Guarding on isArray therefore made every
// scroll a silent no-op in the panel while every Node test passed. Normalise
// instead — and exclude strings, which Array.from would happily split into
// individual characters.
function toRing(ring) {
    if (ring === null || ring === undefined || typeof ring === 'string')
        return [];
    const len = Number(ring.length);
    if (!Number.isFinite(len) || len <= 0)
        return [];
    return Array.from(ring);
}

// The scroll ring. Mirrors active::cycle_at in Rust, except the ring is this
// applet instance's own config rather than the global active_vendor file, so
// two panel instances never fight over one selection.
export function nextVendor(rawRing, current, delta) {
    const ring = toRing(rawRing);
    if (ring.length === 0)
        return String(current ?? '');
    const at = ring.indexOf(current);
    // A current vendor outside the ring (removed from config, say) has no
    // meaningful neighbour — restart at the top rather than guessing.
    if (at === -1)
        return ring[0];
    const n = ring.length;
    // JS % keeps the dividend's sign, so a negative delta needs the +n round
    // trip before the second modulo or ring[-1] comes back undefined.
    return ring[(((at + Number(delta) || 0) % n) + n) % n];
}

// Plasma's executable DataSource passes the string to KProcess::setShellCommand,
// which first tries KShell::splitArgs(AbortOnMeta). AbortOnMeta bails on any
// unquoted ; { } $ * ? etc. — and FORMAT is full of ';;' and '{}'. On abort it
// falls back to `/bin/sh -c "<whole string>"`, where ';;' is a syntax error.
// A single-quoted span is consumed literally with no metacharacter validation,
// so the split succeeds and the command is exec'd directly with no shell at
// all. Double quotes are NOT enough: they still reject unescaped $ and `.
export function shellQuote(value) {
    return `'${String(value ?? '').replace(/'/g, `'\\''`)}'`;
}

// --vendor is ALWAYS passed explicitly. That is what makes the plasmoid
// independent of ~/.cache/ai-usagebar/active_vendor: an explicit --vendor
// short-circuits Cli::resolve_vendor_with before active::read() is reached, so
// the shared file is never even read, let alone written. The test asserts this.
// Pixel geometry for the usage bar's two fill segments.
//
// barMarkup() composes the same shape out of block glyphs and Pango spans,
// which Qt cannot render, so the geometry is re-derived here rather than
// reused. The composition must stay identical: everything up to the pace marker
// carries the absolute-usage colour (colorForPct), and only the overshoot past
// it carries the pace colour (colorForDelta). Drawing the fill as a single
// rectangle in one or the other — which is what this replaced — repainted the
// WHOLE bar in the pace colour the moment usage edged past the clock, so 62%
// used at 40% elapsed showed a fully critical bar where GNOME and Waybar show
// a mostly-normal bar with a critical tail.
export function barSegments(pct, elapsed, width) {
    const w = Number(width);
    if (!Number.isFinite(w) || w <= 0)
        return {hasMarker: false, preWidth: 0, markerX: 0, postX: 0, postWidth: 0};
    const p = Math.max(0, Math.min(100, Number(pct) || 0));
    const fill = w * p / 100;
    // null is the "unknown elapsed" value markerElapsed() hands back, and
    // Number(null) is 0 — without this it would silently mean "0% elapsed" and
    // paint a pace marker hard against the left edge of every bar.
    const e = elapsed === null || elapsed === undefined ? NaN : Number(elapsed);
    if (!Number.isFinite(e) || e < 0 || e > 100)
        return {hasMarker: false, preWidth: fill, markerX: 0, postX: 0, postWidth: 0};
    const markerX = Math.min(w, w * e / 100);
    return {
        hasMarker: true,
        preWidth: Math.min(fill, markerX),
        markerX: markerX,
        postX: markerX,
        postWidth: Math.max(0, fill - markerX),
    };
}

// The executable data engine hands QML no handle on the child process, so the
// applet cannot kill a hung binary: disconnectSource only stops us listening,
// and the process keeps running — one more of them every tick. Wrapping the
// spawn in timeout(1) is what actually bounds it, and is the nearest equivalent
// to the GNOME extension's proc.force_exit(). -k escalates to SIGKILL for a
// binary that ignores SIGTERM. coreutils provides timeout on any system that
// can run Plasma, and the QML watchdog stays on as a backstop for the case
// where the engine itself never reports back.
export const TIMEOUT_KILL_GRACE_SECS = 5;
export const EXIT_TIMED_OUT = 124;  // timeout(1)'s own code, after SIGTERM
export const EXIT_KILLED = 137;     // 128 + SIGKILL, after -k escalated

export function buildArgv(binary, vendor, format, timeoutSecs) {
    const bin = String(binary ?? '').trim() || DEFAULT_BINARY;
    const ven = String(vendor ?? '').trim() || DEFAULT_VENDOR;
    const call = [bin, '--vendor', ven, '--format', String(format ?? '')];
    const secs = Number(timeoutSecs);
    if (!Number.isFinite(secs) || secs <= 0)
        return call;
    return ['timeout', '-k', String(TIMEOUT_KILL_GRACE_SECS), String(Math.round(secs))]
        .concat(call);
}

export function buildCommand(binary, vendor, format, timeoutSecs) {
    return buildArgv(binary, vendor, format, timeoutSecs).map(shellQuote).join(' ');
}

// Launching the TUI needs a terminal, and there is no portable way to probe
// PATH from QML. Emit a shell fallback chain instead and let KProcess take its
// /bin/sh -c path — here the metacharacters are the point, unlike buildCommand
// where they had to be quoted away. Mirrors the candidate list the GNOME
// extension already walks.
export function buildTuiCommand(terminalCommand, tui = 'ai-usagebar-tui') {
    const custom = String(terminalCommand ?? '').trim();
    if (custom)
        return `${custom} ${shellQuote(tui)}`;
    const q = shellQuote(tui);
    return [
        `if command -v konsole >/dev/null 2>&1; then exec konsole -e ${q};`,
        `elif command -v x-terminal-emulator >/dev/null 2>&1; then exec x-terminal-emulator -e ${q};`,
        `elif command -v xdg-terminal-exec >/dev/null 2>&1; then exec xdg-terminal-exec ${q};`,
        `else exit 127; fi`,
    ].join(' ');
}

// The widget always exits 0 and always prints one JSON object, including for
// its own ⚠ and Loading… states — so invalid JSON here means the binary is
// missing or something else wrote to stdout, not that usage lookup failed.
export function parseWidgetJson(stdout) {
    const raw = String(stdout ?? '').trim();
    if (!raw)
        return {ok: false, error: 'no output', raw};
    let data;
    try {
        data = JSON.parse(raw);
    } catch (e) {
        // The binding is required: QML's V4 engine rejects the ES2019 optional
        // catch binding (`catch {`) with a bare syntax error, even though Node
        // accepts it. Anything in this file has to parse under BOTH engines.
        return {ok: false, error: 'invalid JSON', raw};
    }
    if (!data || typeof data !== 'object' || Array.isArray(data))
        return {ok: false, error: 'invalid JSON', raw};
    return {
        ok: true,
        text: plainTextFromPango(data.text),
        tooltip: plainTextFromPango(data.tooltip),
        cls: typeof data.class === 'string' ? data.class : '',
    };
}

// Splits the --format payload into the view model. A short field list is the
// binary's own ⚠ / Loading… text rather than a usage payload: those states are
// plain strings, not ';;'-delimited, so they must be shown verbatim instead of
// parsed into confident zeroes.
export function parseFields(plainText) {
    const f = splitFormatOutput(plainText);
    if (f.length <= FIELD.extraLimit)
        return {kind: 'message', text: field(plainText) || plainText.trim()};
    return {kind: 'data', data: toRows(f)};
}

// Faithful port of the model gnome-extension/extension.js builds inline in
// _consume(). Kept as a pure function so it is actually testable — the GNOME
// copy has no coverage at all.
export function toRows(f) {
    const win = (pctIdx, resetIdx, elapsedIdx, modelIdx) => {
        const reset = field(f[resetIdx]);
        return {
            pct: integer(f[pctIdx]),
            reset,
            model: modelIdx === undefined ? '' : field(f[modelIdx]),
            elapsed: markerElapsed(reset, integer(f[elapsedIdx])),
        };
    };

    return {
        plan: field(f[FIELD.plan]),
        hasUsageWindows: hasUsageWindows(f[FIELD.vendorShort]),
        grouped: isGrouped(f[FIELD.sessionModel]),
        vendorShort: field(f[FIELD.vendorShort]),
        session: win(FIELD.sessionPct, FIELD.sessionReset, FIELD.sessionElapsed, FIELD.sessionModel),
        weekly: win(FIELD.weeklyPct, FIELD.weeklyReset, FIELD.weeklyElapsed, FIELD.weeklyModel),
        sonnet: scopedWindow(f),
        // A named extra window (model + reset) renders as a percentage bar;
        // without a name the slot stays a spent/limit money budget.
        extra: {
            pct: integer(f[FIELD.extraPct]),
            spent: field(f[FIELD.extraSpent]),
            limit: field(f[FIELD.extraLimit]),
            model: field(f[FIELD.extraModel]),
            reset: field(f[FIELD.extraReset]),
            elapsed: markerElapsed(field(f[FIELD.extraReset]), integer(f[FIELD.extraElapsed])),
        },
    };
}

// A non-empty scoped model is the presence signal for the per-model weekly cap.
// Its reset may be unavailable, which must not make us fall back to the
// unrelated legacy Sonnet window.
function scopedWindow(f) {
    const scopedModel = field(f[FIELD.scopedModel]);
    if (!scopedModel) {
        return {
            pct: integer(f[FIELD.sonnetPct]), reset: field(f[FIELD.sonnetReset]),
            model: '', label: 'Sonnet only', elapsed: null,
        };
    }
    const scopedPct = integer(f[FIELD.scopedPct]);
    if (scopedPct != null && scopedPct >= 0 && scopedPct <= 100) {
        const reset = field(f[FIELD.scopedReset]);
        return {
            pct: scopedPct, reset: reset || '—', model: scopedModel, label: scopedModel,
            elapsed: markerElapsed(reset, integer(f[FIELD.scopedElapsed])),
        };
    }
    // Malformed scoped data means unavailable, not "use the Sonnet window".
    return {pct: null, reset: '—', model: scopedModel, label: scopedModel, elapsed: null};
}

// QML wants a plain int for the pace marker; -1 means "unknown".
function elapsedOr(value) {
    return Number.isFinite(value) ? value : -1;
}

// The panel cells, in display order. Mirrors _renderPanel/_selectedPools in
// gnome-extension/extension.js: a grouped vendor's SECOND pool lives in the
// scoped and extra slots, which is why `secondary` reads from sonnet/extra
// rather than from fields of its own.
//
// A cell with `text` set renders that string instead of "N%" (the money budget
// shows "$3.00" but still takes its colour from the percentage).
export function compactCells(data, opts) {
    const o = opts ?? {};
    const showSession = o.showSession !== false;
    const showWeekly = o.showWeekly !== false;
    const mode = o.poolMode || 'all';
    const threshold = o.threshold ?? 80;
    const cells = [];
    if (!data)
        return cells;

    if (data.grouped) {
        // Either secondary window may be absent, so derive its tag from
        // whichever model-bearing slot exists.
        const secondaryModel = data.sonnet.model || data.extra.model;
        const [primaryTag, secondaryTag] = disambiguateTags(data.session.model, secondaryModel);
        const primary = {tag: primaryTag, session: data.session, weekly: data.weekly};
        const secondary = {tag: secondaryTag, session: data.sonnet, weekly: data.extra};
        const pcts = pool => ({session: pool.session.pct, weekly: pool.weekly.pct});
        const chosen = selectPools(pcts(primary), pcts(secondary), mode, threshold,
                                   {session: showSession, weekly: showWeekly});
        for (const name of chosen) {
            const pool = name === 'primary' ? primary : secondary;
            if (showSession && pool.session.pct != null) {
                cells.push({tag: `${pool.tag} 5h`, pct: pool.session.pct,
                            elapsed: elapsedOr(pool.session.elapsed)});
            }
            if (showWeekly && pool.weekly.pct != null) {
                cells.push({tag: `${pool.tag} 7d`, pct: pool.weekly.pct,
                            elapsed: elapsedOr(pool.weekly.elapsed)});
            }
        }
        return cells;
    }

    if (data.hasUsageWindows && showSession && data.session.pct != null)
        cells.push({tag: '5h', pct: data.session.pct, elapsed: elapsedOr(data.session.elapsed)});
    if (data.hasUsageWindows && showWeekly && data.weekly.pct != null)
        cells.push({tag: '7d', pct: data.weekly.pct, elapsed: elapsedOr(data.weekly.elapsed)});
    // A money budget: coloured by percentage, but labelled with the amount.
    // Off by default, matching show-extra in GNOME and macOS.
    if (o.showExtra === true && data.extra.pct != null && data.extra.spent && data.extra.limit)
        cells.push({tag: 'ex', pct: data.extra.pct, text: data.extra.spent, elapsed: -1});
    return cells;
}

// The popup / tooltip rows, in display order. A window with no percentage is
// omitted entirely rather than drawn as 0% — the whole point of
// hasUsageWindows and of the scoped "unavailable" state.
//
// Labels and ordering follow the GNOME dropdown so the two panels read the
// same. For a grouped vendor that includes the Session/Weekly headings: with
// two independent pools, four flat rows give no clue which window belongs to
// which pool. Rows carry `kind` so the view can render a heading.
export function detailRows(data) {
    if (!data)
        return [];
    const rows = [];
    const windowRow = (w, fallbackLabel) => {
        if (!w || w.pct == null)
            return null;
        return {
            kind: 'row',
            label: w.model || w.label || fallbackLabel,
            pct: w.pct,
            valueText: '',
            reset: w.reset || '',
            elapsed: elapsedOr(w.elapsed),
        };
    };
    const push = row => {
        if (row)
            rows.push(row);
    };
    const heading = label => ({kind: 'heading', label, pct: 0, valueText: '', reset: '', elapsed: -1});

    // Two independent pools: the scoped and extra slots hold the SECOND pool's
    // 5h and weekly windows, so group by window rather than by pool.
    //
    // A heading is only emitted when its group actually has a row. Either pool
    // can be absent or partial — the maintainer had to fix exactly that class of
    // bug for the GNOME panel — and a heading with nothing under it is the popup
    // equivalent of the blank panel.
    if (data.grouped) {
        for (const group of [
            {label: 'Session', windows: [data.session, data.sonnet]},
            {label: 'Weekly', windows: [data.weekly, data.extra]},
        ]) {
            const groupRows = group.windows
                .map(w => windowRow(w, group.label))
                .filter(Boolean);
            if (groupRows.length === 0)
                continue;
            push(heading(group.label));
            for (const row of groupRows)
                push(row);
        }
        return rows;
    }

    if (data.hasUsageWindows) {
        push(windowRow(data.session, 'Session'));
        push(windowRow(data.weekly, 'Weekly'));
    }
    push(windowRow(data.sonnet, 'Sonnet only'));

    // A named extra window is a percentage bar; an unnamed one is a money
    // budget, which keeps the percentage for colour but shows spent / limit.
    if (data.extra.model && data.extra.pct != null) {
        rows.push({
            kind: 'row', label: data.extra.model, pct: data.extra.pct, valueText: '',
            reset: data.extra.reset || '', elapsed: elapsedOr(data.extra.elapsed),
        });
    } else if (data.extra.pct != null && data.extra.spent && data.extra.limit) {
        rows.push({
            kind: 'row', label: 'Extra usage', pct: data.extra.pct,
            valueText: `${data.extra.spent} / ${data.extra.limit}`,
            reset: data.extra.reset || '', elapsed: -1,
        });
    }
    return rows;
}

// Which parts of a panel cell are drawn. Kept here rather than as a QML
// binding so the tests exercise the real rule instead of a parallel copy of it.
// The last clause matters: with both toggles off a segment would render empty,
// so the value wins — the same fallback as seg() in the GNOME extension and the
// macOS app.
export function cellParts(showPercent, showBars) {
    const pct = showPercent !== false;
    const bars = showBars !== false;
    if (!pct && !bars)
        return {value: true, bar: false};
    return {value: pct, bar: bars};
}

// Kirigami.Theme -> the {low, mid, high, critical, empty} palette that
// marker-logic's colorForPct/colorForDelta already expect. Only the *destination*
// of each severity changes here; the thresholds stay in marker-logic so the
// Plasma panel and the GNOME panel agree on when a number is a problem.
//
// low and mid deliberately share textColor: on a panel, staying quiet until
// something actually needs attention reads better than painting healthy quota
// green. The Kirigami roles are notifying properties, so as long as QML binds
// (never assigns imperatively) these recolor on a theme switch by themselves.
export function paletteFromTheme(theme) {
    const t = theme ?? {};
    return {
        low: t.textColor,
        mid: t.textColor,
        high: t.neutralTextColor,
        critical: t.negativeTextColor,
        empty: t.disabledTextColor,
    };
}
