// This file was made using https://github.com/Dadoum/Sideloader as a reference.
// I'm planning on redoing this later to better handle entitlements, extensions, etc, but it will do for now

use crate::SideloadError;
use crate::dev::app_ids::{AppId, AppIdsApi};
use crate::dev::developer_session::DeveloperSession;
use crate::dev::teams::DeveloperTeam;
use crate::sideload::bundle::Bundle;
use crate::sideload::cert_identity::CertificateIdentity;
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, Generate, KeyInit},
};
use isideload_vfs::fs::File;
use rootcause::option_ext::OptionExt;
use rootcause::prelude::*;
use sha1::Digest;
use sha2::Sha256;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use zip::ZipArchive;

static ALT_PAIRING_FILE: &str = "ALTPairingFile.dat";

pub struct Application {
    pub bundle: Bundle,
    //pub temp_path: PathBuf,
}

/// Installation metadata, read from IPA Info.plist files without unpacking the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpaExtensionInfo {
    pub bundle_id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpaManifest {
    pub bundle_id: String,
    pub name: String,
    pub extensions: Vec<IpaExtensionInfo>,
}

fn display_name(info: &plist::Dictionary, fallback: &str) -> String {
    info.get("CFBundleDisplayName")
        .or_else(|| info.get("CFBundleName"))
        .and_then(|v| v.as_string())
        .unwrap_or(fallback)
        .to_string()
}

/// Read the main bundle identifier and extension manifests. This is read-only:
/// neither signing nor Apple registration happens until the user confirms.
pub fn inspect_ipa(path: &Path) -> Result<IpaManifest, Report> {
    let file = File::open(path).context("Could not open IPA for inspection")?;
    let mut archive = ZipArchive::new(file).context("IPA is not a valid ZIP archive")?;
    let mut main = None;
    let mut extensions = Vec::new();
    let mut ids = HashSet::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let entry_name = entry.name().to_owned();
        let parts: Vec<_> = entry_name.split('/').collect();
        let is_main = parts.len() == 3 && parts[0] == "Payload"
            && parts[1].ends_with(".app") && parts[2] == "Info.plist";
        let is_extension = parts.len() == 5 && parts[0] == "Payload"
            && parts[1].ends_with(".app") && parts[2] == "PlugIns"
            && parts[3].ends_with(".appex") && parts[4] == "Info.plist";
        if !is_main && !is_extension { continue; }
        if entry.size() > 512 * 1024 {
            bail!(SideloadError::InvalidBundle("Info.plist exceeds 512 KiB".into()));
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        let info: plist::Dictionary = plist::from_bytes(&bytes)
            .context("Cannot parse Info.plist")?;
        let bundle_id = info.get("CFBundleIdentifier")
            .and_then(|v| v.as_string()).ok_or_report()
            .context("Missing CFBundleIdentifier")?.to_string();
        if !ids.insert(bundle_id.clone()) {
            bail!(SideloadError::InvalidBundle("Duplicate bundle identifier".into()));
        }
        let name = display_name(&info, if is_main {
            parts[1].trim_end_matches(".app")
        } else {
            parts[3].trim_end_matches(".appex")
        });
        if is_main {
            if main.replace((bundle_id, name)).is_some() {
                bail!(SideloadError::InvalidBundle("Multiple apps in IPA".into()));
            }
        } else {
            extensions.push(IpaExtensionInfo { bundle_id, name });
        }
    }
    let (bundle_id, name) = main.ok_or_report()
        .context("IPA is missing Payload/*.app/Info.plist")?;
    extensions.sort_by(|a, b| a.bundle_id.cmp(&b.bundle_id));
    Ok(IpaManifest { bundle_id, name, extensions })
}

