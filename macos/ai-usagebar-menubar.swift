// ai-usagebar-menubar — macOS menu bar app for ai-usagebar.
//
// Shows ai-usagebar's 5-hour (session), weekly, and optional extra-usage
// bars in the macOS menu bar, next to the clock, with a native dropdown and
// a Preferences window (⌘,). Mirrors the GNOME Shell extension: same binary,
// same One Dark colors and severity thresholds. Runs as a menu-bar agent.
//
// Settings persist in UserDefaults (edit them in Preferences, no rebuild).
//
// Build:  swiftc -O ai-usagebar-menubar.swift -o ai-usagebar-menubar
//         (needs the Xcode command-line tools: `xcode-select --install`)
// Run:    ./ai-usagebar-menubar &      (or ./install-agent.sh for login start)
// macOS:  12+ (Monterey) for the Preferences window; menu bar works on 10.15+.
//
// First, on the Mac: run `claude` once so the OAuth creds land in the login
// Keychain — ai-usagebar reads them there (src/anthropic/keychain.rs).

import Cocoa
import SwiftUI

// ─── Settings (persisted in UserDefaults; edit in Preferences) ───────────
let DEF = UserDefaults.standard

let SETTINGS_DEFAULTS: [String: Any] = [
    "vendor": "anthropic",
    "interval": 30.0,
    "barWidth": 8,
    "showSession": true,
    "showWeekly": true,
    "showExtra": false,
    "showPercent": true,
    "showBars": true,
    "colorLow": "#98c379",
    "colorMid": "#e5c07b",
    "colorHigh": "#d19a66",
    "colorCritical": "#e06c75",
    "colorEmpty": "#3e4451",
    "binaryPath": "",
]

var VENDOR: String { DEF.string(forKey: "vendor") ?? "anthropic" }
var INTERVAL: Double { let v = DEF.double(forKey: "interval"); return v > 0 ? v : 30 }
var BAR_WIDTH: Int { max(4, min(20, DEF.integer(forKey: "barWidth"))) }
let MENU_BAR_W = 14
var SHOW_SESSION: Bool { DEF.bool(forKey: "showSession") }
var SHOW_WEEKLY: Bool { DEF.bool(forKey: "showWeekly") }
var SHOW_EXTRA: Bool { DEF.bool(forKey: "showExtra") }
var SHOW_PERCENT: Bool { DEF.bool(forKey: "showPercent") }
var SHOW_BARS: Bool { DEF.bool(forKey: "showBars") }
var COLOR_LOW: String { DEF.string(forKey: "colorLow") ?? "#98c379" }
var COLOR_MID: String { DEF.string(forKey: "colorMid") ?? "#e5c07b" }
var COLOR_HIGH: String { DEF.string(forKey: "colorHigh") ?? "#d19a66" }
var COLOR_CRITICAL: String { DEF.string(forKey: "colorCritical") ?? "#e06c75" }
var COLOR_EMPTY: String { DEF.string(forKey: "colorEmpty") ?? "#3e4451" }

let FORMAT = "{plan};;{session_pct};;{session_reset};;{weekly_pct};;{weekly_reset};;" +
             "{sonnet_pct};;{sonnet_reset};;{extra_pct};;{extra_spent};;{extra_limit}"

// ─── Color / text helpers ────────────────────────────────────────────────
func hexColor(_ hex: String) -> NSColor {
    var s = hex
    if s.hasPrefix("#") { s.removeFirst() }
    guard s.count == 6, let v = UInt32(s, radix: 16) else { return .labelColor }
    return NSColor(srgbRed: CGFloat((v >> 16) & 0xff) / 255.0,
                   green: CGFloat((v >> 8) & 0xff) / 255.0,
                   blue: CGFloat(v & 0xff) / 255.0,
                   alpha: 1.0)
}

func colorForPct(_ pct: Int) -> NSColor {
    if pct >= 90 { return hexColor(COLOR_CRITICAL) }
    if pct >= 75 { return hexColor(COLOR_HIGH) }
    if pct >= 50 { return hexColor(COLOR_MID) }
    return hexColor(COLOR_LOW)
}

