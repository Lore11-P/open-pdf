//! Controllo di revoca OCSP (RFC 6960) per firme PAdES B-LT.
//!
//! Il modulo costruisce una `OCSPRequest` con `CertID` SHA-1 (hash del nome
//! dell'emittente e della sua chiave pubblica, più il seriale del certificato),
//! eventuale nonce, e la invia al responder via HTTP POST usando `curl.exe`
//! (`Content-Type: application/ocsp-request`, timeout 30 secondi). La risposta
//! `BasicOCSPResponse` viene decodificata senza dipendenze aggiuntive:
//!
//! - stato per il `CertID` richiesto (`good`, `revoked`, `unknown`);
//! - `thisUpdate` / `nextUpdate` formattati in UTC;
//! - `revocationTime` e motivo della revoca;
//! - `ResponderID` (per nome o per hash di chiave);
//! - verifica della firma della risposta con il certificato del responder
//!   incluso nella risposta oppure, in sua assenza, con il certificato
//!   dell'emittente quando il `ResponderID` lo identifica.
//!
//! [`check`] restituisce solo l'esito; [`check_with_response`] restituisce
//! anche il DER della risposta, necessario per archiviarlo nel DSS di una
//! firma PAdES B-LT.
//!
//! La verifica della firma copre RSA PKCS#1 v1.5 (SHA-1/256/384/512) ed ECDSA
//! P-256/P-384 (SHA-1/256/384/512) sui byte DER esatti del `tbsResponseData`.
//! `signature_ok` vale `false` quando nessun certificato utilizzabile è
//! disponibile o l'algoritmo non è supportato; la struttura della risposta
//! resta comunque leggibile.
//!
//! Limiti dichiarati:
//! - il `CertID` usa solo SHA-1, come ammesso da RFC 6960 e richiesto dai
//!   responder più diffusi; non viene negoziato SHA-256;
//! - un responder delegato viene usato per verificare la firma ma la sua
//!   autorizzazione (EKU `id-kp-OCSPSigning` e firma della CA) non viene
//!   controllata: la catena va validata a monte;
//! - il nonce viene inviato ma il suo eco nella risposta non è obbligatorio,
//!   quindi non viene preteso: la difesa dal replay si basa sulla freschezza
//!   di `thisUpdate`/`nextUpdate`;
//! - le firme RSA-PSS non sono supportate (`signature_ok` risulta `false`);
//! - non esiste fallback su CRL: se il responder non risponde, il controllo
//!   fallisce con un errore esplicito.
//!
//! I test non accedono a Internet: le risposte usate come campione sono
//! generate con OpenSSL e salvate in `tests/fixtures/ocsp/`; il percorso HTTP
//! completo viene provato con un responder minimale su `127.0.0.1`.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use ocsp::common::asn1::{CertId, Oid};
use ocsp::common::ocsp::{OcspExt, OcspExtI};
use ocsp::oid::ALGO_SHA1_DOT;
use ocsp::request::OneReq;
use rasn_pkix::Certificate;
use rsa::pkcs8::DecodePublicKey;
use rsa::signature::hazmat::PrehashVerifier;
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, RsaPublicKey};
use sha2::{Digest, Sha256, Sha384, Sha512};

const OID_AIA: [u32; 9] = [1, 3, 6, 1, 5, 5, 7, 1, 1];
const OID_OCSP_ACCESS: [u32; 9] = [1, 3, 6, 1, 5, 5, 7, 48, 1];
const OID_OCSP_BASIC: &str = "1.3.6.1.5.5.7.48.1.1";
const OID_SHA1: &str = "1.3.14.3.2.26";
const NONCE_MAX: usize = 32;
const TIMEOUT_SECONDI: u64 = 30;

/// Stato di revoca del certificato secondo la risposta OCSP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevocationStatus {
    /// Il responder conosce il certificato e lo considera valido.
    Good,
    /// Il certificato è revocato, con data e motivo indicati dal responder.
    Revoked { time: String, reason: String },
    /// Il responder non conosce il certificato richiesto.
    Unknown,
}

/// Esito del controllo OCSP di un singolo certificato.
#[derive(Debug, Clone)]
pub struct OcspResult {
    /// Stato di revoca riferito al `CertID` richiesto.
    pub status: RevocationStatus,
    /// Data `thisUpdate` della risposta, formattata in UTC.
    pub this_update: String,
    /// Data `nextUpdate`, se il responder l'ha inclusa.
    pub next_update: Option<String>,
    /// Identità del responder (`ResponderID`) in forma leggibile.
    pub responder: Option<String>,
    /// `true` se la firma della risposta è stata verificata con successo.
    pub signature_ok: bool,
}

/// Controlla la revoca del certificato interrogando il responder OCSP.
///
/// `cert_der` è il certificato del firmatario in DER, `issuer_der` il suo
/// emittente. Se `url` è `None` (o vuoto) l'indirizzo viene letto
/// dall'estensione `authorityInfoAccess` del certificato. La richiesta viene
/// inviata in POST con `curl`, con timeout complessivo di 30 secondi.
pub fn check(cert_der: &[u8], issuer_der: &[u8], url: Option<&str>) -> Result<OcspResult, String> {
    check_with_response(cert_der, issuer_der, url).map(|(esito, _)| esito)
}

/// Come [`check`], ma restituisce anche la risposta OCSP in DER.
///
/// I byte restituiti sono quelli da archiviare nel DSS di una firma PAdES
/// B-LT (`/OCSPs` e voce `/VRI`); [`parse_response`] li rilegge senza rete.
pub fn check_with_response(
    cert_der: &[u8],
    issuer_der: &[u8],
    url: Option<&str>,
) -> Result<(OcspResult, Vec<u8>), String> {
    let firmatario = analizza_certificato(cert_der)
        .map_err(|errore| format!("certificato del firmatario non valido: {errore}"))?;
    // Analizza subito l'emittente: un certificato malformato deve fallire prima
    // di contattare la rete.
    analizza_certificato(issuer_der)
        .map_err(|errore| format!("certificato dell'emittente non valido: {errore}"))?;

    let indirizzo = match url.map(str::trim).filter(|valore| !valore.is_empty()) {
        Some(valore) => valore.to_string(),
        None => firmatario.aia.clone().ok_or_else(|| {
            "il certificato non indica un responder OCSP nell'estensione authorityInfoAccess"
                .to_string()
        })?,
    };
    let minuscolo = indirizzo.to_ascii_lowercase();
    if !minuscolo.starts_with("http://") && !minuscolo.starts_with("https://") {
        return Err(format!(
            "URL del responder OCSP non valido (atteso http:// o https://): {indirizzo}"
        ));
    }

    let nonce = genera_nonce();
    let richiesta = costruisci_richiesta(cert_der, issuer_der, Some(&nonce))?;
    let risposta = invia_richiesta(&indirizzo, &richiesta)?;
    let esito = parse_response(&risposta, cert_der, issuer_der)?;
    Ok((esito, risposta))
}

