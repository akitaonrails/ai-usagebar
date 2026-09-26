//! Embedded provider SVGs shared with the popover's provider icon catalog.

const ANTHROPIC: &[u8] = include_bytes!("../../windows/popover/src/icons/providers/anthropic.svg");
const ANTHROPIC_API: &[u8] =
    include_bytes!("../../windows/popover/src/icons/providers/anthropic_api.svg");
const ANTIGRAVITY: &[u8] =
    include_bytes!("../../windows/popover/src/icons/providers/antigravity.svg");
const COPILOT: &[u8] = include_bytes!("../../windows/popover/src/icons/providers/copilot.svg");
const CURSOR: &[u8] = include_bytes!("../../windows/popover/src/icons/providers/cursor.svg");
const DEEPSEEK: &[u8] = include_bytes!("../../windows/popover/src/icons/providers/deepseek.svg");
const GROK: &[u8] = include_bytes!("../../windows/popover/src/icons/providers/grok.svg");
const GROKBOT: &[u8] = include_bytes!("../../windows/popover/src/icons/providers/grokbot.svg");
const KIMI: &[u8] = include_bytes!("../../windows/popover/src/icons/providers/kimi.svg");
const MINIMAX: &[u8] = include_bytes!("../../windows/popover/src/icons/providers/minimax.svg");
const MOONSHOT: &[u8] = include_bytes!("../../windows/popover/src/icons/providers/moonshot.svg");
const OPENAI: &[u8] = include_bytes!("../../windows/popover/src/icons/providers/openai.svg");
const OPENCODE_GO: &[u8] =
    include_bytes!("../../windows/popover/src/icons/providers/opencode_go.svg");
const OPENROUTER: &[u8] =
    include_bytes!("../../windows/popover/src/icons/providers/openrouter.svg");
const ZAI: &[u8] = include_bytes!("../../windows/popover/src/icons/providers/zai.svg");

/// Find the provider mark for a slug or `provider@account` entry id.
///
/// The slug is lowercased like `vendorSlug` in the popover model. SuperGrok
/// shares xAI's Grok mark because it has no separate SVG.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub(super) fn mark_svg(slug_or_id: &str) -> Option<&'static [u8]> {
    let slug = slug_or_id
        .split('@')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match slug.as_str() {
        "anthropic" => Some(ANTHROPIC),
        "anthropic_api" => Some(ANTHROPIC_API),
        "antigravity" => Some(ANTIGRAVITY),
        "copilot" => Some(COPILOT),
        "cursor" => Some(CURSOR),
        "deepseek" => Some(DEEPSEEK),
        "grok" | "supergrok" => Some(GROK),
        "grokbot" => Some(GROKBOT),
        "kimi" => Some(KIMI),
        "minimax" => Some(MINIMAX),
        "moonshot" => Some(MOONSHOT),
        "openai" => Some(OPENAI),
        "opencode_go" => Some(OPENCODE_GO),
        "openrouter" => Some(OPENROUTER),
        "zai" => Some(ZAI),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::mark_svg;

    #[test]
    fn looks_up_known_unknown_supergrok_and_account_scoped_marks() {
        assert!(mark_svg("anthropic").is_some());
        assert!(mark_svg("unknown-provider").is_none());
        assert_eq!(mark_svg("supergrok"), mark_svg("grok"));
        assert_eq!(mark_svg("anthropic@work"), mark_svg("anthropic"));
    }
}
