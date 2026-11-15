//! Validazione della catena di certificati e degli attributi CMS.
//!
//! Il modulo completa la verifica PAdES: `pades::verify_pdf` controlla digest e
//! firma del firmatario, qui invece si verifica che:
//! - ogni certificato della catena sia firmato dalla chiave dell'emittente
//!   (RSA PKCS#1 v1.5 SHA-1/SHA-256/SHA-384/SHA-512, ECDSA P-256/P-384 SHA-256/384);
//! - il periodo di validità di ogni certificato copra l'istante richiesto;
//! - la catena risalga fino a un certificato autofirmato (radice);
//! - gli attributi firmati CMS siano coerenti: `messageDigest`, `contentType`
//!   e `signingCertificateV2` (ESS, obbligatorio per PAdES).
//!
//! ## Come si collega a `pades::verify_pdf`
//!
//! 1. Da `SignedData.certificates` raccogliere i DER di tutti i certificati
//!    (incluso quello del firmatario individuato da `certificato_firmatario`);
//! 2. chiamare [`validate_chain`] passando l'istante della firma
//!    (`signingTime`) oppure `None` per l'ora corrente;
//! 3. chiamare [`check_signed_attributes`] con il DER di
//!    `signer_info.signed_attrs`, il digest calcolato sul `ByteRange` e il DER
//!    del certificato del firmatario;
//! 4. riportare in `SignatureInfo`/UI i campi di [`ChainReport`] e l'elenco dei
//!    problemi restituito da [`check_signed_attributes`] (vuoto = coerente).
//!
//! ## Limiti dichiarati
//!
//! - `trusted` **non** consulta il trust store di Windows né un bundle di CA:
//!   dice solo che la catena arriva a una radice autofirmata con firma valida.
//!   Un certificato autofirmato creato in casa risulterebbe quindi "trusted".
//! - Non applica policy CPS/CP, name constraints, `basicConstraints`,
//!   `keyUsage`/`extendedKeyUsage`, né l'ancoraggio temporale della radice.
//! - Non interroga CRL o OCSP: la revoca non è verificata.
//! - Il nome dell'emittente viene confrontato byte per byte nella codifica DER,
//!   senza normalizzazione RFC 5280 (es. PrintableString vs UTF8String).
//! - Non verifica `issuerSerial` dentro `signingCertificateV2`, né più di un
//!   firmatario per volta.
//! - Algoritmi supportati per le firme dei certificati: RSA PKCS#1 v1.5
//!   (SHA-1/256/384/512) ed ECDSA (SHA-256/384). RSA-PSS, Ed25519 e DSA
//!   vengono segnalati come non supportati.

use chrono::{DateTime, Utc};
use rasn::prelude::*;
use rasn_pkix::{Attribute, Certificate, Name, Time};
use rsa::pkcs1v15::{Signature as RsaSignature, VerifyingKey as RsaVerifyingKey};
use rsa::pkcs8::DecodePublicKey;
use rsa::signature::hazmat::PrehashVerifier;
use rsa::signature::{SignatureEncoding, Verifier};
use rsa::{Pkcs1v15Sign, RsaPublicKey};
use sha2::{Digest, Sha256, Sha384, Sha512};

/// Esito della validazione di una catena di certificati.
#[derive(Debug, Clone, Default)]
pub struct ChainReport {
    /// Tutte le firme presenti sono corrette e le date coprono `at`.
    ///
    /// Se la catena è incompleta (`complete == false`) le firme dei collegamenti
    /// disponibili possono comunque essere corrette: in quel caso `valid` resta
    /// `true`, ma `trusted` è sempre `false`. Eventuali certificati non
    /// decodificabili invalidano invece il report.
    pub valid: bool,
    /// La catena arriva a un certificato autofirmato (subject == issuer).
    pub complete: bool,
    /// Radice autofirmata presente e catena valida.
    ///
    /// **Non** implica che la radice sia attendibile per il sistema: non viene
    /// consultato lo store di Windows, né un elenco di CA note.
    pub trusted: bool,
    /// Problemi riscontrati, in italiano e già pronti per la UI.
    pub errors: Vec<String>,
    /// Nomi dei certificati in ordine foglia → radice (CN, oppure DN completo).
    pub chain: Vec<String>,
    /// Inizio validità della foglia, in UTC (`AAAA-MM-GG HH:MM:SS UTC`).
    pub not_before: String,
    /// Fine validità della foglia, in UTC (`AAAA-MM-GG HH:MM:SS UTC`).
    pub not_after: String,
}

