#![cfg(test)]

use std::collections::HashMap;
use std::path::Path;
use sup::config::{self, AuthMode};

fn env() -> HashMap<String, String> {
    HashMap::new()
}

fn raw(text: &str) -> config::Config {
    toml::from_str(text).unwrap()
}

fn load_text(text: &str) -> Result<config::LoadedConfig, String> {
    let parsed = toml::from_str(text).map_err(|error| format!("{error}"))?;
    config::normalize(parsed, Path::new("/h"), &env(), "fixture".into())
}

#[test]
fn shipped_defaults_are_versioned_and_homebrew_is_on_demand() {
    let loaded = config::load(
        None,
        Path::new("/tmp/home"),
        &HashMap::from([("HOME".into(), "/tmp/home".into())]),
    )
    .unwrap();
    assert_eq!(loaded.source, "embedded defaults");
    assert_eq!(
        loaded.jobs.iter().find(|job| job.name == "brew-upgrade").unwrap().auth,
        AuthMode::OnDemand
    );
}

#[test]
fn expansion_matches_safe_substitute_cases() {
    let mut vars = env();
    vars.insert("X".into(), "value".into());
    let text = "version=1\n[[jobs]]\nname='job'\nphase='core'\ncommand=['echo', '$X ${X} $$ $MISSING $1 $ ${X:-default} $é ${é}']\nrequired_paths=['$X/$UNKNOWN']\nrequired_commands=['$X']\n";
    let loaded = config::normalize(raw(text), Path::new("/h"), &vars, "fixture".into()).unwrap();
    assert_eq!(
        loaded.jobs[0].command[1],
        "value value $ $MISSING $1 $ ${X:-default} $é ${é}"
    );
    assert_eq!(loaded.jobs[0].required_paths[0], Path::new("value/$UNKNOWN"));
    assert_eq!(loaded.jobs[0].required_commands, ["$X"]);
}

#[test]
fn rejects_toml_legacy_boolean_spellings_and_migration_fields() {
    assert!(load_text("version=1\n[[jobs]]\nname='job'\nphase='core'\ncommand=['echo']\noptional='yes'\n").is_err());
    assert!(
        load_text("version=1\n[[jobs]]\nname='job'\nphase='core'\ncommand=['echo']\nsudo_preflight=false\n").is_err()
    );
}

#[test]
fn validates_names_alias_collisions_and_references() {
    let collision = "version=1\n[aliases]\na=['job']\n[[jobs]]\nname='a'\nphase='core'\ncommand=['echo']\n";
    assert!(load_text(collision).unwrap_err().contains("collides"));
    let unknown = "version=1\n[aliases]\na=['missing']\n[[jobs]]\nname='job'\nphase='core'\ncommand=['echo']\n";
    assert!(load_text(unknown).unwrap_err().contains("unknown job"));
    assert!(load_text("version=1\n[[jobs]]\nname='bad name'\nphase='core'\ncommand=['echo']\n").is_err());
}

#[test]
fn validates_log_names_for_traversal_and_duplicates() {
    let traversal = "version=1\n[[jobs]]\nname='one'\nphase='core'\ncommand=['echo']\nlog_name='../outside.log'\n";
    assert!(load_text(traversal).unwrap_err().contains("simple filename"));
    let duplicate = "version=1\n[[jobs]]\nname='one'\nphase='core'\ncommand=['echo']\nlog_name='same.log'\n[[jobs]]\nname='two'\nphase='core'\ncommand=['echo']\nlog_name='same.log'\n";
    assert!(load_text(duplicate).unwrap_err().contains("duplicate log"));
}