let barFont = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)

func run(_ s: String, _ color: NSColor, _ font: NSFont = barFont) -> NSAttributedString {
    NSAttributedString(string: s, attributes: [.foregroundColor: color, .font: font])
}

func barAttr(pct: Int, width: Int) -> NSAttributedString {
    let p = max(0, min(100, pct))
    let filled = Int((Double(p) * Double(width) / 100.0).rounded())
    let out = NSMutableAttributedString()
    out.append(run(String(repeating: "█", count: filled), colorForPct(p)))
    out.append(run(String(repeating: "░", count: max(0, width - filled)), hexColor(COLOR_EMPTY)))
    return out
}

func resolveBinary(_ name: String) -> String? {
    let fm = FileManager.default
    if name == "ai-usagebar" {
        let configured = DEF.string(forKey: "binaryPath") ?? ""
        if !configured.isEmpty, fm.isExecutableFile(atPath: configured) { return configured }
    }
    let home = NSHomeDirectory()
    for c in ["\(home)/.cargo/bin/\(name)", "/opt/homebrew/bin/\(name)", "/usr/local/bin/\(name)"]
    where fm.isExecutableFile(atPath: c) {
        return c
    }
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/which")
    p.arguments = [name]
    let pipe = Pipe()
    p.standardOutput = pipe
    p.standardError = FileHandle.nullDevice
    do {
        try p.run()
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        let path = String(data: data, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !path.isEmpty && fm.isExecutableFile(atPath: path) { return path }
    } catch {}
    return nil
}

// ─── Data model ──────────────────────────────────────────────────────────
struct Window { let pct: Int; let reset: String }
struct Snapshot {
    let plan: String
    let session: Window
    let weekly: Window
    let sonnet: Window?
    let extra: (pct: Int, spent: String, limit: String)?
}

func stripMarkup(_ s: String) -> String {
    s.replacingOccurrences(of: "<[^>]*>", with: "", options: .regularExpression)
}

func parse(_ text: String) -> Snapshot? {
    let f = stripMarkup(text).components(separatedBy: ";;")
    guard f.count >= 10 else { return nil }
    func t(_ i: Int) -> String { f[i].trimmingCharacters(in: .whitespaces) }
    func n(_ i: Int) -> Int? { Int(t(i)) }
    let sonnet = n(5).map { Window(pct: $0, reset: t(6)) }
    let spent = t(8)
    let extra: (pct: Int, spent: String, limit: String)? =
        spent.isEmpty ? nil : (n(7) ?? 0, spent, t(9))
    return Snapshot(plan: t(0),
                    session: Window(pct: n(1) ?? 0, reset: t(2)),
                    weekly: Window(pct: n(3) ?? 0, reset: t(4)),
                    sonnet: sonnet,
                    extra: extra)
}

// ─── Preferences UI (SwiftUI) ────────────────────────────────────────────
extension Color {
    init(hexString: String) { self.init(nsColor: hexColor(hexString)) }
    var hexString: String {
        let ns = NSColor(self).usingColorSpace(.sRGB) ?? .black
        return String(format: "#%02x%02x%02x",
                      Int((ns.redComponent * 255).rounded()),
                      Int((ns.greenComponent * 255).rounded()),
                      Int((ns.blueComponent * 255).rounded()))
    }
}

struct HexColorPicker: View {
    let title: String
    @Binding var hex: String
    var body: some View {
        ColorPicker(title, selection: Binding(
            get: { Color(hexString: hex) },
            set: { hex = $0.hexString }
        ), supportsOpacity: false)
    }
}

// ─── Vendor login / config (mirrors the GNOME "Vendors" tab) ──────────────
struct VendorAuth {
    let id, name, kind, cli, login, pkg, env: String
}

let VENDOR_AUTH: [VendorAuth] = [
    VendorAuth(id: "anthropic", name: "Anthropic (Claude)", kind: "oauth", cli: "claude", login: "claude", pkg: "@anthropic-ai/claude-code", env: ""),
    VendorAuth(id: "openai", name: "OpenAI (Codex)", kind: "oauth", cli: "codex", login: "codex login", pkg: "@openai/codex", env: ""),
    VendorAuth(id: "zai", name: "Z.AI (GLM)", kind: "apikey", cli: "", login: "", pkg: "", env: "ZAI_API_KEY"),
    VendorAuth(id: "openrouter", name: "OpenRouter", kind: "apikey", cli: "", login: "", pkg: "", env: "OPENROUTER_API_KEY"),
    VendorAuth(id: "deepseek", name: "DeepSeek", kind: "apikey", cli: "", login: "", pkg: "", env: "DEEPSEEK_API_KEY"),
]

func configHasApiKeyTOML(_ section: String) -> Bool {
    let path = "\(NSHomeDirectory())/.config/ai-usagebar/config.toml"
    guard let text = try? String(contentsOfFile: path, encoding: .utf8) else { return false }
    var inSection = false
    for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
        let line = String(raw).trimmingCharacters(in: .whitespaces)
        if line.hasPrefix("[") {
            inSection = (line == "[\(section)]")
        } else if inSection && !line.hasPrefix("#") &&
                  line.range(of: "^api_key\\s*=\\s*[\"']\\S", options: .regularExpression) != nil {
            return true
        }
    }
    return false
}

