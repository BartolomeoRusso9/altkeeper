//! altkeeper: rinnova via Wi-Fi i profili di provisioning delle app installate
//! con un Apple ID gratuito, senza computer collegato e senza VPN.
//!
//!   altkeeper phone [--phone ip:porta]            legge i profili dal telefono
//!   altkeeper pair-usb [file]                     crea il pairing remoto via USB
//!   altkeeper login <apple-id>                    accede e salva l'account
//!   altkeeper apps                                elenca App ID e scadenze
//!   altkeeper renew [--phone ip:porta] [--dry-run] [--force] [--min-days N]
//!   altkeeper profile-remove <uuid> [--phone ip:porta]   toglie un profilo (ne salva prima una copia)
//!   altkeeper profile-restore <file> [--phone ip:porta]  rimette un profilo salvato
//!   altkeeper serve [--bind ip:porta] [--pin PIN] [--dir cartella] [--phone ip:porta]
//!                    [--altserver] [--altserver-port N] [--dns-domain D --dns-address IP]
//!                                                  interfaccia web (stato, rinnovo, accesso Apple ID)
//!   altkeeper altserver [--port N] [--dir cartella] [--phone ip:porta]
//!                    [--dns-domain D --dns-address IP [--dns-port N]]
//!                                                  fa da AltServer per AltStore (Refresh All e installazioni)
//!
//! Se non indichi il telefono (`--phone` o ALTKEEPER_PHONE) lo cerca in rete via Bonjour.
//!
//! Con `--dns-domain` pubblica AltServer anche come record DNS normali, così AltStore lo trova
//! da fuori casa attraverso una VPN (iOS non manda Bonjour multicast nei tunnel, le query DNS sì).

mod altserver;
mod apple;
mod dnssd;
mod env;
mod fsperm;
mod i18n;
mod phone;
mod profile;
mod terminal;
mod web;

use std::net::SocketAddr;
use std::path::Path;

use phone::dbg;

const PAIRING_FILE: &str = "rp-pairing.plist";

/// Yes is opt-in: empty or unrecognized responses must never save a password.
fn save_password_confirmed(answer: &str) -> bool {
    matches!(
        answer.trim().to_lowercase().as_str(),
        "s" | "si" | "sì" | "y" | "yes"
    )
}


const USAGE: &str = "uso: altkeeper <phone|pair-usb|login|apps|renew|profile-remove|\
                     profile-restore|serve|altserver|version> [opzioni]";
const USAGE_EN: &str = "usage: altkeeper <phone|pair-usb|login|apps|renew|profile-remove|\
                     profile-restore|serve|altserver|version> [options]";

/// Dove `profile-remove` tiene la copia dei profili tolti dal telefono.
const SAVED_DIR: &str = "profili-salvati";

/// Indirizzo del servizio `_remotepairing._tcp` dell'iPhone (di solito porta 49152):
/// da `--phone ip:porta` oppure dalla variabile d'ambiente ALTKEEPER_PHONE. Se manca non è
/// un errore: il telefono si cerca in rete.
fn phone_addr(opt: Option<String>) -> Result<Option<SocketAddr>, String> {
    opt.or_else(|| env::var("PHONE"))
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            s.trim()
                .parse()
                .map_err(|e| crate::tr!(format!("indirizzo del telefono non valido: {e}"), format!("invalid phone address: {e}")))
        })
        .transpose()
}

/// Porta su cui ascolta AltServer: `--altserver-port`/`--port`, altrimenti 49500.
fn altserver_port(opt: Option<String>) -> Result<u16, String> {
    opt.map(|v| v.parse::<u16>().map_err(|e| crate::tr!(format!("porta non valida: {e}"), format!("invalid port: {e}"))))
        .transpose()
        .map(|p| p.unwrap_or(49500))
}