/// Valida una catena di certificati X.509 in DER.
///
/// `certs` contiene il DER della foglia e degli eventuali intermedi, in
/// qualsiasi ordine. La foglia viene individuata come il certificato che non
/// emette nessun altro certificato della lista; da lì la catena viene risalita
/// confrontando `issuer` e `subject` nella loro codifica DER.
///
/// `at` è l'istante da usare per la validità temporale; `None` significa adesso.
pub fn validate_chain(certs: &[Vec<u8>], at: Option<DateTime<Utc>>) -> ChainReport {
    let istante = at.unwrap_or_else(Utc::now);
    let mut report = ChainReport::default();

    if certs.is_empty() {
        report
            .errors
            .push("Nessun certificato fornito per la validazione.".to_string());
        return report;
    }

    // Decodifica; conserva l'indice originale per poter rileggere il DER.
    let mut decodificati: Vec<(usize, Certificate)> = Vec::new();
    for (indice, der) in certs.iter().enumerate() {
        match rasn::der::decode::<Certificate>(der) {
            Ok(certificato) => decodificati.push((indice, certificato)),
            Err(errore) => report.errors.push(format!(
                "Certificato {} non decodificabile come X.509: {errore}",
                indice + 1
            )),
        }
    }
    if decodificati.is_empty() {
        return report;
    }

    let soggetti: Vec<Vec<u8>> = decodificati
        .iter()
        .map(|(_, certificato)| nome_der(&certificato.tbs_certificate.subject))
        .collect();
    let emittenti: Vec<Vec<u8>> = decodificati
        .iter()
        .map(|(_, certificato)| nome_der(&certificato.tbs_certificate.issuer))
        .collect();

    // La foglia non emette nessun altro certificato della lista.
    let mut candidate: Vec<usize> = (0..decodificati.len())
        .filter(|&i| {
            !(0..decodificati.len()).any(|j| j != i && emittenti[j] == soggetti[i])
        })
        .collect();
    if candidate.len() > 1 {
        // Preferisce un certificato non autofirmato: una radice isolata non è
        // una foglia se nella lista c'è anche un certificato da lei emesso.
        if let Some(non_autofirmato) = candidate
            .iter()
            .copied()
            .find(|&i| soggetti[i] != emittenti[i])
        {
            candidate.retain(|&i| i == non_autofirmato);
        }
    }
    let Some(&foglia) = candidate.first() else {
        report.errors.push(
            "Impossibile individuare la foglia: la catena contiene un ciclo di certificati."
                .to_string(),
        );
        return report;
    };

    // Risale la catena dalla foglia all'eventuale radice autofirmata.
    let mut percorso: Vec<usize> = vec![foglia];
    loop {
        let corrente = *percorso.last().expect("percorso non vuoto");
        if soggetti[corrente] == emittenti[corrente] {
            report.complete = true;
            break;
        }
        let prossimo = (0..decodificati.len()).find(|&j| {
            !percorso.contains(&j) && soggetti[j] == emittenti[corrente]
        });
        match prossimo {
            Some(j) => percorso.push(j),
            None => break,
        }
    }

    report.chain = percorso
        .iter()
        .map(|&i| nome_leggibile(&decodificati[i].1.tbs_certificate.subject))
        .collect();

    let foglia_cert = &decodificati[foglia].1;
    report.not_before = formatta_data(istante_di(&foglia_cert.tbs_certificate.validity.not_before));
    report.not_after = formatta_data(istante_di(&foglia_cert.tbs_certificate.validity.not_after));

    let mut firme_ok = true;
    let mut date_ok = true;

    for posizione in 0..percorso.len() {
        let (indice_originale, certificato) = &decodificati[percorso[posizione]];
        let nome = nome_leggibile(&certificato.tbs_certificate.subject);

        if let Some(problema) = controlla_validita(certificato, &nome, istante) {
            report.errors.push(problema);
            date_ok = false;
        }

        // Emittente: il successivo del percorso, oppure il certificato stesso
        // se la catena termina su una radice autofirmata.
        let emittente = if posizione + 1 < percorso.len() {
            Some(&decodificati[percorso[posizione + 1]].1)
        } else if report.complete {
            Some(certificato)
        } else {
            None
        };

        if let Some(emittente) = emittente {
            if let Err(problema) =
                verifica_firma_certificato(&certs[*indice_originale], certificato, emittente)
            {
                report.errors.push(problema);
                firme_ok = false;
            }
        }
    }

    // Un certificato non decodificabile invalida il quadro: non si può
    // escludere che fosse parte della catena.
    let decodifica_ok = decodificati.len() == certs.len();
    report.valid = firme_ok && date_ok && decodifica_ok;
    report.trusted = report.valid && report.complete;
    report
}