/// Interpreta una risposta OCSP già ricevuta (variante offline).
///
/// `response_der` è la `OCSPResponse` completa, `cert_der` il certificato del
/// firmatario e `issuer_der` il suo emittente. La funzione non effettua
/// accessi alla rete: serve a validare risposte archiviate nel DSS o ricevute
/// da un altro canale.
pub fn parse_response(
    response_der: &[u8],
    cert_der: &[u8],
    issuer_der: &[u8],
) -> Result<OcspResult, String> {
    let firmatario = analizza_certificato(cert_der)
        .map_err(|errore| format!("certificato del firmatario non valido: {errore}"))?;
    let emittente = analizza_certificato(issuer_der)
        .map_err(|errore| format!("certificato dell'emittente non valido: {errore}"))?;

    let (risposta, _) =
        leggi_tlv(response_der).map_err(|errore| format!("risposta OCSP malformata: {errore}"))?;
    if risposta.tag != 0x30 {
        return Err("risposta OCSP malformata: attesa una SEQUENCE".to_string());
    }
    let parti =
        campi(risposta.valore).map_err(|errore| format!("risposta OCSP malformata: {errore}"))?;
    let stato_generale = parti
        .first()
        .ok_or_else(|| "risposta OCSP malformata: responseStatus assente".to_string())?;
    if stato_generale.tag != 0x0a {
        return Err("risposta OCSP malformata: responseStatus non è un ENUMERATED".to_string());
    }
    let codice = stato_generale.valore.first().copied().unwrap_or(0);
    if codice != 0 {
        return Err(format!(
            "il responder OCSP ha rifiutato la richiesta: {}",
            descrivi_stato(codice)
        ));
    }

    let contenitore = parti
        .get(1)
        .filter(|elemento| elemento.tag == 0xa0)
        .ok_or_else(|| "risposta OCSP malformata: responseBytes assente".to_string())?;
    let (response_bytes, _) = leggi_tlv(contenitore.valore)
        .map_err(|errore| format!("risposta OCSP malformata: {errore}"))?;
    if response_bytes.tag != 0x30 {
        return Err("risposta OCSP malformata: responseBytes non è una SEQUENCE".to_string());
    }
    let campi_bytes = campi(response_bytes.valore)
        .map_err(|errore| format!("risposta OCSP malformata: {errore}"))?;
    let tipo = campi_bytes
        .first()
        .filter(|elemento| elemento.tag == 0x06)
        .ok_or_else(|| "risposta OCSP malformata: responseType assente".to_string())?;
    if oid_a_string(tipo.valore) != OID_OCSP_BASIC {
        return Err(format!(
            "risposta OCSP di tipo non supportato: {}",
            oid_a_string(tipo.valore)
        ));
    }
    let contenuto = campi_bytes
        .get(1)
        .filter(|elemento| elemento.tag == 0x04)
        .ok_or_else(|| "risposta OCSP malformata: contenuto della risposta assente".to_string())?;

    let (basic, _) = leggi_tlv(contenuto.valore)
        .map_err(|errore| format!("risposta OCSP malformata: {errore}"))?;
    if basic.tag != 0x30 {
        return Err("risposta OCSP malformata: BasicOCSPResponse non è una SEQUENCE".to_string());
    }
    let campi_basic =
        campi(basic.valore).map_err(|errore| format!("risposta OCSP malformata: {errore}"))?;
    if campi_basic.len() < 3 {
        return Err(
            "risposta OCSP malformata: BasicOCSPResponse incompleta (tbs, algoritmo o firma)"
                .to_string(),
        );
    }
    let tbs = &campi_basic[0];
    let algoritmo = &campi_basic[1];
    let firma_bit = &campi_basic[2];
    if tbs.tag != 0x30 || algoritmo.tag != 0x30 || firma_bit.tag != 0x03 {
        return Err(
            "risposta OCSP malformata: struttura di BasicOCSPResponse non valida".to_string(),
        );
    }
    let certificati_inclusi = certificati_responder(campi_basic.get(3));

    let campi_tbs =
        campi(tbs.valore).map_err(|errore| format!("risposta OCSP malformata: {errore}"))?;
    let base = if campi_tbs.first().map(|elemento| elemento.tag) == Some(0xa0) {
        1
    } else {
        0
    };
    let elemento_responder = campi_tbs
        .get(base)
        .ok_or_else(|| "risposta OCSP malformata: ResponderID assente".to_string())?;
    let risposte = campi_tbs
        .get(base + 2)
        .filter(|elemento| elemento.tag == 0x30)
        .ok_or_else(|| "risposta OCSP malformata: elenco delle risposte assente".to_string())?;
    let responder = identifica_responder(elemento_responder);

    let singole =
        campi(risposte.valore).map_err(|errore| format!("risposta OCSP malformata: {errore}"))?;
    let hash_nome_emittente = sha1(&emittente.soggetto);
    let hash_chiave_emittente = sha1(&emittente.chiave);
    let mut con_seriale = None;
    let mut corrispondente = None;
    for singola in &singole {
        if singola.tag != 0x30 {
            continue;
        }
        let Ok(elementi) = campi(singola.valore) else {
            continue;
        };
        let Some(certid_tlv) = elementi.first() else {
            continue;
        };
        let Ok(certid) = campi(certid_tlv.valore) else {
            continue;
        };
        if certid.len() < 4 || certid[3].tag != 0x02 {
            continue;
        }
        if !stesso_seriale(certid[3].valore, &firmatario.seriale) {
            continue;
        }
        if oid_a_string(certid[0].valore) == OID_SHA1 {
            // Con CertID SHA-1 l'emittente è verificabile: un hash diverso
            // indica la risposta di un'altra CA con lo stesso seriale e va
            // scartato.
            if certid[1].valore == hash_nome_emittente.as_slice()
                && certid[2].valore == hash_chiave_emittente.as_slice()
            {
                corrispondente = Some(singola);
                break;
            }
        } else if con_seriale.is_none() {
            // Algoritmo di hash diverso da SHA-1: non è calcolabile con le
            // sole dipendenze del progetto, ci si limita al seriale.
            con_seriale = Some(singola);
        }
    }
    let singola = corrispondente.or(con_seriale).ok_or_else(|| {
        "CertID del certificato assente nella risposta OCSP: il responder ha risposto per un altro certificato"
            .to_string()
    })?;

    let elementi =
        campi(singola.valore).map_err(|errore| format!("risposta OCSP malformata: {errore}"))?;
    let stato_cert = elementi
        .get(1)
        .ok_or_else(|| "risposta OCSP malformata: certStatus assente".to_string())?;
    let this_update = elementi
        .get(2)
        .filter(|elemento| elemento.tag == 0x18)
        .map(|elemento| formatta_tempo(elemento.valore))
        .ok_or_else(|| "risposta OCSP malformata: thisUpdate assente".to_string())?;
    let next_update = elementi
        .get(3)
        .filter(|elemento| elemento.tag == 0xa0)
        .and_then(|elemento| leggi_tlv(elemento.valore).ok())
        .filter(|(interno, _)| interno.tag == 0x18)
        .map(|(interno, _)| formatta_tempo(interno.valore));

    let status = match stato_cert.tag {
        // `good [0] IMPLICIT NULL` viene codificato primitivo (0x80) da alcuni
        // responder e costruito (0xa0) da altri: entrambe le forme sono valide.
        0x80 | 0xa0 => RevocationStatus::Good,
        0x82 | 0xa2 => RevocationStatus::Unknown,
        0xa1 => {
            let info = campi(stato_cert.valore)
                .map_err(|errore| format!("risposta OCSP malformata (revoca): {errore}"))?;
            let tempo = info
                .first()
                .filter(|elemento| elemento.tag == 0x18)
                .map(|elemento| formatta_tempo(elemento.valore))
                .unwrap_or_else(|| "data di revoca non indicata".to_string());
            let motivo = info
                .iter()
                .skip(1)
                .filter(|elemento| elemento.tag == 0xa0)
                .find_map(|elemento| {
                    leggi_tlv(elemento.valore)
                        .ok()
                        .filter(|(interno, _)| interno.tag == 0x0a)
                        .map(|(interno, _)| motivo_revoca(interno.valore.first().copied()))
                })
                .unwrap_or_else(|| "motivo non specificato".to_string());
            RevocationStatus::Revoked {
                time: tempo,
                reason: motivo,
            }
        }
        _ => {
            return Err(
                "risposta OCSP malformata: stato del certificato non riconosciuto".to_string(),
            )
        }
    };

    let algoritmo_oid = campi(algoritmo.valore)
        .ok()
        .and_then(|elementi| {
            elementi
                .first()
                .filter(|elemento| elemento.tag == 0x06)
                .map(|elemento| oid_a_string(elemento.valore))
        })
        .ok_or_else(|| "risposta OCSP malformata: algoritmo di firma assente".to_string())?;
    let firma = firma_bytes(firma_bit);

    let certificato_firma =
        scegli_certificato_firma(&responder, &certificati_inclusi, issuer_der, &emittente);
    let signature_ok = match certificato_firma {
        Some(der) => {
            let info = match analizza_certificato(&der) {
                Ok(valore) => valore,
                Err(_) => {
                    return Ok(OcspResult {
                        status,
                        this_update,
                        next_update,
                        responder: responder.descrizione,
                        signature_ok: false,
                    })
                }
            };
            match algoritmo_firma(&algoritmo_oid) {
                Some((hash, true)) => verifica_ecdsa(&info.spki, hash, tbs.grezzo, &firma),
                Some((hash, false)) => verifica_rsa(&info.spki, hash, tbs.grezzo, &firma),
                None => false,
            }
        }
        None => false,
    };

    Ok(OcspResult {
        status,
        this_update,
        next_update,
        responder: responder.descrizione,
        signature_ok,
    })
}

