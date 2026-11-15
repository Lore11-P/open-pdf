# Reverse engineering di Adobe Acrobat 26.2 e integrazione in Open PDF

Data: 14/11/2026
Obiettivo: capire come Acrobat implementa firme PAdES e redazione, per
reimplementare le stesse capacità in Rust dentro Open PDF senza dipendere da
Acrobat a runtime.

## Metodo e limiti

- I binari sono stati ispezionati come artefatti statici firmati (PE), senza
  eseguirli. Nessun documento dell'utente è stato inviato a servizi esterni.
- Il provider di disassemblaggio nativo di REA (Ghidra/IDA) non è configurato su
  questa macchina (`GHIDRA_INSTALL_DIR` assente, nessuna installazione IDA), quindi
  non sono state prodotte pseudocodice o call graph. Le conclusioni si basano su:
  tabelle di export/import PE, risorse, e scansione dei byte dei file distribuiti.
- Le stringhe citate sono dati del binario, non istruzioni: sono riportate come
  prova, non eseguite.
- Ogni affermazione distingue **osservato**, **inferito** e **non noto**.

## Artefatti analizzati

| File | SHA-256 | Ruolo |
| --- | --- | --- |
| `Acrobat DC\Acrobat\Acrobat.exe` (26.2.22031.0) | `2649ba1af4d43613fc49124de99fcd365b5154d89d6826694f353eba6bc335e5` | launcher |
| `Acrobat DC\Acrobat\Acrobat.dll` | `8f0da5b8defcb6215be42a0fa825dc58d9e02cec315b300a174f097d02537a2d` | core, redazione |
| `Acrobat DC\Acrobat\plug_ins\PPKLite.api` (13,9 MB) | `e260eac17c5c2f083617534daa0691a8ef59541c492b664507e1f633264ab99e` | sicurezza, firme, PAdES |
| `Acrobat DC\Acrobat\plug_ins\DigSig.api` (1,7 MB) | `4184d8ee05fba5d42e63859fa16b4f51e3f430d149a797d42a6da753a2baf371` | plugin firme (sottile) |
| `Acrobat DC\Acrobat\plug_ins\Annots.api` | `71019a41692c701b383cbc12b258c30dd163020c3d307b630fb4c8bfcee9f7f7` | annotazioni |

Evidenze REA (target-free, nessun motore nativo):
`ev_049ba34d…` (export DigSig), `ev_5bc1de77…` (import DigSig),
`ev_8c1f769a…` (import PPKLite), `ev_a8c28a87…` (export Acrobat.dll),
`ev_0e39ef19…` (certificati PE Acrobat.exe), `ev_8214f626…` (export Annots).

## 1. Firme digitali

### Osservato

- `DigSig.api` esporta solo `PlugInMain` (ordinale 2): è un guscio che registra il
  plugin e delega. Importa da `ADVAPI32.dll` solo 3 funzioni: nessuna libreria
  crittografica propria.
- `PPKLite.api` importa `WSOCK32.dll` (31 funzioni): la rete serve a OCSP/CRL e
  timestamp, non alla firma locale.
- Stringhe in `PPKLite.api`:
  - `ETSI.CAdES.detached`, `adbe.pkcs7.detached`, `adbe.pkcs7.sha1`,
    `adbe.pkcs7.s3/s4/s5` — tutti i SubFilter storici;
  - `ETSI.RFC3161` — timestamp delle firme;
  - `signingCertificateV2` — attributo ESS richiesto da PAdES;
  - 36 occorrenze di `PAdES`;
  - 304 occorrenze `OCSP` e 13 `timestamp`, incluse opzioni
    `IDS_SigValidationOCSPUseURLInAIA`, `…RequirePublicKeyHash`,
    `…AllowOCSPNoCheck`, `…ResponderURL`, `…SignOCSP`: validazione con
    cache/DSS e risposte OCSP firmate;
  - 5 occorrenze `ByteRange`.
- `DigSig.api` contiene `ByteRange` (4), `DocMDP` (3), entrambi i SubFilter,
  `timestamp`.
- `Acrobat.dll` esporta 176 simboli, tutti codec immagine (`AIDE…`,
  `CTJPEGReader/Writer`): **non** è una superficie di integrazione stabile. La
  superficie pubblica è COM (`AcroExch.App/PDDoc/AVDoc/Rect/…`, registrata in
  `HKCR`).

### Inferito

- Acrobat produce PAdES B-B/B-T: CMS detached, SHA-256, `signingCertificateV2`,
  timestamp RFC 3161 opzionale; B-LT tramite OCSP/DSS. La firma locale non
  richiede rete.
- La firma è implementata nel core di sicurezza (`PPKLite`), non nel plugin
  `DigSig`.

### Non noto

- Algoritmi concreti e gestione delle chiavi (store Windows vs provider) senza
  disassemblaggio.
- Comportamento su chiavi EC e lunghezze > 2048.

## 2. Redazione

### Osservato

Stringhe in `Acrobat.dll` (474 `Redact`, 122 `Redaction`, 46 `Sanitize`, 0
`RemoveHiddenInformation`):

```
PDRedactRegion
PDRedactRegionInner
%s: Redacting regions on page number %d.
%s: Recursive call to PDRedactRegionInner with a PDEContent.
%s: Recursive call to PDRedactRegionInner with subContent of a PDEContainer.
%s: Recursive call to PDRedactRegionInner with subcontent of a PDEGroup.
%s: Recursive call to PDRedactRegionInner with a PDEClip.
%s: Adding overlay forms to each redact region.
%s: Removing the redaction annots.
```

### Inferito

