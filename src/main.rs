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
mod log;
mod phone;
mod profile;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod update;
mod web;

use std::net::SocketAddr;
use std::path::Path;

use phone::dbg;

const PAIRING_FILE: &str = "rp-pairing.plist";

const USAGE: &str = "usage: altkeeper <phone|pair-usb|login|apps|renew|profile-remove|\
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
                .map_err(|e| format!("invalid phone address: {e}"))
        })
        .transpose()
}

/// Porta su cui ascolta AltServer: `--altserver-port`/`--port`, altrimenti 49500.
fn altserver_port(opt: Option<String>) -> Result<u16, String> {
    opt.map(|v| v.parse::<u16>().map_err(|e| format!("invalid port: {e}")))
        .transpose()
        .map(|p| p.unwrap_or(49500))
}

/// Legge una rete nella forma `10.7.0.0/24`.
fn parse_cidr(s: &str) -> Result<(std::net::Ipv4Addr, u8), String> {
    let (ip, bits) = s
        .split_once('/')
        .ok_or_else(|| format!("--dns-clients {s}: prefix length missing, e.g. 10.7.0.0/24"))?;
    let ip = ip.parse().map_err(|e| format!("--dns-clients: invalid address: {e}"))?;
    let bits: u8 = bits.parse().map_err(|e| format!("--dns-clients: invalid /{bits}: {e}"))?;
    if bits > 32 {
        return Err(format!("--dns-clients: /{bits} does not exist, the maximum is /32"));
    }
    Ok((ip, bits))
}

/// Avvia, se richiesto, il server DNS che pubblica AltServer come record normali (DNS-SD
/// unicast). Serve per farsi trovare da AltStore quando il telefono non è sulla rete di casa:
/// iOS non manda le richieste Bonjour multicast dentro le VPN, ma le query DNS sì.
fn start_dnssd(opt: impl Fn(&str) -> Option<String>, altserver_port: u16) -> Result<(), String> {
    let Some(domain) = opt("--dns-domain") else { return Ok(()) };
    let address: std::net::Ipv4Addr = opt("--dns-address")
        .ok_or(
            "--dns-domain also needs --dns-address <ip>: the address the phone \
             connects to, usually this machine's VPN address",
        )?
        .parse()
        .map_err(|e| format!("invalid --dns-address: {e}"))?;
    let port: u16 = match opt("--dns-port") {
        Some(p) => p.parse().map_err(|e| format!("invalid --dns-port: {e}"))?,
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
        say("Connected via USB");
    } else if addr.map(phone::Via::Wifi) != Some(found) {
        say(&format!("Found iPhone at {found}"));
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
    format!(
        "   {}  [{}]  expires {} ({} days)",
        p.name,
        p.uuid,
        p.expires_text(),
        p.days_left().map(|d| d.to_string()).unwrap_or("?".into())
    )
}

fn print_profile(p: &profile::PhoneProfile) {
    println!("{}", profile_line(p));
}

async fn cmd_phone(addr: Option<SocketAddr>) -> Result<(), String> {
    let mut link = open_phone(addr, &mut |s| println!("{s}")).await?;
    let profiles = phone_profiles(&mut link).await?;
    println!("{} profiles installed on the phone:", profiles.len());
    profiles.iter().for_each(print_profile);
    Ok(())
}

async fn cmd_login(apple_id: Option<String>) -> Result<(), String> {
    let apple_id = apple_id.ok_or("usage: altkeeper login <apple-id>")?;
    let password = rpassword::prompt_password(format!("Password for {apple_id}: "))
        .map_err(dbg)?;
    let password = password.trim().to_string();
    if password.is_empty() {
        return Err("empty password".into());
    }

    apple::set_interactive(true);
    let mut sess = apple::open_session(Path::new("."), &apple_id, &password).await?;
    let team = apple::pick_team(&mut sess, None, true).await?;
    println!(
        "Signed in. Team: {} ({})",
        team.name.as_deref().unwrap_or("<no name>"),
        team.team_id
    );

    print!(
        "Save the password on this server for automatic renewals (file readable only by its owner)? [y/N] "
    );
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).ok();
    let save = matches!(answer.trim().to_lowercase().as_str(), "s" | "si" | "sì" | "y");

    apple::save_account(
        Path::new("."),
        &apple::Account {
            apple_id,
            password: if save { password } else { String::new() },
            team_id: Some(team.team_id),
        },
    )?;
    println!("Account saved in account.json (permissions 600).");
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
        println!("   {}  ({})  expires {}", a.identifier, a.name, exp);
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
        say(&format!("      New profile valid until {}", new.date_expire.to_xml_format()));
        let bytes: Vec<u8> = new.encoded_profile.into();
        phone::with_timeout(
            "Profile install",
            std::time::Duration::from_secs(60),
            async { link.misagent.install(bytes).await.map_err(dbg) },
        )
        .await?;
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
    let uuid = uuid.ok_or("usage: altkeeper profile-remove <uuid>")?;
    let mut link = open_phone(addr, &mut |s| println!("{s}")).await?;
    let raw = link.misagent.copy_all().await.map_err(dbg)?;
    let (found, parsed) = raw
        .iter()
        .find_map(|r| {
            profile::parse(r)
                .filter(|p| p.uuid.eq_ignore_ascii_case(&uuid))
                .map(|p| (r, p))
        })
        .ok_or("profile not found on the phone")?;

    std::fs::create_dir_all(SAVED_DIR).map_err(dbg)?;
    fsperm::restrict_dir(Path::new(SAVED_DIR)).map_err(dbg)?;
    let path = Path::new(SAVED_DIR).join(format!("{}.mobileprovision", parsed.uuid));
    fsperm::write_private(&path, found).map_err(dbg)?;
    if std::fs::read(&path).map_err(dbg)? != *found {
        return Err(format!("the copy in {} does not match: removing nothing", path.display()));
    }
    println!("Copy saved in {}", path.display());

    link.misagent.remove(&parsed.uuid).await.map_err(dbg)?;
    println!("Removed from the phone: {}  [{}]", parsed.name, parsed.uuid);
    Ok(())
}

