#![cfg(unix)]

use isideload::sideload::{application::Application, bundle::Bundle};
use std::{fs, os::unix::fs::symlink};

#[test]
fn display_name_rewrite_skips_symlinks_and_non_regular_files() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("Test.app");
    fs::create_dir(&app).unwrap();
    let mut info = plist::Dictionary::new();
    info.insert("CFBundleIdentifier".into(), "com.example.test".into());
    info.insert("CFBundleName".into(), "Old".into());
    plist::to_file_xml(app.join("Info.plist"), &info).unwrap();
    let old = b"\"CFBundleDisplayName\" = \"Old\";\n";

    let localized = app.join("en.lproj");
    fs::create_dir(&localized).unwrap();
    fs::write(localized.join("InfoPlist.strings"), old).unwrap();
    let external_dir = temp.path().join("external");
    fs::create_dir(&external_dir).unwrap();
    fs::write(external_dir.join("InfoPlist.strings"), old).unwrap();
    symlink(&external_dir, app.join("linked.lproj")).unwrap();
    let linked_file_dir = app.join("fr.lproj");
    fs::create_dir(&linked_file_dir).unwrap();
    symlink(external_dir.join("InfoPlist.strings"), linked_file_dir.join("InfoPlist.strings")).unwrap();
    fs::write(app.join("file.lproj"), b"not a directory").unwrap();
    fs::create_dir_all(app.join("directory.lproj/InfoPlist.strings")).unwrap();
    fs::create_dir(app.join("unrelated")).unwrap();
    fs::write(app.join("unrelated/InfoPlist.strings"), old).unwrap();

    let mut application = Application { bundle: Bundle::new(app.clone()).unwrap() };
    application.apply_install_identity(Some("New"), None).unwrap();

    assert!(fs::read_to_string(localized.join("InfoPlist.strings")).unwrap().contains("\"New\""));
    assert_eq!(fs::read(external_dir.join("InfoPlist.strings")).unwrap(), old);
    assert_eq!(fs::read(app.join("unrelated/InfoPlist.strings")).unwrap(), old);
    assert!(fs::symlink_metadata(app.join("linked.lproj")).unwrap().is_symlink());
    assert!(fs::symlink_metadata(linked_file_dir.join("InfoPlist.strings")).unwrap().is_symlink());
    assert!(app.join("directory.lproj/InfoPlist.strings").is_dir());
}