/// Controlla gli attributi firmati di una firma PAdES/CAdES.
///
/// `signed_attrs_der` è il DER di `SetOf<Attribute>` (sono accettati sia il tag
/// `SET OF` 0x31 sia il tag implicito CMS `[0]` 0xA0); `content_digest` è il
/// digest calcolato sul contenuto firmato (il `ByteRange` del PDF) e `cert_der`
/// è il DER del certificato del firmatario.
///
/// Il confronto avviene sui byte: `content_digest` va calcolato con l'algoritmo
/// dichiarato in `SignerInfo.digestAlgorithm`. La verifica crittografica della
/// firma CMS sugli attributi resta a carico di `pades::verify_pdf`.
///
/// Restituisce l'elenco dei problemi: vuoto significa attributi coerenti.
pub fn check_signed_attributes(
    signed_attrs_der: &[u8],
    content_digest: &[u8],
    cert_der: &[u8],
) -> Vec<String> {
    let mut problemi: Vec<String> = Vec::new();

    let normalizzati = normalizza_set(signed_attrs_der);
    let attributi = match rasn::der::decode::<SetOf<Attribute>>(&normalizzati) {
        Ok(valore) => valore,
        Err(errore) => {
            problemi.push(format!(
                "Attributi firmati non decodificabili come SET OF Attribute: {errore}"
            ));
            return problemi;
        }
    };

    // -- messageDigest ------------------------------------------------------
    let message_digest = cerca_attributi(&attributi, &oid_message_digest());
    match message_digest.as_slice() {
        [] => problemi.push(
            "Attributo messageDigest assente: la firma non copre il contenuto.".to_string(),
        ),
        [attributo] => {
            if attributo.values.len() != 1 {
                problemi.push(format!(
                    "messageDigest contiene {} valori (atteso uno).",
                    attributo.values.len()
                ));
            }
            if let Some(valore) = attributo.values.iter().next() {
                match rasn::der::decode::<OctetString>(valore.as_bytes()) {
                    Ok(digest) => {
                        if digest.to_vec() != content_digest {
                            problemi.push(
                                "messageDigest non corrisponde al digest del contenuto firmato."
                                    .to_string(),
                            );
                        }
                    }
                    Err(errore) => problemi.push(format!(
                        "messageDigest non è un OCTET STRING valido: {errore}"
                    )),
                }
            }
        }
        _ => problemi.push(format!(
            "Attributo messageDigest duplicato ({} occorrenze).",
            message_digest.len()
        )),
    }

    // -- contentType --------------------------------------------------------
    let content_type = cerca_attributi(&attributi, &oid_content_type());
    match content_type.as_slice() {
        [] => problemi.push(
            "Attributo contentType assente: obbligatorio per una firma PAdES.".to_string(),
        ),
        [attributo] => {
            if attributo.values.len() != 1 {
                problemi.push(format!(
                    "contentType contiene {} valori (atteso uno).",
                    attributo.values.len()
                ));
            }
            if let Some(valore) = attributo.values.iter().next() {
                match rasn::der::decode::<ObjectIdentifier>(valore.as_bytes()) {
                    Ok(tipo) => {
                        if tipo != oid_data() {
                            problemi.push(format!(
                                "contentType è {tipo}, atteso id-data (1.2.840.113549.1.7.1)."
                            ));
                        }
                    }
                    Err(errore) => problemi.push(format!(
                        "contentType non è un OBJECT IDENTIFIER valido: {errore}"
                    )),
                }
            } else {
                problemi.push("contentType senza valori.".to_string());
            }
        }
        _ => problemi.push(format!(
            "Attributo contentType duplicato ({} occorrenze).",
            content_type.len()
        )),
    }

    // -- signingCertificateV2 ----------------------------------------------
    let signing_cert = cerca_attributi(&attributi, &oid_signing_certificate_v2());
    match signing_cert.as_slice() {
        [] => problemi.push(
            "Attributo signingCertificateV2 assente: obbligatorio per una firma PAdES."
                .to_string(),
        ),
        [attributo] => {
            if attributo.values.len() != 1 {
                problemi.push(format!(
                    "signingCertificateV2 contiene {} valori (atteso uno).",
                    attributo.values.len()
                ));
            }
            if cert_der.is_empty() {
                problemi.push(
                    "Certificato del firmatario vuoto: signingCertificateV2 non verificabile."
                        .to_string(),
                );
            } else if let Some(valore) = attributo.values.iter().next() {
                match estrai_cert_hash(valore.as_bytes()) {
                    Ok((impronta, algoritmo)) => {
                        let (atteso, descrizione) = match algoritmo.as_ref() {
                            None => (Sha256::digest(cert_der).to_vec(), "SHA-256"),
                            Some(alg) if *alg == oid_sha1() => (sha1(cert_der).to_vec(), "SHA-1"),
                            Some(alg) if *alg == oid_sha256() => {
                                (Sha256::digest(cert_der).to_vec(), "SHA-256")
                            }
                            Some(alg) if *alg == oid_sha384() => {
                                (Sha384::digest(cert_der).to_vec(), "SHA-384")
                            }
                            Some(alg) if *alg == oid_sha512() => {
                                (Sha512::digest(cert_der).to_vec(), "SHA-512")
                            }
                            Some(alg) => {
                                problemi.push(format!(
                                    "signingCertificateV2: algoritmo di hash «{alg}» non supportato."
                                ));
                                return problemi;
                            }
                        };
                        if impronta != atteso {
                            problemi.push(format!(
                                "signingCertificateV2: certHash ({descrizione}) non corrisponde al certificato del firmatario."
                            ));
                        }
                    }
                    Err(errore) => problemi.push(format!(
                        "signingCertificateV2 non interpretabile: {errore}"
                    )),
                }
            }
        }
        _ => problemi.push(format!(
            "Attributo signingCertificateV2 duplicato ({} occorrenze).",
            signing_cert.len()
        )),
    }

    problemi
}

// -- Verifica delle firme dei certificati ------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HashFirma {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
}

#[derive(Debug, Clone, Copy)]
enum TipoFirma {
    Rsa(HashFirma),
    Ecdsa(HashFirma),
}

impl TipoFirma {
    fn descrizione(self) -> &'static str {
        match self {
            TipoFirma::Rsa(HashFirma::Sha1) => "RSA PKCS#1 v1.5 SHA-1",
            TipoFirma::Rsa(HashFirma::Sha256) => "RSA PKCS#1 v1.5 SHA-256",
            TipoFirma::Rsa(HashFirma::Sha384) => "RSA PKCS#1 v1.5 SHA-384",
            TipoFirma::Rsa(HashFirma::Sha512) => "RSA PKCS#1 v1.5 SHA-512",
            TipoFirma::Ecdsa(HashFirma::Sha1) => "ECDSA SHA-1",
            TipoFirma::Ecdsa(HashFirma::Sha256) => "ECDSA SHA-256",
            TipoFirma::Ecdsa(HashFirma::Sha384) => "ECDSA SHA-384",
            TipoFirma::Ecdsa(HashFirma::Sha512) => "ECDSA SHA-512",
        }
    }
}

fn tipo_firma(algoritmo: &ObjectIdentifier) -> Option<TipoFirma> {
    if *algoritmo == oid(&[1, 2, 840, 113549, 1, 1, 5]) {
        Some(TipoFirma::Rsa(HashFirma::Sha1))
    } else if *algoritmo == oid(&[1, 2, 840, 113549, 1, 1, 11]) {
        Some(TipoFirma::Rsa(HashFirma::Sha256))
    } else if *algoritmo == oid(&[1, 2, 840, 113549, 1, 1, 12]) {
        Some(TipoFirma::Rsa(HashFirma::Sha384))
    } else if *algoritmo == oid(&[1, 2, 840, 113549, 1, 1, 13]) {
        Some(TipoFirma::Rsa(HashFirma::Sha512))
    } else if *algoritmo == oid(&[1, 2, 840, 10045, 4, 3, 2]) {
        Some(TipoFirma::Ecdsa(HashFirma::Sha256))
    } else if *algoritmo == oid(&[1, 2, 840, 10045, 4, 3, 3]) {
        Some(TipoFirma::Ecdsa(HashFirma::Sha384))
    } else {
        None
    }
}

