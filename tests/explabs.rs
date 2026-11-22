use ai_usagebar::config::Config;
use ai_usagebar::vendor::{AuthKind, VendorId};
use ai_usagebar::widget::cli::Cli;
use clap::Parser;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn explabs_is_opt_in_and_resolves_an_inline_key() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(
        &path,
        "[explabs]\nenabled = true\napi_key_env = \"\"\napi_key = \"test-key\"\n",
    )
    .unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let vendor = VendorId::ExperientialLabs;
    let config = Config::load_from(&path).unwrap();
    assert!(config.is_enabled(vendor));
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        config.resolve_account_api_key_for(vendor, None).unwrap(),
        "test-key"
    );
    assert!(!Config::default().is_enabled(vendor));
    assert_eq!(Config::default().api_key_env_for(vendor), "EXPLABS_API_KEY");
    assert_eq!(vendor.auth_kind(), AuthKind::ApiKey);

    let cli = Cli::try_parse_from(["ai-usagebar", "--vendor", "explabs"]).unwrap();
    assert_eq!(cli.vendor.unwrap().to_id(), vendor);
}
