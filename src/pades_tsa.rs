//! Timestamp RFC 3161 per portare una firma PAdES da B-B a B-T.
//!
//! Il modulo costruisce una `TimeStampReq` (versione 1, `messageImprint`,
//! `certReq = TRUE`, nonce casuale), la invia alla TSA via `curl.exe` e
//! interpreta la `TimeStampResp`:
//! - controlla lo stato (`granted` / `grantedWithMods` / rifiuto con motivo);
//! - estrae il `TimeStampToken` DER senza ricodificarlo, così i byte inseriti
//!   nel PDF sono esattamente quelli emessi dalla TSA;
//! - confronta il `messageImprint` del `TSTInfo` con il digest fornito;
//! - se il token incorpora il certificato TSA, verifica la firma del token
//!   (RSA PKCS#1 v1.5 oppure ECDSA P-256/P-384);
//! - verifica che il nonce restituito coincida con quello richiesto.
//!
//! Il token va inserito in [`sign_pdf_con`](crate::pades::sign_pdf_con) come
//! attributo **non firmato** `signatureTimeStampToken`
//! (`1.2.840.113549.1.9.16.2.14`) del `SignerInfo`, il cui valore è il DER
//! completo del `TimeStampToken`. L'attributo non firmato non richiede di
//! rifare la firma CMS: è ciò che distingue B-T da B-B.
//!
//! Limiti dichiarati:
//! - si verifica solo la firma del token con il certificato incorporato, non la
//!   catena fino a una radice fidata né lo stato di revoca della TSA;
//! - sono supportate sole chiavi TSA RSA (PKCS#1 v1.5) ed EC P-256/P-384: i
//!   token firmati con RSA-PSS, Ed25519 o SHA-1 risultano non verificati
//!   (`signature_ok = false`) ma restano utilizzabili come timestamp;
//! - il nonce è generato con la casualità del sistema operativo disponibile in
//!   `std` (seed di `RandomState` più orologio e PID), non con un CSPRNG
//!   dedicato: serve a correlare richiesta e risposta, non come segreto;
//! - l'ora della TSA non viene confrontata con l'orologio locale: un
//!   `genTime` nel futuro è responsabilità del validatore;
//! - `reqPolicy` ed `extensions` non sono inviati.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rasn::prelude::*;
use rasn_cms::tsp::{MessageImprint, PkiStatus, TimeStampReq, TimeStampResp, TstInfo};
use rasn_cms::{CertificateChoices, ContentInfo, SignedData, SignerIdentifier, SignerInfo};
use rasn_pkix::{AlgorithmIdentifier, Certificate};
use rsa::pkcs1v15::{Signature as FirmaRsa, VerifyingKey as VerificaRsa};
use rsa::pkcs8::DecodePublicKey;
use rsa::signature::hazmat::PrehashVerifier;
use rsa::signature::Verifier;
use rsa::RsaPublicKey;
use sha2::{Digest, Sha256, Sha384, Sha512};

/// Timeout complessivo della chiamata HTTP, in secondi.
const TIMEOUT_SECONDI: u32 = 30;
/// Timeout della sola connessione, in secondi.
const TIMEOUT_CONNESSIONE: u32 = 10;
/// Dimensione massima accettata per la risposta della TSA (4 MiB).
const RISPOSTA_MASSIMA: u64 = 4 * 1024 * 1024;
/// Token timestamp RFC 3161 già emesso dalla TSA.
#[derive(Debug, Clone)]
pub struct TimestampToken {
    /// `TimeStampToken` DER completo, per l'attributo non firmato.
    pub der: Vec<u8>,
    /// `genTime` in forma leggibile (`AAAA-MM-GG hh:mm:ss UTC`).
    pub time: String,
    /// `messageImprint` corrisponde al digest fornito.
    pub imprint_ok: bool,
    /// Firma del token verificata con il certificato TSA incorporato.
    /// Resta `false` se il token non contiene il certificato.
    pub signature_ok: bool,
    /// Subject CN del firmatario TSA, se il certificato è presente.
    pub tsa: Option<String>,
}

/// Chiede un timestamp alla TSA indicata e verifica il token ricevuto.
///
/// `imprint` è il digest del valore della firma CMS (i byte DER del
/// `SignedData` che finiscono in `/Contents`), calcolato con `hash`
/// (`sha256`, `sha384` o `sha512`).
///
/// Restituisce un errore se l'URL non è valido, se `curl` non riesce a
/// contattare la TSA, se la risposta non è un `TimeStampResp` DER/BER, se la
/// TSA rifiuta la richiesta, se il token manca o se il token non copre
/// l'impronta/il nonce inviati. Un token con certificato ma firma non valida
/// viene rifiutato.
pub fn request_timestamp(url: &str, imprint: &[u8], hash: &str) -> Result<TimestampToken, String> {
    let url = url.trim();
    valida_url(url)?;
    let hash = HashKind::da_nome(hash)?;
    controlla_impronta(imprint, hash)?;

    let nonce = nonce_casuale();
    let richiesta = costruisci_richiesta(hash, imprint, nonce)?;
    let risposta = invia_richiesta(url, &richiesta)?;
    let esito = interpreta_risposta(&risposta, imprint, hash, Some(nonce))?;

    if !esito.token.imprint_ok {
        return Err(
            "Il token della TSA non copre l'impronta inviata (messageImprint diverso).".into(),
        );
    }
    if esito.certificato_presente && !esito.token.signature_ok {
        return Err("La firma del token TSA non è valida rispetto al certificato incorporato.".into());
    }
    Ok(esito.token)
}

/// Verifica offline un `TimeStampToken` DER già ottenuto.
///
/// Controlla struttura CMS, `TSTInfo`, impronta e, quando il certificato TSA è
/// incorporato, la firma del token. Non verifica la catena né la revoca.
///
/// Se il token è ben formato ma copre un'altra impronta il risultato è `Ok` con
/// `imprint_ok = false`: chi lo incorpora in una firma PAdES deve pretendere
/// che il campo sia `true`. Un token non decodificabile produce invece un
/// errore.
pub fn verify_timestamp(token_der: &[u8], imprint: &[u8], hash: &str) -> Result<TimestampToken, String> {
    let hash = HashKind::da_nome(hash)?;
    controlla_impronta(imprint, hash)?;
    Ok(interpreta_token(token_der, imprint, hash, None)?.token)
}