#[test]
fn rejects_unknown_fields_types_versions_phases_and_empty_executables() {
    assert!(
        load_text("version=2\n[[jobs]]\nname='job'\nphase='core'\ncommand=['echo']\n")
            .unwrap_err()
            .contains("version")
    );
    assert!(
        toml::from_str::<config::Config>(
            "version=1\nunknown=1\n[[jobs]]\nname='job'\nphase='core'\ncommand=['echo']\n"
        )
        .is_err()
    );
    assert!(load_text("version=1\n[[jobs]]\nname='job'\nphase='wat'\ncommand=['echo']\n").is_err());
    assert!(load_text("version=1\n[[jobs]]\nname='job'\nphase='core'\ncommand=['']\n").is_err());
    assert!(load_text("version=1\n[[jobs]]\nname='job'\nphase='core'\ncommand=[]\n").is_err());
    let accepts_empty_arg = load_text("version=1\n[[jobs]]\nname='job'\nphase='core'\ncommand=['echo', '']\n").unwrap();
    assert_eq!(accepts_empty_arg.jobs[0].command, ["echo", ""]);
}

#[test]
fn yaml_migration_changes_only_direct_brew_policy_and_preserves_literal_values() {
    let text = "jobs:\n  - name: brew-upgrade\n    phase: core\n    command: [brew, upgrade, '$HOME']\n    required_commands: ['$TOOL']\n    required_paths: ['$HOME/file']\n    sudo_preflight: true\n  - name: unrelated\n    phase: parallel\n    command: [echo, '$HOME']\n    sudo_preflight: true\n";
    let (migrated, notices) = config::migrate_yaml(text, Path::new("/h"), &env()).unwrap();
    let migrated_config: config::Config = toml::from_str(&migrated).unwrap();
    assert_eq!(migrated_config.jobs[0].command, ["brew", "upgrade", "$HOME"]);
    assert_eq!(migrated_config.jobs[0].required_commands, ["$TOOL"]);
    assert_eq!(migrated_config.jobs[0].required_paths, ["$HOME/file"]);
    assert!(migrated.contains("auth = \"on-demand\""));
    assert!(notices.iter().any(|notice| notice.contains("brew-upgrade")));
    assert_eq!(migrated_config.jobs[1].auth, AuthMode::Preflight);
}

#[test]
fn explicit_false_preflight_is_eliminated_and_round_trips() {
    let source = "jobs:\n  - name: job\n    phase: core\n    command: [echo, hi]\n    sudo_preflight: false\n";
    let (migrated, _) = config::migrate_yaml(source, Path::new("/h"), &env()).unwrap();
    assert!(!migrated.contains("sudo_preflight"));
    let loaded = config::load(Some(Path::new("/does/not/exist")), Path::new("/h"), &env());
    assert!(loaded.is_err());
    let parsed: config::Config = toml::from_str(&migrated).unwrap();
    let loaded = config::normalize(parsed, Path::new("/h"), &env(), "migrated".into()).unwrap();
    assert_eq!(loaded.jobs[0].auth, AuthMode::None);
}

#[test]
fn accepts_supported_legacy_boolean_spellings_and_rejects_other_values() {
    for (value, expected) in [
        ("true", true),
        ("yes", true),
        ("on", true),
        ("false", false),
        ("no", false),
        ("off", false),
    ] {
        let text = format!("jobs:\n  - name: job\n    phase: core\n    command: [echo, hi]\n    optional: {value}\n");
        let (migrated, _) = config::migrate_yaml(&text, Path::new("/h"), &env()).unwrap();
        let parsed: config::Config = toml::from_str(&migrated).unwrap();
        assert_eq!(parsed.jobs[0].optional, expected, "{value}");
    }
    let invalid = "jobs:\n  - name: job\n    phase: core\n    command: [echo, hi]\n    optional: maybe\n";
    assert!(config::migrate_yaml(invalid, Path::new("/h"), &env()).is_err());
}

#[test]
fn yaml_rejects_duplicates_unsupported_tags_and_unknown_fields() {
    let duplicate = "jobs:\n  - name: one\n    name: two\n    phase: core\n    command: [echo, hi]\n";
    assert!(config::migrate_yaml(duplicate, Path::new("/h"), &env()).is_err());
    let tagged = "jobs: !ApplicationTag []\n";
    assert!(config::migrate_yaml(tagged, Path::new("/h"), &env()).is_err());
    let unknown = "jobs:\n  - name: job\n    phase: core\n    command: [echo]\n    mystery: 1\n";
    assert!(config::migrate_yaml(unknown, Path::new("/h"), &env()).is_err());
}