/// Informazioni estratte da un certificato X.509 con lettura DER diretta.
struct Certificato {
    /// Valore dell'INTEGER `serialNumber`, senza alcuna normalizzazione.
    seriale: Vec<u8>,
    /// TLV completo del `Name` dell'emittente.
    emittente: Vec<u8>,
    /// TLV completo del `Name` del soggetto.
    soggetto: Vec<u8>,
    /// Contenuto del BIT STRING `subjectPublicKey` dell'emittente.
    chiave: Vec<u8>,
    /// TLV completo di `subjectPublicKeyInfo`.
    spki: Vec<u8>,
    /// Primo URL OCSP dell'estensione `authorityInfoAccess`.
    aia: Option<String>,
}

/// Elemento DER (tag, valore e TLV completo) usato dal parser interno.
struct Tlv<'a> {
    tag: u8,
    valore: &'a [u8],
    grezzo: &'a [u8],
}

fn leggi_tlv(dati: &[u8]) -> Result<(Tlv<'_>, &[u8]), String> {
    if dati.len() < 2 {
        return Err("dati ASN.1 troncati".to_string());
    }
    let tag = dati[0];
    let primo = dati[1];
    let (lunghezza, inizio) = if primo < 0x80 {
        (primo as usize, 2)
    } else if primo == 0x80 {
        return Err("lunghezza ASN.1 indefinita non ammessa in DER".to_string());
    } else {
        let conteggio = (primo & 0x7f) as usize;
        if conteggio == 0 || conteggio > 4 || dati.len() < 2 + conteggio {
            return Err("lunghezza ASN.1 non valida".to_string());
        }
        let mut valore = 0usize;
        for byte in &dati[2..2 + conteggio] {
            valore = (valore << 8) | *byte as usize;
        }
        (valore, 2 + conteggio)
    };
    let fine = inizio
        .checked_add(lunghezza)
        .ok_or_else(|| "lunghezza ASN.1 non valida".to_string())?;
    if fine > dati.len() {
        return Err("dati ASN.1 troncati".to_string());
    }
    Ok((
        Tlv {
            tag,
            valore: &dati[inizio..fine],
            grezzo: &dati[..fine],
        },
        &dati[fine..],
    ))
}

fn campi(dati: &[u8]) -> Result<Vec<Tlv<'_>>, String> {
    let mut elenco = Vec::new();
    let mut resta = dati;
    while !resta.is_empty() {
        let (elemento, coda) = leggi_tlv(resta)?;
        elenco.push(elemento);
        resta = coda;
    }
    Ok(elenco)
}

/// Codifica un TLV DER con la lunghezza calcolata.
fn tlv(tag: u8, valore: &[u8]) -> Vec<u8> {
    let mut risultato = vec![tag];
    let lunghezza = valore.len();
    if lunghezza < 0x80 {
        risultato.push(lunghezza as u8);
    } else {
        let byte = lunghezza.to_be_bytes();
        let primo = byte
            .iter()
            .position(|valore| *valore != 0)
            .unwrap_or(byte.len() - 1);
        let significativi = &byte[primo..];
        risultato.push(0x80 | significativi.len() as u8);
        risultato.extend_from_slice(significativi);
    }
    risultato.extend_from_slice(valore);
    risultato
}

