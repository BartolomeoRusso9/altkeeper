//! Collegamento all'iPhone, via USB se è collegato a questa macchina, altrimenti in Wi-Fi: pairing remoto (RPPairing), tunnel
//! TLS-PSK, RSD e client misagent per i profili di provisioning.

use std::any::Any;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::time::{Duration, Instant};

use idevice::{
    IdeviceService, ReadWrite, RsdService,
    core_device_proxy::CoreDeviceProxy,
    installation_proxy::InstallationProxyClient,
    misagent::MisagentClient,
    provider::{IdeviceProvider, UsbmuxdProvider},
    remote_pairing::{
        RemotePairingClient, RemotePairingLockdownService, RpPairingFile, RpPairingSocket,
        tunnel::connect_tls_psk_tunnel_native,
    },
    rsd::RsdHandshake,
    services::lockdown::LockdownClient,
    tcp::adapter::Adapter,
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection},
};
use tokio::net::TcpStream;

pub const HOST_NAME: &str = "casa2";

pub fn dbg<E: std::fmt::Debug>(e: E) -> String {
    format!("{e:?}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransportKind {
    Legacy,
    Rsd,
}

fn major_ios_version(version: &str) -> Option<u32> {
    version.split('.').next()?.parse().ok()
}

fn select_transport(version: &str, force_legacy: bool) -> Result<TransportKind, String> {
    let major = major_ios_version(version)
        .ok_or_else(|| format!("Versione iOS non riconosciuta: {version}"))?;
    Ok(if force_legacy || major < 17 {
        TransportKind::Legacy
    } else {
        TransportKind::Rsd
    })
}

/// Trasporto da usare quando la versione di iOS non è stata determinata: RSD, il percorso che
/// esisteva prima della rilevazione, a meno che `ALTKEEPER_LEGACY` non imponga quello classico.
fn fallback_transport(force_legacy: bool) -> TransportKind {
    if force_legacy {
        TransportKind::Legacy
    } else {
        TransportKind::Rsd
    }
}

/// Il Wi-Fi passa dal pairing remoto (RPPairing), che esiste solo da iOS 17: chi ha un iPhone
/// più vecchio deve saperlo, invece di leggere un errore di pairing generico.
const WIFI_IOS17_HINT: &str = "Nota: il Wi-Fi funziona solo con iOS 17 o successivo; \
     iOS 12 (e 13-16) è supportato solo via cavo USB";

fn with_wifi_hint(msg: impl std::fmt::Display) -> String {
    format!("{msg} ({WIFI_IOS17_HINT})")
}

/// Il client misagent, più il tunnel (`adapter`) e la stretta di mano RSD (`hs`) con cui aprire
/// gli altri servizi del telefono (installazione e rimozione di app), più gli oggetti che devono
/// restare vivi finché serve.
pub enum PhoneLink {
    /// Connessione moderna basata su RSD.
    Rsd {
        misagent: MisagentClient,
        adapter: idevice::tcp::handle::AdapterHandle,
        hs: RsdHandshake,
        /// Provider USB conservato per i servizi classici.
        usb: Option<UsbmuxdProvider>,
        _keep: Vec<Box<dyn Any>>,
    },
    /// Connessione classica via lockdown/usbmuxd, senza RSD.
    Legacy {
        misagent: MisagentClient,
        provider: UsbmuxdProvider,
        _keep: Vec<Box<dyn Any>>,
    },
}

async fn lockdown_string(lockdown: &mut LockdownClient, key: &str) -> Result<String, String> {
    let value = lockdown.get_value(Some(key), None).await.map_err(dbg)?;

    value
        .as_string()
        .map(str::to_owned)
        .ok_or_else(|| format!("lockdown non ha restituito una stringa per {key}"))
}

async fn identity_via_provider(
    provider: &UsbmuxdProvider,
) -> Result<(String, String, String), String> {
    let pairing = provider.get_pairing_file().await.map_err(dbg)?;
    let mut lockdown = LockdownClient::connect(provider).await.map_err(dbg)?;

    lockdown.start_session(&pairing).await.map_err(dbg)?;

    let name = lockdown_string(&mut lockdown, "DeviceName").await?;
    let udid = lockdown_string(&mut lockdown, "UniqueDeviceID").await?;
    let version = lockdown_string(&mut lockdown, "ProductVersion").await?;

    Ok((name, udid, version))
}

async fn identity_via_rsd(
    adapter: &mut idevice::tcp::handle::AdapterHandle,
    hs: &mut RsdHandshake,
) -> Result<(String, String, String), String> {
    let mut lockdown = LockdownClient::connect_rsd(adapter, hs)
        .await
        .map_err(dbg)?;

    let name = lockdown_string(&mut lockdown, "DeviceName").await?;
    let udid = lockdown_string(&mut lockdown, "UniqueDeviceID").await?;
    let version = lockdown_string(&mut lockdown, "ProductVersion").await?;

    Ok((name, udid, version))
}

impl PhoneLink {
    pub async fn profiles(&mut self) -> Result<Vec<Vec<u8>>, String> {
        let misagent = match self {
            Self::Rsd { misagent, .. } | Self::Legacy { misagent, .. } => misagent,
        };

        misagent.copy_all().await.map_err(dbg)
    }

    pub async fn install_profile(&mut self, profile: Vec<u8>) -> Result<(), String> {
        let misagent = match self {
            Self::Rsd { misagent, .. } | Self::Legacy { misagent, .. } => misagent,
        };

        misagent.install(profile).await.map_err(dbg)
    }

    pub async fn remove_profile(&mut self, uuid: &str) -> Result<(), String> {
        let misagent = match self {
            Self::Rsd { misagent, .. } | Self::Legacy { misagent, .. } => misagent,
        };

        misagent.remove(uuid).await.map_err(dbg)
    }

    pub async fn device_identity(&mut self) -> Result<(String, String, String), String> {
        const USB_LOCKDOWN_TIMEOUT: Duration = Duration::from_secs(10);

        match self {
            Self::Legacy { provider, .. } => {
                with_timeout(
                    "Lettura dell'identità via lockdown",
                    USB_LOCKDOWN_TIMEOUT,
                    identity_via_provider(provider),
                )
                .await
            }
            // Su USB si prova prima il lockdown classico (start_session); se non riesce resta il
            // tunnel RSD, che su iOS 17+ era l'unica strada prima di questa modifica.
            Self::Rsd {
                adapter,
                hs,
                usb: Some(provider),
                ..
            } => match with_timeout(
                "Lettura dell'identità via lockdown",
                USB_LOCKDOWN_TIMEOUT,
                identity_via_provider(provider),
            )
            .await
            {
                Ok(identity) => Ok(identity),
                Err(usb_err) => identity_via_rsd(adapter, hs)
                    .await
                    .map_err(|rsd_err| format!("{rsd_err} (lockdown via USB: {usb_err})")),
            },
            Self::Rsd {
                adapter,
                hs,
                usb: None,
                ..
            } => identity_via_rsd(adapter, hs).await,
        }
    }

    pub async fn install_app(
        &mut self,
        app_path: &Path,
        progress: &(impl Fn(u64) + Send + Sync),
    ) -> Result<(), String> {
        match self {
            Self::Legacy { provider, .. }
            | Self::Rsd {
                usb: Some(provider),
                ..
            } => isideload::sideload::install::install_app(provider, app_path, progress)
                .await
                .map_err(|e| format!("Installation failed: {e}")),
            Self::Rsd {
                adapter,
                hs,
                usb: None,
                ..
            } => isideload::sideload::install::install_app_rsd(adapter, hs, app_path, progress)
                .await
                .map_err(|e| format!("Installation failed: {e}")),
        }
    }

    pub async fn uninstall_app(&mut self, bundle_id: &str) -> Result<(), String> {
        match self {
            Self::Legacy { provider, .. }
            | Self::Rsd {
                usb: Some(provider),
                ..
            } => {
                let mut client = InstallationProxyClient::connect(provider)
                    .await
                    .map_err(dbg)?;

                client.uninstall(bundle_id, None).await.map_err(dbg)
            }
            Self::Rsd {
                adapter,
                hs,
                usb: None,
                ..
            } => {
                let mut client = InstallationProxyClient::connect_rsd(adapter, hs)
                    .await
                    .map_err(dbg)?;

                client.uninstall(bundle_id, None).await.map_err(dbg)
            }
        }
    }

    pub async fn install_ipa_bytes_with_callback<Fut>(
        &mut self,
        ipa: Vec<u8>,
        callback: impl Fn((u64, ())) -> Fut,
    ) -> Result<(), String>
    where
        Fut: std::future::Future<Output = ()>,
    {
        match self {
            Self::Legacy { provider, .. }
            | Self::Rsd {
                usb: Some(provider),
                ..
            } => idevice::utils::installation::install_bytes_with_callback(
                provider,
                ipa,
                None,
                callback,
                (),
            )
            .await
            .map_err(dbg),
            Self::Rsd {
                adapter,
                hs,
                usb: None,
                ..
            } => idevice::utils::installation::install_bytes_with_callback_rsd(
                adapter,
                hs,
                ipa,
                None,
                callback,
                (),
            )
            .await
            .map_err(dbg),
        }
    }
}

/// Come si è arrivati al telefono: cavo USB (usbmuxd locale) o rete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Usb,
    Wifi(SocketAddr),
}

