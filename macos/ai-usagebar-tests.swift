// Test harness for ai-usagebar-menubar.swift.
//
// The app is a single Swift file with no Xcode project, so there is no XCTest
// bundle. Instead, the app's `@main` entry point is guarded by
// `#if !SWIFT_TEST_HARNESS`, and this file — compiled together with the app in
// one module — supplies its own `@main TestRunner`, calling the app's helpers
// (arcAngles, tomlValueInText, defaultEnabled, parse) directly.
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

    // `#` inside a quoted value is data, not a comment.
    let hashInQuotes = """
    [grok]
    api_key = "sk-abc#def"
    """
    assertEqual(tomlValueInText(hashInQuotes, section: "grok", key: "api_key"), "sk-abc#def",
                "quoted api_key keeps an embedded #")

    // Trailing comment after a quoted value.
    let quotedComment = """
    [grok]
    api_key = "x" # trailing comment
    """
    assertEqual(tomlValueInText(quotedComment, section: "grok", key: "api_key"), "x",
                "quoted api_key with trailing comment")

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

    let overview = """
    [ui]
    overview_vendors = ["cursor", 'anthropic', "openai"] # ordered subset
    """
    assertEqual(tomlStringArrayInText(overview, section: "ui", key: "overview_vendors")?
                    .joined(separator: ","),
                "cursor,anthropic,openai", "quoted string array")
    let multilineOverview = """
    [ui]
    overview_vendors = [
        "anthropic", # every named account
        "cursor",
    ]
    """
    assertEqual(tomlStringArrayInText(multilineOverview, section: "ui",
                                      key: "overview_vendors")?.joined(separator: ","),
                "anthropic,cursor", "multiline string array with comments")
    let emptyOverview = """
    [ui]
    overview_vendors = []
    """
    assertEqual(tomlStringArrayInText(emptyOverview, section: "ui", key: "overview_vendors")?
                    .isEmpty, true, "empty string array")
    let malformedOverview = """
    [ui]
    overview_vendors = ["cursor", 42]
    """
    assertNil(tomlStringArrayInText(malformedOverview, section: "ui", key: "overview_vendors"),
              "malformed string array rejected")
}

