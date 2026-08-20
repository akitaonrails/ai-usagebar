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
import Carbon.HIToolbox  // RegisterEventHotKey for the global vendor-swap shortcut

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
// (16) lets balance-only vendors suppress meaningless quota rows. The balance
// fields (17-22) carry the per-vendor credits — only the selected vendor's is
// populated — and the `aapi_*` fields (23-26) carry the Anthropic API headline
// plus its spend-vs-limit bar. `cursor_total_pct` (27) is followed by the
// Antigravity-only fourth-window fields (28-30). A final literal sentinel
// absorbs the widget's stale suffix, preserving these fields.
let FORMAT = "{plan};;{session_pct};;{session_reset};;{weekly_pct};;{weekly_reset};;" +
             "{sonnet_pct};;{sonnet_reset};;{extra_pct};;{extra_spent};;{extra_limit};;" +
             "{scoped_model};;{scoped_pct};;{scoped_reset};;" +
             "{session_elapsed};;{weekly_elapsed};;{scoped_elapsed};;{vendor_short};;{or_balance};;" +
             "{ds_balance};;{kilo_balance};;{nv_balance};;{km_balance};;{grok_balance};;" +
             "{aapi_headline};;{aapi_pct};;{aapi_spent};;{aapi_limit};;{cursor_total_pct};;" +
             "{extra_model};;{extra_reset};;{extra_elapsed}"

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

/// Squeeze a countdown ("4d 1h", "2h 05m", "now") down to its leading unit for
/// the overview status-bar title, where every character fights for space:
/// "4d 1h" → "4d", "2h 05m" → "2h", "0h 05m" → "5m". Missing/em-dash → nil.
func shortReset(_ r: String) -> String? {
    guard !r.isEmpty, r != "—" else { return nil }
    let parts = r.split(separator: " ")
    guard let first = parts.first else { return nil }
    if first == "0h", parts.count > 1 {
        let m = parts[1].drop(while: { $0 == "0" })
        return m == "m" ? "0m" : String(m)
    }
    return String(first)
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
    let session: Window?
    let weekly: Window?
    /// The per-model weekly bar (model-scoped window, e.g. Fable, or the legacy
    /// flat sonnet window).
    let sonnet: Window?
    /// Label for that bar: the scoped model name ("Fable") or "Sonnet only".
    let sonnetLabel: String
    let extra: (pct: Int, spent: String, limit: String)?
    /// Antigravity's optional fourth rate-limit window (Claude & GPT OSS
    /// weekly). This is distinct from `extra`, which is a monetary budget.
    var secondaryWeekly: Window? = nil
    var secondaryWeeklyLabel: String = ""
    /// Overridable labels for the session/weekly bars. Default to the
    /// time-window names; a vendor whose two windows aren't time-based (Cursor:
    /// "Cursor Models" / "Other Models") sets its own. Tags are the 2-char
    /// prefixes shown on the compact status bar. Defaulted so every other
    /// vendor's construction is unchanged.
    var sessionTag: String = "5h"
    var weeklyTag: String = "7d"
    var sessionLabel: String = "Session"
    var weeklyLabel: String = "Weekly"
    /// Cursor's combined "included total usage" headline (`totalPercentUsed`),
    /// the single number its dashboard shows across both pools. nil for others.
    var cursorTotalPct: Int? = nil
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
    let isAntigravity = vendor == "antigravity"
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
    func quotaWindow(_ pctIndex: Int, _ resetIndex: Int, _ elapsedIndex: Int) -> Window? {
        guard let pct = n(pctIndex), (0...100).contains(pct) else { return nil }
        let reset = t(resetIndex)
        return Window(
            pct: pct,
            reset: reset,
            elapsed: markerElapsed(reset: reset, elapsed: n(elapsedIndex)))
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
            sonnetLabel = isAntigravity ? "\(scopedModel) 5h" : scopedModel
        }
    } else if !sonnetReset.isEmpty, sonnetReset != "—", let p = n(5) {
        sonnet = Window(pct: p, reset: sonnetReset, elapsed: nil)
    }
    let spent = t(8)
    let limit = t(9)
    let extra: (pct: Int, spent: String, limit: String)? =
        (spent.isEmpty || limit.isEmpty) ? nil : n(7).map { (pct: $0, spent: spent, limit: limit) }
    // Dispatch the balance by the SELECTED vendor, not by vendor_short: binaries
    // up to 0.16 report vendor_short = "kmi" for both Kimi and Moonshot, so
    // keying on vendor_short would collide and read the wrong field.
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
    // Cursor carries its two included-usage pools on the session/weekly
    // aliases (session = Cursor Models, weekly = Other Models — both real, not
    // time windows), so relabel those bars rather than call them "Session"/
    // "Weekly". Every other vendor keeps the default time-window labels.
    let isCursor = vendor == "cursor"
    let secondaryWeekly: Window?
    let secondaryWeeklyLabel: String
    if isAntigravity, !t(28).isEmpty {
        secondaryWeekly = quotaWindow(7, 29, 30)
        secondaryWeeklyLabel = "\(t(28)) Weekly"
    } else {
        secondaryWeekly = nil
        secondaryWeeklyLabel = ""
    }
    return Snapshot(plan: t(0),
                    hasUsageWindows: !balanceOnly,
                    creditBalance: displayBalance,
                    session: quotaWindow(1, 2, 13),
                    weekly: quotaWindow(3, 4, 14),
                    sonnet: sonnet,
                    sonnetLabel: sonnetLabel,
                    extra: aapiExtra ?? extra,
                    secondaryWeekly: secondaryWeekly,
                    secondaryWeeklyLabel: secondaryWeeklyLabel,
                    sessionTag: isCursor ? "auto" : "5h",
                    weeklyTag: isCursor ? "premium" : "7d",
                    sessionLabel: isCursor ? "Cursor Models" : (isAntigravity ? "Gemini 5h" : "Session"),
                    weeklyLabel: isCursor ? "Other Models" : (isAntigravity ? "Gemini Weekly" : "Weekly"),
                    cursorTotalPct: isCursor ? n(27) : nil)
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
    VendorAuth(id: "anthropic", name: "Claude", kind: "oauth", cli: "claude", login: "claude", pkg: "@anthropic-ai/claude-code", env: ""),
    VendorAuth(id: "openai", name: "Codex", kind: "oauth", cli: "codex", login: "codex login", pkg: "@openai/codex", env: ""),
    VendorAuth(id: "zai", name: "Z.AI (GLM)", kind: "apikey", cli: "", login: "", pkg: "", env: "ZAI_API_KEY"),
    VendorAuth(id: "openrouter", name: "OpenRouter", kind: "apikey", cli: "", login: "", pkg: "", env: "OPENROUTER_API_KEY"),
    VendorAuth(id: "deepseek", name: "DeepSeek", kind: "apikey", cli: "", login: "", pkg: "", env: "DEEPSEEK_API_KEY"),
    VendorAuth(id: "kimi", name: "Kimi", kind: "apikey", cli: "", login: "", pkg: "", env: "KIMI_API_KEY"),
    VendorAuth(id: "kilo", name: "Kilo", kind: "apikey", cli: "", login: "", pkg: "", env: "KILO_API_KEY"),
    VendorAuth(id: "novita", name: "Novita", kind: "apikey", cli: "", login: "", pkg: "", env: "NOVITA_API_KEY"),
    VendorAuth(id: "moonshot", name: "Moonshot", kind: "apikey", cli: "", login: "", pkg: "", env: "MOONSHOT_API_KEY"),
    VendorAuth(id: "grok", name: "Grok (xAI)", kind: "apikey", cli: "", login: "", pkg: "", env: "XAI_MANAGEMENT_KEY"),
    VendorAuth(id: "anthropic_api", name: "Anthropic API", kind: "apikey", cli: "", login: "", pkg: "", env: "ANTHROPIC_ADMIN_KEY"),
    // Cursor has no API key: the binary reads the session token the Cursor IDE
    // wrote to its own state.vscdb. `kind: "local"` marks the "configured =
    // signed in to the app" case (like Antigravity below), with no login CLI
    // or env var of its own.
    VendorAuth(id: "cursor", name: "Cursor", kind: "local", cli: "", login: "", pkg: "", env: ""),
    // Antigravity 2.0, the `agy` CLI and the IDE are separate products
    // sharing one account-wide quota; any combination may be installed and
    // there is no credential file to check — the binary probes whichever
    // local server is running. `kind: "local"` mirrors Cursor and the GNOME
    // extension (gnome-extension/prefs.js).
    VendorAuth(id: "antigravity", name: "Google Antigravity", kind: "local", cli: "agy", login: "", pkg: "", env: ""),
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
        // A `#` starts a comment only outside quotes. Check for a quoted value
        // first so `api_key = "sk-abc#def"` keeps its embedded `#` instead of
        // being truncated into an unterminated string.
        let value = parts[1].trimmingCharacters(in: .whitespaces)
        if let quote = value.first, quote == "\"" || quote == "'" {
            let content = value.dropFirst()
            guard let end = content.firstIndex(of: quote) else { continue }
            return String(content[..<end])
        }
        // Unquoted value: strip a trailing inline comment — `enabled = false
        // # opt-in` is a bare boolean, not a string starting with '#'.
        var bare = value
        if let hash = bare.firstIndex(of: "#") {
            bare = String(bare[..<hash]).trimmingCharacters(in: .whitespaces)
        }
        // Bare tokens (booleans, numbers) reach here. Only `true`/`false` are
        // meaningful for the keys this reader serves; everything else is left
        // for the caller to ignore.
        return bare
    }
    return nil
}

/// Read a TOML array of quoted strings. This intentionally stays small (the
/// native app is a single dependency-free Swift file), but accepts multiline
/// arrays and rejects malformed values instead of silently widening Overview.
func tomlStringArrayInText(_ text: String, section: String, key: String) -> [String]? {
    func withoutComment(_ line: String) -> String {
        var result = ""
        var quote: Character?
        var escaped = false
        for ch in line {
            if let active = quote {
                result.append(ch)
                if active == "\"", escaped {
                    escaped = false
                } else if active == "\"", ch == "\\" {
                    escaped = true
                } else if ch == active {
                    quote = nil
                }
            } else if ch == "#" {
                break
            } else {
                result.append(ch)
                if ch == "\"" || ch == "'" { quote = ch }
            }
        }
        return result
    }

    func hasClosingBracket(_ value: String) -> Bool {
        var quote: Character?
        var escaped = false
        for ch in value {
            if let active = quote {
                if active == "\"", escaped {
                    escaped = false
                } else if active == "\"", ch == "\\" {
                    escaped = true
                } else if ch == active {
                    quote = nil
                }
            } else if ch == "\"" || ch == "'" {
                quote = ch
            } else if ch == "]" {
                return true
            }
        }
        return false
    }

    var inSection = false
    var rawValue: String?
    for rawLine in text.components(separatedBy: .newlines) {
        let line = withoutComment(rawLine).trimmingCharacters(in: .whitespaces)
        if let current = rawValue {
            let joined = current + " " + line
            rawValue = joined
            if hasClosingBracket(joined) { break }
            continue
        }
        if line.hasPrefix("[") {
            inSection = line == "[\(section)]"
            continue
        }
        guard inSection, !line.isEmpty else { continue }
        let parts = line.split(separator: "=", maxSplits: 1).map(String.init)
        guard parts.count == 2,
              parts[0].trimmingCharacters(in: .whitespaces) == key else { continue }
        let value = parts[1].trimmingCharacters(in: .whitespaces)
        guard value.hasPrefix("[") else { return nil }
        rawValue = value
        if hasClosingBracket(value) { break }
    }

    guard let raw = rawValue, hasClosingBracket(raw) else { return nil }
    let chars = Array(raw)
    guard chars.first == "[" else { return nil }
    var result: [String] = []
    var i = 1

    func skipSpace() {
        while i < chars.count && chars[i].isWhitespace { i += 1 }
    }

    while true {
        skipSpace()
        guard i < chars.count else { return nil }
        if chars[i] == "]" {
            i += 1
            skipSpace()
            return i == chars.count ? result : nil
        }
        let quote = chars[i]
        guard quote == "\"" || quote == "'" else { return nil }
        i += 1
        var value = ""
        var closed = false
        while i < chars.count {
            let ch = chars[i]
            i += 1
            if ch == quote {
                closed = true
                break
            }
            if quote == "\"", ch == "\\", i < chars.count {
                value.append(chars[i])
                i += 1
            } else {
                value.append(ch)
            }
        }
        guard closed else { return nil }
        result.append(value)
        skipSpace()
        guard i < chars.count else { return nil }
        if chars[i] == "," {
            i += 1
            continue
        }
        guard chars[i] == "]" else { return nil }
    }
}

func configValueTOML(_ section: String, _ key: String) -> String? {
    let path = configPathTOML()
    guard let text = try? String(contentsOfFile: path, encoding: .utf8) else { return nil }
    return tomlValueInText(text, section: section, key: key)
}

