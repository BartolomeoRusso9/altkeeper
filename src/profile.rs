//! Lettura dei profili di provisioning (.mobileprovision) presenti sul telefono.

use std::io::Cursor;
use std::time::SystemTime;

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

    #[test]
    fn senza_profili_non_c_e_niente() {
        assert!(latest_by_bundle(&[]).is_empty());
    }
}
