//! Lettura dei profili di provisioning (.mobileprovision) presenti sul telefono.

use std::fs;
use std::io::{self, Cursor};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct PhoneProfile {
    /// Per ora non usato: servirà a togliere dal telefono i profili sostituiti.
    #[allow(dead_code)]
    pub uuid: String,
    pub name: String,
    pub team_id: Option<String>,
    /// Bundle ID senza il prefisso del team (es. `com.spotify.client.ABCDE12345`).
    pub bundle_id: Option<String>,
    pub expires: Option<plist::Date>,
}

impl PhoneProfile {
    /// Giorni mancanti alla scadenza (negativi se già scaduto).
    pub fn days_left(&self) -> Option<i64> {
        let exp = SystemTime::from(self.expires?);
        Some(match exp.duration_since(SystemTime::now()) {
            Ok(d) => (d.as_secs() / 86_400) as i64,
            Err(e) => -((e.duration().as_secs() / 86_400) as i64) - 1,
        })
    }

    pub fn expires_text(&self) -> String {
        self.expires
            .map(|d| d.to_xml_format())
            .unwrap_or_else(|| "?".into())
    }
}

/// AltStore and other signers may insert their *original* ten-character
/// Team ID into an extension bundle, even when the provisioned profile
/// belongs to a different (current) developer team. Match only a complete
/// parent + team-marker + nonempty extension suffix, not arbitrary prefixes.
/// Apple provisioning App IDs may have a *second* Team ID suffix that
/// the UI deliberately hides (e.g. host "com.foo.PXUL22799Q" is shown as
/// "com.foo"). Keep the raw ID for Apple renewals, but ignore this final
/// suffix while comparing parent and extension relationships.
pub fn profile_app_id<'a>(bundle: &'a str, team: &str) -> &'a str {
    if team.is_empty() { return bundle; }
    bundle.strip_suffix(&format!(".{team}")).unwrap_or(bundle)
}

fn extension_of(bundle: &str, parent: &str, team: &str) -> bool {
    let child_id = profile_app_id(bundle, team);
    let parent_id = profile_app_id(parent, team);
    let Some(suffix) = child_id.strip_prefix(parent_id).and_then(|s| s.strip_prefix('.')) else {
        return false;
    };
    let Some((marker, extension_name)) = suffix.split_once('.') else {
        return false;
    };
    !extension_name.is_empty() && (
        marker == team ||
        (marker.len() == 10 &&
            marker.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
    )
}

/// Resolve an extension to an *existing* host bundle: the longest valid
/// parent wins to avoid matching adjacent/unrelated dotted app identifiers.
pub fn group_root<'a>(bundle: &'a str, bundles: &[&'a str], team: &str) -> &'a str {
    bundles.iter().copied()
        .filter(|parent| *parent != bundle && extension_of(bundle, parent, team))
        .max_by_key(|parent| parent.len())
        .unwrap_or(bundle)
}

pub fn members_of_group<'a>(root: &str, bundles: &'a [&'a str], team: &str) -> Vec<&'a str> {
    bundles.iter().copied()
        .filter(|b| group_root(b, bundles, team) == root)
        .collect()
}

/// Per ogni bundle vale il profilo che scade più tardi: dopo un rinnovo quelli vecchi restano
/// sul telefono. I profili senza bundle ID restano ciascuno per conto suo.
pub fn latest_by_bundle<'a>(
    profiles: impl IntoIterator<Item = &'a PhoneProfile>,
) -> Vec<&'a PhoneProfile> {
    let when = |p: &PhoneProfile| p.expires.map(SystemTime::from);
    let mut latest: Vec<&PhoneProfile> = Vec::new();
    for p in profiles {
        let same = latest
            .iter_mut()
            .find(|q| p.bundle_id.is_some() && q.bundle_id == p.bundle_id);
        match same {
            Some(q) => {
                if when(p) > when(q) {
                    *q = p;
                }
            }
            None => latest.push(p),
        }
    }
    latest
}

/// Il profilo è un CMS firmato: dentro c'è il plist XML in chiaro.
pub fn parse(raw: &[u8]) -> Option<PhoneProfile> {
    let start = raw.windows(5).position(|w| w == b"<?xml")?;
    let end = raw.windows(8).rposition(|w| w == b"</plist>")? + 8;
    let value = plist::Value::from_reader(Cursor::new(&raw[start..end])).ok()?;
    let d = value.as_dictionary()?;

    let text = |k: &str| d.get(k).and_then(|v| v.as_string()).map(str::to_string);
    let team_id = d
        .get("TeamIdentifier")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_string())
        .map(str::to_string);
    let app_identifier = d
        .get("Entitlements")
        .and_then(|v| v.as_dictionary())
        .and_then(|e| e.get("application-identifier"))
        .and_then(|v| v.as_string())
        .map(str::to_string);
    let bundle_id = match (&team_id, &app_identifier) {
        (Some(t), Some(a)) => a.strip_prefix(&format!("{t}.")).map(str::to_string),
        _ => None,
    };

    Some(PhoneProfile {
        uuid: text("UUID")?,
        name: text("Name").unwrap_or_default(),
        team_id,
        bundle_id,
        expires: d.get("ExpirationDate").and_then(|v| v.as_date()),
    })
}

