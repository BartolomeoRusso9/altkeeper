//! Login Apple ID (anisette e 2FA) e chiamate ai servizi sviluppatori:
//! elenco di team e App ID, download dei profili di provisioning.
//!
//! Non crea né revoca certificati: legge soltanto e scarica profili.

use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use isideload::{
    anisette::remote_v3::RemoteV3AnisetteProvider,
    auth::apple_account::{AppleAccount, TwoFactorCallbackParams, TwoFactorCallbackResponse},
    dev::{
        app_ids::{AppId, AppIdsApi, Profile},
        developer_session::DeveloperSession,
        device_type::DeveloperDeviceType,
        teams::{DeveloperTeam, TeamsApi},
    },
    util::fs_storage::FsStorage,
};
use serde::{Deserialize, Serialize};

use crate::phone::dbg;

const ACCOUNT_FILE: &str = "account.json";
const STATE_DIR: &str = "state";

/// Se `true` la callback 2FA chiede il codice sul terminale, altrimenti fallisce
/// (un servizio non presidiato non può inserire codici).
static INTERACTIVE: AtomicBool = AtomicBool::new(false);

pub fn set_interactive(on: bool) {
    INTERACTIVE.store(on, Ordering::SeqCst);
}

#[derive(Serialize, Deserialize)]
pub struct Account {
    pub apple_id: String,
    /// Vuota se l'utente ha scelto di non salvarla.
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub team_id: Option<String>,
}

pub fn load_account(dir: &Path) -> Result<Account, String> {
    let raw = std::fs::read_to_string(dir.join(ACCOUNT_FILE))
        .map_err(|_| "manca account.json: esegui prima `altkeeper login <apple-id>`".to_string())?;
    let acc: Account = serde_json::from_str(&raw).map_err(dbg)?;
    if acc.password.is_empty() {
        return Err("password non salvata: rifai `altkeeper login` e scegli di salvarla".into());
    }
    Ok(acc)
}

pub fn save_account(dir: &Path, acc: &Account) -> Result<(), String> {
    let path = dir.join(ACCOUNT_FILE);
    let data = serde_json::to_vec_pretty(acc).map_err(dbg)?;
    crate::fsperm::write_private(&path, &data).map_err(dbg)
}