func configuredOverviewVendorIds() -> [String]? {
    let path = configPathTOML()
    guard let text = try? String(contentsOfFile: path, encoding: .utf8) else { return nil }
    return tomlStringArrayInText(text, section: "ui", key: "overview_vendors")
}

func apiKeyEnvironment(_ v: VendorAuth) -> String {
    configValueTOML(v.id, "api_key_env") ?? v.env
}

// ── Multi-account identities ───────────────────────────────────────────────
// Claude mirrors the Rust side (`src/config.rs`): explicit
// `[[anthropic.accounts]]` entries plus subdirectories of `[anthropic]`
// `accounts_dir`, with explicit labels winning on a clash. Each account is a
// selectable entry with the pseudo-id `anthropic@<label>`, fetched by the
// binary as `--vendor anthropic --account <label>` — the binary resolves the
// account's credentials itself (file or its own Keychain item), so the app
// never needs to know where they live.

/// Account-array labels for one provider, in file order. Pure text scan, like
/// the rest of this app's TOML reading: a `[[provider.accounts]]` header opens
/// a block, and the first `label = "…"` inside it counts.
func accountLabels(inTOML text: String, vendor: String) -> [String] {
    var labels: [String] = []
    var inBlock = false
    let header = "[[\(vendor).accounts]]"
    for rawLine in text.components(separatedBy: .newlines) {
        let line = rawLine.trimmingCharacters(in: .whitespaces)
        if line.hasPrefix("[") {
            inBlock = line == header
            continue
        }
        guard inBlock, line.hasPrefix("label") else { continue }
        let parts = line.split(separator: "=", maxSplits: 1)
        guard parts.count == 2,
              parts[0].trimmingCharacters(in: .whitespaces) == "label" else { continue }
        var value = parts[1].trimmingCharacters(in: .whitespaces)
        if let quote = value.first, quote == "\"" || quote == "'" {
            let content = value.dropFirst()
            guard let end = content.firstIndex(of: quote) else { continue }
            value = String(content[..<end])
        }
        if !value.isEmpty {
            labels.append(value)
            inBlock = false  // one label per block
        }
    }
    return labels
}

func anthropicAccountLabels(inTOML text: String) -> [String] {
    accountLabels(inTOML: text, vendor: "anthropic")
}

/// Immediate subdirectories of `dir`, sorted — the same discovery rule as the
/// Rust `accounts_dir` scan. Credentials may be file- or Keychain-backed.
func discoverAccountLabels(inDir dir: String) -> [String] {
    let fm = FileManager.default
    guard let names = try? fm.contentsOfDirectory(atPath: dir) else { return [] }
    return names.filter { name in
        var isDir: ObjCBool = false
        let sub = "\(dir)/\(name)"
        return fm.fileExists(atPath: sub, isDirectory: &isDir) && isDir.boolValue
    }.sorted()
}

/// Explicit labels first (file order), then discovered ones that don't clash.
func mergedAccountLabels(explicit: [String], discovered: [String]) -> [String] {
    explicit + discovered.filter { !explicit.contains($0) }
}

/// All configured Claude account labels, from the same config the binary reads.
func claudeAccountLabels() -> [String] {
    guard let text = try? String(contentsOfFile: configPathTOML(), encoding: .utf8) else {
        return []
    }
    let explicit = anthropicAccountLabels(inTOML: text)
    var discovered: [String] = []
    if let dir = tomlValueInText(text, section: "anthropic", key: "accounts_dir"), !dir.isEmpty {
        discovered = discoverAccountLabels(inDir: NSString(string: dir).expandingTildeInPath)
    }
    return mergedAccountLabels(explicit: explicit, discovered: discovered)
}

/// Explicit `[[openrouter.accounts]]` labels from the active config.
func openRouterAccountLabels() -> [String] {
    guard let text = try? String(contentsOfFile: configPathTOML(), encoding: .utf8) else {
        return []
    }
    return accountLabels(inTOML: text, vendor: "openrouter")
}

/// Rust semantics (`show_default_account`): `false` hides an unnamed entry,
/// but is ignored when there are no named accounts, so a vendor never loses
/// its only entry.
func showDefaultAccount(configValue: String?, hasAccounts: Bool) -> Bool {
    guard hasAccounts else { return true }
    return configValue != "false"
}

/// Pseudo-id mapping: `<vendor>@<label>` selects a named account.
let CLAUDE_ACCOUNT_ID_PREFIX = "anthropic@"
let OPENROUTER_ACCOUNT_ID_PREFIX = "openrouter@"
// A Claude account whose usage comes from the Desktop app's own token (a saved
// ~/.claude-acc/profiles/<label>), fetched with the widget's `--desktop` flag.
// Distinct prefix so `vendorArgs` knows to pass it; every other helper treats it
// as a Claude account via `accountLabel(of:)`.
let DESKTOP_ACCOUNT_ID_PREFIX = "anthropic-desktop@"

func accountLabel(of id: String) -> String? {
    if id.hasPrefix(DESKTOP_ACCOUNT_ID_PREFIX) {
        return String(id.dropFirst(DESKTOP_ACCOUNT_ID_PREFIX.count))
    }
    guard let separator = id.firstIndex(of: "@") else { return nil }
    let label = String(id[id.index(after: separator)...])
    return label.isEmpty ? nil : label
}

func isDesktopAccountId(_ id: String) -> Bool { id.hasPrefix(DESKTOP_ACCOUNT_ID_PREFIX) }

func baseVendorId(_ id: String) -> String {
    if isDesktopAccountId(id) { return "anthropic" }
    guard let separator = id.firstIndex(of: "@") else { return id }
    return String(id[..<separator])
}

/// The subprocess arguments that select `id` (base vendor or named account).
func vendorArgs(for id: String) -> [String] {
    if let label = accountLabel(of: id) {
        var args = ["--vendor", baseVendorId(id), "--account", label]
        if isDesktopAccountId(id) { args.append("--desktop") }
        return args
    }
    return ["--vendor", id]
}

/// One selectable entry: a base vendor or a named account.
struct MenuEntry {
    let id: String    // "cursor", "anthropic@<label>", or "openrouter@<label>"
    let name: String  // display: "Cursor" or "Vendor · <label>"
}

func claudeAccountMenuEntries(_ accounts: [UsageAccount]) -> [MenuEntry] {
    accounts.map { account in
        let prefix = account.desktop ? DESKTOP_ACCOUNT_ID_PREFIX : CLAUDE_ACCOUNT_ID_PREFIX
        return MenuEntry(id: prefix + account.label, name: "Claude · \(account.label)")
    }
}

func openRouterAccountMenuEntries(_ labels: [String]) -> [MenuEntry] {
    labels.map {
        MenuEntry(id: OPENROUTER_ACCOUNT_ID_PREFIX + $0, name: "OpenRouter · \($0)")
    }
}

/// Apply `[ui] overview_vendors` with the same semantics as the TUI: preserve
/// config order, omit unavailable vendors, and include every named account
/// when its base vendor is requested.
func filterOverviewEntries(_ entries: [MenuEntry], requested: [String]?) -> [MenuEntry] {
    guard let requested else { return entries }
    var result: [MenuEntry] = []
    var seen = Set<String>()
    for vendor in requested {
        for entry in entries where baseVendorId(entry.id) == vendor && !seen.contains(entry.id) {
            result.append(entry)
            seen.insert(entry.id)
        }
    }
    return result
}

/// The selectable entries, in menu order: every enabled+configured vendor,
/// with Claude and OpenRouter expanded into named accounts (their default
/// entries kept only per `show_default_account`). `active` stays listed even when
/// unconfigured — same rule the per-vendor list always had.
func vendorEntries(active: String, usageAccounts: [UsageAccount]? = nil) -> [MenuEntry] {
    var out: [MenuEntry] = []
    for v in VENDOR_AUTH where vendorEnabled(v) {
        if v.id == "anthropic" {
            // Once account status is available, Rust owns profile-directory
            // resolution and CLI/Desktop dedup. The nil fallback preserves
            // configured CLI accounts with an older binary.
            let accounts = usageAccounts ?? claudeAccountLabels().map {
                UsageAccount(label: $0, desktop: false)
            }
            let showDefault = showDefaultAccount(
                configValue: configValueTOML("anthropic", "show_default_account"),
                hasAccounts: !accounts.isEmpty)
            if showDefault && (v.id == active || vendorConfigured(v)) {
                out.append(MenuEntry(id: v.id, name: v.name))
            }
            out.append(contentsOf: claudeAccountMenuEntries(accounts))
        } else if v.id == "openrouter" {
            let labels = openRouterAccountLabels()
            let showDefault = showDefaultAccount(
                configValue: configValueTOML("openrouter", "show_default_account"),
                hasAccounts: !labels.isEmpty)
            if showDefault && (v.id == active || vendorConfigured(v)) {
                out.append(MenuEntry(id: v.id, name: v.name))
            }
            out.append(contentsOf: openRouterAccountMenuEntries(labels))
        } else if v.id == active || vendorConfigured(v) {
            out.append(MenuEntry(id: v.id, name: v.name))
        }
    }
    return out
}

/// Display name for any selectable id (base vendor, account, or overview).
func entryDisplayName(_ id: String) -> String {
    if id == "overview" { return "Visão geral" }
    if let label = accountLabel(of: id) {
        let base = baseVendorId(id)
        let vendor = VENDOR_AUTH.first { $0.id == base }?.name ?? base
        return "\(vendor) · \(label)"
    }
    return VENDOR_AUTH.first { $0.id == id }?.name ?? id
}

// MARK: - Which account each surface is signed in as
//
// Separate from the vendor entries above: those decide whose usage is *shown*,
// these are who the Claude Desktop app and the `claude` CLI are actually signed
// in as. Both come from `ai-usagebar account status --json`; the binary owns
// every path and credential detail, so this side only parses and displays.

struct AccountStatus: Equatable {
    /// False when there is no Claude Desktop app on this machine. Distinct from
    /// "no accounts saved yet": with the app present but nothing captured, the
    /// submenu still has to appear so the user can add the first one.
    var desktopAvailable = false
    var desktopActive: String?
    var cliActive: String?
    var desktopLabels: [String] = []
    var cliLabels: [String] = []
    /// Canonical Rust/TUI usage enumeration. Nil means an older binary, so the
    /// menu falls back to configured CLI labels; an empty array is authoritative.
    var usageAccounts: [UsageAccount]?
    /// Routines one account deleted that another still holds. A switch would
    /// hand them back, so the user is asked before it starts.
    var deletionConflicts: [DeletionConflict] = []
}

struct UsageAccount: Equatable {
    var label: String
    var desktop: Bool
}

struct DeletionConflict: Equatable {
    /// Opaque, type-scoped value accepted by `--delete-conflict`.
    var key: String
    var id: String
    /// "routine" or "chat" — they are swept differently but answered together.
    var kind: String
    var summary: String
    var deletedBy: String
    var stillIn: [String]

    /// One line for the dialog: what it is, and who dropped it.
    var line: String { "[\(kind)] \(summary) — deleted in \(deletedBy)" }
}

/// Parse `account status --json`. Every field is optional on purpose: a Mac
/// with no Claude Desktop app reports `"desktop": null`, and an older binary
/// may not know the subcommand at all — both must degrade to "no submenus"
/// rather than crash.
func parseAccountStatus(_ data: Data) -> AccountStatus? {
    guard let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        return nil
    }
    func labels(_ side: Any?, _ key: String) -> [String] {
        guard let side = side as? [String: Any], let rows = side[key] as? [[String: Any]] else {
            return []
        }
        return rows.compactMap { $0["label"] as? String }
    }
    func active(_ side: Any?) -> String? {
        (side as? [String: Any])?["active_label"] as? String
    }
    let desktop = root["desktop"], cli = root["cli"]
    let usageAccounts: [UsageAccount]?
    if let rows = root["usage_accounts"] as? [[String: Any]] {
        usageAccounts = rows.compactMap { row in
            guard let label = row["label"] as? String,
                  let desktop = row["desktop"] as? Bool else { return nil }
            return UsageAccount(label: label, desktop: desktop)
        }
    } else {
        usageAccounts = nil
    }
    let conflicts = ((desktop as? [String: Any])?["deletion_conflicts"] as? [[String: Any]] ?? [])
        .compactMap { row -> DeletionConflict? in
            guard let id = row["id"] as? String else { return nil }
            let kind = row["kind"] as? String ?? "item"
            guard let key = row["key"] as? String else { return nil }
            return DeletionConflict(key: key,
                                   id: id,
                                   kind: kind,
                                   summary: row["summary"] as? String ?? id,
                                   deletedBy: row["deleted_by"] as? String ?? "?",
                                   stillIn: row["still_in"] as? [String] ?? [])
        }
    return AccountStatus(desktopAvailable: desktop is [String: Any],
                         desktopActive: active(desktop),
                         cliActive: active(cli),
                         desktopLabels: labels(desktop, "profiles"),
                         cliLabels: labels(cli, "accounts"),
                         usageAccounts: usageAccounts,
                         deletionConflicts: conflicts)
}

