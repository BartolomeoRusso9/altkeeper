//! Login Apple ID (anisette e 2FA), chiamate ai servizi sviluppatori e firma IPA.
//! Il rinnovo scarica profili; la firma può registrare il dispositivo e creare certificati.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use isideload::{
    anisette::remote_v3::RemoteV3AnisetteProvider,
    auth::apple_account::{AppleAccount, TwoFactorCallbackParams, TwoFactorCallbackResponse},
    dev::{
        app_ids::{AppId, AppIdsApi, Profile},
        devices::DevicesApi,
        developer_session::DeveloperSession,
        device_type::DeveloperDeviceType,
        teams::{DeveloperTeam, TeamsApi},
    },
    sideload::{SideloaderBuilder, builder::MaxCertsBehavior},
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

/// Interpreta il codice 2FA e i comandi di selezione senza inviare tentativi
/// malformati ad Apple. Un codice può iniziare con zero.
fn parse_2fa_input(input: &str, unknown: bool) -> Option<TwoFactorCallbackResponse> {
    let input = input.trim();
    match input {
        "d" => Some(TwoFactorCallbackResponse::SendToDevices),
        "r" if !unknown => Some(TwoFactorCallbackResponse::ResendCode),
        _ if input.len() == 6 && input.bytes().all(|b| b.is_ascii_digit()) => {
            Some(TwoFactorCallbackResponse::SubmitCode(input.to_owned()))
        }
        _ => input
            .strip_prefix('p')
            .and_then(|id| id.parse::<u32>().ok())
            .map(TwoFactorCallbackResponse::SendSms),
    }
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
        .map_err(|_| crate::tr!("manca account.json: esegui prima `altkeeper login <apple-id>`", "account.json is missing: run `altkeeper login <apple-id>` first").to_string())?;
    let acc: Account = serde_json::from_str(&raw).map_err(dbg)?;
    if acc.password.is_empty() {
        return Err(crate::tr!("password non salvata: rifai `altkeeper login` e scegli di salvarla", "password was not saved: rerun `altkeeper login` and choose to save it").into());
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
        Some(_) => Err(crate::tr!("ALTKEEPER_ANISETTE_URL deve iniziare con http:// o https://", "ALTKEEPER_ANISETTE_URL must start with http:// or https://").into()),
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
                println!("{}", crate::tr!(
                    format!("[anisette] il server non riconosce l'identità salvata ({e}); la metto da parte in state/anisette_state.stale e ne creo una nuova"),
                    format!("[anisette] server did not recognize the saved identity ({e}); moved it to state/anisette_state.stale and provisioning a new one")
                ));
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
    use super::{anisette_url, is_max_certs_error, iso8601_utc, max_certs_behavior, needs_reprovision, parse_2fa_input};
    use isideload::{
        auth::apple_account::TwoFactorCallbackResponse,
        sideload::builder::MaxCertsBehavior,
    };

    #[test]
    fn accetta_codici_2fa_e_zero_iniziale() {
        assert!(matches!(
            parse_2fa_input(" 012345\r\n", false),
            Some(TwoFactorCallbackResponse::SubmitCode(code)) if code == "012345"
        ));
        assert!(matches!(
            parse_2fa_input("999999", true),
            Some(TwoFactorCallbackResponse::SubmitCode(code)) if code == "999999"
        ));
    }

    #[test]
    fn riconosce_opzioni_2fa() {
        assert!(matches!(parse_2fa_input("d", false), Some(TwoFactorCallbackResponse::SendToDevices)));
        assert!(matches!(parse_2fa_input("r", false), Some(TwoFactorCallbackResponse::ResendCode)));
        assert!(matches!(parse_2fa_input("p1", false), Some(TwoFactorCallbackResponse::SendSms(1))));
        assert!(matches!(parse_2fa_input("p42", true), Some(TwoFactorCallbackResponse::SendSms(42))));
    }

    #[test]
    fn scarta_input_2fa_non_valido_senza_contattare_apple() {
        for invalid in ["", " ", "12345", "1234567", "12 456", "abcdef", "p", "pabc", "R"] {
            assert!(parse_2fa_input(invalid, false).is_none(), "{invalid:?}");
        }
        assert!(parse_2fa_input("r", true).is_none());
    }


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

    #[test]
    fn revoca_certificati_solo_se_richiesta() {
        assert!(matches!(max_certs_behavior(None), MaxCertsBehavior::Error));
        assert!(matches!(max_certs_behavior(Some("0")), MaxCertsBehavior::Error));
        assert!(matches!(max_certs_behavior(Some("1")), MaxCertsBehavior::Revoke));
        assert!(is_max_certs_error("error code 7460: You already have"));
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
        .map_err(|e| crate::tr!(format!("login Apple ID non riuscito: {e}"), format!("Apple ID sign-in failed: {e}")))?;

    DeveloperSession::from_account(&mut account)
        .await
        .map_err(|e| crate::tr!(format!("sessione sviluppatore non riuscita: {e}"), format!("Developer session failed: {e}")))
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
            // Non inviare un codice vuoto ai server Apple in esecuzione automatica.
            return Ok(TwoFactorCallbackResponse::Abort);
        }
        if params.unknown {
            println!("{}", crate::tr!(
                "L'ultimo metodo 2FA non ha funzionato, prova un altro.",
                "The previous 2FA method did not work; try another one."
            ));
        } else {
            let destination = if params.sms {
                params
                    .numbers
                    .iter()
                    .find(|n| Some(n.id) == params.selected_number_id)
                    .map(|n| n.number_with_dial_code.clone())
                    .unwrap_or_else(|| crate::tr!("il tuo telefono", "your phone").to_string())
            } else {
                crate::tr!("i tuoi dispositivi", "your devices").to_string()
            };
            println!("{}", crate::tr!(
                format!("Inserisci il codice a 6 cifre inviato a {destination}:"),
                format!("Enter the six-digit code sent to {destination}:")
            ));
        }
        if params.unknown {
            println!("{}", crate::tr!(
                "(d = invia ai dispositivi, p<id> = SMS al numero <id>)",
                "(d = send to devices, p<id> = SMS to number <id>)"
            ));
        } else {
            println!("{}", crate::tr!(
                "(d = invia ai dispositivi, r = reinvia, p<id> = SMS al numero <id>)",
                "(d = send to devices, r = resend, p<id> = SMS to number <id>)"
            ));
        }
        for n in params
            .numbers
            .iter()
            .filter(|n| Some(n.id) != params.selected_number_id)
        {
            println!("  ID {}: {}", n.id, n.number_with_dial_code);
        }

        loop {
            // Non usare stdin().read_line(): in PowerShell lo stdin del processo
            // può essere una pipe a EOF nonostante la console funzioni.
            let code = match crate::terminal::prompt_line(crate::tr!("Codice 2FA o comando: ", "2FA code or command: ")) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("{}", crate::tr!(format!("Impossibile leggere il codice 2FA dal terminale: {e}"), format!("Cannot read 2FA code from the terminal: {e}")));
                    return Ok(TwoFactorCallbackResponse::Abort);
                }
            };

            if let Some(response) = parse_2fa_input(&code, params.unknown) {
                return Ok(response);
            }
            println!("{}", if params.unknown {
                crate::tr!(
                    "Inserisci un codice di 6 cifre valido oppure d o p<id>.",
                    "Enter a valid six-digit code, d or p<id>."
                )
            } else {
                crate::tr!(
                    "Inserisci un codice di 6 cifre valido oppure d, r o p<id>.",
                    "Enter a valid six-digit code, d, r or p<id>."
                )
            });
        }
    };

    let mut account = AppleAccount::builder(apple_id)
        .anisette_provider(provider)
        .login(password, get_2fa_code)
        .await
        .map_err(|e| crate::tr!(format!("login Apple ID non riuscito: {e}"), format!("Apple ID sign-in failed: {e}")))?;

    DeveloperSession::from_account(&mut account)
        .await
        .map_err(|e| crate::tr!(format!("sessione sviluppatore non riuscita: {e}"), format!("Developer session failed: {e}")))
}

