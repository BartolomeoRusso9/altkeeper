//! Pubblica AltServer come record DNS normali (DNS-SD unicast, RFC 6763).
//!
//! Serve quando il telefono non è sulla rete di casa. AltStore cerca il server con Bonjour,
//! che di solito vuol dire multicast, e iOS il multicast dentro le VPN non lo manda mai. Però
//! AltStore cerca nei "domini predefiniti" (`inDomain: ""`), che comprendono anche i domini di
//! ricerca unicast: pubblicando gli stessi dati come record DNS veri, il telefono li trova con
//! query normali, che nella VPN passano senza problemi.
//!
//! Questo è un server DNS minimo: risponde solo per i propri nomi e solo via UDP. Non è un
//! resolver e non inoltra niente. Di solito si mette dietro al DNS che già usi (AdGuard,
//! dnsmasq, Pi-hole) inoltrandogli il solo dominio scelto.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

/// TTL dei record, in secondi: corto, così un cambio di indirizzo si propaga in fretta.
const TTL: u32 = 120;

const TYPE_A: u16 = 1;
const TYPE_PTR: u16 = 12;
const TYPE_TXT: u16 = 16;
const TYPE_SRV: u16 = 33;
const TYPE_ANY: u16 = 255;
const CLASS_IN: u16 = 1;

/// Bandierina "risposta troncata": dice al client di riprovare in TCP.
const FLAG_TRUNCATED: u16 = 0x0200;

const RCODE_OK: u16 = 0;
const RCODE_NXDOMAIN: u16 = 3;

/// Quanto può essere lungo un messaggio DNS su UDP senza EDNS.
const MAX_REPLY: usize = 512;

/// I nomi da pubblicare, calcolati una volta all'avvio.
pub struct Zone {
    /// Il dominio scelto, senza punto finale (per esempio `casa.internal`).
    domain: String,
    /// `_altserver._tcp.<dominio>`: il nome che AltStore interroga per l'elenco dei server.
    browse: String,
    /// `<istanza>._altserver._tcp.<dominio>`: il server vero e proprio.
    instance: String,
    /// Il nome host a cui punta il record SRV.
    host: String,
    server_id: String,
    port: u16,
    address: Ipv4Addr,
    /// La sottorete da cui arrivano i client (di solito quella della VPN), come indirizzo e
    /// numero di bit: serve per la scoperta automatica del dominio.
    clients: (Ipv4Addr, u8),
}

impl Zone {
    /// `instance` è il nome visibile del server, `id` il suo serverID (quello che AltStore usa
    /// per riconoscerlo). Il nome host si ricava dall'id, come nell'annuncio Bonjour.
    pub fn new(
        domain: &str,
        instance: &str,
        id: &str,
        port: u16,
        address: Ipv4Addr,
        clients: (Ipv4Addr, u8),
    ) -> Self {
        let domain = domain.trim_matches('.').to_ascii_lowercase();
        // Ogni nome in arrivo viene messo in minuscolo quando lo leggiamo, quindi anche i nostri
        // devono esserlo: un nome host con le maiuscole farebbe fallire il confronto e il server
        // risponderebbe "non esiste" per il proprio servizio.
        let instance = instance.to_ascii_lowercase();
        // Stesso motivo: l'id di solito è minuscolo, ma può arrivare da un file scritto a
        // mano. Il serverID nel record TXT resta com'è, lì le maiuscole contano.
        let short = id[..id.len().min(8)].to_ascii_lowercase();
        Self {
            browse: format!("_altserver._tcp.{domain}"),
            instance: format!("{instance}._altserver._tcp.{domain}"),
            host: format!("altkeeper-{short}.{domain}"),
            server_id: id.to_string(),
            port,
            address,
            clients,
            domain,
        }
    }