#[test]
fn yaml_accepts_bounded_anchor_and_merge_alias() {
    let anchored = "jobs:\n  - &defaults\n    name: template\n    phase: core\n    command: [echo, hi]\n  - <<: *defaults\n    name: one\n";
    let (toml, _) = config::migrate_yaml(anchored, Path::new("/h"), &env()).unwrap();
    let parsed: config::Config = toml::from_str(&toml).unwrap();
    assert_eq!(parsed.jobs[1].name, "one");
    assert_eq!(parsed.jobs[1].command, ["echo", "hi"]);
}

#[test]
fn selectors_deduplicate_split_commas_skip_wins_and_keep_config_order() {
    let loaded = load_text("version=1\n[[jobs]]\nname='first'\nphase='core'\ncommand=['echo']\n[[jobs]]\nname='second'\nphase='core'\ncommand=['echo']\n[[jobs]]\nname='third'\nphase='core'\ncommand=['echo']\n").unwrap();
    let aliases = HashMap::from([("all".to_owned(), vec!["first".to_owned(), "second".to_owned()])]);
    let selected = config::selectors(&loaded.jobs, &aliases, &["all,third,all".into()], &["first".into()]).unwrap();
    assert_eq!(
        selected.iter().map(|job| job.name.as_str()).collect::<Vec<_>>(),
        ["second", "third"]
    );
    assert!(config::selectors(&loaded.jobs, &aliases, &["missing".into()], &[]).is_err());
    assert!(
        config::selectors(&loaded.jobs, &aliases, &["first".into()], &["first".into()])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn config_lookup_uses_xdg_home_and_never_cwd_fallback() {
    let empty = HashMap::new();
    assert!(config::resolve_path(None, &empty).unwrap_err().contains("HOME"));
    assert_eq!(
        config::resolve_path(Some(Path::new("explicit.toml")), &empty).unwrap(),
        (Path::new("explicit.toml").to_path_buf(), true)
    );
    let xdg = HashMap::from([("XDG_CONFIG_HOME".into(), "/xdg".into())]);
    assert_eq!(
        config::resolve_path(None, &xdg).unwrap().0,
        Path::new("/xdg/sup/config.toml")
    );
    let home = HashMap::from([("HOME".into(), "/home/user".into())]);
    assert_eq!(
        config::resolve_path(None, &home).unwrap().0,
        Path::new("/home/user/.config/sup/config.toml")
    );
}

#[test]
fn load_prefers_xdg_config_and_reports_explicit_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let xdg = dir.path().join("xdg");
    let home = dir.path().join("home");
    std::fs::create_dir_all(xdg.join("sup")).unwrap();
    std::fs::create_dir_all(home.join(".config/sup")).unwrap();
    let configured = "version=1\n[[jobs]]\nname='xdg-job'\nphase='core'\ncommand=['echo']\n";
    let ignored_home = "version=1\n[[jobs]]\nname='home-job'\nphase='core'\ncommand=['echo']\n";
    std::fs::write(xdg.join("sup/config.toml"), configured).unwrap();
    std::fs::write(home.join(".config/sup/config.toml"), ignored_home).unwrap();
    let variables = HashMap::from([
        ("XDG_CONFIG_HOME".into(), xdg.display().to_string()),
        ("HOME".into(), home.display().to_string()),
    ]);
    let loaded = config::load(None, &home, &variables).unwrap();
    assert!(loaded.source.ends_with("xdg/sup/config.toml"));
    assert_eq!(loaded.jobs[0].name, "xdg-job");

    let missing = dir.path().join("missing.toml");
    let error = config::load(Some(&missing), &home, &variables).unwrap_err();
    assert!(error.contains("cannot read config"));
    assert!(error.contains("missing.toml"));
}