/// Sceglie il team: quello indicato, l'unico disponibile, o (in interattivo) chiede.
pub async fn pick_team(
    sess: &mut DeveloperSession,
    wanted: Option<&str>,
    interactive: bool,
) -> Result<DeveloperTeam, String> {
    let teams = sess.list_teams().await.map_err(|e| e.to_string())?;
    if teams.is_empty() {
        return Err(crate::tr!("nessun team sviluppatore su questo Apple ID", "there are no developer teams for this Apple ID").into());
    }
    if let Some(id) = wanted {
        return teams
            .into_iter()
            .find(|t| t.team_id == id)
            .ok_or_else(|| crate::tr!(format!("il team {id} non esiste su questo Apple ID"), format!("Team {id} does not exist for this Apple ID")));
    }
    if teams.len() == 1 || !interactive {
        return Ok(teams.into_iter().next().unwrap());
    }
    println!("{}", crate::tr!("Team disponibili:", "Available teams:"));
    for (i, t) in teams.iter().enumerate() {
        println!(
            "  {}: {} ({})",
            i + 1,
            t.name.as_deref().unwrap_or(crate::tr!("<senza nome>", "<unnamed>")),
            t.team_id
        );
    }
    let s = crate::terminal::prompt_line(crate::tr!("Scegli il numero: ", "Choose a number: ")).map_err(dbg)?;
    let n: usize = s.trim().parse().map_err(|_| crate::tr!("scelta non valida", "invalid selection").to_string())?;
    teams
        .into_iter()
        .nth(n.wrapping_sub(1))
        .ok_or_else(|| crate::tr!("scelta non valida", "invalid selection").to_string())
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

pub struct SignedApp {
    pub path: PathBuf,
    cleanup_dir: PathBuf,
}

impl Drop for SignedApp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.cleanup_dir);
    }
}