/// Legge una rete nella forma `10.7.0.0/24`.
fn parse_cidr(s: &str) -> Result<(std::net::Ipv4Addr, u8), String> {
    let (ip, bits) = s
        .split_once('/')
        .ok_or_else(|| crate::tr!(format!("--dns-clients {s}: manca la lunghezza, per esempio 10.7.0.0/24"), format!("--dns-clients {s}: prefix length missing, e.g. 10.7.0.0/24")))?;
    let ip = ip.parse().map_err(|e| crate::tr!(format!("--dns-clients: indirizzo non valido: {e}"), format!("--dns-clients: invalid address: {e}")))?;
    let bits: u8 = bits.parse().map_err(|e| crate::tr!(format!("--dns-clients: /{bits} non valido: {e}"), format!("--dns-clients: invalid /{bits}: {e}")))?;
    if bits > 32 {
        return Err(crate::tr!(format!("--dns-clients: /{bits} non esiste, il massimo è /32"), format!("--dns-clients: /{bits} is invalid (maximum /32)")));
    }
    Ok((ip, bits))
}

/// Avvia, se richiesto, il server DNS che pubblica AltServer come record normali (DNS-SD
/// unicast). Serve per farsi trovare da AltStore quando il telefono non è sulla rete di casa:
/// iOS non manda le richieste Bonjour multicast dentro le VPN, ma le query DNS sì.
fn start_dnssd(opt: impl Fn(&str) -> Option<String>, altserver_port: u16) -> Result<(), String> {
    let Some(domain) = opt("--dns-domain") else { return Ok(()) };
    let address: std::net::Ipv4Addr = opt("--dns-address")
        .ok_or(crate::tr!(
            "--dns-domain vuole anche --dns-address <ip>: l'indirizzo a cui il telefono si collega, di solito quello della VPN di questa macchina",
            "--dns-domain requires --dns-address <ip>: the address the phone connects to, usually this machine's VPN address"
        ))?
        .parse()
        .map_err(|e| crate::tr!(format!("--dns-address non valido: {e}"), format!("invalid --dns-address: {e}")))?;
    let port: u16 = match opt("--dns-port") {
        Some(p) => p.parse().map_err(|e| crate::tr!(format!("--dns-port non valido: {e}"), format!("invalid --dns-port: {e}")))?,
        None => 5533,
    };
    // Da dove arrivano i client: serve perché iOS scopre il dominio chiedendo del "rovescio"
    // del proprio indirizzo. Se non si indica, si assume la /24 dell'indirizzo pubblicato.
    let clients = match opt("--dns-clients") {
        Some(cidr) => parse_cidr(&cidr)?,
        None => (address, 24),
    };
    let zone = dnssd::Zone::new(
        &domain,
        &altserver::instance_name(),
        &altserver::id()?,
        altserver_port,
        address,
        clients,
    );
    dnssd::start(zone, SocketAddr::from(([0, 0, 0, 0], port)))?;
    Ok(())
}

/// Si collega al telefono, all'indirizzo indicato o a quello trovato in rete.
async fn open_phone(
    addr: Option<SocketAddr>,
    say: &mut dyn FnMut(&str),
) -> Result<phone::PhoneLink, String> {
    let (found, link) = phone::connect_auto(addr, PAIRING_FILE, &mut *say).await?;
    if found == phone::Via::Usb {
        say(crate::tr!("Collegato via USB", "Connected via USB"));
    } else if addr.map(phone::Via::Wifi) != Some(found) {
        say(&crate::tr!(format!("Telefono trovato a {found}"), format!("Found phone at {found}")));
    }
    Ok(link)
}

/// Legge e interpreta tutti i profili presenti sul telefono.
async fn phone_profiles(
    link: &mut phone::PhoneLink,
) -> Result<Vec<profile::PhoneProfile>, String> {
    let raw = link.misagent.copy_all().await.map_err(dbg)?;
    Ok(raw.iter().filter_map(|r| profile::parse(r)).collect())
}

