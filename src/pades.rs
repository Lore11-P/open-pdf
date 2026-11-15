//! Firma digitale PAdES Baseline B-B (ETSI EN 319 142-1).
//!
//! Produce una firma `ETSI.CAdES.detached` con:
//! - ByteRange calcolato sul file reale e campo `/Contents` a dimensione fissa;
//! - CMS SignedData con attributi firmati `contentType`, `messageDigest`,
//!   `signingTime` e `signingCertificateV2` (ESS, obbligatorio per PAdES);
//! - certificato del firmatario incorporato, catena inclusa;
//! - digest SHA-256 per RSA e P-256, SHA-384 per P-384;
//! - apparenza visiva del widget (`/AP`) con nome, data, motivo e luogo,
//!   disattivabile con `SignOptions::visible = false`;
//! - firma di certificazione DocMDP opzionale (`/Reference` con `TransformMethod /DocMDP`);
//! - marca temporale RFC 3161 (`signatureTimeStampToken`) se `SignOptions::tsa_url`
//!   è valorizzato: la firma diventa PAdES Baseline B-T.
//!
//! La verifica (`verify_pdf_con`) controlla digest, firma CMS, catena di
//! certificati, attributi firmati, marca temporale ed eventualmente la revoca
//! OCSP. Non è una firma qualificata eIDAS: non c'è un trust store, né la
//! consultazione di un elenco di CA accreditate.
//!
//! Credenziali supportate:
//! - certificato PEM + chiave PEM RSA (PKCS#8 o PKCS#1) ed EC P-256/P-384
//!   (PKCS#8 o SEC1), non cifrata;
//! - archivio PKCS#12 / `.pfx` protetto da password, con MAC verificato e
//!   chiave PKCS#8 decifrata dal crate `keystore-pkcs12`.
//!
//! Limiti dichiarati:
//! - il PDF viene riscritto per intero: eventuali firme precedenti non restano
//!   valide (una firma successiva richiede un aggiornamento incrementale);
//! - i PDF cifrati non sono supportati;
//! - chiavi RSA-PSS e chiavi private PEM cifrate non sono supportate;
//! - gli archivi PKCS#12 con `authSafe` firmato (signedData) non sono supportati.

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use keystore_pkcs12::{CertType, MacParams, PbeScheme, SafeBag};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat};
use rasn::prelude::*;
use rasn_cms::{
    CertificateChoices, CertificateSet, ContentInfo, DigestAlgorithmIdentifiers,
    EncapsulatedContentInfo, IssuerAndSerialNumber, SignedData, SignerIdentifier, SignerInfo,
    SignerInfos,
};
use rasn_pkix::{AlgorithmIdentifier, Attribute, Certificate};
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::{
    Signature as RsaSignature, SigningKey as RsaSigningKey, VerifyingKey as RsaVerifyingKey,
};
use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey};
use rsa::signature::hazmat::PrehashVerifier;
use rsa::signature::{SignatureEncoding, Signer, Verifier};
use rsa::{RsaPrivateKey, RsaPublicKey};
use sha2::{Digest, Sha256, Sha384};
use std::path::{Path, PathBuf};

const PLACEHOLDER_LEN: usize = 16384;
const TENTATIVI_SPAZIO: usize = 3;
const MARKER: &[u8] = b"OPENPDF-SIGNATURE-SLOT";
const SUBFILTER: &str = "ETSI.CAdES.detached";

#[derive(Debug, Clone)]
pub struct SignatureInfo {
    pub field: String,
    pub signer: String,
    pub time: String,
    pub subfilter: String,
    pub covers_all: bool,
    pub digest_ok: bool,
    pub signature_ok: bool,
    /// Catena di certificati con firme e date coerenti.
    pub chain_valid: Option<bool>,
    /// La catena termina su un certificato autofirmato.
    pub chain_complete: Option<bool>,
    /// Radice autofirmata e catena valida: **non** consulta lo store di Windows.
    pub trusted: Option<bool>,
    /// Problemi della catena, già in italiano.
    pub chain_errors: Vec<String>,
    /// Problemi degli attributi firmati CMS (messageDigest, contentType, ESS).
    pub attribute_errors: Vec<String>,
    /// Nomi dei certificati in ordine foglia → radice.
    pub chain: Vec<String>,
    /// Inizio validità del certificato del firmatario.
    pub valid_from: Option<String>,
    /// Fine validità del certificato del firmatario.
    pub valid_to: Option<String>,
    /// Livello DocMDP dichiarato nel dizionario `/Reference` (1, 2 o 3).
    pub docmdp: Option<u8>,
    /// Data della marca temporale RFC 3161 verificata.
    pub timestamp: Option<String>,
    /// Esito del controllo di revoca OCSP, se richiesto.
    pub revocation: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SignOutcome {
    pub field: String,
    pub signer: String,
    pub size: u64,
    pub path: String,
}

/// Credenziali del firmatario: coppia PEM oppure archivio PKCS#12.
#[derive(Clone)]
pub enum Credenziali {
    /// Certificato X.509 in PEM (può contenere la catena) e chiave privata PEM.
    Pem { cert: PathBuf, key: PathBuf },
    /// Archivio PKCS#12 / `.pfx` con la password di apertura.
    P12 { path: PathBuf, password: String },
}

impl std::fmt::Debug for Credenziali {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credenziali::Pem { cert, key } => f
                .debug_struct("Pem")
                .field("cert", cert)
                .field("key", key)
                .finish(),
            Credenziali::P12 { path, .. } => f
                .debug_struct("P12")
                .field("path", path)
                .field("password", &"<redatta>")
                .finish(),
        }
    }
}

impl Credenziali {
    /// Coppia PEM: certificato (eventualmente con catena) e chiave privata.
    pub fn pem(cert: impl Into<PathBuf>, key: impl Into<PathBuf>) -> Self {
        Credenziali::Pem {
            cert: cert.into(),
            key: key.into(),
        }
    }

    /// Archivio PKCS#12 protetto da password.
    pub fn p12(path: impl Into<PathBuf>, password: impl Into<String>) -> Self {
        Credenziali::P12 {
            path: path.into(),
            password: password.into(),
        }
    }
}

/// Parametri della firma, indipendenti dalle credenziali.
#[derive(Clone)]
pub struct SignOptions {
    /// Pagina su cui inserire il campo firma (indice a base zero).
    pub page_index: usize,
    /// Rettangolo normalizzato `[x, y, larghezza, altezza]` (0..1, origine in alto a sinistra).
    pub rect: [f32; 4],
    pub name: String,
    pub reason: String,
    pub location: String,
    /// Password di apertura se il PDF da firmare è cifrato.
    pub pdf_password: Option<String>,
    /// Se presente, firma di certificazione DocMDP con livello 1, 2 o 3.
    pub certification: Option<u8>,
    /// Se `false` il campo firma non riceve l'apparenza visiva (`/AP`).
    pub visible: bool,
    /// URL del servizio di marca temporale RFC 3161 (PAdES B-T).
    pub tsa_url: Option<String>,
}

impl std::fmt::Debug for SignOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignOptions")
            .field("page_index", &self.page_index)
            .field("rect", &self.rect)
            .field("name", &self.name)
            .field("reason", &self.reason)
            .field("location", &self.location)
            .field(
                "pdf_password",
                &self.pdf_password.as_ref().map(|_| "<redatta>"),
            )
            .field("certification", &self.certification)
            .field("visible", &self.visible)
            .field("tsa_url", &self.tsa_url)
            .finish()
    }
}

impl Default for SignOptions {
    fn default() -> Self {
        SignOptions {
            page_index: 0,
            rect: [0.06, 0.85, 0.34, 0.09],
            name: String::new(),
            reason: String::new(),
            location: String::new(),
            pdf_password: None,
            certification: None,
            visible: true,
            tsa_url: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HashKind {
    Sha256,
    Sha384,
}

impl HashKind {
    fn oid(self) -> ObjectIdentifier {
        match self {
            HashKind::Sha256 => oid(&[2, 16, 840, 1, 101, 3, 4, 2, 1]),
            HashKind::Sha384 => oid(&[2, 16, 840, 1, 101, 3, 4, 2, 2]),
        }
    }

    fn algorithm(self) -> AlgorithmIdentifier {
        AlgorithmIdentifier {
            algorithm: self.oid(),
            parameters: Some(Any::new(vec![0x05, 0x00])),
        }
    }

    fn digest(self, parti: &[&[u8]]) -> Vec<u8> {
        match self {
            HashKind::Sha256 => {
                let mut digest = Sha256::new();
                for parte in parti {
                    digest.update(parte);
                }
                digest.finalize().to_vec()
            }
            HashKind::Sha384 => {
                let mut digest = Sha384::new();
                for parte in parti {
                    digest.update(parte);
                }
                digest.finalize().to_vec()
            }
        }
    }

    fn from_oid(id: &ObjectIdentifier) -> Option<Self> {
        if *id == oid(&[2, 16, 840, 1, 101, 3, 4, 2, 1]) {
            Some(HashKind::Sha256)
        } else if *id == oid(&[2, 16, 840, 1, 101, 3, 4, 2, 2]) {
            Some(HashKind::Sha384)
        } else {
            None
        }
    }
}

/// Chiave privata utilizzabile per la firma CMS.
enum Chiave {
    Rsa(RsaPrivateKey),
    P256(p256::ecdsa::SigningKey),
    P384(p384::ecdsa::SigningKey),
}

impl Chiave {
    /// Riconosce il tipo di chiave da un PKCS#8 `PrivateKeyInfo` DER.
    fn da_pkcs8(der: &[u8]) -> Result<Self, String> {
        if let Ok(chiave) = RsaPrivateKey::from_pkcs8_der(der) {
            return Ok(Chiave::Rsa(chiave));
        }
        if let Ok(chiave) = p256::ecdsa::SigningKey::from_pkcs8_der(der) {
            return Ok(Chiave::P256(chiave));
        }
        if let Ok(chiave) = p384::ecdsa::SigningKey::from_pkcs8_der(der) {
            return Ok(Chiave::P384(chiave));
        }
        Err("Chiave privata non supportata: sono accettate chiavi RSA ed EC P-256/P-384.".into())
    }

    fn algoritmo_digest(&self) -> HashKind {
        match self {
            Chiave::Rsa(_) | Chiave::P256(_) => HashKind::Sha256,
            Chiave::P384(_) => HashKind::Sha384,
        }
    }

    fn algoritmo_firma(&self) -> AlgorithmIdentifier {
        match self {
            Chiave::Rsa(_) => rsa_algorithm(),
            Chiave::P256(_) => AlgorithmIdentifier {
                algorithm: oid(&[1, 2, 840, 10045, 4, 3, 2]),
                parameters: None,
            },
            Chiave::P384(_) => AlgorithmIdentifier {
                algorithm: oid(&[1, 2, 840, 10045, 4, 3, 3]),
                parameters: None,
            },
        }
    }

    fn descrizione(&self) -> &'static str {
        match self {
            Chiave::Rsa(_) => "RSA",
            Chiave::P256(_) => "EC P-256",
            Chiave::P384(_) => "EC P-384",
        }
    }

    /// Firma il messaggio: PKCS#1 v1.5 per RSA, ECDSA deterministico (RFC 6979)
    /// con firma in codifica DER per le curve.
    fn firma(&self, messaggio: &[u8]) -> Result<Vec<u8>, String> {
        match self {
            Chiave::Rsa(chiave) => {
                let firmatore = RsaSigningKey::<Sha256>::new(chiave.clone());
                let firma: RsaSignature = firmatore.sign(messaggio);
                Ok(firma.to_vec())
            }
            Chiave::P256(chiave) => {
                let firma: p256::ecdsa::DerSignature = chiave.sign(messaggio);
                Ok(firma.as_bytes().to_vec())
            }
            Chiave::P384(chiave) => {
                let firma: p384::ecdsa::DerSignature = chiave.sign(messaggio);
                Ok(firma.as_bytes().to_vec())
            }
        }
    }
}

/// Materiale pronto per costruire il CMS.
struct MaterialeFirma {
    chiave: Chiave,
    foglia: Certificate,
    /// Catena DER, con il certificato foglia in prima posizione.
    catena: Vec<Vec<u8>>,
}

fn der_tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len() + 6);
    out.push(tag);
    let len = content.len();
    if len < 128 {
        out.push(len as u8);
    } else {
        let mut bytes = Vec::new();
        let mut value = len;
        while value > 0 {
            bytes.push((value & 0xFF) as u8);
            value >>= 8;
        }
        bytes.reverse();
        out.push(0x80 | bytes.len() as u8);
        out.extend_from_slice(&bytes);
    }
    out.extend_from_slice(content);
    out
}