/// Replace quoted display-name values in OpenStep InfoPlist.strings without
/// disturbing other localized strings. Binary plist is handled separately.
fn replace_localized_display_name(text: &str, display_name: &str) -> String {
    let quoted_name = display_name.replace('\\', "\\\\").replace('"', "\\\"");
    let mut value = text.to_string();
    for key in ["CFBundleDisplayName", "CFBundleName"] {
        for prefix in [format!("\"{key}\""), key.to_string()] {
            let mut cursor = 0;
            while let Some(offset) = value[cursor..].find(&prefix) {
                let end_key = cursor + offset + prefix.len();
                let bytes = value.as_bytes();
                let mut p = end_key;
                while p < bytes.len() && bytes[p].is_ascii_whitespace() { p += 1; }
                if bytes.get(p) != Some(&b'=') { cursor = end_key; continue; }
                p += 1;
                while p < bytes.len() && bytes[p].is_ascii_whitespace() { p += 1; }
                if bytes.get(p) != Some(&b'"') { cursor = end_key; continue; }
                let first = p + 1;
                let mut last = first;
                let mut escaped = false;
                while last < bytes.len() {
                    let current = bytes[last];
                    if current == b'"' && !escaped { break; }
                    if current == b'\\' && !escaped { escaped = true; }
                    else { escaped = false; }
                    last += 1;
                }
                if last == bytes.len() { cursor = end_key; continue; }
                value.replace_range(first..last, &quoted_name);
                cursor = first + quoted_name.len() + 1;
            }
        }
    }
    value
}

fn localized_name_bytes(original: &[u8], name: &str) -> Result<Vec<u8>, Report> {
    if let Ok(mut dict) = plist::from_bytes::<plist::Dictionary>(original) {
        for key in ["CFBundleDisplayName", "CFBundleName"] {
            if dict.contains_key(key) {
                dict.insert(key.into(), plist::Value::String(name.into()));
            }
        }
        let mut result = Vec::new();
        plist::to_writer_binary(&mut result, &dict)?;
        return Ok(result);
    }
    let le = original.starts_with(&[0xff, 0xfe]);
    let be = original.starts_with(&[0xfe, 0xff]);
    let text = if le || be {
        let mut chunks = original[2..].chunks_exact(2);
        let words: Vec<u16> = chunks.by_ref().map(|c| if le {
            u16::from_le_bytes([c[0], c[1]])
        } else {
            u16::from_be_bytes([c[0], c[1]])
        }).collect();
        if !chunks.remainder().is_empty() {
            bail!(SideloadError::InvalidBundle("Invalid UTF-16 localization file".into()));
        }
        String::from_utf16(&words)?
    } else {
        String::from_utf8(original.to_vec())?
    };
    let updated = replace_localized_display_name(&text, name);
    if updated == text { return Ok(original.to_vec()); }
    if le || be {
        let mut out = if le { vec![0xff, 0xfe] } else { vec![0xfe, 0xff] };
        for c in updated.encode_utf16() {
            let encoded = if le { c.to_le_bytes() } else { c.to_be_bytes() };
            out.extend_from_slice(&encoded);
        }
        Ok(out)
    } else {
        Ok(updated.into_bytes())
    }
}