// -- Algoritmi di digest ------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HashKind {
    Sha256,
    Sha384,
    Sha512,
}

impl HashKind {
    fn da_nome(nome: &str) -> Result<Self, String> {
        let normalizzato = nome.trim().to_ascii_lowercase().replace('-', "");
        match normalizzato.as_str() {
            "sha256" => Ok(HashKind::Sha256),
            "sha384" => Ok(HashKind::Sha384),
            "sha512" => Ok(HashKind::Sha512),
            _ => Err(format!(
                "Algoritmo di hash non supportato: \"{nome}\". Usare SHA-256, SHA-384 o SHA-512."
            )),
        }
    }

    fn da_oid(id: &ObjectIdentifier) -> Option<Self> {
        if *id == oid(&[2, 16, 840, 1, 101, 3, 4, 2, 1]) {
            Some(HashKind::Sha256)
        } else if *id == oid(&[2, 16, 840, 1, 101, 3, 4, 2, 2]) {
            Some(HashKind::Sha384)
        } else if *id == oid(&[2, 16, 840, 1, 101, 3, 4, 2, 3]) {
            Some(HashKind::Sha512)
        } else {
            None
        }
    }

    fn oid(self) -> ObjectIdentifier {
        match self {
            HashKind::Sha256 => oid(&[2, 16, 840, 1, 101, 3, 4, 2, 1]),
            HashKind::Sha384 => oid(&[2, 16, 840, 1, 101, 3, 4, 2, 2]),
            HashKind::Sha512 => oid(&[2, 16, 840, 1, 101, 3, 4, 2, 3]),
        }
    }

    fn algorithm(self) -> AlgorithmIdentifier {
        AlgorithmIdentifier {
            algorithm: self.oid(),
            parameters: Some(Any::new(vec![0x05, 0x00])),
        }
    }

    fn lunghezza(self) -> usize {
        match self {
            HashKind::Sha256 => 32,
            HashKind::Sha384 => 48,
            HashKind::Sha512 => 64,
        }
    }

    fn nome(self) -> &'static str {
        match self {
            HashKind::Sha256 => "SHA-256",
            HashKind::Sha384 => "SHA-384",
            HashKind::Sha512 => "SHA-512",
        }
    }

    fn digest(self, dati: &[u8]) -> Vec<u8> {
        match self {
            HashKind::Sha256 => Sha256::digest(dati).to_vec(),
            HashKind::Sha384 => Sha384::digest(dati).to_vec(),
            HashKind::Sha512 => Sha512::digest(dati).to_vec(),
        }
    }
}

fn oid(archi: &[u32]) -> ObjectIdentifier {
    ObjectIdentifier::new(archi.to_vec()).expect("OID valido")
}

fn oid_signed_data() -> ObjectIdentifier {
    oid(&[1, 2, 840, 113549, 1, 7, 2])
}

fn oid_tst_info() -> ObjectIdentifier {
    oid(&[1, 2, 840, 113549, 1, 9, 16, 1, 4])
}

fn oid_rsa_encryption() -> ObjectIdentifier {
    oid(&[1, 2, 840, 113549, 1, 1, 1])
}

fn oid_ec_public_key() -> ObjectIdentifier {
    oid(&[1, 2, 840, 10045, 2, 1])
}

fn oid_subject_key_identifier() -> ObjectIdentifier {
    oid(&[2, 5, 29, 14])
}

fn controlla_impronta(imprint: &[u8], hash: HashKind) -> Result<(), String> {
    if imprint.len() != hash.lunghezza() {
        return Err(format!(
            "Impronta di {} byte incompatibile con {}: ne servono {}.",
            imprint.len(),
            hash.nome(),
            hash.lunghezza()
        ));
    }
    Ok(())
}

fn valida_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("URL della TSA vuoto.".into());
    }
    let minuscola = url.to_ascii_lowercase();
    if !(minuscola.starts_with("http://") || minuscola.starts_with("https://")) {
        return Err(format!(
            "URL della TSA non valido: serve un indirizzo http:// o https:// (ricevuto \"{url}\")."
        ));
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("L'URL della TSA contiene spazi o caratteri di controllo.".into());
    }
    Ok(())
}

// -- Costruzione della richiesta ---------------------------------------------

fn costruisci_richiesta(hash: HashKind, imprint: &[u8], nonce: u64) -> Result<Vec<u8>, String> {
    let richiesta = TimeStampReq {
        version: 1,
        message_imprint: MessageImprint {
            hash_algorithm: hash.algorithm(),
            hashed_message: OctetString::from(imprint.to_vec()),
        },
        req_policy: None,
        nonce: Some(Integer::from(nonce)),
        cert_req: true,
        extensions: None,
    };
    rasn::der::encode(&richiesta).map_err(|e| format!("Richiesta timestamp non codificabile: {e}"))
}

fn nonce_casuale() -> u64 {
    let mut hasher = RandomState::new().build_hasher();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|durata| durata.as_nanos())
        .unwrap_or(0);
    hasher.write_u128(nanos);
    hasher.write_u32(std::process::id());
    // Nonce positivo a 63 bit, mai zero.
    (hasher.finish() & 0x7FFF_FFFF_FFFF_FFFF) | 1
}

// -- Invio HTTP tramite curl --------------------------------------------------

static CONTATORE_TEMP: AtomicU64 = AtomicU64::new(0);

/// File temporaneo cancellato automaticamente, anche in caso di errore.
struct FileTemporaneo {
    percorso: PathBuf,
}

impl FileTemporaneo {
    fn nuovo(estensione: &str) -> Self {
        let progressivo = CONTATORE_TEMP.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|durata| durata.as_nanos())
            .unwrap_or(0);
        let nome = format!(
            "openpdf-tsa-{}-{progressivo}-{nanos}.{estensione}",
            std::process::id()
        );
        FileTemporaneo {
            percorso: std::env::temp_dir().join(nome),
        }
    }

    fn percorso(&self) -> &Path {
        &self.percorso
    }
}

impl Drop for FileTemporaneo {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.percorso);
    }
}

fn curl_eseguibile() -> PathBuf {
    // Su Windows 11 curl.exe è in System32; si preferisce il percorso assoluto
    // per non dipendere dall'ordine delle directory nel PATH.
    if let Some(radice) = std::env::var_os("SystemRoot") {
        let candidato = PathBuf::from(radice)
            .join("System32")
            .join("curl.exe");
        if candidato.is_file() {
            return candidato;
        }
    }
    PathBuf::from("curl.exe")
}

