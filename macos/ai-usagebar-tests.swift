// Test harness for ai-usagebar-menubar.swift.
//
// The app is a single Swift file with no Xcode project, so there is no XCTest
// bundle. Instead, the entry point (`app.run()`) is guarded by
// `#if !SWIFT_TEST_HARNESS`, and this file is compiled together with the app in
// one module (the only top-level code, so Swift treats it as the main file),
// calling the app's helpers (arcAngles, tomlValueInText, defaultEnabled, parse)
// directly.
//
// Run:  ./macos/run-tests.sh
// Gate: pure-logic regression coverage for the review fixes.

import Foundation

private var failures = 0

private func assertEqual<T: Equatable>(_ got: T, _ expected: T, _ name: String,
                                       file: String = #file, line: Int = #line) {
    if got == expected {
        print("  ✓ \(name)")
    } else {
        print("  ✗ \(name): got \(got), expected \(expected) (\(file):\(line))")
        failures += 1
    }
}

// Floating-point arc angles accumulate rounding error; compare within an epsilon.
private func assertEqualDeg(_ got: CGFloat, _ expected: CGFloat, _ name: String,
                            file: String = #file, line: Int = #line) {
    if abs(got - expected) < 1e-6 {
        print("  ✓ \(name)")
    } else {
        print("  ✗ \(name): got \(got), expected \(expected) (\(file):\(line))")
        failures += 1
    }
}

private func assertNil(_ got: Any?, _ name: String,
                       file: String = #file, line: Int = #line) {
    if got == nil {
        print("  ✓ \(name)")
    } else {
        print("  ✗ \(name): expected nil, got \(String(describing: got)) (\(file):\(line))")
        failures += 1
    }
}

private func assertNotNil(_ got: Any?, _ name: String,
                          file: String = #file, line: Int = #line) {
    if got != nil {
        print("  ✓ \(name)")
    } else {
        print("  ✗ \(name): expected non-nil (\(file):\(line))")
        failures += 1
    }
}

// ─── Ring arc geometry (the pace-arc regression) ─────────────────────────
//
// drawArc previously restarted at 12 o'clock, so the overshoot overpainted the
// calm fill. arcAngles(from:to:) must place [from,to] contiguously: the segment
// end at `from` equals the start at `to`, so two adjacent segments join.
func testRingArc() {
    print("ring arc geometry")
    // p=80, e=50 → boundary = min(0.8, 0.5) = 0.5. Calm [0, 0.5], over [0.5, 0.8].
    let calm = arcAngles(from: 0, to: 0.5)
    let over = arcAngles(from: 0.5, to: 0.8)
    // Contiguous: calm end == over start (the overshoot picks up at the marker).
    assertEqual(calm.endDeg, over.startDeg, "p80/e50 calm-end == over-start (no gap)")
    // Calm spans 0..0.5 of the ring (180°), over spans 0.5..0.8 (108°).
    let calmSpan = calm.startDeg - calm.endDeg
    let overSpan = over.startDeg - over.endDeg
    assertEqualDeg(calmSpan, 180.0, "calm span is half the ring (180°)")
    assertEqualDeg(overSpan, 108.0, "over span is 0.3 of the ring (108°)")

    // p=30, e=50 → no overshoot; only the calm arc [0, 0.3] is drawn.
    let only = arcAngles(from: 0, to: 0.3)
    assertEqualDeg(only.startDeg - only.endDeg, 108.0, "p30/e50 single arc span (108°)")

    // From == to → zero-length segment (guard prevents drawing).
    let zero = arcAngles(from: 0.4, to: 0.4)
    assertEqual(zero.startDeg, zero.endDeg, "zero-length segment is degenerate")
}

// ─── TOML enabled / api_key_env parsing ──────────────────────────────────
func testTomlParsing() {
    print("TOML enabled + api_key_env")
    // Bare false.
    let bareFalse = """
    [deepseek]
    enabled = false
    """
    assertEqual(tomlValueInText(bareFalse, section: "deepseek", key: "enabled"), "false",
                "bare enabled = false")

    // Bare true.
    let bareTrue = """
    [kimi]
    enabled = true
    """
    assertEqual(tomlValueInText(bareTrue, section: "kimi", key: "enabled"), "true",
                "bare enabled = true")

    // Inline comment on a bare boolean.
    let commented = """
    [kilo]
    enabled = false  # opt-in balance vendor
    """
    assertEqual(tomlValueInText(commented, section: "kilo", key: "enabled"), "false",
                "bare bool with inline comment")

    // Quoted string still works (api_key).
    let quoted = """
    [grok]
    api_key = "sk-test-123"
    """
    assertEqual(tomlValueInText(quoted, section: "grok", key: "api_key"), "sk-test-123",
                "quoted api_key")

    // Custom api_key_env.
    let customEnv = """
    [novita]
    api_key_env = "MY_NOVITA_KEY"
    """
    assertEqual(tomlValueInText(customEnv, section: "novita", key: "api_key_env"), "MY_NOVITA_KEY",
                "custom api_key_env")

    // Omitted key returns nil.
    let omitted = """
    [anthropic]
    credentials_path = "/tmp/creds.json"
    """
    assertNil(tomlValueInText(omitted, section: "anthropic", key: "enabled"),
              "omitted enabled is nil")

    // Section scoping: a key under another section must not leak.
    let scoped = """
    [openrouter]
    enabled = true

    [deepseek]
    api_key = "ds-key"
    """
    assertNil(tomlValueInText(scoped, section: "deepseek", key: "enabled"),
              "enabled does not leak across sections")
    assertEqual(tomlValueInText(scoped, section: "deepseek", key: "api_key"), "ds-key",
                "api_key read from the right section")
}