fn oid(arcs: &[u32]) -> ObjectIdentifier {
    ObjectIdentifier::new(arcs.to_vec()).expect("OID valido")
}

fn oid_content_type() -> ObjectIdentifier {
    oid(&[1, 2, 840, 113549, 1, 9, 3])
}

fn oid_message_digest() -> ObjectIdentifier {
    oid(&[1, 2, 840, 113549, 1, 9, 4])
}

fn oid_signing_time() -> ObjectIdentifier {
    oid(&[1, 2, 840, 113549, 1, 9, 5])
}

fn oid_data() -> ObjectIdentifier {
    oid(&[1, 2, 840, 113549, 1, 7, 1])
}

fn oid_signed_data() -> ObjectIdentifier {
    oid(&[1, 2, 840, 113549, 1, 7, 2])
}

fn oid_rsa_encryption() -> ObjectIdentifier {
    oid(&[1, 2, 840, 113549, 1, 1, 1])
}

fn oid_ec_public_key() -> ObjectIdentifier {
    oid(&[1, 2, 840, 10045, 2, 1])
}

fn rsa_algorithm() -> AlgorithmIdentifier {
    AlgorithmIdentifier {
        algorithm: oid_rsa_encryption(),
        parameters: Some(Any::new(vec![0x05, 0x00])),
    }
}

fn pem_blocchi(pem: &str, etichetta: &str) -> Result<Vec<Vec<u8>>, String> {
    let inizio = format!("-----BEGIN {etichetta}-----");
    let fine = format!("-----END {etichetta}-----");
    let mut blocchi = Vec::new();
    let mut corrente: Option<String> = None;
    for riga in pem.lines() {
        let riga = riga.trim();
        if riga.starts_with(&inizio) {
            corrente = Some(String::new());
        } else if riga.starts_with(&fine) {
            if let Some(corpo) = corrente.take() {
                let der = B64
                    .decode(corpo.trim())
                    .map_err(|e| format!("PEM non valido: {e}"))?;
                blocchi.push(der);
            }
        } else if let Some(corpo) = corrente.as_mut() {
            corpo.push_str(riga);
        }
    }
    if blocchi.is_empty() {
        Err(format!(
            "Nessun blocco PEM «{etichetta}» trovato nel file indicato."
        ))
    } else {
        Ok(blocchi)
    }
}

fn chiave_da_pem(pem: &str) -> Result<Chiave, String> {
    if pem.contains("ENCRYPTED PRIVATE KEY") {
        return Err(
            "La chiave privata PEM è cifrata: fornisci la chiave non cifrata oppure usa un archivio PKCS#12."
                .into(),
        );
    }
    if let Ok(chiave) = RsaPrivateKey::from_pkcs8_pem(pem) {
        return Ok(Chiave::Rsa(chiave));
    }
    if let Ok(chiave) = p256::ecdsa::SigningKey::from_pkcs8_pem(pem) {
        return Ok(Chiave::P256(chiave));
    }
    if let Ok(chiave) = p384::ecdsa::SigningKey::from_pkcs8_pem(pem) {
        return Ok(Chiave::P384(chiave));
    }
    if let Ok(chiave) = RsaPrivateKey::from_pkcs1_pem(pem) {
        return Ok(Chiave::Rsa(chiave));
    }
    if let Ok(chiave) = p256::SecretKey::from_sec1_pem(pem) {
        return Ok(Chiave::P256(chiave.into()));
    }
    if let Ok(chiave) = p384::SecretKey::from_sec1_pem(pem) {
        return Ok(Chiave::P384(chiave.into()));
    }
    Err(
        "Chiave privata non riconosciuta: sono supportate chiavi RSA (PKCS#8 o PKCS#1) ed EC P-256/P-384 (PKCS#8 o SEC1) in PEM."
            .into(),
    )
}

/// Verifica che la chiave pubblica del certificato corrisponda alla chiave privata.
fn chiave_corrisponde(cert: &Certificate, chiave: &Chiave) -> bool {
    let Ok(spki) = rasn::der::encode(&cert.tbs_certificate.subject_public_key_info) else {
        return false;
    };
    match chiave {
        Chiave::Rsa(privata) => RsaPublicKey::from_public_key_der(&spki)
            .map(|pubblica| pubblica == privata.to_public_key())
            .unwrap_or(false),
        Chiave::P256(privata) => p256::ecdsa::VerifyingKey::from_public_key_der(&spki)
            .map(|pubblica| {
                pubblica.to_encoded_point(false).as_bytes()
                    == privata.verifying_key().to_encoded_point(false).as_bytes()
            })
            .unwrap_or(false),
        Chiave::P384(privata) => p384::ecdsa::VerifyingKey::from_public_key_der(&spki)
            .map(|pubblica| {
                pubblica.to_encoded_point(false).as_bytes()
                    == privata.verifying_key().to_encoded_point(false).as_bytes()
            })
            .unwrap_or(false),
    }
}

/// Ordina la catena partendo dalla foglia e seguendo issuer/subject.
fn ordina_catena(foglia: Vec<u8>, disponibili: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut catena = vec![foglia];
    loop {
        let Some(ultimo) = catena.last() else {
            break;
        };
        let Ok(cert) = rasn::der::decode::<Certificate>(ultimo) else {
            break;
        };
        let issuer = cert.tbs_certificate.issuer.clone();
        if issuer == cert.tbs_certificate.subject {
            break;
        }
        let prossimo = disponibili.iter().find(|der| {
            !catena.iter().any(|presente| presente == *der)
                && rasn::der::decode::<Certificate>(der)
                    .map(|candidato| candidato.tbs_certificate.subject == issuer)
                    .unwrap_or(false)
        });
        match prossimo {
            Some(der) => catena.push(der.clone()),
            None => break,
        }
    }
    catena
}

fn materiale_da_pem(cert_path: &Path, key_path: &Path) -> Result<MaterialeFirma, String> {
    let cert_pem = std::fs::read_to_string(cert_path)
        .map_err(|e| format!("Certificato non leggibile: {e}"))?;
    let key_pem = std::fs::read_to_string(key_path)
        .map_err(|e| format!("Chiave privata non leggibile: {e}"))?;
    let certificati = pem_blocchi(&cert_pem, "CERTIFICATE")?;
    let chiave = chiave_da_pem(&key_pem)?;

    let mut foglia: Option<(usize, Certificate)> = None;
    for (indice, der) in certificati.iter().enumerate() {
        if let Ok(cert) = rasn::der::decode::<Certificate>(der) {
            if chiave_corrisponde(&cert, &chiave) {
                foglia = Some((indice, cert));
                break;
            }
        }
    }
    let Some((indice, foglia)) = foglia else {
        return Err(format!(
            "Il certificato non corrisponde alla chiave privata {}: verifica di aver scelto la coppia corretta.",
            chiave.descrizione()
        ));
    };
    let altri: Vec<Vec<u8>> = certificati
        .iter()
        .enumerate()
        .filter(|(posizione, _)| *posizione != indice)
        .map(|(_, der)| der.clone())
        .collect();
    let catena = ordina_catena(certificati[indice].clone(), altri);
    Ok(MaterialeFirma {
        chiave,
        foglia,
        catena,
    })
}

// -- Lettura del contenitore PKCS#12 -----------------------------------------
//
// Il crate `keystore-pkcs12` espone `SafeBag`, `PbeScheme` e `MacParams`, ma il
// tipo `Password` vive in `keystore-core`, che non è una dipendenza diretta del
// progetto. Il contenitore PFX viene quindi aperto qui con un piccolo lettore
// BER e i singoli bag vengono decifrati dal crate.

const OID_DATA_CONTENT: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x01];
const OID_ENCRYPTED_DATA_CONTENT: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x06];

/// Un elemento ASN.1 BER: tag, contenuto e codifica completa.
struct Tlv<'a> {
    tag: u8,
    content: &'a [u8],
    raw: &'a [u8],
}

fn leggi_tlv(ber: &[u8]) -> Result<Tlv<'_>, String> {
    if ber.len() < 2 {
        return Err("Struttura ASN.1 troncata.".into());
    }
    let tag = ber[0];
    if tag & 0x1f == 0x1f {
        return Err("Tag ASN.1 esteso non supportato nell'archivio PKCS#12.".into());
    }
    let primo = ber[1];
    if primo == 0x80 {
        if tag & 0x20 == 0 {
            return Err("Lunghezza ASN.1 indefinita su un elemento primitivo.".into());
        }
        let mut posizione = 2usize;
        while posizione + 2 <= ber.len() {
            if ber[posizione] == 0 && ber[posizione + 1] == 0 {
                return Ok(Tlv {
                    tag,
                    content: &ber[2..posizione],
                    raw: &ber[..posizione + 2],
                });
            }
            let figlio = leggi_tlv(&ber[posizione..])?;
            posizione += figlio.raw.len();
        }
        Err("Lunghezza ASN.1 indefinita senza terminatore.".into())
    } else if primo & 0x80 == 0 {
        let lunghezza = primo as usize;
        let fine = 2 + lunghezza;
        if fine > ber.len() {
            return Err("Struttura ASN.1 troncata.".into());
        }
        Ok(Tlv {
            tag,
            content: &ber[2..fine],
            raw: &ber[..fine],
        })
    } else {
        let cifre = (primo & 0x7f) as usize;
        if cifre == 0 || cifre > 4 || ber.len() < 2 + cifre {
            return Err("Campo di lunghezza ASN.1 non valido.".into());
        }
        let mut lunghezza = 0usize;
        for indice in 0..cifre {
            lunghezza = (lunghezza << 8) | ber[2 + indice] as usize;
        }
        let inizio = 2 + cifre;
        let fine = inizio + lunghezza;
        if fine > ber.len() {
            return Err("Struttura ASN.1 troncata.".into());
        }
        Ok(Tlv {
            tag,
            content: &ber[inizio..fine],
            raw: &ber[..fine],
        })
    }
}

fn tlv_figli<'a>(contenuto: &'a [u8]) -> Result<Vec<Tlv<'a>>, String> {
    let mut figli = Vec::new();
    let mut posizione = 0usize;
    while posizione < contenuto.len() {
        let elemento = leggi_tlv(&contenuto[posizione..])?;
        posizione += elemento.raw.len();
        figli.push(elemento);
    }
    Ok(figli)
}

fn tlv_intero(elemento: &Tlv<'_>) -> Result<u32, String> {
    if elemento.tag != 0x02 || elemento.content.is_empty() || elemento.content.len() > 4 {
        return Err("Valore INTEGER ASN.1 non valido nell'archivio PKCS#12.".into());
    }
    let mut valore = 0u32;
    for byte in elemento.content {
        valore = (valore << 8) | *byte as u32;
    }
    Ok(valore)
}

/// Contenuto di un OCTET STRING, anche costruito o implicito (`[0]`).
fn tlv_ottetti(elemento: &Tlv<'_>) -> Result<Vec<u8>, String> {
    match elemento.tag {
        0x04 | 0x80 => Ok(elemento.content.to_vec()),
        0x24 | 0xa0 => {
            let mut out = Vec::new();
            for figlio in tlv_figli(elemento.content)? {
                out.extend(tlv_ottetti(&figlio)?);
            }
            Ok(out)
        }
        altro => Err(format!(
            "Atteso un OCTET STRING nell'archivio PKCS#12, trovato il tag {altro:#04x}."
        )),
    }
}

fn oid_uguale(elemento: &Tlv<'_>, atteso: &[u8]) -> bool {
    elemento.tag == 0x06 && elemento.content == atteso
}

fn primo_figlio<'a>(elemento: &Tlv<'a>) -> Result<Tlv<'a>, String> {
    tlv_figli(elemento.content)?
        .into_iter()
        .next()
        .ok_or_else(|| "Elemento ASN.1 vuoto nell'archivio PKCS#12.".to_string())
}