fn invia_richiesta(url: &str, corpo: &[u8]) -> Result<Vec<u8>, String> {
    let richiesta = FileTemporaneo::nuovo("tsq");
    let risposta = FileTemporaneo::nuovo("tsr");
    std::fs::write(richiesta.percorso(), corpo)
        .map_err(|e| format!("Scrittura della richiesta TSA non riuscita: {e}"))?;

    let mut comando = Command::new(curl_eseguibile());
    comando
        .arg("--silent")
        .arg("--show-error")
        .arg("--connect-timeout")
        .arg(TIMEOUT_CONNESSIONE.to_string())
        .arg("--max-time")
        .arg(TIMEOUT_SECONDI.to_string())
        .arg("--header")
        .arg("Content-Type: application/timestamp-query")
        .arg("--header")
        .arg("Accept: application/timestamp-reply")
        .arg("--data-binary")
        .arg(format!("@{}", richiesta.percorso().display()))
        .arg("--output")
        .arg(risposta.percorso())
        .arg("--write-out")
        .arg("%{http_code}")
        .arg("--url")
        .arg(url);

    // In un'applicazione grafica non deve comparire una finestra di console.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        comando.creation_flags(0x0800_0000);
    }

    let uscita = comando.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "curl.exe non trovato: è necessario per inviare la richiesta alla TSA.".to_string()
        } else {
            format!("Avvio di curl per la richiesta TSA non riuscito: {e}")
        }
    })?;

    let codice_testo = String::from_utf8_lossy(&uscita.stdout).trim().to_string();
    let errore_curl = String::from_utf8_lossy(&uscita.stderr)
        .replace(['\r', '\n'], " ");
    let errore_curl = errore_curl.trim();

    if !uscita.status.success() {
        let dettaglio = if errore_curl.is_empty() {
            format!("codice {:?}", uscita.status.code())
        } else {
            errore_curl.to_string()
        };
        return Err(format!(
            "Invio della richiesta TSA non riuscito: {dettaglio}."
        ));
    }

    let codice: u16 = codice_testo.parse().map_err(|_| {
        format!("Risposta HTTP della TSA senza codice di stato valido (\"{codice_testo}\").")
    })?;

    let dimensione = std::fs::metadata(risposta.percorso())
        .map(|meta| meta.len())
        .unwrap_or(0);
    if dimensione > RISPOSTA_MASSIMA {
        return Err(format!(
            "Risposta della TSA troppo grande ({dimensione} byte, limite {RISPOSTA_MASSIMA})."
        ));
    }

    if !(200..300).contains(&codice) {
        let corpo = std::fs::read(risposta.percorso()).unwrap_or_default();
        return Err(format!(
            "La TSA ha risposto con HTTP {codice}: {}.",
            anteprima(&corpo)
        ));
    }

    if dimensione == 0 {
        return Err(format!(
            "La TSA ha risposto HTTP {codice} senza alcun contenuto."
        ));
    }

    let dati = std::fs::read(risposta.percorso())
        .map_err(|e| format!("Lettura della risposta TSA non riuscita: {e}"))?;
    if dati.is_empty() {
        return Err("La TSA ha restituito una risposta vuota.".into());
    }
    Ok(dati)
}

fn anteprima(corpo: &[u8]) -> String {
    let taglio = &corpo[..corpo.len().min(200)];
    let testo = String::from_utf8_lossy(taglio).replace(['\r', '\n', '\t'], " ");
    let compatto = testo.split_whitespace().collect::<Vec<_>>().join(" ");
    if compatto.is_empty() {
        "corpo assente o binario".into()
    } else {
        compatto
    }
}

// -- Lettura della risposta ---------------------------------------------------

#[derive(Debug)]
struct EsitoToken {
    token: TimestampToken,
    /// Il token incorpora almeno un certificato (la firma è verificabile).
    certificato_presente: bool,
}