// ─── Rust enabled defaults (src/config.rs) ───────────────────────────────
func testDefaultEnabled() {
    print("Rust enabled defaults")
    for id in ["anthropic", "openai", "zai", "openrouter"] {
        assertEqual(defaultEnabled(id), true, "\(id) defaults enabled")
    }
    for id in ["deepseek", "kimi", "kilo", "novita", "moonshot", "grok", "anthropic_api"] {
        assertEqual(defaultEnabled(id), false, "\(id) defaults disabled (opt-in)")
    }
}

// ─── Parser: balances per vendor, no fake 0% rows ────────────────────────
//
// A balance-only vendor must surface its real balance and suppress the 5h/7d
// windows; a rate-limit vendor must show windows and no balance.
func snapshot(_ format: String, vendor: String, fields: [String]) -> Snapshot? {
    // Substitute the requested fields into the FORMAT layout, mirroring what the
    // Rust binary emits. Unknown placeholders stay literal and `t()` discards them.
    var values: [String: String] = [:]
    for (i, f) in fields.enumerated() { values["\(i)"] = f }
    let joined = format.components(separatedBy: ";;").enumerated().map { (i, tok) -> String in
        // Replace {placeholder} tokens with the test field at that index when the
        // caller wants a concrete value; otherwise leave the placeholder so `t()`
        // treats it as empty.
        if let v = values["\(i)"], !v.isEmpty { return v }
        return tok
    }.joined(separator: ";;")
    return parse(joined + ";;__aiub_end__", vendor: vendor)
}

func testParserBalances() {
    print("parser balances per vendor")
    // Build a field array long enough to cover index 26 (aapi_limit).
    func fields(through max: Int, set: [Int: String]) -> [String] {
        (0...max).map { set[$0] ?? "" }
    }

    // OpenRouter: balance at 17, vendor_short "opr".
    let opr = snapshot(FORMAT, vendor: "openrouter",
                       fields: fields(through: 17, set: [16: "opr", 17: "$12.34"]))
    assertNotNil(opr?.creditBalance, "openrouter has a balance")
    assertEqual(opr?.creditBalance, "$12.34", "openrouter balance value")
    assertEqual(opr?.hasUsageWindows, false, "openrouter suppresses 5h/7d windows")

    // DeepSeek: balance at 18.
    let dsk = snapshot(FORMAT, vendor: "deepseek",
                       fields: fields(through: 18, set: [18: "$5.00"]))
    assertEqual(dsk?.creditBalance, "$5.00", "deepseek balance value")
    assertEqual(dsk?.hasUsageWindows, false, "deepseek suppresses 5h/7d windows")

    // Kilo: balance at 19.
    let klo = snapshot(FORMAT, vendor: "kilo",
                       fields: fields(through: 19, set: [19: "$3.50"]))
    assertEqual(klo?.creditBalance, "$3.50", "kilo balance value")
    assertEqual(klo?.hasUsageWindows, false, "kilo suppresses 5h/7d windows")

    // Moonshot: balance at 21 (km_balance) — proves the dispatch keys on the
    // selected vendor, not on vendor_short "kmi" (which collides with Kimi).
    let moon = snapshot(FORMAT, vendor: "moonshot",
                        fields: fields(through: 21, set: [21: "¥42.00"]))
    assertEqual(moon?.creditBalance, "¥42.00", "moonshot balance via km_balance")
    assertEqual(moon?.hasUsageWindows, false, "moonshot suppresses 5h/7d windows")

    // Grok: balance at 22.
    let grk = snapshot(FORMAT, vendor: "grok",
                       fields: fields(through: 22, set: [22: "$9.99"]))
    assertEqual(grk?.creditBalance, "$9.99", "grok balance value")

    // Anthropic API with a monthly limit → spend-vs-limit bar, no duplicate
    // session/weekly, and no headline balance (the bar replaces it).
    let aapiLimit = snapshot(FORMAT, vendor: "anthropic_api",
                             fields: fields(through: 26, set: [
                                23: "$12.00 / $100 · 12%", 24: "12", 25: "$12.00", 26: "$100"
                             ]))
    assertEqual(aapiLimit?.hasUsageWindows, false, "anthropic_api suppresses session/weekly")
    assertNil(aapiLimit?.creditBalance, "anthropic_api with limit drops the headline")
    assertEqual(aapiLimit?.extra?.pct, 12, "anthropic_api extra bar pct")
    assertEqual(aapiLimit?.extra?.limit, "$100", "anthropic_api extra bar limit")

    // Anthropic API without a limit → headline balance only.
    let aapiNoLimit = snapshot(FORMAT, vendor: "anthropic_api",
                               fields: fields(through: 23, set: [23: "$12.34/mo"]))
    assertEqual(aapiNoLimit?.creditBalance, "$12.34/mo", "anthropic_api headline without limit")
    assertNil(aapiNoLimit?.extra, "anthropic_api without limit has no extra bar")

    // Rate-limit vendor (Anthropic): windows present, no balance.
    let cld = snapshot(FORMAT, vendor: "anthropic",
                       fields: fields(through: 16, set: [
                          1: "42", 2: "5h", 3: "60", 4: "Mon", 16: "cld"
                       ]))
    assertEqual(cld?.hasUsageWindows, true, "anthropic shows windows")
    assertNil(cld?.creditBalance, "anthropic has no balance")
    assertEqual(cld?.session.pct, 42, "anthropic session pct")
}

// ─── Run ─────────────────────────────────────────────────────────────────
@main
struct TestRunner {
    static func main() {
        testRingArc()
        testTomlParsing()
        testDefaultEnabled()
        testParserBalances()
        if failures > 0 {
            print("\n\(failures) test(s) FAILED")
            exit(1)
        }
        print("\nall tests passed")
    }
}
