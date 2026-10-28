use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const REGISTRY_FILE: &str = "state/devices.json";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
}

impl Device {
    pub fn directory(&self, root: &Path) -> PathBuf {
        if self.id == "default" {
            root.to_path_buf()
        } else {
            root.join("devices").join(&self.id)
        }
    }
}

#[derive(Deserialize, Serialize)]
struct Registry {
    devices: Vec<Device>,
}

pub fn load(root: &Path) -> Result<Vec<Device>, String> {
    let path = root.join(REGISTRY_FILE);
    match std::fs::read(&path) {
        Ok(bytes) => {
            let registry: Registry = serde_json::from_slice(&bytes)
                .map_err(|e| format!("Cannot read device registry: {e}"))?;
            if registry.devices.is_empty()
                || !registry.devices.iter().any(|device| device.id == "default")
                || registry.devices.iter().any(|device| {
                    !valid_id(&device.id)
                        || device.name.trim().is_empty()
                        || device.name.chars().count() > 40
                        || device.name.chars().any(char::is_control)
                })
            {
                return Err("Device registry is invalid.".into());
            }
            Ok(registry.devices)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let devices = vec![Device {
                id: "default".into(),
                name: "Default iPhone".into(),
            }];
            save(root, &devices)?;
            Ok(devices)
        }
        Err(error) => Err(format!("Cannot read device registry: {error}")),
    }
}

pub fn add(root: &Path, devices: &mut Vec<Device>, name: &str) -> Result<Device, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 || name.chars().any(char::is_control) {
        return Err("Device name must be 1-40 characters without control characters.".into());
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("Cannot create device ID: {e}"))?
        .as_nanos();
    let id = format!("device-{nonce:x}-{}", std::process::id());
    let device = Device {
        id,
        name: name.to_owned(),
    };
    let directory = device.directory(root);
    std::fs::create_dir_all(&directory).map_err(crate::phone::dbg)?;
    crate::fsperm::restrict_dir(&directory).map_err(crate::phone::dbg)?;
    devices.push(device.clone());
    if let Err(error) = save(root, devices) {
        devices.pop();
        return Err(error);
    }
    Ok(device)
}

pub fn save(root: &Path, devices: &[Device]) -> Result<(), String> {
    let state = root.join("state");
    std::fs::create_dir_all(&state).map_err(crate::phone::dbg)?;
    crate::fsperm::restrict_dir(&state).map_err(crate::phone::dbg)?;
    let data = serde_json::to_vec_pretty(&Registry {
        devices: devices.to_vec(),
    })
    .map_err(crate::phone::dbg)?;
    let path = root.join(REGISTRY_FILE);
    let temp = root.join("state/devices.json.tmp");
    crate::fsperm::write_private(&temp, &data).map_err(crate::phone::dbg)?;
    if let Err(error) = std::fs::rename(temp, path) {
        return Err(format!("Cannot save device registry: {error}"));
    }
    Ok(())
}

pub fn active_id(root: &Path, devices: &[Device]) -> Result<String, String> {
    let path = root.join("state/active-device");
    match std::fs::read_to_string(path) {
        Ok(id) if devices.iter().any(|device| device.id == id.trim()) => Ok(id.trim().to_owned()),
        Ok(_) => Err("Saved active device is not present in the device registry.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok("default".into()),
        Err(error) => Err(format!("Cannot read selected device: {error}")),
    }
}

pub fn save_active(root: &Path, id: &str) -> Result<(), String> {
    let state = root.join("state");
    std::fs::create_dir_all(&state).map_err(crate::phone::dbg)?;
    crate::fsperm::restrict_dir(&state).map_err(crate::phone::dbg)?;
    crate::fsperm::write_private(&root.join("state/active-device"), id.as_bytes())
        .map_err(crate::phone::dbg)
}

fn valid_id(id: &str) -> bool {
    id == "default"
        || (id.starts_with("device-")
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_directory_is_the_default_profile() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("account.json"), b"legacy").unwrap();
        let devices = load(root.path()).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].directory(root.path()), root.path());
        assert_eq!(
            std::fs::read(root.path().join("account.json")).unwrap(),
            b"legacy"
        );
        assert_eq!(active_id(root.path(), &devices).unwrap(), "default");
    }

    #[test]
    fn new_devices_get_an_isolated_directory_and_active_selection() {
        let root = tempfile::tempdir().unwrap();
        let mut devices = load(root.path()).unwrap();
        let device = add(root.path(), &mut devices, "Work iPhone").unwrap();
        assert_ne!(device.directory(root.path()), root.path());
        assert!(device.directory(root.path()).is_dir());
        save_active(root.path(), &device.id).unwrap();
        assert_eq!(active_id(root.path(), &devices).unwrap(), device.id);
        assert_eq!(load(root.path()).unwrap().len(), 2);
    }

    #[test]
    fn rejects_invalid_device_names() {
        let root = tempfile::tempdir().unwrap();
        let mut devices = load(root.path()).unwrap();
        assert!(add(root.path(), &mut devices, " ").is_err());
        assert!(add(root.path(), &mut devices, "invalid\nname").is_err());
    }
}