fn interpreta_risposta(
    risposta: &[u8],
    imprint: &[u8],
    hash: HashKind,
    nonce_atteso: Option<u64>,
) -> Result<EsitoToken, String> {
    let decodificata = rasn::der::decode::<TimeStampResp>(risposta)
        .or_else(|errore_der| {
            rasn::ber::decode::<TimeStampResp>(risposta)
                .map_err(|_| errore_der)
        })
        .map_err(|e| {
            format!("Risposta della TSA non decodificabile come TimeStampResp DER/BER: {e}")
        })?;

    let stato = decodificata.status.status;
    match stato {
        PkiStatus::Granted | PkiStatus::GrantedWithMods => {}
        altro => {
            let testo = decodificata
                .status
                .status_string
                .as_ref()
                .map(|righe| {
                    righe
                        .iter()
                        .map(|riga| String::from_utf8_lossy(riga.as_bytes()).to_string())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            let motivo = if testo.trim().is_empty() {
                if decodificata.status.fail_info.is_some() {
                    "la TSA ha indicato un motivo di errore (failInfo)".to_string()
                } else {
                    "nessun dettaglio fornito".to_string()
                }
            } else {
                testo.trim().to_string()
            };
            return Err(format!(
                "La TSA ha rifiutato la richiesta (stato {}): {motivo}.",
                codice_stato(altro)
            ));
        }
    }

    let token_der = estrai_token(risposta)
        .or_else(|| {
            decodificata
                .time_stamp_token
                .as_ref()
                .and_then(|token| rasn::der::encode(token).ok())
        })
        .ok_or("La risposta della TSA non contiene alcun TimeStampToken.")?;

    interpreta_token(&token_der, imprint, hash, nonce_atteso)
}

fn codice_stato(stato: PkiStatus) -> i32 {
    match stato {
        PkiStatus::Granted => 0,
        PkiStatus::GrantedWithMods => 1,
        PkiStatus::Rejection => 2,
        PkiStatus::Waiting => 3,
        PkiStatus::RevocationWarning => 4,
        PkiStatus::RevocationNotification => 5,
    }
}

// -- Interpretazione e verifica del token ------------------------------------

fn interpreta_token(
    token_der: &[u8],
    imprint: &[u8],
    hash: HashKind,
    nonce_atteso: Option<u64>,
) -> Result<EsitoToken, String> {
    let token_der = delimita_token(token_der);
    if token_der.is_empty() {
        return Err("Token timestamp vuoto.".into());
    }

    let content_info = rasn::der::decode::<ContentInfo>(token_der)
        .map_err(|e| format!("Token timestamp non decodificabile come CMS ContentInfo: {e}"))?;
    if content_info.content_type != oid_signed_data() {
        return Err(format!(
            "Il token non è un SignedData CMS (contentType {}).",
            content_info.content_type
        ));
    }

    let signed_data = rasn::der::decode::<SignedData>(content_info.content.as_bytes())
        .map_err(|e| format!("SignedData del token non decodificabile: {e}"))?;
    if signed_data.encap_content_info.content_type != oid_tst_info() {
        return Err(format!(
            "Il token non contiene un TSTInfo (contentType {}).",
            signed_data.encap_content_info.content_type
        ));
    }
    let econtent = signed_data
        .encap_content_info
        .content
        .as_ref()
        .ok_or("Il token non contiene il TSTInfo (eContent assente).")?;

    let tst = rasn::der::decode::<TstInfo>(&econtent[..])
        .map_err(|e| format!("TSTInfo del token non decodificabile: {e}"))?;
    if tst.version != Integer::from(1u8) {
        return Err(format!(
            "Versione TSTInfo non supportata: {} (attesa 1).",
            tst.version
        ));
    }

    if let Some(atteso) = nonce_atteso {
        if tst.nonce.as_ref() != Some(&Integer::from(atteso)) {
            return Err(
                "Il nonce del token TSA non corrisponde a quello della richiesta.".into(),
            );
        }
    }

    let impronta_corrisponde = tst.message_imprint.hash_algorithm.algorithm == hash.oid()
        && &tst.message_imprint.hashed_message[..] == imprint;

    let signer_info = signed_data
        .signer_infos
        .iter()
        .next()
        .ok_or("Il token non contiene alcun SignerInfo.")?;
    let certificato = certificato_firmatario(&signed_data, signer_info);
    let certificato_presente = signed_data
        .certificates
        .as_ref()
        .map(|insieme| !insieme.is_empty())
        .unwrap_or(false);

    let signature_ok = certificato
        .map(|cert| verifica_firma_token(cert, signer_info, &econtent[..]))
        .unwrap_or(false);

    let time = tst
        .gen_time
        .with_timezone(&chrono::Utc)
        .format("%Y-%m-%d %H:%M:%S UTC")
        .to_string();

    let tsa = certificato.and_then(common_name);

    Ok(EsitoToken {
        token: TimestampToken {
            der: token_der.to_vec(),
            time,
            imprint_ok: impronta_corrisponde,
            signature_ok,
            tsa,
        },
        certificato_presente,
    })
}

/// Delimita il token DER ignorando l'eventuale riempimento a zero usato nel
/// campo `/Contents` di un PDF. La lunghezza dichiarata dal TLV esterno è
/// l'unico criterio affidabile: la firma può terminare con byte zero.
fn delimita_token(dati: &[u8]) -> &[u8] {
    match leggi_tlv(dati) {
        Ok(elemento) if elemento.tag == 0x30 => &dati[..elemento.codifica.len()],
        _ => trim_zeri_finali(dati),
    }
}

fn trim_zeri_finali(dati: &[u8]) -> &[u8] {
    match dati.iter().rposition(|byte| *byte != 0) {
        Some(indice) => &dati[..=indice],
        None => &[],
    }
}

fn certificato_firmatario<'a>(
    signed_data: &'a SignedData,
    signer_info: &SignerInfo,
) -> Option<&'a Certificate> {
    let insieme = signed_data.certificates.as_ref()?;
    let mut primo: Option<&Certificate> = None;
    let mut totale = 0usize;
    for scelta in insieme.iter() {
        let CertificateChoices::Certificate(certificato) = scelta else {
            continue;
        };
        totale += 1;
        let certificato = certificato.as_ref();
        if primo.is_none() {
            primo = Some(certificato);
        }
        match &signer_info.sid {
            SignerIdentifier::IssuerAndSerialNumber(sid) => {
                if certificato.tbs_certificate.issuer == sid.issuer
                    && certificato.tbs_certificate.serial_number == sid.serial_number
                {
                    return Some(certificato);
                }
            }
            SignerIdentifier::SubjectKeyIdentifier(ski) => {
                let atteso = &ski[..];
                if chiave_soggetto(certificato)
                    .map(|chiave| chiave == atteso)
                    .unwrap_or(false)
                {
                    return Some(certificato);
                }
            }
        }
    }
    // Con un solo certificato l'identificazione è di fatto univoca anche se il
    // SubjectKeyIdentifier non è presente o usa un'altra codifica.
    if totale == 1 {
        primo
    } else {
        None
    }
}

fn chiave_soggetto(cert: &Certificate) -> Option<Vec<u8>> {
    let estensioni = cert.tbs_certificate.extensions.as_ref()?;
    for estensione in estensioni.iter() {
        if estensione.extn_id != oid_subject_key_identifier() {
            continue;
        }
        let valore = rasn::der::decode::<OctetString>(&estensione.extn_value[..]).ok()?;
        return Some(valore.to_vec());
    }
    None
}

fn verifica_firma_token(
    certificato: &Certificate,
    signer_info: &SignerInfo,
    econtent: &[u8],
) -> bool {
    let Some(hash) = HashKind::da_oid(&signer_info.digest_algorithm.algorithm) else {
        return false;
    };
    let messaggio = match &signer_info.signed_attrs {
        Some(attributi) => match rasn::der::encode(attributi) {
            Ok(bytes) => bytes,
            Err(_) => return false,
        },
        None => econtent.to_vec(),
    };
    let Ok(spki) = rasn::der::encode(&certificato.tbs_certificate.subject_public_key_info) else {
        return false;
    };
    let algoritmo = &certificato
        .tbs_certificate
        .subject_public_key_info
        .algorithm
        .algorithm;
    let firma = &signer_info.signature[..];
    if *algoritmo == oid_rsa_encryption() {
        verifica_rsa(&spki, hash, &messaggio, firma)
    } else if *algoritmo == oid_ec_public_key() {
        verifica_ecdsa(&spki, hash, &messaggio, firma)
    } else {
        false
    }
}