/// The one dim line under the header. Empty when neither surface resolved, so
/// the caller can hide the row entirely rather than show a bare label.
func accountsSummaryLine(_ s: AccountStatus) -> String {
    var parts: [String] = []
    if !s.desktopLabels.isEmpty { parts.append("Desktop: \(s.desktopActive ?? "?")") }
    if !s.cliLabels.isEmpty { parts.append("Code: \(s.cliActive ?? "?")") }
    return parts.joined(separator: "   ·   ")
}

// ─── Sync state (6-01) ────────────────────────────────────────────────────
//
// D-01: this side parses and displays, nothing more. The crypto, the transport
// and the index all live in Rust, and the only thing reached from here is
// `ai-usagebar sync status --json` — a read that uploads nothing, writes
// nothing, and (unlike the human rendering) builds no plan, so it never wants
// a password on a stdin a subprocess cannot answer.
//
// There is deliberately no action on this row. `sync push` and `sync pull`
// carry confirmations this surface cannot ask for, so they stay in 6-02 behind
// their own flow; a row that can be clicked before that exists is a row that
// can push by accident.

struct SyncCategoryLine: Equatable {
    var category: String
    var enabled: Bool
    var files: Int
    var bytes: Int
}

struct SyncStatus: Equatable {
    var lastSync: Date?
    /// Three states, exactly as the binary reports them: pending, not pending,
    /// and **unknown** — the local index would not open. Flattening nil into
    /// false would draw a stale backup as an up-to-date one, which is the one
    /// thing surfacing sync state exists to prevent (D-04).
    var pending: Bool?
    var pendingFiles: Int?
    var pendingBytes: Int?
    /// The frozen contract, parsed so 6-02 does not have to re-derive it when
    /// it attaches the push/pull actions. Nothing on this surface renders them
    /// yet, and nothing here reaches an NSMenuItem.
    var categories: [SyncCategoryLine] = []
    var warnings: [String] = []
}

/// `chrono`'s `to_rfc3339` emits up to nanosecond precision, which
/// `ISO8601DateFormatter`'s fractional-seconds mode does not accept. A row that
/// reads "há 2 h" has no use for the fraction, so it is dropped and the rest
/// parsed with one formatter rather than guessed at with two.
private let ISO_SYNC: ISO8601DateFormatter = {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime]
    return f
}()

private let RELATIVE_SYNC: RelativeDateTimeFormatter = {
    let f = RelativeDateTimeFormatter()
    f.unitsStyle = .abbreviated
    return f
}()

private let BYTES_SYNC: ByteCountFormatter = {
    let f = ByteCountFormatter()
    f.countStyle = .binary
    f.allowedUnits = [.useKB, .useMB, .useGB]
    return f
}()

/// "3 pendentes (99 MB)" when the binary said how much is waiting, plain
/// "pendente" when it only said that something is. How much is not backed up
/// is the half of D-04 a bare marker leaves out.
private func pendingLabel(_ s: SyncStatus) -> String {
    guard let files = s.pendingFiles, files > 0 else { return "pendente" }
    let noun = files == 1 ? "pendente" : "pendentes"
    guard let bytes = s.pendingBytes, bytes > 0 else { return "\(files) \(noun)" }
    return "\(files) \(noun) (\(BYTES_SYNC.string(fromByteCount: Int64(bytes))))"
}

func parseSyncDate(_ s: String) -> Date? {
    ISO_SYNC.date(from: s.replacingOccurrences(of: #"\.\d+"#, with: "",
                                               options: .regularExpression))
}

/// Parse `sync status --json`, built exactly like `parseAccountStatus`: every
/// field read through an optional cast with a default, a non-object rejected.
/// An older binary that does not know `--json` prints an error and nothing
/// usable, and that must hide the row rather than break the menu.
func parseSyncStatus(_ data: Data) -> SyncStatus? {
    guard let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
        return nil
    }
    let categories = (root["categories"] as? [[String: Any]] ?? [])
        .compactMap { row -> SyncCategoryLine? in
            guard let category = row["category"] as? String else { return nil }
            return SyncCategoryLine(category: category,
                                    enabled: row["enabled"] as? Bool ?? false,
                                    files: row["files"] as? Int ?? 0,
                                    bytes: row["bytes"] as? Int ?? 0)
        }
    // An unknown key is ignored rather than rejected, which is what lets a
    // later phase add one without breaking a menu bar already shipped.
    return SyncStatus(lastSync: (root["last_sync"] as? String).flatMap(parseSyncDate),
                      pending: root["pending"] as? Bool,
                      pendingFiles: root["pending_files"] as? Int,
                      pendingBytes: root["pending_bytes"] as? Int,
                      categories: categories,
                      warnings: root["warnings"] as? [String] ?? [])
}

/// The one dim row: when sync last ran, and whether anything local is not in it
/// yet. Pure, and "" for a nil status so the caller can hide the row entirely
/// rather than show a bare label.
func syncSummaryLine(_ status: SyncStatus?, now: Date = Date()) -> String {
    guard let s = status else { return "" }
    let when = s.lastSync.map { RELATIVE_SYNC.localizedString(for: $0, relativeTo: now) }
    var parts = ["Sync: " + (when ?? "nunca")]
    switch s.pending {
    case .some(true): parts.append(pendingLabel(s))
    case .some(false): break
    // Unknown is not "up to date". The binary says why in `warnings`; that text
    // arrives from a subprocess, so it is stripped before display.
    case .none: parts.append(stripMarkup(s.warnings.first ?? "estado desconhecido"))
    }
    return parts.joined(separator: "   ·   ")
}

// MARK: - The two sync actions the dropdown offers
//
// **Neither of them runs here, and that is the design.** `sync push` publishes
// ciphertext that cannot be un-published; `sync pull --apply` writes over local
// credentials. Both derive their key from a sync password that arrives on stdin
// and nowhere else — never argv, never the environment, never `config.toml` —
// and both of `sync pull`'s confirmations are offered *only* when stdin is a
// terminal. Piped, they are not asked at all: they are answered by `--yes` and
// `--force-credentials`.
//
// So a background subprocess launched from a menu click could reach a restore
// only by passing the flags that skip the questions — a shorter path to an
// irreversible write than the CLI itself offers. Instead the menu confirms, and
// then hands the command to Terminal.app and steps out of the way, exactly as
// "Adicionar conta…" already does for a sign-in it cannot perform. The password
// is typed where the CLI asks for it, and every gate happens where it was
// designed to (D-01, D-02).

enum SyncAction: CaseIterable {
    case push, pull
}

/// Single-quoted with `'` doubled out, so a path with a quote in it cannot
/// break out of the generated command.
func shellQuote(_ s: String) -> String {
    "'" + s.replacingOccurrences(of: "'", with: "'\\''") + "'"
}

/// The argument vector each action names — the only place it is spelled.
///
/// `--apply`, `--yes`, `--force` and `--force-credentials` are the four
/// spellings that turn `sync pull` from a report into a write, and none of them
/// belongs in a string a menu click produces: what is handed over is always the
/// form that stops and asks. `testSyncActions` asserts that, and asserts no
/// password option is spelled here either.
func syncCommand(_ action: SyncAction) -> [String] {
    switch action {
    case .push: return ["sync", "push"]
    case .pull: return ["sync", "pull"]
    }
}

/// The command as the user will see it in the confirmation and as the script
/// will run it — one spelling, so the line they approved is the line that runs.
/// Only the binary is quoted; the rest are fixed literals with no metacharacter
/// in them, and quoting those would make the dialog harder to read for nothing.
func syncCommandLine(binary: String, _ action: SyncAction) -> String {
    ([shellQuote(binary)] + syncCommand(action)).joined(separator: " ")
}

/// The script handed to `runInTerminal`, in the shape `addAccountScript`
/// established: run it, then hold the window open so a refusal can be read.
func syncTerminalScript(binary: String, _ action: SyncAction) -> String {
    "#!/usr/bin/env bash\n"
        + syncCommandLine(binary: binary, action) + "\n"
        + "echo; read -n 1 -s -r -p 'Pressione qualquer tecla para fechar…'\n"
}

struct SyncPrompt: Equatable {
    var title: String
    var body: String
    var go: String
}

/// What the confirmation says. Each one states the consequence the CLI's own
/// gate states, because handing the operation to a terminal is not a reason to
/// hand it over silently.
func syncPrompt(_ action: SyncAction) -> SyncPrompt {
    switch action {
    case .push:
        return SyncPrompt(
            title: "Enviar este backup agora?",
            body: "Uma publicação não se desfaz: os bytes cifrados vão para o repositório "
                + "remoto e podem já ter sido copiados de lá antes de você mudar de ideia.\n\n"
                + "O comando abre no Terminal, que é onde a senha do sync é pedida — ela "
                + "nunca passa por este menu.",
            go: "Abrir no Terminal")
    case .pull:
        return SyncPrompt(
            title: "Restaurar esta máquina do backup?",
            body: "O comando abre no Terminal e, sozinho, apenas mostra o que mudaria — "
                + "nada é escrito. A restauração só acontece quando você repete o comando "
                + "com --apply ali mesmo, e cada credencial a ser sobrescrita é confirmada "
                + "nesse terminal.\n\n"
                + "Este menu não tem como fazer essas perguntas, então não as pula.",
            go: "Abrir no Terminal")
    }
}

/// `SyncCategory::label()` is a machine key (`chat_index`), so the menu names
/// it. Five fixed rows, not a provider table: an unrecognised category still
/// renders, under the label the binary sent, stripped because it crossed a
/// process boundary.
private let SYNC_CATEGORY_NAMES = [
    "config": "Configuração",
    "credentials": "Credenciais",
    "routines": "Rotinas",
    "chat_index": "Índice de conversas",
    "transcripts": "Transcrições",
]

/// What is actually in this backup, one row per category — the half of D-04 the
/// one-line summary has no room for. A category that is switched off says so
/// rather than being left out, because "credentials: desativado" is exactly the
/// thing a user needs to discover before the machine they are backing up dies.
func syncCategoryRows(_ status: SyncStatus?) -> [String] {
    guard let s = status else { return [] }
    return s.categories.map { line in
        let name = SYNC_CATEGORY_NAMES[line.category] ?? stripMarkup(line.category)
        guard line.enabled else { return "\(name): desativado" }
        guard line.files > 0 else { return "\(name): vazio" }
        let noun = line.files == 1 ? "arquivo" : "arquivos"
        let size = BYTES_SYNC.string(fromByteCount: Int64(line.bytes))
        return "\(name): \(line.files) \(noun) (\(size))"
    }
}

/// `-y` because the menu has already asked; without it the binary would prompt
/// on a stdin that is not a terminal and abort.
func switchArgs(label: String, desktop: Bool, deleting: [String] = []) -> [String] {
    var args = ["account", "switch", label, desktop ? "--desktop" : "--cli", "-y"]
    // Passing any --delete-conflict also tells the binary the question was
    // already answered, so it never blocks on a prompt it cannot show here.
    for id in deleting { args += ["--delete-conflict", id] }
    return args
}

/// The first few conflicts spelled out, the rest summarised — a switch after a
/// big clean-up must not grow a dialog taller than the screen.
func conflictPreview(_ conflicts: [DeletionConflict], limit: Int = 10) -> String {
    var lines = conflicts.prefix(limit).map { "• \($0.line)" }
    if conflicts.count > limit {
        lines.append("… e mais \(conflicts.count - limit)")
    }
    return lines.joined(separator: "\n")
}

/// Adding an account is interactive — a Desktop capture waits for you to sign
/// in, and a CLI one runs `claude` — so it goes to Terminal rather than being
/// swallowed by a background subprocess. Quoted through `shellQuote`, which the
/// sync scripts share, so a label with a quote in it can't break out.
func addAccountScript(binary: String, label: String, desktop: Bool) -> String {
    return "#!/usr/bin/env bash\n"
        + "\(shellQuote(binary)) account add \(shellQuote(label))\(desktop ? " --desktop" : "")\n"
        + "echo; read -n 1 -s -r -p 'Pressione qualquer tecla para fechar…'\n"
}

/// Rust defaults (`src/config.rs`): the OAuth/api-key vendors that ship enabled,
/// versus the opt-in balance vendors that default to disabled. An omitted
/// `[vendor].enabled` must reproduce these, not silently enable everything.
func defaultEnabled(_ id: String) -> Bool {
    switch id {
    case "anthropic", "openai", "zai", "openrouter": return true
    case "deepseek", "kimi", "kilo", "novita", "moonshot", "grok", "anthropic_api", "cursor", "antigravity": return false
    default: return true
    }
}

func vendorEnabled(_ v: VendorAuth) -> Bool {
    if let explicit = configEnabledTOML(v.id) { return explicit }
    return defaultEnabled(v.id)
}

