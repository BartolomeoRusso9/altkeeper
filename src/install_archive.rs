//! Package a *signed* .app as one IPA before staging it over Wi-Fi.
//! The old directory-based sideload path remains available as a rollback.
use std::fs::{self, File, Metadata};
use std::io::{Seek, Write};
use std::path::Path;

use idevice::{
    RsdService,
    afc::{AfcClient, opcode::AfcFopenMode},
    installation_proxy::InstallationProxyClient,
    rsd::RsdHandshake,
    tcp::handle::AdapterHandle,
};
use tempfile::NamedTempFile;
use tokio::io::AsyncReadExt;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// One IPA avoids thousands of remote AFC create/open/close operations, so it is the default
/// on every platform. `ALTKEEPER_INSTALL_MODE=directory` goes back to the file-by-file upload.
pub(crate) fn use_archive_on_wifi() -> bool {
    !std::env::var("ALTKEEPER_INSTALL_MODE")
        .is_ok_and(|mode| mode.eq_ignore_ascii_case("directory"))
}

fn permissions(metadata: &Metadata, directory: bool) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = directory;
        metadata.permissions().mode() & 0o777
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        if directory { 0o755 } else { 0o644 }
    }
}

/// On Windows there are no Unix modes, so executables would be archived as 0o644 and
/// installd would reject them. Mach-O binaries (thin or fat) get 0o755.
#[cfg(any(not(unix), test))]
fn is_macho(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 4];
    File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok()
        && matches!(
            u32::from_be_bytes(magic),
            0xfeedface | 0xfeedfacf | 0xcefaedfe | 0xcffaedfe | 0xcafebabe | 0xbebafeca
                | 0xcafebabf | 0xbfbafeca
        )
}

fn write_tree<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    local: &Path,
    archived: &str,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(local).map_err(err)?;
    let mode = permissions(&metadata, metadata.is_dir());

    if metadata.file_type().is_symlink() {
        // Preserve relative framework links instead of following them or reading
        // from outside the signed bundle. Apple extraction must retain these links.
        let target = fs::read_link(local).map_err(err)?;
        let target = target.to_str().ok_or("Non-UTF-8 symlink target in signed app")?;
        zip.add_symlink(
            archived,
            target.replace('\\', "/"),
            SimpleFileOptions::default().unix_permissions(mode),
        ).map_err(err)?;
    } else if metadata.is_dir() {
        zip.add_directory(
            format!("{archived}/"),
            SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .unix_permissions(mode),
        ).map_err(err)?;
        let mut children: Vec<_> = fs::read_dir(local)
            .map_err(err)?
            .collect::<Result<_, _>>()
            .map_err(err)?;
        children.sort_by_key(|entry| entry.file_name());
        for entry in children {
            let name = entry.file_name();
            let name = name.to_str().ok_or("Non-UTF-8 filename in signed app")?;
            write_tree(zip, &entry.path(), &format!("{archived}/{name}"))?;
        }
    } else if metadata.is_file() {
        #[cfg(not(unix))]
        let mode = if is_macho(local) { 0o755 } else { mode };
        zip.start_file(
            archived,
            SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .compression_level(Some(1))
                .unix_permissions(mode),
        ).map_err(err)?;
        let mut input = File::open(local).map_err(err)?;
        std::io::copy(&mut input, zip).map_err(err)?;
    } else {
        return Err(format!("Unsupported file type in signed app: {}", local.display()));
    }
    Ok(())
}

/// Compress on disk with bounded memory, after signing and Bundle ID/name edits.
/// tempfile deletes partial/successful archives when the install finishes.
fn prepare_signed_app_archive(app_path: &Path) -> Result<NamedTempFile, String> {
    let app_name = app_path.file_name()
        .and_then(|name| name.to_str())
        .ok_or("Signed app has no UTF-8 filename")?;
    if !app_name.ends_with(".app") || !app_path.is_dir() {
        return Err(format!("Expected a signed .app directory, got {}", app_path.display()));
    }
    let mut temporary = tempfile::Builder::new()
        .prefix("altkeeper-install-")
        .suffix(".ipa")
        .tempfile()
        .map_err(err)?;
    {
        let mut zip = ZipWriter::new(temporary.as_file_mut());
        zip.add_directory(
            "Payload/",
            SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .unix_permissions(0o755),
        ).map_err(err)?;
        write_tree(&mut zip, app_path, &format!("Payload/{app_name}"))?;
        zip.finish().map_err(err)?;
    }
    temporary.as_file_mut().flush().map_err(err)?;
    Ok(temporary)
}

pub(crate) fn signed_bundle_id(app_path: &Path) -> Result<String, String> {
    let info = plist::Value::from_file(app_path.join("Info.plist")).map_err(err)?;
    info.as_dictionary()
        .and_then(|values| values.get("CFBundleIdentifier"))
        .and_then(plist::Value::as_string)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("Signed app {} has no CFBundleIdentifier", app_path.display()))
}