fn verifica_rsa(spki: &[u8], hash: HashKind, messaggio: &[u8], firma: &[u8]) -> bool {
    let Ok(chiave) = RsaPublicKey::from_public_key_der(spki) else {
        return false;
    };
    let Ok(firma) = FirmaRsa::try_from(firma) else {
        return false;
    };
    match hash {
        HashKind::Sha256 => VerificaRsa::<Sha256>::new(chiave)
            .verify(messaggio, &firma)
            .is_ok(),
        HashKind::Sha384 => VerificaRsa::<Sha384>::new(chiave)
            .verify(messaggio, &firma)
            .is_ok(),
        HashKind::Sha512 => VerificaRsa::<Sha512>::new(chiave)
            .verify(messaggio, &firma)
            .is_ok(),
    }
}

fn verifica_ecdsa(spki: &[u8], hash: HashKind, messaggio: &[u8], firma: &[u8]) -> bool {
    let digest = hash.digest(messaggio);
    if let Ok(chiave) = p256::ecdsa::VerifyingKey::from_public_key_der(spki) {
        if let Ok(firma) = p256::ecdsa::DerSignature::from_bytes(firma) {
            if chiave.verify_prehash(&digest, &firma).is_ok() {
                return true;
            }
        }
    }
    if let Ok(chiave) = p384::ecdsa::VerifyingKey::from_public_key_der(spki) {
        if let Ok(firma) = p384::ecdsa::DerSignature::from_bytes(firma) {
            if chiave.verify_prehash(&digest, &firma).is_ok() {
                return true;
            }
        }
    }
    false
}

fn any_to_string(bytes: &[u8]) -> String {
    if bytes.len() < 2 {
        return String::new();
    }
    let tag = bytes[0];
    let mut indice = 2usize;
    let mut lunghezza = bytes[1] as usize;
    if lunghezza & 0x80 != 0 {
        let conteggio = lunghezza & 0x7F;
        lunghezza = 0;
        for _ in 0..conteggio {
            if indice >= bytes.len() {
                break;
            }
            lunghezza = (lunghezza << 8) | bytes[indice] as usize;
            indice += 1;
        }
    }
    let fine = (indice + lunghezza).min(bytes.len());
    let contenuto = &bytes[indice..fine];
    if tag == 0x1E {
        let unita: Vec<u16> = contenuto
            .chunks(2)
            .map(|pezzo| {
                if pezzo.len() == 2 {
                    ((pezzo[0] as u16) << 8) | pezzo[1] as u16
                } else {
                    pezzo[0] as u16
                }
            })
            .collect();
        String::from_utf16_lossy(&unita)
    } else {
        String::from_utf8_lossy(contenuto).to_string()
    }
}

fn common_name(cert: &Certificate) -> Option<String> {
    let rasn_pkix::Name::RdnSequence(rdns) = &cert.tbs_certificate.subject;
    for rdn in rdns.iter() {
        for attributo in rdn.iter() {
            if attributo.r#type == oid(&[2, 5, 4, 3]) {
                let valore = any_to_string(attributo.value.as_bytes());
                if !valore.trim().is_empty() {
                    return Some(valore);
                }
            }
        }
    }
    None
}

// -- Estrazione del token dalla risposta -------------------------------------

struct Tlv<'a> {
    tag: u8,
    contenuto: &'a [u8],
    codifica: &'a [u8],
}

fn leggi_tlv(ber: &[u8]) -> Result<Tlv<'_>, String> {
    if ber.len() < 2 {
        return Err("TLV ASN.1 troncato.".into());
    }
    let tag = ber[0];
    if tag & 0x1F == 0x1F {
        return Err("Tag ASN.1 multibyte non supportato.".into());
    }
    let primo = ber[1];
    let mut indice = 2usize;
    let lunghezza = if primo & 0x80 == 0 {
        primo as usize
    } else {
        let conteggio = (primo & 0x7F) as usize;
        if conteggio == 0 {
            return Err("Lunghezza ASN.1 indefinita non ammessa in DER.".into());
        }
        if conteggio > 4 {
            return Err("Lunghezza ASN.1 troppo grande.".into());
        }
        let mut valore = 0usize;
        for _ in 0..conteggio {
            if indice >= ber.len() {
                return Err("TLV ASN.1 troncato.".into());
            }
            valore = (valore << 8) | ber[indice] as usize;
            indice += 1;
        }
        valore
    };
    let fine = indice
        .checked_add(lunghezza)
        .ok_or("Lunghezza ASN.1 fuori intervallo.")?;
    if fine > ber.len() {
        return Err("TLV ASN.1 troncato.".into());
    }
    Ok(Tlv {
        tag,
        contenuto: &ber[indice..fine],
        codifica: &ber[..fine],
    })
}

fn estrai_token(risposta: &[u8]) -> Option<Vec<u8>> {
    let esterno = leggi_tlv(risposta).ok()?;
    if esterno.tag != 0x30 {
        return None;
    }
    let mut resto = esterno.contenuto;
    let mut indice = 0usize;
    while !resto.is_empty() && indice < 2 {
        let elemento = leggi_tlv(resto).ok()?;
        if indice == 1 {
            if elemento.tag != 0x30 {
                return None;
            }
            return Some(elemento.codifica.to_vec());
        }
        resto = &resto[elemento.codifica.len()..];
        indice += 1;
    }
    None
}

// -- Test --------------------------------------------------------------------

#[cfg(test)]
mod test {
    use super::*;
    use rasn_cms::{
        DigestAlgorithmIdentifiers, EncapsulatedContentInfo, IssuerAndSerialNumber, SignerInfos,
    };
    use std::path::PathBuf;
    use std::process::Stdio;

    fn impronta_di(dati: &[u8], hash: HashKind) -> Vec<u8> {
        hash.digest(dati)
    }