func keychainHasClaude() -> Bool {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/security")
    p.arguments = ["find-generic-password", "-s", "Claude Code-credentials"]
    p.standardOutput = FileHandle.nullDevice
    p.standardError = FileHandle.nullDevice
    do { try p.run(); p.waitUntilExit(); return p.terminationStatus == 0 } catch { return false }
}

func vendorConfigured(_ v: VendorAuth) -> Bool {
    let home = NSHomeDirectory()
    let fm = FileManager.default
    if v.id == "anthropic" {
        return fm.fileExists(atPath: "\(home)/.claude/.credentials.json") || keychainHasClaude()
    }
    if v.id == "openai" {
        return fm.fileExists(atPath: "\(home)/.codex/auth.json")
    }
    if let e = ProcessInfo.processInfo.environment[v.env], !e.isEmpty { return true }
    return configHasApiKeyTOML(v.id)
}

func cliInstalled(_ cli: String) -> Bool {
    let home = NSHomeDirectory()
    let fm = FileManager.default
    for dir in ["\(home)/.local/bin", "/opt/homebrew/bin", "/usr/local/bin", "\(home)/.cargo/bin"]
    where fm.isExecutableFile(atPath: "\(dir)/\(cli)") {
        return true
    }
    // Fall back to a login shell (covers nvm etc.).
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/bin/bash")
    p.arguments = ["-lc", "command -v \(cli)"]
    let pipe = Pipe()
    p.standardOutput = pipe
    p.standardError = FileHandle.nullDevice
    do {
        try p.run()
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        p.waitUntilExit()
        return !(String(data: data, encoding: .utf8) ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    } catch { return false }
}

// Write a script to a temp file and run it in Terminal.app (no AppleScript quoting hell).
func runInTerminal(_ script: String) {
    let tmp = NSTemporaryDirectory() + "ai-usagebar-vendor.sh"
    try? script.write(toFile: tmp, atomically: true, encoding: .utf8)
    try? FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: tmp)
    let osa = "tell application \"Terminal\" to do script \"bash '\(tmp)'; rm -f '\(tmp)'\"\n" +
        "tell application \"Terminal\" to activate"
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
    p.arguments = ["-e", osa]
    try? p.run()
}

func oauthScript(_ v: VendorAuth) -> String {
    return """
    export PATH="$HOME/.local/bin:$PATH"
    if command -v \(v.cli) >/dev/null 2>&1; then
      \(v.login)
    else
      echo "\(v.cli) nao encontrado. Instalo em ~/.local sem sudo. Pacote: \(v.pkg)"
      read -p "Instalar agora? [y/N] " a
      if [ "$a" = y ] || [ "$a" = Y ]; then npm i -g --prefix "$HOME/.local" \(v.pkg) && hash -r && \(v.login); fi
    fi
    echo
    read -p "Enter para fechar..."
    """
}

