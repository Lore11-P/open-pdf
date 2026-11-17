# Open PDF MCP

Server **MCP** (Model Context Protocol) locale per Windows che dà a un agente AI gli
strumenti di Open PDF: leggere, cercare, modificare, redigere, pulire, firmare e
convertire PDF, oltre a **creare documenti nuovi** in PDF, PowerPoint, Word, testo,
Markdown e HTML. Gira come processo figlio del client e parla JSON-RPC 2.0 su stdio:
nessun documento esce dal computer.

Funziona con qualsiasi client MCP che avvii server stdio. Le configurazioni provate
sono **Claude Code**, **Codex CLI**, **OpenCode**, **Claude Desktop**, **Cursor** e
**VS Code**.

---

## 1. Ottenere l'eseguibile

### Opzione A — archivio della release (consigliata)

Scarica `open-pdf-mcp-windows-x64.zip` dalla pagina
[Releases](https://github.com/Lore11-P/open-pdf/releases) e
scompattalo in una cartella stabile, per esempio `C:\OpenPDF`. L'archivio contiene:

| File | A cosa serve |
| --- | --- |
| `open-pdf-mcp.exe` | il server MCP |
| `pdfium.dll` | motore di rendering e scrittura PDF (obbligatorio) |
| `LICENSE`, `pdfium-LICENSE.txt` | licenze MIT e BSD |

### Opzione B — compilare da sorgente

Servono Rust stable con toolchain `x86_64-pc-windows-gnu` e MSYS2 (manca il linker
MSVC su molte installazioni):

```powershell
$env:PATH = "C:\msys64\mingw64\bin;" + $env:PATH
cargo +stable-x86_64-pc-windows-gnu build --release --bin open-pdf-mcp
```

L'eseguibile finisce in `target\release\open-pdf-mcp.exe`; copia accanto
`engines\pdfium.dll`.

### Dove cerca `pdfium.dll`

1. accanto a `open-pdf-mcp.exe`;
2. nella cartella `engines\` accanto all'eseguibile;
3. nel percorso indicato dalla variabile d'ambiente `OPENPDF_PDFIUM`.

### Prova rapida

```powershell
.\open-pdf-mcp.exe
```

Su stderr compare `Open PDF MCP 1.0.0 in ascolto su stdio`, poi il processo attende
JSON su stdin. `Ctrl+C` per uscire: se resta in attesa senza errori, va bene.

---

## 2. Claude Code

```powershell
claude mcp add open-pdf -- "C:\OpenPDF\open-pdf-mcp.exe"
```

- `-s user` registra il server per tutti i progetti, `-s project` lo scrive in
  `.mcp.json` del repository.
- Verifica dentro Claude Code con `/mcp`: deve comparire `open-pdf` con i suoi
  strumenti.
- Rimozione: `claude mcp remove open-pdf`.

Le virgolette servono sui percorsi con spazi; su Windows usa il percorso assoluto
dell'exe.

## 3. Codex CLI

```powershell
codex mcp add open-pdf -- "C:\OpenPDF\open-pdf-mcp.exe"
codex mcp list
```

Oppure a mano in `%USERPROFILE%\.codex\config.toml`:

```toml
[mcp_servers.open-pdf]
command = 'C:\OpenPDF\open-pdf-mcp.exe'
```

In una sessione Codex, `/mcp` mostra lo stato della connessione e gli strumenti.

## 4. OpenCode

In `opencode.json` (globale: `%USERPROFILE%\.config\opencode\opencode.json`):

```json
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "open-pdf": {
      "type": "local",
      "command": ["C:\\OpenPDF\\open-pdf-mcp.exe"],
      "enabled": true
    }
  }
}
```

Riavvia OpenCode: gli strumenti `pdf_*` e `doc_create` compaiono nella sessione.

## 5. Claude Desktop

In `%APPDATA%\Claude\claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "open-pdf": {
      "command": "C:\\OpenPDF\\open-pdf-mcp.exe"
    }
  }
}
```

Chiudi del tutto Claude Desktop (icona nella tray compresa) e riaprilo.

## 6. Cursor e VS Code

Cursor, `%USERPROFILE%\.cursor\mcp.json`:

```json
{
  "mcpServers": {
    "open-pdf": {
      "command": "C:\\OpenPDF\\open-pdf-mcp.exe"
    }
  }
}
```

VS Code, `.vscode/mcp.json` (nota la chiave `servers` e il `type`):

```json
{
  "servers": {
    "open-pdf": {
      "type": "stdio",
      "command": "C:\\OpenPDF\\open-pdf-mcp.exe"
    }
  }
}
```

---

## 7. Strumenti

### Creare e convertire

| Strumento | Cosa fa |
| --- | --- |
| `doc_create` | Crea un documento da zero: `pdf`, `pptx`, `docx`, `txt`, `md`, `html`. Il formato viene dall'estensione di `output`. |
| `pdf_from_text` | PDF A4 impaginato da testo semplice. |
| `pdf_office_to_pdf` | Word/Excel/PowerPoint, oppure `.txt`, `.md`, `.csv`, `.log`, `.json` → PDF. |
| `pdf_export` | PDF → testo, Word, Excel, PowerPoint. |
| `pdf_images_to_pdf` | Immagini → PDF, una per pagina. |
| `pdf_merge` | Unisce più PDF nell'ordine indicato. |
| `pdf_extract_pages` | Estrae pagine in un nuovo PDF. |
| `pdf_rotate_page` | Ruota una pagina di 90/180/270 gradi. |
| `pdf_compress` | Compressione senza perdita o rasterizzazione a DPI e qualità scelti. |

### Leggere e cercare

| Strumento | Cosa fa |
| --- | --- |
| `pdf_info` | Pagine, dimensioni, metadati, campi modulo. |
| `pdf_text` | Testo pagina per pagina. |
| `pdf_text_lines` | Righe con i rettangoli, da passare a modifica ed eliminazione. |
| `pdf_search` | Ricerca nel testo, con pagina e rettangolo di ogni occorrenza. |
| `pdf_render_page` | Rende una pagina in PNG (immagine restituita al modello). |

### Modificare e proteggere

| Strumento | Cosa fa |
| --- | --- |
| `pdf_edit_text` | Sostituisce il testo esistente nel content stream, non sopra un'annotazione. |
| `pdf_delete_text` | Elimina le righe che intersecano i rettangoli indicati. |
| `pdf_redact` | Redazione irreversibile: rimuove testo, immagini, path e link nelle aree. |
| `pdf_sanitize` | Rimuove metadati, XMP, JavaScript, allegati, EXIF, annotazioni nascoste. |
| `pdf_protect` | Password di apertura e permessi con AES-128. |

### Firmare

| Strumento | Cosa fa |
| --- | --- |
| `pdf_sign_pades` | Firma PAdES B-B o B-T (marca temporale RFC 3161), visibile o invisibile, DocMDP. |
| `pdf_verify_signatures` | Verifica digest, CMS, catena, attributi firmati, marca temporale, revoca OCSP. |

---

## 8. Esempi

### Creare una presentazione

`doc_create` accetta markdown leggero: `##` apre una sezione, `-` crea un punto
elenco. Per un `.pptx` produce una slide di titolo e una slide per sezione, con punti
elenco veri e testo modificabile.

