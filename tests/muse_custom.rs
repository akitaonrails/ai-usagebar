//! The shipped Muse recipe must consume the bundled counter's public shape.
use ai_usagebar::{config::Config, custom::mapping};
use serde_json::json;

#[test]
fn muse_recipe_projects_local_counters_without_inventing_quota() {
    let example = include_str!("../config.example.toml");
    let start = example
        .find("# Recipe: Muse Code local token usage.")
        .unwrap();
    let end = example[start..].find("\n# Windows/macOS tray.").unwrap() + start;
    let recipe = example[start..end]
        .lines()
        .skip_while(|line| *line != "# [[custom]]")
        .map(|line| line.strip_prefix("# ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, recipe).unwrap();
    let config = Config::load_from(&path).unwrap();
    let spec = config.custom_by_id("muse").unwrap();
    assert!(spec.enabled);
    assert_eq!(spec.resolve_api_key().unwrap(), "local");
    assert_eq!(spec.url, "http://127.0.0.1:41837/usage");
    let body = json!({
        "tokens": {
            "today": {"input_tokens": 100, "output_tokens": 20, "cached_tokens": 80},
            "last30": {"input_tokens": 200, "output_tokens": 40, "cached_tokens": 160}
        },
        "calls": {"today": 1, "last30": 2},
        "status": "Recorded local completions only"
    });
    let snapshot = mapping::project(&body, spec).unwrap();
    assert!(
        snapshot.metrics.is_empty(),
        "local counts have no quota denominator"
    );
    assert_eq!(snapshot.texts.len(), 7);
    assert_eq!(snapshot.texts[0].value, "100");
    assert_eq!(snapshot.texts[1].value, "20");
    assert_eq!(snapshot.texts[2].value, "80");
    assert_eq!(snapshot.texts[3].value, "1");
    assert_eq!(snapshot.texts[4].value, "200");
    assert_eq!(snapshot.texts[5].value, "40");
    assert_eq!(snapshot.texts[6].value, body["status"].as_str().unwrap());
}