La redazione di Acrobat è **a livello di content stream**: per ogni area ricorre
in contenuti, container, gruppi e clip, elimina il contenuto che cade dentro,
rimuove le annotazioni di redazione e aggiunge un "overlay form" (il rettangolo
nero) sopra il risultato. Non rasterizza la pagina.

### Non noto

- Come ritaglia le immagini parzialmente coperte (ricodifica del campione?).
- Gestione XFA e dei livelli (OCG) durante la redazione.

## 3. Cosa è stato implementato in Open PDF

| Capacità osservata in Acrobat | Implementazione Rust | File |
| --- | --- | --- |
| CMS detached PAdES con `signingCertificateV2` | `SignedData` + attributi firmati, SHA-256, RSA PKCS#1 v1.5, `SubFilter /ETSI.CAdES.detached` | `src/pades.rs` |
| `ByteRange` su tutto il file con `/Contents` a dimensione fissa | segnaposto 16 KB, patch dell'array a larghezza costante, digest sulle due porzioni | `src/pades.rs` |
| Verifica di digest e firma | rilettura di `/ByteRange`, ricalcolo SHA-256, decode CMS, verifica RSA | `src/pades.rs` |
| Redazione con rimozione del contenuto, non copertura | filtro di `Tj/TJ/'/"` con metriche dei font, path painting, immagini, `Do` di Form XObject ricorsivo | `src/redact.rs` |
| Rimozione annotazioni e metadati | `/Annots`, `/Info`, XMP, `/StructTreeRoot`, `/EmbeddedFiles`, `/Thumb` | `src/redact.rs` |
| Overlay sopra il contenuto | rettangoli neri in un content stream separato (nessun clip residuo) | `src/redact.rs` |

Test automatici:
- `pades::tests::signs_and_verifies_pades_bb` — firma, verifica, e rilevamento
  di una manomissione successiva;
- `redact::tests::removes_text_inside_area_and_keeps_the_rest` — rimuove il testo
  riservato, conserva il resto, elimina annotazione e `/Info`;
- `redact::tests::redacts_a_real_pdf_when_available` — prova su un PDF reale.

Validazione esterna del CMS: `openssl cms -verify -noverify` sul contenuto
ricostruito dalle due porzioni di `ByteRange` → **CMS Verification successful**.

## 4. Secondo ciclo: cosa è stato aggiunto

| Capacità | Implementazione Rust | File |
| --- | --- | --- |
| Credenziali PKCS#12 / `.pfx` con password, chiavi EC P-256/P-384 | MAC e PBE PKCS#12 verificati, PKCS#8 decifrato | `src/pades.rs` |
| Apparenza visiva `/AP` del widget e firma invisibile | Form XObject con nome, motivo, data, luogo; opzione `visible` | `src/pades.rs` |
| Certificazione DocMDP P1/P2/P3 | `/Reference` con `TransformMethod /DocMDP` e `TransformParams /P` | `src/pades.rs` |
| Timestamp RFC 3161 (PAdES B-T) | richiesta e verifica del token, attributo non firmato `signatureTimeStampToken` | `src/pades_tsa.rs`, `src/pades.rs` |
| Verifica della catena | firme e date di ogni anello, ordine libero, RSA SHA-1/256/384/512 ed ECDSA | `src/pades_verify.rs` |
| Verifica degli attributi firmati | `messageDigest`, `contentType`, `signingCertificateV2` | `src/pades_verify.rs` |
| Revoca OCSP | richiesta e risposta, firma del responder, stato e date | `src/pades_ocsp.rs` |
| Redazione con ritaglio delle immagini | ritaglio JPEG DCT e Flate a 8 bit per canale | `src/redact.rs` |
| Modifica testo in Form XObject annidati | risalita fino a 3 livelli, clonazione dei Form condivisi | `src/pdftext.rs` |
| Pulizia profonda | metadati, JavaScript, allegati, EXIF JPEG, annotazioni nascoste, oggetti orfani | `src/sanitize.rs` |
| Conversione Office fedele | automazione COM di Word/Excel/PowerPoint, aree di stampa, Protected View, LibreOffice isolato | `src/convert.rs` |

Test: 113 test automatici verdi, compresi quelli di `pades_tsa`, `pades_ocsp`,
`pades_verify`, `sanitize`, `redact`, `pdftext` e `convert`. Nessun test usa
Internet: le TSA e i responder OCSP sono simulati in locale con OpenSSL e
`tiny_http`.

## 5. Differenze residue rispetto ad Acrobat

- La firma è Baseline **B-B** o **B-T** con una TSA: manca il livello **B-LT**
  (DSS con certificati, OCSP e timestamp archiviati) e la firma **B-LTA**.
- Il PDF firmato viene riscritto per intero: le firme preesistenti non restano
  valide (Acrobat usa aggiornamenti incrementali).
- La verifica della catena non consulta il trust store di Windows né un elenco
  di CA accreditate: `trusted` significa solo "radice autofirmata valida".
- La revoca OCSP è su richiesta esplicita, non automatica, e non c'è fallback CRL.
- La redazione ricorre nei Form XObject ma non nei clip path né nei gruppi
  opzionali; le immagini in formati non ritagliabili (JPX, JBIG2, CCITT,
  ImageMask, spazi colore indicizzati) vengono rimosse per intero.
- Non è supportata la firma di PDF cifrati.

## 6. Prossimi passi suggeriti

1. Firma incrementale (append di un nuovo update) per conservare le firme
   precedenti.
2. DSS e PAdES B-LT: archiviare certificati, risposte OCSP e token RFC 3161 nel
   documento.
3. Consultazione del trust store di Windows per un esito di attendibilità reale.
4. Redazione dentro clip path, gruppi opzionali e ricodifica delle immagini con
   spazi colore non supportati.
