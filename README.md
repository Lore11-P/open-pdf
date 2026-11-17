# Open PDF

Lettore e editor PDF **locale** per Windows: interfaccia desktop in Tauri 2, motore di
rendering PDFium, logica in Rust. Nessun documento viene caricato su servizi cloud.

Il progetto è nato come alternativa pratica ad Acrobat per le operazioni che servono
davvero tutti i giorni: leggere, annotare, compilare moduli, **modificare il testo
esistente**, **redigere in modo irreversibile**, convertire da Office e **firmare in
formato PAdES**. Le funzionalità sono state ricostruite studiando il comportamento dei
binari Adobe (vedi [docs/reverse-engineering-acrobat.md](docs/reverse-engineering-acrobat.md));
il codice di Acrobat non è incluso né richiesto a runtime.

> **Stato:** applicazione Windows 10/11 a 64 bit. Su Linux e macOS **non è
> disponibile** una build: vedi [Linux e macOS](#linux-e-macos).

---

## Funzionalità

### Lettura e annotazione
- Apertura PDF, miniature, ricerca nel testo, zoom con Ctrl+rotella o pinch.
- Lettura ad alta voce con la voce italiana di Windows (SAPI, offline).
- Evidenziatore, sottolineatura, note adesive, timbri, disegno a mano libera,
  blocchi di testo e firma a mano libera o scritta.
- Annulla/ripristina ed elenco completo delle annotazioni; le annotazioni vengono
  impresse nel PDF esportato.

### Modifica del contenuto
- **Modifica del testo in place**: l'operazione di testo originale viene sostituita nel
  content stream, non coperta da un'annotazione. Il testo resta selezionabile e
  ricercabile e conserva posizione, corpo, colore, `Tc`/`Tw`/`Tz` e leading.
- Supporto a testi multi-riga e a contenuti dentro Form XObject annidati (fino a 3
  livelli), con clonazione dei Form condivisi tra più istanze.
- **Redazione irreversibile**: rimuove testo, immagini, path e link dentro le aree a
  livello di content stream, elimina annotazioni e metadati collegati e disegna i
  rettangoli neri. Le immagini parzialmente coperte vengono ritagliate quando il
  formato lo consente (JPEG DCT e Flate a 8 bit per canale).
- **Pulizia profonda**: metadati e XMP, JavaScript e azioni automatiche, allegati,
  EXIF nelle immagini JPEG, annotazioni nascoste, struttura di accessibilità e
  oggetti orfani, con riapertura del documento pulito.

### Pagine e conversione
- Riordino, eliminazione, estrazione, unione e rotazione delle pagine.
- Compressione senza perdita o con rasterizzazione a qualità regolabile.
- Immagini → PDF, pagina web → PDF (Microsoft Edge headless).
- Office → PDF con automazione locale di Word/Excel/PowerPoint, fallback LibreOffice e,
  come ultima risorsa, estrazione del testo. Gestisce aree di stampa e Protected View.
- File di testo → PDF senza Office: `.txt`, `.md`, `.csv`, `.log`, `.json` e simili
  vengono impaginati direttamente (UTF-8, con ripiego su Latin-1).
- Esportazione PDF → Word/Excel/PowerPoint/testo.
- OCR con il motore di Windows e livello OCR invisibile per rendere ricercabili le
  scansioni.

### Moduli e firme
- Compilazione di moduli esistenti e creazione di campi di testo e caselle di spunta.
- **Firma PAdES**:
  - credenziali PEM (RSA, EC P-256/P-384) oppure PKCS#12/`.pfx` con password;
  - CMS `ETSI.CAdES.detached`, SHA-256/384, `signingCertificateV2`, `/ByteRange`
    sull'intero file e `/Contents` a dimensione fissa;
  - timbro visibile con nome, motivo, data e luogo, oppure firma invisibile;
  - certificazione **DocMDP** P1/P2/P3;
  - **marca temporale RFC 3161** opzionale: la firma passa da Baseline B-B a B-T.
- **Verifica delle firme**: digest del documento, firma CMS, copertura del file, catena
  dei certificati (firme e date di ogni anello), attributi firmati (`messageDigest`,
  `contentType`, `signingCertificateV2`), marca temporale e livello DocMDP. Con
  «Controlla revoca online» interroga il responder **OCSP**.
- Richiesta di firme nella rete locale: link protetto da token, commenti e firme dal
  browser, senza Internet.

### Sicurezza
- Password di apertura e permessi di stampa/copia/modifica con AES-128.
- Confronto del testo tra due versioni dello stesso PDF.

---

## Requisiti

- Windows 10/11 a 64 bit con **Microsoft Edge WebView2 Runtime** (presente di default
  su Windows 11).
- Per la build: Rust stable, toolchain `x86_64-pc-windows-gnu` e MSYS2 (manca il linker
  MSVC su molte installazioni), più `pdfium.dll` in `engines/`.
- Opzionali: Word/Excel/PowerPoint per la conversione Office fedele, LibreOffice come
  alternativa, FFmpeg per alcuni formati immagine.

## Build da sorgente

```powershell
# da PowerShell, nella cartella del progetto
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
cargo +stable-x86_64-pc-windows-gnu build --release
```

L'eseguibile finisce in `target\release\open-pdf.exe`; accanto servono `pdfium.dll` e
`WebView2Loader.dll` (forniti da Tauri/PDFium). La UI è in `ui/` (HTML/CSS/JS senza
bundler) e viene incorporata nell'eseguibile tramite `frontendDist`.

```powershell
cargo +stable-x86_64-pc-windows-gnu test
```

## Test

La suite conta **115 test automatici** che coprono firma, verifica, redazione, modifica
testo, conversione, OCR, condivisione, sicurezza e il server MCP. I test non usano
Internet: TSA e responder OCSP sono simulati in locale con OpenSSL e `tiny_http`. Le
chiavi e i certificati in `tests/fixtures/` sono **usa e getta** e servono solo ai test.

## Struttura

| Percorso | Contenuto |
| --- | --- |
| `src/pades.rs` | firma PAdES B-B/B-T, PKCS#12, apparenza, DocMDP |
| `src/pades_tsa.rs` | richiesta e verifica di marche temporali RFC 3161 |
| `src/pades_ocsp.rs` | richiesta, parsing e verifica delle risposte OCSP |
| `src/pades_verify.rs` | catena di certificati e attributi CMS firmati |
| `src/redact.rs` | redazione a livello di content stream e ritaglio immagini |
| `src/pdftext.rs` | modifica del testo esistente in place |
| `src/sanitize.rs` | pulizia profonda dei dati nascosti |
| `src/convert.rs` | Office/Web → PDF ed esportazioni |
| `src/engine.rs`, `src/ocr.rs` | rendering PDFium, OCR e voce |
| `src/collab.rs` | revisione collaborativa in rete locale |
| `src/main.rs` | comandi Tauri e stato della sessione |
| `src/mcp_server.rs` | server MCP (stdio) per gli agenti AI |
| `ui/` | interfaccia Focus Dark |

## Uso da un agente AI (MCP)

C'è un secondo eseguibile, `open-pdf-mcp.exe`, che espone le stesse funzioni a un
agente AI tramite il **Model Context Protocol** (JSON-RPC 2.0 su stdio). Lavora in
locale e non invia nulla in rete.

```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
cargo +stable-x86_64-pc-windows-gnu build --release --bin open-pdf-mcp
```

Serve `pdfium.dll` accanto all'eseguibile (come per l'app) oppure la variabile
`OPENPDF_PDFIUM` con il percorso della DLL. Registrazione in un client MCP (Claude
Desktop, Claude Code, OpenCode e altri):