    /// Le risposte per una domanda, o `None` se il nome non è nostro.
    /// `Some(lista vuota)` vuol dire "il nome esiste ma non ha record di quel tipo".
    fn answer(&self, name: &str, qtype: u16) -> Option<Vec<Record>> {
        let want = |t: u16| qtype == t || qtype == TYPE_ANY;

        // I tre nomi con cui un client chiede "quali domini posso esplorare?" (RFC 6763 par. 11).
        //
        // La domanda arriva in due forme. La prima è sul dominio stesso, e serve a chi lo ha già
        // come dominio di ricerca. La seconda, più importante, è sul "rovescio" dell'indirizzo
        // che il client ha sulla rete: iOS la fa da solo, senza che sul telefono sia impostato
        // niente. Rispondendo anche a quella, il server si fa trovare senza configurazione.
        if let Some(rest) = ["b", "db", "lb"].iter().find_map(|p| {
            name.strip_prefix(&format!("{p}._dns-sd._udp."))
        }) {
            if rest == self.domain || self.covers_reverse(rest) {
                return Some(if want(TYPE_PTR) {
                    vec![Record::Ptr { name: name.to_string(), target: self.domain.clone() }]
                } else {
                    vec![]
                });
            }
        }

        if name == self.browse {
            return Some(if want(TYPE_PTR) {
                vec![Record::Ptr { name: name.to_string(), target: self.instance.clone() }]
            } else {
                vec![]
            });
        }

        if name == self.instance {
            let mut out = Vec::new();
            if want(TYPE_SRV) {
                out.push(Record::Srv {
                    name: name.to_string(),
                    port: self.port,
                    target: self.host.clone(),
                });
            }
            if want(TYPE_TXT) {
                out.push(Record::Txt {
                    name: name.to_string(),
                    text: format!("serverID={}", self.server_id),
                });
            }
            return Some(out);
        }

        if name == self.host {
            return Some(if want(TYPE_A) {
                vec![Record::A { name: name.to_string(), address: self.address }]
            } else {
                vec![]
            });
        }

        None
    }

    /// Vero se `name` è un nome inverso (`…in-addr.arpa`) che ricade nella sottorete dei client.
    ///
    /// Accetta sia l'indirizzo intero (`2.0.7.10.in-addr.arpa`, che è come lo chiede iOS) sia la
    /// sola parte di rete (`0.7.10.in-addr.arpa`): in quel caso gli ottetti mancanti valgono zero.
    fn covers_reverse(&self, name: &str) -> bool {
        let Some(rev) = name.strip_suffix(".in-addr.arpa") else { return false };
        let mut octets = [0u8; 4];
        let mut quanti = 0;
        // Il nome inverso ha gli ottetti al contrario: 2.0.7.10 vuol dire 10.7.0.2.
        for (i, part) in rev.split('.').enumerate() {
            if i >= 4 {
                return false;
            }
            let Ok(n) = part.parse::<u8>() else { return false };
            octets[3 - i] = n;
            quanti += 1;
        }
        if quanti == 0 {
            return false;
        }
        // Se mancano ottetti in testa, il nome copre una rete: si allinea a sinistra.
        if quanti < 4 {
            let mancanti = 4 - quanti;
            octets.rotate_left(mancanti);
        }
        let (rete, bits) = self.clients;
        if bits == 0 {
            return true;
        }
        if bits > 32 {
            return false;
        }
        let maschera = u32::MAX << (32 - bits);
        (u32::from(Ipv4Addr::from(octets)) & maschera) == (u32::from(rete) & maschera)
    }
}

enum Record {
    A { name: String, address: Ipv4Addr },
    Ptr { name: String, target: String },
    Txt { name: String, text: String },
    Srv { name: String, port: u16, target: String },
}

// ---------------------------------------------------------------------------------------------
// Avvio
// ---------------------------------------------------------------------------------------------

/// Si mette in ascolto e risponde finché il programma vive. Torna la porta davvero usata.
pub fn start(zone: Zone, bind: SocketAddr) -> Result<u16, String> {
    let socket = UdpSocket::bind(bind)
        .map_err(|e| format!("DNS: non riesco ad ascoltare su {bind}: {e}"))?;
    let port = socket.local_addr().map_err(|e| e.to_string())?.port();

    // Quello che serve nel messaggio va letto ora: subito dopo `zone` passa al thread.
    println!(
        "[dns-sd] porta {port}: pubblico {} -> {}:{}",
        zone.instance, zone.address, zone.port
    );

    std::thread::spawn(move || {
        let mut buf = [0u8; 512];
        loop {
            let Ok((len, from)) = socket.recv_from(&mut buf) else { continue };
            if let Some(reply) = handle(&zone, &buf[..len]) {
                let _ = socket.send_to(&reply, from);
            }
        }
    });

    Ok(port)
}