fn materiale_da_p12(percorso: &Path, password_testo: &str) -> Result<MaterialeFirma, String> {
    let dati = std::fs::read(percorso).map_err(|e| format!("File PKCS#12 non leggibile: {e}"))?;
    let pfx = leggi_tlv(&dati)?;
    if pfx.tag != 0x30 {
        return Err("Il file indicato non è un archivio PKCS#12 valido.".into());
    }
    let campi = tlv_figli(pfx.content)?;
    if campi.len() < 2 {
        return Err("Archivio PKCS#12 incompleto.".into());
    }
    let versione = tlv_intero(&campi[0])?;
    if versione != 3 {
        return Err(format!(
            "Versione PKCS#12 non supportata ({versione}); è attesa la versione 3."
        ));
    }

    let content_info = tlv_figli(campi[1].content)?;
    if content_info.len() < 2 {
        return Err("Archivio PKCS#12 incompleto.".into());
    }
    if !oid_uguale(&content_info[0], OID_DATA_CONTENT) {
        return Err("Archivio PKCS#12 con contenuto firmato (signedData) non supportato.".into());
    }
    let auth_safe = tlv_ottetti(&primo_figlio(&content_info[1])?)?;

    // Il tipo `Password` del crate viene dedotto dalle chiamate che seguono.
    let password = password_testo.into();

    // I bag vengono decifrati dal crate `keystore-pkcs12`.
    let leggi_bags = |der: &[u8]| -> Result<Vec<SafeBag>, String> {
        let sequenza = leggi_tlv(der)?;
        if sequenza.tag != 0x30 {
            return Err("Contenuto del file PKCS#12 non valido.".into());
        }
        let mut bags = Vec::new();
        for elemento in tlv_figli(sequenza.content)? {
            let bag = SafeBag::parse(elemento.raw, &password)
                .map_err(|e| format!("Contenuto del file PKCS#12 non valido: {e}"))?;
            bags.push(bag);
        }
        Ok(bags)
    };

    if let Some(mac_elemento) = campi.get(2) {
        let mac = MacParams::parse(mac_elemento.raw)
            .map_err(|e| format!("MAC del file PKCS#12 non valido: {e}"))?;
        let valido = mac
            .verify(&password, &auth_safe)
            .map_err(|e| format!("Verifica del MAC del file PKCS#12 non riuscita: {e}"))?;
        if !valido {
            return Err("Password del file PKCS#12 errata o archivio danneggiato.".into());
        }
    }

    let autenticato = leggi_tlv(&auth_safe)?;
    if autenticato.tag != 0x30 {
        return Err("Contenuto del file PKCS#12 non valido.".into());
    }

    let mut chiavi: Vec<(Option<Vec<u8>>, Vec<u8>)> = Vec::new();
    let mut certificati: Vec<(Option<Vec<u8>>, Vec<u8>)> = Vec::new();

    for info in tlv_figli(autenticato.content)? {
        let parti = tlv_figli(info.content)?;
        if parti.len() < 2 {
            return Err("Archivio PKCS#12 incompleto.".into());
        }
        let interno = primo_figlio(&parti[1])?;
        let contenuto = if oid_uguale(&parti[0], OID_DATA_CONTENT) {
            tlv_ottetti(&interno)?
        } else if oid_uguale(&parti[0], OID_ENCRYPTED_DATA_CONTENT) {
            let dati_cifrati = tlv_figli(interno.content)?;
            if dati_cifrati.len() < 2 {
                return Err("Contenuto cifrato del file PKCS#12 incompleto.".into());
            }
            let info_cifrata = tlv_figli(dati_cifrati[1].content)?;
            if info_cifrata.len() < 3 {
                return Err("Contenuto cifrato del file PKCS#12 incompleto.".into());
            }
            let schema =
                PbeScheme::parse_algorithm_identifier(info_cifrata[1].raw).map_err(|e| {
                    format!("Algoritmo di cifratura del file PKCS#12 non supportato: {e}")
                })?;
            let cifrato = tlv_ottetti(&info_cifrata[2])?;
            schema.decrypt(&password, &cifrato).map_err(|e| {
                format!(
                    "Apertura del contenuto del file PKCS#12 non riuscita (password errata?): {e}"
                )
            })?
        } else {
            return Err("Tipo di contenuto PKCS#12 non supportato.".into());
        };

        for bag in leggi_bags(&contenuto)? {
            match bag {
                SafeBag::Key(bag) => {
                    chiavi.push((bag.attributes.local_key_id(), bag.key.0.clone()))
                }
                SafeBag::Cert(bag) if bag.cert_type == CertType::X509 => {
                    certificati.push((bag.attributes.local_key_id(), bag.cert.0.clone()));
                }
                _ => {}
            }
        }
    }

    if chiavi.is_empty() {
        return Err("Il file PKCS#12 non contiene una chiave privata.".into());
    }
    if certificati.is_empty() {
        return Err("Il file PKCS#12 non contiene un certificato.".into());
    }

    let mut scelta: Option<(Chiave, Vec<u8>)> = None;
    'chiavi: for (id_chiave, der_chiave) in &chiavi {
        let Ok(chiave) = Chiave::da_pkcs8(der_chiave) else {
            continue;
        };
        let mut candidati: Vec<&Vec<u8>> = Vec::new();
        if let Some(id) = id_chiave {
            candidati.extend(
                certificati
                    .iter()
                    .filter(|(id_cert, _)| id_cert.as_ref() == Some(id))
                    .map(|(_, der)| der),
            );
        }
        candidati.extend(certificati.iter().map(|(_, der)| der));
        for der_cert in candidati {
            if let Ok(cert) = rasn::der::decode::<Certificate>(der_cert) {
                if chiave_corrisponde(&cert, &chiave) {
                    scelta = Some((chiave, der_cert.clone()));
                    break 'chiavi;
                }
            }
        }
    }

    let Some((chiave, foglia_der)) = scelta else {
        return Err(
            "Nessun certificato del file PKCS#12 corrisponde alla chiave privata: verifica la coppia chiave/certificato."
                .into(),
        );
    };
    let foglia = rasn::der::decode::<Certificate>(&foglia_der)
        .map_err(|e| format!("Certificato del file PKCS#12 non valido: {e}"))?;
    let altri: Vec<Vec<u8>> = certificati
        .iter()
        .map(|(_, der)| der.clone())
        .filter(|der| *der != foglia_der)
        .collect();
    let catena = ordina_catena(foglia_der, altri);
    Ok(MaterialeFirma {
        chiave,
        foglia,
        catena,
    })
}

fn carica_materiale(credenziali: &Credenziali) -> Result<MaterialeFirma, String> {
    match credenziali {
        Credenziali::Pem { cert, key } => materiale_da_pem(cert, key),
        Credenziali::P12 { path, password } => materiale_da_p12(path, password),
    }
}

// -- Costruzione del CMS ------------------------------------------------------

fn signing_certificate_v2(cert_der: &[u8]) -> Vec<u8> {
    let cert_hash = Sha256::digest(cert_der);
    let ess_cert_id = der_tlv(0x30, &der_tlv(0x04, &cert_hash));
    der_tlv(0x30, &der_tlv(0x30, &ess_cert_id))
}

fn utc_time_string() -> String {
    chrono::Utc::now().format("%y%m%d%H%M%SZ").to_string()
}

fn pdf_date_string() -> String {
    // Formato PDF 32000-1 §7.9.4: D:YYYYMMDDHHmmSS±HH'mm'
    // (apostrofo finale obbligatorio quando c'è il fuso orario).
    let now = chrono::Local::now();
    let base = now.format("D:%Y%m%d%H%M%S").to_string();
    let offset = now.format("%:z").to_string().replace(':', "'");
    format!("{base}{offset}'")
}

fn attr(kind: ObjectIdentifier, value: Vec<u8>) -> Attribute {
    Attribute {
        r#type: kind,
        values: SetOf::from_vec(vec![Any::new(value)]),
    }
}

fn build_cms(
    materiale: &MaterialeFirma,
    content_digest: &[u8],
    tsa_url: Option<&str>,
) -> Result<Vec<u8>, String> {
    let hash = materiale.chiave.algoritmo_digest();
    let signing_time = utc_time_string();

    let mut attributes = vec![
        attr(
            oid_content_type(),
            rasn::der::encode(&oid_data()).map_err(|e| e.to_string())?,
        ),
        attr(
            oid_message_digest(),
            rasn::der::encode(&OctetString::from(content_digest.to_vec()))
                .map_err(|e| e.to_string())?,
        ),
        attr(oid_signing_time(), der_tlv(0x17, signing_time.as_bytes())),
        attr(
            oid(&[1, 2, 840, 113549, 1, 9, 16, 2, 47]),
            signing_certificate_v2(&materiale.catena[0]),
        ),
    ];

    // DER richiede che SET OF sia ordinato per codifica degli elementi.
    attributes.sort_by_key(|attribute| rasn::der::encode(attribute).unwrap_or_default());
    let signed_attrs = SetOf::from_vec(attributes);

    let signed_bytes = rasn::der::encode(&signed_attrs)
        .map_err(|e| format!("Attributi firmati non codificabili: {e}"))?;
    let signature = materiale.chiave.firma(&signed_bytes)?;

    // Marca temporale RFC 3161 (PAdES B-T): l'attributo è NON firmato, quindi
    // va aggiunto dopo il calcolo della firma e non la invalida. L'impronta si
    // calcola sui byte del valore di firma, non sul CMS che lo contiene.
    let unsigned_attrs = match tsa_url {
        Some(url) => {
            let impronta = hash.digest(&[&signature]);
            let nome_hash = match hash {
                HashKind::Sha256 => "sha256",
                HashKind::Sha384 => "sha384",
            };
            let token = crate::pades_tsa::request_timestamp(url, &impronta, nome_hash)?;
            if !token.imprint_ok {
                return Err(
                    "La marca temporale ricevuta non corrisponde alla firma: l'impronta inviata e quella restituita differiscono.".into(),
                );
            }
            Some(SetOf::from_vec(vec![attr(
                oid(&[1, 2, 840, 113549, 1, 9, 16, 2, 14]),
                token.der,
            )]))
        }
        None => None,
    };

    let signer_info = SignerInfo {
        version: Integer::from(1),
        sid: SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
            issuer: materiale.foglia.tbs_certificate.issuer.clone(),
            serial_number: materiale.foglia.tbs_certificate.serial_number.clone(),
        }),
        digest_algorithm: hash.algorithm(),
        signed_attrs: Some(signed_attrs),
        signature_algorithm: materiale.chiave.algoritmo_firma(),
        signature: OctetString::from(signature),
        unsigned_attrs,
    };

    let mut certificati = Vec::with_capacity(materiale.catena.len());
    for der in &materiale.catena {
        let cert = rasn::der::decode::<Certificate>(der)
            .map_err(|e| format!("Certificato della catena non valido: {e}"))?;
        certificati.push(CertificateChoices::Certificate(Box::new(cert)));
    }

    let signed_data = SignedData {
        version: Integer::from(1),
        digest_algorithms: DigestAlgorithmIdentifiers::from_vec(vec![hash.algorithm()]),
        encap_content_info: EncapsulatedContentInfo {
            content_type: oid_data(),
            content: None,
        },
        certificates: Some(CertificateSet::from_vec(certificati)),
        crls: None,
        signer_infos: SignerInfos::from_vec(vec![signer_info]),
    };

    let signed_data_der =
        rasn::der::encode(&signed_data).map_err(|e| format!("SignedData non codificabile: {e}"))?;

    let content_info = ContentInfo {
        content_type: oid_signed_data(),
        content: Any::new(signed_data_der),
    };
    rasn::der::encode(&content_info).map_err(|e| format!("CMS non codificabile: {e}"))
}

// -- Geometria e apparenza del widget ----------------------------------------

fn norm_rect_to_pdf(rect: &[f32; 4], rotation: i64, media: (f64, f64, f64, f64)) -> Vec<f64> {
    let (mx0, my0, mx1, my1) = media;
    let width = mx1 - mx0;
    let height = my1 - my0;
    let mut xs = Vec::with_capacity(4);
    let mut ys = Vec::with_capacity(4);
    for (dx, dy) in [
        (rect[0], rect[1]),
        (rect[0] + rect[2], rect[1]),
        (rect[0], rect[1] + rect[3]),
        (rect[0] + rect[2], rect[1] + rect[3]),
    ] {
        let (du, dv) = (dx as f64, dy as f64);
        let (u, v) = match rotation.rem_euclid(360) {
            90 => (dv, du),
            180 => (1.0 - du, dv),
            270 => (1.0 - dv, 1.0 - du),
            _ => (du, 1.0 - dv),
        };
        xs.push(mx0 + u * width);
        ys.push(my0 + v * height);
    }
    let x0 = xs.iter().cloned().fold(f64::INFINITY, f64::min);
    let x1 = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let y0 = ys.iter().cloned().fold(f64::INFINITY, f64::min);
    let y1 = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    vec![x0, y0, x1, y1]
}

