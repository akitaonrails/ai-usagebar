// Renders the real UsageBar.qml offscreen and asserts what it actually paints.
//
// Possible because UsageBar.qml never touches the Plasmoid attached property —
// unlike main.qml, which the applet host injects it into and which therefore
// genuinely cannot be instantiated here.
//
// `visible` is deliberately NOT asserted: nothing is inside a shown window, so
// every item reports visible=false regardless of its binding. Width is the
// honest proxy — a zero-width segment paints nothing either way.
import QtQuick
import QtTest
import "../package/contents/ui" as Ui

TestCase {
    id: root
    name: "UsageBar"
    when: windowShown

    // One Dark, the five the other frontends default to (src/theme.rs).
    readonly property var palette: ({
        low: "#98c379", mid: "#e5c07b", high: "#d19a66",
        critical: "#e06c75", empty: "#5c6370",
    })

    Ui.UsageBar {
        id: bar
        width: 200
        height: 10
        colors: root.palette
        pct: 0
        elapsed: -1
    }

    function parts() {
        var track = bar.children[0];
        return {track: track, marker: bar.children[1],
                pre: track.children[0], post: track.children[1]};
    }

    function test_01_structure() {
        var p = parts();
        compare(bar.children.length, 2, "root holds the track and the marker");
        compare(p.track.children.length, 2, "the fill is TWO segments, not one");
        compare(p.track.width, 200, "track spans the widget");
    }

    function test_02_no_marker_single_fill() {
        bar.pct = 50; bar.elapsed = -1;
        var p = parts();
        compare(bar.hasMarker, false, "no marker when elapsed is unknown");
        compare(p.pre.width, 100, "fill is pct% of the track");
        compare(p.post.width, 0, "nothing painted past a marker that is not there");
        compare(String(p.pre.color), "#e5c07b", "50% is the mid band in colorForPct");
    }

    function test_03_under_pace_single_colour() {
        bar.pct = 30; bar.elapsed = 60;
        var p = parts();
        compare(bar.hasMarker, true);
        compare(p.pre.width, 60, "the whole fill sits before the marker");
        compare(p.post.width, 0, "no overshoot segment while under pace");
    }

    // The regression this file exists for. A single-rectangle bar repainted the
    // WHOLE fill in the pace colour once usage passed the marker; Waybar and
    // GNOME keep everything before the marker in the absolute-usage colour and
    // give only the tail the pace colour.
    function test_04_ahead_of_pace_two_colours() {
        bar.pct = 62; bar.elapsed = 40;
        var p = parts();
        compare(p.pre.width, 80, "up to the marker: 40% of 200");
        compare(p.post.x, 80, "the overshoot starts at the marker");
        compare(p.post.width, 44, "and runs to the fill edge: (62-40)% of 200");
        verify(String(p.pre.color) !== String(p.post.color),
               "the two segments must NOT share a colour — that was the bug");
        compare(String(p.pre.color), "#e5c07b", "62% absolute is the mid band");
        compare(String(p.post.color), "#e06c75", "22 points ahead of the clock is critical");
    }

    function test_05_marker_is_the_shared_blue() {
        bar.pct = 62; bar.elapsed = 40;
        var p = parts();
        compare(String(p.marker.color), "#61afef",
                "the pace marker is fixed across all three frontends");
        compare(p.marker.x, 80, "marker sits at elapsed% of the track");
    }

    function test_06_segments_tile_the_fill() {
        for (var pct = 0; pct <= 100; pct += 5) {
            for (var el = 0; el <= 100; el += 10) {
                bar.pct = pct; bar.elapsed = el;
                var p = parts();
                var painted = p.pre.width + p.post.width;
                var want = Math.round(200 * pct / 100);
                verify(Math.abs(painted - want) <= 1,
                       "pct=" + pct + " el=" + el + ": painted " + painted + " want " + want);
                verify(p.pre.width >= 0 && p.post.width >= 0, "no negative segment");
                verify(p.post.x + p.post.width <= 201, "must not overflow the track");
            }
        }
    }

    function test_07_clamps_out_of_range() {
        bar.pct = 140; bar.elapsed = -1;
        compare(parts().pre.width, 200, "over 100% cannot overflow the track");
        bar.pct = -40;
        compare(parts().pre.width, 0, "negative cannot paint backwards");
    }

    // A theme switch must recolour without reinstantiating anything: the panel
    // has to follow Breeze Light/Dark live, and no One Dark grey may survive.
    function test_08_repaints_on_a_palette_change() {
        bar.pct = 62; bar.elapsed = 40;
        compare(String(parts().pre.color), "#e5c07b");
        bar.colors = {low: "#007700", mid: "#0000ff", high: "#884400",
                      critical: "#ff0000", empty: "#eeeeee"};
        compare(String(parts().pre.color), "#0000ff", "the fill must follow the new palette");
        compare(String(parts().post.color), "#ff0000");
        compare(String(parts().marker.color), "#61afef",
                "...but the pace marker stays fixed, by design");
        bar.colors = root.palette;
    }
}