fn profile_line(p: &profile::PhoneProfile) -> String {
    let expiry = p.expires_text();
    let days = p.days_left().map(|d| d.to_string()).unwrap_or("?".into());
    crate::tr!(
        format!("   {}  [{}]  scade {expiry} ({days} giorni)", p.name, p.uuid),
        format!("   {}  [{}]  expires {expiry} ({days} days)", p.name, p.uuid),
    )
}

fn print_profile(p: &profile::PhoneProfile) {
    println!("{}", profile_line(p));
}

async fn cmd_phone(addr: Option<SocketAddr>) -> Result<(), String> {
    let mut link = open_phone(addr, &mut |s| println!("{s}")).await?;
    let profiles = phone_profiles(&mut link).await?;
    println!("{}", crate::tr!(format!("{} profili installati sul telefono:", profiles.len()), format!("{} profiles installed on the phone:", profiles.len())));
    profiles.iter().for_each(print_profile);
    Ok(())
}

async fn cmd_login(apple_id: Option<String>) -> Result<(), String> {
    let apple_id = apple_id.ok_or(crate::tr!("uso: altkeeper login <apple-id>", "usage: altkeeper login <apple-id>"))?;
    let password = rpassword::prompt_password(crate::tr!(format!("Password di {apple_id}: "), format!("Password for {apple_id}: ")))
        .map_err(dbg)?;
    let password = password.trim().to_string();
    if password.is_empty() {
        return Err(crate::tr!("password vuota", "empty password").into());
    }

    apple::set_interactive(true);
    let mut sess = apple::open_session(Path::new("."), &apple_id, &password).await?;
    let team = apple::pick_team(&mut sess, None, true).await?;
    println!("{}", crate::tr!(
        format!("Accesso riuscito. Team: {} ({})", team.name.as_deref().unwrap_or("<senza nome>"), team.team_id),
        format!("Signed in successfully. Team: {} ({})", team.name.as_deref().unwrap_or("<unnamed>"), team.team_id)
    ));

    let answer = match terminal::prompt_line(crate::tr!(
        "Salvare la password su questo server per i rinnovi automatici (file riservato a root)? [s/N] ",
        "Save the password on this server for automatic renewals (restricted file)? [y/N] ",
    )) {
        Ok(answer) => answer,
        // A noninteractive session may still save the account without its password.
        // EOF must never be interpreted as approval to store credentials.
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => String::new(),
        Err(e) => return Err(dbg(e)),
    };
    let save = save_password_confirmed(&answer);

    apple::save_account(
        Path::new("."),
        &apple::Account {
            apple_id,
            password: if save { password } else { String::new() },
            team_id: Some(team.team_id),
        },
    )?;
    println!("{}", crate::tr!("Account salvato in account.json (permessi 600).", "Account saved to account.json (restricted permissions)."));
    Ok(())
}

async fn cmd_apps() -> Result<(), String> {
    let acc = apple::load_account(Path::new("."))?;
    apple::set_interactive(false);
    let mut sess = apple::open_session(Path::new("."), &acc.apple_id, &acc.password).await?;
    let team = apple::pick_team(&mut sess, acc.team_id.as_deref(), false).await?;
    let ids = apple::app_ids(&mut sess, &team).await?;
    println!("Team {}: {} App ID", team.team_id, ids.len());
    for a in &ids {
        let exp = a
            .expiration_date
            .map(|d| d.to_xml_format())
            .unwrap_or_else(|| "-".into());
        println!("{}", crate::tr!(format!("   {}  ({})  scade {}", a.identifier, a.name, exp), format!("   {}  ({})  expires {}", a.identifier, a.name, exp)));
    }
    Ok(())
}