/// Rimette sul telefono un profilo salvato da `profile-remove`.
async fn cmd_profile_restore(addr: Option<SocketAddr>, file: Option<String>) -> Result<(), String> {
    let file = file.ok_or("usage: altkeeper profile-restore <file>")?;
    let bytes = std::fs::read(&file).map_err(dbg)?;
    let parsed = profile::parse(&bytes).ok_or("the file is not a valid profile")?;
    let mut link = open_phone(addr, &mut |s| println!("{s}")).await?;
    link.misagent.install(bytes).await.map_err(dbg)?;
    println!("Restored on the phone: {}  [{}]", parsed.name, parsed.uuid);
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
    // Su Windows un doppio clic sull'exe (nessun argomento) apre la versione con l'icona.
    if args.is_empty() && cfg!(windows) {
        args.push("tray".into());
    }
    if args.is_empty() {
        return Err(USAGE.into());
    }
    let cmd = args.remove(0);
    let flag = |name: &str| args.iter().any(|a| a == name);
    let opt = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    match cmd.as_str() {
        "version" | "--version" | "-V" => {
            println!("altkeeper {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "phone" => cmd_phone(phone_addr(opt("--phone"))?).await,
        "pair-usb" => {
            let out = args.first().cloned().unwrap_or_else(|| PAIRING_FILE.into());
            phone::pair_usb(&out).await?;
            println!("remote pairing created and saved in {out}");
            Ok(())
        }
        "login" => cmd_login(args.first().cloned()).await,
        "apps" => cmd_apps().await,
        "renew" => {
            let min_days = opt("--min-days")
                .map(|v| v.parse::<i64>().map_err(|e| format!("invalid --min-days: {e}")))
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
                .map_err(|e| format!("invalid --bind (expected ip:port): {e}"))?;
            // Meglio la variabile d'ambiente: un PIN sulla riga di comando si vede con `ps`.
            let pin = opt("--pin")
                .or_else(|| env::var("WEB_PIN"))
                .filter(|p| !p.is_empty());
            if let Some(dir) = opt("--dir") {
                std::env::set_current_dir(&dir).map_err(|e| format!("--dir {dir}: {e}"))?;
            }
            log::init(Path::new("."));
            let phone = phone_addr(opt("--phone"))?;
            web::check_options(bind, &pin)?;
            apple::anisette_url(env::var("ANISETTE_URL").as_deref())?;
            if flag("--altserver") {
                let port = altserver::start(altserver_port(opt("--altserver-port"))?, phone)?;
                start_dnssd(&opt, port)?;
            } else if opt("--dns-domain").is_some() {
                return Err("--dns-domain only works together with --altserver".into());
            }
            web::serve(bind, pin, phone).await
        }
        #[cfg(windows)]
        "tray" => {
            // Senza --dir si usa la cartella dell'exe: all'avvio con Windows la cartella di
            // lavoro è un'altra.
            let dir = match opt("--dir") {
                Some(d) => std::path::PathBuf::from(d),
                None => std::env::current_exe()
                    .map_err(dbg)?
                    .parent()
                    .ok_or("cannot find the exe folder")?
                    .to_path_buf(),
            };
            std::env::set_current_dir(&dir).map_err(|e| format!("--dir {}: {e}", dir.display()))?;
            let dir = std::env::current_dir().map_err(dbg)?;
            log::init(&dir);
            if !flag("--console") {
                tray::hide_console();
            }
            // Avviato da un aggiornamento: prima si aspetta che la versione vecchia liberi le porte.
            let replacing = opt("--replacing").and_then(|p| p.parse::<u32>().ok());
            if let Some(pid) = replacing {
                tray::wait_for_exit(pid);
            }
            let exe = std::env::current_exe().map_err(dbg)?;
            let bind: SocketAddr = "127.0.0.1:8787".parse().map_err(dbg)?;
            let phone = phone_addr(opt("--phone"))?;
            let start = || -> Result<(), String> {
                apple::anisette_url(env::var("ANISETTE_URL").as_deref())?;
                let port = altserver::start(altserver_port(opt("--altserver-port"))?, phone)?;
                start_dnssd(&opt, port)
            };
            if let Err(e) = start() {
                log::err(&e);
                // La versione nuova non parte: si rimette quella vecchia e si riavvia quella.
                if replacing.is_some() && update::rollback(&exe) {
                    log::err("[update] the new version did not start, going back to the old one");
                    let _ = std::process::Command::new(&exe).arg("tray").arg("--dir").arg(&dir).spawn();
                    return Err(e);
                }
                tray::message_box(&format!("AltKeeperPlus cannot start:\n\n{e}"));
                return Err(e);
            }
            update::cleanup(&exe);
            let rt = tokio::runtime::Handle::current();
            tokio::task::block_in_place(|| {
                tray::run(bind, &dir, |proxy| {
                    rt.spawn(async move {
                        if let Err(e) = web::serve(bind, None, phone).await {
                            let _ = proxy.send_event(tray::UserEvent::ServerFailed(e));
                        }
                    });
                })
            })
        }
        "altserver" => {
            if let Some(dir) = opt("--dir") {
                std::env::set_current_dir(&dir).map_err(|e| format!("--dir {dir}: {e}"))?;
            }
            log::init(Path::new("."));
            apple::anisette_url(env::var("ANISETTE_URL").as_deref())?;
            let port = altserver::start(altserver_port(opt("--port"))?, phone_addr(opt("--phone"))?)?;
            start_dnssd(&opt, port)?;
            log::line("AltServer running. Ctrl-C to stop it.");
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
        _ => Err(USAGE.into()),
    }
}
