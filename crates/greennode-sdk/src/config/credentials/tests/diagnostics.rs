use super::*;

#[test]
fn diagnostic_producers_expose_only_fixed_labels_and_reasons() {
    use crate::config::mode;
    let fixture = Fixture::new("[default]\ninvalid line\n");
    let source = Map::default();
    let settings = settings("default");
    let selection = fixture.selection(Mode::Greennode);
    let parse = select(
        &selection,
        &settings,
        &SettingsOptions::default(),
        &Options::default(),
        &source,
        Some(fixture.file()),
    )
    .unwrap_err();
    let missing = mode::select(
        None,
        Some(&fixture.0.join("missing-canary")),
        &source,
        None,
        mode::probe,
    )
    .unwrap_err();
    let config_path = fixture.0.join("config-canary");
    std::fs::File::create(&config_path)
        .unwrap()
        .set_len(1_048_577)
        .unwrap();
    let mut config_selection = fixture.selection(Mode::Greennode);
    config_selection.paths.config = Some(config_path);
    config_selection.paths.config_named = true;
    config_selection.config_present = true;
    config_selection.config_source = Some("option");
    let config_open = crate::config::file::open_pair(&config_selection)
        .err()
        .unwrap();
    let mut credentials_selection = fixture.selection(Mode::Greennode);
    credentials_selection.paths.credentials = Some(fixture.0.join("missing-canary"));
    let credentials_open = crate::config::file::open_pair(&credentials_selection)
        .err()
        .unwrap();
    let expected = [
        (
            parse,
            crate::FileSlot::Credentials,
            "line has no '='",
            "~/.greennode",
        ),
        (missing, crate::FileSlot::Credentials, "not found", "option"),
        (
            config_open,
            crate::FileSlot::Config,
            "file too large",
            "option",
        ),
        (
            credentials_open,
            crate::FileSlot::Credentials,
            "open failed",
            "~/.greennode",
        ),
    ];
    for (error, slot, reason, source) in expected {
        let diagnostic = error.detail::<FileDiagnostic>().unwrap();
        assert_eq!(diagnostic.slot(), slot);
        assert_eq!(diagnostic.source(), source);
        assert_eq!(diagnostic.reason(), reason);
        assert!(!format!("{diagnostic:?}").contains("canary"));
        assert!(!diagnostic.reason().contains('/'));
    }
}
