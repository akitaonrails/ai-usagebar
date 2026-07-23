// ai-usagebar-menubar — macOS menu bar app for ai-usagebar.
//
// Shows ai-usagebar's 5-hour (session), weekly, and optional extra-usage
// bars in the macOS menu bar, next to the clock, with a native dropdown and
// a Preferences window (⌘,). Mirrors the GNOME Shell extension: same binary,
// same One Dark colors and severity thresholds. Runs as a menu-bar agent.
//
// Settings persist in UserDefaults (edit them in Preferences, no rebuild).
//
// Build:  swiftc -O -parse-as-library ai-usagebar-menubar.swift -o ai-usagebar-menubar
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
    "showMeta": true,
    "barStyle": "block",
    "colorLow": "#98c379",
    "colorMid": "#e5c07b",
    "colorHigh": "#d19a66",
    "colorCritical": "#e06c75",
    "colorEmpty": "#3e4451",
    "binaryPath": "",
]

var VENDOR: String { DEF.string(forKey: "vendor") ?? "anthropic" }
var INTERVAL: Double { let v = DEF.double(forKey: "interval"); return v > 0 ? v : 30 }
/// Upper bound on one `ai-usagebar` invocation. It can block on the cache
/// flock (up to 15s) and then refresh OAuth over the network, so without a
/// bound a hung run holds a worker indefinitely and the panel simply stops
/// updating with no explanation. Matches the GNOME extension's own timeout.
let REFRESH_TIMEOUT: Double = 45
var BAR_WIDTH: Int { max(4, min(20, DEF.integer(forKey: "barWidth"))) }
let MENU_BAR_W = 14
var SHOW_SESSION: Bool { DEF.bool(forKey: "showSession") }
var SHOW_WEEKLY: Bool { DEF.bool(forKey: "showWeekly") }
var SHOW_EXTRA: Bool { DEF.bool(forKey: "showExtra") }
var SHOW_PERCENT: Bool { DEF.bool(forKey: "showPercent") }
var SHOW_BARS: Bool { DEF.bool(forKey: "showBars") }
// Layout of the progress indicator: "block" (default, text bars ░█) or "ring"
// (a Core Graphics arc image). Both honor the meta marker the same way.
var BAR_STYLE: String { DEF.string(forKey: "barStyle") ?? "block" }
var COLOR_LOW: String { DEF.string(forKey: "colorLow") ?? "#98c379" }
var COLOR_MID: String { DEF.string(forKey: "colorMid") ?? "#e5c07b" }
var COLOR_HIGH: String { DEF.string(forKey: "colorHigh") ?? "#d19a66" }
var COLOR_CRITICAL: String { DEF.string(forKey: "colorCritical") ?? "#e06c75" }
var COLOR_EMPTY: String { DEF.string(forKey: "colorEmpty") ?? "#3e4451" }
// Meta reference: draw a pace marker at the elapsed-time position and flag the
// over-meta segment of the fill. Off = plain absolute-usage bars, no marker.
var SHOW_META: Bool { DEF.bool(forKey: "showMeta") }
// The meta marker is a fixed blue, matching the binary's default theme `marker`
// color and distinct from the over-pace warning fill.
let COLOR_MARKER = "#61afef"
let POINT_MID_MIN = -10
let POINT_CRITICAL_MIN = 10

// The `{scoped_*}` fields (10-12) carry the model-scoped weekly window (e.g.
// "Fable") from the API's `limits[]`; empty on older binaries → the row falls
// back to the flat `{sonnet_*}` window and the "Sonnet only" label. The trailing
// `*_elapsed` fields (13-15) carry the meta (pace) position; `vendor_short`
// (16) lets balance-only vendors suppress meaningless quota rows; the trailing
// OpenRouter balance (17) is shown as credits. A final literal sentinel absorbs
// the widget's stale suffix, preserving these fields.
let FORMAT = "{plan};;{session_pct};;{session_reset};;{weekly_pct};;{weekly_reset};;" +
             "{sonnet_pct};;{sonnet_reset};;{extra_pct};;{extra_spent};;{extra_limit};;" +
             "{scoped_model};;{scoped_pct};;{scoped_reset};;" +
             "{session_elapsed};;{weekly_elapsed};;{scoped_elapsed};;{vendor_short};;{or_balance};;" +
             "{ds_balance};;{kilo_balance};;{nv_balance};;{km_balance};;{grok_balance};;" +
             "{aapi_headline};;{aapi_pct};;{aapi_spent};;{aapi_limit}"

let FORMAT_WITH_SENTINEL = FORMAT + ";;__aiub_end__"

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

// Matches pacing::pace_severity: < -10 low, -10...0 mid, 1...9 high, >= 10 critical.
func colorForDelta(_ delta: Int) -> NSColor {
    if delta >= POINT_CRITICAL_MIN { return hexColor(COLOR_CRITICAL) }
    if delta > 0 { return hexColor(COLOR_HIGH) }
    if delta >= POINT_MID_MIN { return hexColor(COLOR_MID) }
    return hexColor(COLOR_LOW)
}