/// Legge l'account salvato senza pretendere la password (serve a mostrarne lo stato).
pub fn read_account(dir: &Path) -> Option<Account> {
    let raw = std::fs::read_to_string(dir.join(ACCOUNT_FILE)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Cosa risponde chi usa la pagina web quando Apple chiede il codice 2FA.
pub enum TfaAnswer {
    Code(String),
    Resend,
}

/// Ponte tra il login (che chiede il codice 2FA) e la pagina web (che lo raccoglie).
#[derive(Default)]
pub struct TfaChannel {
    /// Il login sta aspettando un codice.
    pub asking: AtomicBool,
    /// Perché l'ultimo codice non è andato bene, se è il caso.
    pub last_error: Mutex<Option<String>>,
    tx: Mutex<Option<tokio::sync::oneshot::Sender<TfaAnswer>>>,
}

impl TfaChannel {
    /// Consegna la risposta al login in attesa. `false` se nessuno stava aspettando.
    pub fn answer(&self, answer: TfaAnswer) -> bool {
        let tx = self.tx.lock().unwrap().take();
        tx.map(|t| t.send(answer).is_ok()).unwrap_or(false)
    }
}

/// Il server anisette da usare: `None` = quello pubblico di isideload. Con
/// `ALTKEEPER_ANISETTE_URL` se ne può usare uno proprio (per esempio `anisette-v3-server`),
/// invece di pesare su un servizio di terzi.
pub fn anisette_url(value: Option<&str>) -> Result<Option<String>, String> {
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(u) if u.starts_with("https://") || u.starts_with("http://") => {
            Ok(Some(u.trim_end_matches('/').to_string()))
        }
        Some(_) => Err("ALTKEEPER_ANISETTE_URL deve iniziare con http:// o https://".into()),
    }
}

/// L'identità anisette del "dispositivo" resta su disco (`state/`): dopo il primo codice 2FA
/// Apple di solito non lo richiede più.
fn anisette_provider(dir: &Path) -> Result<RemoteV3AnisetteProvider, String> {
    let state = dir.join(STATE_DIR);
    std::fs::create_dir_all(&state).map_err(dbg)?;
    crate::fsperm::restrict_dir(&state).map_err(dbg)?;
    let mut provider = RemoteV3AnisetteProvider::default().map_err(|e| e.to_string())?;
    if let Some(url) = anisette_url(crate::env::var("ANISETTE_URL").as_deref())? {
        provider = provider.set_url(&url);
    }
    Ok(provider
        .set_storage(Box::new(FsStorage::new(state)))
        .set_serial_number("2".to_string()))
}

/// Data e ora in UTC come le scrive `NSISO8601DateFormatter` (es. `2026-09-21T19:00:00Z`).
pub fn iso8601_utc(epoch_secs: u64) -> String {
    let days = (epoch_secs / 86_400) as i64;
    let rem = epoch_secs % 86_400;
    // Da giorni dal 1970 a data civile (algoritmo di Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// Il server anisette risponde -45061 ("reprovision required") quando l'identità salvata in
/// `state/anisette_state` non vale più per lui: server cambiato, backend rifatto, `adi.pb`
/// perso. Riprovare con la stessa identità non serve a niente, bisogna provisionarne una nuova.
fn needs_reprovision(err: &str) -> bool {
    err.contains("-45061") || err.contains("reprovision required")
}

/// Mette da parte l'identità anisette scaduta (`anisette_state` -> `anisette_state.stale`):
/// al prossimo uso `isideload` ne crea e ne provisiona una nuova. `false` se non c'era niente.
fn retire_anisette_state(dir: &Path) -> Result<bool, String> {
    let file = dir.join(STATE_DIR).join("anisette_state");
    if !file.exists() {
        return Ok(false);
    }
    std::fs::rename(&file, file.with_extension("stale")).map_err(dbg)?;
    Ok(true)
}

/// I dati anisette che AltStore chiede al suo AltServer (`AnisetteDataRequest`): stesso
/// "dispositivo" del login (`state/`), formato `ALTAnisetteData.json()` di AltSign, tutte
/// stringhe. `deviceDescription` porta l'identità `akd` che Apple accetta.
///
/// Se il server dice "reprovision required" (-45061) l'identità salvata viene messa da parte
/// e si riprova una volta sola con una nuova, invece di restare bloccati finché qualcuno
/// non cancella `state/anisette_state` a mano.
pub async fn anisette_for_altstore(dir: &Path) -> Result<serde_json::Value, String> {
    match anisette_for_altstore_once(dir).await {
        Err(e) if needs_reprovision(&e) => {
            if retire_anisette_state(dir)? {
                println!(
                    "[anisette] il server non riconosce l'identità salvata ({e}); \
                     la metto da parte in state/anisette_state.stale e ne creo una nuova"
                );
                anisette_for_altstore_once(dir).await
            } else {
                Err(e)
            }
        }
        other => other,
    }
}

async fn anisette_for_altstore_once(dir: &Path) -> Result<serde_json::Value, String> {
    use isideload::{anisette::AnisetteDataGenerator, auth::grandslam::GrandSlam};

    let provider = anisette_provider(dir)?;
    let mut generator =
        AnisetteDataGenerator::new(Arc::new(tokio::sync::RwLock::new(provider)));
    let info = generator.get_client_info().await.map_err(|e| e.to_string())?;
    let gs = Arc::new(
        GrandSlam::new(info.clone(), false, None)
            .await
            .map_err(|e| e.to_string())?,
    );
    let data = generator.get_anisette_data(gs).await.map_err(|e| e.to_string())?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(serde_json::json!({
        "machineID": data.machine_id(),
        "oneTimePassword": data.one_time_password(),
        "localUserID": data.local_user_id(),
        "routingInfo": data.routing_info.clone(),
        "deviceUniqueIdentifier": data.device_unique_identifier(),
        "deviceSerialNumber": "2",
        "deviceDescription": info.client_info,
        "date": iso8601_utc(now),
        "locale": "en_US",
        "timeZone": "GMT",
    }))
}

#[cfg(test)]
mod tests {
    use super::{anisette_url, iso8601_utc, needs_reprovision};

    #[test]
    fn riconosce_la_richiesta_di_riprovisioning() {
        assert!(needs_reprovision(
            "not provisioned for this server (-45061); reprovision required: no usable account"
        ));
        assert!(needs_reprovision("reprovision required"));
        assert!(!needs_reprovision("unknown session (-45025)"));
    }

    #[test]
    fn server_anisette_proprio_dalla_variabile() {
        assert_eq!(anisette_url(None), Ok(None));
        assert_eq!(anisette_url(Some("")), Ok(None));
        assert_eq!(anisette_url(Some("   ")), Ok(None));
        assert_eq!(
            anisette_url(Some("http://192.168.1.3:6969/")),
            Ok(Some("http://192.168.1.3:6969".to_string()))
        );
        assert_eq!(
            anisette_url(Some(" https://ani.example.org ")),
            Ok(Some("https://ani.example.org".to_string()))
        );
        assert!(anisette_url(Some("ani.example.org")).is_err());
        assert!(anisette_url(Some("ftp://x")).is_err());
    }

    #[test]
    fn data_iso8601_come_la_scrive_apple() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso8601_utc(951_782_400), "2000-02-29T00:00:00Z"); // anno bisestile
        assert_eq!(iso8601_utc(1_782_432_000 + 86_399), "2026-06-26T23:59:59Z");
    }
}