func openTuiInTerminal() {
    let cargo = "\(NSHomeDirectory())/.cargo/bin/ai-usagebar-tui"
    let tui = FileManager.default.isExecutableFile(atPath: cargo) ? cargo : "ai-usagebar-tui"
    runInTerminal("\"\(tui)\"\necho\nread -p \"Enter para fechar...\"")
}

struct VendorsSection: View {
    @State private var configured: [String: Bool] = [:]
    @State private var cliPresent: [String: Bool] = [:]
    @State private var checking = false

    var body: some View {
        Section("Vendors") {
            ForEach(VENDOR_AUTH, id: \.id) { v in
                HStack(alignment: .firstTextBaseline) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(v.name)
                        Text(statusText(v)).font(.caption).foregroundColor(.secondary)
                    }
                    Spacer()
                    Button(buttonLabel(v)) { action(v) }
                }
            }
            if checking {
                Text("verificando…").font(.caption).foregroundColor(.secondary)
            }
        }
        .onAppear(perform: refresh)
    }

    private func refresh() {
        checking = true
        DispatchQueue.global(qos: .userInitiated).async {
            var conf: [String: Bool] = [:]
            var cli: [String: Bool] = [:]
            for v in VENDOR_AUTH {
                conf[v.id] = vendorConfigured(v)
                if v.kind == "oauth" { cli[v.id] = cliInstalled(v.cli) }
            }
            DispatchQueue.main.async {
                self.configured = conf
                self.cliPresent = cli
                self.checking = false
            }
        }
    }

    private func statusText(_ v: VendorAuth) -> String {
        if configured[v.id] == true { return "✓ Configurado" }
        if v.kind == "oauth" {
            if cliPresent[v.id] == false { return "⚠ \(v.cli) não instalado" }
            return "⚠ Não logado — \(v.login)"
        }
        return "⚠ Sem API key — \(v.env)"
    }

    private func buttonLabel(_ v: VendorAuth) -> String {
        if v.kind == "oauth" {
            if configured[v.id] == true { return "Re-logar" }
            if cliPresent[v.id] == false { return "Instalar + logar" }
            return "Logar"
        }
        return "Configurar (TUI)"
    }

    private func action(_ v: VendorAuth) {
        if v.kind == "oauth" { runInTerminal(oauthScript(v)) }
        else { openTuiInTerminal() }
        DispatchQueue.main.asyncAfter(deadline: .now() + 4) { refresh() }
    }
}

struct SettingsView: View {
    @AppStorage("vendor") private var vendor = "anthropic"
    @AppStorage("interval") private var interval = 30.0
    @AppStorage("barWidth") private var barWidth = 8
    @AppStorage("showSession") private var showSession = true
    @AppStorage("showWeekly") private var showWeekly = true
    @AppStorage("showExtra") private var showExtra = false
    @AppStorage("showPercent") private var showPercent = true
    @AppStorage("showBars") private var showBars = true
    @AppStorage("colorLow") private var colorLow = "#98c379"
    @AppStorage("colorMid") private var colorMid = "#e5c07b"
    @AppStorage("colorHigh") private var colorHigh = "#d19a66"
    @AppStorage("colorCritical") private var colorCritical = "#e06c75"
    @AppStorage("colorEmpty") private var colorEmpty = "#3e4451"
    @AppStorage("binaryPath") private var binaryPath = ""

    private let vendors = ["anthropic", "openai", "zai", "openrouter", "deepseek"]