func menuBarTextColor(_ appearance: NSAppearance, secondary: Bool = false) -> NSColor {
    let isDark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
    let color = isDark ? NSColor.white : NSColor.black
    return secondary ? color.withAlphaComponent(0.72) : color
}

// The ring track needs to stay visible over both light and dark menu bars /
// dropdowns. The user-configured COLOR_EMPTY is a dark charcoal meant for the
// block bar's ░ glyphs on a light surface; on a dark wallpaper or dark menu bar
// it vanishes. So the ring track uses a faint white in dark appearance (visible
// against the dark background) and falls back to COLOR_EMPTY in light
// appearance, keeping parity with the block bar there.
func ringTrackColor(_ appearance: NSAppearance) -> NSColor {
    let isDark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
    return isDark ? NSColor.white.withAlphaComponent(0.25) : hexColor(COLOR_EMPTY)
}

// A missing reset keeps its row visible but never has a meaningful pace marker.
func markerElapsed(reset: String, elapsed: Int?) -> Int? {
    guard !reset.isEmpty, reset != "—" else { return nil }
    return elapsed
}

let barFont = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)

func run(_ s: String, _ color: NSColor, _ font: NSFont = barFont) -> NSAttributedString {
    NSAttributedString(string: s, attributes: [.foregroundColor: color, .font: font])
}

// Block bar. When `elapsed` (0..100) is known and the meta is on, the fill stays
// in the calm absolute-usage color up to a blue marker at the elapsed position,
// and only the part that overshoots the meta (how far ahead of pace you are →
// risk of paid extra usage) is painted in the warning color. Otherwise it's a
// plain absolute-color bar with no marker.
func barAttr(pct: Int, width: Int, elapsed: Int?) -> NSAttributedString {
    let p = max(0, min(100, pct))
    let filled = Int((Double(p) * Double(width) / 100.0).rounded())
    let out = NSMutableAttributedString()

    guard SHOW_META, let elapsedVal = elapsed else {
        out.append(run(String(repeating: "█", count: filled), colorForPct(p)))
        out.append(run(String(repeating: "░", count: max(0, width - filled)), hexColor(COLOR_EMPTY)))
        return out
    }

    let e = max(0, min(100, elapsedVal))
    let base = colorForPct(p)        // on-track portion → calm absolute color
    let over = colorForDelta(p - e)  // excess beyond the meta → pace warning
    var m = Int(Double(e) * Double(width) / 100.0) // floor
    if m > width - 1 { m = width - 1 }
    if m < 0 { m = 0 }
    let preF = min(filled, m)
    let postF = filled > m + 1 ? filled - m - 1 : 0
    let preE = m - preF
    let postE = width - m - 1 - postF
    out.append(run(String(repeating: "█", count: max(0, preF)), base))
    out.append(run(String(repeating: "░", count: max(0, preE)), hexColor(COLOR_EMPTY)))
    out.append(run("│", hexColor(COLOR_MARKER)))
    out.append(run(String(repeating: "█", count: max(0, postF)), over))
    out.append(run(String(repeating: "░", count: max(0, postE)), hexColor(COLOR_EMPTY)))
    return out
}

// Ring indicator (optional layout). A Core Graphics arc whose sweep is the
// usage fraction, painted in the severity color, over a faint track. When the
// meta is on, the elapsed position marks a blue tick and the arc beyond it (how
// far ahead of pace you are) shifts to the pace-warning color — the same idea
// as the block bar, just radial. The image is rendered as an attachment so it
// composes in an attributed string alongside the percentage text.
/// Arc geometry for a ring segment, in degrees for `NSBezierPath.appendArc`.
/// The ring starts at 12 o'clock (`startRad = -π/2`) and fills clockwise, so a
/// segment [from, to] spans `[start - 2π·from, start - 2π·to]`. Pure and tested
/// so the pace-arc regression (overshoot restarting at 12h) cannot return.
func arcAngles(from fromFraction: CGFloat, to toFraction: CGFloat,
               startRad: CGFloat = -.pi / 2) -> (startDeg: CGFloat, endDeg: CGFloat) {
    ((startRad - 2 * .pi * fromFraction) * 180 / .pi,
     (startRad - 2 * .pi * toFraction) * 180 / .pi)
}

