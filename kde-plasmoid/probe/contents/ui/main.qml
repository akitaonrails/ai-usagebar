// TEMPORARY Fase 0 probe — replaced by the real applet in Fase 1.
//
// Proves the .mjs code-sharing contract inside a real KPackage, loaded by a real
// Plasma applet host. Everything here also runs under Node via
// kde-plasmoid/plasmoid-logic.test.mjs; the point of this file is the checks
// that Node CANNOT catch, because QML's V4 engine differs from V8:
//   - V4 rejects the ES2019 optional catch binding (`catch {`) outright, and
//   - V4 silently evaluates Unicode property escapes (\p{L}) to false.
// The second is the dangerous one: no error, just wrong answers.
//
// Run: QT_LOGGING_RULES="qml.debug=true" plasmoidviewer -a kde-plasmoid/package
import QtQuick
import org.kde.plasma.plasmoid
import org.kde.plasma.plasma5support as Plasma5Support
import "../../../package/contents/code/marker-logic.mjs" as Marker
import "../../../package/contents/code/plasmoid-logic.mjs" as Logic

PlasmoidItem {
    id: probe

    // buildCommand() single-quotes every argument, including the timeout(1)
    // wrapper. Node can assert the STRING; only a real applet host can prove
    // KShell::splitArgs(AbortOnMeta) parses it and KProcess runs it. /bin/echo
    // stands in for the binary so this touches no network and no credential.
    Plasma5Support.DataSource {
        id: runner
        engine: "executable"
        connectedSources: []
        onNewData: (sourceName, data) => {
            disconnectSource(sourceName);
            const code = data["exit code"];
            const out = String(data["stdout"] || "").trim();
            const want = "--vendor zai --format PROBE";
            if (code === 0 && out === want)
                console.log("ok   timeout wrapper runs through the executable engine");
            else
                console.log("FAIL timeout wrapper: exit=" + code + " stdout=" + JSON.stringify(out)
                            + " want " + JSON.stringify(want));
            console.log("MJS PROBE EXEC DONE");
        }
    }

    Component.onCompleted: {
        let failures = 0;
        function check(label, actual, expected) {
            if (JSON.stringify(actual) === JSON.stringify(expected)) {
                console.log("ok   " + label + " = " + JSON.stringify(actual));
            } else {
                failures++;
                console.log("FAIL " + label + ": got " + JSON.stringify(actual)
                            + " want " + JSON.stringify(expected));
            }
        }

        // marker-logic.mjs — the canonical shared file, imported unmodified.
        check("FIELD.vendorShort (Object.freeze)", Marker.FIELD.vendorShort, 16);
        check("colorForPct(95)",
              Marker.colorForPct(95, {low: "l", mid: "m", high: "h", critical: "c", empty: "e"}),
              "c");
        check("plainTextFromPango", Marker.plainTextFromPango("<span>A &amp; B</span>"), "A & B");
        check("integer('27 ⏸') rejects stale suffix", Marker.integer("27 ⏸"), null);
        // The regression that made every Antigravity pool tag render empty.
        check("poolTag", Marker.poolTag("Gemini"), "G");
        check("poolTag strips leading punctuation", Marker.poolTag("  &claude"), "C");
        check("poolTag keeps a non-BMP letter whole",
              Array.from(Marker.poolTag("𐐨elta")).length, 1);
        check("disambiguateTags", Marker.disambiguateTags("Gemini", "GPT OSS"), ["GE", "GP"]);

        // plasmoid-logic.mjs — proves a .mjs importing another .mjs resolves.
        check("nextVendor wraps", Logic.nextVendor(["a", "b", "c"], "c", 1), "a");
        check("shellQuote escapes", Logic.shellQuote("it's"), "'it'\\''s'");
        check("buildArgv always has --vendor",
              Logic.buildArgv("", "zai", "F").indexOf("--vendor") >= 0, true);
        check("parseWidgetJson rejects garbage", Logic.parseWidgetJson("nope").ok, false);
        check("paletteFromTheme maps critical",
              Logic.paletteFromTheme({textColor: "t", neutralTextColor: "n",
                                      negativeTextColor: "g", disabledTextColor: "d"}).critical,
              "g");

        check("buildArgv wraps in timeout when given a timeout",
              Logic.buildArgv("b", "v", "f", 60).slice(0, 4), ["timeout", "-k", "5", "60"]);

        console.log(failures === 0 ? "MJS PROBE OK" : "MJS PROBE FAILED (" + failures + ")");

        // Fire the real thing last, so the synchronous checks are already
        // reported even if the engine never calls back.
        runner.connectSource(Logic.buildCommand("/bin/echo", "zai", "PROBE", 60));
    }
}