fn media_box(doc: &Document, page_id: ObjectId) -> (f64, f64, f64, f64) {
    let mut current = Some(page_id);
    while let Some(id) = current {
        if let Ok(dict) = doc.get_dictionary(id) {
            if let Ok(Object::Array(values)) = dict.get(b"MediaBox") {
                let n: Vec<f64> = values
                    .iter()
                    .filter_map(|object| match object {
                        Object::Integer(v) => Some(*v as f64),
                        Object::Real(v) => Some(*v as f64),
                        _ => None,
                    })
                    .collect();
                if n.len() >= 4 {
                    return (n[0], n[1], n[2], n[3]);
                }
            }
            current = dict.get(b"Parent").ok().and_then(|p| p.as_reference().ok());
        } else {
            break;
        }
    }
    (0.0, 0.0, 595.0, 842.0)
}

fn rotation(doc: &Document, page_id: ObjectId) -> i64 {
    doc.get_dictionary(page_id)
        .ok()
        .and_then(|d| d.get(b"Rotate").ok().cloned())
        .and_then(|o| match o {
            Object::Integer(v) => Some(v),
            Object::Real(v) => Some(v as i64),
            _ => None,
        })
        .unwrap_or(0)
}

/// Dimensioni dello spazio di disegno dell'apparenza e matrice che lo porta
/// nella pagina, compensando la rotazione del foglio.
fn matrice_apparenza(rect: &[f64], rotazione: i64) -> (f64, f64, [f64; 6]) {
    let x0 = rect[0].min(rect[2]);
    let y0 = rect[1].min(rect[3]);
    let larghezza_pagina = (rect[2] - rect[0]).abs().max(1.0);
    let altezza_pagina = (rect[3] - rect[1]).abs().max(1.0);
    match rotazione.rem_euclid(360) {
        90 => (
            altezza_pagina,
            larghezza_pagina,
            [0.0, 1.0, -1.0, 0.0, x0 + larghezza_pagina, y0],
        ),
        180 => (
            larghezza_pagina,
            altezza_pagina,
            [
                -1.0,
                0.0,
                0.0,
                -1.0,
                x0 + larghezza_pagina,
                y0 + altezza_pagina,
            ],
        ),
        270 => (
            altezza_pagina,
            larghezza_pagina,
            [0.0, -1.0, 1.0, 0.0, x0, y0 + altezza_pagina],
        ),
        _ => (
            larghezza_pagina,
            altezza_pagina,
            [1.0, 0.0, 0.0, 1.0, x0, y0],
        ),
    }
}

/// Converte una stringa in una stringa letterale PDF con codifica WinAnsi.
/// I byte non ASCII vengono scritti come sequenze ottali, così il flusso resta
/// ASCII e i caratteri accentati mantengono la codifica a un byte.
fn testo_pdf(testo: &str) -> String {
    let mut out = String::with_capacity(testo.len() + 4);
    for carattere in testo.chars() {
        let mappato = match carattere {
            '\u{2018}' | '\u{2019}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{2013}' | '\u{2014}' => '-',
            '\u{2026}' => {
                out.push_str("...");
                continue;
            }
            '\n' | '\r' | '\t' => ' ',
            altro => altro,
        };
        if (mappato as u32) <= 0xFF {
            let byte = mappato as u8;
            if (0x20..0x7f).contains(&byte) {
                if matches!(byte, b'(' | b')' | b'\\') {
                    out.push('\\');
                }
                out.push(byte as char);
            } else {
                out.push_str(&format!("\\{byte:03o}"));
            }
        } else {
            out.push('?');
        }
    }
    out
}

/// Accorcia il testo perché entri nella larghezza disponibile.
fn adatta_testo(testo: &str, larghezza: f64, corpo: f64) -> String {
    let massimo = ((larghezza / (corpo * 0.5)).floor() as usize).max(1);
    if testo.chars().count() <= massimo {
        return testo.to_string();
    }
    if massimo <= 4 {
        return testo.chars().take(massimo).collect();
    }
    let mut out: String = testo.chars().take(massimo - 3).collect();
    out.push_str("...");
    out
}

fn contenuto_apparenza(
    larghezza: f64,
    altezza: f64,
    nome: &str,
    motivo: &str,
    luogo: &str,
    data: &str,
    etichetta: &str,
) -> String {
    let margine = (altezza * 0.07).clamp(1.8, 6.0);
    let barra = (larghezza * 0.012).clamp(1.2, 3.5);
    let x = margine + barra + 3.0;
    let disponibile = (larghezza - x - margine).max(10.0);
    let piccolo = (altezza * 0.105).clamp(4.5, 8.0);
    let grande = (altezza * 0.21).clamp(6.0, 15.0);
    let compatto = altezza < 34.0;

    let mut contenuto = String::with_capacity(600);
    contenuto.push_str("q\n");
    contenuto.push_str("1 1 1 rg\n");
    contenuto.push_str(&format!("0 0 {larghezza:.2} {altezza:.2} re f\n"));
    contenuto.push_str("0.12 0.26 0.44 rg\n");
    contenuto.push_str(&format!("0 0 {barra:.2} {altezza:.2} re f\n"));
    contenuto.push_str("0.72 0.75 0.79 RG\n");
    contenuto.push_str("0.6 w\n");
    contenuto.push_str(&format!(
        "0.3 0.3 {:.2} {:.2} re S\n",
        (larghezza - 0.6).max(0.1),
        (altezza - 0.6).max(0.1)
    ));

    let testo_nome = adatta_testo(nome, disponibile, grande);
    let mut righe: Vec<(f64, f64, String, bool)> = Vec::new();

    if compatto {
        let y_nome = altezza - margine - grande * 0.75;
        righe.push((y_nome, grande, testo_nome, true));
        let y_data = y_nome - piccolo * 1.25;
        if y_data > margine {
            righe.push((
                y_data,
                piccolo,
                adatta_testo(data, disponibile, piccolo),
                false,
            ));
        }
    } else {
        let y_etichetta = altezza - margine - piccolo * 0.85;
        let y_nome = y_etichetta - piccolo * 0.55 - grande * 0.75;
        let y_motivo = y_nome - grande * 0.35 - piccolo * 0.9;
        righe.push((y_etichetta, piccolo, etichetta.to_string(), false));
        righe.push((y_nome, grande, testo_nome, true));

        let motivo_completo = if motivo.trim().is_empty() {
            String::new()
        } else {
            format!("Motivo: {}", motivo.trim())
        };
        let mut y = y_motivo;
        if !motivo_completo.is_empty() {
            righe.push((
                y,
                piccolo,
                adatta_testo(&motivo_completo, disponibile, piccolo),
                false,
            ));
            y -= piccolo * 1.5;
        }
        let data_completa = if luogo.trim().is_empty() {
            data.to_string()
        } else {
            format!("{data} · {}", luogo.trim())
        };
        if y > margine {
            righe.push((
                y,
                piccolo,
                adatta_testo(&data_completa, disponibile, piccolo),
                false,
            ));
        }
    }

    for (y, corpo, testo, grassetto) in righe {
        let font = if grassetto { "F2" } else { "F1" };
        let colore = if grassetto {
            "0.10 0.16 0.28 rg"
        } else {
            "0.42 0.45 0.50 rg"
        };
        contenuto.push_str("BT\n");
        contenuto.push_str(&format!("/{font} {corpo:.2} Tf\n"));
        contenuto.push_str(colore);
        contenuto.push('\n');
        contenuto.push_str(&format!("{x:.2} {y:.2} Td\n"));
        contenuto.push_str(&format!("({}) Tj\n", testo_pdf(&testo)));
        contenuto.push_str("ET\n");
    }
    contenuto.push_str("Q\n");
    contenuto
}

fn risorse_apparenza() -> Dictionary {
    let mut helvetica = Dictionary::new();
    helvetica.set("Type", Object::Name(b"Font".to_vec()));
    helvetica.set("Subtype", Object::Name(b"Type1".to_vec()));
    helvetica.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    helvetica.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));

    let mut helvetica_bold = helvetica.clone();
    helvetica_bold.set("BaseFont", Object::Name(b"Helvetica-Bold".to_vec()));

    let mut font = Dictionary::new();
    font.set("F1", Object::Dictionary(helvetica));
    font.set("F2", Object::Dictionary(helvetica_bold));

    let mut risorse = Dictionary::new();
    risorse.set("Font", Object::Dictionary(font));
    risorse
}

fn add_signature_field(
    doc: &mut Document,
    page_index: usize,
    rect: &[f32; 4],
    name: &str,
    reason: &str,
    location: &str,
    placeholder_len: usize,
    certification: Option<u8>,
    visible: bool,
) -> Result<String, String> {
    let pages = doc.get_pages();
    let page_id = *pages
        .get(&(page_index as u32 + 1))
        .ok_or("Pagina della firma non trovata.")?;
    let rotazione = rotation(doc, page_id);
    let user_rect = norm_rect_to_pdf(rect, rotazione, media_box(doc, page_id));

    let field_name = format!("FirmaOpenPDF{}", chrono::Utc::now().timestamp_millis());
    let data_firma = chrono::Local::now().format("%d/%m/%Y %H:%M").to_string();
    let etichetta = match certification {
        Some(livello) => format!("Firma di certificazione (P{livello})"),
        None => "Firma digitale PAdES".to_string(),
    };

    // Apparenza: Form XObject con riquadro, nome, motivo, data e luogo. Con
    // `visible = false` il widget resta un campo firma invisibile ma valido.
    let mut widget = Dictionary::new();
    widget.set("Type", Object::Name(b"Annot".to_vec()));
    widget.set("Subtype", Object::Name(b"Widget".to_vec()));
    widget.set(
        "Rect",
        Object::Array(user_rect.iter().map(|v| Object::Real(*v as f32)).collect()),
    );
    widget.set("F", Object::Integer(132));
    widget.set("P", Object::Reference(page_id));
    widget.set("FT", Object::Name(b"Sig".to_vec()));
    widget.set("T", Object::string_literal(field_name.clone()));
    widget.set("V", Object::Null);
    if visible {
        let (larghezza_ap, altezza_ap, matrice) = matrice_apparenza(&user_rect, rotazione);
        let contenuto_ap = contenuto_apparenza(
            larghezza_ap,
            altezza_ap,
            name,
            reason,
            location,
            &data_firma,
            &etichetta,
        );
        let mut dizionario_ap = Dictionary::new();
        dizionario_ap.set("Type", Object::Name(b"XObject".to_vec()));
        dizionario_ap.set("Subtype", Object::Name(b"Form".to_vec()));
        dizionario_ap.set("FormType", Object::Integer(1));
        dizionario_ap.set(
            "BBox",
            Object::Array(vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Real(larghezza_ap as f32),
                Object::Real(altezza_ap as f32),
            ]),
        );
        dizionario_ap.set(
            "Matrix",
            Object::Array(matrice.iter().map(|v| Object::Real(*v as f32)).collect()),
        );
        dizionario_ap.set("Resources", Object::Dictionary(risorse_apparenza()));
        let ap_id = doc.add_object(Stream::new(dizionario_ap, contenuto_ap.into_bytes()));
        let mut apparenza = Dictionary::new();
        apparenza.set("N", Object::Reference(ap_id));
        widget.set("AP", Object::Dictionary(apparenza));
    }
    let widget_id = doc.add_object(Object::Dictionary(widget));

    let mut annots = doc
        .get_dictionary(page_id)
        .ok()
        .and_then(|page| match page.get(b"Annots") {
            Ok(Object::Array(items)) => Some(items.clone()),
            Ok(Object::Reference(id)) => match doc.get_object(*id) {
                Ok(Object::Array(items)) => Some(items.clone()),
                _ => None,
            },
            _ => None,
        })
        .unwrap_or_default();
    annots.push(Object::Reference(widget_id));
    if let Ok(page) = doc.get_dictionary_mut(page_id) {
        page.set("Annots", Object::Array(annots));
    }

    let root_id = doc
        .trailer
        .get(b"Root")
        .ok()
        .and_then(|o| o.as_reference().ok())
        .ok_or("Catalogo del PDF non trovato.")?;
    let existing_form = doc
        .get_dictionary(root_id)
        .ok()
        .and_then(|root| root.get(b"AcroForm").ok())
        .and_then(|form| form.as_reference().ok());
    match existing_form {
        Some(form_id) => {
            if let Ok(form) = doc.get_dictionary_mut(form_id) {
                let mut fields = match form.get(b"Fields") {
                    Ok(Object::Array(items)) => items.clone(),
                    _ => Vec::new(),
                };
                fields.push(Object::Reference(widget_id));
                form.set("Fields", Object::Array(fields));
                form.set("SigFlags", Object::Integer(3));
            }
        }
        None => {
            let mut form = Dictionary::new();
            form.set("Fields", Object::Array(vec![Object::Reference(widget_id)]));
            form.set("SigFlags", Object::Integer(3));
            let form_id = doc.add_object(Object::Dictionary(form));
            if let Ok(root) = doc.get_dictionary_mut(root_id) {
                root.set("AcroForm", Object::Reference(form_id));
            }
        }
    }

    // Il dizionario /Sig viene aggiunto per ultimo: il campo /Contents e
    // /ByteRange occupano spazio variabile e, così facendo, nessun offset di
    // oggetto nella xref resta dopo i byte inseriti dal padding.
    let mut sig = Dictionary::new();
    sig.set("Type", Object::Name(b"Sig".to_vec()));
    sig.set("Filter", Object::Name(b"Adobe.PPKLite".to_vec()));
    sig.set("SubFilter", Object::Name(SUBFILTER.as_bytes().to_vec()));
    sig.set(
        "ByteRange",
        Object::Array(vec![
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(0),
            Object::Integer(0),
        ]),
    );
    let mut placeholder = MARKER.to_vec();
    placeholder.resize(placeholder_len, 0);
    sig.set(
        "Contents",
        Object::String(placeholder, StringFormat::Hexadecimal),
    );
    sig.set("M", Object::string_literal(pdf_date_string()));
    sig.set("Name", Object::string_literal(name));
    if !reason.is_empty() {
        sig.set("Reason", Object::string_literal(reason));
    }
    if !location.is_empty() {
        sig.set("Location", Object::string_literal(location));
    }
    if let Some(livello) = certification {
        let mut parametri = Dictionary::new();
        parametri.set("Type", Object::Name(b"TransformParams".to_vec()));
        parametri.set("P", Object::Integer(livello as i64));
        parametri.set("V", Object::Name(b"1.2".to_vec()));
        let mut riferimento = Dictionary::new();
        riferimento.set("Type", Object::Name(b"SigRef".to_vec()));
        riferimento.set("TransformMethod", Object::Name(b"DocMDP".to_vec()));
        riferimento.set("TransformParams", Object::Dictionary(parametri));
        sig.set(
            "Reference",
            Object::Array(vec![Object::Dictionary(riferimento)]),
        );
    }
    let sig_id = doc.add_object(Object::Dictionary(sig));

    if let Ok(widget) = doc.get_dictionary_mut(widget_id) {
        widget.set("V", Object::Reference(sig_id));
    }
    if certification.is_some() {
        let mut permessi = Dictionary::new();
        permessi.set("DocMDP", Object::Reference(sig_id));
        if let Ok(root) = doc.get_dictionary_mut(root_id) {
            root.set("Perms", Object::Dictionary(permessi));
        }
    }

    Ok(field_name)
}