func ringImage(pct: Int, size: CGFloat, elapsed: Int?, appearance: NSAppearance) -> NSImage {
    let p = CGFloat(max(0, min(100, pct))) / 100.0
    let img = NSImage(size: NSSize(width: size, height: size))
    img.lockFocus()

    let box = NSRect(x: 0, y: 0, width: size, height: size)
    let lw = max(1.6, size * 0.16)
    let inset = lw / 2 + 0.5
    let rect = box.insetBy(dx: inset, dy: inset)
    let start: CGFloat = -.pi / 2

    // Track (empty background ring).
    let track = NSBezierPath()
    track.appendArc(withCenter: CGPoint(x: size / 2, y: size / 2),
                    radius: rect.width / 2, startAngle: 0, endAngle: 360)
    track.lineWidth = lw
    ringTrackColor(appearance).setStroke()
    track.stroke()

    // Filled arc. With the meta on, the part behind the pace marker keeps the
    // calm absolute color and the overshoot turns warning; otherwise a single
    // severity-colored sweep. The helper draws [from, to] so the overshoot can
    // continue from the elapsed marker to pct instead of restarting at 12h.
    let drawArc = { (fromFraction: CGFloat, toFraction: CGFloat, color: NSColor) in
        guard toFraction > fromFraction else { return }
        let a = arcAngles(from: fromFraction, to: toFraction, startRad: start)
        let arc = NSBezierPath()
        arc.appendArc(withCenter: CGPoint(x: size / 2, y: size / 2),
                      radius: rect.width / 2,
                      startAngle: a.startDeg,
                      endAngle: a.endDeg,
                      clockwise: true)
        arc.lineWidth = lw
        color.setStroke()
        arc.stroke()
    }
    let pInt = max(0, min(100, pct))
    if SHOW_META, let elapsedVal = elapsed {
        let e = max(0, min(100, elapsedVal))
        let base = colorForPct(pInt)
        let over = colorForDelta(pInt - e)
        let eFrac = CGFloat(e) / 100.0
        let boundary = min(p, eFrac)
        drawArc(0, boundary, base)
        if p > eFrac { drawArc(boundary, p, over) }
        // Pace tick at the elapsed position.
        let tickAngle = start - 2 * .pi * eFrac
        let c = CGPoint(x: size / 2, y: size / 2)
        let r = rect.width / 2
        let tick = NSBezierPath()
        tick.move(to: CGPoint(x: c.x + (r - lw) * cos(tickAngle),
                              y: c.y + (r - lw) * sin(tickAngle)))
        tick.line(to: CGPoint(x: c.x + (r + lw) * cos(tickAngle),
                              y: c.y + (r + lw) * sin(tickAngle)))
        tick.lineWidth = max(1, lw * 0.5)
        hexColor(COLOR_MARKER).setStroke()
        tick.stroke()
    } else {
        drawArc(0, p, colorForPct(pInt))
    }
    img.unlockFocus()
    img.isTemplate = false
    return img
}

final class ColoredAttachmentCell: NSTextAttachmentCell {
    override func draw(withFrame cellFrame: NSRect, in controlView: NSView?) {
        guard let image else { return }
        image.draw(in: cellFrame,
                   from: .zero,
                   operation: .sourceOver,
                   fraction: 1.0,
                   respectFlipped: true,
                   hints: nil)
    }
}

func ringAttr(pct: Int, size: CGFloat, elapsed: Int?, appearance: NSAppearance) -> NSAttributedString {
    let out = NSMutableAttributedString()
    let attachment = NSTextAttachment()
    let image = ringImage(pct: pct, size: size, elapsed: elapsed, appearance: appearance)
    attachment.image = image
    attachment.attachmentCell = ColoredAttachmentCell(imageCell: image)
    // Vertically center the ring on the text's cap height rather than sitting it
    // on the baseline: without this the attachment grows upward only, so larger
    // rings drift toward the top of the menu bar. The origin is baseline-relative,
    // so offset by half the gap between the image and the cap height.
    let cap = barFont.capHeight
    let dy = (cap - size) / 2
    attachment.bounds = NSRect(x: 0, y: dy, width: size, height: size)
    out.append(NSAttributedString(attachment: attachment))
    return out
}

