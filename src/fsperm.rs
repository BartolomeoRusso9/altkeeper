//! File e cartelle riservati (account.json, state/, profili salvati) su Unix e su Windows.
//!
//! Su Unix si usano i permessi classici (600 per i file, 700 per le cartelle). Su Windows non
//! esistono: si toglie l'ereditarietà dei permessi e si lascia l'accesso solo all'utente
//! corrente, con `icacls` (incluso in Windows). Se `icacls` non riesce non è un errore: la
//! cartella dell'utente è comunque privata per impostazione predefinita.

use std::io::Write as _;
use std::path::Path;

/// Scrive `data` in `path` (lo crea o lo sovrascrive) lasciandolo leggibile solo dall'utente.
pub fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(data)?;
    drop(f);
    restrict_file(path)
}

/// Rende il file leggibile e scrivibile solo dall'utente.
pub fn restrict_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }
    #[cfg(windows)]
    {
        windows_acl(path, false);
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(())
    }
}

/// Rende la cartella (e quello che ci verrà creato dentro) accessibile solo all'utente.
pub fn restrict_dir(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        windows_acl(path, true);
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(windows)]
fn windows_acl(path: &Path, dir: bool) {
    use std::process::{Command, Stdio};

    let Ok(user) = std::env::var("USERNAME") else {
        return;
    };
    let who = match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{user}"),
        _ => user,
    };
    // (OI)(CI): i file creati dentro la cartella ereditano lo stesso permesso.
    let grant = format!("{who}:{}", if dir { "(OI)(CI)F" } else { "F" });
    let _ = Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(grant)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrive_e_rilegge_un_file_riservato() {
        let dir = std::env::temp_dir().join(format!("altkeeper-fsperm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        restrict_dir(&dir).unwrap();
        let file = dir.join("segreto.json");
        write_private(&file, b"uno").unwrap();
        write_private(&file, b"due").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"due");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