async fn cmd_renew(
    addr: Option<SocketAddr>,
    dry_run: bool,
    force: bool,
    min_days: i64,
    say: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let acc = apple::load_account(Path::new("."))?;
    let team_id = acc.team_id.clone().ok_or("team not saved: sign in again")?;

    say("Connecting to the phone...");
    let mut link = open_phone(addr, &mut *say).await?;
    let profiles = phone_profiles(&mut link).await?;
    let mine: Vec<&profile::PhoneProfile> = profiles
        .iter()
        .filter(|p| p.team_id.as_deref() == Some(team_id.as_str()))
        .collect();
    say(&format!(
        "{} profiles on the phone, {} for team {team_id}",
        profiles.len(),
        mine.len()
    ));
    if mine.is_empty() {
        return Ok(());
    }

    // Dopo un rinnovo i profili vecchi restano sul telefono: per ogni bundle vale quello che
    // scade più tardi, e si rinnova una volta sola (altrimenti ogni giro rifarebbe il rinnovo
    // finché il profilo vecchio non scade).
    let latest = profile::latest_by_bundle(mine);

    // Si decide cosa rinnovare prima di parlare con Apple: se niente è in scadenza non si
    // accede affatto (meno login, meno rischio che Apple limiti le richieste).
    let mut due: Vec<(&str, i64)> = Vec::new();
    for p in latest {
        let Some(bundle) = p.bundle_id.as_deref() else {
            say(&format!("   {}: bundle ID cannot be read, skipping", p.name));
            continue;
        };
        let days = p.days_left().unwrap_or(-1);
        if !force && days > min_days {
            say(&format!("   {bundle}: expires in {days} days, no renewal needed"));
            continue;
        }
        due.push((bundle, days));
    }
    if due.is_empty() {
        say("Nothing to renew: skipping Apple sign-in.");
        return Ok(());
    }

    say("Signing in to Apple...");
    apple::set_interactive(false);
    let mut sess = apple::open_session(Path::new("."), &acc.apple_id, &acc.password).await?;
    let team = apple::pick_team(&mut sess, Some(&team_id), false).await?;
    let ids = apple::app_ids(&mut sess, &team).await?;

    let mut done = 0;
    for (bundle, days) in due {
        let Some(app) = ids.iter().find(|a| a.identifier == bundle) else {
            say(&format!("   {bundle}: no matching App ID on Apple, skipping"));
            continue;
        };
        if dry_run {
            say(&format!("   {bundle}: expires in {days} days, WOULD RENEW (dry run)"));
            continue;
        }
        say(&format!("   {bundle}: expires in {days} days, downloading a new profile..."));
        let new = apple::download_profile(&mut sess, &team, app).await?;
        say(&crate::tr!(format!("      nuovo profilo valido fino al {}", new.date_expire.to_xml_format()), format!("      new profile valid until {}", new.date_expire.to_xml_format())));
        let bytes: Vec<u8> = new.encoded_profile.into();
        link.misagent.install(bytes).await.map_err(dbg)?;
        say("      installed on the phone");
        done += 1;
    }

    if !dry_run && done > 0 {
        say("Final check on the phone:");
        let after = phone_profiles(&mut link).await?;
        for p in after.iter().filter(|p| p.team_id.as_deref() == Some(team_id.as_str())) {
            say(&profile_line(p));
        }
    }
    Ok(())
}