/// Allarga l'array `/ByteRange` a larghezza fissa. Restituisce la posizione
/// dell'array e di quanto è cresciuto il file.
fn pad_byte_range(buffer: &mut Vec<u8>) -> Result<(usize, usize), String> {
    let marker = b"/ByteRange";
    // L'ultima occorrenza è quella del campo appena creato: eventuali firme
    // precedenti restano più indietro nel file.
    let start = buffer
        .windows(marker.len())
        .rposition(|window| window == marker)
        .ok_or("Campo ByteRange non trovato.")?;
    let open = buffer[start..]
        .iter()
        .position(|b| *b == b'[')
        .map(|offset| start + offset)
        .ok_or("ByteRange non è un array.")?;
    let close = buffer[open..]
        .iter()
        .position(|b| *b == b']')
        .map(|offset| open + offset)
        .ok_or("ByteRange non è un array.")?;
    const WIDTH: usize = 48;
    let current = close - open + 1;
    if current >= WIDTH {
        return Ok((open, 0));
    }
    let mut replacement = b"[0 0 0 0".to_vec();
    replacement.resize(WIDTH - 1, b' ');
    replacement.push(b']');
    buffer.splice(open..=close, replacement);
    Ok((open, WIDTH - current))
}

/// Dopo il padding di `/ByteRange` la tabella xref si è spostata: il valore di
/// `startxref` va incrementato della stessa quantità. Gli offset degli oggetti
/// non cambiano perché il dizionario `/Sig` è l'ultimo oggetto del file.
fn aggiorna_startxref(buffer: &mut Vec<u8>, delta: usize) -> Result<(), String> {
    if delta == 0 {
        return Ok(());
    }
    let marker = b"startxref";
    let start = buffer
        .windows(marker.len())
        .rposition(|window| window == marker)
        .ok_or("Voce startxref non trovata.")?;
    let mut indice = start + marker.len();
    while indice < buffer.len() && !buffer[indice].is_ascii_digit() {
        indice += 1;
    }
    let inizio = indice;
    while indice < buffer.len() && buffer[indice].is_ascii_digit() {
        indice += 1;
    }
    if inizio == indice {
        return Err("Valore di startxref non valido.".into());
    }
    let valore: usize = std::str::from_utf8(&buffer[inizio..indice])
        .map_err(|e| format!("Valore di startxref non valido: {e}"))?
        .parse()
        .map_err(|e| format!("Valore di startxref non valido: {e}"))?;
    let nuovo = (valore + delta).to_string();
    buffer.splice(inizio..indice, nuovo.into_bytes());
    Ok(())
}

fn locate_contents(buffer: &[u8]) -> Result<(usize, usize), String> {
    let needle: String = MARKER.iter().map(|b| format!("{b:02X}")).collect();
    let position = buffer
        .windows(needle.len())
        .rposition(|window| {
            window
                .iter()
                .zip(needle.as_bytes())
                .all(|(byte, atteso)| byte.to_ascii_uppercase() == *atteso)
        })
        .ok_or("Segnaposto del contenuto firma non trovato.")?;
    let lt = buffer[..position]
        .iter()
        .rposition(|b| *b == b'<')
        .ok_or("Delimitatore della firma non trovato.")?;
    let gt = buffer[position..]
        .iter()
        .position(|b| *b == b'>')
        .map(|offset| position + offset)
        .ok_or("Delimitatore della firma non trovato.")?;
    Ok((lt, gt))
}

fn patch_byte_range(buffer: &mut Vec<u8>, open: usize, values: [u64; 4]) -> Result<(), String> {
    let close = buffer[open..]
        .iter()
        .position(|b| *b == b']')
        .map(|offset| open + offset)
        .ok_or("ByteRange non è un array.")?;
    let width = close - open + 1;
    let text = format!(
        "[{:>1} {:>10} {:>10} {:>10}]",
        values[0], values[1], values[2], values[3]
    );
    if text.len() > width {
        return Err("Spazio insufficiente per ByteRange.".into());
    }
    let mut replacement = text.into_bytes();
    replacement.resize(width, b' ');
    buffer.splice(open..=close, replacement);
    Ok(())
}

struct DocumentoPronto {
    buffer: Vec<u8>,
    open: usize,
    lt: usize,
    gt: usize,
    field: String,
}

fn prepara_documento(
    src: &Path,
    opts: &SignOptions,
    placeholder_len: usize,
) -> Result<DocumentoPronto, String> {
    let mut doc = match opts.pdf_password.as_deref() {
        Some(password) => Document::load_with_password(src, password),
        None => Document::load(src),
    }
    .map_err(|e| format!("Apertura del PDF non riuscita: {e}"))?;
    if doc.is_encrypted() {
        return Err("La firma di PDF cifrati non è supportata: rimuovi prima la password.".into());
    }

    let field = add_signature_field(
        &mut doc,
        opts.page_index,
        &opts.rect,
        &opts.name,
        &opts.reason,
        &opts.location,
        placeholder_len,
        opts.certification,
        opts.visible,
    )?;

    let mut buffer = Vec::new();
    doc.save_to(&mut buffer)
        .map_err(|e| format!("Salvataggio del PDF non riuscito: {e}"))?;

    let (open, delta) = pad_byte_range(&mut buffer)?;
    aggiorna_startxref(&mut buffer, delta)?;
    let (lt, gt) = locate_contents(&buffer)?;
    Ok(DocumentoPronto {
        buffer,
        open,
        lt,
        gt,
        field,
    })
}

/// Firma il PDF con credenziali PEM o PKCS#12 e parametri espliciti.
pub fn sign_pdf_con(
    src: &Path,
    dest: &Path,
    credenziali: &Credenziali,
    opts: &SignOptions,
) -> Result<SignOutcome, String> {
    if let Some(livello) = opts.certification {
        if !(1..=3).contains(&livello) {
            return Err(
                "Livello di certificazione DocMDP non valido: sono ammessi solo 1, 2 o 3.".into(),
            );
        }
    }

    let materiale = carica_materiale(credenziali)?;
    let hash = materiale.chiave.algoritmo_digest();
    let mut spazio = PLACEHOLDER_LEN;

    for tentativo in 0..=TENTATIVI_SPAZIO {
        let pronto = prepara_documento(src, opts, spazio)?;
        let mut buffer = pronto.buffer;
        let ranges = [
            0u64,
            pronto.lt as u64,
            (pronto.gt + 1) as u64,
            (buffer.len() - pronto.gt - 1) as u64,
        ];
        patch_byte_range(&mut buffer, pronto.open, ranges)?;

        let digest = hash.digest(&[
            &buffer[ranges[0] as usize..ranges[1] as usize],
            &buffer[ranges[2] as usize..(ranges[2] + ranges[3]) as usize],
        ]);
        let cms = build_cms(&materiale, &digest, opts.tsa_url.as_deref())?;

        if cms.len() <= spazio {
            let lunghezza_hex = spazio * 2;
            if pronto.gt != pronto.lt + 1 + lunghezza_hex {
                return Err("Il segnaposto del contenuto firma ha una dimensione inattesa.".into());
            }
            const CIFRE: &[u8; 16] = b"0123456789abcdef";
            let mut hex = vec![b'0'; lunghezza_hex];
            for (indice, byte) in cms.iter().enumerate() {
                hex[indice * 2] = CIFRE[(byte >> 4) as usize];
                hex[indice * 2 + 1] = CIFRE[(byte & 0x0f) as usize];
            }
            buffer[pronto.lt + 1..pronto.lt + 1 + lunghezza_hex].copy_from_slice(&hex);

            std::fs::write(dest, &buffer)
                .map_err(|e| format!("Scrittura del PDF firmato non riuscita: {e}"))?;

            let signer = common_name(&materiale.foglia);
            return Ok(SignOutcome {
                field: pronto.field,
                signer,
                size: buffer.len() as u64,
                path: dest.to_string_lossy().to_string(),
            });
        }

        if tentativo == TENTATIVI_SPAZIO {
            return Err(format!(
                "La firma CMS occupa {} byte ma lo spazio riservato nel PDF è di {}. Serve uno spazio /Contents di almeno {} byte (circa {} caratteri esadecimali).",
                cms.len(),
                spazio,
                cms.len(),
                cms.len() * 2
            ));
        }
        spazio = (cms.len() * 2).next_power_of_two().max(spazio * 2);
    }

    Err("Firma non riuscita: spazio per il CMS non sufficiente.".into())
}

#[allow(clippy::too_many_arguments)]
pub fn sign_pdf(
    src: &Path,
    dest: &Path,
    cert_pem_path: &Path,
    key_pem_path: &Path,
    page_index: usize,
    rect: &[f32; 4],
    name: &str,
    reason: &str,
    location: &str,
    password: Option<&str>,
) -> Result<SignOutcome, String> {
    let credenziali = Credenziali::pem(cert_pem_path, key_pem_path);
    let opts = SignOptions {
        page_index,
        rect: *rect,
        name: name.to_string(),
        reason: reason.to_string(),
        location: location.to_string(),
        pdf_password: password.map(|valore| valore.to_string()),
        certification: None,
        visible: true,
        tsa_url: None,
    };
    sign_pdf_con(src, dest, &credenziali, &opts)
}