fn verifica_firma_certificato(
    der: &[u8],
    certificato: &Certificate,
    emittente: &Certificate,
) -> Result<(), String> {
    let tbs = estrai_tbs(der)?;
    let algoritmo = &certificato.signature_algorithm.algorithm;
    let nome = nome_leggibile(&certificato.tbs_certificate.subject);
    let nome_emittente = nome_leggibile(&emittente.tbs_certificate.subject);
    let tipo = tipo_firma(algoritmo).ok_or_else(|| {
        format!("Certificato «{nome}»: algoritmo di firma «{algoritmo}» non supportato.")
    })?;

    let spki = rasn::der::encode(&emittente.tbs_certificate.subject_public_key_info)
        .map_err(|errore| format!("Chiave pubblica di «{nome_emittente}» non codificabile: {errore}"))?;
    let firma = certificato.signature_value.as_raw_slice();

    let esito = match tipo {
        TipoFirma::Rsa(hash) => verifica_rsa(&spki, hash, tbs, firma),
        TipoFirma::Ecdsa(hash) => verifica_ecdsa(&spki, hash, tbs, firma),
    };

    match esito {
        Ok(()) => Ok(()),
        Err(dettaglio) => Err(format!(
            "Firma del certificato «{nome}» non valida ({}): {dettaglio}",
            tipo.descrizione()
        )),
    }
}

/// Estrae il TLV completo di `tbsCertificate` dal DER del certificato.
fn estrai_tbs(der: &[u8]) -> Result<&[u8], String> {
    let (certificato, _) = leggi_tlv(der)?;
    if certificato.tag != 0x30 {
        return Err("Il certificato non è una SEQUENCE DER.".to_string());
    }
    let (tbs, _) = leggi_tlv(certificato.contenuto)?;
    if tbs.tag != 0x30 {
        return Err("tbsCertificate non è una SEQUENCE DER.".to_string());
    }
    Ok(tbs.intero)
}

fn verifica_rsa(
    spki: &[u8],
    hash: HashFirma,
    messaggio: &[u8],
    firma: &[u8],
) -> Result<(), String> {
    let chiave = RsaPublicKey::from_public_key_der(spki)
        .map_err(|errore| format!("chiave pubblica RSA non leggibile: {errore}"))?;
    let firma = RsaSignature::try_from(firma)
        .map_err(|errore| format!("firma RSA non codificata correttamente: {errore}"))?;

    let valida = match hash {
        HashFirma::Sha1 => {
            // RSA PKCS#1 v1.5 su SHA-1: la firma copre il DigestInfo completo.
            let digest = sha1(messaggio);
            let mut digest_info = Vec::with_capacity(35);
            digest_info.extend_from_slice(&[
                0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00,
                0x04, 0x14,
            ]);
            digest_info.extend_from_slice(&digest);
            chiave
                .verify(Pkcs1v15Sign::new_unprefixed(), &digest_info, &firma.to_vec())
                .is_ok()
        }
        HashFirma::Sha256 => RsaVerifyingKey::<Sha256>::new(chiave)
            .verify(messaggio, &firma)
            .is_ok(),
        HashFirma::Sha384 => RsaVerifyingKey::<Sha384>::new(chiave)
            .verify(messaggio, &firma)
            .is_ok(),
        HashFirma::Sha512 => RsaVerifyingKey::<Sha512>::new(chiave)
            .verify(messaggio, &firma)
            .is_ok(),
    };

    if valida {
        Ok(())
    } else {
        Err("la firma non corrisponde alla chiave dell'emittente".to_string())
    }
}

fn verifica_ecdsa(
    spki: &[u8],
    hash: HashFirma,
    messaggio: &[u8],
    firma: &[u8],
) -> Result<(), String> {
    let digest = match hash {
        HashFirma::Sha1 => sha1(messaggio).to_vec(),
        HashFirma::Sha256 => Sha256::digest(messaggio).to_vec(),
        HashFirma::Sha384 => Sha384::digest(messaggio).to_vec(),
        HashFirma::Sha512 => Sha512::digest(messaggio).to_vec(),
    };

    if let Ok(chiave) = p256::ecdsa::VerifyingKey::from_public_key_der(spki) {
        let firma = p256::ecdsa::DerSignature::from_bytes(firma)
            .map_err(|errore| format!("firma ECDSA P-256 non valida: {errore}"))?;
        return chiave
            .verify_prehash(&digest, &firma)
            .map_err(|_| "la firma non corrisponde alla chiave P-256 dell'emittente".to_string());
    }
    if let Ok(chiave) = p384::ecdsa::VerifyingKey::from_public_key_der(spki) {
        let firma = p384::ecdsa::DerSignature::from_bytes(firma)
            .map_err(|errore| format!("firma ECDSA P-384 non valida: {errore}"))?;
        return chiave
            .verify_prehash(&digest, &firma)
            .map_err(|_| "la firma non corrisponde alla chiave P-384 dell'emittente".to_string());
    }

    Err("chiave pubblica EC non riconosciuta (attese P-256 o P-384).".to_string())
}

// -- Validità temporale -------------------------------------------------------

fn controlla_validita(
    certificato: &Certificate,
    nome: &str,
    istante: DateTime<Utc>,
) -> Option<String> {
    let inizio = istante_di(&certificato.tbs_certificate.validity.not_before);
    let fine = istante_di(&certificato.tbs_certificate.validity.not_after);
    if istante < inizio {
        Some(format!(
            "Certificato «{nome}» non ancora valido: notBefore {}.",
            formatta_data(inizio)
        ))
    } else if istante > fine {
        Some(format!(
            "Certificato «{nome}» scaduto: notAfter {}.",
            formatta_data(fine)
        ))
    } else {
        None
    }
}

fn istante_di(tempo: &Time) -> DateTime<Utc> {
    match tempo {
        Time::Utc(data) => *data,
        Time::General(data) => data.with_timezone(&Utc),
    }
}