/// Toglie dal telefono il profilo con questo UUID, dopo averne salvato e riletto una copia in
/// `profili-salvati/` (si rimette con `profile-restore`). Serve a capire se iOS accetta i
/// profili nuovi per un'app installata con quello originale.
async fn cmd_profile_remove(addr: Option<SocketAddr>, uuid: Option<String>) -> Result<(), String> {
    let uuid = uuid.ok_or(crate::tr!("uso: altkeeper profile-remove <uuid>", "usage: altkeeper profile-remove <uuid>"))?;
    let mut link = open_phone(addr, &mut |s| println!("{s}")).await?;
    let raw = link.misagent.copy_all().await.map_err(dbg)?;
    let (found, parsed) = raw
        .iter()
        .find_map(|r| {
            profile::parse(r)
                .filter(|p| p.uuid.eq_ignore_ascii_case(&uuid))
                .map(|p| (r, p))
        })
        .ok_or(crate::tr!("profilo non trovato sul telefono", "profile not found on the phone"))?;

    std::fs::create_dir_all(SAVED_DIR).map_err(dbg)?;
    fsperm::restrict_dir(Path::new(SAVED_DIR)).map_err(dbg)?;
    let path = Path::new(SAVED_DIR).join(format!("{}.mobileprovision", parsed.uuid));
    fsperm::write_private(&path, found).map_err(dbg)?;
    if std::fs::read(&path).map_err(dbg)? != *found {
        return Err(crate::tr!(format!("la copia in {} non coincide: non tolgo niente", path.display()), format!("saved copy at {} does not match; refusing to remove the profile", path.display())));
    }
    println!("{}", crate::tr!(format!("Copia salvata in {}", path.display()), format!("Copy saved to {}", path.display())));

    link.misagent.remove(&parsed.uuid).await.map_err(dbg)?;
    println!("{}", crate::tr!(format!("Tolto dal telefono: {}  [{}]", parsed.name, parsed.uuid), format!("Removed from the phone: {}  [{}]", parsed.name, parsed.uuid)));
    Ok(())
}

/// Rimette sul telefono un profilo salvato da `profile-remove`.
async fn cmd_profile_restore(addr: Option<SocketAddr>, file: Option<String>) -> Result<(), String> {
    let file = file.ok_or(crate::tr!("uso: altkeeper profile-restore <file>", "usage: altkeeper profile-restore <file>"))?;
    let bytes = std::fs::read(&file).map_err(dbg)?;
    let parsed = profile::parse(&bytes).ok_or(crate::tr!("il file non è un profilo valido", "the file is not a valid provisioning profile"))?;
    let mut link = open_phone(addr, &mut |s| println!("{s}")).await?;
    link.misagent.install(bytes).await.map_err(dbg)?;
    println!("{}", crate::tr!(format!("Rimesso sul telefono: {}  [{}]", parsed.name, parsed.uuid), format!("Restored on the phone: {}  [{}]", parsed.name, parsed.uuid)));
    Ok(())
}

/// Aspetta la richiesta di chiusura del sistema: SIGTERM su Unix (`docker stop`, systemd),
/// chiusura della finestra o spegnimento su Windows. Ctrl-C è gestito a parte.
#[cfg(unix)]
async fn terminated() {
    use tokio::signal::unix::{signal, SignalKind};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            term.recv().await;
        }
        Err(_) => std::future::pending::<()>().await,
    }
}

#[cfg(windows)]
async fn terminated() {
    use tokio::signal::windows::{ctrl_close, ctrl_shutdown};
    let (Ok(mut close), Ok(mut shutdown)) = (ctrl_close(), ctrl_shutdown()) else {
        return std::future::pending::<()>().await;
    };
    tokio::select! {
        _ = close.recv() => {}
        _ = shutdown.recv() => {}
    }
}

#[cfg(not(any(unix, windows)))]
async fn terminated() {
    std::future::pending::<()>().await
}

