use apple_codesign::{SigningSettings, UnifiedSigner};
use plist::Dictionary;
use plist_macro::plist_to_xml_string;
use rootcause::{option_ext::OptionExt, prelude::*};
use tracing::info;

use crate::{
    dev::{app_ids::Profile, teams::DeveloperTeam},
    sideload::{
        application::{Application, SpecialApp},
        cert_identity::CertificateIdentity,
    },
    util::plist::PlistDataExtract,
};

pub async fn sign<F, Fut>(
    app: &mut Application,
    cert_identity: &CertificateIdentity,
    provisioning_profile: &Profile,
    extension_profiles: &[(std::path::PathBuf, Profile)],
    special: &Option<SpecialApp>,
    team: &DeveloperTeam,
    progress_callback: Option<F>,
) -> Result<(), Report>
where
    F: Fn(f32) -> Fut,
    Fut: Future<Output = ()>,
{
    let main_entitlements: Dictionary =
        entitlements_from_prov(provisioning_profile.encoded_profile.as_ref(), special, team)?;
    let signer = signer_with(cert_identity, &main_entitlements)?;

    // Each app extension is signed with the entitlements of its own profile, so its signature
    // carries its own application identifier instead of the main app's.
    let mut extension_signers = Vec::with_capacity(extension_profiles.len());
    for (dir, profile) in extension_profiles {
        let ent = entitlements_from_prov(profile.encoded_profile.as_ref(), special, team)?;
        extension_signers.push((dir.clone(), signer_with(cert_identity, &ent)?));
    }

    let sorted_bundles = app.bundle.collect_bundles_sorted();

    for (index, bundle) in sorted_bundles.iter().enumerate() {
        if let Some(callback) = &progress_callback {
            callback(0.3 + 0.7 * (index as f32 / sorted_bundles.len() as f32)).await;
        }
        info!(
            "Signing {}",
            bundle
                .bundle_dir
                .file_name()
                .unwrap_or(bundle.bundle_dir.as_os_str())
                .to_string_lossy()
        );

        let bundle_signer = extension_signers
            .iter()
            .find(|(dir, _)| *dir == bundle.bundle_dir)
            .map(|(_, s)| s)
            .unwrap_or(&signer);
        bundle_signer
            .sign_path_in_place(&bundle.bundle_dir)
            .context(format!(
                "Failed to sign bundle: {}",
                bundle.bundle_dir.display()
            ))?;
    }

    Ok(())
}

fn signer_with<'a>(
    cert: &'a CertificateIdentity,
    entitlements: &Dictionary,
) -> Result<UnifiedSigner<'a>, Report> {
    let mut settings = signing_settings(cert)?;
    settings
        .set_entitlements_xml(
            apple_codesign::SettingsScope::Main,
            plist_to_xml_string(entitlements),
        )
        .context("Failed to set entitlements XML")?;
    Ok(UnifiedSigner::new(settings))
}

pub fn signing_settings<'a>(cert: &'a CertificateIdentity) -> Result<SigningSettings<'a>, Report> {
    let mut settings = SigningSettings::default();

    cert.setup_signing_settings(&mut settings)?;
    settings.set_for_notarization(false);
    settings.set_shallow(true);

    Ok(settings)
}

fn entitlements_from_prov(
    data: &[u8],
    special: &Option<SpecialApp>,
    team: &DeveloperTeam,
) -> Result<Dictionary, Report> {
    let start = data
        .windows(6)
        .position(|w| w == b"<plist")
        .ok_or_report()?;
    let end = data
        .windows(8)
        .rposition(|w| w == b"</plist>")
        .ok_or_report()?
        + 8;
    let plist_data = &data[start..end];
    let plist = plist::Value::from_reader_xml(plist_data)?;

    let mut entitlements = plist
        .as_dictionary()
        .ok_or_report()?
        .get_dict("Entitlements")?
        .clone();

    if matches!(
        special,
        Some(SpecialApp::SideStoreLc) | Some(SpecialApp::LiveContainer)
    ) {
        let mut keychain_access = vec![plist::Value::String(format!(
            "{}.com.kdt.livecontainer.shared",
            team.team_id
        ))];

        for number in 1..128 {
            keychain_access.push(plist::Value::String(format!(
                "{}.com.kdt.livecontainer.shared.{}",
                team.team_id, number
            )));
        }

        entitlements.insert(
            "keychain-access-groups".to_string(),
            plist::Value::Array(keychain_access),
        );
    }

    Ok(entitlements)
}