// Dispatches to the block bar or the ring according to the selected layout, so
// the panel and dropdown render with one call regardless of style. The ring has
// its own fixed pixel sizes (it does not scale with the block `width`, which is
// a character count); `menu` picks the larger ring used in dropdown rows. The
// appearance is threaded through so the ring track can adapt to light/dark.
func progressAttr(pct: Int, width: Int, elapsed: Int?, menu: Bool = false,
                  appearance: NSAppearance) -> NSAttributedString {
    if BAR_STYLE == "ring" {
        let size: CGFloat = menu ? CGFloat(MENU_BAR_W) + 4 : CGFloat(BAR_WIDTH) + 6
        return ringAttr(pct: pct, size: size, elapsed: elapsed, appearance: appearance)
    }
    return barAttr(pct: pct, width: width, elapsed: elapsed)
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
struct Window { let pct: Int; let reset: String; let elapsed: Int? }
struct Snapshot {
    let plan: String
    let hasUsageWindows: Bool
    let creditBalance: String?
    let session: Window
    let weekly: Window
    /// The per-model weekly bar (model-scoped window, e.g. Fable, or the legacy
    /// flat sonnet window).
    let sonnet: Window?
    /// Label for that bar: the scoped model name ("Fable") or "Sonnet only".
    let sonnetLabel: String
    let extra: (pct: Int, spent: String, limit: String)?
}

func stripMarkup(_ s: String) -> String {
    // Decode exactly one layer after removing tags. Rust escapes API-controlled
    // labels for Pango; the native surface consumes plain text and must not
    // display those entities literally or reactivate decoded markup.
    s.replacingOccurrences(of: "<[^>]*>", with: "", options: .regularExpression)
        .replacingOccurrences(of: "&lt;", with: "<")
        .replacingOccurrences(of: "&gt;", with: ">")
        .replacingOccurrences(of: "&quot;", with: "\"")
        .replacingOccurrences(of: "&apos;", with: "'")
        .replacingOccurrences(of: "&amp;", with: "&")
}

func parse(_ text: String, vendor: String) -> Snapshot? {
    let f = stripMarkup(text).components(separatedBy: ";;")
    guard f.count >= 10 else { return nil }
    func unknownPlaceholder(_ s: String) -> Bool {
        s.hasPrefix("{") && s.hasSuffix("}")
    }
    func t(_ i: Int) -> String {
        guard i < f.count else { return "" }
        let v = f[i].trimmingCharacters(in: .whitespaces)
        return unknownPlaceholder(v) ? "" : v
    }
    // Do not accept a numeric prefix: a stale suffix such as "27 ⏸" is not elapsed.
    func n(_ i: Int) -> Int? {
        let value = t(i)
        guard value.range(of: "^-?[0-9]+$", options: .regularExpression) != nil else { return nil }
        return Int(value)
    }
    // Third bar = the per-model weekly window: a non-empty scoped model is the
    // presence signal. Its reset can legitimately be unavailable, so do not
    // mistake a missing reset for an absent scoped window and show Sonnet.
    let sonnetReset = t(6)
    var sonnet: Window? = nil
    var sonnetLabel = "Sonnet only"
    let scopedReset = t(12)
    let scopedModel = t(10)
    if !scopedModel.isEmpty {
        // A malformed scoped percentage is unavailable too, but must not make
        // us fall back to the unrelated legacy Sonnet window.
        if let p = n(11), (0...100).contains(p) {
            let reset = scopedReset.isEmpty ? "—" : scopedReset
            sonnet = Window(pct: p, reset: reset, elapsed: markerElapsed(reset: reset, elapsed: n(15)))
            sonnetLabel = scopedModel
        }
    } else if !sonnetReset.isEmpty, sonnetReset != "—", let p = n(5) {
        sonnet = Window(pct: p, reset: sonnetReset, elapsed: nil)
    }
    let spent = t(8)
    let limit = t(9)
    let extra: (pct: Int, spent: String, limit: String)? =
        (spent.isEmpty || limit.isEmpty) ? nil : n(7).map { (pct: $0, spent: spent, limit: limit) }
    // Dispatch the balance by the SELECTED vendor, not by vendor_short: Kimi and
    // Moonshot both report vendor_short = "kmi" in the Rust binary, so keying on
    // vendor_short would collide and read the wrong field.
    let balanceFieldIndex: Int?
    switch vendor {
    case "openrouter": balanceFieldIndex = 17
    case "deepseek": balanceFieldIndex = 18
    case "kilo": balanceFieldIndex = 19
    case "novita": balanceFieldIndex = 20
    case "moonshot": balanceFieldIndex = 21
    case "grok": balanceFieldIndex = 22
    case "anthropic_api": balanceFieldIndex = 23
    default: balanceFieldIndex = nil
    }
    let balance = balanceFieldIndex.flatMap { t($0).isEmpty ? nil : t($0) }
    // Vendors with no rate-limit windows show only a balance; suppress the fake
    // 5h/7d 0% rows their session_pct/weekly_pct aliases would otherwise paint.
    let balanceOnly = balanceFieldIndex != nil
    // Anthropic API exposes spend-vs-limit instead of a balance, and reports the
    // spend % through the session/weekly aliases. When a limit is configured it
    // becomes an extra ($) bar; otherwise it is balance-only headline display.
    // FORMAT tail: aapi_headline(23) aapi_pct(24) aapi_spent(25) aapi_limit(26).
    let aapiLimit = t(26)
    let aapiExtra: (pct: Int, spent: String, limit: String)?
    if vendor == "anthropic_api", !aapiLimit.isEmpty, aapiLimit != "—",
       let aapiPct = n(24), (0...100).contains(aapiPct), !t(25).isEmpty {
        aapiExtra = (pct: aapiPct, spent: t(25), limit: aapiLimit)
    } else {
        aapiExtra = nil
    }
    // With a limit configured the spend-vs-limit bar replaces the headline, so
    // avoid showing both the "cr" balance and the extra ($) row at once.
    let displayBalance = aapiExtra == nil ? balance : nil
    return Snapshot(plan: t(0),
                    hasUsageWindows: !balanceOnly,
                    creditBalance: displayBalance,
                    session: Window(pct: n(1) ?? 0, reset: t(2), elapsed: markerElapsed(reset: t(2), elapsed: n(13))),
                    weekly: Window(pct: n(3) ?? 0, reset: t(4), elapsed: markerElapsed(reset: t(4), elapsed: n(14))),
                    sonnet: sonnet,
                    sonnetLabel: sonnetLabel,
                    extra: aapiExtra ?? extra)
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
    VendorAuth(id: "kimi", name: "Kimi", kind: "apikey", cli: "", login: "", pkg: "", env: "KIMI_API_KEY"),
    VendorAuth(id: "kilo", name: "Kilo", kind: "apikey", cli: "", login: "", pkg: "", env: "KILO_API_KEY"),
    VendorAuth(id: "novita", name: "Novita", kind: "apikey", cli: "", login: "", pkg: "", env: "NOVITA_API_KEY"),
    VendorAuth(id: "moonshot", name: "Moonshot", kind: "apikey", cli: "", login: "", pkg: "", env: "MOONSHOT_API_KEY"),
    VendorAuth(id: "grok", name: "Grok (xAI)", kind: "apikey", cli: "", login: "", pkg: "", env: "XAI_MANAGEMENT_KEY"),
    VendorAuth(id: "anthropic_api", name: "Anthropic (API)", kind: "apikey", cli: "", login: "", pkg: "", env: "ANTHROPIC_ADMIN_KEY"),
]

// The config file the Rust binary would actually read. On macOS
// `directories::ProjectDirs` resolves to ~/Library/Application Support, so
// checking only ~/.config reported "no key configured" for a key the binary
// was happily using. Prefer the canonical location, fall back to the legacy
// Unix path the docs have always shown (the binary accepts both).
func configPathTOML() -> String {
    let appSupport = "\(NSHomeDirectory())/Library/Application Support/ai-usagebar/config.toml"
    if FileManager.default.fileExists(atPath: appSupport) { return appSupport }
    return "\(NSHomeDirectory())/.config/ai-usagebar/config.toml"
}

func configHasApiKeyTOML(_ section: String) -> Bool {
    guard let value = configValueTOML(section, "api_key") else { return false }
    return !value.isEmpty
}

func configEnabledTOML(_ section: String) -> Bool? {
    guard let value = configValueTOML(section, "enabled") else { return nil }
    switch value.lowercased() {
    case "true": return true
    case "false": return false
    default: return nil
    }
}

/// Read a single `key` under `[section]` from TOML text. Pure (no filesystem)
/// so the enabled-flag and api_key_env parsing is testable. Handles quoted
/// strings, bare booleans (`enabled = false`), inline comments, and `api_key_env`.
func tomlValueInText(_ text: String, section: String, key: String) -> String? {
    var inSection = false
    for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
        let line = String(raw).trimmingCharacters(in: .whitespaces)
        if line.hasPrefix("[") {
            inSection = line == "[\(section)]"
            continue
        }
        guard inSection, !line.hasPrefix("#") else { continue }
        let parts = line.split(separator: "=", maxSplits: 1).map(String.init)
        guard parts.count == 2, parts[0].trimmingCharacters(in: .whitespaces) == key else { continue }
        // Strip an inline comment before evaluating the value: `enabled = false  # opt-in`
        // is a bare boolean, not a string starting with '#'.
        var value = parts[1].trimmingCharacters(in: .whitespaces)
        if let hash = value.firstIndex(of: "#") {
            value = String(value[..<hash]).trimmingCharacters(in: .whitespaces)
        }
        if let quote = value.first, quote == "\"" || quote == "'" {
            let content = value.dropFirst()
            guard let end = content.firstIndex(of: quote) else { continue }
            return String(content[..<end])
        }
        // Bare tokens (booleans, numbers) reach here. Only `true`/`false` are
        // meaningful for the keys this reader serves; everything else is left
        // for the caller to ignore.
        return value
    }
    return nil
}