#[tokio::main]
async fn main() -> Result<(), String> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();
    isideload::init().map_err(dbg)?;
    // RUST_LOG=debug mostra i messaggi delle librerie. Attenzione: a livello debug
    // idevice stampa anche le chiavi del pairing: non usarlo su log condivisi.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return Err(crate::tr!(USAGE, USAGE_EN).into());
    }
    let cmd = args.remove(0);
    let flag = |name: &str| args.iter().any(|a| a == name);
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    // A command-level --help must not try to authenticate or access a device.
    if matches!(cmd.as_str(), "help" | "--help" | "-h") || flag("--help") || flag("-h") {
        println!("{}", crate::tr!(USAGE, USAGE_EN));
        return Ok(());
    }

    match cmd.as_str() {
        "version" | "--version" | "-V" => {
            println!("altkeeper {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "phone" => cmd_phone(phone_addr(opt("--phone"))?).await,
        "pair-usb" => {
            let out = args.first().cloned().unwrap_or_else(|| PAIRING_FILE.into());
            phone::pair_usb(&out).await?;
            println!("{}", crate::tr!(format!("pairing remoto creato e salvato in {out}"), format!("Remote pairing created and saved to {out}")));
            Ok(())
        }
        "login" => cmd_login(args.first().cloned()).await,
        "apps" => cmd_apps().await,
        "renew" => {
            let min_days = opt("--min-days")
                .map(|v| v.parse::<i64>().map_err(|e| crate::tr!(format!("--min-days non valido: {e}"), format!("invalid --min-days: {e}"))))
                .transpose()?
                .unwrap_or(3);
            cmd_renew(
                phone_addr(opt("--phone"))?,
                flag("--dry-run"),
                flag("--force"),
                min_days,
                &mut |s| println!("{s}"),
            )
            .await
        }
        "serve" => {
            let bind: SocketAddr = opt("--bind")
                .unwrap_or_else(|| "127.0.0.1:8787".into())
                .parse()
                .map_err(|e| crate::tr!(format!("--bind non valido (serve ip:porta): {e}"), format!("invalid --bind (expected ip:port): {e}")))?;
            // Meglio la variabile d'ambiente: un PIN sulla riga di comando si vede con `ps`.
            let pin = opt("--pin")
                .or_else(|| env::var("WEB_PIN"))
                .filter(|p| !p.is_empty());
            if let Some(dir) = opt("--dir") {
                std::env::set_current_dir(&dir).map_err(|e| crate::tr!(format!("--dir {dir}: {e}"), format!("--dir {dir}: {e}")))?;
            }
            let phone = phone_addr(opt("--phone"))?;
            web::check_options(bind, &pin)?;
            apple::anisette_url(env::var("ANISETTE_URL").as_deref())?;
            if flag("--altserver") {
                let port = altserver::start(altserver_port(opt("--altserver-port"))?, phone)?;
                start_dnssd(&opt, port)?;
            } else if opt("--dns-domain").is_some() {
                return Err(crate::tr!("--dns-domain serve solo insieme a --altserver", "--dns-domain requires --altserver").into());
            }
            web::serve(bind, pin, phone).await
        }
        "altserver" => {
            if let Some(dir) = opt("--dir") {
                std::env::set_current_dir(&dir).map_err(|e| crate::tr!(format!("--dir {dir}: {e}"), format!("--dir {dir}: {e}")))?;
            }
            apple::anisette_url(env::var("ANISETTE_URL").as_deref())?;
            let port = altserver::start(altserver_port(opt("--port"))?, phone_addr(opt("--phone"))?)?;
            start_dnssd(&opt, port)?;
            println!("{}", crate::tr!("AltServer attivo. Ctrl-C per fermarlo.", "AltServer running. Press Ctrl-C to stop."));
            tokio::select! {
                r = tokio::signal::ctrl_c() => r.map_err(dbg)?,
                _ = terminated() => {}
            }
            altserver::stop();
            Ok(())
        }
        "profile-remove" => {
            cmd_profile_remove(phone_addr(opt("--phone"))?, args.first().cloned()).await
        }
        "profile-restore" => {
            cmd_profile_restore(phone_addr(opt("--phone"))?, args.first().cloned()).await
        }
        _ => Err(crate::tr!(USAGE, USAGE_EN).into()),
    }
}

#[cfg(test)]
mod cli_tests {
    use super::save_password_confirmed;

    #[test]
    fn confirmation_accepts_italian_and_english_yes() {
        for yes in ["s", "S", "si", "SÌ", "y", "Y", "yes", "YES", " yes\r\n"] {
            assert!(save_password_confirmed(yes), "{yes:?}");
        }
    }

    #[test]
    fn confirmation_never_saves_on_negative_or_missing_input() {
        for no in ["", "\r\n", " ", "n", "N", "no", "NO", "unknown"] {
            assert!(!save_password_confirmed(no), "{no:?}");
        }
    }
}