```json
{
  "mcpServers": {
    "open-pdf": {
      "command": "C:\\percorso\\open-pdf-mcp.exe"
    }
  }
}
```

Strumenti disponibili: `pdf_info`, `pdf_text`, `pdf_text_lines`, `pdf_search`,
`pdf_render_page`, `pdf_from_text`, `pdf_edit_text`, `pdf_delete_text`, `pdf_redact`,
`pdf_sanitize`, `pdf_sign_pades`, `pdf_verify_signatures`, `pdf_office_to_pdf`,
`pdf_export`, `pdf_merge`, `pdf_extract_pages`, `pdf_rotate_page`, `pdf_compress`,
`pdf_images_to_pdf`, `pdf_protect`.

Regole comuni: gli indici di pagina partono da 0, i rettangoli sono normalizzati
`[x, y, larghezza, altezza]` con origine in alto a sinistra, e ogni operazione di
scrittura richiede un `output` separato: l'originale non viene mai sovrascritto.
`pdf_text_lines` restituisce i rettangoli da passare a `pdf_edit_text` e
`pdf_delete_text`; `pdf_render_page` restituisce un PNG come immagine, utile per le
scansioni.

## Privacy

Tutto avviene sul computer dell'utente. L'originale non viene mai sovrascritto: le
modifiche sono salvate su copie in `%APPDATA%\OpenPDF`. La condivisione LAN espone il
documento solo nella rete locale, protetto da un token casuale nel link.