fn any_to_string(bytes: &[u8]) -> String {
    if bytes.len() < 2 {
        return String::new();
    }
    let tag = bytes[0];
    let mut index = 2usize;
    let mut length = bytes[1] as usize;
    if length & 0x80 != 0 {
        let count = length & 0x7F;
        length = 0;
        for _ in 0..count {
            if index >= bytes.len() {
                break;
            }
            length = (length << 8) | bytes[index] as usize;
            index += 1;
        }
    }
    let end = (index + length).min(bytes.len());
    let content = &bytes[index..end];
    if tag == 0x1E {
        let units: Vec<u16> = content
            .chunks(2)
            .map(|chunk| {
                if chunk.len() == 2 {
                    ((chunk[0] as u16) << 8) | chunk[1] as u16
                } else {
                    chunk[0] as u16
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(content).to_string()
    }
}

fn common_name(cert: &Certificate) -> String {
    let rasn_pkix::Name::RdnSequence(rdns) = &cert.tbs_certificate.subject;
    for rdn in rdns.iter() {
        for attribute in rdn.iter() {
            if attribute.r#type == oid(&[2, 5, 4, 3]) {
                let value = any_to_string(attribute.value.as_bytes());
                if !value.is_empty() {
                    return value;
                }
            }
        }
    }
    "Firmatario sconosciuto".to_string()
}

fn read_u8(doc: &Document, object: &Object) -> Option<Vec<u8>> {
    match object {
        Object::String(bytes, _) => Some(bytes.clone()),
        Object::Reference(id) => match doc.get_object(*id) {
            Ok(Object::String(bytes, _)) => Some(bytes.clone()),
            Ok(Object::Stream(stream)) => stream.decompressed_content().ok(),
            _ => None,
        },
        _ => None,
    }
}

fn certificato_firmatario<'a>(
    signed_data: &'a SignedData,
    signer_info: &SignerInfo,
) -> Option<&'a Certificate> {
    let set = signed_data.certificates.as_ref()?;
    let mut primo = None;
    for scelta in set.iter() {
        let CertificateChoices::Certificate(cert) = scelta else {
            continue;
        };
        if primo.is_none() {
            primo = Some(cert.as_ref());
        }
        if let SignerIdentifier::IssuerAndSerialNumber(sid) = &signer_info.sid {
            if cert.tbs_certificate.issuer == sid.issuer
                && cert.tbs_certificate.serial_number == sid.serial_number
            {
                return Some(cert.as_ref());
            }
        }
    }
    primo
}

fn verifica_rsa(spki: &[u8], hash: HashKind, messaggio: &[u8], firma: &[u8]) -> bool {
    let Ok(public_key) = RsaPublicKey::from_public_key_der(spki) else {
        return false;
    };
    let Ok(signature) = RsaSignature::try_from(firma) else {
        return false;
    };
    match hash {
        HashKind::Sha256 => RsaVerifyingKey::<Sha256>::new(public_key)
            .verify(messaggio, &signature)
            .is_ok(),
        HashKind::Sha384 => RsaVerifyingKey::<Sha384>::new(public_key)
            .verify(messaggio, &signature)
            .is_ok(),
    }
}

fn verifica_ecdsa(spki: &[u8], hash: HashKind, messaggio: &[u8], firma: &[u8]) -> bool {
    let digest = hash.digest(&[messaggio]);
    if let Ok(verifying) = p256::ecdsa::VerifyingKey::from_public_key_der(spki) {
        if let Ok(signature) = p256::ecdsa::DerSignature::from_bytes(firma) {
            if verifying.verify_prehash(&digest, &signature).is_ok() {
                return true;
            }
        }
    }
    if let Ok(verifying) = p384::ecdsa::VerifyingKey::from_public_key_der(spki) {
        if let Ok(signature) = p384::ecdsa::DerSignature::from_bytes(firma) {
            if verifying.verify_prehash(&digest, &signature).is_ok() {
                return true;
            }
        }
    }
    false
}

/// Il nome del campo firma sta sul widget (/T), non sul dizionario /Sig.
fn widget_field_name(doc: &Document, sig_id: ObjectId) -> Option<String> {    for (_, object) in doc.objects.iter() {
        let Object::Dictionary(widget) = object else {
            continue;
        };
        if widget.get(b"V").ok().and_then(|v| v.as_reference().ok()) != Some(sig_id) {
            continue;
        }
        return widget
            .get(b"T")
            .ok()
            .and_then(|t| read_u8(doc, t))
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string());
    }
    None
}

/// Legge un `signingTime` CMS (UTCTime o GeneralizedTime) in UTC.
fn parse_cms_time(attributo: &Attribute) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{NaiveDateTime, TimeZone, Utc};
    let bytes = attributo.values.iter().next()?.as_bytes().to_vec();
    let testo = std::str::from_utf8(&bytes).ok()?.trim().to_string();
    let senza_z = testo.trim_end_matches('Z');
    let formato = match senza_z.len() {
        12 => "%y%m%d%H%M%S",
        14 => "%Y%m%d%H%M%S",
        _ => return None,
    };
    NaiveDateTime::parse_from_str(senza_z, formato)
        .ok()
        .map(|n| Utc.from_utc_datetime(&n))
}

/// Livello DocMDP dal dizionario `/Reference` della firma, se è una firma di
/// certificazione. Acrobat scrive lo stesso schema.
fn docmdp_level(doc: &Document, dict: &Dictionary) -> Option<u8> {
    let riferimento = dict.get(b"Reference").ok()?.as_array().ok()?;
    for voce in riferimento.iter() {
        let Ok(id) = voce.as_reference() else {
            continue;
        };
        let Ok(entry) = doc.get_dictionary(id) else {
            continue;
        };
        let metodo = entry
            .get(b"TransformMethod")
            .ok()
            .and_then(|valore| valore.as_name().ok());
        if metodo != Some(b"DocMDP".as_slice()) {
            continue;
        }
        let livello = entry
            .get(b"TransformParams")
            .ok()
            .and_then(|params| params.as_reference().ok())
            .and_then(|params| doc.get_dictionary(params).ok())
            .and_then(|params| params.get(b"P").ok())
            .and_then(|p| p.as_i64().ok());
        if let Some(livello) = livello {
            if let Ok(livello) = u8::try_from(livello) {
                return Some(livello);
            }
        }
    }
    None
}

/// Certificato dell'emittente presente nella catena inclusa nella firma.
fn issuer_der_of(foglia: &Certificate, catena: &[Vec<u8>]) -> Option<Vec<u8>> {
    for der in catena {
        if let Ok(candidato) = rasn::der::decode::<Certificate>(der) {
            if candidato.tbs_certificate.subject == foglia.tbs_certificate.issuer {
                return Some(der.clone());
            }
        }
    }
    None
}

/// Verifica offline delle firme presenti nel PDF (nessuna richiesta di rete).
pub fn verify_pdf(path: &Path, password: Option<&str>) -> Result<Vec<SignatureInfo>, String> {
    verify_pdf_con(path, password, false)
}