const BACKUP_KEEP_AFTER_EXPIRY: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(30 * 24 * 60 * 60);
const CLEANUP_STAMP: &str = "state/profile-backup-last-cleanup";

/// Remove only verified expired backup files matching their original UUID.
/// An unreadable or malformed backup is retained, never guessed to be expired.
/// Symlinks, subdirectories and unrelated files are not touched.
pub fn cleanup_expired_backups(dir: &Path, now: SystemTime) -> io::Result<usize> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() { continue; } // no symlink following
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("mobileprovision") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let data = fs::read(&path)?;
        let Some(p) = parse(&data) else {
            continue;
        };
        // Never delete a file whose name does not match the embedded profile UUID.
        if !p.uuid.eq_ignore_ascii_case(stem) { continue; }
        let Some(expiry) = p.expires else { continue; };
        let Ok(elapsed) = now.duration_since(SystemTime::from(expiry)) else {
            continue;
        };
        if elapsed >= BACKUP_KEEP_AFTER_EXPIRY {
            fs::remove_file(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Once per 30 days, clean backups that expired 30+ days ago.
/// Store the last successful sweep in the persistent /data/state directory.
pub fn cleanup_backups_if_due(now: SystemTime) -> io::Result<Option<usize>> {
    let stamp = Path::new(CLEANUP_STAMP);
    if let Ok(raw) = fs::read_to_string(stamp) {
        if let Ok(last_seconds) = raw.trim().parse::<u64>() {
            if let Some(last) = UNIX_EPOCH.checked_add(Duration::from_secs(last_seconds)) {
                if now.duration_since(last).is_ok_and(|elapsed| elapsed < CLEANUP_INTERVAL) {
                    return Ok(None);
                }
            }
        }
    }
    let removed = cleanup_expired_backups(Path::new(crate::SAVED_DIR), now)?;
    fs::create_dir_all("state")?;
    let timestamp = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    crate::fsperm::write_private(stamp, timestamp.to_string().as_bytes())?;
    Ok(Some(removed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn profile(uuid: &str, bundle: Option<&str>, days: u64) -> PhoneProfile {
        PhoneProfile {
            uuid: uuid.into(),
            name: "test".into(),
            team_id: Some("TEAM".into()),
            bundle_id: bundle.map(str::to_string),
            expires: Some(plist::Date::from(
                SystemTime::now() + Duration::from_secs(days * 86_400),
            )),
        }
    }

    #[test]
    fn un_profilo_per_bundle_quello_che_scade_piu_tardi() {
        let all = [
            profile("vecchio", Some("com.a"), 1),
            profile("nuovo", Some("com.a"), 6),
            profile("medio", Some("com.a"), 3),
            profile("altro", Some("com.b"), 2),
        ];
        let latest = latest_by_bundle(&all);
        assert_eq!(latest.len(), 2);
        let a = latest.iter().find(|p| p.bundle_id.as_deref() == Some("com.a")).unwrap();
        assert_eq!(a.uuid, "nuovo");
        let b = latest.iter().find(|p| p.bundle_id.as_deref() == Some("com.b")).unwrap();
        assert_eq!(b.uuid, "altro");
    }

    #[test]
    fn i_profili_senza_bundle_non_si_fondono() {
        let all = [profile("x", None, 1), profile("y", None, 2)];
        assert_eq!(latest_by_bundle(&all).len(), 2);
    }


    fn backup_xml(uuid: &str, expires: Option<SystemTime>) -> Vec<u8> {
        let mut d = plist::Dictionary::new();
        d.insert("UUID".into(), plist::Value::String(uuid.into()));
        if let Some(t) = expires {
            d.insert("ExpirationDate".into(), plist::Value::Date(plist::Date::from(t)));
        }
        let mut out = Vec::new();
        plist::to_writer_xml(&mut out, &plist::Value::Dictionary(d)).unwrap();
        out
    }

    #[test]
    fn cleanup_only_expired_over_30_days_after_expiration() {
        let now = SystemTime::now();
        let dir = std::env::temp_dir().join(format!(
            "altkeeper-profile-cleanup-{}-{:?}", std::process::id(), std::thread::current().id()));
        fs::create_dir_all(&dir).unwrap();

        let old = dir.join("old.mobileprovision");
        let recent = dir.join("recent.mobileprovision");
        let active = dir.join("active.mobileprovision");
        let corrupt = dir.join("corrupt.mobileprovision");
        let mismatch = dir.join("mismatch.mobileprovision");
        let no_expiry = dir.join("unknown.mobileprovision");
        let unrelated = dir.join("notes.txt");
        fs::write(&old, backup_xml("old", Some(now - Duration::from_secs(31 * 86400)))).unwrap();
        fs::write(&recent, backup_xml("recent", Some(now - Duration::from_secs(29 * 86400)))).unwrap();
        fs::write(&active, backup_xml("active", Some(now + Duration::from_secs(86400)))).unwrap();
        fs::write(&corrupt, b"bad content").unwrap();
        fs::write(&mismatch, backup_xml("another", Some(now - Duration::from_secs(60 * 86400)))).unwrap();
        fs::write(&no_expiry, backup_xml("unknown", None)).unwrap();
        fs::write(&unrelated, b"unrelated").unwrap();

        assert_eq!(cleanup_expired_backups(&dir, now).unwrap(), 1);
        assert!(!old.exists());
        for file in [recent, active, corrupt, mismatch, no_expiry, unrelated] {
            assert!(file.exists(), "unexpected deletion: {}", file.display());
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nonexistent_backup_folder_is_a_noop() {
        let dir = std::env::temp_dir().join(format!(
            "altkeeper-no-backups-{}-{:?}", std::process::id(), std::thread::current().id()));
        assert_eq!(cleanup_expired_backups(&dir, SystemTime::now()).unwrap(), 0);
    }

    #[test]
    fn extension_groups_do_not_swallow_unrelated_apps() {
        let bundles = [
            "com.google.ios.youtube",
            "com.google.ios.youtube.TEAM.OpenYouTube.Extension",
            "com.kdt.livecontainer",
            "com.kdt.livecontainer.TEAM.LiveProcess",
            "com.google.ios.youtube.another.app",
        ];
        assert_eq!(group_root(bundles[1], &bundles, "TEAM"), bundles[0]);
        assert_eq!(group_root(bundles[3], &bundles, "TEAM"), bundles[2]);
        assert_eq!(group_root(bundles[4], &bundles, "TEAM"), bundles[4]);
        assert_eq!(members_of_group(bundles[2], &bundles, "TEAM").len(), 2);
    }
    #[test]
    fn real_altstore_extension_bundles_group_even_if_embedded_team_differs() {
        // Real bundle shapes seen on iOS; the embedded marker is the app's
        // original signer, which need not equal this profile's Apple Team ID.
        let bundles = [
            "com.google.ios.youtube",
            "com.google.ios.youtube.PXUL22799Q.OpenYouTube.Extension",
            "com.kdt.livecontainer",
            "com.kdt.livecontainer.PXUL22799Q.LaunchAppExtension",
            "com.kdt.livecontainer.PXUL22799Q.LiveProcess",
            "com.kdt.livecontainer.PXUL22799Q.ShareExtension",
        ];
        let profile_team = "DIFFERENT1";
        assert_eq!(group_root(bundles[1], &bundles, profile_team), bundles[0]);
        for extension in &bundles[3..] {
            assert_eq!(group_root(extension, &bundles, profile_team), bundles[2]);
        }
        assert_eq!(members_of_group(bundles[0], &bundles, profile_team).len(), 2);
        assert_eq!(members_of_group(bundles[2], &bundles, profile_team).len(), 4);
    }

    #[test]
    fn real_provisioning_app_ids_include_trailing_team_suffix() {
        // The web UI strips the final ".PXUL22799Q" from each raw profile
        // bundle when it displays the names seen in the user screenshot.
        // Previous tests used the displayed IDs and missed this failure.
        let team = "PXUL22799Q";
        let raw = [
            "com.google.ios.youtube.PXUL22799Q",
            "com.google.ios.youtube.PXUL22799Q.OpenYouTube.Extension.PXUL22799Q",
            "com.kdt.livecontainer.PXUL22799Q",
            "com.kdt.livecontainer.PXUL22799Q.LaunchAppExtension.PXUL22799Q",
            "com.kdt.livecontainer.PXUL22799Q.LiveProcess.PXUL22799Q",
            "com.kdt.livecontainer.PXUL22799Q.ShareExtension.PXUL22799Q",
        ];
        assert_eq!(profile_app_id(raw[0], team), "com.google.ios.youtube");
        assert_eq!(
            profile_app_id(raw[1], team),
            "com.google.ios.youtube.PXUL22799Q.OpenYouTube.Extension"
        );
        assert_eq!(group_root(raw[1], &raw, team), raw[0]);
        for ext in &raw[3..] {
            assert_eq!(group_root(ext, &raw, team), raw[2]);
        }
        assert_eq!(members_of_group(raw[0], &raw, team).len(), 2);
        assert_eq!(members_of_group(raw[2], &raw, team).len(), 4);
    }

    #[test]
    fn trailing_suffix_must_match_current_profile_team_exactly() {
        let raw = [
            "com.example.app.AAAAAAAAAA",
            "com.example.app.BBBBBBBBBB.Ext.AAAAAAAAAA",
            "com.example.app.feature.other",
        ];
        assert_eq!(group_root(raw[1], &raw, "AAAAAAAAAA"), raw[0]);
        assert_eq!(group_root(raw[2], &raw, "AAAAAAAAAA"), raw[2]);
        assert_eq!(profile_app_id(raw[0], "BBBBBBBBBB"), raw[0]);
    }

    #[test]
    fn malformed_team_markers_do_not_group_unrelated_bundles() {
        let bundles = [
            "com.example.app",
            "com.example.app.feature.widget",
            "com.example.app.short.Widget",
            "com.example.app.PXUL22799Q",
            "com.example.app.PXUL22799q.Widget",
            "com.example.app.PXUL22799Q.Widget",
            "com.example.app.PXUL22799Q.", // no extension name
            "com.example.app.PXUL22799Q.Widget.More",
        ];
        for index in [1,2,3,4,6] {
            assert_eq!(group_root(bundles[index], &bundles, "SAVEDTEAM1"), bundles[index]);
        }
        assert_eq!(group_root(bundles[5], &bundles, "SAVEDTEAM1"), bundles[0]);
        assert_eq!(group_root(bundles[7], &bundles, "SAVEDTEAM1"), bundles[0]);
    }

    #[test]
    fn senza_profili_non_c_e_niente() {
        assert!(latest_by_bundle(&[]).is_empty());
    }
}