    #[test]
    fn richiesta_der_contiene_i_campi_rfc3161() {
        let impronta = [0x5Au8; 32];
        let nonce = 0x0123_4567_89AB_CDEFu64;
        let der = costruisci_richiesta(HashKind::Sha256, &impronta, nonce)
            .expect("la richiesta deve essere codificabile");

        assert_eq!(der[0], 0x30, "la richiesta inizia con SEQUENCE");
        let richiesta: TimeStampReq =
            rasn::der::decode(&der).expect("la richiesta deve essere DER valido");

        assert_eq!(richiesta.version, 1);
        assert_eq!(
            richiesta.message_imprint.hash_algorithm.algorithm,
            HashKind::Sha256.oid()
        );
        assert_eq!(
            richiesta.message_imprint.hashed_message.to_vec(),
            impronta.to_vec()
        );
        assert!(richiesta.cert_req, "certReq deve essere TRUE");
        assert_eq!(richiesta.nonce, Some(Integer::from(nonce)));

        let parametri = richiesta
            .message_imprint
            .hash_algorithm
            .parameters
            .as_ref()
            .expect("SHA-256 deve avere parametri NULL");
        assert_eq!(parametri.as_bytes(), &[0x05, 0x00]);
        assert!(richiesta.req_policy.is_none());
        assert!(richiesta.extensions.is_none());
    }

    #[test]
    fn impronta_di_lunghezza_errata_viene_rifiutata() {
        // Il controllo è esplicito all'ingresso delle API pubbliche.
        let errore = request_timestamp("http://127.0.0.1:1", &[0u8; 20], "sha256")
            .expect_err("impronta da 20 byte non valida per SHA-256");
        assert!(errore.contains("Impronta"), "messaggio poco chiaro: {errore}");
    }

    #[test]
    fn url_non_valido_viene_rifiutato_prima_della_rete() {
        let errore = request_timestamp("file:///etc/passwd", &[0u8; 32], "sha256")
            .expect_err("schema non http deve essere rifiutato");
        assert!(errore.contains("URL"), "messaggio poco chiaro: {errore}");

        let errore = request_timestamp("   ", &[0u8; 32], "sha256")
            .expect_err("URL vuoto deve essere rifiutato");
        assert!(errore.contains("URL"), "messaggio poco chiaro: {errore}");
    }

    #[test]
    fn hash_non_supportato_viene_rifiutato() {
        let errore = verify_timestamp(b"qualsiasi", &[0u8; 20], "md5")
            .expect_err("MD5 non è supportato");
        assert!(
            errore.contains("non supportato"),
            "messaggio poco chiaro: {errore}"
        );
    }

    #[test]
    fn rete_irraggiungibile_non_manda_in_panic() {
        let esito = request_timestamp("http://127.0.0.1:1", &[0u8; 32], "sha256");
        let errore = esito.expect_err("la connessione a 127.0.0.1:1 deve fallire");
        assert!(
            errore.contains("TSA") || errore.contains("curl"),
            "errore di rete poco chiaro: {errore}"
        );
    }

    #[test]
    fn risposta_rifiutata_riporta_il_motivo() {
        let risposta = TimeStampResp {
            status: rasn_cms::tsp::PkiStatusInfo {
                status: PkiStatus::Rejection,
                status_string: Some(vec!["policy non supportata".to_string()]),
                fail_info: None,
            },
            time_stamp_token: None,
        };
        let der = rasn::der::encode(&risposta).expect("TimeStampResp codificabile");
        let errore = interpreta_risposta(&der, &[0u8; 32], HashKind::Sha256, Some(1))
            .expect_err("lo stato 2 è un rifiuto");
        assert!(
            errore.contains("rifiutato"),
            "messaggio poco chiaro: {errore}"
        );
        assert!(
            errore.contains("policy non supportata"),
            "il motivo della TSA deve essere riportato: {errore}"
        );
    }

    #[test]
    fn risposta_senza_token_restituisce_errore() {
        let risposta = TimeStampResp {
            status: rasn_cms::tsp::PkiStatusInfo {
                status: PkiStatus::Granted,
                status_string: None,
                fail_info: None,
            },
            time_stamp_token: None,
        };
        let der = rasn::der::encode(&risposta).expect("TimeStampResp codificabile");
        let errore = interpreta_risposta(&der, &[0u8; 32], HashKind::Sha256, Some(1))
            .expect_err("granted senza token non è una risposta valida");
        assert!(
            errore.contains("TimeStampToken"),
            "messaggio poco chiaro: {errore}"
        );
    }

    #[test]
    fn token_malformato_restituisce_errore_chiaro() {
        let errore = verify_timestamp(b"non un token DER", &[0u8; 32], "sha256")
            .expect_err("un testo qualsiasi non è un token");
        assert!(
            errore.contains("Token timestamp"),
            "messaggio poco chiaro: {errore}"
        );

        let errore = verify_timestamp(&[], &[0u8; 32], "sha256")
            .expect_err("un token vuoto non è valido");
        assert!(errore.contains("vuoto"), "messaggio poco chiaro: {errore}");
    }

    #[test]
    fn token_incompleto_viene_rifiutato() {
        // ContentInfo SignedData senza TSTInfo: struttura CMS valida ma non
        // conforme a RFC 3161.
        let signed_data = SignedData {
            version: Integer::from(3u8),
            digest_algorithms: DigestAlgorithmIdentifiers::from_vec(vec![HashKind::Sha256
                .algorithm()]),
            encap_content_info: EncapsulatedContentInfo {
                content_type: oid(&[1, 2, 840, 113549, 1, 7, 1]),
                content: None,
            },
            certificates: None,
            crls: None,
            signer_infos: SignerInfos::from_vec(Vec::new()),
        };
        let signed_der = rasn::der::encode(&signed_data).expect("SignedData codificabile");
        let content_info = ContentInfo {
            content_type: oid_signed_data(),
            content: Any::new(signed_der),
        };
        let token = rasn::der::encode(&content_info).expect("ContentInfo codificabile");
        let errore = verify_timestamp(&token, &[0u8; 32], "sha256")
            .expect_err("senza TSTInfo il token non è valido");
        assert!(
            errore.contains("TSTInfo") || errore.contains("SignerInfo"),
            "messaggio poco chiaro: {errore}"
        );
    }

