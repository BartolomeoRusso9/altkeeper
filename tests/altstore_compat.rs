use isideload::sideload::application::{Application, SpecialApp};

fn write_bundle(path: &std::path::Path, id: &str) {
    std::fs::create_dir_all(path).unwrap();
    let mut info = plist::Dictionary::new();
    info.insert("CFBundleIdentifier".into(), id.into());
    plist::to_file_xml(path.join("Info.plist"), &info).unwrap();
}

#[test]
fn altstore_host_and_widget_keep_one_prefix_after_resigning() {
    for original in ["com.rileytestut.AltStore", "com.OLDTEAM123.com.rileytestut.AltStore"] {
        let dir = tempfile::tempdir().unwrap();
        let app_dir = dir.path().join("AltStore.app");
        write_bundle(&app_dir, original);
        write_bundle(&app_dir.join("PlugIns/AltWidget.appex"), &format!("{original}.AltWidget"));
        let mut app = Application::new(app_dir).unwrap();
        assert_eq!(app.get_special_app(), Some(SpecialApp::AltStore));
        assert_eq!(app.original_bundle_id().unwrap(), "com.rileytestut.AltStore");
        app.update_bundle_id(original, "com.NEWTEAM123.com.rileytestut.AltStore").unwrap();
        assert_eq!(app.main_bundle_id().unwrap(), "com.NEWTEAM123.com.rileytestut.AltStore");
        assert_eq!(app.bundle.app_extensions()[0].bundle_identifier(), Some("com.NEWTEAM123.com.rileytestut.AltStore.AltWidget"));
    }
}

#[test]
fn ordinary_youtube_and_livecontainer_identity_rules_are_unchanged() {
    for original in ["com.example.app", "com.google.ios.youtube", "com.kdt.livecontainer"] {
        let dir = tempfile::tempdir().unwrap();
        let app_dir = dir.path().join("Test.app");
        write_bundle(&app_dir, original);
        write_bundle(&app_dir.join("PlugIns/Share.appex"), &format!("{original}.Share"));
        let mut app = Application::new(app_dir).unwrap();
        assert_eq!(app.original_bundle_id().unwrap(), original);
        let signed = format!("{original}.NEWTEAM123");
        app.update_bundle_id(original, &signed).unwrap();
        assert_eq!(app.main_bundle_id().unwrap(), signed);
        assert_eq!(app.bundle.app_extensions()[0].bundle_identifier(), Some(format!("{signed}.Share").as_str()));
    }
}
