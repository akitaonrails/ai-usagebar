import QtQuick
import org.kde.kirigami as Kirigami

// Severity colors, derived from the active color scheme so the applet follows
// light/dark themes instead of hardcoding a palette.
QtObject {
    function forSeverity(severity) {
        switch (severity) {
        case "low":
            return Kirigami.Theme.positiveTextColor
        case "mid":
            return Kirigami.Theme.neutralTextColor
        case "high":
            // Kirigami.ColorUtils.linearInterpolation does not blend these two
            // the way a plain channel mix does (it lands on a complementary
            // hue), so mix the channels here instead.
            return mix(Kirigami.Theme.neutralTextColor, Kirigami.Theme.negativeTextColor, 0.5)
        case "critical":
            return Kirigami.Theme.negativeTextColor
        default:
            return Kirigami.Theme.highlightColor
        }
    }

    function mix(from, to, ratio) {
        return Qt.rgba(from.r + (to.r - from.r) * ratio,
                       from.g + (to.g - from.g) * ratio,
                       from.b + (to.b - from.b) * ratio,
                       from.a + (to.a - from.a) * ratio)
    }

    function dimmed(alpha) {
        return Qt.rgba(Kirigami.Theme.textColor.r,
                       Kirigami.Theme.textColor.g,
                       Kirigami.Theme.textColor.b,
                       alpha)
    }
}