    /// Costruisce un token sintetico conforme nella struttura ma senza
    /// certificato e senza firma valida: serve a esercitare la lettura del
    /// `TSTInfo` e il confronto dell'impronta senza dipendere da una TSA.
    fn token_sintetico(impronta: &[u8], nonce: u64, gen_time: &str) -> Vec<u8> {
        let tst = TstInfo {
            version: Integer::from(1u8),
            policy: oid(&[1, 2, 3, 4, 1]),
            message_imprint: MessageImprint {
                hash_algorithm: HashKind::Sha256.algorithm(),
                hashed_message: OctetString::from(impronta.to_vec()),
            },
            serial_number: Integer::from(42u8),
            gen_time: chrono::DateTime::parse_from_rfc3339(gen_time).expect("data di prova"),
            accuracy: None,
            ordering: false,
            nonce: Some(Integer::from(nonce)),
            tsa: None,
            extensions: None,
        };
        let tst_der = rasn::der::encode(&tst).expect("TSTInfo codificabile");

        let firmatario = SignerInfo {
            version: Integer::from(1u8),
            sid: SignerIdentifier::IssuerAndSerialNumber(IssuerAndSerialNumber {
                issuer: rasn_pkix::Name::RdnSequence(Vec::new()),
                serial_number: Integer::from(7u8),
            }),
            digest_algorithm: HashKind::Sha256.algorithm(),
            signed_attrs: None,
            signature_algorithm: AlgorithmIdentifier {
                algorithm: oid_rsa_encryption(),
                parameters: Some(Any::new(vec![0x05, 0x00])),
            },
            signature: OctetString::from(vec![0u8; 32]),
            unsigned_attrs: None,
        };

        let signed_data = SignedData {
            version: Integer::from(3u8),
            digest_algorithms: DigestAlgorithmIdentifiers::from_vec(vec![HashKind::Sha256
                .algorithm()]),
            encap_content_info: EncapsulatedContentInfo {
                content_type: oid_tst_info(),
                content: Some(OctetString::from(tst_der)),
            },
            certificates: None,
            crls: None,
            signer_infos: SignerInfos::from_vec(vec![firmatario]),
        };
        let signed_der = rasn::der::encode(&signed_data).expect("SignedData codificabile");
        let content_info = ContentInfo {
            content_type: oid_signed_data(),
            content: Any::new(signed_der),
        };
        rasn::der::encode(&content_info).expect("ContentInfo codificabile")
    }

    #[test]
    fn token_sintetico_espone_tempo_e_impronta() {
        let impronta = [0x11u8; 32];
        let token = token_sintetico(&impronta, 99, "2026-01-02T03:04:05+00:00");
        let esito = verify_timestamp(&token, &impronta, "sha256")
            .expect("il token sintetico è strutturalmente valido");
        assert!(esito.imprint_ok);
        assert!(!esito.signature_ok, "senza certificato la firma non si verifica");
        assert!(esito.tsa.is_none());
        assert_eq!(esito.time, "2026-01-02 03:04:05 UTC");
        assert_eq!(esito.der, token);
    }

    #[test]
    fn token_sintetico_con_altra_impronta_lo_segnala() {
        let impronta = [0x22u8; 32];
        let token = token_sintetico(&impronta, 99, "2026-01-02T03:04:05+00:00");
        let esito = verify_timestamp(&token, &[0x33u8; 32], "sha256")
            .expect("il token resta strutturalmente valido");
        assert!(!esito.imprint_ok);
        assert!(!esito.signature_ok);
    }

    // -- Test con openssl locale (nessuna rete) ------------------------------