    var body: some View {
        Form {
            Section("Exibição") {
                Toggle("Mostrar barra de 5h (sessão)", isOn: $showSession)
                Toggle("Mostrar barra semanal", isOn: $showWeekly)
                Toggle("Mostrar barra de uso extra ($)", isOn: $showExtra)
                Toggle("Mostrar porcentagem/valor", isOn: $showPercent)
                Toggle("Mostrar barras (off = só números)", isOn: $showBars)
                Stepper("Largura da barra: \(barWidth)", value: $barWidth, in: 4...20)
            }
            Section("Cores") {
                HexColorPicker(title: "Baixo (<50%)", hex: $colorLow)
                HexColorPicker(title: "Médio (50–74%)", hex: $colorMid)
                HexColorPicker(title: "Alto (75–89%)", hex: $colorHigh)
                HexColorPicker(title: "Crítico (≥90%)", hex: $colorCritical)
                HexColorPicker(title: "Vazio (fundo da barra)", hex: $colorEmpty)
            }
            Section("Dados") {
                Picker("Vendor", selection: $vendor) {
                    ForEach(vendors, id: \.self) { Text($0) }
                }
                Stepper("Intervalo: \(Int(interval))s", value: $interval, in: 5...3600, step: 5)
                TextField("Caminho do binário (vazio = auto)", text: $binaryPath)
            }
            VendorsSection()
        }
        .padding(20)
        .frame(width: 460, height: 560)
    }
}

// ─── App ─────────────────────────────────────────────────────────────────
class AppDelegate: NSObject, NSApplicationDelegate {
    var statusItem: NSStatusItem!
    var timer: Timer?
    var prefsWindow: NSWindow?
    var lastSnapshot: Snapshot?
    var pendingRefresh: DispatchWorkItem?
    let binary = resolveBinary("ai-usagebar")
    let headerItem = NSMenuItem()
    var rows: [String: NSMenuItem] = [:]

    func applicationDidFinishLaunching(_ notification: Notification) {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        statusItem.button?.title = "5h …"
        buildMenu()
        refresh()
        restartTimer()
        NotificationCenter.default.addObserver(
            self, selector: #selector(settingsChanged),
            name: UserDefaults.didChangeNotification, object: nil)
    }

    func buildMenu() {
        let menu = NSMenu()
        menu.autoenablesItems = false

        menu.addItem(headerItem)
        for key in ["session", "weekly", "sonnet", "extra"] {
            let it = NSMenuItem()
            rows[key] = it
            menu.addItem(it)
        }

        menu.addItem(.separator())
        addAction(menu, "Atualizar agora", #selector(refreshAction), "r")
        addAction(menu, "Abrir TUI", #selector(openTui), "t")
        addAction(menu, "Preferências…", #selector(openPrefs), ",")
        menu.addItem(.separator())
        addAction(menu, "Sair", #selector(quit), "q")

        statusItem.menu = menu
    }

    func addAction(_ menu: NSMenu, _ title: String, _ sel: Selector, _ key: String) {
        let it = NSMenuItem(title: title, action: sel, keyEquivalent: key)
        it.target = self
        menu.addItem(it)
    }

    @objc func refreshAction() { refresh() }
    @objc func quit() { NSApp.terminate(nil) }

    @objc func openPrefs() {
        if prefsWindow == nil {
            let host = NSHostingController(rootView: SettingsView())
            let w = NSWindow(contentViewController: host)
            w.title = "AI Usage Bar — Preferências"
            w.styleMask = [.titled, .closable]
            w.isReleasedWhenClosed = false
            w.center()
            prefsWindow = w
        }
        NSApp.activate(ignoringOtherApps: true)
        prefsWindow?.makeKeyAndOrderFront(nil)
    }

    @objc func openTui() {
        guard let tui = resolveBinary("ai-usagebar-tui") else { return }
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        p.arguments = ["-e", "tell application \"Terminal\" to do script \"\(tui)\""]
        try? p.run()
    }

    // Settings changed in Preferences: re-render instantly from cache, re-arm
    // the timer, and re-fetch (debounced) in case vendor/binary changed.
    @objc func settingsChanged() {
        if let s = lastSnapshot { renderPanel(s); renderMenu(s) }
        restartTimer()
        pendingRefresh?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.refresh() }
        pendingRefresh = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6, execute: work)
    }

    func restartTimer() {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: INTERVAL, repeats: true) { [weak self] _ in
            self?.refresh()
        }
    }

    func refresh() {
        guard let bin = binary else {
            setError("ai-usagebar não encontrado (PATH / ~/.cargo/bin / homebrew)")
            return
        }
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let p = Process()
            p.executableURL = URL(fileURLWithPath: bin)
            p.arguments = ["--vendor", VENDOR, "--format", FORMAT]
            let pipe = Pipe()
            p.standardOutput = pipe
            p.standardError = FileHandle.nullDevice
            var out = ""
            do {
                try p.run()
                let data = pipe.fileHandleForReading.readDataToEndOfFile()  // read before wait
                p.waitUntilExit()
                out = String(data: data, encoding: .utf8) ?? ""
            } catch {
                DispatchQueue.main.async { self?.setError("falha ao executar ai-usagebar") }
                return
            }
            DispatchQueue.main.async { self?.consume(out) }
        }
    }