```json
{
  "name": "doc_create",
  "arguments": {
    "output": "C:\\Users\\utente\\Desktop\\profilo.pptx",
    "title": "Profilo aziendale",
    "content": "## Chi siamo\n- Azienda locale dal 2019\n- Sei persone, tre sedi\n\n## Contatti\n- info@esempio.it"
  }
}
```

Lo stesso contenuto con `output` che termina in `.pdf`, `.docx`, `.txt`, `.md` o
`.html` produce il formato corrispondente, senza cambiare nient'altro.

### Leggere e redigere

```
pdf_info   { "path": "contratto.pdf" }
pdf_text   { "path": "contratto.pdf", "from": 1, "to": 2 }
pdf_redact { "path": "contratto.pdf", "output": "contratto-pulito.pdf",
             "areas": [{ "page": 0, "rect": [0.62, 0.18, 0.25, 0.06] }] }
```

### Firmare e verificare

```
pdf_sign_pades        { "path": "verbale.pdf", "output": "verbale-firmato.pdf",
                        "certificate": "mario.pfx", "certificate_password": "...",
                        "name": "Mario Rossi", "reason": "Approvazione" }
pdf_verify_signatures { "path": "verbale-firmato.pdf" }
```

---

## 9. Regole comuni

- **Gli indici di pagina partono da 0.**
- **I rettangoli sono normalizzati** `[x, y, larghezza, altezza]`, con origine in alto
  a sinistra e valori fra 0 e 1.
- **Ogni scrittura richiede un `output`**: il file di partenza non viene mai
  sovrascritto.
- Gli errori arrivano come `isError: true` con il messaggio in `content[0].text`.

## 10. Limiti

- Solo **Windows 10/11 a 64 bit**: l'app usa PDFium per Windows, COM di Office, OCR e
  voce di Windows.
- `doc_create` genera **testo impaginato**, non grafica: il PPTX ha titolo, sezioni e
  punti elenco; il DOCX un paragrafo per riga. Per un layout curato serve un editor.
- La conversione Office → PDF usa Word/Excel/PowerPoint locali, poi LibreOffice, poi
  l'estrazione del testo. Senza Office e senza LibreOffice il risultato è testo, non
  impaginazione.
- La firma è Baseline B-B o B-T: non è una firma qualificata eIDAS e non produce
  B-LT/B-LTA.
- `pdf_export` ricostruisce il testo, non l'impaginazione originale.

## 11. Problemi frequenti

| Sintomo | Causa e rimedio |
| --- | --- |
| Lo strumento non compare nel client | Percorso relativo o exe non avviabile: usa il percorso assoluto, avvia l'exe da terminale e riavvia il client. |
| `pdfium.dll non trovato` | Metti la DLL accanto all'exe oppure imposta `OPENPDF_PDFIUM` con il percorso completo. |
| Funziona nel terminale ma non nel client | Il client parte con un ambiente diverso: scrivi percorsi assoluti senza `~`, e non contare su variabili impostate solo nella tua shell. |
| Errore di parola d'ordine | Passa `password` negli argomenti dello strumento; per i PDF protetti da permessi serve la password utente. |
| `doc_create` rifiuta il formato | L'estensione di `output` deve essere `pdf`, `pptx`, `docx`, `txt`, `md` o `html`. |

## 12. Privacy

Il server non apre connessioni verso Internet: legge e scrive solo file locali. Le
uniche eccezioni sono esplicite e richieste dall'utente: `pdf_verify_signatures` con
`check_revocation` (OCSP), `pdf_sign_pades` con `tsa_url` (marca temporale) e
`pdf_office_to_pdf` quando il sistema avvia Office.

Licenza MIT. PDFium è distribuito con licenza BSD-style; i testi sono in
[`licenze/`](../licenze/).