    fn openssl_disponibile() -> Option<PathBuf> {
        let candidati = [
            PathBuf::from(r"C:\msys64\usr\bin\openssl.exe"),
            PathBuf::from("openssl.exe"),
        ];
        for candidato in candidati {
            if let Ok(uscita) = Command::new(&candidato)
                .arg("version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                if uscita.success() {
                    return Some(candidato);
                }
            }
        }
        None
    }

    struct CartellaTemporanea {
        percorso: PathBuf,
    }

    impl CartellaTemporanea {
        fn nuova() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|durata| durata.as_nanos())
                .unwrap_or(0);
            let percorso = std::env::temp_dir().join(format!(
                "openpdf-tsa-test-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&percorso).expect("creazione cartella temporanea");
            CartellaTemporanea { percorso }
        }

        fn file(&self, nome: &str) -> PathBuf {
            self.percorso.join(nome)
        }
    }

    impl Drop for CartellaTemporanea {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.percorso);
        }
    }

    fn prepara_tsa(openssl: &Path, cartella: &CartellaTemporanea) -> Result<(), String> {
        let cert = cartella.file("tsa.crt");
        let chiave = cartella.file("tsa.key");
        let uscita = Command::new(openssl)
            .args([
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
            ])
            .arg(&chiave)
            .arg("-out")
            .arg(&cert)
            .args(["-days", "2", "-subj", "/CN=TSA di prova/O=Open PDF"])
            .args(["-addext", "extendedKeyUsage=critical,timeStamping"])
            .output()
            .map_err(|e| format!("avvio openssl req: {e}"))?;
        if !uscita.status.success() {
            return Err(format!(
                "openssl req fallito: {}",
                String::from_utf8_lossy(&uscita.stderr)
            ));
        }

        let cartella_config = cartella.percorso.display().to_string().replace('\\', "/");
        let config = format!(
            "[tsa]\ndefault_tsa = tsa_config1\n\
             [tsa_config1]\n\
             dir = {cartella_config}\n\
             serial = {cartella_config}/serial.txt\n\
             signer_cert = {cartella_config}/tsa.crt\n\
             signer_key = {cartella_config}/tsa.key\n\
             signer_digest = sha256\n\
             default_policy = 1.2.3.4.1\n\
             other_policies = 1.2.3.4.2\n\
             digests = sha256, sha384, sha512\n\
             accuracy = secs:1\n\
             ordering = yes\n\
             tsa_name = yes\n\
             ess_cert_id_chain = no\n\
             ess_cert_id_alg = sha256\n"
        );
        std::fs::write(cartella.file("tsa.cnf"), config)
            .map_err(|e| format!("config TSA: {e}"))?;
        std::fs::write(cartella.file("serial.txt"), b"01")
            .map_err(|e| format!("serial TSA: {e}"))?;
        Ok(())
    }

    fn risposta_da_query(
        openssl: &Path,
        cartella: &CartellaTemporanea,
        query: &[u8],
    ) -> Result<Vec<u8>, String> {
        let query_path = cartella.file("query.tsq");
        let reply_path = cartella.file("reply.tsr");
        std::fs::write(&query_path, query).map_err(|e| format!("query TSA: {e}"))?;
        let uscita = Command::new(openssl)
            .args(["ts", "-reply", "-config"])
            .arg(cartella.file("tsa.cnf"))
            .arg("-queryfile")
            .arg(&query_path)
            .arg("-out")
            .arg(&reply_path)
            .output()
            .map_err(|e| format!("avvio openssl ts -reply: {e}"))?;
        if !uscita.status.success() {
            return Err(format!(
                "openssl ts -reply fallito: {}",
                String::from_utf8_lossy(&uscita.stderr)
            ));
        }
        std::fs::read(&reply_path).map_err(|e| format!("lettura risposta TSA: {e}"))
    }

    #[test]
    fn verifica_offline_di_un_token_openssl() {
        let Some(openssl) = openssl_disponibile() else {
            eprintln!("openssl non trovato: test del token reale saltato");
            return;
        };
        let cartella = CartellaTemporanea::nuova();
        if let Err(motivo) = prepara_tsa(&openssl, &cartella) {
            eprintln!("TSA di prova non preparata ({motivo}): test saltato");
            return;
        }

        let dati = b"contenuto di prova per il timestamp PAdES";
        // Il digest dell'impronta può essere SHA-256 o SHA-384; la firma del
        // token usa invece l'algoritmo scelto dalla TSA (`signer_digest`).
        for (hash, nome) in [(HashKind::Sha256, "sha256"), (HashKind::Sha384, "sha384")] {
            let impronta = impronta_di(dati, hash);
            let richiesta = costruisci_richiesta(hash, &impronta, 123_456_789)
                .expect("richiesta codificabile");
            let risposta = match risposta_da_query(&openssl, &cartella, &richiesta) {
                Ok(risposta) => risposta,
                Err(motivo) => {
                    eprintln!("openssl ts -reply non disponibile ({motivo}): test saltato");
                    return;
                }
            };

            let token = estrai_token(&risposta).expect("il token è presente nella risposta");
            let esito = verify_timestamp(&token, &impronta, nome).expect("il token openssl è valido");
            assert!(esito.imprint_ok, "l'impronta {nome} deve coincidere");
            assert!(
                esito.signature_ok,
                "la firma RSA del token deve essere verificata ({nome})"
            );
            assert_eq!(esito.tsa.as_deref(), Some("TSA di prova"));
            assert!(esito.time.ends_with("UTC"), "tempo inatteso: {}", esito.time);
            assert!(!esito.der.is_empty());
        }
    }

    #[test]
    fn richiesta_completa_verso_una_tsa_locale() {
        let Some(openssl) = openssl_disponibile() else {
            eprintln!("openssl non trovato: test HTTP locale saltato");
            return;
        };
        let cartella = CartellaTemporanea::nuova();
        if let Err(motivo) = prepara_tsa(&openssl, &cartella) {
            eprintln!("TSA di prova non preparata ({motivo}): test HTTP saltato");
            return;
        }

        let server = match tiny_http::Server::http("127.0.0.1:0") {
            Ok(server) => server,
            Err(errore) => {
                eprintln!("server HTTP locale non disponibile ({errore}): test saltato");
                return;
            }
        };
        let indirizzo = match server.server_addr().to_ip() {
            Some(addr) => addr,
            None => {
                eprintln!("indirizzo del server locale non disponibile: test saltato");
                return;
            }
        };

        let (mittente, ricevente) = std::sync::mpsc::channel::<Result<(), String>>();
        let openssl_thread = openssl.clone();
        let cartella_thread = cartella.file("thread");
        std::fs::create_dir_all(&cartella_thread).expect("cartella del thread");
        // La TSA di prova riusa gli stessi file, ma in una cartella separata per
        // non mescolare query e risposta con il resto del test.
        for nome in ["tsa.crt", "tsa.key", "tsa.cnf", "serial.txt"] {
            std::fs::copy(cartella.file(nome), cartella_thread.join(nome))
                .expect("copia dei file TSA");
        }
        let config_thread = std::fs::read_to_string(cartella_thread.join("tsa.cnf"))
            .expect("config TSA")
            .replace(
                &cartella.percorso.display().to_string().replace('\\', "/"),
                &cartella_thread.display().to_string().replace('\\', "/"),
            );
        std::fs::write(cartella_thread.join("tsa.cnf"), config_thread).expect("config TSA");

        let gestore = std::thread::spawn(move || {
            let mut richiesta = match server.recv() {
                Ok(richiesta) => richiesta,
                Err(errore) => {
                    let _ = mittente.send(Err(format!("ricezione richiesta: {errore}")));
                    return;
                }
            };
            let mut corpo = Vec::new();
            if let Err(errore) = richiesta.as_reader().read_to_end(&mut corpo) {
                let _ = mittente.send(Err(format!("lettura corpo: {errore}")));
                return;
            }
            let cartella = CartellaTemporanea {
                percorso: cartella_thread,
            };
            match risposta_da_query(&openssl_thread, &cartella, &corpo) {
                Ok(risposta) => {
                    let _ = mittente.send(Ok(()));
                    let risposta = tiny_http::Response::from_data(risposta).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/timestamp-reply"[..],
                        )
                        .expect("header valido"),
                    );
                    let _ = richiesta.respond(risposta);
                }
                Err(motivo) => {
                    let _ = mittente.send(Err(motivo.clone()));
                    let risposta = tiny_http::Response::from_string(motivo).with_status_code(500);
                    let _ = richiesta.respond(risposta);
                }
            }
        });

        let impronta = impronta_di(b"firma CMS di prova", HashKind::Sha256);
        let esito = request_timestamp(&format!("http://{indirizzo}/"), &impronta, "sha256");
        let generazione = match ricevente.recv_timeout(std::time::Duration::from_secs(45)) {
            Ok(esito_generazione) => esito_generazione,
            Err(_) => {
                eprintln!("la TSA locale non ha risposto in tempo: test HTTP saltato");
                return;
            }
        };
        let _ = gestore.join();

        match generazione {
            Err(motivo) => {
                eprintln!("generazione della risposta non riuscita ({motivo}): test saltato");
                return;
            }
            Ok(()) => {}
        }

        let token = esito.expect("la richiesta HTTP verso la TSA locale deve riuscire");
        assert!(token.imprint_ok);
        assert!(token.signature_ok, "la firma del token deve essere valida");
        assert_eq!(token.tsa.as_deref(), Some("TSA di prova"));
        assert!(!token.der.is_empty());
    }
}
