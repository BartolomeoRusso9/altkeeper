//! Controllo e installazione degli aggiornamenti dalle release GitHub di AltKeeper.
//! Solo Windows: lo usa l'icona nella barra di sistema.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

const LATEST: &str = "https://api.github.com/repos/BartolomeoRusso9/altkeeper/releases/latest";
pub const EXE_ASSET: &str = "altkeeper.exe";
pub const SHA_ASSET: &str = "altkeeper.exe.sha256";

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub tag: String,
    pub exe_url: String,
    pub sha_url: String,
    pub page: String,
}

/// `v0.2.0-plus.3` -> (0, 2, 0, 3); senza `-plus.N` il quarto numero è 0.
fn version_key(v: &str) -> Option<(u64, u64, u64, u64)> {
    let v = v.trim().trim_start_matches('v');
    let (core, plus) = match v.split_once("-plus.") {
        Some((c, p)) => (c, p.parse().ok()?),
        None => (v, 0),
    };
    let mut it = core.split('.').map(|n| n.parse::<u64>());
    let key = (it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?, plus);
    it.next().is_none().then_some(key)
}

pub fn newer(current: &str, latest_tag: &str) -> bool {
    match (version_key(current), version_key(latest_tag)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

pub fn parse_release(v: &Value) -> Option<Release> {
    let asset = |name: &str| {
        v["assets"].as_array()?.iter().find(|a| a["name"] == name)?["browser_download_url"]
            .as_str()
            .map(String::from)
    };
    Some(Release {
        tag: v["tag_name"].as_str()?.to_string(),
        exe_url: asset(EXE_ASSET)?,
        sha_url: asset(SHA_ASSET)?,
        page: v["html_url"].as_str()?.to_string(),
    })
}

pub fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("AltKeeper/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())
}

/// Il JSON all'indirizzo dato; `None` se GitHub risponde 404 (per esempio nessuna release).
fn get_json(url: &str) -> Result<Option<Value>, String> {
    let r = client()?.get(url).send().map_err(|e| format!("update check failed: {e}"))?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    r.error_for_status()
        .and_then(|r| r.json::<Value>())
        .map(Some)
        .map_err(|e| format!("update check failed: {e}"))
}

/// L'ultima release, se è più nuova di quella in uso.
pub fn check() -> Result<Option<Release>, String> {
    let v = get_json(LATEST)?;
    Ok(v.as_ref()
        .and_then(parse_release)
        .filter(|r| newer(env!("CARGO_PKG_VERSION"), &r.tag)))
}

/// Nome dell'exe precedente dopo un aggiornamento, finché quello nuovo non parte bene.
pub const OLD_EXE: &str = "altkeeper.old.exe";
const NEW_EXE: &str = "altkeeper.new.exe";
const FAILED_EXE: &str = "altkeeper.failed.exe";

/// Il file `.sha256` (hash esadecimale, eventualmente seguito dal nome) corrisponde a `bytes`?
pub fn verify(bytes: &[u8], sha_file: &str) -> bool {
    use sha2::{Digest, Sha256};
    let want = sha_file.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    let got: String = Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect();
    !want.is_empty() && want == got
}

/// Mette `bytes` al posto di `exe` (che può essere in esecuzione: Windows lo lascia rinominare)
/// e tiene il vecchio come `altkeeper.old.exe`. Con un hash sbagliato non tocca niente.
pub fn install_new(exe: &Path, bytes: &[u8], sha_file: &str) -> Result<(), String> {
    if !verify(bytes, sha_file) {
        return Err("the downloaded update does not match its SHA-256: nothing changed".into());
    }
    let (new, old) = (exe.with_file_name(NEW_EXE), exe.with_file_name(OLD_EXE));
    std::fs::write(&new, bytes).map_err(|e| format!("cannot write {}: {e}", new.display()))?;
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).map_err(|e| format!("cannot move the current exe: {e}"))?;
    if let Err(e) = std::fs::rename(&new, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(format!("cannot put the new exe in place: {e}"));
    }
    Ok(())
}

/// Scarica la release, la controlla e la mette al posto di `exe`.
pub fn stage(rel: &Release, exe: &Path) -> Result<(), String> {
    let c = client()?;
    let get = |url: &str| {
        c.get(url)
            .timeout(Duration::from_secs(300))
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.bytes())
            .map_err(|e| format!("download failed: {e}"))
    };
    let sha = get(&rel.sha_url)?;
    let bytes = get(&rel.exe_url)?;
    install_new(exe, &bytes, &String::from_utf8_lossy(&sha))
}