/// Come `open_session`, ma il codice 2FA lo porta la pagina web tramite `chan`.
/// Se il codice non arriva entro 5 minuti il login viene annullato.
pub async fn open_session_web(
    dir: &Path,
    apple_id: &str,
    password: &str,
    chan: Arc<TfaChannel>,
) -> Result<DeveloperSession, String> {
    let provider = anisette_provider(dir)?;

    let get_2fa_code = move |params: TwoFactorCallbackParams| {
        let chan = chan.clone();
        async move {
            let (tx, rx) = tokio::sync::oneshot::channel();
            *chan.last_error.lock().unwrap() = params.last_error.clone();
            *chan.tx.lock().unwrap() = Some(tx);
            chan.asking.store(true, Ordering::SeqCst);
            let answer = tokio::time::timeout(Duration::from_secs(300), rx).await;
            chan.asking.store(false, Ordering::SeqCst);
            chan.tx.lock().unwrap().take();
            Ok(match answer {
                Ok(Ok(TfaAnswer::Code(code))) => TwoFactorCallbackResponse::SubmitCode(code),
                Ok(Ok(TfaAnswer::Resend)) => TwoFactorCallbackResponse::ResendCode,
                _ => TwoFactorCallbackResponse::Abort,
            })
        }
    };

    let mut account = AppleAccount::builder(apple_id)
        .anisette_provider(provider)
        .login(password, get_2fa_code)
        .await
        .map_err(|e| format!("login Apple ID non riuscito: {e}"))?;

    DeveloperSession::from_account(&mut account)
        .await
        .map_err(|e| format!("sessione sviluppatore non riuscita: {e}"))
}