// ─── Rust enabled defaults (src/config.rs) ───────────────────────────────
func testDefaultEnabled() {
    print("Rust enabled defaults")
    for id in ["anthropic", "openai", "zai", "openrouter"] {
        assertEqual(defaultEnabled(id), true, "\(id) defaults enabled")
    }
    for id in ["deepseek", "kimi", "kilo", "novita", "moonshot", "grok", "anthropic_api", "cursor", "antigravity"] {
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
    // selected vendor, not on vendor_short ("kmi" collided with Kimi on
    // binaries up to 0.16).
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
    assertEqual(cld?.session?.pct, 42, "anthropic session pct")

    let oaiWeekly = snapshot(FORMAT, vendor: "openai",
                             fields: fields(through: 16, set: [
                                0: "ChatGPT Pro", 3: "66", 4: "5d 12h", 14: "23", 16: "gpt"
                             ]))
    assertEqual(oaiWeekly?.hasUsageWindows, true, "openai weekly-only shows windows")
    assertNil(oaiWeekly?.creditBalance, "openai weekly-only has no balance")
    assertNil(oaiWeekly?.session, "openai weekly-only suppresses absent 5h session")
    assertEqual(oaiWeekly?.weekly?.pct, 66, "openai weekly-only reports 7d pct")
    assertEqual(oaiWeekly?.weekly?.reset, "5d 12h", "openai weekly-only reports 7d reset")
    assertEqual(oaiWeekly?.weekly?.elapsed, 23, "openai weekly-only reports 7d pace elapsed")

    // Cursor: two included-usage pools carried on the session/weekly aliases
    // (session = Cursor Models, weekly = Other Models), both real, no balance.
    // The bars are relabeled away from the "Session"/"Weekly" time-window names.
    let cur = snapshot(FORMAT, vendor: "cursor",
                       fields: fields(through: 16, set: [
                          0: "Cursor Ultra", 1: "98", 2: "8d", 3: "100", 4: "8d", 16: "cur"
                       ]))
    assertEqual(cur?.hasUsageWindows, true, "cursor shows windows")
    assertNil(cur?.creditBalance, "cursor has no balance")
    assertEqual(cur?.session?.pct, 98, "cursor Cursor Models pct")
    assertEqual(cur?.weekly?.pct, 100, "cursor Other Models pct")
    assertEqual(cur?.sessionLabel, "Cursor Models", "cursor relabels the session bar")
    assertEqual(cur?.weeklyLabel, "Other Models", "cursor relabels the weekly bar")
    assertEqual(cur?.sessionTag, "auto", "cursor session tag")
    assertEqual(cur?.weeklyTag, "premium", "cursor weekly tag")

    // Antigravity has two independent model pools, each with a 5h and weekly
    // window. The fourth window reuses `extra_pct`, but it is not a spend bar:
    // its model/reset/elapsed fields follow Cursor's total at the FORMAT tail.
    let agy = snapshot(FORMAT, vendor: "antigravity",
                       fields: fields(through: 30, set: [
                          0: "Pro", 1: "12", 2: "4h", 3: "34", 4: "5d",
                          7: "78", 10: "Claude & GPT OSS", 11: "56", 12: "3h",
                          13: "20", 14: "30", 15: "40", 16: "agy",
                          28: "Claude & GPT OSS", 29: "6d", 30: "50"
                       ]))
    assertEqual(agy?.session?.pct, 12, "antigravity Gemini 5h pct")
    assertEqual(agy?.weekly?.pct, 34, "antigravity Gemini weekly pct")
    assertEqual(agy?.sonnet?.pct, 56, "antigravity third-party 5h pct")
    assertEqual(agy?.secondaryWeekly?.pct, 78, "antigravity third-party weekly pct")
    assertEqual(agy?.secondaryWeekly?.reset, "6d", "antigravity fourth reset")
    assertEqual(agy?.secondaryWeekly?.elapsed, 50, "antigravity fourth elapsed")
    assertEqual(agy?.sessionLabel, "Gemini 5h", "antigravity primary 5h label")
    assertEqual(agy?.weeklyLabel, "Gemini Weekly", "antigravity primary weekly label")
    assertEqual(agy?.sonnetLabel, "Claude & GPT OSS 5h", "antigravity third-party 5h label")
    assertEqual(agy?.secondaryWeeklyLabel, "Claude & GPT OSS Weekly", "antigravity fourth label")
    assertNil(agy?.extra, "antigravity fourth window is not a spend bar")

    // A non-Cursor vendor keeps the default time-window labels.
    assertEqual(cld?.sessionLabel, "Session", "anthropic keeps the Session label")
    assertEqual(cld?.weeklyTag, "7d", "anthropic keeps the 7d tag")
}

// ─── Run ─────────────────────────────────────────────────────────────────
func testOverviewHeadline() {
    print("overview headline (cursor combined, rate-limit worst window)")
    let app = AppDelegate()  // overviewHeadline reads only the snapshot
    // Anthropic-style: biggest of 5h / weekly / scoped model (Fable), not the first.
    let anthropic = Snapshot(
        plan: "Max", hasUsageWindows: true, creditBalance: nil,
        session: Window(pct: 30, reset: "3h", elapsed: nil),
        weekly: Window(pct: 55, reset: "5d", elapsed: nil),
        sonnet: Window(pct: 80, reset: "5d", elapsed: nil), sonnetLabel: "Fable", extra: nil)
    assertEqual(app.overviewHeadline(anthropic).pct, 80, "anthropic = biggest of 5h/weekly/Fable")
    let antigravity = Snapshot(
        plan: "Pro", hasUsageWindows: true, creditBalance: nil,
        session: Window(pct: 20, reset: "3h", elapsed: nil),
        weekly: Window(pct: 40, reset: "4d", elapsed: nil),
        sonnet: Window(pct: 60, reset: "2h", elapsed: nil),
        sonnetLabel: "Claude & GPT OSS", extra: nil,
        secondaryWeekly: Window(pct: 95, reset: "6d", elapsed: nil),
        secondaryWeeklyLabel: "Claude & GPT OSS")
    assertEqual(app.overviewHeadline(antigravity).pct, 95,
                "antigravity overview includes fourth window")
    // Cursor: the combined total, not the worse of the two pools.
    let cursor = Snapshot(
        plan: "Cursor Ultra", hasUsageWindows: true, creditBalance: nil,
        session: Window(pct: 98, reset: "12d", elapsed: nil),
        weekly: Window(pct: 100, reset: "12d", elapsed: nil),
        sonnet: nil, sonnetLabel: "", extra: nil, cursorTotalPct: 62)
    assertEqual(app.overviewHeadline(cursor).pct, 62, "cursor = combined total (not max pool)")
}

func testVendorCycle() {
    print("vendor swap cycle (⌥⌘\\)")
    let ids = ["anthropic", "cursor", "zai"]
    assertEqual(nextVendorId(current: "anthropic", in: ids) ?? "?", "cursor", "forward from first")
    assertEqual(nextVendorId(current: "zai", in: ids) ?? "?", "anthropic", "forward wraps to first")
    assertEqual(
        nextVendorId(current: "cursor", in: ids, forward: false) ?? "?", "anthropic", "backward")
    assertEqual(
        nextVendorId(current: "anthropic", in: ids, forward: false) ?? "?", "zai",
        "backward wraps to last")
    assertEqual(
        nextVendorId(current: "gone", in: ids) ?? "?", "anthropic",
        "an absent current starts at the first")
    assertEqual(nextVendorId(current: "x", in: []) == nil, true, "empty list yields nil")
    // The ring ends on the synthetic "overview" target, then wraps to the first.
    let ring = ["anthropic", "cursor", "overview"]
    assertEqual(nextVendorId(current: "cursor", in: ring) ?? "?", "overview", "last vendor → overview")
    assertEqual(nextVendorId(current: "overview", in: ring) ?? "?", "anthropic", "overview wraps to first")
}

func testClaudeAccounts() {
    // [[anthropic.accounts]] label extraction, in file order, ignoring other
    // sections/keys and both quote styles.
    let toml = """
    [ui]
    primary = "cursor"

    [anthropic]
    enabled = true
    show_default_account = false

    [[anthropic.accounts]]
    label = "struct"
    credentials_path = "~/x/struct/.credentials.json"

    [[anthropic.accounts]]
    label = 'gmail'
    credentials_path = "~/x/gmail/.credentials.json"

    [cursor]
    enabled = true
    """
    assertEqual(anthropicAccountLabels(inTOML: toml).joined(separator: ","),
                "struct,gmail", "labels in file order")
    assertEqual(anthropicAccountLabels(inTOML: "[anthropic]\nenabled = true").isEmpty, true,
                "no account blocks → no labels")
    // A `label` key outside an accounts block must not count.
    assertEqual(anthropicAccountLabels(inTOML: "[ui]\nlabel = \"nope\"").isEmpty, true,
                "label outside [[anthropic.accounts]] ignored")

    // Explicit wins over discovered on a clash; discovered appended sorted.
    assertEqual(mergedAccountLabels(explicit: ["b"], discovered: ["a", "b", "c"])
                    .joined(separator: ","),
                "b,a,c", "explicit first, clash dropped")

    // show_default_account semantics mirror Rust: ignored without accounts.
    assertEqual(showDefaultClaudeAccount(configValue: "false", hasAccounts: false), true,
                "no accounts → default always shown")
    assertEqual(showDefaultClaudeAccount(configValue: "false", hasAccounts: true), false,
                "false hides the default")
    assertEqual(showDefaultClaudeAccount(configValue: nil, hasAccounts: true), true,
                "omitted → shown")

    // accounts_dir discovery: every subdir is an account (Keychain-backed
    // logins need not have a .credentials.json), sorted.
    let fm = FileManager.default
    let dir = NSTemporaryDirectory() + "aiusagebar-tests-\(ProcessInfo.processInfo.processIdentifier)"
    defer { try? fm.removeItem(atPath: dir) }
    for sub in ["zeta", "alpha", "nofile"] {
        try? fm.createDirectory(atPath: "\(dir)/\(sub)", withIntermediateDirectories: true)
    }
    fm.createFile(atPath: "\(dir)/zeta/.credentials.json", contents: Data("{}".utf8))
    fm.createFile(atPath: "\(dir)/alpha/.credentials.json", contents: Data("{}".utf8))
    fm.createFile(atPath: "\(dir)/stray.json", contents: Data("{}".utf8))
    assertEqual(discoverAccountLabels(inDir: dir).joined(separator: ","),
                "alpha,nofile,zeta", "account subdirs, sorted")
    assertEqual(discoverAccountLabels(inDir: dir + "/missing").isEmpty, true,
                "missing dir → empty")

    // Pseudo-id mapping round-trip.
    assertEqual(accountLabel(of: "anthropic@gmail") ?? "?", "gmail", "label extracted")
    assertEqual(accountLabel(of: "anthropic") == nil, true, "base id has no label")
    assertEqual(baseVendorId("anthropic@gmail"), "anthropic", "account → base vendor")
    assertEqual(baseVendorId("cursor"), "cursor", "base id unchanged")
    assertEqual(vendorArgs(for: "anthropic@gmail").joined(separator: " "),
                "--vendor anthropic --account gmail", "account fetch args")
    assertEqual(vendorArgs(for: "zai").joined(separator: " "), "--vendor zai", "vendor fetch args")
    assertEqual(entryDisplayName("anthropic@gmail"), "Claude · gmail", "account display name")
    assertEqual(entryDisplayName("overview"), "Visão geral", "overview display name")

    let overviewEntries = [
        MenuEntry(id: "anthropic@struct", name: "Claude · struct"),
        MenuEntry(id: "anthropic@gmail", name: "Claude · gmail"),
        MenuEntry(id: "openai", name: "OpenAI"),
        MenuEntry(id: "cursor", name: "Cursor"),
    ]
    assertEqual(filterOverviewEntries(overviewEntries, requested: ["cursor", "anthropic"])
                    .map { $0.id }.joined(separator: ","),
                "cursor,anthropic@struct,anthropic@gmail",
                "overview config order includes every requested account")
    assertEqual(filterOverviewEntries(overviewEntries, requested: ["missing", "openai"])
                    .map { $0.id }.joined(separator: ","),
                "openai", "overview skips unavailable vendors")
    assertEqual(filterOverviewEntries(overviewEntries, requested: nil)
                    .map { $0.id }.joined(separator: ","),
                "anthropic@struct,anthropic@gmail,openai,cursor",
                "omitted overview list keeps all entries")

    // The swap ring cycles across accounts like any other entry.
    let ring = ["anthropic@struct", "anthropic@gmail", "cursor", "overview"]
    assertEqual(nextVendorId(current: "anthropic@struct", in: ring) ?? "?",
                "anthropic@gmail", "ring steps between accounts")
    assertEqual(nextVendorId(current: "overview", in: ring) ?? "?",
                "anthropic@struct", "ring wraps to the first account")
}

func testCompactToggle() {
    // Under the threshold → bars, unless Compactar forces the text mode.
    assertEqual(overviewUsesBars(count: 3, barsMax: 4, compact: false), true,
                "≤ barsMax without compact → bars")
    assertEqual(overviewUsesBars(count: 3, barsMax: 4, compact: true), false,
                "Compactar forces %-text even under the threshold")
    assertEqual(overviewUsesBars(count: 5, barsMax: 4, compact: false), false,
                "past the threshold → %-text regardless")
    assertEqual(overviewUsesBars(count: 4, barsMax: 4, compact: false), true,
                "boundary: exactly barsMax still draws bars")
}

func testShortReset() {
    assertEqual(shortReset("4d 1h"), "4d", "days+hours → leading days")
    assertEqual(shortReset("2h 05m"), "2h", "hours+minutes → leading hours")
    assertEqual(shortReset("0h 05m"), "5m", "under an hour → minutes, no leading zero")
    assertEqual(shortReset("0h 00m"), "0m", "zero minutes stays 0m")
    assertEqual(shortReset("now"), "now", "already reset")
    assertEqual(shortReset("—"), nil, "em-dash → nil")
    assertEqual(shortReset(""), nil, "empty → nil")
}

func testOverviewProviderToggle() {
    // The top-bar summary keeps only providers not toggled off, in order.
    let ids = ["anthropic@struct", "anthropic@gmail", "cursor"]
    assertEqual(overviewVisibleIds(ids, hidden: []), ids, "nothing hidden → all shown, in order")
    assertEqual(overviewVisibleIds(ids, hidden: ["anthropic@gmail"]),
                ["anthropic@struct", "cursor"], "a hidden provider drops out, order preserved")
    assertEqual(overviewVisibleIds(ids, hidden: Set(ids)), [], "all hidden → empty summary")
    assertEqual(overviewVisibleIds(ids, hidden: ["gone"]), ids, "a stale hidden id matches nothing")
}

func testSystemIntegrations() {
    print("launch agent + hot-key status")
    do {
        let executable = "/Applications/Test & Tools/ai-usagebar-menubar"
        let data = try launchAgentPlist(executable: executable)
        let object = try PropertyListSerialization.propertyList(from: data, options: [], format: nil)
        let plist = object as? [String: Any]
        assertEqual(plist?["Label"] as? String, LAUNCH_AGENT_LABEL,
                    "launch agent label")
        assertEqual((plist?["ProgramArguments"] as? [String])?.first, executable,
                    "launch agent executable is preserved")
        assertEqual(plist?["RunAtLoad"] as? Bool, true, "launch agent runs at login")
    } catch {
        print("  ✗ launch agent plist: \(error)")
        failures += 1
    }
    assertEqual(hotKeyRegistrationSucceeded(noErr), true, "noErr is registration success")
    assertEqual(hotKeyRegistrationSucceeded(OSStatus(-1)), false,
                "nonzero OSStatus is registration failure")
}

/// `account status --json` parsing and the two strings built from it. Every
/// field is optional in the schema, so the interesting cases are the missing
/// ones: a Mac with no Claude Desktop app, and an older binary that doesn't
/// know the subcommand at all.
func testAccountStatus() {
    let full = """
    {"desktop":{"available":true,"data_dir":"/d","profiles_dir":"/p",
      "active_label":"toptal","active_account_uuid":"u1",
      "profiles":[{"label":"gmail","active":false},{"label":"toptal","active":true}]},
     "cli":{"active_label":"struct","active_account_uuid":"u2",
      "accounts":[{"label":"struct","active":true},{"label":"toptal","active":false}]},
     "usage_accounts":[{"label":"struct","desktop":false},
                       {"label":"gmail","desktop":true}]}
    """
    let s = parseAccountStatus(Data(full.utf8))
    assertEqual(s?.desktopActive, "toptal", "desktop active label")
    assertEqual(s?.cliActive, "struct", "cli active label")
    assertEqual(s?.desktopLabels ?? [], ["gmail", "toptal"], "desktop labels keep file order")
    assertEqual(s?.cliLabels ?? [], ["struct", "toptal"], "cli labels keep file order")
    assertEqual(s?.usageAccounts ?? [],
                [UsageAccount(label: "struct", desktop: false),
                 UsageAccount(label: "gmail", desktop: true)],
                "usage accounts preserve Rust source selection")
    assertEqual(accountsSummaryLine(s!), "Desktop: toptal   ·   Code: struct", "summary line")

    // No Claude Desktop app: the whole half is null, the CLI half still shows.
    let linux = parseAccountStatus(Data("""
        {"desktop":null,"cli":{"active_label":"work","accounts":[{"label":"work"}]}}
        """.utf8))
    assertNil(linux?.desktopActive, "null desktop has no active label")
    assertEqual(linux?.desktopLabels ?? ["x"], [], "null desktop has no labels")
    assertEqual(accountsSummaryLine(linux!), "Code: work", "summary drops the absent half")

    // An account list with nobody active still renders, marked unknown.
    let orphan = parseAccountStatus(Data(#"{"cli":{"accounts":[{"label":"work"}]}}"#.utf8))
    assertEqual(accountsSummaryLine(orphan!), "Code: ?", "unknown active is shown, not hidden")
    assertEqual(orphan?.cliLabels ?? [], ["work"], "an unmatched active still lists its accounts")

    // A Mac with the app installed but nothing captured yet: no summary line,
    // but `desktopAvailable` keeps the submenu (and "Adicionar conta…") alive.
    let fresh = parseAccountStatus(Data(#"{"desktop":{"available":true,"profiles":[]}}"#.utf8))
    assertEqual(fresh?.desktopAvailable, true, "an empty profile list is still available")
    assertEqual(accountsSummaryLine(fresh!), "", "nothing captured renders no line")
    assertEqual(linux?.desktopAvailable, false, "null desktop is unavailable")

    // Nothing at all: no summary line, and no Desktop submenu.
    let bare = parseAccountStatus(Data("{}".utf8))
    assertEqual(bare?.desktopAvailable, false, "absent desktop key is unavailable")
    assertEqual(accountsSummaryLine(bare!), "", "empty status renders no line")

    // An older binary rejects the subcommand and prints usage on stderr, so
    // stdout is empty or not JSON — must be nil, never a crash.
    assertNil(parseAccountStatus(Data()), "empty output")
    assertNil(parseAccountStatus(Data("error: unrecognized subcommand".utf8)), "non-JSON output")
    assertNil(parseAccountStatus(Data("[1,2,3]".utf8)), "JSON that is not an object")

    // Routines deleted in one account but alive in another.
    let withConflicts = parseAccountStatus(Data("""
        {"desktop":{"available":true,"profiles":[{"label":"gmail"}],"deletion_conflicts":[
          {"key":"opaque-conflict-1","id":"t1","kind":"routine","summary":"0 5 * * *  (daily-report)","deleted_by":"hotmail",
           "still_in":["gmail","struct"]}]}}
        """.utf8))
    assertEqual(withConflicts?.deletionConflicts.count, 1, "conflict parsed")
    assertEqual(withConflicts?.deletionConflicts.first?.id, "t1", "conflict id")
    assertEqual(withConflicts?.deletionConflicts.first?.key, "opaque-conflict-1", "typed conflict key")
    assertEqual(withConflicts?.deletionConflicts.first?.line,
                "[routine] 0 5 * * *  (daily-report) — deleted in hotmail", "conflict line")
    // An older binary has no such key, and a status with none must stay empty
    // so the dialog never appears for nothing.
    assertEqual(withConflicts.map { _ in fresh?.deletionConflicts.isEmpty }, true,
                "absent deletion_conflicts is empty")

    let many = (1...12).map {
        DeletionConflict(key: "opaque-\($0)", id: "t\($0)", kind: "chat", summary: "s\($0)",
                         deletedBy: "b", stillIn: ["a"])
    }
    assertEqual(conflictPreview(many).components(separatedBy: "\n").count, 11,
                "preview caps at 10 plus a summary line")
    assertEqual(conflictPreview(many).hasSuffix("… e mais 2"), true, "preview counts the rest")
    assertEqual(conflictPreview(Array(many.prefix(3))).contains("e mais"), false,
                "a short list is not summarised")

    assertEqual(switchArgs(label: "work", desktop: true,
                           deleting: ["opaque-1", "opaque-2"]),
                ["account", "switch", "work", "--desktop", "-y",
                 "--delete-conflict", "opaque-1",
                 "--delete-conflict", "opaque-2"],
                "confirmed deletions are passed through")
    assertEqual(switchArgs(label: "work", desktop: true),
                ["account", "switch", "work", "--desktop", "-y"], "desktop switch args")
    assertEqual(switchArgs(label: "work", desktop: false),
                ["account", "switch", "work", "--cli", "-y"], "cli switch args")

    // The add script is shell text, so a label from a free-text field must not
    // be able to end the quoted argument and run something else.
    let script = addAccountScript(binary: "/usr/local/bin/ai-usagebar", label: "work", desktop: true)
    assertEqual(script.contains("'/usr/local/bin/ai-usagebar' account add 'work' --desktop"), true,
                "desktop add command")
    assertEqual(addAccountScript(binary: "/b", label: "w", desktop: false)
                    .contains("'/b' account add 'w'\n"), true, "cli add command has no --desktop")
    let nasty = addAccountScript(binary: "/b", label: "a'; rm -rf ~; echo '", desktop: false)
    assertEqual(nasty.contains("rm -rf ~;\n"), false, "injected command stays inside the quotes")
    assertEqual(nasty.contains(#"'a'\''; rm -rf ~; echo '\'''"#), true, "label is escaped")
}

func testDesktopAccounts() {
    // A Desktop account id round-trips through accountLabel (so display, dedup
    // and baseVendorId treat it as Claude) but is flagged for --desktop.
    let id = DESKTOP_ACCOUNT_ID_PREFIX + "gmail"
    assertEqual(accountLabel(of: id), "gmail", "desktop id yields its label")
    assertEqual(baseVendorId(id), "anthropic", "desktop id is an anthropic entry")
    assertEqual(isDesktopAccountId(id), true, "desktop id detected")
    assertEqual(isDesktopAccountId(ACCOUNT_ID_PREFIX + "gmail"), false, "cli id is not desktop")

    // vendorArgs adds --desktop only for a desktop account.
    assertEqual(vendorArgs(for: id), ["--vendor", "anthropic", "--account", "gmail", "--desktop"],
                "desktop account passes --desktop")
    assertEqual(vendorArgs(for: ACCOUNT_ID_PREFIX + "gmail"),
                ["--vendor", "anthropic", "--account", "gmail"],
                "cli account omits --desktop")

    let shared = claudeAccountMenuEntries([
        UsageAccount(label: "work", desktop: true),
        UsageAccount(label: "personal", desktop: false),
    ])
    assertEqual(shared.map { $0.id },
                [DESKTOP_ACCOUNT_ID_PREFIX + "work", ACCOUNT_ID_PREFIX + "personal"],
                "menu uses the source selected by Rust status")
}

@main
struct TestRunner {
    static func main() {
        testRingArc()
        testTomlParsing()
        testDefaultEnabled()
        testParserBalances()
        testOverviewHeadline()
        testVendorCycle()
        testClaudeAccounts()
        testDesktopAccounts()
        testCompactToggle()
        testShortReset()
        testOverviewProviderToggle()
        testAccountStatus()
        testSystemIntegrations()
        if failures > 0 {
            print("\n\(failures) test(s) FAILED")
            exit(1)
        }
        print("\nall tests passed")
    }
}