/// Accept a conservative reverse-DNS identifier, without the signer-added team suffix.
pub fn valid_custom_bundle_id(id: &str) -> bool {
    if id.len() > 160 || id.len() < 3 || !id.contains('.') { return false; }
    id.split('.').all(|part| {
        !part.is_empty() && part.len() <= 63
            && part.as_bytes()[0].is_ascii_alphanumeric()
            && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}


impl Application {
    pub fn new(path: PathBuf) -> Result<Self, Report> {
        if !isideload_vfs::fs::metadata(&path).is_ok() {
            bail!(SideloadError::InvalidBundle(
                "Application path does not exist".to_string(),
            ));
        }

        let mut bundle_path = path.clone();
        //let mut temp_path = PathBuf::new();

        if isideload_vfs::fs::metadata(&bundle_path)?.is_file() {
            let temp_dir = isideload_vfs::fs::temp_dir();
            let temp_path = temp_dir.join(
                path.file_name()
                    .ok_or_report()?
                    .to_string_lossy()
                    .to_string()
                    + "_extracted",
            );
            if isideload_vfs::fs::metadata(&temp_path).is_ok() {
                isideload_vfs::fs::remove_dir_all(&temp_path)
                    .context("Failed to remove existing temporary directory")?;
            }
            isideload_vfs::fs::create_dir_all(&temp_path)
                .context("Failed to create temporary directory")?;

            let file = File::open(&path).context("Failed to open application archive")?;
            let mut archive =
                ZipArchive::new(file).context("Failed to open application archive")?;

            archive
                .extract(&temp_path)
                .context("Failed to extract application archive")?;

            let payload_folder = temp_path.join("Payload");
            if isideload_vfs::fs::metadata(&payload_folder).is_ok() && payload_folder.is_dir() {
                let app_dirs: Vec<_> = isideload_vfs::fs::read_dir(&payload_folder)
                    .context("Failed to read Payload directory")?
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
                    .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "app"))
                    .collect();
                if app_dirs.len() == 1 {
                    bundle_path = app_dirs[0].path();
                } else if app_dirs.is_empty() {
                    bail!(SideloadError::InvalidBundle(
                        "No .app directory found in Payload".to_string(),
                    ));
                } else {
                    bail!(SideloadError::InvalidBundle(
                        "Multiple .app directories found in Payload".to_string(),
                    ));
                }
            } else {
                // gather the directory contents as a string for debugging
                let mut contents = String::new();
                if isideload_vfs::fs::metadata(&temp_path).is_ok() && temp_path.is_dir() {
                    let entries = isideload_vfs::fs::read_dir(&temp_path)
                        .context("Failed to read temporary directory for error reporting")?;
                    for entry in entries {
                        if let Ok(entry) = entry {
                            contents
                                .push_str(&format!("{}\n", entry.file_name().to_string_lossy()));
                        }
                    }
                }
                bail!(SideloadError::InvalidBundle(format!(
                    "No Payload directory found in the application archive, instead: {}",
                    contents
                ),));
            }
        }
        let bundle = Bundle::new(bundle_path)?;

        Ok(Application {
            bundle, /*temp_path*/
        })
    }

    /// Exclude unchecked .appex bundles before registering App IDs or signing.
    /// This mutates only the extracted temporary copy, never the original IPA.
    pub fn retain_extensions(&mut self, keep_ids: &[String]) -> Result<(), Report> {
        let keep: HashSet<&str> = keep_ids.iter().map(String::as_str).collect();
        let known: HashSet<&str> = self.bundle.app_extensions().iter()
            .filter_map(|e| e.bundle_identifier()).collect();
        for id in &keep {
            if !known.contains(id) {
                bail!(SideloadError::InvalidBundle(
                    format!("Selected extension is not present in the IPA: {id}"),
                ));
            }
        }
        let unwanted: Vec<PathBuf> = self.bundle.app_extensions().iter()
            .filter(|e| !e.bundle_identifier().is_some_and(|id| keep.contains(id)))
            .map(|e| e.bundle_dir.clone()).collect();
        for path in &unwanted {
            isideload_vfs::fs::remove_dir_all(path)
                .context("Could not remove an excluded extension")?;
        }
        // Re-scan the bundle so removed extensions' nested frameworks and
        // dylibs are not left in the signing tree.
        self.bundle = Bundle::new(self.bundle.bundle_dir.clone())?;
        Ok(())
    }

    /// Apply requested new-install identity before the existing team-ID signing rewrite.
    /// The original IPA is untouched; this only edits the extracted temp bundle.
    pub fn apply_install_identity(
        &mut self,
        display_name: Option<&str>,
        custom_bundle_id: Option<&str>,
    ) -> Result<(), Report> {
        if let Some(custom_id) = custom_bundle_id {
            if !valid_custom_bundle_id(custom_id) {
                bail!(SideloadError::InvalidBundle("Invalid custom Bundle ID".into()));
            }
            let original = self.main_bundle_id()?;
            if original != custom_id {
                self.update_bundle_id(&original, custom_id)?;
                for ext in self.bundle.app_extensions_mut() {
                    ext.write_info()?;
                }
            }
        }
        if let Some(name) = display_name {
            if name.is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control) {
                bail!(SideloadError::InvalidBundle("Invalid custom display name".into()));
            }
            self.bundle.app_info.insert("CFBundleDisplayName".into(), plist::Value::String(name.into()));
            self.bundle.app_info.insert("CFBundleName".into(), plist::Value::String(name.into()));
            // iOS prefers localized InfoPlist.strings names over CFBundleDisplayName.
            // Update the temporary IPA extraction only, not any installed app.
            for entry in isideload_vfs::fs::read_dir(&self.bundle.bundle_dir)? {
                let entry = entry?;
                let localized = entry.path();
                if localized.extension().is_none_or(|ext| ext != "lproj") { continue; }
                let path = localized.join("InfoPlist.strings");
                if !isideload_vfs::fs::metadata(&path).is_ok() { continue; }
                let old = isideload_vfs::fs::read(&path)?;
                if old.len() > 1024 * 1024 {
                    bail!(SideloadError::InvalidBundle("Localized display-name file too large".into()));
                }
                let new = localized_name_bytes(&old, name)?;
                if old != new {
                    isideload_vfs::fs::write(&path, &new)?;
                }
            }
        }
        Ok(())
    }

    pub fn get_special_app(&self) -> Option<SpecialApp> {
        let bundle_id = self.bundle.bundle_identifier().unwrap_or("");
        let special_app = match bundle_id {
            "com.rileytestut.AltStore" => Some(SpecialApp::AltStore),
            "com.SideStore.SideStore" => Some(SpecialApp::SideStore),
            "app.stik.store" => Some(SpecialApp::StikStore),
            _ => None,
        };
        if special_app.is_some() {
            return special_app;
        }

        if self
            .bundle
            .frameworks()
            .iter()
            .any(|f| f.bundle_identifier().unwrap_or("") == "com.SideStore.SideStore")
        {
            return Some(SpecialApp::SideStoreLc);
        }

        if bundle_id == "com.kdt.livecontainer" {
            return Some(SpecialApp::LiveContainer);
        }

        None
    }

    pub fn main_bundle_id(&self) -> Result<String, Report> {
        let str = self
            .bundle
            .bundle_identifier()
            .ok_or_report()
            .context("Failed to get main bundle identifier")?
            .to_string();

        Ok(str)
    }

    pub fn main_app_name(&self) -> Result<String, Report> {
        let str = self
            .bundle
            .bundle_name()
            .ok_or_report()
            .context("Failed to get main app name")?
            .to_string();

        Ok(str)
    }

    pub fn update_bundle_id(
        &mut self,
        main_app_bundle_id: &str,
        main_app_id_str: &str,
    ) -> Result<(), Report> {
        let extensions = self.bundle.app_extensions_mut();
        for ext in extensions.iter_mut() {
            if let Some(id) = ext.bundle_identifier() {
                if !(id.starts_with(main_app_bundle_id) && id.len() > main_app_bundle_id.len()) {
                    bail!(SideloadError::InvalidBundle(format!(
                        "Extension {} is not part of the main app bundle identifier: {}",
                        ext.bundle_name().unwrap_or("Unknown"),
                        id
                    )));
                } else {
                    ext.set_bundle_identifier(&format!(
                        "{}{}",
                        main_app_id_str,
                        &id[main_app_bundle_id.len()..]
                    ));
                }
            }
        }
        self.bundle.set_bundle_identifier(main_app_id_str);

        Ok(())
    }

    pub async fn register_app_ids(
        &self,
        //mode: &ExtensionsBehavior,
        dev_session: &mut DeveloperSession,
        team: &DeveloperTeam,
    ) -> Result<Vec<AppId>, Report> {
        let extension_refs: Vec<_> = self.bundle.app_extensions().iter().collect();
        let mut bundles_with_app_id = vec![&self.bundle];
        bundles_with_app_id.extend(extension_refs);

        let list_app_ids_response = dev_session
            .list_app_ids(team, None)
            .await
            .context("Failed to list app IDs for the developer team")?;
        let app_ids_to_register = bundles_with_app_id
            .iter()
            .filter(|bundle| {
                let bundle_id = bundle.bundle_identifier().unwrap_or("");
                !list_app_ids_response
                    .app_ids
                    .iter()
                    .any(|app_id| app_id.identifier == bundle_id)
            })
            .collect::<Vec<_>>();

        if let Some(available) = list_app_ids_response.available_quantity {
            if available < 0 {
                warn!(
                    "Apple reports a negative number of available app IDs ({}), which shouldn't be possible.",
                    available
                );
                // Since the App IDs should never be negative in the first place, it might still be worth trying to register them anyways. Who knows.
            } else {
                // We only do the conversion if available is positive, else we get an integral conversion error
                if app_ids_to_register.len() > available.try_into()? {
                    bail!(
                        "Not enough available app IDs. {} {} required, but only {} {} available.",
                        app_ids_to_register.len(),
                        if app_ids_to_register.len() == 1 {
                            "is"
                        } else {
                            "are"
                        },
                        available,
                        if available == 1 { "is" } else { "are" }
                    );
                }
            }
        }

        for bundle in app_ids_to_register {
            let id = bundle.bundle_identifier().unwrap_or("");
            let name = bundle.bundle_name().unwrap_or("");
            dev_session.add_app_id(team, name, id, None).await?;
        }
        let list_app_id_response = dev_session.list_app_ids(team, None).await?;
        let app_ids: Vec<_> = list_app_id_response
            .app_ids
            .into_iter()
            .filter(|app_id| {
                bundles_with_app_id
                    .iter()
                    .any(|bundle| app_id.identifier == bundle.bundle_identifier().unwrap_or(""))
            })
            .collect();

        info!("Registered app IDs");
        Ok(app_ids)
    }

    pub async fn apply_special_app_behavior(
        &mut self,
        special: &Option<SpecialApp>,
        group_identifier: &str,
        cert: &CertificateIdentity,
        pairing_file: Option<&[u8]>,
        device_udid: Option<&str>,
    ) -> Result<(), Report> {
        let Some(special) = special.as_ref() else {
            return Ok(());
        };

        if matches!(
            special,
            SpecialApp::SideStoreLc
                | SpecialApp::SideStore
                | SpecialApp::AltStore
                | SpecialApp::StikStore
        ) {
            if !matches!(special, SpecialApp::StikStore) {
                self.bundle.app_info.insert(
                    "ALTAppGroups".to_string(),
                    plist::Value::Array(vec![plist::Value::String(group_identifier.to_string())]),
                );
                let extensions = self.bundle.app_extensions_mut();
                for ext in extensions.iter_mut() {
                    ext.app_info.insert(
                        "ALTAppGroups".to_string(),
                        plist::Value::Array(vec![plist::Value::String(
                            group_identifier.to_string(),
                        )]),
                    );
                }
            }
            info!("Injecting certificate for {}", special);

            let target_bundle =
                match special {
                    SpecialApp::SideStoreLc => self.bundle.frameworks_mut().iter_mut().find(|fw| {
                        fw.bundle_identifier().unwrap_or("") == "com.SideStore.SideStore"
                    }),
                    _ => Some(&mut self.bundle),
                };

            if let Some(target_bundle) = target_bundle {
                let id_key = match special {
                    SpecialApp::StikStore => "MachineID",
                    _ => "ALTCertificateID",
                };
                let cert_file_name = match special {
                    SpecialApp::StikStore => "Certificate.p12",
                    _ => "ALTCertificate.p12",
                };
                target_bundle.app_info.insert(
                    id_key.to_string(),
                    plist::Value::String(cert.get_serial_number()),
                );

                let p12_bytes = cert
                    .as_p12(&cert.machine_id)
                    .await
                    .context("Failed to encode cert as p12")?;
                let alt_cert_path = target_bundle.bundle_dir.join(cert_file_name);

                let mut file = isideload_vfs::fs::File::create(&alt_cert_path)
                    .context(format!("Failed to create {}", cert_file_name))?;
                file.write_all(&p12_bytes)
                    .context(format!("Failed to write {}", cert_file_name))?;
            }
        }

        if matches!(special, SpecialApp::AltStore) {
            if let Some(pairing_file) = pairing_file {
                info!("Injecting pairing file for AltStore");

                let key_bytes = Sha256::digest(cert.machine_id.as_bytes());

                let cipher = Aes256Gcm::new(&key_bytes);

                let nonce = Nonce::generate();
                let ciphertext = cipher.encrypt(&nonce, pairing_file)?;

                let mut sealed_box = Vec::with_capacity(nonce.len() + ciphertext.len());
                sealed_box.extend_from_slice(&nonce);
                sealed_box.extend_from_slice(&ciphertext);

                let mut file =
                    isideload_vfs::fs::File::create(&self.bundle.bundle_dir.join(ALT_PAIRING_FILE))
                        .context(format!("Failed to create {}", ALT_PAIRING_FILE))?;
                file.write_all(&sealed_box)
                    .context(format!("Failed to write {}", ALT_PAIRING_FILE))?;
            } else {
                warn!(
                    "Pairing file is required for AltStore remote AltServer setup, but was not provided"
                )
            }

            if let Some(device_udid) = device_udid {
                self.bundle.app_info.insert(
                    "ALTDeviceId".to_string(),
                    plist::Value::String(device_udid.to_string()),
                );
            } else {
                warn!(
                    "Device UDID is required for AltStore remote AltServer setup, but was not provided"
                )
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecialApp {
    SideStore,
    SideStoreLc,
    LiveContainer,
    AltStore,
    StikStore,
}

// impl display
impl std::fmt::Display for SpecialApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecialApp::SideStore => write!(f, "SideStore"),
            SpecialApp::SideStoreLc => write!(f, "SideStore+LiveContainer"),
            SpecialApp::LiveContainer => write!(f, "LiveContainer"),
            SpecialApp::AltStore => write!(f, "AltStore"),
            SpecialApp::StikStore => write!(f, "StikStore"),
        }
    }
}