/// Transfer a single IPA over AFC, then have installation_proxy unpack/install it.
/// 0–70% reflects actual IPA bytes uploaded, 70–100% device installation.
pub(crate) async fn install_signed_app_rsd(
    adapter: &mut AdapterHandle,
    handshake: &mut RsdHandshake,
    app_path: &Path,
    progress: &(impl Fn(u64) + Send + Sync),
) -> Result<(), String> {
    let bundle_id = signed_bundle_id(app_path)?;
    let local_path = app_path.to_owned();
    let archive = tokio::task::spawn_blocking(move || prepare_signed_app_archive(&local_path))
        .await
        .map_err(|e| format!("IPA packaging task failed: {e}"))??;

    let name = archive.path().file_name()
        .and_then(|name| name.to_str())
        .ok_or("Temporary IPA filename is invalid")?;
    let remote_path = format!("PublicStaging/{name}");

    let mut afc = AfcClient::connect_rsd(adapter, handshake)
        .await
        .map_err(err)?;
    // idevice's IPA helper creates PublicStaging if it does not exist.
    if afc.get_file_info("PublicStaging").await.is_err() {
        afc.mk_dir("PublicStaging").await.map_err(err)?;
    }
    let mut target = afc.open(remote_path.clone(), AfcFopenMode::WrOnly)
        .await
        .map_err(err)?;
    let mut source = tokio::fs::File::open(archive.path()).await.map_err(err)?;
    let total = source.metadata().await.map_err(err)?.len().max(1);
    let mut sent = 0u64;
    let mut chunk = vec![0u8; 1024 * 1024];
    loop {
        let count = source.read(&mut chunk).await.map_err(err)?;
        if count == 0 { break; }
        target.write_entire(&chunk[..count]).await.map_err(err)?;
        sent += count as u64;
        progress((sent.saturating_mul(70) / total).min(70));
    }
    target.close().await.map_err(err)?;

    let mut install = InstallationProxyClient::connect_rsd(adapter, handshake)
        .await
        .map_err(err)?;
    let mut options = plist::Dictionary::new();
    // IPA installs use CFBundleIdentifier; PackageType=Developer is for a
    // staged .app directory, not a compressed IPA package.
    options.insert("CFBundleIdentifier".into(), plist::Value::String(bundle_id));
    let result = install.install_with_callback(
        remote_path.clone(),
        Some(plist::Value::Dictionary(options)),
        async |(percent, _)| { progress(70 + percent.min(100) * 30 / 100); },
        (),
    ).await.map_err(err);
    // Do not retain a potentially large staged IPA on the phone after success
    // or an iOS-reported installation failure. Cleanup errors must not mask
    // the actual installation result.
    let _ = afc.remove(remote_path).await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn recognizes_thin_and_fat_macho_magic_values() {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("Demo");
        for magic in [
            0xfeedface_u32, 0xfeedfacf, 0xcefaedfe, 0xcffaedfe,
            0xcafebabe, 0xbebafeca, 0xcafebabf, 0xbfbafeca,
        ] {
            fs::write(&binary, magic.to_be_bytes()).unwrap();
            assert!(is_macho(&binary), "unrecognized Mach-O magic: {magic:#x}");
        }
        for data in [b"".as_slice(), b"\xca\xfe\xba", b"data"] {
            fs::write(&binary, data).unwrap();
            assert!(!is_macho(&binary));
        }
        assert!(!is_macho(&root.path().join("missing")));
    }

    #[test]
    fn packages_signed_app_with_payload_and_nested_files() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("Renamed YouTube.app");
        fs::create_dir_all(app.join("_CodeSignature")).unwrap();
        fs::write(app.join("Info.plist"), b"updated bundle").unwrap();
        fs::write(app.join("_CodeSignature/CodeResources"), b"signed").unwrap();
        let archive = prepare_signed_app_archive(&app).unwrap();
        let mut zip = zip::ZipArchive::new(File::open(archive.path()).unwrap()).unwrap();
        assert!(zip.by_name("Payload/Renamed YouTube.app/").is_ok());
        assert_eq!(zip.by_name("Payload/Renamed YouTube.app/Info.plist").unwrap().size(), 14);
        assert!(zip.by_name("Payload/Renamed YouTube.app/_CodeSignature/CodeResources").is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn keeps_executable_permissions_and_symlinks() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("Demo.app");
        fs::create_dir(&app).unwrap();
        let executable = app.join("Demo");
        fs::write(&executable, b"binary").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        symlink("Demo", app.join("Current")).unwrap();
        let archive = prepare_signed_app_archive(&app).unwrap();
        let mut zip = zip::ZipArchive::new(File::open(archive.path()).unwrap()).unwrap();
        let mode = zip.by_name("Payload/Demo.app/Demo").unwrap().unix_mode().unwrap();
        assert_eq!(mode & 0o111, 0o111);
        let mut target = String::new();
        zip.by_name("Payload/Demo.app/Current")
            .unwrap().read_to_string(&mut target).unwrap();
        assert_eq!(target, "Demo");
    }

    #[test]
    fn uses_post_signing_bundle_id_for_ipa_install() {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("Customized.app");
        fs::create_dir(&app).unwrap();
        let mut info = plist::Dictionary::new();
        info.insert("CFBundleIdentifier".into(),
            plist::Value::String("com.example.custom.TEAM1234".into()));
        plist::Value::Dictionary(info)
            .to_file_xml(app.join("Info.plist")).unwrap();
        assert_eq!(signed_bundle_id(&app).unwrap(), "com.example.custom.TEAM1234");
    }

    #[test]
    fn rejects_non_app_directories() {
        let root = tempfile::tempdir().unwrap();
        assert!(prepare_signed_app_archive(root.path()).is_err());
    }
}
