//! Probe isolato per verificare il percorso lockdown classico su un iPhone USB.
//!
//! Eseguire con:
//!   cargo run --bin ios12_lockdown_probe
//!
//! Non modifica profili e non installa/rimuove applicazioni.

use std::time::Duration;

use idevice::{
    misagent::MisagentClient,
    provider::IdeviceProvider,
    services::lockdown::LockdownClient,
    usbmuxd::{Connection, UsbmuxdAddr},
    IdeviceService,
};

fn dbg<E: std::fmt::Debug>(e: E) -> String {
    format!("{e:?}")
}

async fn with_timeout<T>(
    what: &str,
    future: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    match tokio::time::timeout(Duration::from_secs(20), future).await {
        Ok(result) => result,
        Err(_) => Err(format!("{what}: timeout dopo 20 secondi")),
    }
}

async fn run() -> Result<(), String> {
    println!("[1/5] Connessione a usbmuxd...");
    let addr = UsbmuxdAddr::from_env_var().map_err(dbg)?;
    let mut mux = with_timeout("connessione usbmuxd", async {
        addr.connect(1).await.map_err(dbg)
    })
    .await?;

    let devices = with_timeout("elenco dispositivi USB", async {
        mux.get_devices().await.map_err(dbg)
    })
    .await?;

    let device = devices
        .iter()
        .find(|device| device.connection_type == Connection::Usb)
        .ok_or_else(|| "Nessun dispositivo USB trovato da usbmuxd".to_string())?;

    println!("[2/5] Dispositivo USB trovato; preparo il provider...");
    let provider = device.to_provider(addr, "altkeeper-ios12-probe");

    println!("[3/5] Apro lockdown e leggo ProductVersion...");
    let mut lockdown = with_timeout("LockdownClient::connect", async {
        LockdownClient::connect(&provider).await.map_err(dbg)
    })
    .await?;

    let version_value = with_timeout("lettura ProductVersion", async {
        lockdown
            .get_value(Some("ProductVersion"), None)
            .await
            .map_err(dbg)
    })
    .await?;

    let version = version_value
        .as_string()
        .map(str::to_owned)
        .ok_or_else(|| "Lockdown non ha restituito ProductVersion come stringa".to_string())?;

    println!("      ProductVersion = {version}");

    println!("[4/5] Carico il pairing file e avvio la sessione lockdown...");
    let pairing = with_timeout("lettura pairing file", async {
        provider.get_pairing_file().await.map_err(dbg)
    })
    .await?;

    with_timeout(
        "LockdownClient::start_session (incl. negoziazione TLS)",
        async { lockdown.start_session(&pairing).await.map_err(dbg) },
    )
    .await?;

    let name_value = with_timeout("lettura DeviceName dopo start_session", async {
        lockdown
            .get_value(Some("DeviceName"), None)
            .await
            .map_err(dbg)
    })
    .await?;

    let device_name = name_value
        .as_string()
        .map(str::to_owned)
        .ok_or_else(|| "Lockdown non ha restituito DeviceName come stringa".to_string())?;

    println!("      DeviceName = {device_name}");
    drop(lockdown);

    println!("[5/5] Apro Misagent e leggo i profili (sola lettura)...");
    let mut misagent = with_timeout("MisagentClient::connect", async {
        MisagentClient::connect(&provider).await.map_err(dbg)
    })
    .await?;

    let profiles = with_timeout("MisagentClient::copy_all", async {
        misagent.copy_all().await.map_err(dbg)
    })
    .await?;

    println!("OK: lockdown, sessione e Misagent riusciti.");
    println!("iOS: {version}; profili leggibili: {}", profiles.len());
    println!("Questo test non installa profili né IPA e non verifica la firma delle app.");
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("PROBE FALLITO: {error}");
        std::process::exit(1);
    }
}