fn formatta_data(istante: DateTime<Utc>) -> String {
    istante.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

// -- Nomi dei certificati -----------------------------------------------------

fn nome_der(nome: &Name) -> Vec<u8> {
    rasn::der::encode(nome).unwrap_or_default()
}

/// CN se presente, altrimenti il DN completo con le etichette usuali.
fn nome_leggibile(nome: &Name) -> String {
    let Name::RdnSequence(sequenza) = nome;
    let mut comune: Option<String> = None;
    let mut parti: Vec<String> = Vec::new();

    for rdn in sequenza.iter() {
        for voce in rdn.iter() {
            let valore = valore_testo(voce.value.as_bytes());
            if valore.is_empty() {
                continue;
            }
            parti.push(format!("{}={valore}", etichetta(&voce.r#type)));
            if *voce.r#type == oid(&[2, 5, 4, 3]) && comune.is_none() {
                comune = Some(valore);
            }
        }
    }

    comune.unwrap_or_else(|| {
        if parti.is_empty() {
            "(nome vuoto)".to_string()
        } else {
            parti.join(", ")
        }
    })
}

fn etichetta(tipo: &ObjectIdentifier) -> &'static str {
    if *tipo == oid(&[2, 5, 4, 3]) {
        "CN"
    } else if *tipo == oid(&[2, 5, 4, 6]) {
        "C"
    } else if *tipo == oid(&[2, 5, 4, 7]) {
        "L"
    } else if *tipo == oid(&[2, 5, 4, 8]) {
        "ST"
    } else if *tipo == oid(&[2, 5, 4, 10]) {
        "O"
    } else if *tipo == oid(&[2, 5, 4, 11]) {
        "OU"
    } else if *tipo == oid(&[2, 5, 4, 5]) {
        "serialNumber"
    } else {
        "OID"
    }
}

fn valore_testo(der: &[u8]) -> String {
    let Ok((elemento, _)) = leggi_tlv(der) else {
        return String::new();
    };
    match elemento.tag {
        // BMPString: UTF-16 big endian.
        0x1e => {
            let unita: Vec<u16> = elemento
                .contenuto
                .chunks_exact(2)
                .map(|coppia| u16::from_be_bytes([coppia[0], coppia[1]]))
                .collect();
            String::from_utf16_lossy(&unita)
        }
        // UniversalString: UTF-32 big endian.
        0x1c => {
            let punti: Vec<u32> = elemento
                .contenuto
                .chunks_exact(4)
                .map(|quattro| {
                    u32::from_be_bytes([quattro[0], quattro[1], quattro[2], quattro[3]])
                })
                .collect();
            punti.into_iter().filter_map(char::from_u32).collect()
        }
        // UTF8String, PrintableString, IA5String, TeletexString, ...
        _ => String::from_utf8_lossy(elemento.contenuto).to_string(),
    }
}

// -- Attributi ESS ------------------------------------------------------------

/// Legge `SigningCertificateV2` e restituisce `(certHash, hashAlgorithm)`.
fn estrai_cert_hash(der: &[u8]) -> Result<(Vec<u8>, Option<ObjectIdentifier>), String> {
    let (firma_cert, _) = leggi_tlv(der)?;
    if firma_cert.tag != 0x30 {
        return Err("SigningCertificateV2 non è una SEQUENCE.".to_string());
    }
    let (certificati, _) = leggi_tlv(firma_cert.contenuto)?;
    if certificati.tag != 0x30 {
        return Err("SigningCertificateV2.certs non è una SEQUENCE.".to_string());
    }
    let (ess_cert_id, _) = leggi_tlv(certificati.contenuto)?;
    if ess_cert_id.tag != 0x30 {
        return Err("ESSCertIDv2 non è una SEQUENCE.".to_string());
    }

    let mut algoritmo: Option<ObjectIdentifier> = None;
    let mut impronta: Option<Vec<u8>> = None;
    let mut resto = ess_cert_id.contenuto;
    while !resto.is_empty() {
        let (elemento, successivo) = leggi_tlv(resto)?;
        match elemento.tag {
            // hashAlgorithm è un AlgorithmIdentifier (SEQUENCE) e, per il DEFAULT,
            // non è taggato: compare prima di certHash.
            0x30 if impronta.is_none() && algoritmo.is_none() => {
                let (oid_algoritmo, _) = leggi_tlv(elemento.contenuto)?;
                if oid_algoritmo.tag != 0x06 {
                    return Err("hashAlgorithm senza OBJECT IDENTIFIER.".to_string());
                }
                let oid_algoritmo = rasn::der::decode::<ObjectIdentifier>(oid_algoritmo.intero)
                    .map_err(|errore| format!("hashAlgorithm non valido: {errore}"))?;
                algoritmo = Some(oid_algoritmo);
            }
            0x04 if impronta.is_none() => impronta = Some(elemento.contenuto.to_vec()),
            _ => {}
        }
        resto = successivo;
    }

    let impronta = impronta.ok_or_else(|| "ESSCertIDv2 senza certHash.".to_string())?;
    Ok((impronta, algoritmo))
}

/// I `SignedAttributes` CMS usano il tag implicito `[0]` (0xA0) al posto di
/// `SET OF` (0x31): il contenuto e la lunghezza non cambiano.
fn normalizza_set(der: &[u8]) -> Vec<u8> {
    let mut copia = der.to_vec();
    if copia.first() == Some(&0xa0) {
        copia[0] = 0x31;
    }
    copia
}

fn cerca_attributi<'a>(
    attributi: &'a SetOf<Attribute>,
    tipo: &ObjectIdentifier,
) -> Vec<&'a Attribute> {
    attributi
        .iter()
        .filter(|attributo| attributo.r#type == *tipo)
        .collect()
}

// -- DER minimale -------------------------------------------------------------

struct Tlv<'a> {
    tag: u8,
    contenuto: &'a [u8],
    /// TLV completo (intestazione + contenuto).
    intero: &'a [u8],
}