/// Il nuovo exe non è partito: rimette quello vecchio. `false` se non c'era niente da rimettere.
pub fn rollback(exe: &Path) -> bool {
    let old = exe.with_file_name(OLD_EXE);
    if !old.exists() {
        return false;
    }
    let failed = exe.with_file_name(FAILED_EXE);
    let _ = std::fs::remove_file(&failed);
    let _ = std::fs::rename(exe, &failed);
    std::fs::rename(&old, exe).is_ok()
}

/// Dopo un avvio riuscito i file dell'aggiornamento non servono più.
pub fn cleanup(exe: &Path) {
    for name in [OLD_EXE, FAILED_EXE, NEW_EXE] {
        let _ = std::fs::remove_file(exe.with_file_name(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn temp_exe(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("altkeeper-upd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let exe = d.join("altkeeper.exe");
        std::fs::write(&exe, b"old").unwrap();
        exe
    }

    #[test]
    fn verify_checks_the_sha256_file() {
        // sha256("new")
        let sha = "11507a0e2f5e69d5dfa40a62a1bd7b6ee57e6bcd85c67c9b8431b36fff21c437";
        assert!(verify(b"new", &format!("{sha}  altkeeper.exe\n")));
        assert!(verify(b"new", &sha.to_uppercase()));
        assert!(!verify(b"tampered", sha));
        assert!(!verify(b"new", ""));
    }

    #[test]
    fn update_rejects_bad_hash() {
        let exe = temp_exe("badhash");
        assert!(install_new(&exe, b"new", "00").is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        assert!(!exe.with_file_name(OLD_EXE).exists());
    }

    #[test]
    fn update_offline_is_noop() {
        rustls::crypto::ring::default_provider().install_default().ok();
        let exe = temp_exe("offline");
        let r = Release {
            tag: "v9.9.9".into(),
            exe_url: "http://127.0.0.1:9/altkeeper.exe".into(),
            sha_url: "http://127.0.0.1:9/altkeeper.exe.sha256".into(),
            page: String::new(),
        };
        assert!(stage(&r, &exe).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    }

    #[test]
    fn swap_and_rollback() {
        let exe = temp_exe("swap");
        let sha = "11507a0e2f5e69d5dfa40a62a1bd7b6ee57e6bcd85c67c9b8431b36fff21c437";
        install_new(&exe, b"new", sha).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(exe.with_file_name(OLD_EXE)).unwrap(), b"old");

        assert!(rollback(&exe));
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        assert!(!exe.with_file_name(OLD_EXE).exists());
        assert!(!rollback(&exe)); // niente da rimettere
    }

    #[test]
    fn version_ordering() {
        assert!(newer("0.2.0-plus.1", "v0.2.0-plus.2"));
        assert!(newer("0.2.0-plus.9", "v0.3.0"));
        assert!(newer("0.2.0", "v0.2.0-plus.1"));
        assert!(!newer("0.2.0-plus.1", "v0.2.0-plus.1"));
        assert!(!newer("0.2.0-plus.2", "v0.2.0-plus.1"));
        assert!(!newer("0.2.0-plus.1", "garbage"));
        assert!(!newer("0.2.0-plus.1", "v1.2"));
    }

    #[test]
    #[ignore = "network"]
    fn live_check() {
        rustls::crypto::ring::default_provider().install_default().ok();
        println!("latest: {:?}", check());
    }

    #[test]
    fn parse_release_json() {
        let v = json!({
            "tag_name": "v0.2.0-plus.2",
            "html_url": "https://github.com/BartolomeoRusso9/altkeeper/releases/tag/v0.2.0-plus.2",
            "assets": [
                { "name": "altkeeper.exe", "browser_download_url": "https://x/altkeeper.exe" },
                { "name": "altkeeper.exe.sha256", "browser_download_url": "https://x/altkeeper.exe.sha256" },
            ],
        });
        let r = parse_release(&v).unwrap();
        assert_eq!(r.tag, "v0.2.0-plus.2");
        assert_eq!(r.exe_url, "https://x/altkeeper.exe");
        assert_eq!(r.sha_url, "https://x/altkeeper.exe.sha256");

        let mut missing = v.clone();
        missing["assets"].as_array_mut().unwrap().pop();
        assert_eq!(parse_release(&missing), None);
    }
}