    func consume(_ output: String) {
        guard let data = output.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let text = obj["text"] as? String else {
            setError("saída inválida")
            return
        }
        guard let snap = parse(text) else {
            lastSnapshot = nil
            statusItem.button?.attributedTitle = run(stripMarkup(text), .labelColor)  // Loading… / ⚠
            return
        }
        lastSnapshot = snap
        renderPanel(snap)
        renderMenu(snap)
    }

    func renderPanel(_ s: Snapshot) {
        let title = NSMutableAttributedString()
        func seg(_ tag: String, _ pct: Int, _ value: String) {
            if title.length > 0 { title.append(run("   ", .secondaryLabelColor)) }
            title.append(run("\(tag) ", .secondaryLabelColor))
            if SHOW_PERCENT { title.append(run(value + (SHOW_BARS ? " " : ""), colorForPct(pct))) }
            if SHOW_BARS { title.append(barAttr(pct: pct, width: BAR_WIDTH)) }
            if !SHOW_PERCENT && !SHOW_BARS { title.append(run(value, colorForPct(pct))) }
        }
        if SHOW_SESSION { seg("5h", s.session.pct, "\(s.session.pct)%") }
        if SHOW_WEEKLY { seg("7d", s.weekly.pct, "\(s.weekly.pct)%") }
        if SHOW_EXTRA, let e = s.extra { seg("ex", e.pct, e.spent) }
        statusItem.button?.attributedTitle = title.length > 0 ? title : run("ai", .secondaryLabelColor)
    }

    func renderMenu(_ s: Snapshot) {
        headerItem.attributedTitle = run(s.plan.isEmpty ? "AI Usage" : s.plan,
                                         .labelColor, NSFont.boldSystemFont(ofSize: 13))

        func row(_ key: String, _ name: String, _ pct: Int, _ value: String, _ reset: String?) {
            guard let item = rows[key] else { return }
            item.isHidden = false
            let a = NSMutableAttributedString()
            a.append(run(name.padding(toLength: 12, withPad: " ", startingAt: 0), .labelColor))
            a.append(barAttr(pct: pct, width: MENU_BAR_W))
            a.append(run("  \(value)", colorForPct(pct)))
            if let r = reset, !r.isEmpty { a.append(run("   ↺ \(r)", .secondaryLabelColor)) }
            item.attributedTitle = a
        }
        row("session", "Session", s.session.pct, "\(s.session.pct)%", s.session.reset)
        row("weekly", "Weekly", s.weekly.pct, "\(s.weekly.pct)%", s.weekly.reset)
        if let sn = s.sonnet { row("sonnet", "Sonnet only", sn.pct, "\(sn.pct)%", sn.reset) }
        else { rows["sonnet"]?.isHidden = true }
        if let e = s.extra { row("extra", "Extra usage", e.pct, "\(e.spent) / \(e.limit)", nil) }
        else { rows["extra"]?.isHidden = true }
    }

    func setError(_ msg: String) {
        lastSnapshot = nil
        statusItem.button?.attributedTitle = run("⚠ ai", hexColor(COLOR_CRITICAL))
        headerItem.attributedTitle = run(msg, .labelColor)
        for (_, it) in rows { it.isHidden = true }
    }
}

DEF.register(defaults: SETTINGS_DEFAULTS)
let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.accessory)   // menu-bar agent, no Dock icon
app.run()