fn leggi_tlv(dati: &[u8]) -> Result<(Tlv<'_>, &[u8]), String> {
    if dati.len() < 2 {
        return Err("TLV troncato.".to_string());
    }
    let tag = dati[0];
    if tag & 0x1f == 0x1f {
        return Err("tag ASN.1 multi-byte non supportato.".to_string());
    }
    let primo = dati[1];
    let (lunghezza, intestazione) = if primo & 0x80 == 0 {
        (primo as usize, 2usize)
    } else {
        let byte_lunghezza = (primo & 0x7f) as usize;
        if byte_lunghezza == 0 || byte_lunghezza > 4 {
            return Err("lunghezza ASN.1 non valida.".to_string());
        }
        let fine_lunghezza = 2 + byte_lunghezza;
        if dati.len() < fine_lunghezza {
            return Err("lunghezza ASN.1 troncata.".to_string());
        }
        let mut valore = 0usize;
        for byte in &dati[2..fine_lunghezza] {
            valore = (valore << 8) | *byte as usize;
        }
        (valore, fine_lunghezza)
    };

    let fine = intestazione
        .checked_add(lunghezza)
        .ok_or_else(|| "lunghezza ASN.1 fuori scala.".to_string())?;
    if fine > dati.len() {
        return Err("contenuto ASN.1 troncato.".to_string());
    }
    Ok((
        Tlv {
            tag,
            contenuto: &dati[intestazione..fine],
            intero: &dati[..fine],
        },
        &dati[fine..],
    ))
}

// -- SHA-1 (solo per certificati e ESS legacy) --------------------------------