/// Login e sessione verso i servizi sviluppatori di Apple.
pub async fn open_session(
    dir: &Path,
    apple_id: &str,
    password: &str,
) -> Result<DeveloperSession, String> {
    let provider = anisette_provider(dir)?;

    let get_2fa_code = async |params: TwoFactorCallbackParams| {
        if !INTERACTIVE.load(Ordering::SeqCst) {
            // Senza terminale non si può chiedere il codice: il login fallirà.
            return Ok(TwoFactorCallbackResponse::SubmitCode(String::new()));
        }
        if params.unknown {
            println!("L'ultimo metodo 2FA non ha funzionato, prova un altro.");
        } else {
            println!(
                "Inserisci il codice a 6 cifre inviato a {}:",
                if params.sms {
                    params
                        .numbers
                        .iter()
                        .find(|n| Some(n.id) == params.selected_number_id)
                        .map(|n| n.number_with_dial_code.clone())
                        .unwrap_or_else(|| "il tuo telefono".to_string())
                } else {
                    "i tuoi dispositivi".to_string()
                }
            );
        }
        println!("(d = invia ai dispositivi, r = reinvia, p<id> = SMS al numero <id>)");
        for n in params
            .numbers
            .iter()
            .filter(|n| Some(n.id) != params.selected_number_id)
        {
            println!("  ID {}: {}", n.id, n.number_with_dial_code);
        }

        let mut code = String::new();
        let _ = std::io::stdin().read_line(&mut code);
        let code = code.trim();

        if let Some(id) = code.strip_prefix('p').and_then(|s| s.parse::<u32>().ok()) {
            return Ok(TwoFactorCallbackResponse::SendSms(id));
        }
        if code == "d" {
            return Ok(TwoFactorCallbackResponse::SendToDevices);
        }
        if code == "r" && !params.unknown {
            return Ok(TwoFactorCallbackResponse::ResendCode);
        }
        Ok(TwoFactorCallbackResponse::SubmitCode(code.to_string()))
    };

    let mut account = AppleAccount::builder(apple_id)
        .anisette_provider(provider)
        .login(password, get_2fa_code)
        .await
        .map_err(|e| format!("login Apple ID non riuscito: {e}"))?;

    DeveloperSession::from_account(&mut account)
        .await
        .map_err(|e| format!("sessione sviluppatore non riuscita: {e}"))
}

/// Sceglie il team: quello indicato, l'unico disponibile, o (in interattivo) chiede.
pub async fn pick_team(
    sess: &mut DeveloperSession,
    wanted: Option<&str>,
    interactive: bool,
) -> Result<DeveloperTeam, String> {
    let teams = sess.list_teams().await.map_err(|e| e.to_string())?;
    if teams.is_empty() {
        return Err("nessun team sviluppatore su questo Apple ID".into());
    }
    if let Some(id) = wanted {
        return teams
            .into_iter()
            .find(|t| t.team_id == id)
            .ok_or_else(|| format!("il team {id} non esiste su questo Apple ID"));
    }
    if teams.len() == 1 || !interactive {
        return Ok(teams.into_iter().next().unwrap());
    }
    println!("Team disponibili:");
    for (i, t) in teams.iter().enumerate() {
        println!(
            "  {}: {} ({})",
            i + 1,
            t.name.as_deref().unwrap_or("<senza nome>"),
            t.team_id
        );
    }
    print!("Scegli il numero: ");
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
    let n: usize = s.trim().parse().map_err(|_| "scelta non valida".to_string())?;
    teams
        .into_iter()
        .nth(n.wrapping_sub(1))
        .ok_or_else(|| "scelta non valida".to_string())
}

pub async fn app_ids(
    sess: &mut DeveloperSession,
    team: &DeveloperTeam,
) -> Result<Vec<AppId>, String> {
    sess.list_app_ids(team, DeveloperDeviceType::Ios)
        .await
        .map(|r| r.app_ids)
        .map_err(|e| e.to_string())
}

pub async fn download_profile(
    sess: &mut DeveloperSession,
    team: &DeveloperTeam,
    app: &AppId,
) -> Result<Profile, String> {
    sess.download_team_provisioning_profile(team, app, DeveloperDeviceType::Ios)
        .await
        .map_err(|e| e.to_string())
}