/// Converte il contenuto di un OBJECT IDENTIFIER in notazione puntata.
fn oid_a_string(contenuto: &[u8]) -> String {
    if contenuto.is_empty() {
        return String::new();
    }
    let primo = contenuto[0] as u32;
    let (arco_primario, arco_secondario) = if primo < 80 {
        (primo / 40, primo % 40)
    } else {
        (2, primo - 80)
    };
    let mut archi: Vec<u64> = vec![arco_primario as u64, arco_secondario as u64];
    let mut corrente: u64 = 0;
    let mut in_corso = false;
    for byte in &contenuto[1..] {
        corrente = (corrente << 7) | (*byte & 0x7f) as u64;
        in_corso = *byte & 0x80 != 0;
        if !in_corso {
            archi.push(corrente);
            corrente = 0;
        }
    }
    if in_corso {
        archi.push(corrente);
    }
    archi
        .iter()
        .map(|arco| arco.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

fn formatta_tempo(grezzo: &[u8]) -> String {
    let testo = String::from_utf8_lossy(grezzo);
    let testo = testo.trim();
    let senza_z = testo.trim_end_matches('Z');
    let cifre = senza_z.split('.').next().unwrap_or(senza_z);
    if cifre.len() >= 14 {
        if let Ok(quando) = chrono::NaiveDateTime::parse_from_str(&cifre[..14], "%Y%m%d%H%M%S") {
            return quando.format("%Y-%m-%d %H:%M:%S UTC").to_string();
        }
    }
    testo.to_string()
}

fn descrivi_stato(codice: u8) -> String {
    match codice {
        1 => "richiesta malformata (malformedRequest)".to_string(),
        2 => "errore interno (internalError)".to_string(),
        3 => "riprovare più tardi (tryLater)".to_string(),
        5 => "firma richiesta (sigRequired)".to_string(),
        6 => "richiesta non autorizzata (unauthorized)".to_string(),
        altro => format!("stato sconosciuto ({altro})"),
    }
}

fn motivo_revoca(codice: Option<u8>) -> String {
    match codice {
        Some(0) => "motivo non specificato".to_string(),
        Some(1) => "compromissione della chiave".to_string(),
        Some(2) => "compromissione della CA".to_string(),
        Some(3) => "cambio di affiliazione".to_string(),
        Some(4) => "sostituito".to_string(),
        Some(5) => "cessata operatività".to_string(),
        Some(6) => "certificato sospeso".to_string(),
        Some(8) => "rimozione dalla CRL".to_string(),
        Some(9) => "ritiro del privilegio".to_string(),
        Some(10) => "compromissione della autorità di attribuzione".to_string(),
        Some(altro) => format!("motivo sconosciuto ({altro})"),
        None => "motivo non specificato".to_string(),
    }
}

fn senza_zeri_iniziali(valore: &[u8]) -> &[u8] {
    let mut indice = 0;
    while indice + 1 < valore.len() && valore[indice] == 0 {
        indice += 1;
    }
    &valore[indice..]
}

fn stesso_seriale(uno: &[u8], altro: &[u8]) -> bool {
    !uno.is_empty() && senza_zeri_iniziali(uno) == senza_zeri_iniziali(altro)
}

fn hex_breve(dati: &[u8]) -> String {
    let mut testo = String::new();
    for (indice, byte) in dati.iter().take(8).enumerate() {
        if indice > 0 {
            testo.push(':');
        }
        testo.push_str(&format!("{byte:02x}"));
    }
    if dati.len() > 8 {
        testo.push('…');
    }
    testo
}

fn stringa_da_tlv(elemento: &Tlv<'_>) -> String {
    if elemento.tag == 0x1e {
        let unita: Vec<u16> = elemento
            .valore
            .chunks(2)
            .map(|coppia| {
                if coppia.len() == 2 {
                    ((coppia[0] as u16) << 8) | coppia[1] as u16
                } else {
                    coppia[0] as u16
                }
            })
            .collect();
        String::from_utf16_lossy(&unita)
    } else {
        String::from_utf8_lossy(elemento.valore).to_string()
    }
}

fn nome_comune(nome: &rasn_pkix::Name) -> Option<String> {
    let rasn_pkix::Name::RdnSequence(rdns) = nome;
    for rdn in rdns.iter() {
        for attributo in rdn.iter() {
            if [2u32, 5, 4, 3] == attributo.r#type {
                if let Ok((valore, _)) = leggi_tlv(attributo.value.as_bytes()) {
                    let testo = stringa_da_tlv(&valore);
                    let testo = testo.trim();
                    if !testo.is_empty() {
                        return Some(testo.to_string());
                    }
                }
            }
        }
    }
    None
}

fn url_ocsp(certificato: &Certificate) -> Option<String> {
    let estensioni = certificato.tbs_certificate.extensions.as_ref()?;
    for estensione in estensioni.iter() {
        if OID_AIA != estensione.extn_id {
            continue;
        }
        let Ok(info) =
            rasn::der::decode::<rasn_pkix::AuthorityInfoAccessSyntax>(&estensione.extn_value)
        else {
            continue;
        };
        for descrizione in info.iter() {
            if OID_OCSP_ACCESS != descrizione.access_method {
                continue;
            }
            if let rasn_pkix::GeneralName::Uri(uri) = &descrizione.access_location {
                let indirizzo = String::from_utf8_lossy(uri.as_iso646_bytes())
                    .trim()
                    .to_string();
                if !indirizzo.is_empty() {
                    return Some(indirizzo);
                }
            }
        }
    }
    None
}

fn analizza_certificato(der: &[u8]) -> Result<Certificato, String> {
    let (certificato, _) = leggi_tlv(der)?;
    if certificato.tag != 0x30 {
        return Err("attesa una SEQUENCE di certificato".to_string());
    }
    let parti = campi(certificato.valore)?;
    let tbs = parti
        .first()
        .filter(|elemento| elemento.tag == 0x30)
        .ok_or_else(|| "tbsCertificate assente".to_string())?;
    let campi_tbs = campi(tbs.valore)?;
    let base = if campi_tbs.first().map(|elemento| elemento.tag) == Some(0xa0) {
        1
    } else {
        0
    };
    if campi_tbs.len() < base + 6 {
        return Err("tbsCertificate incompleto".to_string());
    }
    if campi_tbs[base].tag != 0x02 {
        return Err("serialNumber non è un INTEGER".to_string());
    }
    let spki_tlv = &campi_tbs[base + 5];
    if spki_tlv.tag != 0x30 {
        return Err("subjectPublicKeyInfo non è una SEQUENCE".to_string());
    }
    let spki_interni = campi(spki_tlv.valore)?;
    let bit = spki_interni
        .get(1)
        .filter(|elemento| elemento.tag == 0x03 && elemento.valore.len() >= 2)
        .ok_or_else(|| "chiave pubblica del certificato assente".to_string())?;
    // Il primo byte del BIT STRING conta i bit non usati: le chiavi X.509 sono
    // allineate al byte, quindi il contenuto utile parte dal secondo.
    let mut chiave = bit.valore.to_vec();
    chiave.remove(0);

    let certificato_rasn = rasn::der::decode::<Certificate>(der)
        .map_err(|errore| format!("struttura X.509 non decodificabile: {errore}"))?;

    Ok(Certificato {
        seriale: campi_tbs[base].valore.to_vec(),
        emittente: campi_tbs[base + 2].grezzo.to_vec(),
        soggetto: campi_tbs[base + 4].grezzo.to_vec(),
        chiave,
        spki: spki_tlv.grezzo.to_vec(),
        aia: url_ocsp(&certificato_rasn),
    })
}

/// Costruisce la `OCSPRequest` DER per il certificato indicato.
fn costruisci_richiesta(
    cert_der: &[u8],
    issuer_der: &[u8],
    nonce: Option<&[u8]>,
) -> Result<Vec<u8>, String> {
    let firmatario = analizza_certificato(cert_der)
        .map_err(|errore| format!("certificato del firmatario non valido: {errore}"))?;
    let emittente = analizza_certificato(issuer_der)
        .map_err(|errore| format!("certificato dell'emittente non valido: {errore}"))?;

    let oid_sha1 = Oid::new_from_dot(ALGO_SHA1_DOT)
        .map_err(|errore| format!("OID SHA-1 non disponibile: {errore}"))?;
    let certid = CertId::new(
        oid_sha1,
        &sha1(&emittente.soggetto),
        &sha1(&emittente.chiave),
        &firmatario.seriale,
    );
    let richiesta = OneReq {
        certid,
        one_req_ext: None,
    };
    let mut corpo = OneReq::list_to_der(std::slice::from_ref(&richiesta))
        .map_err(|errore| format!("richiesta OCSP non codificabile: {errore}"))?;

    if let Some(valore) = nonce {
        if valore.len() > NONCE_MAX {
            return Err(format!(
                "nonce OCSP troppo lungo ({} byte, massimo {NONCE_MAX})",
                valore.len()
            ));
        }
        // L'estensione nonce contiene il DER di un OCTET STRING, a sua volta
        // racchiuso nell'extnValue: il crate aggiunge la seconda busta.
        let estensione = OcspExtI {
            id: 0,
            ext: OcspExt::Nonce {
                nonce: tlv(0x04, valore),
            },
        };
        corpo.extend(
            OcspExtI::list_to_der(std::slice::from_ref(&estensione), 0xa2)
                .map_err(|errore| format!("nonce OCSP non codificabile: {errore}"))?,
        );
    }

    let tbs = tlv(0x30, &corpo);
    Ok(tlv(0x30, &tbs))
}

/// Invia la richiesta OCSP in POST tramite `curl.exe`.
fn invia_richiesta(url: &str, corpo: &[u8]) -> Result<Vec<u8>, String> {
    let mut ultimo_errore = "curl non trovato".to_string();
    for eseguibile in ["curl.exe", "curl"] {
        let mut comando = Command::new(eseguibile);
        comando
            .arg("-sS")
            .arg("--max-time")
            .arg(TIMEOUT_SECONDI.to_string())
            .arg("--connect-timeout")
            .arg("10")
            .arg("-H")
            .arg("Content-Type: application/ocsp-request")
            .arg("-H")
            .arg("Accept: application/ocsp-response")
            .arg("--data-binary")
            .arg("@-")
            .arg(url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut figlio = match comando.spawn() {
            Ok(valore) => valore,
            Err(errore) => {
                ultimo_errore = errore.to_string();
                continue;
            }
        };
        if let Some(mut ingresso) = figlio.stdin.take() {
            if let Err(errore) = ingresso.write_all(corpo) {
                let _ = figlio.kill();
                let _ = figlio.wait();
                return Err(format!("invio della richiesta OCSP non riuscito: {errore}"));
            }
        }
        let uscita = figlio
            .wait_with_output()
            .map_err(|errore| format!("risposta del responder OCSP non leggibile: {errore}"))?;
        let dettaglio = String::from_utf8_lossy(&uscita.stderr)
            .trim()
            .replace('\r', " ")
            .replace('\n', " ");
        if !uscita.status.success() && uscita.stdout.is_empty() {
            let motivo = if dettaglio.is_empty() {
                format!("curl ha restituito {}", uscita.status)
            } else {
                dettaglio
            };
            return Err(format!("responder OCSP irraggiungibile ({url}): {motivo}"));
        }
        if uscita.stdout.is_empty() {
            return Err(format!(
                "il responder OCSP ha restituito una risposta vuota ({url})"
            ));
        }
        return Ok(uscita.stdout);
    }
    Err(format!(
        "impossibile contattare il responder OCSP: curl non disponibile ({ultimo_errore})"
    ))
}

/// Nonce casuale di 16 byte, derivato dall'entropia di `RandomState`.
fn genera_nonce() -> Vec<u8> {
    let stato = RandomState::new();
    let adesso = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|durata| durata.as_nanos() as u64)
        .unwrap_or(0);
    let mut nonce = Vec::with_capacity(16);
    for seme in 0u64..2 {
        let mut hasher = stato.build_hasher();
        hasher.write_u64(seme);
        hasher.write_u64(adesso);
        hasher.write_usize(&stato as *const _ as usize);
        nonce.extend_from_slice(&hasher.finish().to_be_bytes());
    }
    nonce
}

struct IdentitaResponder {
    descrizione: Option<String>,
    nome: Option<Vec<u8>>,
    chiave: Option<Vec<u8>>,
}

fn identifica_responder(elemento: &Tlv<'_>) -> IdentitaResponder {
    match elemento.tag {
        0xa1 => {
            let nome = leggi_tlv(elemento.valore)
                .map(|(interno, _)| interno.grezzo.to_vec())
                .unwrap_or_else(|_| elemento.valore.to_vec());
            let comune = rasn::der::decode::<rasn_pkix::Name>(&nome)
                .ok()
                .and_then(|decodificato| nome_comune(&decodificato));
            IdentitaResponder {
                descrizione: Some(match comune {
                    Some(comune) => format!("responder OCSP: {comune}"),
                    None => "responder OCSP identificato per nome".to_string(),
                }),
                nome: Some(nome),
                chiave: None,
            }
        }
        0xa2 => {
            // `ResponderID ::= CHOICE { byKey [2] KeyHash }`: secondo
            // l'encoding il tag può essere implicito o esplicito sull'OCTET STRING.
            let hash = leggi_tlv(elemento.valore)
                .ok()
                .filter(|(interno, _)| interno.tag == 0x04)
                .map(|(interno, _)| interno.valore.to_vec())
                .unwrap_or_else(|| elemento.valore.to_vec());
            IdentitaResponder {
                descrizione: Some(format!(
                    "responder OCSP identificato per hash di chiave {}",
                    hex_breve(&hash)
                )),
                nome: None,
                chiave: Some(hash),
            }
        }
        _ => IdentitaResponder {
            descrizione: Some("responder OCSP non identificato".to_string()),
            nome: None,
            chiave: None,
        },
    }
}

/// Estrae i certificati dall'elemento `[0] EXPLICIT SEQUENCE OF Certificate`.
fn certificati_responder(elemento: Option<&Tlv<'_>>) -> Vec<Vec<u8>> {
    let Some(elemento) = elemento.filter(|valore| valore.tag == 0xa0) else {
        return Vec::new();
    };
    let Ok((sequenza, _)) = leggi_tlv(elemento.valore) else {
        return Vec::new();
    };
    campi(sequenza.valore)
        .map(|elenco| {
            elenco
                .into_iter()
                .filter(|certificato| certificato.tag == 0x30)
                .map(|certificato| certificato.grezzo.to_vec())
                .collect()
        })
        .unwrap_or_default()
}

fn scegli_certificato_firma(
    responder: &IdentitaResponder,
    inclusi: &[Vec<u8>],
    issuer_der: &[u8],
    emittente: &Certificato,
) -> Option<Vec<u8>> {
    if let Some(nome) = &responder.nome {
        if let Some(trovato) = inclusi.iter().find(|der| {
            analizza_certificato(der)
                .map(|certificato| certificato.soggetto == *nome)
                .unwrap_or(false)
        }) {
            return Some(trovato.clone());
        }
    }
    if let Some(chiave) = &responder.chiave {
        if let Some(trovato) = inclusi.iter().find(|der| {
            analizza_certificato(der)
                .map(|certificato| sha1(&certificato.chiave).as_slice() == chiave.as_slice())
                .unwrap_or(false)
        }) {
            return Some(trovato.clone());
        }
    }
    let emittente_corrisponde = match (&responder.nome, &responder.chiave) {
        (Some(nome), _) => emittente.soggetto == *nome,
        (None, Some(chiave)) => sha1(&emittente.chiave).as_slice() == chiave.as_slice(),
        _ => false,
    };
    if emittente_corrisponde {
        return Some(issuer_der.to_vec());
    }
    // RFC 6960: il primo certificato incluso è quello del firmatario. Se il
    // ResponderID non è confrontabile lo si usa comunque, perché
    // `signature_ok` attesta la firma e non l'autorizzazione del responder.
    inclusi.first().cloned()
}

/// Estrae i byte della firma da un BIT STRING (primo byte = bit non usati).
fn firma_bytes(elemento: &Tlv<'_>) -> Vec<u8> {
    if elemento.valore.len() < 2 || elemento.valore[0] != 0 {
        return Vec::new();
    }
    elemento.valore[1..].to_vec()
}

#[derive(Clone, Copy)]
enum HashKind {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

impl HashKind {
    fn digest(self, dati: &[u8]) -> Vec<u8> {
        match self {
            HashKind::Sha1 => sha1(dati).to_vec(),
            HashKind::Sha256 => Sha256::digest(dati).to_vec(),
            HashKind::Sha384 => Sha384::digest(dati).to_vec(),
            HashKind::Sha512 => Sha512::digest(dati).to_vec(),
        }
    }

    /// Prefisso DER del `DigestInfo` PKCS#1 per il digest indicato.
    fn digest_info(self) -> &'static [u8] {
        match self {
            HashKind::Sha1 => &[
                0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00, 0x04,
                0x14,
            ],
            HashKind::Sha256 => &[
                0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x01, 0x05, 0x00, 0x04, 0x20,
            ],
            HashKind::Sha384 => &[
                0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x02, 0x05, 0x00, 0x04, 0x30,
            ],
            HashKind::Sha512 => &[
                0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
                0x03, 0x05, 0x00, 0x04, 0x40,
            ],
        }
    }
}

/// Associa un OID di `signatureAlgorithm` al digest e al tipo di chiave.
fn algoritmo_firma(oid: &str) -> Option<(HashKind, bool)> {
    match oid {
        "1.2.840.113549.1.1.5" => Some((HashKind::Sha1, false)),
        "1.2.840.113549.1.1.11" => Some((HashKind::Sha256, false)),
        "1.2.840.113549.1.1.12" => Some((HashKind::Sha384, false)),
        "1.2.840.113549.1.1.13" => Some((HashKind::Sha512, false)),
        "1.2.840.10045.4.1" => Some((HashKind::Sha1, true)),
        "1.2.840.10045.4.3.2" => Some((HashKind::Sha256, true)),
        "1.2.840.10045.4.3.3" => Some((HashKind::Sha384, true)),
        "1.2.840.10045.4.3.4" => Some((HashKind::Sha512, true)),
        _ => None,
    }
}

/// Verifica una firma RSA PKCS#1 v1.5 ricostruendo il blocco `DigestInfo`.
fn verifica_rsa(spki: &[u8], hash: HashKind, messaggio: &[u8], firma: &[u8]) -> bool {
    let Ok(chiave) = RsaPublicKey::from_public_key_der(spki) else {
        return false;
    };
    let dimensione = chiave.size();
    if firma.is_empty() || firma.len() != dimensione {
        return false;
    }
    let intero = BigUint::from_bytes_be(firma);
    if &intero >= chiave.n() {
        return false;
    }
    let chiaro = intero.modpow(chiave.e(), chiave.n()).to_bytes_be();
    if chiaro.len() > dimensione {
        return false;
    }
    let mut blocco = vec![0u8; dimensione - chiaro.len()];
    blocco.extend_from_slice(&chiaro);
    if blocco.len() < 11 || blocco[0] != 0x00 || blocco[1] != 0x01 {
        return false;
    }
    let digest = hash.digest(messaggio);
    let info = hash.digest_info();
    if blocco.len() < info.len() + digest.len() + 11 {
        return false;
    }
    let inizio_info = blocco.len() - info.len() - digest.len();
    if blocco[inizio_info - 1] != 0x00 {
        return false;
    }
    if !blocco[2..inizio_info - 1].iter().all(|byte| *byte == 0xff) {
        return false;
    }
    &blocco[inizio_info..inizio_info + info.len()] == info
        && &blocco[inizio_info + info.len()..] == digest.as_slice()
}

/// Verifica una firma ECDSA P-256/P-384 sui byte DER della risposta.
fn verifica_ecdsa(spki: &[u8], hash: HashKind, messaggio: &[u8], firma: &[u8]) -> bool {
    let digest = hash.digest(messaggio);
    if let Ok(chiave) = p256::ecdsa::VerifyingKey::from_public_key_der(spki) {
        if let Ok(signature) = p256::ecdsa::DerSignature::from_bytes(firma) {
            if chiave.verify_prehash(&digest, &signature).is_ok() {
                return true;
            }
        }
    }
    if let Ok(chiave) = p384::ecdsa::VerifyingKey::from_public_key_der(spki) {
        if let Ok(signature) = p384::ecdsa::DerSignature::from_bytes(firma) {
            if chiave.verify_prehash(&digest, &signature).is_ok() {
                return true;
            }
        }
    }
    false
}

/// SHA-1 (FIPS 180-4) usato per il `CertID` OCSP e per l'hash della chiave.
fn sha1(dati: &[u8]) -> [u8; 20] {
    let mut stato: [u32; 5] = [
        0x6745_2301,
        0xefcd_ab89,
        0x98ba_dcfe,
        0x1032_5476,
        0xc3d2_e1f0,
    ];
    let bit = (dati.len() as u64).wrapping_mul(8);
    let mut messaggio = dati.to_vec();
    messaggio.push(0x80);
    while messaggio.len() % 64 != 56 {
        messaggio.push(0);
    }
    messaggio.extend_from_slice(&bit.to_be_bytes());

    for blocco in messaggio.chunks_exact(64) {
        let mut parole = [0u32; 80];
        for indice in 0..16 {
            parole[indice] = u32::from_be_bytes([
                blocco[indice * 4],
                blocco[indice * 4 + 1],
                blocco[indice * 4 + 2],
                blocco[indice * 4 + 3],
            ]);
        }
        for indice in 16..80 {
            parole[indice] = (parole[indice - 3]
                ^ parole[indice - 8]
                ^ parole[indice - 14]
                ^ parole[indice - 16])
                .rotate_left(1);
        }
        let mut a = stato[0];
        let mut b = stato[1];
        let mut c = stato[2];
        let mut d = stato[3];
        let mut e = stato[4];
        for indice in 0..80 {
            let (f, k) = match indice {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let temporaneo = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(parole[indice]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temporaneo;
        }
        stato[0] = stato[0].wrapping_add(a);
        stato[1] = stato[1].wrapping_add(b);
        stato[2] = stato[2].wrapping_add(c);
        stato[3] = stato[3].wrapping_add(d);
        stato[4] = stato[4].wrapping_add(e);
    }

    let mut digest = [0u8; 20];
    for (indice, valore) in stato.iter().enumerate() {
        digest[indice * 4..indice * 4 + 4].copy_from_slice(&valore.to_be_bytes());
    }
    digest
}

#[cfg(test)]
mod test {
    use super::*;
    use ocsp::common::asn1::GeneralizedTime;
    use ocsp::oid::OCSP_RESPONSE_BASIC_DOT;
    use ocsp::response::{
        BasicResponse, CertStatus, CertStatusCode, OcspResponse, OneResp, ResponderId,
        ResponseBytes, ResponseData,
    };

    // Fixture generate con OpenSSL 3.6 (CA minima, scadenza 2036):
    //   openssl req -x509 -newkey rsa:2048 -nodes -keyout ca.key -out ca.pem \
    //       -subj "/CN=OpenPDF OCSP Test CA/O=OpenPDF/C=IT" -addext "basicConstraints=critical,CA:TRUE"
    //   openssl ca -batch -in leaf.csr -out leaf.pem   (AIA http://127.0.0.1:1)
    //   openssl ocsp -index demoCA/index.txt -CA ca.pem -rsigner ca.pem \
    //       -rkey ca.key -reqin req.der -respout resp-good.der -ndays 7
    // La variante revocata usa una riga "R" nell'indice con motivo keyCompromise.
    const CA: &[u8] = include_bytes!("../tests/fixtures/ocsp/ca.der");
    const FOGLIA: &[u8] = include_bytes!("../tests/fixtures/ocsp/leaf.der");
    const RISPOSTA_GOOD: &[u8] = include_bytes!("../tests/fixtures/ocsp/resp-good.der");
    const RISPOSTA_REVOKED: &[u8] = include_bytes!("../tests/fixtures/ocsp/resp-revoked.der");

    fn hex(dati: &[u8]) -> String {
        dati.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn sha1_vettori_noti() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex(&sha1(&vec![b'a'; 1000])),
            "291e9a6c66994949b57ba5e650361e98fc36b1ba"
        );
    }

    #[test]
    fn richiesta_contiene_certid_sha1() {
        let richiesta = costruisci_richiesta(FOGLIA, CA, None).expect("richiesta OCSP");
        let decodificata =
            ocsp::request::OcspRequest::parse(&richiesta).expect("richiesta decodificabile");
        let certid = decodificata
            .extract_certid()
            .first()
            .cloned()
            .expect("CertID presente");

        let foglia = analizza_certificato(FOGLIA).expect("certificato foglia");
        let ca = analizza_certificato(CA).expect("certificato CA");
        assert_eq!(certid.serial_num, foglia.seriale);
        assert_eq!(certid.issuer_name_hash, sha1(&ca.soggetto).to_vec());
        assert_eq!(certid.issuer_key_hash, sha1(&ca.chiave).to_vec());
        assert_eq!(certid.issuer_key_hash.len(), 20);
        // Valori attesi della CA di test, come stampati da OpenSSL.
        assert_eq!(
            hex(&certid.issuer_name_hash),
            "171fe9e57ac3742f096c4b531192a9c78f1109ac"
        );
        assert_eq!(
            hex(&certid.issuer_key_hash),
            "39234456c0f697c69f8c726297804277455a7d5e"
        );
    }

    #[test]
    fn richiesta_con_nonce() {
        let nonce = [0x11u8, 0x22, 0x33, 0x44];
        let richiesta =
            costruisci_richiesta(FOGLIA, CA, Some(&nonce)).expect("richiesta con nonce");
        let decodificata =
            ocsp::request::OcspRequest::parse(&richiesta).expect("richiesta decodificabile");
        let estensioni = decodificata.extract_ext().expect("estensioni presenti");
        assert_eq!(estensioni.len(), 1);
        match &estensioni[0].ext {
            OcspExt::Nonce { nonce: letto } => {
                // extnValue contiene il DER dell'OCTET STRING del nonce.
                assert_eq!(letto, &tlv(0x04, &nonce));
            }
            altro => panic!("estensione inattesa: {altro:?}"),
        }
    }

    #[test]
    fn richiesta_troppo_lunga_rifiutata() {
        let errore = costruisci_richiesta(FOGLIA, CA, Some(&[0u8; NONCE_MAX + 1]))
            .expect_err("nonce oltre il limite");
        assert!(errore.contains("nonce"));
    }

    #[test]
    fn aia_letta_dal_certificato() {
        let foglia = analizza_certificato(FOGLIA).expect("certificato foglia");
        assert_eq!(foglia.aia.as_deref(), Some("http://127.0.0.1:1"));
        let ca = analizza_certificato(CA).expect("certificato CA");
        assert!(ca.aia.is_none());
    }

    #[test]
    fn check_responder_irraggiungibile() {
        // La porta 1 di loopback rifiuta la connessione: nessun accesso a Internet.
        let errore = check(FOGLIA, CA, Some("http://127.0.0.1:1"))
            .expect_err("il responder non è raggiungibile");
        assert!(errore.contains("OCSP"), "messaggio inatteso: {errore}");
        assert!(
            errore.contains("irraggiungibile")
                || errore.contains("curl")
                || errore.contains("connessione"),
            "messaggio inatteso: {errore}"
        );
    }

    #[test]
    fn check_url_non_http() {
        let errore =
            check(FOGLIA, CA, Some("ftp://esempio.invalid")).expect_err("schema non supportato");
        assert!(errore.contains("URL"), "messaggio inatteso: {errore}");
    }

    #[test]
    fn parse_response_byte_casuali() {
        let casuali: Vec<u8> = (0..64u8)
            .map(|indice| indice.wrapping_mul(37) ^ 0xa5)
            .collect();
        let errore = parse_response(&casuali, FOGLIA, CA).expect_err("risposta non valida");
        assert!(
            errore.contains("OCSP") || errore.contains("ASN.1"),
            "messaggio inatteso: {errore}"
        );

        let vuota = parse_response(&[], FOGLIA, CA).expect_err("risposta vuota");
        assert!(vuota.contains("OCSP"), "messaggio inatteso: {vuota}");
    }

    #[test]
    fn check_usa_aia_se_url_assente() {
        // L'AIA della foglia punta a loopback:1, che rifiuta la connessione.
        let errore = check(FOGLIA, CA, None).expect_err("nessun url esplicito");
        assert!(
            errore.contains("127.0.0.1:1"),
            "l'URL AIA non è stato usato: {errore}"
        );
    }

    /// Risponde a una sola richiesta HTTP con la risposta OCSP indicata.
    fn avvia_responder(risposta: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
        use std::io::Read;
        use std::net::TcpListener;

        let ascoltatore = TcpListener::bind("127.0.0.1:0").expect("porta locale disponibile");
        let indirizzo = ascoltatore.local_addr().expect("indirizzo locale");
        let gestore = std::thread::spawn(move || {
            let (mut flusso, _) = ascoltatore.accept().expect("connessione in arrivo");
            let mut dati = Vec::new();
            let mut blocco = [0u8; 1024];
            let mut attesi = 0usize;
            loop {
                let letti = flusso.read(&mut blocco).expect("lettura richiesta");
                if letti == 0 {
                    break;
                }
                dati.extend_from_slice(&blocco[..letti]);
                if attesi == 0 {
                    if let Some(posizione) =
                        dati.windows(4).position(|finestra| finestra == b"\r\n\r\n")
                    {
                        let intestazioni =
                            String::from_utf8_lossy(&dati[..posizione + 4]).to_ascii_lowercase();
                        let lunghezza = intestazioni
                            .lines()
                            .find_map(|riga| {
                                riga.strip_prefix("content-length:")
                                    .map(|valore| valore.trim().parse::<usize>().unwrap_or(0))
                            })
                            .unwrap_or(0);
                        attesi = posizione + 4 + lunghezza;
                    }
                }
                if attesi > 0 && dati.len() >= attesi {
                    break;
                }
            }
            let intestazione = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/ocsp-response\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                risposta.len()
            );
            flusso
                .write_all(intestazione.as_bytes())
                .expect("intestazioni risposta");
            flusso.write_all(&risposta).expect("corpo risposta");
            let _ = flusso.flush();
        });
        (format!("http://{indirizzo}"), gestore)
    }

    #[test]
    fn check_end_to_end_su_loopback() {
        let (indirizzo, gestore) = avvia_responder(RISPOSTA_GOOD.to_vec());
        let (esito, risposta) =
            check_with_response(FOGLIA, CA, Some(&indirizzo)).expect("controllo OCSP locale");
        assert_eq!(esito.status, RevocationStatus::Good);
        assert!(esito.signature_ok, "firma non verificata via HTTP");
        assert_eq!(risposta, RISPOSTA_GOOD, "risposta non conservata in DER");
        gestore.join().expect("responder terminato");
    }

    #[test]
    fn parse_response_senza_certificati_usa_emittente() {
        // Ricostruisce la stessa risposta eliminando il campo certs: la firma
        // deve essere verificata con l'emittente indicato dal ResponderID.
        let (risposta, _) = leggi_tlv(RISPOSTA_GOOD).expect("OCSPResponse");
        let parti = campi(risposta.valore).expect("campi OCSPResponse");
        let (response_bytes, _) = leggi_tlv(parti[1].valore).expect("responseBytes");
        let campi_bytes = campi(response_bytes.valore).expect("campi responseBytes");
        let (basic, _) = leggi_tlv(campi_bytes[1].valore).expect("BasicOCSPResponse");
        let campi_basic = campi(basic.valore).expect("campi BasicOCSPResponse");

        let mut contenuto_basic = Vec::new();
        for elemento in campi_basic.iter().take(3) {
            contenuto_basic.extend_from_slice(elemento.grezzo);
        }
        let mut contenuto_bytes = Vec::new();
        contenuto_bytes.extend_from_slice(campi_bytes[0].grezzo);
        contenuto_bytes.extend_from_slice(&tlv(0x04, &tlv(0x30, &contenuto_basic)));
        let mut contenuto_risposta = Vec::new();
        contenuto_risposta.extend_from_slice(parti[0].grezzo);
        contenuto_risposta.extend_from_slice(&tlv(0xa0, &tlv(0x30, &contenuto_bytes)));
        let ridotta = tlv(0x30, &contenuto_risposta);

        let esito = parse_response(&ridotta, FOGLIA, CA).expect("risposta senza certs");
        assert_eq!(esito.status, RevocationStatus::Good);
        assert!(esito.signature_ok, "firma non verificata con l'emittente");
    }

    #[test]
    fn parse_response_buona() {
        let esito = parse_response(RISPOSTA_GOOD, FOGLIA, CA).expect("risposta good");
        assert_eq!(esito.status, RevocationStatus::Good);
        assert!(esito.this_update.contains("2026"), "{}", esito.this_update);
        let next = esito.next_update.expect("nextUpdate presente");
        assert!(next.contains("2026"), "{next}");
        assert_eq!(
            esito.responder.as_deref(),
            Some("responder OCSP: OpenPDF OCSP Test CA")
        );
        assert!(esito.signature_ok, "firma della risposta non verificata");
    }

    #[test]
    fn parse_response_revocata() {
        let esito = parse_response(RISPOSTA_REVOKED, FOGLIA, CA).expect("risposta revoked");
        match esito.status {
            RevocationStatus::Revoked { time, reason } => {
                assert_eq!(time, "2026-01-01 00:00:00 UTC");
                assert!(reason.contains("compromissione"), "motivo: {reason}");
            }
            altro => panic!("stato inatteso: {altro:?}"),
        }
        assert!(esito.signature_ok, "firma della risposta non verificata");
    }

    #[test]
    fn parse_response_firma_alterata() {
        let mut alterata = RISPOSTA_GOOD.to_vec();
        // Individua la firma navigando la stessa struttura del parser.
        let (risposta, _) = leggi_tlv(&alterata).expect("OCSPResponse");
        let parti = campi(risposta.valore).expect("campi OCSPResponse");
        let (response_bytes, _) = leggi_tlv(parti[1].valore).expect("responseBytes");
        let campi_bytes = campi(response_bytes.valore).expect("campi responseBytes");
        let (basic, _) = leggi_tlv(campi_bytes[1].valore).expect("BasicOCSPResponse");
        let campi_basic = campi(basic.valore).expect("campi BasicOCSPResponse");
        let firma = &campi_basic[2];
        let inizio = firma.valore.as_ptr() as usize - alterata.as_ptr() as usize;
        // `valore` inizia con il conteggio dei bit non usati: si altera un byte
        // della firma vera e propria, senza toccare la struttura ASN.1.
        alterata[inizio + 1] ^= 0x01;

        let esito = parse_response(&alterata, FOGLIA, CA).expect("struttura ancora valida");
        assert_eq!(esito.status, RevocationStatus::Good);
        assert!(!esito.signature_ok, "la firma alterata è stata accettata");
    }

    #[test]
    fn parse_response_certid_assente() {
        // Risposta sintetica per un seriale diverso: il CertID non deve
        // combaciare e la firma (vuota) non viene nemmeno considerata.
        let oid_sha1 = Oid::new_from_dot(ALGO_SHA1_DOT).expect("OID SHA-1");
        let certid = CertId::new(oid_sha1, &[0u8; 20], &[0u8; 20], &[0x7f, 0x01]);
        let prodotta = GeneralizedTime::new(2026, 1, 1, 0, 0, 0).expect("data valida");
        let singola = OneResp {
            cid: certid,
            cert_status: CertStatus::new(CertStatusCode::Good, None),
            this_update: prodotta.clone(),
            next_update: None,
            one_resp_ext: None,
        };
        let dati = ResponseData::new(
            ResponderId::new_key_hash(&[0u8; 20]),
            prodotta,
            vec![singola],
            None,
        );
        let algoritmo = Oid::new_from_dot("1.2.840.113549.1.1.11").expect("OID SHA-256 con RSA");
        let basic = BasicResponse::new(dati, algoritmo, vec![0u8; 256], None);
        let tipo = Oid::new_from_dot(OCSP_RESPONSE_BASIC_DOT).expect("OID basic");
        let bytes = ResponseBytes::new_basic(tipo, basic).expect("responseBytes");
        let risposta = OcspResponse::new_success(bytes)
            .to_der()
            .expect("risposta DER");

        let errore = parse_response(&risposta, FOGLIA, CA).expect_err("CertID diverso");
        assert!(errore.contains("CertID"), "messaggio inatteso: {errore}");
    }

    #[test]
    fn parse_response_stato_unknown() {
        // Risposta sintetica con stato unknown (0x82): la firma non è valida
        // ma lo stato deve essere interpretato correttamente.
        let foglia = analizza_certificato(FOGLIA).expect("certificato foglia");
        let ca = analizza_certificato(CA).expect("certificato CA");
        let oid_sha1 = Oid::new_from_dot(ALGO_SHA1_DOT).expect("OID SHA-1");
        let certid = CertId::new(
            oid_sha1,
            &sha1(&ca.soggetto),
            &sha1(&ca.chiave),
            &foglia.seriale,
        );
        let prodotta = GeneralizedTime::new(2026, 1, 1, 0, 0, 0).expect("data valida");
        let singola = OneResp {
            cid: certid,
            cert_status: CertStatus::new(CertStatusCode::Unknown, None),
            this_update: prodotta.clone(),
            next_update: None,
            one_resp_ext: None,
        };
        let dati = ResponseData::new(
            ResponderId::new_key_hash(&sha1(&ca.chiave)),
            prodotta,
            vec![singola],
            None,
        );
        let algoritmo = Oid::new_from_dot("1.2.840.113549.1.1.11").expect("OID SHA-256 con RSA");
        let basic = BasicResponse::new(dati, algoritmo, vec![0u8; 256], None);
        let tipo = Oid::new_from_dot(OCSP_RESPONSE_BASIC_DOT).expect("OID basic");
        let bytes = ResponseBytes::new_basic(tipo, basic).expect("responseBytes");
        let risposta = OcspResponse::new_success(bytes)
            .to_der()
            .expect("risposta DER");

        let esito = parse_response(&risposta, FOGLIA, CA).expect("risposta unknown");
        assert_eq!(esito.status, RevocationStatus::Unknown);
        assert!(!esito.signature_ok);
    }

    #[test]
    fn parse_response_troncata() {
        let tagliata = &RISPOSTA_GOOD[..RISPOSTA_GOOD.len() / 2];
        let errore = parse_response(tagliata, FOGLIA, CA).expect_err("risposta troncata");
        assert!(errore.contains("OCSP"), "messaggio inatteso: {errore}");
    }

    #[test]
    fn tempo_generalized() {
        assert_eq!(
            formatta_tempo(b"20261115000649Z"),
            "2026-11-15 00:06:49 UTC"
        );
        assert_eq!(
            formatta_tempo(b"20261115000649.123Z"),
            "2026-11-15 00:06:49 UTC"
        );
        assert_eq!(formatta_tempo(b"non una data"), "non una data");
    }

    #[test]
    fn seriale_confrontato_senza_zeri() {
        assert!(stesso_seriale(&[0x00, 0x10, 0x00], &[0x10, 0x00]));
        assert!(stesso_seriale(&[0x10, 0x00], &[0x10, 0x00]));
        assert!(!stesso_seriale(&[0x10, 0x01], &[0x10, 0x00]));
        assert!(!stesso_seriale(&[], &[0x00]));
    }
}