/// `ALTKEEPER_REVOKE_CERTS=1` autorizza a revocare i certificati di sviluppo già presenti quando
/// Apple ha raggiunto il limite (errore 7460). Di default si ferma con un errore, perché la
/// revoca invalida le app firmate con quei certificati.
pub fn max_certs_behavior(value: Option<&str>) -> MaxCertsBehavior {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("1" | "true" | "yes" | "si" | "sì" | "on") => MaxCertsBehavior::Revoke,
        _ => MaxCertsBehavior::Error,
    }
}

fn is_max_certs_error(detail: &str) -> bool {
    detail.contains("7460") || detail.contains("Maximum number of certificates")
}

/// Firma un IPA per il team configurato, registrando l'iPhone se necessario.
pub async fn sign_ipa(
    mut sess: DeveloperSession,
    account: &Account,
    team: DeveloperTeam,
    device_name: &str,
    udid: &str,
    ipa: &Path,
) -> Result<SignedApp, String> {
    let filename = ipa
        .file_name()
        .ok_or(crate::tr!("nome del file IPA mancante", "IPA filename is missing"))?
        .to_string_lossy();
    let cleanup_dir = std::env::temp_dir().join(format!("{filename}_extracted"));
    let storage_path = Path::new(STATE_DIR).join("sideload");
    std::fs::create_dir_all(&storage_path).map_err(dbg)?;
    crate::fsperm::restrict_dir(&storage_path).map_err(dbg)?;

    sess.ensure_device_registered(&team, device_name, udid, DeveloperDeviceType::Ios)
        .await
        .map_err(|e| crate::tr!(format!("registrazione iPhone non riuscita: {e}"), format!("iPhone registration failed: {e}")))?;

    let mut sideloader = SideloaderBuilder::new(sess, account.apple_id.clone())
        .storage(Box::new(FsStorage::new(storage_path)))
        .machine_name("AltKeeper".into())
        .max_certs_behavior(max_certs_behavior(
            crate::env::var("REVOKE_CERTS").as_deref(),
        ))
        .build();
    let signed = sideloader
        .sign_app::<_, std::future::Ready<()>>(
            ipa.to_path_buf(),
            Some(team),
            false,
            None::<fn(f32) -> std::future::Ready<()>>,
        )
        .await
        .map_err(|e| {
            let _ = std::fs::remove_dir_all(&cleanup_dir);
            let detail = format!("{e:?}");
            if is_max_certs_error(&detail) {
                crate::tr!(
                    "firma dell'app non riuscita: l'Apple ID ha già un certificato (AltStore/SideStore). Per sostituirlo imposta ALTKEEPER_REVOKE_CERTS=1 e riprova; le altre app vanno poi rinnovate con un Refresh.".to_string(),
                    "App signing failed: this Apple ID already has a certificate (AltStore/SideStore). To replace it, set ALTKEEPER_REVOKE_CERTS=1 and retry; other apps will then need refreshing.".to_string()
                )
            } else {
                crate::tr!(format!("firma dell'app non riuscita: {e}"), format!("App signing failed: {e}"))
            }
        })?;
    Ok(SignedApp {
        path: signed.0,
        cleanup_dir,
    })
}