func configValueTOML(_ section: String, _ key: String) -> String? {
    let path = configPathTOML()
    guard let text = try? String(contentsOfFile: path, encoding: .utf8) else { return nil }
    return tomlValueInText(text, section: section, key: key)
}

func apiKeyEnvironment(_ v: VendorAuth) -> String {
    configValueTOML(v.id, "api_key_env") ?? v.env
}

/// Rust defaults (`src/config.rs`): the OAuth/api-key vendors that ship enabled,
/// versus the opt-in balance vendors that default to disabled. An omitted
/// `[vendor].enabled` must reproduce these, not silently enable everything.
func defaultEnabled(_ id: String) -> Bool {
    switch id {
    case "anthropic", "openai", "zai", "openrouter": return true
    case "deepseek", "kimi", "kilo", "novita", "moonshot", "grok", "anthropic_api": return false
    default: return true
    }
}

func vendorEnabled(_ v: VendorAuth) -> Bool {
    if let explicit = configEnabledTOML(v.id) { return explicit }
    return defaultEnabled(v.id)
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
    guard vendorEnabled(v) else { return false }
    let home = NSHomeDirectory()
    let fm = FileManager.default
    if v.id == "anthropic" {
        return fm.fileExists(atPath: "\(home)/.claude/.credentials.json") || keychainHasClaude()
    }
    if v.id == "openai" {
        return fm.fileExists(atPath: "\(home)/.codex/auth.json")
    }
    if let e = ProcessInfo.processInfo.environment[apiKeyEnvironment(v)], !e.isEmpty { return true }
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
        GroupBox("Vendors") {
            VStack(alignment: .leading, spacing: 8) {
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
            .frame(maxWidth: .infinity, alignment: .leading)
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
                // OAuth vendors need their CLI to log in; apikey vendors are
                // configured via the TUI.
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
        return "⚠ Sem API key — \(apiKeyEnvironment(v))"
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
    @AppStorage("showMeta") private var showMeta = true
    @AppStorage("barStyle") private var barStyle = "block"
    @AppStorage("colorLow") private var colorLow = "#98c379"
    @AppStorage("colorMid") private var colorMid = "#e5c07b"
    @AppStorage("colorHigh") private var colorHigh = "#d19a66"
    @AppStorage("colorCritical") private var colorCritical = "#e06c75"
    @AppStorage("colorEmpty") private var colorEmpty = "#3e4451"
    @AppStorage("binaryPath") private var binaryPath = ""

    // Only enabled vendors appear in the selector: Rust treats opt-in vendors
    // (deepseek/kimi/kilo/novita/moonshot/grok/anthropic_api) as disabled when
    // their `[vendor].enabled` is omitted, and so must this picker.
    private var vendors: [String] {
        VENDOR_AUTH.filter { vendorEnabled($0) }.map { $0.id }
    }

    var body: some View {
        // A ScrollView (not a Form) so the pane reliably scrolls on every macOS
        // version: on short displays the window can't grow past the screen, and
        // a plain Form clipped its top rows with no way to reach them.
        ScrollView(.vertical) {
            VStack(alignment: .leading, spacing: 18) {
                GroupBox("Exibição") {
                    VStack(alignment: .leading, spacing: 8) {
                        Toggle("Mostrar barra de 5h (sessão)", isOn: $showSession)
                        Toggle("Mostrar barra semanal", isOn: $showWeekly)
                        Toggle("Mostrar barra de uso extra ($)", isOn: $showExtra)
                        Toggle("Mostrar porcentagem/valor", isOn: $showPercent)
                        Toggle("Mostrar barras (off = só números)", isOn: $showBars)
                        Toggle("Mostrar referência da meta (linha de ritmo)", isOn: $showMeta)
                        Picker("Estilo do indicador", selection: $barStyle) {
                            Text("Barras (░█)").tag("block")
                            Text("Anel (○)").tag("ring")
                        }
                        Stepper("Largura da barra: \(barWidth)", value: $barWidth, in: 4...20)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                GroupBox("Cores") {
                    VStack(alignment: .leading, spacing: 8) {
                        HexColorPicker(title: "Baixo (<50%)", hex: $colorLow)
                        HexColorPicker(title: "Médio (50–74%)", hex: $colorMid)
                        HexColorPicker(title: "Alto (75–89%)", hex: $colorHigh)
                        HexColorPicker(title: "Crítico (≥90%)", hex: $colorCritical)
                        HexColorPicker(title: "Vazio (fundo da barra)", hex: $colorEmpty)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                GroupBox("Dados") {
                    VStack(alignment: .leading, spacing: 8) {
                        Picker("Vendor", selection: $vendor) {
                            ForEach(vendors, id: \.self) { Text($0) }
                        }
                        Stepper("Intervalo: \(Int(interval))s", value: $interval, in: 5...3600, step: 5)
                        TextField("Caminho do binário (vazio = auto)", text: $binaryPath)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                VendorsSection()
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(width: 460)
        .frame(minHeight: 300, idealHeight: 560, maxHeight: .infinity)
    }
}

// ─── App ─────────────────────────────────────────────────────────────────
class AppDelegate: NSObject, NSApplicationDelegate {
    var statusItem: NSStatusItem!
    var timer: Timer?
    var prefsWindow: NSWindow?
    var appearanceObservation: NSKeyValueObservation?
    var lastSnapshot: Snapshot?
    var pendingRefresh: DispatchWorkItem?
    /// Bumped on every refresh attempt. A result whose generation is no longer
    /// current belongs to a superseded attempt — most often the previously
    /// selected vendor — and must not be rendered. Without this, the timer,
    /// the Preferences window and a vendor change could each start their own
    /// subprocess and whichever finished last won, regardless of what the user
    /// had actually selected. Main-thread only.
    var refreshGeneration: Int = 0
    /// At most one subprocess in flight; a request arriving while one runs is
    /// coalesced rather than stacked.
    var refreshInFlight = false
    var refreshQueued = false
    let headerItem = NSMenuItem()
    var rows: [String: NSMenuItem] = [:]
    // Rebuilt on every render so only configured vendors show, and the active
    // one is checked. Kept as a field so the menu owns it for its lifetime.
    let vendorSubmenu = NSMenu()
    let vendorSubmenuItem = NSMenuItem(title: "Trocar vendor", action: nil, keyEquivalent: "")
    var vendorItems: [NSMenuItem] = []

    func applicationDidFinishLaunching(_ notification: Notification) {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        statusItem.button?.title = "5h …"
        buildMenu()
        rebuildVendorSubmenu()
        observeAppearanceChanges()
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
        vendorSubmenuItem.submenu = vendorSubmenu
        menu.addItem(vendorSubmenuItem)
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
            // Install the host view directly so this window owns its size on
            // macOS 12 as well. The SwiftUI ScrollView still fills the
            // resizable content area without expanding it to its full height.
            let avail = NSScreen.main?.visibleFrame.height ?? 700
            let initialSize = NSSize(width: 460, height: min(560, avail - 40))
            let w = NSWindow(contentRect: NSRect(origin: .zero, size: initialSize),
                             styleMask: [.titled, .closable, .resizable],
                             backing: .buffered,
                             defer: false)
            w.contentViewController = host
            w.title = "AI Usage Bar — Preferências"
            // Resizable so the content can always be reached; a min size keeps
            // it usable, and the initial height is clamped to the visible screen
            // so the top never lands under the menu bar on short displays.
            w.contentMinSize = NSSize(width: 460, height: 360)
            w.setContentSize(initialSize)
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

    func observeAppearanceChanges() {
        guard let button = statusItem.button else { return }
        appearanceObservation = button.observe(\NSStatusBarButton.effectiveAppearance,
                                               options: [.new]) { [weak self] _, _ in
            DispatchQueue.main.async { self?.rerenderAppearance() }
        }
    }

    func rerenderAppearance() {
        guard let snapshot = lastSnapshot else { return }
        renderPanel(snapshot)
    }

    func refresh() {
        guard let bin = resolveBinary("ai-usagebar") else {
            setError("ai-usagebar não encontrado (PATH / ~/.cargo/bin / homebrew)")
            return
        }
        // Coalesce: one subprocess at a time, and remember that another was
        // asked for so a vendor change during a fetch is not simply dropped.
        if refreshInFlight {
            refreshQueued = true
            return
        }
        refreshInFlight = true
        refreshGeneration += 1
        let generation = refreshGeneration
        // Captured for THIS attempt: reading `VENDOR` again on completion would
        // label a late result with whatever is selected by then.
        let vendor = VENDOR

        DispatchQueue.global(qos: .utility).async { [weak self] in
            let p = Process()
            p.executableURL = URL(fileURLWithPath: bin)
            p.arguments = ["--vendor", vendor, "--format", FORMAT_WITH_SENTINEL]
            let pipe = Pipe()
            p.standardOutput = pipe
            p.standardError = FileHandle.nullDevice

            // The subprocess takes the cache lock and may refresh OAuth over
            // the network; without a bound it can hold this worker for a very
            // long time. Kill it and report instead of hanging silently.
            let watchdog = DispatchWorkItem { if p.isRunning { p.terminate() } }
            DispatchQueue.global(qos: .utility)
                .asyncAfter(deadline: .now() + REFRESH_TIMEOUT, execute: watchdog)

            var out = ""
            do {
                try p.run()
                let data = pipe.fileHandleForReading.readDataToEndOfFile()  // read before wait
                p.waitUntilExit()
                out = String(data: data, encoding: .utf8) ?? ""
            } catch {
                watchdog.cancel()
                DispatchQueue.main.async {
                    self?.finishRefresh(generation) { $0.setError("falha ao executar ai-usagebar") }
                }
                return
            }
            watchdog.cancel()
            let timedOut = out.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            DispatchQueue.main.async {
                self?.finishRefresh(generation) { me in
                    // Selection may have changed while this ran.
                    guard vendor == VENDOR else { return }
                    if timedOut {
                        me.setError("ai-usagebar demorou demais (>\(Int(REFRESH_TIMEOUT))s)")
                    } else {
                        me.consume(out)
                    }
                }
            }
        }
    }

    /// Applies `body` only when `generation` is still the current attempt, then
    /// releases the in-flight slot and runs any request that arrived meanwhile.
    private func finishRefresh(_ generation: Int, _ body: (AppDelegate) -> Void) {
        let current = generation == refreshGeneration
        if current {
            refreshInFlight = false
            body(self)
        }
        if current && refreshQueued {
            refreshQueued = false
            refresh()
        }
    }

    func consume(_ output: String) {
        guard let data = output.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let text = obj["text"] as? String else {
            setError("saída inválida")
            return
        }
        guard let snap = parse(text, vendor: VENDOR) else {
            lastSnapshot = nil
            let appearance = statusItem.button?.effectiveAppearance ?? NSApp.effectiveAppearance
            statusItem.button?.attributedTitle = run(stripMarkup(text), menuBarTextColor(appearance))  // Loading… / ⚠
            return
        }
        lastSnapshot = snap
        renderPanel(snap)
        renderMenu(snap)
    }

    func renderPanel(_ s: Snapshot) {
        let title = NSMutableAttributedString()
        let appearance = statusItem.button?.effectiveAppearance ?? NSApp.effectiveAppearance
        let primaryTextColor = menuBarTextColor(appearance)
        let secondaryTextColor = menuBarTextColor(appearance, secondary: true)
        func seg(_ tag: String, _ pct: Int, _ value: String, _ elapsed: Int?) {
            if title.length > 0 { title.append(run("   ", secondaryTextColor)) }
            title.append(run("\(tag) ", secondaryTextColor))
            if SHOW_PERCENT { title.append(run(value + (SHOW_BARS ? " " : ""), colorForPct(pct))) }
            if SHOW_BARS { title.append(progressAttr(pct: pct, width: BAR_WIDTH, elapsed: elapsed, appearance: appearance)) }
            if !SHOW_PERCENT && !SHOW_BARS { title.append(run(value, colorForPct(pct))) }
        }
        if let creditBalance = s.creditBalance {
            title.append(run("cr ", secondaryTextColor))
            title.append(run(creditBalance, primaryTextColor))
        } else if s.hasUsageWindows && SHOW_SESSION {
            seg("5h", s.session.pct, "\(s.session.pct)%", s.session.elapsed)
        }
        if s.creditBalance == nil && s.hasUsageWindows && SHOW_WEEKLY {
            seg("7d", s.weekly.pct, "\(s.weekly.pct)%", s.weekly.elapsed)
        }
        if SHOW_EXTRA, let e = s.extra { seg("ex", e.pct, e.spent, nil) } // $ budget → no meta
        statusItem.button?.attributedTitle = title.length > 0 ? title : run("ai", secondaryTextColor)
    }

    func renderMenu(_ s: Snapshot) {
        let appearance = statusItem.button?.effectiveAppearance ?? NSApp.effectiveAppearance
        headerItem.attributedTitle = run(s.plan.isEmpty ? "AI Usage" : s.plan,
                                         .labelColor, NSFont.boldSystemFont(ofSize: 13))

        func row(_ key: String, _ name: String, _ pct: Int, _ value: String, _ reset: String?, _ elapsed: Int?) {
            guard let item = rows[key] else { return }
            item.isHidden = false
            let a = NSMutableAttributedString()
            let label = name.count < 12
                ? name.padding(toLength: 12, withPad: " ", startingAt: 0)
                : name
            a.append(run(label, .labelColor))
            a.append(progressAttr(pct: pct, width: MENU_BAR_W, elapsed: elapsed, menu: true, appearance: appearance))
            a.append(run("  \(value)", colorForPct(pct)))
            if let r = reset, !r.isEmpty { a.append(run("   ↺ \(r)", .secondaryLabelColor)) }
            item.attributedTitle = a
        }
        if let creditBalance = s.creditBalance {
            rows["session"]?.isHidden = false
            rows["session"]?.attributedTitle = run("Credits      \(creditBalance)", .labelColor)
            rows["weekly"]?.isHidden = true
            rows["sonnet"]?.isHidden = true
        } else if s.hasUsageWindows {
            row("session", "Session", s.session.pct, "\(s.session.pct)%", s.session.reset, s.session.elapsed)
            row("weekly", "Weekly", s.weekly.pct, "\(s.weekly.pct)%", s.weekly.reset, s.weekly.elapsed)
        } else {
            rows["session"]?.isHidden = true
            rows["weekly"]?.isHidden = true
        }
        if let sn = s.sonnet { row("sonnet", s.sonnetLabel, sn.pct, "\(sn.pct)%", sn.reset, sn.elapsed) }
        else { rows["sonnet"]?.isHidden = true }
        if let e = s.extra { row("extra", "Extra usage", e.pct, "\(e.spent) / \(e.limit)", nil, nil) }
        else { rows["extra"]?.isHidden = true }
        rebuildVendorSubmenu()
    }

    // Vendor switch submenu: lists only configured vendors, with a checkmark on
    // the active one. Selecting one rewrites the `vendor` default and triggers a
    // refresh via the shared settings-change observer.
    func rebuildVendorSubmenu() {
        vendorSubmenu.removeAllItems()
        vendorItems = []
        let active = VENDOR
        let configured = VENDOR_AUTH.filter { vendorEnabled($0) && ($0.id == active || vendorConfigured($0)) }
        if configured.isEmpty {
            let none = NSMenuItem(title: "Nenhum configurado", action: nil, keyEquivalent: "")
            none.isEnabled = false
            vendorSubmenu.addItem(none)
            vendorSubmenuItem.isHidden = false
            return
        }
        for v in configured {
            let it = NSMenuItem(title: v.name, action: #selector(switchVendor(_:)), keyEquivalent: "")
            it.target = self
            it.representedObject = v.id
            it.state = (v.id == active) ? .on : .off
            vendorSubmenu.addItem(it)
            vendorItems.append(it)
        }
        vendorSubmenuItem.isHidden = false
    }

    @objc func switchVendor(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        DEF.set(id, forKey: "vendor")
    }

    func setError(_ msg: String) {
        lastSnapshot = nil
        statusItem.button?.attributedTitle = run("⚠ ai", hexColor(COLOR_CRITICAL))
        let appearance = statusItem.button?.effectiveAppearance ?? NSApp.effectiveAppearance
        headerItem.attributedTitle = run(msg, menuBarTextColor(appearance))
        for (_, it) in rows { it.isHidden = true }
    }
}

#if !SWIFT_TEST_HARNESS
@main
struct AppMain {
    static func main() {
        DEF.register(defaults: SETTINGS_DEFAULTS)
        let app = NSApplication.shared
        let delegate = AppDelegate()
        app.delegate = delegate
        app.setActivationPolicy(.accessory)   // menu-bar agent, no Dock icon
        app.run()
    }
}
#endif
