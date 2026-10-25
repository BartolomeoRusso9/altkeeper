//! Controllo degli aggiornamenti sulle release GitHub del fork (e, come promemoria, su quelle
//! del progetto originale). Solo Windows: lo usa l'icona nella barra di sistema.

use std::time::Duration;

use serde_json::Value;

const FORK_LATEST: &str = "https://api.github.com/repos/WazeJinx/AltKeeperPlus/releases/latest";
// Il progetto originale non pubblica release: si guarda l'ultimo commit su main.
const UPSTREAM_HEAD: &str = "https://api.github.com/repos/BartolomeoRusso9/altkeeper/commits/main";
pub const UPSTREAM_COMMITS: &str = "https://github.com/BartolomeoRusso9/altkeeper/commits/main";
/// Commit del progetto originale su cui si basa questo fork (da aggiornare dopo ogni sync).
const UPSTREAM_BASE: &str = "940d315413451cee4edd4f14a3c4f85892312791";
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
        .user_agent(concat!("AltKeeperPlus/", env!("CARGO_PKG_VERSION")))
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

/// La release del fork più nuova di quella in uso, se c'è.
pub fn check() -> Result<Option<Release>, String> {
    let v = get_json(FORK_LATEST)?;
    Ok(v.as_ref()
        .and_then(parse_release)
        .filter(|r| newer(env!("CARGO_PKG_VERSION"), &r.tag)))
}

/// I primi 7 caratteri dell'ultimo commit del progetto originale, se non è la base del fork.
pub fn upstream_newer() -> Result<Option<String>, String> {
    let v = get_json(UPSTREAM_HEAD)?;
    Ok(v.as_ref()
        .and_then(|v| v["sha"].as_str())
        .filter(|sha| *sha != UPSTREAM_BASE)
        .map(|sha| sha.chars().take(7).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        println!("fork: {:?}", check());
        println!("upstream: {:?}", upstream_newer());
    }

    #[test]
    fn parse_release_json() {
        let v = json!({
            "tag_name": "v0.2.0-plus.2",
            "html_url": "https://github.com/WazeJinx/AltKeeperPlus/releases/tag/v0.2.0-plus.2",
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