// Cached: the check spawns a `security` subprocess, and it is consulted from the
// menu-rebuild path (main thread). Signed-in state doesn't change within a run,
// so compute it at most once — a restart picks up a fresh login.
var keychainClaudeCache: Bool?
func keychainHasClaude() -> Bool {
    if let cached = keychainClaudeCache { return cached }
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/security")
    p.arguments = ["find-generic-password", "-s", "Claude Code-credentials"]
    p.standardOutput = FileHandle.nullDevice
    p.standardError = FileHandle.nullDevice
    let result: Bool
    do { try p.run(); p.waitUntilExit(); result = p.terminationStatus == 0 } catch { result = false }
    keychainClaudeCache = result
    return result
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
    if v.id == "cursor" {
        // Configured == signed in to the Cursor IDE, i.e. its state DB exists.
        // Honor a [cursor] db_path override the same way the binary does.
        let dbPath = configValueTOML("cursor", "db_path")
            ?? "\(home)/Library/Application Support/Cursor/User/globalStorage/state.vscdb"
        return fm.fileExists(atPath: dbPath)
    }
    if v.id == "antigravity" {
        // Same check as gnome-extension/prefs.js: having any of the three
        // products' state directories is enough — there is no credential
        // file, and the binary itself probes whichever local server answers.
        return ["antigravity", "antigravity-cli", "antigravity-ide"]
            .contains { d in
                var isDir: ObjCBool = false
                return fm.fileExists(atPath: "\(home)/.gemini/\(d)", isDirectory: &isDir) && isDir.boolValue
            }
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
    let tmp = NSTemporaryDirectory() + "ai-usagebar-\(UUID().uuidString).sh"
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

// Launch a .app by name (e.g. "Cursor") via `open -a`, so a local-kind vendor's
// button can bring the user to the app they need to sign into.
func openApp(_ name: String) {
    let p = Process()
    p.executableURL = URL(fileURLWithPath: "/usr/bin/open")
    p.arguments = ["-a", name]
    try? p.run()
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
        // Local vendors have no key: "configured" means signed in to the app
        // AND the vendor's own section enabled in config.
        if v.id == "antigravity" {
            return "⚠ Abra o Antigravity (app, IDE ou agy) e ative [antigravity] no config"
        }
        if v.kind == "local" {
            return "⚠ Entre no app Cursor e ative [cursor] no config"
        }
        return "⚠ Sem API key — \(apiKeyEnvironment(v))"
    }

    private func buttonLabel(_ v: VendorAuth) -> String {
        if v.kind == "oauth" {
            if configured[v.id] == true { return "Re-logar" }
            if cliPresent[v.id] == false { return "Instalar + logar" }
            return "Logar"
        }
        if v.id == "antigravity" { return "Abrir Antigravity" }
        if v.kind == "local" { return "Abrir Cursor" }
        return "Configurar (TUI)"
    }

    private func action(_ v: VendorAuth) {
        if v.kind == "oauth" { runInTerminal(oauthScript(v)) }
        else if v.id == "antigravity" { openApp("Antigravity") }
        else if v.kind == "local" { openApp(v.name) }
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
    @AppStorage("swapShortcutEnabled") private var swapShortcutEnabled = true
    @AppStorage("compactShortcutEnabled") private var compactShortcutEnabled = true
    @AppStorage("colorLow") private var colorLow = "#98c379"
    @AppStorage("colorMid") private var colorMid = "#e5c07b"
    @AppStorage("colorHigh") private var colorHigh = "#d19a66"
    @AppStorage("colorCritical") private var colorCritical = "#e06c75"
    @AppStorage("colorEmpty") private var colorEmpty = "#3e4451"
    @AppStorage("binaryPath") private var binaryPath = ""
    @State private var launchAtLogin = launchAgentIsInstalled()
    @State private var launchAtLoginError: String?

    // Only enabled vendors appear in the selector: Rust treats opt-in vendors
    // (deepseek/kimi/kilo/novita/moonshot/grok/anthropic_api) as disabled when
    // their `[vendor].enabled` is omitted, and so must this picker. Claude
    // accounts appear as their `vendor@<label>` pseudo-ids, same as the
    // "Trocar vendor" submenu.
    private var vendors: [String] {
        var ids = VENDOR_AUTH.filter { vendorEnabled($0) }.map { $0.id }
        let labels = claudeAccountLabels()
        if let at = ids.firstIndex(of: "anthropic") {
            ids.insert(contentsOf: labels.map { CLAUDE_ACCOUNT_ID_PREFIX + $0 }, at: at + 1)
        }
        let openRouterLabels = openRouterAccountLabels()
        if let at = ids.firstIndex(of: "openrouter") {
            ids.insert(
                contentsOf: openRouterLabels.map { OPENROUTER_ACCOUNT_ID_PREFIX + $0 },
                at: at + 1)
        }
        return ids
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
                GroupBox("Atalho") {
                    VStack(alignment: .leading, spacing: 4) {
                        Toggle("Trocar vendor com ⌥⌘\\ (atalho global)", isOn: $swapShortcutEnabled)
                        Text("Alterna o vendor ativo a partir de qualquer app.")
                            .font(.caption).foregroundColor(.secondary)
                        Toggle("Compactar/Expandir com ⌥⌘E (atalho global)", isOn: $compactShortcutEnabled)
                        Text("Alterna a Visão geral entre barras e modo compacto.")
                            .font(.caption).foregroundColor(.secondary)
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
                GroupBox("Sistema") {
                    VStack(alignment: .leading, spacing: 4) {
                        Toggle("Iniciar no login", isOn: Binding(
                            get: { launchAtLogin },
                            set: { enabled in
                                do {
                                    try setLaunchAtLogin(enabled)
                                    launchAtLogin = launchAgentIsInstalled()
                                    launchAtLoginError = nil
                                } catch {
                                    launchAtLogin = launchAgentIsInstalled()
                                    launchAtLoginError = error.localizedDescription
                                }
                            }))
                        Text("Instala um LaunchAgent que sobe o app ao entrar na sua conta.")
                            .font(.caption).foregroundColor(.secondary)
                        if let error = launchAtLoginError {
                            Text("Não foi possível atualizar: \(error)")
                                .font(.caption).foregroundColor(.red)
                        }
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
        .onAppear { launchAtLogin = launchAgentIsInstalled() }
    }
}

// ─── Launch at login (macOS LaunchAgent) ─────────────────────────────────
//
// The app is an unbundled single binary, so the modern SMAppService.mainApp
// API (which needs a bundle id) doesn't apply. A per-user LaunchAgent is the
// portable way to start an unbundled executable at login — the same mechanism
// install-agent.sh sets up, now toggleable from Preferences.
let LAUNCH_AGENT_LABEL = "com.akitaonrails.ai-usagebar-menubar"

func launchAgentPlistPath() -> String {
    "\(NSHomeDirectory())/Library/LaunchAgents/\(LAUNCH_AGENT_LABEL).plist"
}

func launchAgentIsInstalled() -> Bool {
    FileManager.default.fileExists(atPath: launchAgentPlistPath())
}

/// Absolute path of the running executable, for the LaunchAgent to relaunch.
func selfExecutablePath() -> String {
    if let p = Bundle.main.executablePath, !p.isEmpty { return p }
    let arg0 = CommandLine.arguments.first ?? "ai-usagebar-menubar"
    if arg0.hasPrefix("/") { return arg0 }
    return URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        .appendingPathComponent(arg0).standardized.path
}

/// Install (or remove) the login LaunchAgent by managing its plist file.
/// Building the plist via PropertyListSerialization sidesteps the XML-escaping
/// the shell installer has to do by hand.
///
/// Deliberately no `launchctl load`/`unload`: the app is already running when
/// this is toggled, so `RunAtLoad` on load would spawn a *second* copy, and
/// `unload` on disable would SIGTERM this very process (quitting the app the
/// user is still using) if it happened to be the launchd-managed one. launchd
/// loads every agent in ~/Library/LaunchAgents at the next login, so writing
/// (or removing) the file is all that's needed for a start-at-login toggle.
func launchAgentPlist(executable: String) throws -> Data {
    let plist: [String: Any] = [
        "Label": LAUNCH_AGENT_LABEL,
        "ProgramArguments": [executable],
        "RunAtLoad": true,
        "ProcessType": "Interactive",
    ]
    return try PropertyListSerialization.data(
        fromPropertyList: plist, format: .xml, options: 0)
}

func setLaunchAtLogin(_ enabled: Bool) throws {
    let path = launchAgentPlistPath()
    if enabled {
        let dir = (path as NSString).deletingLastPathComponent
        try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        let data = try launchAgentPlist(executable: selfExecutablePath())
        try data.write(to: URL(fileURLWithPath: path), options: .atomic)
    } else if FileManager.default.fileExists(atPath: path) {
        try FileManager.default.removeItem(atPath: path)
    }
}

// ─── App ─────────────────────────────────────────────────────────────────
// ─── Global vendor-swap shortcut (⌥⌘\) ───────────────────────────────────
//
// A menu-bar app has no focused window, so a system-wide shortcut needs a
// Carbon hot key (RegisterEventHotKey), which fires regardless of focus and —
// unlike an NSEvent global monitor — needs no Accessibility permission. The
// Carbon handler is a C callback that can't capture Swift state, so it just
// posts a notification the delegate observes.

let swapHotKeyNotification = Notification.Name("aiusagebar.swapHotKey")
let compactHotKeyNotification = Notification.Name("aiusagebar.compactHotKey")

/// Default shortcuts: `\` (kVK_ANSI_Backslash) with Command+Option swaps the
/// vendor; `E` with Command+Option toggles the overview's Compactar/Expandir.
/// `[ui]` on Linux has no equivalent; this is macOS-only.
let SWAP_HOTKEY_KEYCODE = UInt32(kVK_ANSI_Backslash)
let COMPACT_HOTKEY_KEYCODE = UInt32(kVK_ANSI_E)
let SWAP_HOTKEY_MODIFIERS = UInt32(cmdKey | optionKey)

/// Registered hot-key ids, carried back to us in the event so one shared C
/// callback can tell which shortcut fired.
let SWAP_HOTKEY_ID = UInt32(1)
let COMPACT_HOTKEY_ID = UInt32(2)

private func swapHotKeyCallback(
    _ next: EventHandlerCallRef?, _ event: EventRef?, _ userData: UnsafeMutableRawPointer?
) -> OSStatus {
    var hk = EventHotKeyID()
    GetEventParameter(event, EventParamName(kEventParamDirectObject),
                      EventParamType(typeEventHotKeyID), nil,
                      MemoryLayout<EventHotKeyID>.size, nil, &hk)
    NotificationCenter.default.post(
        name: hk.id == COMPACT_HOTKEY_ID ? compactHotKeyNotification : swapHotKeyNotification,
        object: nil)
    return noErr
}

/// Whether the overview status-bar title draws mini bars (vs. the compact
/// %-text mode). "Compactar" forces the text mode even under the bars-count
/// threshold. Pure + testable — the render path is not.
func overviewUsesBars(count: Int, barsMax: Int, compact: Bool) -> Bool {
    !compact && count <= barsMax
}

/// Provider ids the user toggled out of the always-visible top-bar summary in
/// Overview mode. The dropdown still lists them (unchecked + dimmed) so they can
/// be turned back on. Persisted in UserDefaults; absent key → nothing hidden.
func overviewHiddenProviders() -> Set<String> {
    Set(DEF.stringArray(forKey: "overviewHiddenProviders") ?? [])
}

func setOverviewProvider(_ id: String, hidden: Bool) {
    var hiddenSet = overviewHiddenProviders()
    if hidden { hiddenSet.insert(id) } else { hiddenSet.remove(id) }
    DEF.set(hiddenSet.sorted(), forKey: "overviewHiddenProviders")
}

/// The ids that survive the top-bar filter, in their original order. Pure +
/// testable — the render path (which also drops nil snapshots) is not.
func overviewVisibleIds(_ ids: [String], hidden: Set<String>) -> [String] {
    ids.filter { !hidden.contains($0) }
}

/// The next id in the cycle, wrapping. `current` absent from `ids` (e.g. the
/// selected vendor was disabled) starts at the first (or last, backward).
/// Pure + testable — the hot-key handler is not.
func nextVendorId(current: String, in ids: [String], forward: Bool = true) -> String? {
    guard !ids.isEmpty else { return nil }
    let n = ids.count
    let i = ids.firstIndex(of: current) ?? (forward ? -1 : 0)
    let j = forward ? (i + 1) % n : (i - 1 + n) % n
    return ids[j]
}

func hotKeyRegistrationSucceeded(_ status: OSStatus) -> Bool {
    status == noErr
}

class AppDelegate: NSObject, NSApplicationDelegate, NSMenuDelegate {
    var statusItem: NSStatusItem!
    var swapHotKeyRef: EventHotKeyRef?
    var compactHotKeyRef: EventHotKeyRef?
    var swapHotKeyHandlerInstalled = false
    var timer: Timer?
    var prefsWindow: NSWindow?
    var appearanceObservation: NSKeyValueObservation?
    /// Last resolved light/dark name, so the appearance observer ignores the
    /// layout-driven KVO fires that don't actually change the theme.
    var lastAppearanceName: NSAppearance.Name?
    /// Tracks the active vendor so `settingsChanged` can tell a vendor swap (show
    /// Loading) apart from an unrelated preference change (repaint from cache).
    var lastVendor = ""
    var lastSnapshot: Snapshot?
    var pendingRefresh: DispatchWorkItem?
    /// Live watch on config.toml so external edits (a text editor, the TUI's
    /// Settings overlay, `ai-usagebar account add`) hot-reload
    /// the vendor list without restarting the app. The fd is closed by the
    /// source's cancel handler; `pendingConfigReload` coalesces an editor's
    /// burst of vnode events into a single reload.
    var configWatchSource: DispatchSourceFileSystemObject?
    var configWatchFD: CInt = -1
    var pendingConfigReload: DispatchWorkItem?
    /// A visibility checkbox changes presentation only. Its UserDefaults
    /// notification should repaint from cache without restarting the timer or
    /// launching a fresh round of provider subprocesses.
    var overviewVisibilityChangePending = false
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
    // Overview mode fills these (one per vendor/account); hidden in
    // single-vendor mode. The pool starts at the built-in vendor count and can
    // grow if account discovery adds more rows.
    var overviewRows: [NSMenuItem] = []
    /// Last overview fetch, so a settings/appearance change re-renders it without
    /// flashing the stale single-vendor snapshot.
    var lastOverview: [(name: String, id: String, snap: Snapshot?)]?
    // Rebuilt on every render so only configured vendors show, and the active
    // one is checked. Kept as a field so the menu owns it for its lifetime.
    let vendorSubmenu = NSMenu()
    let vendorSubmenuItem = NSMenuItem(title: "Trocar vendor", action: nil, keyEquivalent: "")
    /// Overview-only: forces the status-bar title into the compact %-text mode
    /// ("Compactar"); while compact it reads "Expandir" and turns it back off.
    let compactItem = NSMenuItem(title: "Compactar", action: nil, keyEquivalent: "")
    /// Which account each surface is signed in as. One dim line under the
    /// header, plus a submenu per surface. All three stay hidden until
    /// `account status --json` answers, so an older binary that doesn't know
    /// the subcommand simply shows the menu it always did.
    let accountsInfoItem = NSMenuItem()
    let desktopAccountSubmenu = NSMenu()
    let desktopAccountItem = NSMenuItem(title: "Claude Desktop", action: nil, keyEquivalent: "")
    let cliAccountSubmenu = NSMenu()
    let cliAccountItem = NSMenuItem(title: "Claude Code", action: nil, keyEquivalent: "")
    var lastAccountStatus: AccountStatus?
    var accountStatusFetchedAt = Date.distantPast
    var accountStatusGeneration = 0
    /// When sync last ran and whether anything local is not backed up yet. One
    /// dim row, disabled and hidden until `sync status --json` answers, so a
    /// binary that predates the flag shows exactly the menu it always did.
    let syncInfoItem = NSMenuItem()
    /// What is in the backup, and the two actions the menu hands to a terminal.
    /// Hidden on the same condition as the row above.
    let syncSubmenu = NSMenu()
    let syncMenuItem = NSMenuItem(title: "Sync", action: nil, keyEquivalent: "")
    var lastSyncStatus: SyncStatus?
    var syncStatusFetchedAt = Date.distantPast
    var syncStatusGeneration = 0
    /// A switch runs a subprocess that quits and reopens another app; both
    /// submenus grey out until it returns so it cannot be fired twice.
    var accountSwitchInFlight = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        DEF.register(defaults: ["swapShortcutEnabled": true, "compactShortcutEnabled": true])
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        statusItem.button?.title = "5h …"
        buildMenu()
        rebuildVendorSubmenu()
        observeAppearanceChanges()
        lastVendor = VENDOR  // so the first settingsChanged isn't mistaken for a swap
        refresh()
        restartTimer()
        NotificationCenter.default.addObserver(
            self, selector: #selector(settingsChanged),
            name: UserDefaults.didChangeNotification, object: nil)
        NotificationCenter.default.addObserver(
            self, selector: #selector(handleSwapHotKey),
            name: swapHotKeyNotification, object: nil)
        NotificationCenter.default.addObserver(
            self, selector: #selector(handleCompactHotKey),
            name: compactHotKeyNotification, object: nil)
        installSwapHotKey()
        installCompactHotKey()
        installConfigWatch()
        fetchAccountStatus()
    }

    /// Watch config.toml and hot-reload the vendor list when it changes on disk.
    /// Editors usually save atomically (write a temp file, then rename it over
    /// the target), which unlinks the inode our descriptor points at — so a
    /// `.delete`/`.rename` event means "re-open the path to keep watching the
    /// new file". A plain in-place write (`>>`, some editors) fires `.write` on
    /// the same descriptor. Idempotent: always tears down the previous watch.
    func installConfigWatch() {
        removeConfigWatch()
        let path = configPathTOML()
        let fd = open(path, O_EVTONLY)
        guard fd >= 0 else {
            // No readable file yet (not created, or momentarily gone between an
            // editor's unlink and rename). Retry shortly so a config created
            // after launch — or a non-atomic save's brief gap — is still picked
            // up. ponytail: a fixed 3s retry, not exponential backoff; this only
            // spins in the rare window before the file exists.
            DispatchQueue.main.asyncAfter(deadline: .now() + 3) { [weak self] in
                self?.installConfigWatch()
            }
            return
        }
        let src = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: fd, eventMask: [.write, .extend, .delete, .rename, .attrib],
            queue: .main)
        src.setEventHandler { [weak self] in
            guard let self, let flags = self.configWatchSource?.data else { return }
            self.scheduleConfigReload()
            // The watched inode was replaced; re-arm on whatever is at the path
            // now so future edits keep firing.
            if flags.contains(.delete) || flags.contains(.rename) {
                self.installConfigWatch()
            }
        }
        src.setCancelHandler { close(fd) }
        configWatchFD = fd
        configWatchSource = src
        src.resume()
    }

    func removeConfigWatch() {
        configWatchSource?.cancel()  // the cancel handler closes the fd
        configWatchSource = nil
        configWatchFD = -1
    }

    /// Coalesce the burst of vnode events a save emits (write → rename → attrib)
    /// into one reload a beat later, mirroring `settingsChanged`'s debounce.
    func scheduleConfigReload() {
        pendingConfigReload?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.configFileChanged() }
        pendingConfigReload = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3, execute: work)
    }

    /// A config.toml edit landed: rebuild the vendor submenu/ring, re-render the
    /// current view from cache so an `[ui]` tweak shows at once, then debounce a
    /// full refresh to fetch any newly-added vendor/account. Reads are all fresh
    /// from disk (nothing caches config.toml), so no invalidation is needed.
    @objc func configFileChanged() {
        rebuildVendorSubmenu()
        // A new [[anthropic.accounts]] entry changes the Claude Code list.
        fetchAccountStatus()
        if VENDOR == "overview" {
            if let ov = lastOverview { renderOverview(ov) }
        } else if let s = lastSnapshot {
            renderPanel(s); renderMenu(s)
        }
        pendingRefresh?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.refresh() }
        pendingRefresh = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6, execute: work)
    }

    /// (Re)register the global ⌥⌘\ hot key to match the `swapShortcutEnabled`
    /// preference. Idempotent: always unregisters first.
    func installSwapHotKey() {
        guard removeSwapHotKey() else {
            // The old registration is still live; reflect that instead of
            // showing a disabled toggle that continues intercepting the key.
            DEF.set(true, forKey: "swapShortcutEnabled")
            return
        }
        let enabled = DEF.bool(forKey: "swapShortcutEnabled")
        // Mirror the swap shortcut as a hint on the "Trocar vendor" item. A native
        // keyEquivalent is suppressed by the submenu's disclosure arrow, so paint
        // the hint into the title instead (right-aligned, dimmed). Cleared when off.
        if enabled {
            // Inline, dimmed hint right after the label. A right-aligned column
            // (like the other items' ⌘R) isn't possible here: the submenu arrow
            // takes the trailing slot, and a fixed tab stop would over-widen the
            // menu whenever the wide overview rows are hidden.
            let font = NSFont.menuFont(ofSize: 0)
            let s = NSMutableAttributedString(
                string: "Trocar vendor  ", attributes: [.font: font])
            s.append(NSAttributedString(string: "⌥⌘\\", attributes: [
                .font: font, .foregroundColor: NSColor.tertiaryLabelColor,
            ]))
            vendorSubmenuItem.attributedTitle = s
        } else {
            vendorSubmenuItem.attributedTitle = NSAttributedString(string: "Trocar vendor")
        }
        guard enabled else { return }
        guard installHotKeyHandlerOnce() else {
            disableFailedShortcut("swapShortcutEnabled", name: "⌥⌘\\")
            vendorSubmenuItem.attributedTitle = NSAttributedString(string: "Trocar vendor")
            return
        }
        let id = EventHotKeyID(signature: OSType(0x4149_4242), id: SWAP_HOTKEY_ID)  // 'AIBB'
        var ref: EventHotKeyRef?
        let status = RegisterEventHotKey(
            SWAP_HOTKEY_KEYCODE, SWAP_HOTKEY_MODIFIERS, id, GetApplicationEventTarget(), 0,
            &ref)
        guard hotKeyRegistrationSucceeded(status), let ref else {
            if let ref { UnregisterEventHotKey(ref) }
            disableFailedShortcut("swapShortcutEnabled", name: "⌥⌘\\", status: status)
            vendorSubmenuItem.attributedTitle = NSAttributedString(string: "Trocar vendor")
            return
        }
        swapHotKeyRef = ref
    }

    /// (Re)register the global ⌥⌘E Compactar/Expandir hot key to match the
    /// `compactShortcutEnabled` preference. Idempotent, mirroring
    /// `installSwapHotKey` — including the painted-in hint (a native
    /// keyEquivalent would fire a second time while the menu is open, double-
    /// toggling on one press).
    func installCompactHotKey() {
        guard removeCompactHotKey() else {
            DEF.set(true, forKey: "compactShortcutEnabled")
            return
        }
        updateCompactItemTitle()
        guard DEF.bool(forKey: "compactShortcutEnabled") else { return }
        guard installHotKeyHandlerOnce() else {
            disableFailedShortcut("compactShortcutEnabled", name: "⌥⌘E")
            updateCompactItemTitle()
            return
        }
        let id = EventHotKeyID(signature: OSType(0x4149_4242), id: COMPACT_HOTKEY_ID)
        var ref: EventHotKeyRef?
        let status = RegisterEventHotKey(
            COMPACT_HOTKEY_KEYCODE, SWAP_HOTKEY_MODIFIERS, id, GetApplicationEventTarget(), 0,
            &ref)
        guard hotKeyRegistrationSucceeded(status), let ref else {
            if let ref { UnregisterEventHotKey(ref) }
            disableFailedShortcut("compactShortcutEnabled", name: "⌥⌘E", status: status)
            updateCompactItemTitle()
            return
        }
        compactHotKeyRef = ref
    }

    /// Compactar ↔ Expandir label plus the dimmed ⌥⌘E hint when the global
    /// shortcut is on. Shared by the overview render and the hot-key installer.
    func updateCompactItemTitle() {
        let label = DEF.bool(forKey: "overviewCompact") ? "Expandir" : "Compactar"
        guard DEF.bool(forKey: "compactShortcutEnabled") else {
            compactItem.attributedTitle = NSAttributedString(string: label)
            return
        }
        let font = NSFont.menuFont(ofSize: 0)
        let s = NSMutableAttributedString(string: "\(label)  ", attributes: [.font: font])
        s.append(NSAttributedString(string: "⌥⌘E", attributes: [
            .font: font, .foregroundColor: NSColor.tertiaryLabelColor,
        ]))
        compactItem.attributedTitle = s
    }

    /// One Carbon handler serves every hot key; the event's EventHotKeyID
    /// says which one fired.
    private func installHotKeyHandlerOnce() -> Bool {
        guard !swapHotKeyHandlerInstalled else { return true }
        var spec = EventTypeSpec(
            eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let status = InstallEventHandler(
            GetApplicationEventTarget(), swapHotKeyCallback, 1, &spec, nil, nil)
        guard hotKeyRegistrationSucceeded(status) else {
            NSLog("ai-usagebar: global hot-key handler registration failed (OSStatus %d)", status)
            return false
        }
        swapHotKeyHandlerInstalled = true
        return true
    }

    private func disableFailedShortcut(_ key: String, name: String,
                                       status: OSStatus? = nil) {
        if let status {
            NSLog("ai-usagebar: global shortcut %@ registration failed (OSStatus %d)",
                  name, status)
        } else {
            NSLog("ai-usagebar: global shortcut %@ registration failed", name)
        }
        // Keep the visible preference honest when another app already owns the
        // shortcut or Carbon refuses the registration.
        DEF.set(false, forKey: key)
    }

    @discardableResult
    func removeSwapHotKey() -> Bool {
        if let ref = swapHotKeyRef {
            let status = UnregisterEventHotKey(ref)
            guard hotKeyRegistrationSucceeded(status) else {
                NSLog("ai-usagebar: could not unregister ⌥⌘\\ (OSStatus %d)", status)
                return false
            }
            swapHotKeyRef = nil
        }
        return true
    }

    @discardableResult
    private func removeCompactHotKey() -> Bool {
        if let ref = compactHotKeyRef {
            let status = UnregisterEventHotKey(ref)
            guard hotKeyRegistrationSucceeded(status) else {
                NSLog("ai-usagebar: could not unregister ⌥⌘E (OSStatus %d)", status)
                return false
            }
            compactHotKeyRef = nil
        }
        return true
    }

    /// Advance the active vendor to the next configured one, wrapping. Fired by
    /// the global hot key; sets `vendor` in defaults, which drives the usual
    /// re-render via `settingsChanged`.
    @objc func handleSwapHotKey() {
        if let next = nextVendorId(current: VENDOR, in: swapCycleIds()) {
            DEF.set(next, forKey: "vendor")
        }
    }

    /// ⌥⌘E: toggle Compactar/Expandir. Overview-only — flipping a hidden
    /// preference from another view would be invisible and confusing.
    @objc func handleCompactHotKey() {
        guard VENDOR == "overview" else { return }
        toggleCompact()
    }

    /// The swap-shortcut ring: every configured entry (vendors *and* Claude
    /// accounts) plus the synthetic "overview" target, so ⌥⌘\ cycles them all.
    func swapCycleIds() -> [String] {
        var ids = vendorEntries(active: VENDOR,
                                usageAccounts: lastAccountStatus?.usageAccounts).map { $0.id }
        ids.append("overview")
        return ids
    }

    func buildMenu() {
        let menu = NSMenu()
        menu.autoenablesItems = false

        menu.addItem(headerItem)
        // Reads as a sub-line of the header, so it sits above the usage rows.
        accountsInfoItem.isEnabled = false
        accountsInfoItem.isHidden = true
        menu.addItem(accountsInfoItem)
        // Sync state, one line lower. Disabled and actionless on purpose: the
        // push/pull triggers are 6-02's, behind the confirmation this surface
        // cannot ask for.
        syncInfoItem.isEnabled = false
        syncInfoItem.isHidden = true
        menu.addItem(syncInfoItem)
        // One hidden slot per built-in vendor for Overview mode. Named accounts
        // can take the total higher; renderOverview grows the pool on demand.
        for _ in 0..<VENDOR_AUTH.count {
            let it = NSMenuItem()
            it.isHidden = true
            overviewRows.append(it)
            menu.addItem(it)
        }
        for key in ["session", "weekly", "sonnet", "extra"] {
            let it = NSMenuItem()
            rows[key] = it
            menu.addItem(it)
        }
        // First item after the usage rows, Overview-only (hidden elsewhere).
        compactItem.action = #selector(toggleCompact)
        compactItem.target = self
        compactItem.isHidden = true
        menu.addItem(compactItem)

        menu.addItem(.separator())
        addAction(menu, "Atualizar agora", #selector(refreshAction), "r")
        addAction(menu, "Abrir TUI", #selector(openTui), "t")
        vendorSubmenuItem.submenu = vendorSubmenu
        menu.addItem(vendorSubmenuItem)
        for (item, submenu) in [(desktopAccountItem, desktopAccountSubmenu),
                                (cliAccountItem, cliAccountSubmenu)] {
            item.submenu = submenu
            item.isHidden = true
            menu.addItem(item)
        }
        // Beside the account submenus, not beside the dim status row: the row
        // is a sub-line of the header, and every item that *does* something
        // lives down here.
        syncSubmenu.autoenablesItems = false
        syncMenuItem.submenu = syncSubmenu
        syncMenuItem.isHidden = true
        menu.addItem(syncMenuItem)
        addAction(menu, "Preferências…", #selector(openPrefs), ",")
        menu.addItem(.separator())
        addAction(menu, "Sair", #selector(quit), "q")

        // Catches a switch made elsewhere (a terminal, claude-acc) without
        // polling for state that changes at most a few times a day.
        menu.delegate = self
        statusItem.menu = menu
    }

    func menuWillOpen(_ menu: NSMenu) {
        if Date().timeIntervalSince(accountStatusFetchedAt) >= 5 { fetchAccountStatus() }
        if Date().timeIntervalSince(syncStatusFetchedAt) >= 5 { fetchSyncStatus() }
    }

    func addAction(_ menu: NSMenu, _ title: String, _ sel: Selector, _ key: String) {
        let it = NSMenuItem(title: title, action: sel, keyEquivalent: key)
        it.target = self
        menu.addItem(it)
    }

    @objc func refreshAction() { refresh() }
    @objc func quit() { NSApp.terminate(nil) }

    /// Flip the overview's compact mode; the UserDefaults observer re-renders,
    /// which also relabels the item (Compactar ↔ Expandir).
    @objc func toggleCompact() {
        DEF.set(!DEF.bool(forKey: "overviewCompact"), forKey: "overviewCompact")
    }

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
        if overviewVisibilityChangePending {
            overviewVisibilityChangePending = false
            if VENDOR == "overview", let ov = lastOverview { renderOverview(ov) }
            return
        }
        // The swap-shortcut toggle lives in defaults too; re-register only when
        // its state and the live registration disagree (avoids churn on every
        // vendor switch, which itself writes defaults).
        if DEF.bool(forKey: "swapShortcutEnabled") != (swapHotKeyRef != nil) {
            installSwapHotKey()
        }
        if DEF.bool(forKey: "compactShortcutEnabled") != (compactHotKeyRef != nil) {
            installCompactHotKey()
        }
        // A vendor swap re-fetches, which takes a moment; keeping the previous
        // vendor's view up reads as a freeze. Show a Loading placeholder at once.
        let vendorChanged = VENDOR != lastVendor
        lastVendor = VENDOR
        if vendorChanged {
            showLoading()
        } else if VENDOR == "overview" {
            if let ov = lastOverview { renderOverview(ov) }
        } else if let s = lastSnapshot {
            renderPanel(s); renderMenu(s)
        }
        restartTimer()
        pendingRefresh?.cancel()
        let work = DispatchWorkItem { [weak self] in self?.refresh() }
        pendingRefresh = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6, execute: work)
    }

    /// Instant feedback for a vendor swap: replace the whole view with a Loading
    /// placeholder naming the target, until its data arrives.
    func showLoading() {
        lastSnapshot = nil
        lastOverview = nil
        let appearance = statusItem.button?.effectiveAppearance ?? NSApp.effectiveAppearance
        let name = entryDisplayName(VENDOR)
        statusItem.button?.attributedTitle = run("\(name) …", menuBarTextColor(appearance))
        headerItem.attributedTitle = run("\(name) · carregando…", .labelColor,
                                         NSFont.boldSystemFont(ofSize: 13))
        for (_, it) in rows { it.isHidden = true }
        for it in overviewRows { it.isHidden = true }
        compactItem.isHidden = true
        rebuildVendorSubmenu()
    }

    func restartTimer() {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: INTERVAL, repeats: true) { [weak self] _ in
            self?.refresh()
        }
    }

    func observeAppearanceChanges() {
        guard let button = statusItem.button else { return }
        lastAppearanceName = button.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua])
        appearanceObservation = button.observe(\NSStatusBarButton.effectiveAppearance,
                                               options: [.new]) { [weak self] btn, _ in
            // The KVO fires on every layout pass, not only on a real light↔dark
            // flip — and rendering (which relays out the button) would retrigger
            // it, spinning the main thread. Act only when the theme truly changes.
            let name = btn.effectiveAppearance.bestMatch(from: [.aqua, .darkAqua])
            DispatchQueue.main.async {
                guard let self, self.lastAppearanceName != name else { return }
                self.lastAppearanceName = name
                self.rerenderAppearance()
            }
        }
    }

    func rerenderAppearance() {
        // Appearance only changes colors; the configured-vendor set is unchanged,
        // so repaint without rebuilding the (subprocess-touching) submenu.
        if VENDOR == "overview", let ov = lastOverview { renderOverview(ov, rebuildSubmenu: false); return }
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

        if vendor == "overview" {
            refreshOverview(bin: bin, generation: generation)
            return
        }

        DispatchQueue.global(qos: .utility).async { [weak self] in
            let p = Process()
            p.executableURL = URL(fileURLWithPath: bin)
            p.arguments = vendorArgs(for: vendor) + ["--format", FORMAT_WITH_SENTINEL]
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

    /// Overview mode: fetch every configured vendor and render one compact row
    /// each. Sequential on purpose — reads are cache-first, and serializing
    /// respects the per-vendor cache flock and avoids an OAuth-refresh 429 burst.
    /// ponytail: parallelize only if it ever feels slow.
    func refreshOverview(bin: String, generation: Int) {
        // vendorEntries(active: "") keeps only configured entries — the
        // active-vendor courtesy slot has no place in an all-vendors sweep.
        let entries = filterOverviewEntries(vendorEntries(
                                                active: "",
                                                usageAccounts: lastAccountStatus?.usageAccounts),
                                            requested: configuredOverviewVendorIds())
        DispatchQueue.global(qos: .utility).async { [weak self] in
            var results: [(name: String, id: String, snap: Snapshot?)] = []
            for v in entries {
                let p = Process()
                p.executableURL = URL(fileURLWithPath: bin)
                p.arguments = vendorArgs(for: v.id) + ["--format", FORMAT_WITH_SENTINEL]
                let pipe = Pipe()
                p.standardOutput = pipe
                p.standardError = FileHandle.nullDevice
                let watchdog = DispatchWorkItem { if p.isRunning { p.terminate() } }
                DispatchQueue.global(qos: .utility)
                    .asyncAfter(deadline: .now() + REFRESH_TIMEOUT, execute: watchdog)
                var out = ""
                do {
                    try p.run()
                    let data = pipe.fileHandleForReading.readDataToEndOfFile()
                    p.waitUntilExit()
                    out = String(data: data, encoding: .utf8) ?? ""
                } catch { out = "" }
                watchdog.cancel()
                var snap: Snapshot?
                if let data = out.data(using: .utf8),
                   let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                   let text = obj["text"] as? String {
                    snap = parse(text, vendor: baseVendorId(v.id))
                }
                results.append((name: v.name, id: v.id, snap: snap))
            }
            DispatchQueue.main.async {
                self?.finishRefresh(generation) { me in
                    guard VENDOR == "overview" else { return }
                    me.renderOverview(results)
                }
            }
        }
    }

    /// The single most-relevant number for a vendor in the overview.
    /// Cursor: the combined "included total usage" headline (both pools as one).
    /// Everyone else: the most-exhausted of its windows — for Anthropic that is
    /// the biggest of 5h, weekly, and the scoped model bar (Fable). Falls back to
    /// the $ budget when a vendor has no time windows.
    func overviewHeadline(_ s: Snapshot) -> (pct: Int, value: String, reset: String?, elapsed: Int?) {
        if let t = s.cursorTotalPct {
            return (t, "\(t)%", s.weekly?.reset ?? s.session?.reset, nil)
        }
        let windows = [s.session, s.weekly, s.sonnet, s.secondaryWeekly].compactMap { $0 }
        if let w = windows.max(by: { $0.pct < $1.pct }) {
            return (w.pct, "\(w.pct)%", w.reset, w.elapsed)
        }
        if let e = s.extra { return (e.pct, "\(e.spent) / \(e.limit)", nil, nil) }
        return (0, "—", nil, nil)
    }

    /// Compact bar label for a vendor name. Usually the first word; for a
    /// Claude account ("Claude · gmail") the account label is the part that
    /// distinguishes entries, so use it instead of the shared "Claude".
    func ovLabel(_ name: String, _ maxLen: Int) -> String {
        let base = name.range(of: " · ").map { String(name[$0.upperBound...]) }
            ?? String(name.split(separator: " ").first ?? Substring(name))
        return base.count <= maxLen ? base : String(base.prefix(maxLen))
    }

    /// Status-bar summary for overview mode: every vendor at once. Mini bar per
    /// vendor when few; worst-first numbers (capped, with `+K` overflow) when
    /// many. Tunable via `[ui] overview_menubar_bars_max` (bar↔number threshold,
    /// default 4) and `overview_menubar_max` (how many numbers fit, default 4).
    func overviewBarTitle(_ items: [(name: String, id: String, snap: Snapshot?)],
                          _ appearance: NSAppearance) -> NSAttributedString {
        let secondary = menuBarTextColor(appearance, secondary: true)
        // Providers toggled off in the dropdown are dropped from the always-on
        // top summary (but still listed there so they can be re-enabled).
        let hidden = overviewHiddenProviders()
        let visible = Set(overviewVisibleIds(items.map { $0.id }, hidden: hidden))
        let heads: [(name: String, pct: Int, elapsed: Int?, value: String, reset: String?)] =
            items.compactMap {
                guard visible.contains($0.id), let s = $0.snap else { return nil }
                if let cb = s.creditBalance { return ($0.name, -1, nil, "cr \(cb)", nil) }
                let h = overviewHeadline(s)
                return ($0.name, h.pct, h.elapsed, "\(h.pct)%", h.reset.flatMap(shortReset))
            }
        guard !heads.isEmpty else { return run("ovr", secondary) }

        let barsMax = Int(configValueTOML("ui", "overview_menubar_bars_max") ?? "") ?? 4
        let compact = DEF.bool(forKey: "overviewCompact")
        let t = NSMutableAttributedString()
        if overviewUsesBars(count: heads.count, barsMax: barsMax, compact: compact) {
            for (i, e) in heads.enumerated() {
                if i > 0 { t.append(run("   ", secondary)) }
                t.append(run("\(ovLabel(e.name, 6)) ", secondary))
                if e.pct >= 0 {
                    t.append(run("\(e.pct)% ", colorForPct(e.pct)))
                    t.append(progressAttr(pct: e.pct, width: BAR_WIDTH, elapsed: e.elapsed,
                                          appearance: appearance))
                    if let r = e.reset { t.append(run(" \(r)", secondary)) }
                } else {
                    t.append(run(e.value, .labelColor))  // credit balance
                }
            }
        } else {
            let maxN = Int(configValueTOML("ui", "overview_menubar_max") ?? "") ?? 4
            // Entry order, not worst-first: entries are provider-grouped
            // (standard Claude, then its accounts, then the other vendors), and
            // stable positions beat a pct sort that reshuffles labels on every
            // refresh.
            for (i, e) in heads.prefix(maxN).enumerated() {
                if i > 0 { t.append(run("  ", secondary)) }
                t.append(run("\(ovLabel(e.name, 3)) ", secondary))
                t.append(run(e.value, e.pct >= 0 ? colorForPct(e.pct) : .labelColor))
                if let r = e.reset { t.append(run(" \(r)", secondary)) }
            }
            if heads.count > maxN { t.append(run("  +\(heads.count - maxN)", secondary)) }
        }
        return t
    }

    func renderOverview(_ items: [(name: String, id: String, snap: Snapshot?)],
                        rebuildSubmenu: Bool = true) {
        lastSnapshot = nil
        lastOverview = items
        let appearance = statusItem.button?.effectiveAppearance ?? NSApp.effectiveAppearance
        headerItem.attributedTitle = run("Visão geral", .labelColor, NSFont.boldSystemFont(ofSize: 13))
        for key in ["session", "weekly", "sonnet", "extra"] { rows[key]?.isHidden = true }
        ensureOverviewRowCapacity(items.count)

        // Rows render in a monospaced font, so fixed character widths line the
        // columns up. Pre-compute the widest headline pct and reset so both can be
        // right-aligned — the reset then ends flush at the line's right edge.
        let nameW = 14
        var pctW = 0, resetW = 0
        for item in items.prefix(overviewRows.count) {
            guard let s = item.snap, s.creditBalance == nil else { continue }
            let h = overviewHeadline(s)
            pctW = max(pctW, h.value.count)
            if let r = h.reset, !r.isEmpty { resetW = max(resetW, r.count) }
        }
        func rpad(_ s: String, _ w: Int) -> String {
            String(repeating: " ", count: max(0, w - s.count)) + s
        }

        let hidden = overviewHiddenProviders()
        for (i, item) in items.enumerated() where i < overviewRows.count {
            let it = overviewRows[i]
            it.isHidden = false
            // Click a row to toggle whether this provider shows in the top-bar
            // summary. Checkmark = shown; unchecked + dimmed = hidden from the
            // bar (still listed here so it can be turned back on). Jump-to-vendor
            // moved to the "Trocar vendor" submenu and ⌥⌘\.
            let isHidden = hidden.contains(item.id)
            it.state = isHidden ? .off : .on
            it.representedObject = item.id
            it.target = self
            it.action = #selector(toggleOverviewProvider(_:))
            let a = NSMutableAttributedString()
            let fitted = item.name.count > nameW
                ? String(item.name.prefix(nameW - 1)) + "…"
                : item.name.padding(toLength: nameW, withPad: " ", startingAt: 0)
            a.append(run(fitted + " ", .labelColor))
            if let s = item.snap {
                if let cb = s.creditBalance {
                    a.append(run("cr \(cb)", .labelColor))
                } else {
                    let h = overviewHeadline(s)
                    a.append(progressAttr(pct: h.pct, width: MENU_BAR_W, elapsed: h.elapsed,
                                          menu: true, appearance: appearance))
                    a.append(run("  \(rpad(h.value, pctW))", colorForPct(h.pct)))
                    if let r = h.reset, !r.isEmpty {
                        a.append(run("   ↺ \(rpad(r, resetW))", .secondaryLabelColor))
                    }
                }
            } else {
                a.append(run("—", .secondaryLabelColor))
            }
            // Grey the whole row while hidden, so "off" reads at a glance.
            if isHidden {
                a.addAttribute(.foregroundColor, value: NSColor.tertiaryLabelColor,
                               range: NSRange(location: 0, length: a.length))
            }
            it.attributedTitle = a
        }
        for i in items.count..<overviewRows.count { overviewRows[i].isHidden = true }

        // Menu-bar title: every vendor at once (mini bars when few, numbers when
        // many). The dropdown always holds the full per-vendor list.
        statusItem.button?.attributedTitle = overviewBarTitle(items, appearance)
        compactItem.isHidden = false
        updateCompactItemTitle()
        if rebuildSubmenu { rebuildVendorSubmenu() }
    }

    /// Add overview row slots before the single-vendor rows when newly
    /// discovered Claude accounts outgrow the initial built-in-vendor pool.
    private func ensureOverviewRowCapacity(_ count: Int) {
        guard count > overviewRows.count,
              let menu = statusItem.menu,
              let firstDetail = rows["session"] else { return }
        while overviewRows.count < count {
            let item = NSMenuItem()
            item.isHidden = true
            menu.insertItem(item, at: menu.index(of: firstDetail))
            overviewRows.append(item)
        }
    }

    func consume(_ output: String) {
        guard let data = output.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let text = obj["text"] as? String else {
            setError("saída inválida")
            return
        }
        guard let snap = parse(text, vendor: baseVendorId(VENDOR)) else {
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
        } else if s.hasUsageWindows && SHOW_SESSION, let session = s.session {
            seg(s.sessionTag, session.pct, "\(session.pct)%", session.elapsed)
        }
        if s.creditBalance == nil && s.hasUsageWindows && SHOW_WEEKLY, let weekly = s.weekly {
            seg(s.weeklyTag, weekly.pct, "\(weekly.pct)%", weekly.elapsed)
        }
        if SHOW_EXTRA, let e = s.extra { seg("ex", e.pct, e.spent, nil) } // $ budget → no meta
        statusItem.button?.attributedTitle = title.length > 0 ? title : run("ai", secondaryTextColor)
    }

    func renderMenu(_ s: Snapshot) {
        let appearance = statusItem.button?.effectiveAppearance ?? NSApp.effectiveAppearance
        for it in overviewRows { it.isHidden = true }
        compactItem.isHidden = true
        // With several Claude accounts the plan alone ("Claude Max 20x") no
        // longer says WHICH account this is — suffix the label.
        let plan = s.plan.isEmpty ? "AI Usage" : s.plan
        let header = accountLabel(of: VENDOR).map { "\(plan) · \($0)" } ?? plan
        headerItem.attributedTitle = run(header, .labelColor, NSFont.boldSystemFont(ofSize: 13))

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
        } else {
            if s.hasUsageWindows, let session = s.session {
                row("session", s.sessionLabel, session.pct, "\(session.pct)%", session.reset, session.elapsed)
            } else { rows["session"]?.isHidden = true }
            if s.hasUsageWindows, let weekly = s.weekly {
                row("weekly", s.weeklyLabel, weekly.pct, "\(weekly.pct)%", weekly.reset, weekly.elapsed)
            } else { rows["weekly"]?.isHidden = true }
        }
        if let sn = s.sonnet { row("sonnet", s.sonnetLabel, sn.pct, "\(sn.pct)%", sn.reset, sn.elapsed) }
        else { rows["sonnet"]?.isHidden = true }
        if let weekly = s.secondaryWeekly {
            row("extra", s.secondaryWeeklyLabel, weekly.pct, "\(weekly.pct)%", weekly.reset, weekly.elapsed)
        } else if let e = s.extra {
            row("extra", "Extra usage", e.pct, "\(e.spent) / \(e.limit)", nil, nil)
        } else {
            rows["extra"]?.isHidden = true
        }
        rebuildVendorSubmenu()
    }

    // Vendor switch submenu: lists only configured vendors, with a checkmark on
    // the active one. Selecting one rewrites the `vendor` default and triggers a
    // refresh via the shared settings-change observer.
    func rebuildVendorSubmenu() {
        vendorSubmenu.removeAllItems()
        let active = VENDOR
        let entries = vendorEntries(active: active,
                                    usageAccounts: lastAccountStatus?.usageAccounts)
        if entries.isEmpty {
            let none = NSMenuItem(title: "Nenhum configurado", action: nil, keyEquivalent: "")
            none.isEnabled = false
            vendorSubmenu.addItem(none)
            vendorSubmenuItem.isHidden = false
            return
        }
        for v in entries {
            let it = NSMenuItem(title: v.name, action: #selector(switchVendor(_:)), keyEquivalent: "")
            it.target = self
            it.representedObject = v.id
            it.state = (v.id == active) ? .on : .off
            vendorSubmenu.addItem(it)
        }
        // Synthetic overview target — same one the ⌥⌘\ ring ends on.
        vendorSubmenu.addItem(.separator())
        let ov = NSMenuItem(title: "Visão geral", action: #selector(switchVendor(_:)), keyEquivalent: "")
        ov.target = self
        ov.representedObject = "overview"
        ov.state = (active == "overview") ? .on : .off
        vendorSubmenu.addItem(ov)
        vendorSubmenuItem.isHidden = false
    }

    @objc func switchVendor(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        DEF.set(id, forKey: "vendor")
    }

    /// Ask the binary who each surface is signed in as. Same off-main shape as
    /// `refresh()`; failures leave `lastAccountStatus` alone so a transient
    /// hiccup doesn't blank a working menu.
    func fetchAccountStatus() {
        guard let bin = resolveBinary("ai-usagebar") else { return }
        accountStatusFetchedAt = Date()
        accountStatusGeneration += 1
        let generation = accountStatusGeneration
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let p = Process()
            p.executableURL = URL(fileURLWithPath: bin)
            p.arguments = ["account", "status", "--json"]
            let pipe = Pipe()
            p.standardOutput = pipe
            p.standardError = FileHandle.nullDevice

            let watchdog = DispatchWorkItem { if p.isRunning { p.terminate() } }
            DispatchQueue.global(qos: .utility)
                .asyncAfter(deadline: .now() + REFRESH_TIMEOUT, execute: watchdog)
            var data = Data()
            do {
                try p.run()
                data = pipe.fileHandleForReading.readDataToEndOfFile()  // read before wait
                p.waitUntilExit()
            } catch {
                watchdog.cancel()
                return
            }
            watchdog.cancel()
            guard let status = parseAccountStatus(data) else { return }
            DispatchQueue.main.async {
                guard let me = self, generation == me.accountStatusGeneration else { return }
                me.applyAccountStatus(status)
            }
        }
    }

    /// Ask the binary about sync. The same shape as `fetchAccountStatus` — a
    /// generation counter, `.utility`, the `REFRESH_TIMEOUT` watchdog, the pipe
    /// read before `waitUntilExit`, and a main-thread hop guarded by the
    /// generation so a slow reply cannot overwrite a newer one (T-6-04).
    ///
    /// stdin is /dev/null: `sync status --json` never reads it, and pinning
    /// that here means a future build that did could not hang the menu (D-02).
    func fetchSyncStatus() {
        guard let bin = resolveBinary("ai-usagebar") else { return }
        syncStatusFetchedAt = Date()
        syncStatusGeneration += 1
        let generation = syncStatusGeneration
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let p = Process()
            p.executableURL = URL(fileURLWithPath: bin)
            p.arguments = ["sync", "status", "--json"]
            let pipe = Pipe()
            p.standardOutput = pipe
            p.standardError = FileHandle.nullDevice
            p.standardInput = FileHandle.nullDevice

            let watchdog = DispatchWorkItem { if p.isRunning { p.terminate() } }
            DispatchQueue.global(qos: .utility)
                .asyncAfter(deadline: .now() + REFRESH_TIMEOUT, execute: watchdog)
            var data = Data()
            do {
                try p.run()
                data = pipe.fileHandleForReading.readDataToEndOfFile()  // read before wait
                p.waitUntilExit()
            } catch {
                watchdog.cancel()
                return
            }
            watchdog.cancel()
            // A non-zero exit or an unparseable body leaves the previous value
            // alone, exactly as the account fetch does.
            guard p.terminationStatus == 0, let status = parseSyncStatus(data) else { return }
            DispatchQueue.main.async {
                guard let me = self, generation == me.syncStatusGeneration else { return }
                me.lastSyncStatus = status
                me.renderSyncRow()
            }
        }
    }

    /// Nil means the binary predates `--json`: leave the menu exactly as it was
    /// before this row existed.
    func renderSyncRow() {
        let line = syncSummaryLine(lastSyncStatus)
        syncInfoItem.isHidden = line.isEmpty
        syncInfoItem.attributedTitle = run(line, .secondaryLabelColor)
        renderSyncSubmenu()
    }

    /// Rebuilt from the last status, so the category rows can never be stale,
    /// and hidden until `sync status --json` answers for the same reason the
    /// row is: a binary that predates the flag shows the menu it always did.
    func renderSyncSubmenu() {
        syncMenuItem.isHidden = lastSyncStatus == nil
        syncSubmenu.removeAllItems()
        for row in syncCategoryRows(lastSyncStatus) {
            let it = NSMenuItem()
            it.attributedTitle = run(row, .secondaryLabelColor)
            it.isEnabled = false
            syncSubmenu.addItem(it)
        }
        if syncSubmenu.numberOfItems > 0 { syncSubmenu.addItem(.separator()) }
        addAction(syncSubmenu, "Atualizar estado", #selector(refreshSyncStatus), "")
        syncSubmenu.addItem(.separator())
        // No keyEquivalent on either. Every other item in this menu is
        // read-only or locally reversible; a hot-key that publishes bytes is
        // not something to hand out by accident.
        addAction(syncSubmenu, "Enviar agora…", #selector(pushSync), "")
        addAction(syncSubmenu, "Restaurar deste backup…", #selector(pullSync), "")
    }

    @objc func refreshSyncStatus() { fetchSyncStatus() }
    @objc func pushSync() { confirmSyncInTerminal(.push) }
    @objc func pullSync() { confirmSyncInTerminal(.pull) }

    /// Both sync actions leave through here, and **neither starts a
    /// subprocess**: the consequence is stated, the exact command is shown, and
    /// on confirmation Terminal.app is handed the script. There is no in-flight
    /// flag because nothing is in flight — the work happens in a window the
    /// user owns, where the password is asked for and every gate is answered.
    private func confirmSyncInTerminal(_ action: SyncAction) {
        guard let bin = resolveBinary("ai-usagebar") else { return }
        let prompt = syncPrompt(action)
        let alert = NSAlert()
        alert.messageText = prompt.title
        alert.informativeText = prompt.body + "\n\n" + syncCommandLine(binary: bin, action)
        alert.addButton(withTitle: prompt.go)
        alert.addButton(withTitle: "Cancelar")
        NSApp.activate(ignoringOtherApps: true)
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        runInTerminal(syncTerminalScript(binary: bin, action))
    }

    func applyAccountStatus(_ status: AccountStatus) {
        lastAccountStatus = status
        renderAccountMenus()
        rebuildVendorSubmenu()
    }

    func renderAccountMenus() {
        // Nil means the binary predates the subcommand: leave the menu exactly
        // as it was before this feature existed.
        guard let status = lastAccountStatus else {
            accountsInfoItem.isHidden = true
            desktopAccountItem.isHidden = true
            cliAccountItem.isHidden = true
            return
        }
        let line = accountsSummaryLine(status)
        accountsInfoItem.isHidden = line.isEmpty
        accountsInfoItem.attributedTitle = run(line, .secondaryLabelColor)

        fill(desktopAccountSubmenu, status.desktopLabels, status.desktopActive,
             #selector(switchDesktopAccount(_:)))
        fill(cliAccountSubmenu, status.cliLabels, status.cliActive,
             #selector(switchCliAccount(_:)))
        // Both stay visible with an empty list — that is when "Adicionar
        // conta…" matters most. Only a machine with no Claude Desktop app at
        // all loses its submenu.
        desktopAccountItem.isHidden = !status.desktopAvailable
        cliAccountItem.isHidden = false
        desktopAccountItem.isEnabled = !accountSwitchInFlight
        cliAccountItem.isEnabled = !accountSwitchInFlight
    }

    private func fill(_ submenu: NSMenu, _ labels: [String], _ active: String?, _ action: Selector) {
        submenu.removeAllItems()
        for label in labels {
            let it = NSMenuItem(title: label, action: action, keyEquivalent: "")
            it.target = self
            it.representedObject = label
            it.state = (label == active) ? .on : .off
            it.isEnabled = !accountSwitchInFlight && label != active
            submenu.addItem(it)
        }
        submenu.addItem(.separator())
        let add = NSMenuItem(title: "Adicionar conta…",
                             action: #selector(addAccount(_:)), keyEquivalent: "")
        add.target = self
        add.representedObject = (submenu === desktopAccountSubmenu)
        add.isEnabled = !accountSwitchInFlight
        submenu.addItem(add)
    }

    /// Ask for a label, then hand the interactive part to Terminal: a Desktop
    /// capture waits for a browser sign-in, a CLI one runs `claude`. Neither
    /// belongs in a background subprocess the user cannot see or answer.
    @objc func addAccount(_ sender: NSMenuItem) {
        guard let desktop = sender.representedObject as? Bool,
              let bin = resolveBinary("ai-usagebar") else { return }
        let alert = NSAlert()
        alert.messageText = desktop ? "Adicionar conta do Claude Desktop"
                                    : "Adicionar conta do Claude Code"
        alert.informativeText = desktop
            ? "Escolha um nome. O app será fechado e reaberto na tela de login para você "
                + "entrar com a conta nova; a atual é restaurada se você cancelar."
            : "Escolha um nome. O `claude` abrirá no Terminal para você entrar; seu login "
                + "padrão não é alterado."
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 220, height: 24))
        field.placeholderString = "trabalho"
        alert.accessoryView = field
        alert.addButton(withTitle: "Continuar")
        alert.addButton(withTitle: "Cancelar")
        NSApp.activate(ignoringOtherApps: true)
        alert.window.initialFirstResponder = field
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        let label = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !label.isEmpty else { return }
        runInTerminal(addAccountScript(binary: bin, label: label, desktop: desktop))
    }

    /// Quits and reopens Claude.app, so confirm first — an unsent message or an
    /// in-flight Cowork run would go with it. The CLI switch has no visible
    /// side effect and needs no prompt.
    @objc func switchDesktopAccount(_ sender: NSMenuItem) {
        guard let label = sender.representedObject as? String else { return }
        let alert = NSAlert()
        alert.messageText = "Trocar a conta do Claude Desktop para «\(label)»?"
        alert.informativeText = "O app será fechado e reaberto. Seu histórico local é "
            + "mesclado nessa conta antes da troca, e uma cópia de segurança é gravada."
        alert.addButton(withTitle: "Trocar e reiniciar")
        alert.addButton(withTitle: "Cancelar")
        NSApp.activate(ignoringOtherApps: true)
        guard alert.runModal() == .alertFirstButtonReturn else { return }
        // Routines deleted in one account but alive in another would be handed
        // straight back by the merge. Decide before the switch starts, since a
        // background subprocess has no way to ask.
        guard let doomed = resolveDeletionConflicts() else { return }
        runAccountSwitch(label: label, desktop: true, deleting: doomed)
    }

    /// Returns the routine ids to delete everywhere, or nil if the user backed
    /// out of the whole switch. An empty array means "keep them all".
    private func resolveDeletionConflicts() -> [String]? {
        let conflicts = lastAccountStatus?.deletionConflicts ?? []
        if conflicts.isEmpty { return [] }

        let alert = NSAlert()
        alert.messageText = conflicts.count == 1
            ? "1 item foi apagado em uma conta e ainda existe em outra"
            : "\(conflicts.count) itens foram apagados em uma conta e ainda existem em outra"
        alert.informativeText = conflictPreview(conflicts)
            + "\n\nManter traz de volta os apagados. Apagar remove de todas as contas — uma conversa perde só o índice, a transcrição permanece."
        alert.addButton(withTitle: "Manter todas")
        alert.addButton(withTitle: "Apagar em todas")
        alert.addButton(withTitle: "Escolher…")
        NSApp.activate(ignoringOtherApps: true)
        switch alert.runModal() {
        case .alertFirstButtonReturn: return []
        case .alertSecondButtonReturn: return conflicts.map { $0.key }
        default: return chooseDeletionConflicts(conflicts)
        }
    }

    /// A checkbox per conflict — checked means keep. Everything left unchecked
    /// is deleted everywhere, so the destructive outcome takes a deliberate
    /// uncheck rather than being the default.
    private func chooseDeletionConflicts(_ conflicts: [DeletionConflict]) -> [String]? {
        let rowHeight = 22
        let list = NSStackView(frame: NSRect(x: 0, y: 0, width: 420,
                                             height: rowHeight * conflicts.count))
        list.orientation = .vertical
        list.alignment = .leading
        list.spacing = 4
        var boxes: [NSButton] = []
        for conflict in conflicts {
            let box = NSButton(checkboxWithTitle: conflict.line, target: nil, action: nil)
            box.state = .on
            boxes.append(box)
            list.addArrangedSubview(box)
        }
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 420,
                                                height: min(260, rowHeight * conflicts.count + 8)))
        scroll.hasVerticalScroller = true
        scroll.documentView = list

        let alert = NSAlert()
        alert.messageText = "O que manter?"
        alert.informativeText = "Marcados continuam. Os desmarcados são apagados em todas as contas."
        alert.accessoryView = scroll
        alert.addButton(withTitle: "Confirmar")
        alert.addButton(withTitle: "Cancelar")
        NSApp.activate(ignoringOtherApps: true)
        guard alert.runModal() == .alertFirstButtonReturn else { return nil }
        return zip(conflicts, boxes).filter { $0.1.state != .on }.map { $0.0.key }
    }

    @objc func switchCliAccount(_ sender: NSMenuItem) {
        guard let label = sender.representedObject as? String else { return }
        runAccountSwitch(label: label, desktop: false)
    }

    private func runAccountSwitch(label: String, desktop: Bool, deleting: [String] = []) {
        guard let bin = resolveBinary("ai-usagebar"), !accountSwitchInFlight else { return }
        accountSwitchInFlight = true
        renderAccountMenus()
        let args = switchArgs(label: label, desktop: desktop, deleting: deleting)
        DispatchQueue.global(qos: .utility).async { [weak self] in
            let p = Process()
            p.executableURL = URL(fileURLWithPath: bin)
            p.arguments = args
            let pipe = Pipe()
            p.standardOutput = pipe
            p.standardError = pipe
            let failure: String?
            do {
                try p.run()
                let data = pipe.fileHandleForReading.readDataToEndOfFile()
                p.waitUntilExit()
                if p.terminationStatus != 0 {
                    let detail = String(decoding: data, as: UTF8.self)
                        .trimmingCharacters(in: .whitespacesAndNewlines)
                    failure = detail.isEmpty
                        ? "ai-usagebar terminou com status \(p.terminationStatus)."
                        : String(detail.prefix(2_000))
                } else {
                    failure = nil
                }
            } catch {
                failure = "Não foi possível iniciar ai-usagebar: \(error.localizedDescription)"
            }
            DispatchQueue.main.async {
                guard let me = self else { return }
                me.accountSwitchInFlight = false
                me.fetchAccountStatus()
                if let failure {
                    let alert = NSAlert()
                    alert.alertStyle = .warning
                    alert.messageText = "Não foi possível trocar a conta"
                    alert.informativeText = failure
                    alert.addButton(withTitle: "OK")
                    NSApp.activate(ignoringOtherApps: true)
                    alert.runModal()
                } else {
                    // The usage numbers belong to whoever is signed in now.
                    me.refresh()
                }
            }
        }
    }

    /// Toggle whether a provider appears in the Overview top-bar summary, from
    /// its dropdown row. The UserDefaults observer re-renders from the last
    /// fetch and consumes this presentation-only change without re-fetching.
    @objc func toggleOverviewProvider(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        overviewVisibilityChangePending = true
        setOverviewProvider(id, hidden: !overviewHiddenProviders().contains(id))
    }

    func setError(_ msg: String) {
        lastSnapshot = nil
        statusItem.button?.attributedTitle = run("⚠ ai", hexColor(COLOR_CRITICAL))
        let appearance = statusItem.button?.effectiveAppearance ?? NSApp.effectiveAppearance
        headerItem.attributedTitle = run(msg, menuBarTextColor(appearance))
        for (_, it) in rows { it.isHidden = true }
        for it in overviewRows { it.isHidden = true }
        compactItem.isHidden = true
        accountsInfoItem.isHidden = true
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