/// Costruisce la risposta a un messaggio, o `None` se non è una domanda a cui sappiamo reagire.
fn handle(zone: &Zone, query: &[u8]) -> Option<Vec<u8>> {
    let q = parse_query(query)?;
    let answers = zone.answer(&q.name, q.qtype);

    let rcode = if answers.is_none() { RCODE_NXDOMAIN } else { RCODE_OK };
    let records = answers.unwrap_or_default();

    let mut out = build_reply(&q, rcode, &records);
    // Oltre il limite UDP si manda una risposta vuota ma con la bandierina "troncata": senza,
    // il client la leggerebbe come "questi record non esistono" e si fermerebbe, invece di
    // riprovare in TCP.
    if out.len() > MAX_REPLY {
        out = build_reply(&q, RCODE_OK, &[]);
        out[2] |= (FLAG_TRUNCATED >> 8) as u8;
    }
    Some(out)
}

fn build_reply(q: &Query, rcode: u16, records: &[Record]) -> Vec<u8> {
    let mut out = Vec::with_capacity(MAX_REPLY);

    // Intestazione: stesso id della domanda, bandierine "è una risposta, sono autorevole",
    // e l'eventuale "ricorsione richiesta" rimandata indietro com'è.
    out.extend_from_slice(&q.id.to_be_bytes());
    let flags = 0x8400 | (if q.recursion_desired { 0x0100 } else { 0 }) | rcode;
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // domande
    out.extend_from_slice(&(records.len() as u16).to_be_bytes()); // risposte
    out.extend_from_slice(&0u16.to_be_bytes()); // autorità
    out.extend_from_slice(&0u16.to_be_bytes()); // extra

    // La domanda va ricopiata identica.
    write_name(&mut out, &q.name);
    out.extend_from_slice(&q.qtype.to_be_bytes());
    out.extend_from_slice(&q.qclass.to_be_bytes());

    for r in records {
        write_record(&mut out, r);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Lettura e scrittura del formato DNS
// ---------------------------------------------------------------------------------------------

struct Query {
    id: u16,
    recursion_desired: bool,
    name: String,
    qtype: u16,
    qclass: u16,
}

/// Legge l'intestazione e la prima domanda. Scarta tutto ciò che non è una domanda semplice:
/// non ci interessa reggere il caso generale, solo rispondere per i nostri nomi.
fn parse_query(buf: &[u8]) -> Option<Query> {
    if buf.len() < 12 {
        return None;
    }
    let id = u16::from_be_bytes([buf[0], buf[1]]);
    let flags = u16::from_be_bytes([buf[2], buf[3]]);
    // Bit più alto acceso = è già una risposta, non una domanda.
    if flags & 0x8000 != 0 {
        return None;
    }
    // Solo le domande normali (opcode 0).
    if (flags >> 11) & 0xF != 0 {
        return None;
    }
    if u16::from_be_bytes([buf[4], buf[5]]) != 1 {
        return None;
    }

    let (name, next) = read_name(buf, 12)?;
    if next + 4 > buf.len() {
        return None;
    }
    let qtype = u16::from_be_bytes([buf[next], buf[next + 1]]);
    let qclass = u16::from_be_bytes([buf[next + 2], buf[next + 3]]);
    if qclass != CLASS_IN && qclass != TYPE_ANY {
        return None;
    }

    Some(Query { id, recursion_desired: flags & 0x0100 != 0, name, qtype, qclass })
}

/// Legge un nome a etichette e torna anche dove finisce. I puntatori di compressione non sono
/// ammessi: nelle domande non servono, e accettarli aprirebbe la porta ai cicli infiniti.
fn read_name(buf: &[u8], mut at: usize) -> Option<(String, usize)> {
    let mut parts: Vec<String> = Vec::new();
    loop {
        let len = *buf.get(at)? as usize;
        at += 1;
        if len == 0 {
            break;
        }
        if len >= 0xC0 || len > 63 {
            return None;
        }
        let end = at + len;
        let label = buf.get(at..end)?;
        parts.push(String::from_utf8_lossy(label).to_ascii_lowercase());
        at = end;
        if parts.len() > 32 {
            return None;
        }
    }
    Some((parts.join("."), at))
}

fn write_name(out: &mut Vec<u8>, name: &str) {
    for label in name.split('.').filter(|l| !l.is_empty()) {
        let bytes = label.as_bytes();
        let len = bytes.len().min(63);
        out.push(len as u8);
        out.extend_from_slice(&bytes[..len]);
    }
    out.push(0);
}

fn write_record(out: &mut Vec<u8>, r: &Record) {
    let (name, rtype) = match r {
        Record::A { name, .. } => (name, TYPE_A),
        Record::Ptr { name, .. } => (name, TYPE_PTR),
        Record::Txt { name, .. } => (name, TYPE_TXT),
        Record::Srv { name, .. } => (name, TYPE_SRV),
    };
    write_name(out, name);
    out.extend_from_slice(&rtype.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    out.extend_from_slice(&TTL.to_be_bytes());

    // La lunghezza dei dati si sa solo dopo averli scritti: si lascia il posto e si torna qui.
    let len_at = out.len();
    out.extend_from_slice(&0u16.to_be_bytes());
    let start = out.len();

    match r {
        Record::A { address, .. } => out.extend_from_slice(&address.octets()),
        Record::Ptr { target, .. } => write_name(out, target),
        Record::Txt { text, .. } => {
            let bytes = text.as_bytes();
            let len = bytes.len().min(255);
            out.push(len as u8);
            out.extend_from_slice(&bytes[..len]);
        }
        Record::Srv { port, target, .. } => {
            out.extend_from_slice(&0u16.to_be_bytes()); // priorità
            out.extend_from_slice(&0u16.to_be_bytes()); // peso
            out.extend_from_slice(&port.to_be_bytes());
            write_name(out, target);
        }
    }

    let len = (out.len() - start) as u16;
    out[len_at..len_at + 2].copy_from_slice(&len.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zona() -> Zone {
        Zone::new(
            "casa.internal",
            "altkeeper-casa2",
            "8e68e3c9e57288e9f7260efb6ea76c93",
            49500,
            Ipv4Addr::new(10, 0, 0, 5),
            (Ipv4Addr::new(10, 0, 0, 0), 24),
        )
    }

    /// Costruisce una domanda vera, così i test passano dal parser come farebbe un client.
    fn domanda(name: &str, qtype: u16) -> Vec<u8> {
        let mut q = Vec::new();
        q.extend_from_slice(&0x1234u16.to_be_bytes());
        q.extend_from_slice(&0x0100u16.to_be_bytes()); // ricorsione richiesta
        q.extend_from_slice(&1u16.to_be_bytes());
        q.extend_from_slice(&0u16.to_be_bytes());
        q.extend_from_slice(&0u16.to_be_bytes());
        q.extend_from_slice(&0u16.to_be_bytes());
        write_name(&mut q, name);
        q.extend_from_slice(&qtype.to_be_bytes());
        q.extend_from_slice(&CLASS_IN.to_be_bytes());
        q
    }

    fn risposta(name: &str, qtype: u16) -> Vec<u8> {
        handle(&zona(), &domanda(name, qtype)).expect("nessuna risposta")
    }

    fn conta_risposte(msg: &[u8]) -> u16 {
        u16::from_be_bytes([msg[6], msg[7]])
    }

    fn rcode(msg: &[u8]) -> u16 {
        u16::from_be_bytes([msg[2], msg[3]]) & 0xF
    }

    #[test]
    fn il_nome_va_e_torna_uguale() {
        let mut buf = Vec::new();
        write_name(&mut buf, "casa2._altserver._tcp.casa.internal");
        let (letto, fine) = read_name(&buf, 0).unwrap();
        assert_eq!(letto, "casa2._altserver._tcp.casa.internal");
        assert_eq!(fine, buf.len());
    }

    #[test]
    fn la_ricerca_dei_server_trova_listanza() {
        let msg = risposta("_altserver._tcp.casa.internal", TYPE_PTR);
        assert_eq!(rcode(&msg), RCODE_OK);
        assert_eq!(conta_risposte(&msg), 1);
        // Il nome dell'istanza compare nei dati della risposta.
        let testo = String::from_utf8_lossy(&msg);
        assert!(testo.contains("altkeeper-casa2"), "istanza assente: {testo:?}");
    }

    #[test]
    fn listanza_da_porta_e_serverid() {
        let srv = risposta("altkeeper-casa2._altserver._tcp.casa.internal", TYPE_SRV);
        assert_eq!(conta_risposte(&srv), 1);
        // La porta 49500 sta nei dati come due byte.
        assert!(srv.windows(2).any(|w| w == 49500u16.to_be_bytes()), "porta assente");

        let txt = risposta("altkeeper-casa2._altserver._tcp.casa.internal", TYPE_TXT);
        assert_eq!(conta_risposte(&txt), 1);
        let testo = String::from_utf8_lossy(&txt);
        assert!(testo.contains("serverID=8e68e3c9"), "serverID assente: {testo:?}");
    }

    #[test]
    fn il_nome_host_da_lindirizzo() {
        let msg = risposta("altkeeper-8e68e3c9.casa.internal", TYPE_A);
        assert_eq!(conta_risposte(&msg), 1);
        assert!(msg.windows(4).any(|w| w == [10, 0, 0, 5]), "indirizzo assente");
    }

    #[test]
    fn i_domini_di_navigazione_rimandano_al_nostro() {
        for prefisso in ["b", "db", "lb"] {
            let msg = risposta(&format!("{prefisso}._dns-sd._udp.casa.internal"), TYPE_PTR);
            assert_eq!(rcode(&msg), RCODE_OK, "{prefisso}");
            assert_eq!(conta_risposte(&msg), 1, "{prefisso}");
        }
    }

    #[test]
    fn ios_scopre_il_dominio_dal_rovescio_del_proprio_indirizzo() {
        // È la domanda che iOS fa da solo, senza niente di impostato sul telefono.
        let msg = risposta("lb._dns-sd._udp.7.0.0.10.in-addr.arpa", TYPE_PTR);
        assert_eq!(rcode(&msg), RCODE_OK);
        assert_eq!(conta_risposte(&msg), 1);
        let testo = String::from_utf8_lossy(&msg);
        assert!(testo.contains("casa"), "dominio assente: {testo:?}");
    }

    #[test]
    fn vale_anche_la_forma_con_la_sola_rete() {
        let msg = risposta("lb._dns-sd._udp.0.0.10.in-addr.arpa", TYPE_PTR);
        assert_eq!(conta_risposte(&msg), 1);
    }

    #[test]
    fn un_rovescio_di_unaltra_rete_non_ci_riguarda() {
        // 192.168.9.4: fuori dalla sottorete dei client, non dobbiamo rispondere.
        let msg = risposta("lb._dns-sd._udp.4.9.168.192.in-addr.arpa", TYPE_PTR);
        assert_eq!(rcode(&msg), RCODE_NXDOMAIN);
    }

    #[test]
    fn un_rovescio_malformato_non_fa_danni() {
        for nome in [
            "lb._dns-sd._udp.999.0.0.10.in-addr.arpa",
            "lb._dns-sd._udp.a.b.c.d.in-addr.arpa",
            "lb._dns-sd._udp.1.2.3.4.5.in-addr.arpa",
            "lb._dns-sd._udp..in-addr.arpa",
        ] {
            let msg = risposta(nome, TYPE_PTR);
            assert_eq!(rcode(&msg), RCODE_NXDOMAIN, "{nome}");
        }
    }

    #[test]
    fn il_nome_istanza_con_le_maiuscole_si_trova_lo_stesso() {
        // Un nome host con le maiuscole non deve impedire al server di trovare se stesso.
        let z = Zone::new(
            "casa.internal",
            "AltKeeper-Casa2",
            "8e68e3c9e57288e9f7260efb6ea76c93",
            49500,
            Ipv4Addr::new(10, 0, 0, 5),
            (Ipv4Addr::new(10, 0, 0, 0), 24),
        );
        assert_eq!(z.instance, "altkeeper-casa2._altserver._tcp.casa.internal");
        let msg = handle(&z, &domanda("altkeeper-casa2._altserver._tcp.casa.internal", TYPE_SRV))
            .expect("nessuna risposta");
        assert_eq!(conta_risposte(&msg), 1);
    }

    #[test]
    fn un_serverid_con_le_maiuscole_non_rompe_il_nome_host() {
        let z = Zone::new(
            "casa.internal",
            "altkeeper-casa2",
            "8E68E3C9E57288E9F7260EFB6EA76C93",
            49500,
            Ipv4Addr::new(10, 0, 0, 5),
            (Ipv4Addr::new(10, 0, 0, 0), 24),
        );
        let msg = handle(&z, &domanda("altkeeper-8e68e3c9.casa.internal", TYPE_A))
            .expect("nessuna risposta");
        assert_eq!(conta_risposte(&msg), 1);
        // Nel TXT il serverID deve restare come l'utente l'ha scritto.
        let txt = handle(&z, &domanda("altkeeper-casa2._altserver._tcp.casa.internal", TYPE_TXT))
            .expect("nessuna risposta");
        assert!(String::from_utf8_lossy(&txt).contains("serverID=8E68E3C9"));
    }

    #[test]
    fn una_risposta_troppo_grande_viene_segnalata_come_troncata() {
        // Con un dominio lunghissimo la risposta supera il limite UDP: deve tornare vuota ma
        // con la bandierina di troncamento, cosi il client riprova in TCP.
        let lungo = ["abcdefghijklmnopqrstuvwxyz012345678901234567890123456789012"; 4].join(".");
        let z = Zone::new(
            &lungo,
            "altkeeper-casa2",
            "8e68e3c9e57288e9f7260efb6ea76c93",
            49500,
            Ipv4Addr::new(10, 0, 0, 5),
            (Ipv4Addr::new(10, 0, 0, 0), 24),
        );
        let nome = format!("altkeeper-casa2._altserver._tcp.{lungo}");
        let msg = handle(&z, &domanda(&nome, TYPE_ANY)).expect("nessuna risposta");
        let flags = u16::from_be_bytes([msg[2], msg[3]]);
        assert_ne!(flags & FLAG_TRUNCATED, 0, "manca la bandierina di troncamento");
        assert_eq!(rcode(&msg), RCODE_OK);
        assert_eq!(conta_risposte(&msg), 0);
        assert!(msg.len() <= MAX_REPLY);
    }

    #[test]
    fn un_nome_che_non_e_nostro_da_nxdomain() {
        let msg = risposta("qualcosa.altro.internal", TYPE_A);
        assert_eq!(rcode(&msg), RCODE_NXDOMAIN);
        assert_eq!(conta_risposte(&msg), 0);
    }

    #[test]
    fn un_nome_nostro_col_tipo_sbagliato_non_da_errore() {
        // Esiste, ma non ha record di quel tipo: la risposta è vuota, non NXDOMAIN.
        // Se rispondessimo NXDOMAIN, il client smetterebbe di chiedere anche gli altri tipi.
        let msg = risposta("altkeeper-8e68e3c9.casa.internal", TYPE_TXT);
        assert_eq!(rcode(&msg), RCODE_OK);
        assert_eq!(conta_risposte(&msg), 0);
    }

    #[test]
    fn maiuscole_e_minuscole_non_contano() {
        let msg = risposta("_ALTSERVER._TCP.CASA.INTERNAL", TYPE_PTR);
        assert_eq!(conta_risposte(&msg), 1);
    }

    #[test]
    fn una_risposta_non_viene_scambiata_per_domanda() {
        let mut msg = domanda("_altserver._tcp.casa.internal", TYPE_PTR);
        msg[2] |= 0x80; // bandierina "è una risposta"
        assert!(handle(&zona(), &msg).is_none());
    }

    #[test]
    fn un_messaggio_troncato_non_fa_danni() {
        let q = domanda("_altserver._tcp.casa.internal", TYPE_PTR);
        for fino in 0..q.len() {
            let _ = handle(&zona(), &q[..fino]);
        }
    }

    #[test]
    fn i_puntatori_di_compressione_sono_rifiutati() {
        let mut q = domanda("_altserver._tcp.casa.internal", TYPE_PTR);
        q[12] = 0xC0; // primo byte del nome: puntatore
        assert!(handle(&zona(), &q).is_none());
    }
}