## Limiti dichiarati

- La firma è Baseline **B-B** o **B-T**: non produce un DSS con certificati, OCSP e
  timestamp archiviati (**B-LT/B-LTA**) e non è una firma qualificata eIDAS.
- La catena è verificata sulle firme e sulle date dei certificati inclusi, **non**
  contro il trust store di Windows: "attendibile" significa solo radice autofirmata
  valida.
- La revoca OCSP è su richiesta esplicita, senza fallback CRL.
- Il PDF firmato viene riscritto per intero: le firme già presenti non restano valide
  (manca la firma incrementale).
- Firma di PDF cifrati non supportata.
- La modifica del testo usa Helvetica quando il font originale è incorporato o
  sottoinsiemato e rifiuta i caratteri fuori da WinAnsi (per esempio CJK).
- La redazione non entra nei clip path né nei gruppi opzionali; i formati immagine non
  ritagliabili (JPX, JBIG2, CCITT, ImageMask, spazi colore indicizzati) vengono rimossi
  per intero.
- L'esportazione PDF → Office produce documenti con il testo estratto, senza
  ricostruire l'impaginazione.

## Linux e macOS

**Oggi no.** L'applicazione è Windows-only: non esiste una build per Linux o macOS e
non è testata su Wine (WebView2 sotto Wine è inaffidabile). La buona notizia è che la
parte difficile è già portabile: firma, verifica, redazione, modifica testo, sanitize,
gestione pagine e UI sono Rust/HTML standard.

Per una build Linux servirebbe sostituire i moduli legati a Windows:

| Modulo | Dipendenza Windows | Alternativa Linux |
| --- | --- | --- |
| rendering | `pdfium.dll` | `libpdfium.so` + percorso in `engine.rs` |
| OCR | `Windows.Media.Ocr` via PowerShell | Tesseract |
| voce | `System.Speech` (SAPI) | Piper / eSpeak NG |
| Office | COM di Word/Excel/PowerPoint | solo LibreOffice headless (già presente come fallback) |
| web → PDF | Microsoft Edge headless | Chromium headless |
| immagini esotiche | WIC/GDI+ | `image` crate, FFmpeg o ImageMagick |
| dati locali | `%APPDATA%` | `$XDG_DATA_HOME` |
| packaging | NSIS | `.deb`/AppImage |
| TSA/OCSP | `curl.exe` | `curl` (già previsto con `cfg`) |

Stima realistica: **2-4 settimane** per una build Linux usabile, più tempo per
rifinire OCR e conversione Office. Le pull request in questa direzione sono benvenute;
prima però conviene aprire una issue per concordare l'approccio.

## Note legali

- Il reverse engineering documentato in `docs/` è stato fatto per **interoperabilità**
  su copie legalmente possedute di Acrobat, senza aggirare protezioni e senza
  ridistribuire codice Adobe. Il decompilato prodotto con Ghidra è escluso dal
  repository.
- PDFium è distribuito con licenza BSD-style: i testi delle licenze di terze parti
  sono in [`licenze/`](licenze/).
- Adobe e Acrobat sono marchi di Adobe Inc.; questo progetto non è affiliato ad Adobe.

## Licenza

[MIT](LICENSE).