/// Verifica le firme; con `check_revocation` interroga anche i responder OCSP.
pub fn verify_pdf_con(
    path: &Path,
    password: Option<&str>,
    check_revocation: bool,
) -> Result<Vec<SignatureInfo>, String> {
    let doc = match password {
        Some(p) => Document::load_with_password(path, p),
        None => Document::load(path),
    }
    .map_err(|e| format!("Apertura del PDF non riuscita: {e}"))?;
    let raw = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();

    for (sig_id, object) in doc.objects.iter() {
        let Object::Dictionary(dict) = object else {
            continue;
        };
        if dict.get(b"Type").ok().and_then(|t| t.as_name().ok()) != Some(b"Sig".as_slice()) {
            continue;
        }
        let subfilter = dict
            .get(b"SubFilter")
            .ok()
            .and_then(|s| s.as_name().ok())
            .map(|n| String::from_utf8_lossy(n).to_string())
            .unwrap_or_default();
        let contents = dict
            .get(b"Contents")
            .ok()
            .and_then(|c| read_u8(&doc, c))
            .unwrap_or_default();
        let byte_range: Vec<u64> = dict
            .get(b"ByteRange")
            .ok()
            .and_then(|b| b.as_array().ok())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| match item {
                        Object::Integer(v) => Some(*v as u64),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let field = widget_field_name(&doc, *sig_id).unwrap_or_else(|| "Firma".into());
        let time = dict
            .get(b"M")
            .ok()
            .and_then(|t| read_u8(&doc, t))
            .map(|b| String::from_utf8_lossy(&b).to_string())
            .unwrap_or_default();

        let mut info = SignatureInfo {
            field,
            signer: "Firmatario sconosciuto".into(),
            time,
            subfilter,
            covers_all: false,
            digest_ok: false,
            signature_ok: false,
            chain_valid: None,
            chain_complete: None,
            trusted: None,
            chain_errors: Vec::new(),
            attribute_errors: Vec::new(),
            chain: Vec::new(),
            valid_from: None,
            valid_to: None,
            docmdp: None,
            timestamp: None,
            revocation: None,
        };

        let fine = byte_range
            .get(2)
            .zip(byte_range.get(3))
            .and_then(|(inizio, lunghezza)| inizio.checked_add(*lunghezza));
        if byte_range.len() != 4
            || byte_range[0] > byte_range[1]
            || byte_range[1] > raw.len() as u64
            || byte_range[2] > raw.len() as u64
            || fine.map(|valore| valore > raw.len() as u64).unwrap_or(true)
        {
            out.push(info);
            continue;
        }
        let fine = fine.unwrap_or_default();
        info.covers_all = fine == raw.len() as u64;

        let trimmed_len = contents
            .iter()
            .rposition(|b| *b != 0)
            .map(|index| index + 1)
            .unwrap_or(0);
        let contents: Vec<u8> = contents[..trimmed_len].to_vec();

        let Ok(content_info) = rasn::der::decode::<ContentInfo>(&contents) else {
            out.push(info);
            continue;
        };
        let Ok(signed_data) = rasn::der::decode::<SignedData>(content_info.content.as_bytes())
        else {
            out.push(info);
            continue;
        };
        let Some(signer_info) = signed_data.signer_infos.iter().next() else {
            out.push(info);
            continue;
        };
        let Some(cert) = certificato_firmatario(&signed_data, signer_info) else {
            out.push(info);
            continue;
        };
        info.signer = common_name(cert);

        let Some(hash) = HashKind::from_oid(&signer_info.digest_algorithm.algorithm) else {
            out.push(info);
            continue;
        };

        let content_digest = hash.digest(&[
            &raw[byte_range[0] as usize..byte_range[1] as usize],
            &raw[byte_range[2] as usize..fine as usize],
        ]);

        let mut message_digest_ok = false;
        let mut signed_attrs: Option<SetOf<Attribute>> = None;
        if let Some(attributes) = &signer_info.signed_attrs {
            signed_attrs = Some(attributes.clone());
            for attribute in attributes.iter() {
                if attribute.r#type == oid_message_digest() {
                    if let Some(value) = attribute.values.iter().next() {
                        if let Ok(octets) = rasn::der::decode::<OctetString>(value.as_bytes()) {
                            message_digest_ok = octets.to_vec() == content_digest;
                        }
                    }
                }
            }
        }
        info.digest_ok = message_digest_ok;

        if let Some(attributes) = &signed_attrs {
            if let Ok(bytes) = rasn::der::encode(attributes) {
                if let Ok(spki) = rasn::der::encode(&cert.tbs_certificate.subject_public_key_info) {
                    let algoritmo = &cert
                        .tbs_certificate
                        .subject_public_key_info
                        .algorithm
                        .algorithm;
                    let firma = signer_info.signature.to_vec();
                    if *algoritmo == oid_rsa_encryption() {
                        info.signature_ok = verifica_rsa(&spki, hash, &bytes, &firma);
                    } else if *algoritmo == oid_ec_public_key() {
                        info.signature_ok = verifica_ecdsa(&spki, hash, &bytes, &firma);
                    }
                }
            }
        }

        // -- Catena, attributi firmati, marca temporale, DocMDP, revoca ------
        info.docmdp = docmdp_level(&doc, dict);

        let leaf_der = rasn::der::encode(cert).ok();
        let mut catena_der: Vec<Vec<u8>> = Vec::new();
        if let Some(insieme) = &signed_data.certificates {
            for scelta in insieme.iter() {
                if let CertificateChoices::Certificate(elemento) = scelta {
                    if let Ok(der) = rasn::der::encode(elemento.as_ref()) {
                        catena_der.push(der);
                    }
                }
            }
        }

        let istante_firma = signed_attrs
            .as_ref()
            .and_then(|attributi| {
                attributi
                    .iter()
                    .find(|attributo| attributo.r#type == oid_signing_time())
            })
            .and_then(parse_cms_time);
        let rapporto = crate::pades_verify::validate_chain(&catena_der, istante_firma);
        info.chain_valid = Some(rapporto.valid);
        info.chain_complete = Some(rapporto.complete);
        info.trusted = Some(rapporto.trusted);
        info.chain_errors = rapporto.errors;
        info.chain = rapporto.chain;
        if !rapporto.not_before.is_empty() {
            info.valid_from = Some(rapporto.not_before);
        }
        if !rapporto.not_after.is_empty() {
            info.valid_to = Some(rapporto.not_after);
        }

        if let (Some(attributi), Some(foglia)) = (signed_attrs.as_ref(), leaf_der.as_ref()) {
            if let Ok(bytes) = rasn::der::encode(attributi) {
                info.attribute_errors = crate::pades_verify::check_signed_attributes(
                    &bytes,
                    &content_digest,
                    foglia,
                );
            }
        }

        if let Some(non_firmati) = &signer_info.unsigned_attrs {
            for attributo in non_firmati.iter() {
                if attributo.r#type != oid(&[1, 2, 840, 113549, 1, 9, 16, 2, 14]) {
                    continue;
                }
                let Some(valore) = attributo.values.iter().next() else {
                    continue;
                };
                let token = valore.as_bytes().to_vec();
                let impronta = hash.digest(&[&signer_info.signature.to_vec()]);
                let nome_hash = match hash {
                    HashKind::Sha256 => "sha256",
                    HashKind::Sha384 => "sha384",
                };
                match crate::pades_tsa::verify_timestamp(&token, &impronta, nome_hash) {
                    Ok(marca) if marca.imprint_ok => info.timestamp = Some(marca.time),
                    Ok(_) => info.attribute_errors.push(
                        "La marca temporale non corrisponde alla firma: impronta diversa.".into(),
                    ),
                    Err(errore) => info
                        .attribute_errors
                        .push(format!("Marca temporale non verificata: {errore}")),
                }
            }
        }

        if check_revocation {
            if let Some(foglia) = leaf_der.as_ref() {
                let emittente = issuer_der_of(cert, &catena_der).unwrap_or_else(|| foglia.clone());
                info.revocation = Some(match crate::pades_ocsp::check(foglia, &emittente, None) {
                    Ok(esito) => {
                        let stato = match &esito.status {
                            crate::pades_ocsp::RevocationStatus::Good => {
                                format!("Buono (aggiornato al {})", esito.this_update)
                            }
                            crate::pades_ocsp::RevocationStatus::Revoked { time, reason } => {
                                format!("Revocato il {time}: {reason}")
                            }
                            crate::pades_ocsp::RevocationStatus::Unknown => {
                                format!("Stato sconosciuto al responder ({})", esito.this_update)
                            }
                        };
                        if esito.signature_ok {
                            stato
                        } else {
                            format!("{stato} — firma della risposta non valida")
                        }
                    }
                    Err(errore) => format!("Non determinabile: {errore}"),
                });
            }
        }

        out.push(info);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_pdf_con_fuso_orario_chiusa_da_apostrofo() {
        let value = pdf_date_string();
        assert!(value.starts_with("D:"), "formato data PDF: {value}");
        assert!(value.ends_with('\''), "manca l'apostrofo finale: {value}");
        assert!(value.contains('\''), "fuso orario non formato: {value}");
    }

    fn sample_pdf() -> Vec<u8> {
        sample_pdf_ruotato(0)
    }

    fn sample_pdf_ruotato(rotazione: i64) -> Vec<u8> {
        let content = b"BT /F1 12 Tf 72 700 Td (Contratto) Tj ET\n";
        let objects: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Rotate {rotazione} /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            )
            .into_bytes(),
            {
                let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
                stream.extend_from_slice(content);
                stream.extend_from_slice(b"endstream");
                stream
            },
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /MissingWidth 500 >>".to_vec(),
        ];
        let mut file: Vec<u8> = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(file.len());
            file.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            file.extend_from_slice(body);
            file.extend_from_slice(b"\nendobj\n");
        }
        let xref = file.len();
        file.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in &offsets {
            file.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        file.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        file
    }

    fn fixture(nome: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(nome)
    }

    fn percorso_temporaneo(nome: &str) -> PathBuf {
        // Nome univoco per processo: più esecuzioni di test in parallelo non
        // devono sovrascriversi a vicenda.
        static CONTATORE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let contatore = CONTATORE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("openpdf-{}-{contatore}-{nome}", std::process::id()))
    }

    fn opzioni(nome: &str) -> SignOptions {
        SignOptions {
            page_index: 0,
            rect: [0.06, 0.85, 0.34, 0.09],
            name: nome.to_string(),
            reason: "Approvazione".to_string(),
            location: "Roma".to_string(),
            pdf_password: None,
            certification: None,
            visible: true,
            tsa_url: None,
        }
    }

    fn firma_con_pem(src: &Path, dest: &Path, cert: &str, key: &str, nome: &str) -> SignOutcome {
        let credenziali = Credenziali::pem(fixture(cert), fixture(key));
        sign_pdf_con(src, dest, &credenziali, &opzioni(nome)).unwrap()
    }

    fn widget_firmato<'a>(doc: &'a Document) -> &'a Dictionary {
        for (_, object) in doc.objects.iter() {
            if let Object::Dictionary(dict) = object {
                if dict.get(b"Type").ok().and_then(|t| t.as_name().ok())
                    == Some(b"Annot".as_slice())
                    && dict.get(b"Subtype").ok().and_then(|t| t.as_name().ok())
                        == Some(b"Widget".as_slice())
                {
                    return dict;
                }
            }
        }
        panic!("widget della firma assente");
    }

    fn firma_dict<'a>(doc: &'a Document) -> &'a Dictionary {
        for (_, object) in doc.objects.iter() {
            if let Object::Dictionary(dict) = object {
                if dict.get(b"Type").ok().and_then(|t| t.as_name().ok()) == Some(b"Sig".as_slice())
                {
                    return dict;
                }
            }
        }
        panic!("dizionario /Sig assente");
    }

    fn cms_firmatario(dest: &Path) -> SignerInfo {
        let doc = Document::load(dest).unwrap();
        let dict = firma_dict(&doc);
        let contenuto = dict
            .get(b"Contents")
            .ok()
            .and_then(|c| read_u8(&doc, c))
            .unwrap();
        let tagliato = contenuto
            .iter()
            .rposition(|b| *b != 0)
            .map(|indice| indice + 1)
            .unwrap_or(0);
        let content_info = rasn::der::decode::<ContentInfo>(&contenuto[..tagliato]).unwrap();
        let signed_data = rasn::der::decode::<SignedData>(content_info.content.as_bytes()).unwrap();
        let firmatario = signed_data.signer_infos.iter().next().cloned().unwrap();
        firmatario
    }

    #[test]
    fn firma_invisibile_omette_l_apparenza() {
        let src = percorso_temporaneo("pades-invisibile-src.pdf");
        let dest = percorso_temporaneo("pades-invisibile.pdf");
        std::fs::write(&src, sample_pdf()).unwrap();
        if !fixture("test-cert.pem").exists() || !fixture("test-key.pem").exists() {
            eprintln!("fixture del certificato assenti: test saltato");
            return;
        }

        let mut opts = opzioni("Mario Rossi");
        opts.visible = false;
        let credenziali = Credenziali::pem(fixture("test-cert.pem"), fixture("test-key.pem"));
        sign_pdf_con(&src, &dest, &credenziali, &opts).unwrap();

        let documento = Document::load(&dest).unwrap();
        let widget = widget_firmato(&documento);
        assert!(
            widget.get(b"AP").is_err(),
            "il widget invisibile non deve avere /AP"
        );

        let firme = verify_pdf(&dest, None).unwrap();
        assert_eq!(firme.len(), 1);
        assert!(firme[0].digest_ok, "digest del contenuto non valido");
        assert!(firme[0].signature_ok, "firma CMS non valida");
    }

    #[test]
    fn verifica_riporta_catena_e_attributi_firmati() {
        let src = percorso_temporaneo("pades-catena-src.pdf");
        let dest = percorso_temporaneo("pades-catena.pdf");
        std::fs::write(&src, sample_pdf()).unwrap();
        if !fixture("test-cert.pem").exists() || !fixture("test-key.pem").exists() {
            eprintln!("fixture del certificato assenti: test saltato");
            return;
        }

        firma_con_pem(&src, &dest, "test-cert.pem", "test-key.pem", "Mario Rossi");
        let firme = verify_pdf(&dest, None).unwrap();
        assert_eq!(firme.len(), 1);
        let firma = &firme[0];
        assert!(
            firma.chain_valid.is_some(),
            "la verifica deve riportare l'esito della catena"
        );
        assert!(
            !firma.chain.is_empty(),
            "la verifica deve riportare i nomi dei certificati"
        );
        assert!(
            firma.attribute_errors.is_empty(),
            "attributi firmati con problemi: {:?}",
            firma.attribute_errors
        );
        assert!(firma.valid_from.is_some(), "manca l'inizio validità");
        assert!(firma.docmdp.is_none(), "firma non di certificazione");
    }

    #[test]
    fn signs_and_verifies_pades_bb() {
        let src = percorso_temporaneo("pades-src.pdf");
        let dest = percorso_temporaneo("pades-signed.pdf");
        std::fs::write(&src, sample_pdf()).unwrap();

        let cert = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-cert.pem");
        let key = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test-key.pem");
        if !cert.exists() || !key.exists() {
            eprintln!("fixture del certificato assenti: test saltato");
            return;
        }

        let outcome = sign_pdf(
            &src,
            &dest,
            &cert,
            &key,
            0,
            &[0.06, 0.85, 0.34, 0.09],
            "Mario Rossi",
            "Approvazione",
            "Roma",
            None,
        )
        .unwrap();
        assert!(outcome.size > 0);

        let signatures = verify_pdf(&dest, None).unwrap();
        assert_eq!(signatures.len(), 1);
        let signature = &signatures[0];
        assert!(signature.digest_ok, "digest del contenuto non valido");
        assert!(signature.signature_ok, "firma CMS non valida");
        assert!(
            signature.covers_all,
            "la firma non copre tutto il documento"
        );
        assert_eq!(signature.subfilter, "ETSI.CAdES.detached");
        assert_eq!(signature.signer, "OpenPDF Test");

        // Una modifica dopo la firma deve invalidare il digest.
        let mut tampered = std::fs::read(&dest).unwrap();
        let position = tampered.windows(9).position(|w| w == b"Contratto").unwrap();
        tampered[position] = b'X';
        let tampered_path = percorso_temporaneo("pades-tampered.pdf");
        std::fs::write(&tampered_path, tampered).unwrap();
        let signatures = verify_pdf(&tampered_path, None).unwrap();
        assert!(!signatures[0].digest_ok, "manomissione non rilevata");
    }

    #[test]
    fn firma_e_verifica_p12_rsa() {
        let p12 = fixture("test-rsa.p12");
        if !p12.exists() {
            eprintln!("fixture PKCS#12 assente: test saltato");
            return;
        }
        let src = percorso_temporaneo("openpdf-p12-src.pdf");
        let dest = percorso_temporaneo("openpdf-p12-firmato.pdf");
        std::fs::write(&src, sample_pdf()).unwrap();

        let credenziali = Credenziali::p12(p12, "test");
        let outcome = sign_pdf_con(&src, &dest, &credenziali, &opzioni("Niccolò Rossi")).unwrap();
        assert_eq!(outcome.signer, "OpenPDF P12 Test");

        let firme = verify_pdf(&dest, None).unwrap();
        assert_eq!(firme.len(), 1);
        assert!(firme[0].digest_ok, "digest P12 non valido");
        assert!(firme[0].signature_ok, "firma P12 non valida");
        assert!(firme[0].covers_all, "la firma P12 non copre il documento");

        // Il flusso deve restare un PDF leggibile da lopdf e avere /AP /N.
        let doc = Document::load(&dest).unwrap();
        let widget = widget_firmato(&doc);
        let ap = widget
            .get(b"AP")
            .ok()
            .and_then(|o| o.as_dict().ok())
            .expect("widget senza /AP");
        let ap_n = ap
            .get(b"N")
            .ok()
            .and_then(|o| o.as_reference().ok())
            .expect("/AP /N assente");
        let stream = doc.get_object(ap_n).and_then(|o| o.as_stream()).unwrap();
        assert_eq!(
            stream.dict.get(b"Subtype").unwrap().as_name().unwrap(),
            b"Form".as_slice()
        );
        assert!(
            stream.dict.get(b"BBox").is_ok(),
            "BBox assente nell'apparenza"
        );
        let testo_apparenza = String::from_utf8(stream.decompressed_content().unwrap()).unwrap();
        assert!(
            testo_apparenza.contains("Niccol\\362 Rossi"),
            "il nome accentato non è codificato in WinAnsi: {testo_apparenza}"
        );
    }

    #[test]
    fn firma_e_verifica_p12_legacy() {
        let p12 = fixture("test-rsa-legacy.p12");
        if !p12.exists() {
            eprintln!("fixture PKCS#12 legacy assente: test saltato");
            return;
        }
        let src = percorso_temporaneo("openpdf-p12-legacy-src.pdf");
        let dest = percorso_temporaneo("openpdf-p12-legacy-firmato.pdf");
        std::fs::write(&src, sample_pdf()).unwrap();

        let credenziali = Credenziali::p12(p12, "test");
        let outcome = sign_pdf_con(&src, &dest, &credenziali, &opzioni("Mario Rossi")).unwrap();
        assert_eq!(outcome.signer, "OpenPDF P12 Test");

        let firme = verify_pdf(&dest, None).unwrap();
        assert_eq!(firme.len(), 1);
        assert!(firme[0].digest_ok, "digest P12 legacy non valido");
        assert!(firme[0].signature_ok, "firma P12 legacy non valida");
    }

    #[test]
    fn password_p12_errata_segnalata() {
        let p12 = fixture("test-rsa.p12");
        if !p12.exists() {
            eprintln!("fixture PKCS#12 assente: test saltato");
            return;
        }
        let src = percorso_temporaneo("openpdf-p12-errata-src.pdf");
        std::fs::write(&src, sample_pdf()).unwrap();
        let dest = percorso_temporaneo("openpdf-p12-errata.pdf");
        let credenziali = Credenziali::p12(p12, "sbagliata");
        let errore = sign_pdf_con(&src, &dest, &credenziali, &opzioni("Mario Rossi")).unwrap_err();
        assert!(
            errore.contains("PKCS#12") && errore.contains("Password"),
            "messaggio poco chiaro: {errore}"
        );
    }

    #[test]
    fn firma_e_verifica_ec_p256() {
        let src = percorso_temporaneo("openpdf-ec256-src.pdf");
        let dest = percorso_temporaneo("openpdf-ec256-firmato.pdf");
        std::fs::write(&src, sample_pdf()).unwrap();
        if !fixture("test-ec256-cert.pem").exists() || !fixture("test-ec256-key.pem").exists() {
            eprintln!("fixture EC P-256 assente: test saltato");
            return;
        }

        let outcome = firma_con_pem(
            &src,
            &dest,
            "test-ec256-cert.pem",
            "test-ec256-key.pem",
            "Mario Rossi",
        );
        assert_eq!(outcome.signer, "OpenPDF EC256 Test");

        let firme = verify_pdf(&dest, None).unwrap();
        assert_eq!(firme.len(), 1);
        assert!(firme[0].digest_ok, "digest EC P-256 non valido");
        assert!(firme[0].signature_ok, "firma EC P-256 non valida");

        let signer = cms_firmatario(&dest);
        assert_eq!(
            signer.digest_algorithm.algorithm,
            HashKind::Sha256.oid(),
            "digestAlgorithm inatteso per P-256"
        );
        assert_eq!(
            signer.signature_algorithm.algorithm,
            oid(&[1, 2, 840, 10045, 4, 3, 2]),
            "signatureAlgorithm inatteso per P-256"
        );
        assert!(
            signer.signature_algorithm.parameters.is_none(),
            "ecdsa-with-SHA256 non deve avere parametri"
        );
    }

    #[test]
    fn firma_e_verifica_ec_p384() {
        let src = percorso_temporaneo("openpdf-ec384-src.pdf");
        let dest = percorso_temporaneo("openpdf-ec384-firmato.pdf");
        std::fs::write(&src, sample_pdf()).unwrap();
        if !fixture("test-ec384-cert.pem").exists() || !fixture("test-ec384-key.pem").exists() {
            eprintln!("fixture EC P-384 assente: test saltato");
            return;
        }

        let outcome = firma_con_pem(
            &src,
            &dest,
            "test-ec384-cert.pem",
            "test-ec384-key.pem",
            "Mario Rossi",
        );
        assert_eq!(outcome.signer, "OpenPDF EC384 Test");

        let firme = verify_pdf(&dest, None).unwrap();
        assert_eq!(firme.len(), 1);
        assert!(firme[0].digest_ok, "digest EC P-384 non valido");
        assert!(firme[0].signature_ok, "firma EC P-384 non valida");

        let signer = cms_firmatario(&dest);
        assert_eq!(
            signer.digest_algorithm.algorithm,
            HashKind::Sha384.oid(),
            "digestAlgorithm inatteso per P-384"
        );
        assert_eq!(
            signer.signature_algorithm.algorithm,
            oid(&[1, 2, 840, 10045, 4, 3, 3]),
            "signatureAlgorithm inatteso per P-384"
        );
    }

    #[test]
    fn certificato_non_corrispondente() {
        let src = percorso_temporaneo("openpdf-mismatch-src.pdf");
        let dest = percorso_temporaneo("openpdf-mismatch.pdf");
        std::fs::write(&src, sample_pdf()).unwrap();
        if !fixture("test-ec256-cert.pem").exists() || !fixture("test-key.pem").exists() {
            eprintln!("fixture assenti: test saltato");
            return;
        }
        let credenziali = Credenziali::pem(fixture("test-ec256-cert.pem"), fixture("test-key.pem"));
        let errore = sign_pdf_con(&src, &dest, &credenziali, &opzioni("Mario Rossi")).unwrap_err();
        assert!(
            errore.contains("non corrisponde"),
            "messaggio poco chiaro: {errore}"
        );
    }

    #[test]
    fn testo_pdf_codifica_winansi() {
        assert_eq!(testo_pdf("Niccolò"), "Niccol\\362");
        assert_eq!(testo_pdf("A (B)"), "A \\(B\\)");
        assert_eq!(testo_pdf("Roma · RM"), "Roma \\267 RM");
        assert_eq!(testo_pdf("Euro €"), "Euro ?");
        assert_eq!(testo_pdf("Linea\nnuova"), "Linea nuova");
    }

    #[test]
    fn docmdp_presente_quando_richiesto() {
        let src = percorso_temporaneo("openpdf-docmdp-src.pdf");
        let dest = percorso_temporaneo("openpdf-docmdp-firmato.pdf");
        std::fs::write(&src, sample_pdf()).unwrap();
        if !fixture("test-cert.pem").exists() || !fixture("test-key.pem").exists() {
            eprintln!("fixture del certificato assenti: test saltato");
            return;
        }

        let credenziali = Credenziali::pem(fixture("test-cert.pem"), fixture("test-key.pem"));
        let mut opts = opzioni("Mario Rossi");
        opts.certification = Some(2);
        let outcome = sign_pdf_con(&src, &dest, &credenziali, &opts).unwrap();
        assert!(!outcome.field.is_empty());

        let doc = Document::load(&dest).unwrap();
        let sig = firma_dict(&doc);
        let riferimenti = sig.get(b"Reference").unwrap().as_array().unwrap();
        assert_eq!(riferimenti.len(), 1);
        let riferimento = riferimenti[0].as_dict().unwrap();
        assert_eq!(
            riferimento
                .get(b"TransformMethod")
                .unwrap()
                .as_name()
                .unwrap(),
            b"DocMDP".as_slice()
        );
        let parametri = riferimento
            .get(b"TransformParams")
            .unwrap()
            .as_dict()
            .unwrap();
        assert_eq!(parametri.get(b"P").unwrap().as_i64().unwrap(), 2);
        assert_eq!(
            parametri.get(b"V").unwrap().as_name().unwrap(),
            b"1.2".as_slice()
        );

        // Il catalogo deve puntare alla firma DocMDP.
        let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        let catalogo = doc.get_dictionary(root).unwrap();
        let perms = catalogo.get(b"Perms").unwrap().as_dict().unwrap();
        let docmdp = perms.get(b"DocMDP").unwrap().as_reference().unwrap();
        assert_eq!(
            doc.get_dictionary(docmdp)
                .unwrap()
                .get(b"Type")
                .unwrap()
                .as_name()
                .unwrap(),
            b"Sig".as_slice()
        );

        // La firma di certificazione resta verificabile.
        let firme = verify_pdf(&dest, None).unwrap();
        assert!(firme[0].digest_ok);
        assert!(firme[0].signature_ok);
    }

    #[test]
    fn certificazione_fuori_intervallo_rifiutata() {
        let src = percorso_temporaneo("openpdf-docmdp-invalido-src.pdf");
        let dest = percorso_temporaneo("openpdf-docmdp-invalido.pdf");
        std::fs::write(&src, sample_pdf()).unwrap();
        if !fixture("test-cert.pem").exists() || !fixture("test-key.pem").exists() {
            eprintln!("fixture del certificato assenti: test saltato");
            return;
        }
        let credenziali = Credenziali::pem(fixture("test-cert.pem"), fixture("test-key.pem"));
        let mut opts = opzioni("Mario Rossi");
        opts.certification = Some(4);
        let errore = sign_pdf_con(&src, &dest, &credenziali, &opts).unwrap_err();
        assert!(
            errore.contains("1, 2 o 3"),
            "messaggio poco chiaro: {errore}"
        );
        assert!(
            !dest.exists(),
            "il PDF non deve essere scritto in caso di errore"
        );
    }

    #[test]
    fn seconda_firma_su_documento_gia_firmato() {
        let src = percorso_temporaneo("openpdf-seconda-src.pdf");
        let prima = percorso_temporaneo("openpdf-seconda-prima.pdf");
        let seconda = percorso_temporaneo("openpdf-seconda-firmata.pdf");
        std::fs::write(&src, sample_pdf()).unwrap();
        if !fixture("test-cert.pem").exists()
            || !fixture("test-key.pem").exists()
            || !fixture("test-ec256-cert.pem").exists()
            || !fixture("test-ec256-key.pem").exists()
        {
            eprintln!("fixture assenti: test saltato");
            return;
        }

        firma_con_pem(&src, &prima, "test-cert.pem", "test-key.pem", "Mario Rossi");
        firma_con_pem(
            &prima,
            &seconda,
            "test-ec256-cert.pem",
            "test-ec256-key.pem",
            "Anna Bianchi",
        );

        let firme = verify_pdf(&seconda, None).unwrap();
        assert_eq!(firme.len(), 2, "attese due firme nel documento");
        let nuova = firme
            .iter()
            .find(|firma| firma.signer == "OpenPDF EC256 Test")
            .expect("firma EC assente");
        assert!(nuova.digest_ok, "digest della seconda firma non valido");
        assert!(
            nuova.signature_ok,
            "firma CMS della seconda firma non valida"
        );
        assert!(nuova.covers_all, "la seconda firma non copre il documento");

        let doc = Document::load(&seconda).unwrap();
        let widget = doc
            .objects
            .values()
            .filter(|oggetto| {
                oggetto
                    .as_dict()
                    .map(|dict| {
                        dict.get(b"Subtype").ok().and_then(|s| s.as_name().ok())
                            == Some(b"Widget".as_slice())
                    })
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(widget, 2, "attesi due widget firmati");
    }

    #[test]
    fn apparenza_su_pagina_ruotata() {
        let src = percorso_temporaneo("openpdf-ruotata-src.pdf");
        let dest = percorso_temporaneo("openpdf-ruotata-firmato.pdf");
        std::fs::write(&src, sample_pdf_ruotato(90)).unwrap();
        if !fixture("test-cert.pem").exists() || !fixture("test-key.pem").exists() {
            eprintln!("fixture del certificato assenti: test saltato");
            return;
        }

        let outcome = firma_con_pem(&src, &dest, "test-cert.pem", "test-key.pem", "Mario Rossi");
        assert!(!outcome.field.is_empty());

        let doc = Document::load(&dest).unwrap();
        let widget = widget_firmato(&doc);
        let ap = widget.get(b"AP").unwrap().as_dict().unwrap();
        let ap_n = ap.get(b"N").unwrap().as_reference().unwrap();
        let stream = doc.get_object(ap_n).unwrap().as_stream().unwrap();
        let matrice = stream.dict.get(b"Matrix").unwrap().as_array().unwrap();
        assert_eq!(matrice.len(), 6);
        let valori: Vec<f32> = matrice.iter().map(|v| v.as_f32().unwrap()).collect();
        assert_ne!(
            (valori[0], valori[1], valori[2], valori[3]),
            (1.0, 0.0, 0.0, 1.0),
            "la pagina ruotata deve usare una matrice non identitaria"
        );

        let firme = verify_pdf(&dest, None).unwrap();
        assert!(firme[0].digest_ok);
        assert!(firme[0].signature_ok);
    }
}