/// SHA-1 compatto: `sha2` non lo espone e la dipendenza non è presente.
fn sha1(dati: &[u8]) -> [u8; 20] {
    let mut stato: [u32; 5] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476, 0xc3d2_e1f0];

    let lunghezza_bit = (dati.len() as u64).wrapping_mul(8);
    let mut messaggio = dati.to_vec();
    messaggio.push(0x80);
    while messaggio.len() % 64 != 56 {
        messaggio.push(0);
    }
    messaggio.extend_from_slice(&lunghezza_bit.to_be_bytes());

    for blocco in messaggio.chunks_exact(64) {
        let mut parole = [0u32; 80];
        for (indice, parola) in parole.iter_mut().take(16).enumerate() {
            let base = indice * 4;
            *parola = u32::from_be_bytes([
                blocco[base],
                blocco[base + 1],
                blocco[base + 2],
                blocco[base + 3],
            ]);
        }
        for indice in 16..80 {
            parole[indice] = (parole[indice - 3]
                ^ parole[indice - 8]
                ^ parole[indice - 14]
                ^ parole[indice - 16])
                .rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) =
            (stato[0], stato[1], stato[2], stato[3], stato[4]);
        for (indice, parola) in parole.iter().enumerate() {
            let (funzione, costante) = match indice {
                0..=19 => ((b & c) | ((!b) & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let temporaneo = a
                .rotate_left(5)
                .wrapping_add(funzione)
                .wrapping_add(e)
                .wrapping_add(costante)
                .wrapping_add(*parola);
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

    let mut uscita = [0u8; 20];
    for (indice, parola) in stato.iter().enumerate() {
        uscita[indice * 4..indice * 4 + 4].copy_from_slice(&parola.to_be_bytes());
    }
    uscita
}

// -- OID ----------------------------------------------------------------------

fn oid(archi: &[u32]) -> ObjectIdentifier {
    ObjectIdentifier::new(archi.to_vec()).expect("OID valido")
}

fn oid_content_type() -> ObjectIdentifier {
    oid(&[1, 2, 840, 113549, 1, 9, 3])
}

fn oid_message_digest() -> ObjectIdentifier {
    oid(&[1, 2, 840, 113549, 1, 9, 4])
}

fn oid_signing_certificate_v2() -> ObjectIdentifier {
    oid(&[1, 2, 840, 113549, 1, 9, 16, 2, 47])
}

fn oid_data() -> ObjectIdentifier {
    oid(&[1, 2, 840, 113549, 1, 7, 1])
}

fn oid_sha1() -> ObjectIdentifier {
    oid(&[1, 3, 14, 3, 2, 26])
}

fn oid_sha256() -> ObjectIdentifier {
    oid(&[2, 16, 840, 1, 101, 3, 4, 2, 1])
}

fn oid_sha384() -> ObjectIdentifier {
    oid(&[2, 16, 840, 1, 101, 3, 4, 2, 2])
}

fn oid_sha512() -> ObjectIdentifier {
    oid(&[2, 16, 840, 1, 101, 3, 4, 2, 3])
}

// -- Test ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use chrono::TimeZone;
    use rasn_pkix::AlgorithmIdentifier;
    use std::path::{Path, PathBuf};

    fn fixture(nome: &str) -> Vec<u8> {
        let percorso = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(nome);
        pem_der(&percorso)
    }

    fn pem_der(percorso: &Path) -> Vec<u8> {
        let testo = std::fs::read_to_string(percorso)
            .unwrap_or_else(|errore| panic!("Fixture «{}»: {errore}", percorso.display()));
        let intestazione = "-----BEGIN CERTIFICATE-----";
        let chiusura = "-----END CERTIFICATE-----";
        let inizio = testo
            .find(intestazione)
            .unwrap_or_else(|| panic!("Fixture «{}» senza PEM", percorso.display()))
            + intestazione.len();
        let fine = testo
            .find(chiusura)
            .unwrap_or_else(|| panic!("Fixture «{}» senza chiusura PEM", percorso.display()));
        let corpo: String = testo[inizio..fine]
            .chars()
            .filter(|carattere| !carattere.is_whitespace())
            .collect();
        base64::engine::general_purpose::STANDARD
            .decode(corpo)
            .expect("base64 PEM valido")
    }

    fn catena_valida() -> Vec<Vec<u8>> {
        vec![
            fixture("pv-leaf.pem"),
            fixture("pv-intermediate.pem"),
            fixture("pv-ca.pem"),
        ]
    }

    #[test]
    fn catena_valida_fino_alla_radice() {
        let certificati = catena_valida();
        let report = validate_chain(&certificati, None);
        assert!(report.valid, "errori: {:?}", report.errors);
        assert!(report.complete);
        assert!(report.trusted);
        assert!(report.errors.is_empty(), "errori: {:?}", report.errors);
        assert_eq!(
            report.chain,
            vec![
                "OpenPDF Verify Leaf",
                "OpenPDF Verify Intermediate",
                "OpenPDF Verify Root"
            ]
        );
        assert!(report.not_before.starts_with("2026-11-01"));
        assert!(report.not_after.starts_with("2045-01-01"));
    }

    #[test]
    fn ordine_di_input_irrilevante() {
        let foglia = fixture("pv-leaf.pem");
        let intermedio = fixture("pv-intermediate.pem");
        let radice = fixture("pv-ca.pem");

        let report = validate_chain(&[intermedio.clone(), radice.clone(), foglia.clone()], None);
        assert!(report.valid, "errori: {:?}", report.errors);
        assert_eq!(
            report.chain,
            vec![
                "OpenPDF Verify Leaf",
                "OpenPDF Verify Intermediate",
                "OpenPDF Verify Root"
            ]
        );

        let report = validate_chain(&[radice, foglia, intermedio], None);
        assert!(report.valid, "errori: {:?}", report.errors);
        assert!(report.complete);
    }

    #[test]
    fn foglia_scaduta_segnalata() {
        let certificati = vec![
            fixture("pv-leaf-expired.pem"),
            fixture("pv-intermediate.pem"),
            fixture("pv-ca.pem"),
        ];
        let report = validate_chain(&certificati, None);
        assert!(!report.valid);
        assert!(report.complete);
        assert!(!report.trusted);
        assert!(
            report
                .errors
                .iter()
                .any(|errore| errore.contains("scaduto") && errore.contains("Scaduta")),
            "errori: {:?}",
            report.errors
        );
    }

    #[test]
    fn firma_manomessa_non_valida() {
        let mut foglia = fixture("pv-leaf.pem");
        let ultimo = foglia.len() - 1;
        foglia[ultimo] ^= 0x01; // rompe la firma, non la struttura DER

        let report = validate_chain(
            &[
                foglia,
                fixture("pv-intermediate.pem"),
                fixture("pv-ca.pem"),
            ],
            None,
        );
        assert!(!report.valid);
        assert!(!report.trusted);
        assert!(
            report
                .errors
                .iter()
                .any(|errore| errore.contains("Firma del certificato")
                    && errore.contains("non valida")),
            "errori: {:?}",
            report.errors
        );
    }

    #[test]
    fn catena_incompleta_non_attendibile() {
        let report = validate_chain(&[fixture("pv-leaf.pem")], None);
        assert!(!report.complete);
        assert!(!report.trusted);
        assert_eq!(report.chain, vec!["OpenPDF Verify Leaf"]);
    }

    #[test]
    fn radice_autofirmata_valida() {
        let report = validate_chain(&[fixture("pv-ca.pem")], None);
        assert!(report.valid, "errori: {:?}", report.errors);
        assert!(report.complete);
        assert!(report.trusted);
        assert_eq!(report.chain, vec!["OpenPDF Verify Root"]);
    }

    #[test]
    fn elenco_vuoto() {
        let report = validate_chain(&[], None);
        assert!(!report.valid);
        assert!(!report.complete);
        assert!(!report.trusted);
        assert_eq!(report.errors.len(), 1);
    }

    #[test]
    fn istante_esplicito_nel_futuro() {
        let certificati = catena_valida();
        let futuro = Utc.with_ymd_and_hms(2050, 1, 1, 0, 0, 0).unwrap();
        let report = validate_chain(&certificati, Some(futuro));
        assert!(!report.valid);
        assert!(
            report
                .errors
                .iter()
                .any(|errore| errore.contains("scaduto")),
            "errori: {:?}",
            report.errors
        );
    }

    #[test]
    fn catena_rsa_sha1() {
        let certificati = vec![
            fixture("pv-leaf-sha1.pem"),
            fixture("pv-intermediate.pem"),
            fixture("pv-ca.pem"),
        ];
        let report = validate_chain(&certificati, None);
        assert!(report.valid, "errori: {:?}", report.errors);
        assert!(report.complete);
        assert!(report.trusted);
    }

    #[test]
    fn catena_ecdsa_p256() {
        let certificati = vec![
            fixture("pv-leaf-ec.pem"),
            fixture("pv-ca-ec.pem"),
        ];
        let report = validate_chain(&certificati, None);
        assert!(report.valid, "errori: {:?}", report.errors);
        assert!(report.complete);
        assert!(report.trusted);
        assert_eq!(
            report.chain,
            vec!["OpenPDF Verify EC Leaf", "OpenPDF Verify EC Root"]
        );
    }

    // -- Attributi firmati ----------------------------------------------------

    fn der_tlv(tag: u8, contenuto: &[u8]) -> Vec<u8> {
        let mut uscita = vec![tag];
        let lunghezza = contenuto.len();
        if lunghezza < 0x80 {
            uscita.push(lunghezza as u8);
        } else if lunghezza <= 0xff {
            uscita.push(0x81);
            uscita.push(lunghezza as u8);
        } else {
            uscita.push(0x82);
            uscita.push((lunghezza >> 8) as u8);
            uscita.push(lunghezza as u8);
        }
        uscita.extend_from_slice(contenuto);
        uscita
    }

    fn attributo(tipo: ObjectIdentifier, valore: Vec<u8>) -> Attribute {
        Attribute {
            r#type: tipo,
            values: SetOf::from_vec(vec![Any::new(valore)]),
        }
    }

    fn signing_certificate_v2(
        certificato: &[u8],
        algoritmo: Option<&ObjectIdentifier>,
    ) -> Vec<u8> {
        let impronta = match algoritmo {
            None => Sha256::digest(certificato).to_vec(),
            Some(alg) if *alg == oid_sha1() => sha1(certificato).to_vec(),
            Some(alg) if *alg == oid_sha256() => Sha256::digest(certificato).to_vec(),
            Some(alg) if *alg == oid_sha384() => Sha384::digest(certificato).to_vec(),
            Some(_) => panic!("algoritmo non previsto dal test"),
        };

        let mut ess_cert_id = Vec::new();
        if let Some(alg) = algoritmo {
            let identificatore = AlgorithmIdentifier {
                algorithm: alg.clone(),
                parameters: None,
            };
            ess_cert_id.extend_from_slice(&rasn::der::encode(&identificatore).unwrap());
        }
        ess_cert_id.extend_from_slice(&der_tlv(0x04, &impronta));

        // SigningCertificateV2 ::= SEQUENCE { certs SEQUENCE OF ESSCertIDv2 }
        der_tlv(
            0x30,
            &der_tlv(0x30, &der_tlv(0x30, &ess_cert_id)),
        )
    }

    fn costruisci_attributi(
        digest: &[u8],
        certificato: &[u8],
        tipo_contenuto: &ObjectIdentifier,
        algoritmo_ess: Option<&ObjectIdentifier>,
    ) -> Vec<u8> {
        let mut elenco = vec![
            attributo(
                oid_content_type(),
                rasn::der::encode(tipo_contenuto).unwrap(),
            ),
            attributo(
                oid_message_digest(),
                rasn::der::encode(&OctetString::from(digest.to_vec())).unwrap(),
            ),
            attributo(
                oid_signing_certificate_v2(),
                signing_certificate_v2(certificato, algoritmo_ess),
            ),
        ];
        // DER richiede l'ordinamento dei SET OF per codifica degli elementi.
        elenco.sort_by_key(|voce| rasn::der::encode(voce).unwrap_or_default());
        rasn::der::encode(&SetOf::from_vec(elenco)).expect("attributi codificabili")
    }

    #[test]
    fn attributi_coerenti() {
        let certificato = fixture("pv-leaf.pem");
        let digest = Sha256::digest(b"contratto di prova").to_vec();
        let attributi = costruisci_attributi(&digest, &certificato, &oid_data(), None);

        let problemi = check_signed_attributes(&attributi, &digest, &certificato);
        assert!(problemi.is_empty(), "problemi: {problemi:?}");
    }

    #[test]
    fn set_con_tag_implicito_cms() {
        let certificato = fixture("pv-leaf.pem");
        let digest = Sha256::digest(b"contratto di prova").to_vec();
        let mut attributi = costruisci_attributi(&digest, &certificato, &oid_data(), None);
        attributi[0] = 0xa0; // [0] IMPLICIT dei SignedAttributes

        let problemi = check_signed_attributes(&attributi, &digest, &certificato);
        assert!(problemi.is_empty(), "problemi: {problemi:?}");
    }

    #[test]
    fn message_digest_errato() {
        let certificato = fixture("pv-leaf.pem");
        let digest = Sha256::digest(b"contratto di prova").to_vec();
        let altro = Sha256::digest(b"documento diverso").to_vec();
        let attributi = costruisci_attributi(&digest, &certificato, &oid_data(), None);

        let problemi = check_signed_attributes(&attributi, &altro, &certificato);
        assert!(
            problemi
                .iter()
                .any(|problema| problema.contains("messageDigest non corrisponde")),
            "problemi: {problemi:?}"
        );
    }

    #[test]
    fn signing_certificate_v2_errato() {
        let firmatario = fixture("pv-leaf.pem");
        let altro_certificato = fixture("pv-leaf-expired.pem");
        let digest = Sha256::digest(b"contratto di prova").to_vec();
        let attributi =
            costruisci_attributi(&digest, &altro_certificato, &oid_data(), None);

        let problemi = check_signed_attributes(&attributi, &digest, &firmatario);
        assert!(
            problemi
                .iter()
                .any(|problema| problema.contains("signingCertificateV2")
                    && problema.contains("non corrisponde")),
            "problemi: {problemi:?}"
        );
    }

    #[test]
    fn content_type_errato() {
        let certificato = fixture("pv-leaf.pem");
        let digest = Sha256::digest(b"contratto di prova").to_vec();
        let signed_data = oid(&[1, 2, 840, 113549, 1, 7, 2]);
        let attributi = costruisci_attributi(&digest, &certificato, &signed_data, None);

        let problemi = check_signed_attributes(&attributi, &digest, &certificato);
        assert!(
            problemi
                .iter()
                .any(|problema| problema.contains("contentType") && problema.contains("id-data")),
            "problemi: {problemi:?}"
        );
    }

    #[test]
    fn ess_con_sha1_esplicito() {
        let certificato = fixture("pv-leaf.pem");
        let digest = Sha256::digest(b"contratto di prova").to_vec();
        let attributi = costruisci_attributi(
            &digest,
            &certificato,
            &oid_data(),
            Some(&oid_sha1()),
        );

        let problemi = check_signed_attributes(&attributi, &digest, &certificato);
        assert!(problemi.is_empty(), "problemi: {problemi:?}");
    }

    #[test]
    fn attributi_malformati() {
        let problemi = check_signed_attributes(&[0x31, 0x02, 0xff, 0xff], &[0u8; 32], &[]);
        assert!(!problemi.is_empty());
        assert!(
            problemi
                .iter()
                .any(|problema| problema.contains("non decodificabili")),
            "problemi: {problemi:?}"
        );
    }

    #[test]
    fn sha1_vettore_noto() {
        // FIPS 180-1: SHA-1("abc").
        let atteso = [
            0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
            0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d,
        ];
        assert_eq!(sha1(b"abc"), atteso);
    }
}