impl std::fmt::Display for Via {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Via::Usb => write!(f, "USB"),
            Via::Wifi(addr) => write!(f, "{addr}"),
        }
    }
}

/// Aspetta il PIN mostrato sull'iPhone in `pin.txt` (max 2 minuti).
async fn ask_pin() -> String {
    println!("{}", crate::tr!("   l'iPhone chiede un PIN: scrivilo in pin.txt (attendo 2 minuti)", "   iPhone asks for a PIN: write it to pin.txt (waiting up to 2 minutes)"));
    for _ in 0..120 {
        if let Ok(s) = tokio::fs::read_to_string("pin.txt").await {
            let pin = s.trim().to_string();
            if !pin.is_empty() {
                let _ = tokio::fs::remove_file("pin.txt").await;
                return pin;
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    String::new()
}

/// Si collega al telefono via Wi-Fi con un pairing remoto già esistente.
pub async fn connect(addr: SocketAddr, pairing_path: &str) -> Result<PhoneLink, String> {
    if !Path::new(pairing_path).exists() {
        return Err(crate::tr!(
            format!("manca il pairing remoto {pairing_path}: crealo con `altkeeper pair-usb`"),
            format!("remote pairing file {pairing_path} is missing; create it with `altkeeper pair-usb`")
        ));
    }
    let mut pf = RpPairingFile::read_from_file(pairing_path)
        .await
        .map_err(dbg)?;

    let tcp = TcpStream::connect(addr).await.map_err(dbg)?;
    let mut client = RemotePairingClient::new(RpPairingSocket::new(tcp), HOST_NAME);
    client
        .connect(&mut pf, || async { ask_pin().await })
        .await
        .map_err(|e| {
            crate::tr!(
                format!("pairing con l'iPhone non riuscito ({e:?}). Se il telefono non riconosce questo pairing, rifallo con `altkeeper pair-usb`"),
                format!("iPhone pairing failed ({e:?}). If the phone does not recognize the pairing, repeat `altkeeper pair-usb`")
            )
        })?;

    let port = client.create_tcp_listener().await.map_err(dbg)?;
    let key = client.encryption_key().to_vec();

    let stream = TcpStream::connect(SocketAddr::new(addr.ip(), port))
        .await
        .map_err(dbg)?;
    let tunnel = connect_tls_psk_tunnel_native(stream, &key)
        .await
        .map_err(dbg)?;
    let info = tunnel.info.clone();

    let our_ip: IpAddr = info.client_address.parse().map_err(dbg)?;
    let their_ip: IpAddr = info.server_address.parse().map_err(dbg)?;
    let inner: Box<dyn ReadWrite> = Box::new(tunnel.into_inner());
    let mut adapter = Adapter::new(Box::new(inner), our_ip, their_ip);
    adapter.set_mss((info.mtu as usize).saturating_sub(60));
    let mut adapter = adapter.to_async_handle();

    let rsd_stream = adapter.connect(info.server_rsd_port).await.map_err(dbg)?;
    let mut hs = RsdHandshake::new(rsd_stream).await.map_err(dbg)?;
    let misagent = MisagentClient::connect_rsd(&mut adapter, &mut hs)
        .await
        .map_err(dbg)?;

    Ok(PhoneLink::Rsd {
        misagent,
        adapter,
        hs,
        usb: None,
        _keep: vec![Box::new(client)],
    })
}

/// Si collega via USB, se un iPhone è collegato alla macchina su cui gira AltKeeper (o a un
/// usbmuxd raggiungibile con `USBMUXD_SOCKET_ADDRESS`). Usa il tunnel CoreDeviceProxy: serve
/// solo che il telefono si fidi già del computer ("Registra"), non il pairing remoto.
/// `Ok(None)` = nessun telefono su USB (o usbmuxd non c'è): si prosegue in Wi-Fi.
pub async fn connect_usb(say: &mut impl FnMut(&str)) -> Result<Option<PhoneLink>, String> {
    if crate::env::is_set("NO_USB") {
        return Ok(None);
    }
    let addr = UsbmuxdAddr::from_env_var().map_err(dbg)?;
    let Ok(Ok(mut mux)) = tokio::time::timeout(Duration::from_secs(2), addr.connect(1)).await
    else {
        return Ok(None);
    };
    let Ok(Ok(devs)) = tokio::time::timeout(Duration::from_secs(3), mux.get_devices()).await
    else {
        return Ok(None);
    };
    let Some(dev) = devs.iter().find(|d| d.connection_type == Connection::Usb) else {
        return Ok(None);
    };
    say(crate::tr!("iPhone trovato via USB, mi collego col cavo...", "iPhone found via USB, connecting over cable..."));
    let provider = dev.to_provider(addr, "altkeeper");

    // Rileva iOS prima di provare CoreDeviceProxy/RSD. Il probe dedicato verifica anche
    // start_session e Misagent; questa fase cambia solo il rilevamento della versione.
    let version = tokio::time::timeout(Duration::from_secs(10), async {
        let mut lockdown = LockdownClient::connect(&provider).await.map_err(dbg)?;
        let value = lockdown
            .get_value(Some("ProductVersion"), None)
            .await
            .map_err(dbg)?;
        value
            .as_string()
            .map(str::to_owned)
            .ok_or_else(|| "ProductVersion non restituita come stringa".to_string())
    })
    .await
    .map_err(|_| "timeout leggendo ProductVersion via lockdown".to_string())
    .and_then(|read| read);

    // Se la versione non si legge (o non si capisce) non si rinuncia al cavo: si prosegue sul
    // percorso RSD, come prima che esistesse la rilevazione, salvo ALTKEEPER_LEGACY.
    let force_legacy = crate::env::is_set("LEGACY");
    let detected = version.and_then(|v| select_transport(&v, force_legacy).map(|t| (v, t)));
    let transport = match detected {
        Ok((version, transport)) => {
            say(&format!(
                "iOS rilevato: {version} (trasporto {transport:?})"
            ));
            transport
        }
        Err(e) => {
            let transport = fallback_transport(force_legacy);
            say(&format!(
                "Versione iOS non determinata ({e}): provo il trasporto {transport:?}"
            ));
            transport
        }
    };
    if transport == TransportKind::Legacy {
        let legacy_connect = async {
            let pairing = provider.get_pairing_file().await.map_err(dbg)?;

            let mut lockdown = LockdownClient::connect(&provider).await.map_err(dbg)?;

            lockdown.start_session(&pairing).await.map_err(dbg)?;

            drop(lockdown);

            MisagentClient::connect(&provider).await.map_err(dbg)
        };

        let misagent = tokio::time::timeout(Duration::from_secs(15), legacy_connect)
            .await
            .map_err(|_| "timeout collegando lockdown/Misagent via USB".to_string())??;

        return Ok(Some(PhoneLink::Legacy {
            misagent,
            provider,
            _keep: Vec::new(),
        }));
    }

    let open = async {
        let proxy = CoreDeviceProxy::connect(&provider).await.map_err(dbg)?;
        let rsd_port = proxy.tunnel_info().server_rsd_port;
        let adapter = proxy.create_software_tunnel().map_err(dbg)?;
        let mut adapter = adapter.to_async_handle();
        let rsd_stream = adapter.connect(rsd_port).await.map_err(dbg)?;
        let mut hs = RsdHandshake::new(rsd_stream).await.map_err(dbg)?;
        // misagent: prima il servizio lockdown via usbmuxd (come pymobiledevice3), che su USB è
        // il più affidabile; se non parte, ripiego sul tunnel RSD.
        let misagent = match MisagentClient::connect(&provider).await {
            Ok(m) => m,
            Err(_) => MisagentClient::connect_rsd(&mut adapter, &mut hs)
                .await
                .map_err(dbg)?,
        };
        Ok::<_, String>(PhoneLink::Rsd {
            misagent,
            adapter,
            hs,
            usb: None,
            _keep: Vec::new(),
        })
    };
    match tokio::time::timeout(Duration::from_secs(15), open).await {
        Ok(Ok(mut link)) => {
            if let PhoneLink::Rsd { usb, .. } = &mut link {
                *usb = Some(provider);
            }
            Ok(Some(link))
        }
        Ok(Err(e)) => Err(e),
        Err(_) => Err(crate::tr!("il telefono non risponde via USB", "the phone is not responding over USB").into()),
    }
}

/// Esegue `fut` con un tempo massimo: un telefono che smette di rispondere non deve bloccare
/// il processo per sempre, deve dare un errore leggibile.
pub async fn with_timeout<T>(
    what: &str,
    limit: Duration,
    fut: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    match tokio::time::timeout(limit, fut).await {
        Ok(r) => r,
        Err(_) => Err(format!(
            "{what}: nessuna risposta dal telefono dopo {} s (operazione interrotta)",
            limit.as_secs()
        )),
    }
}

/// L'unico demone mDNS del processo: ricerca dell'iPhone e annuncio di AltServer lo condividono
/// (due demoni nello stesso processo si contendono la porta 5353 e le risposte si perdono).
pub fn mdns() -> Option<&'static mdns_sd::ServiceDaemon> {
    static DAEMON: std::sync::OnceLock<Option<mdns_sd::ServiceDaemon>> = std::sync::OnceLock::new();
    DAEMON.get_or_init(|| mdns_sd::ServiceDaemon::new().ok()).as_ref()
}

/// Cerca via Bonjour gli iPhone in rete (servizio `_remotepairing._tcp`): indirizzi IPv4 e porta.
/// L'indirizzo del telefono cambia di tanto in tanto: così non serve saperlo.
pub async fn discover(wait: Duration) -> Vec<SocketAddr> {
    // ALTKEEPER_NO_MDNS=1 spegne Bonjour (per provare la scansione della rete, o se sulla rete il
    // multicast è bloccato).
    if crate::env::is_set("NO_MDNS") {
        return Vec::new();
    }
    tokio::task::spawn_blocking(move || {
        let Some(daemon) = mdns() else {
            return Vec::new();
        };
        let Ok(events) = daemon.browse("_remotepairing._tcp.local.") else {
            return Vec::new();
        };
        let mut deadline = Instant::now() + wait;
        let mut found: Vec<SocketAddr> = Vec::new();
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match events.recv_timeout(left) {
                Ok(mdns_sd::ServiceEvent::ServiceResolved(svc)) => {
                    for ip in svc.get_addresses_v4() {
                        let addr = SocketAddr::new(IpAddr::V4(ip), svc.get_port());
                        if !found.contains(&addr) {
                            found.push(addr);
                        }
                    }
                    // Trovato: si aspetta ancora un attimo per un eventuale secondo telefono.
                    deadline = deadline.min(Instant::now() + Duration::from_millis(700));
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let _ = daemon.stop_browse("_remotepairing._tcp.local.");
        found
    })
    .await
    .unwrap_or_default()
}

/// Gli altri indirizzi della stessa rete /24 (senza il proprio, né rete né broadcast).
fn subnet_hosts(own: std::net::Ipv4Addr) -> Vec<std::net::Ipv4Addr> {
    let [a, b, c, d] = own.octets();
    (1..=254u8).filter(|h| *h != d).map(|h| std::net::Ipv4Addr::new(a, b, c, h)).collect()
}

/// Cerca il telefono senza Bonjour: prova la porta del servizio (di solito 49152) su tutta la
/// rete /24 della macchina, in parallelo. Serve quando le risposte Bonjour non arrivano.
pub async fn scan_lan(port: u16) -> Vec<SocketAddr> {
    // "Collegando" un socket UDP (senza mandare niente) si legge l'indirizzo locale in uso.
    let Ok(sock) = tokio::net::UdpSocket::bind("0.0.0.0:0").await else {
        return Vec::new();
    };
    if sock.connect("192.0.2.1:9").await.is_err() {
        return Vec::new();
    }
    let Ok(IpAddr::V4(own)) = sock.local_addr().map(|a| a.ip()) else {
        return Vec::new();
    };
    let mut set = tokio::task::JoinSet::new();
    for ip in subnet_hosts(own) {
        set.spawn(async move {
            let addr = SocketAddr::new(IpAddr::V4(ip), port);
            match tokio::time::timeout(Duration::from_millis(600), TcpStream::connect(addr)).await {
                Ok(Ok(_)) => Some(addr),
                _ => None,
            }
        });
    }
    let mut found = Vec::new();
    while let Some(r) = set.join_next().await {
        if let Ok(Some(addr)) = r {
            found.push(addr);
        }
    }
    found.sort();
    found
}

/// Ultimo indirizzo con cui ci si è collegati al telefono. Si prova per primo: la ricerca via
/// Bonjour serve solo quando l'IP cambia. Su un server dove altri programmi ascoltano sulla
/// stessa porta mDNS (per esempio Home Assistant) le risposte dell'iPhone possono andare perse,
/// quindi affidarsi solo alla ricerca sarebbe intermittente.
const LAST_ADDR_FILE: &str = "state/phone-addr";

fn read_last_addr(path: &Path) -> Option<SocketAddr> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn write_last_addr(path: &Path, addr: SocketAddr) {
    if read_last_addr(path) == Some(addr) {
        return;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, addr.to_string());
}

/// Si collega al telefono: prima all'indirizzo indicato (se c'è), poi all'ultimo che ha
/// funzionato, poi a quelli trovati via Bonjour (fino a 3 ricerche). Con più iPhone in rete
/// vale quello che riconosce il nostro pairing.
pub async fn connect_auto(
    preferred: Option<SocketAddr>,
    pairing_path: &str,
    mut say: impl FnMut(&str),
) -> Result<(Via, PhoneLink), String> {
    // Un indirizzo indicato a mano vale più del cavo; altrimenti si prova prima l'USB.
    if preferred.is_none() {
        match connect_usb(&mut say).await {
            Ok(Some(link)) => return Ok((Via::Usb, link)),
            Ok(None) => {}
            Err(e) => say(&crate::tr!(format!("USB non riuscita ({e}), provo in Wi-Fi..."), format!("USB connection failed ({e}); trying Wi-Fi..."))),
        }
    }
    if !Path::new(pairing_path).exists() {
        return Err(with_wifi_hint(crate::tr!(
            format!("manca il pairing remoto {pairing_path}: crealo con `altkeeper pair-usb`"),
            format!("remote pairing file {pairing_path} is missing; create it with `altkeeper pair-usb`")
        )));
    }
    const PER_TRY: Duration = Duration::from_secs(12);
    let cache = Path::new(LAST_ADDR_FILE);
    let mut tried: Vec<SocketAddr> = Vec::new();
    let mut last_err = String::new();

    let mut known: Vec<SocketAddr> = Vec::new();
    known.extend(preferred);
    if let Some(last) = read_last_addr(cache) {
        if !known.contains(&last) {
            known.push(last);
        }
    }
    for addr in known {
        tried.push(addr);
        match tokio::time::timeout(PER_TRY, connect(addr, pairing_path)).await {
            Ok(Ok(link)) => {
                write_last_addr(cache, addr);
                return Ok((Via::Wifi(addr), link));
            }
            Ok(Err(e)) => last_err = e,
            Err(_) => last_err = crate::tr!(format!("{addr} non risponde"), format!("{addr} did not respond")),
        }
        say(&crate::tr!(format!("{addr} non risponde, cerco il telefono in rete..."), format!("{addr} did not respond; searching the network...")));
    }
    if tried.is_empty() {
        say(crate::tr!("Cerco il telefono in rete...", "Searching for the phone on the network..."));
    }

    // Bonjour, poi la scansione della rete (non dipende da Bonjour), poi Bonjour ancora una volta.
    let port = read_last_addr(cache).map(|a| a.port()).unwrap_or(49152);
    let mut step = 0;
    while step < 3 {
        let mut found = match step {
            1 => {
                say(crate::tr!("Bonjour non risponde: cerco sulla rete locale...", "Bonjour did not respond; scanning the local network..."));
                scan_lan(port).await
            }
            _ => discover(Duration::from_secs(4)).await,
        };
        found.retain(|a| !tried.contains(a));
        step += 1;
        for addr in found {
            tried.push(addr);
            say(&crate::tr!(format!("Trovato un telefono a {addr}, provo..."), format!("Found an iPhone at {addr}; connecting...")));
            match tokio::time::timeout(PER_TRY, connect(addr, pairing_path)).await {
                Ok(Ok(link)) => {
                    write_last_addr(cache, addr);
                    return Ok((Via::Wifi(addr), link));
                }
                Ok(Err(e)) => last_err = e,
                Err(_) => last_err = crate::tr!(format!("{addr} non risponde"), format!("{addr} did not respond")),
            }
        }
    }

    if tried.is_empty() || last_err.is_empty() {
        Err(with_wifi_hint(crate::tr!(
            "non trovo l'iPhone in rete: è acceso e sul Wi-Fi di casa?",
            "cannot find the iPhone on the network: is it on and connected to your home Wi-Fi?"
        )))
    } else {
        Err(with_wifi_hint(last_err))
    }
}

/// Crea il pairing remoto via USB, senza richieste sul telefono.
/// Serve un usbmuxd raggiungibile via TCP in USBMUXD_SOCKET_ADDRESS
/// (per esempio quello del Mac, inoltrato con un tunnel SSH).
pub async fn pair_usb(out: &str) -> Result<(), String> {
    let var = std::env::var("USBMUXD_SOCKET_ADDRESS")
        .map_err(|_| crate::tr!("manca USBMUXD_SOCKET_ADDRESS (ip:porta di un usbmuxd)", "USBMUXD_SOCKET_ADDRESS is missing (ip:port of usbmuxd)").to_string())?;
    let sock: SocketAddr = var.parse().map_err(dbg)?;
    let stream = TcpStream::connect(sock).await.map_err(dbg)?;
    let mut mux = UsbmuxdConnection::new(Box::new(stream), 1);
    let devs = mux.get_devices().await.map_err(dbg)?;
    let dev = devs
        .iter()
        .find(|d| d.connection_type == Connection::Usb)
        .ok_or(crate::tr!("nessun iPhone collegato via USB", "no iPhone connected via USB"))?;
    let provider = dev.to_provider(UsbmuxdAddr::from_env_var().map_err(dbg)?, "altkeeper");

    let service = RemotePairingLockdownService::connect(&provider)
        .await
        .map_err(dbg)?;
    let mut client = service.into_client(HOST_NAME).map_err(dbg)?;
    let mut pf = RpPairingFile::generate(HOST_NAME);
    // Su un canale USB già fidato non compare nessuna richiesta: il PIN non serve.
    client
        .connect(&mut pf, || async { "000000".to_string() })
        .await
        .map_err(dbg)?;
    pf.write_to_file(out).await.map_err(dbg)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legge_la_versione_ios() {
        assert_eq!(major_ios_version("12.5.8"), Some(12));
        assert_eq!(major_ios_version("16.7.12"), Some(16));
        assert_eq!(major_ios_version("17.0"), Some(17));
        assert_eq!(major_ios_version("27.0.1"), Some(27));
        assert_eq!(major_ios_version("unknown"), None);
        assert_eq!(major_ios_version(""), None);
    }

    #[test]
    fn seleziona_il_trasporto_per_versione() {
        assert_eq!(select_transport("12.5.8", false), Ok(TransportKind::Legacy));
        assert_eq!(
            select_transport("16.7.12", false),
            Ok(TransportKind::Legacy)
        );
        assert_eq!(select_transport("17.0", false), Ok(TransportKind::Rsd));
        assert_eq!(select_transport("27.0.1", false), Ok(TransportKind::Rsd));
        assert_eq!(select_transport("27.0.1", true), Ok(TransportKind::Legacy));
        assert!(select_transport("unknown", false).is_err());
    }

    #[test]
    fn senza_versione_si_prova_rsd_salvo_legacy_forzato() {
        assert_eq!(fallback_transport(false), TransportKind::Rsd);
        assert_eq!(fallback_transport(true), TransportKind::Legacy);
    }

    #[test]
    fn il_wifi_spiega_che_ios_12_vuole_il_cavo() {
        let e = with_wifi_hint("non trovo l'iPhone in rete");
        assert!(e.starts_with("non trovo l'iPhone in rete"));
        assert!(e.contains("iOS 12"));
        assert!(e.contains("USB"));
    }

    fn temp_file(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("altkeeper-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn la_rete_da_scansionare_esclude_se_stessi() {
        let hosts = subnet_hosts("192.168.1.63".parse().unwrap());
        assert_eq!(hosts.len(), 253);
        assert!(!hosts.contains(&"192.168.1.63".parse().unwrap()));
        assert!(hosts.contains(&"192.168.1.1".parse().unwrap()));
        assert!(hosts.contains(&"192.168.1.254".parse().unwrap()));
        assert!(!hosts.contains(&"192.168.1.0".parse().unwrap()));
        assert!(!hosts.contains(&"192.168.1.255".parse().unwrap()));
        assert!(hosts.iter().all(|h| h.octets()[..3] == [192, 168, 1]));
    }

    #[test]
    fn ultimo_indirizzo_scritto_e_riletto() {
        let p = temp_file("addr/phone-addr");
        assert_eq!(read_last_addr(&p), None);
        let a: SocketAddr = "192.168.1.149:49152".parse().unwrap();
        write_last_addr(&p, a);
        assert_eq!(read_last_addr(&p), Some(a));
        let b: SocketAddr = "192.168.1.146:49152".parse().unwrap();
        write_last_addr(&p, b);
        assert_eq!(read_last_addr(&p), Some(b));
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn un_file_con_dentro_altro_non_e_un_indirizzo() {
        let p = temp_file("garbage");
        std::fs::write(&p, "non è un indirizzo\n").unwrap();
        assert_eq!(read_last_addr(&p), None);
        std::fs::write(&p, "  10.0.0.5:49152 \n").unwrap();
        assert_eq!(read_last_addr(&p), Some("10.0.0.5:49152".parse().unwrap()));
        let _ = std::fs::remove_file(&p);
    }
}
