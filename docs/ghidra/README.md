# Ghidra per REA — installazione e decompilato Acrobat

Data: 2026-11-15
Macchina: Windows 11, utente non amministratore (`C:\Users\utente`)
Obiettivo: disassemblatore headless utilizzabile da REA + decompilato dei punti rilevanti
dei binari Adobe Acrobat (solo analisi statica, nessuna esecuzione dei target).

---

## 1. Versioni installate

| Componente | Versione | Percorso |
|---|---|---|
| Ghidra | **12.2** (build 20261016, release ufficiale NSA) | `C:\Users\utente\Tools\ghidra` |
| JDK usato da Ghidra | **Temurin 25.0.5+8 (LTS)** | `C:\Users\utente\Tools\jdk25` |
| JDK aggiuntivo (non usato) | Temurin 21.0.13+9 (LTS) | `C:\Users\utente\Tools\jdk21` |
| Java di sistema preesistente | Oracle 1.8.0_503 (JRE 8, su PATH di sistema) | `C:\Program Files (x86)\Common Files\Oracle\Java\java8path` |

**Perché JDK 25 e non 21:** Ghidra 12.2 dichiara in `Ghidra\application.properties`
`application.java.min=25` (e `application.java.compiler=25`). Con JDK 21 `analyzeHeadless`
rifiuta l'avvio ("JDK 25+ (64-bit) could not be found"). Il JDK 21 è stato installato per
primo seguendo la richiesta iniziale, ma è **inutilizzato**: si può eliminare.

Checksum verificati:

- `ghidra_12.2_PUBLIC_20261016.zip` — SHA-256 `2c2cd776db3c1f6a097ba418922378ac3649cadd5e844a6f5f088ae628a7be67`
  (pubblicato nelle release notes GitHub; **corrisponde**)
- `OpenJDK25U-jdk_x64_windows_hotspot_25.0.5_8.zip` — SHA-256 `636bdc13dc649ddc7d5921c5c7d23cca6e44fd19d662b45cc9c02135a6c96c42`
  (API Adoptium; **corrisponde**)
- `OpenJDK21U-jdk_x64_windows_hotspot_21.0.13_9.zip` — SHA-256 `a4dbaa05fca4bd89d84584e58c459bd732efebb1722af80ab13c6516832004f9`
  (API Adoptium; **corrisponde**)

IDA Pro: **non installata** (nessun `ida.exe`/`idat64.exe` in `C:\Program Files`, `C:\Program Files (x86)`,
`C:\Users\utente`, né su PATH). Non è stata scaricata: richiede licenza/account.

---

## 2. Configurazione applicata

Variabili d'ambiente **a livello utente** (`HKCU\Environment`):

| Variabile | Valore | Motivo |
|---|---|---|
| `GHIDRA_INSTALL_DIR` | `C:\Users\utente\Tools\ghidra` | richiesta da REA e dagli script Ghidra |
| `GHIDRA_HEADLESS_MAXMEM` | `8G` | default headless = 2G → `OutOfMemoryError` su PPKLite |
| `GHIDRA_MAXMEM` | `8G` | coerenza per la GUI |
| `JAVA_HOME` | `C:\Users\utente\Tools\jdk25` | JDK 25 per Ghidra/REA |
| `Path` (utente) | prefisso `C:\Users\utente\Tools\jdk25\bin` | REA sonda `java.exe` dal PATH e lo confronta con `java.min=25` |

Backup del PATH utente precedente: `C:\Users\utente\AppData\Local\Temp\opencode\user-path-backup.txt`.

Inoltre `C:\Users\utente\Tools\ghidra\support\launch.properties` è stato modificato in una riga:

```
JAVA_HOME_OVERRIDE=C:/Users/utente/Tools/jdk25
```

Sintassi con slash in avanti: il file è un `.properties` Java e i due punti/backslash
sfuggiti (`C\:\\...`) vengono letti letteralmente e rifiutati dal launcher.

Nessuna installazione ha richiesto privilegi di amministratore; nessuna modifica al PATH
di sistema né al PATH della macchina.

---

## 3. Comandi usati

```powershell
# JDK (ZIP ufficiale Adoptium, estratto a mano: nessun installer, nessun admin)
curl.exe -L -o temurin25.zip "https://github.com/adoptium/temurin25-binaries/releases/download/jdk-25.0.5%2B8/OpenJDK25U-jdk_x64_windows_hotspot_25.0.5_8.zip"
tar -xf temurin25.zip -C C:\Users\utente\AppData\Local\Temp\opencode
Move-Item ...\jdk-25.0.5+8 C:\Users\utente\Tools\jdk25

# Ghidra
curl.exe -L -o ghidra_12.2_PUBLIC_20261016.zip "https://github.com/NationalSecurityAgency/ghidra/releases/download/Ghidra_12.2_build/ghidra_12.2_PUBLIC_20261016.zip"
tar -xf ghidra_12.2_PUBLIC_20261016.zip
Move-Item ghidra_12.2_PUBLIC C:\Users\utente\Tools\ghidra

# variabili
setx GHIDRA_INSTALL_DIR "C:\Users\utente\Tools\ghidra"
setx GHIDRA_HEADLESS_MAXMEM "8G"
```

Analisi headless (schema generale; `-process` riusa il programma già importato nel progetto,
`-noanalysis` salta la ri-analisi):

```
support\analyzeHeadless.bat <projectDir> <projectName> -import <binario> -overwrite ^
    -analysisTimeoutPerFile 10800 ^
    -scriptPath "C:\Users\utente\Desktop\open pdf\docs\ghidra\scripts" ^
    -postScript DecompileByPattern.java <outDir> <pattern...> --max <n>
```

Comandi esatti eseguiti:

```powershell
$env:GHIDRA_INSTALL_DIR="C:\Users\utente\Tools\ghidra"; $env:GHIDRA_HEADLESS_MAXMEM="8G"
$proj="C:\Users\utente\AppData\Local\Temp\opencode\ghidra-proj"

# DigSig.api
& "C:\Users\utente\Tools\ghidra\support\analyzeHeadless.bat" $proj rea-acrobat `
  -import "C:\Program Files\Adobe\Acrobat DC\Acrobat\plug_ins\DigSig.api" -overwrite `
  -scriptPath "C:\Users\utente\Desktop\open pdf\docs\ghidra\scripts" `
  -postScript DecompileByPattern.java `
  "C:\Users\utente\Desktop\open pdf\docs\ghidra\DigSig.api" `
  ETSI.CAdES signingCertificateV2 ByteRange Adobe.PPKLite ETSI.RFC3161 signingTime SignerInfo

# PPKLite.api  (stessi pattern + adbe.pkcs7, cap 250 funzioni)
... -import "...\plug_ins\PPKLite.api" -overwrite ... --max 250

# Acrobat.dll  (heap 12G)
... -import "...\Acrobat\Acrobat.dll" -overwrite -analysisTimeoutPerFile 10800 `
  ... -postScript DecompileByPattern.java "...\docs\ghidra\Acrobat.dll" `
  PDRedactRegion PDRedact Redact Sanitize Redaction --max 400
```

Nota sull'argomento `--max`: il parser di `analyzeHeadless` spezza gli argomenti di script
sulle virgole **e sui caratteri `=`**. Per questo i pattern vanno passati come argomenti
separati (non `"a,b,c"`) e il flag va scritto `--max <n>` / `--max:<n>`, mai `--max=<n>`.

---

## 4. Script di decompilazione

`docs\ghidra\scripts\DecompileByPattern.java` (GhidraScript Java, compilato al volo da Ghidra
con il JDK 25 — non serve Jython/PyGhidra).

Cosa fa:

1. **Nome funzione**: raccoglie le funzioni il cui simbolo contiene un pattern.
2. **Stringhe definite**: scorre tutta la dati definita (`StringDataInstance`), caso-insensitive.
3. **Stringhe non definite**: scansione byte a byte della memoria per ASCII e UTF-16LE
   (Adobe usa largamente `wchar_t`), con backtrack all'inizio della C-string.
4. **Risoluzione dei riferimenti**: xref diretti, poi *pointer slots* a 3 livelli
   (tabelle di puntatori a puntatori, tipiche delle string table Adobe), poi scansione di
   **tutti gli operandi immediati** delle istruzioni (cattura `lea rcx,[rip+..]`,
   `mov rax, imm64` e i casi in cui l'analizzatore non ha creato xref).
5. **Decompilazione**: `DecompInterface` (`toggleCCode`, `toggleSyntaxTree`), timeout 180 s
   per funzione, troncamento di sicurezza a 400.000 caratteri.
6. **Output**: un file `.c` per funzione, nome `<indirizzo>_<nomefunzione>.c`, con header
   (programma, funzione, indirizzo, prototipo, dimensione, stringhe corrispondenti),
   più `_index.tsv` (indice) e `_summary.txt` (versioni, SHA-256 del binario, elenco stringhe).

Uso:

```
-postScript DecompileByPattern.java <outDir> <pattern1> [<pattern2> ...] [--max <n>]
```

---

## 5. Risultati

### DigSig.api — 1,6 MB, PE x64, image base `0x180000000`, SHA-256 `4184d8ee...af371`

8 funzioni decompilate (`docs\ghidra\DigSig.api\`). Stringhe rilevate: 10, fra cui
`ByteRange`, `ByteRangeNotMultipleOf2`, `ByteRangeInvalidTypeOrValue`, `byteRange`,
`ETSI.RFC3161`, `Adobe.PPKLite`, `ETSI.CAdES.detached`,
`Signature's byterange digest is not valid, modifications can't be computed`.

Funzioni più interessanti:

| Indirizzo | Cosa fa (dal decompilato) |
|---|---|
| `0x1800452FC` | validazione del `ByteRange` (errori `ByteRangeNotMultipleOf2`, `ByteRangeInvalidTypeOrValue`, `ByteRangeNotCosArray`) |
| `0x180069C20` | gestione dell'errore "Signature's byterange digest is not valid" |
| `0x18006F9A4`, `0x18006FCE0` | accesso/modifica del dizionario `byteRange` |
| `0x180092CC0` | handler `ETSI.RFC3161` (timestamp) |
| `0x1800A5EC0`, `0x1800DA550`, `0x1800DADB0` | handler `Adobe.PPKLite`, incluso il percorso `security.getHandler( "Adobe.PPKLite", false )` |

`ETSI.CAdES.detached` è stato localizzato a `0x180120128` ma **non** attribuito a una funzione:
la stringa è raggiunta solo tramite una tabella di descrittori costruita a runtime, senza
riferimenti statici (vedi limiti).

### PPKLite.api — 13,2 MB, PE x64, image base `0x28000000`, SHA-256 `e260eac1...ab99e`

7 funzioni decompilate (`docs\ghidra\PPKLite.api\`). 34 stringhe rilevate, fra cui:
`adbe.pkcs7.sha1`, `adbe.pkcs7.detached`, `ETSI.CAdES.detached`, `Adobe.PPKLite`,
`Adobe.PPKLite.DCS`, `ByteRange`, `ByteRangeRef`, `signingTime`,
`id-smime-aa-signingCertificateV2`, `SigDict /ByteRange value`, `PubSec:UseTSAsSigningTime`,
`dss:ReturnSigningTime`, `dss:SigningTime`, più i nomi RTTI delle classi CMS
(`CMSSignerInfo@ASCrypt`, `CSigningTimeAttr@ASCrypt`, `_dss__SigningTime`).

### Acrobat.dll — 87,7 MB, PE x64

Vedi sezione aggiornata in fondo (`docs\ghidra\Acrobat.dll\`), pattern:
`PDRedactRegion`, `PDRedact`, `Redact`, `Sanitize`, `Redaction`.

---

## 6. Limiti noti

- **Stringhe senza riferimento statico**: quando una stringa è usata solo tramite una tabella
  di descrittori costruita a runtime o indice calcolato, non esiste xref e la funzione non può
  essere attribuita automaticamente. Restano elencate in `_summary.txt` con l'indirizzo, per
  il cross-reference manuale (in Ghidra GUI: `Search > For Address`).
- **RTTI**: i nomi classe MSVC (`.?AV...`) sono riferiti solo da dati (type descriptor / vtable),
  non da codice; non producono funzioni decompilate.
- **Copertura non esaustiva**: lo script seleziona per pattern, non decompila l'intero binario.
  Per Acrobat.dll (88 MB) una decompilazione totale richiederebbe ore.
- **Decompilato Ghidra**, non codice sorgente: nomi `FUN_*`, tipi `undefined*`, e le chiamate
  indirette (`(*pcVar1)()`) restano opache.
- `_guard_dispatch_icall` / `__security_check_cookie` compaiono come "injection" nei commenti:
  è il comportamento normale di Ghidra sui binari MSVC con CFG.

---

## 7. Uso dentro REA (importante)

Il provider nativo Ghidra di REA legge `GHIDRA_INSTALL_DIR` **all'avvio del server MCP**.

> **Serve riavviare OpenCode** (chiudere e riaprire la sessione/app) perché REA veda
> l'installazione. Prima del riavvio il provider resta `unavailable / not_configured`,
> anche se Ghidra funziona da riga di comando.

Requisiti verificati dai diagnostici di REA (`binary_session` → `analysis_provider_candidates`):

| Check | Stato dopo l'installazione |
|---|---|
| `configuration` (`GHIDRA_INSTALL_DIR`) | impostata a livello utente |
| `platform` / `architecture` | `win32` / `x64` |
| `installation` (`application.properties`) | presente (Ghidra 12.2 ≥ 12.1 richiesto) |
| `headless` (`support/analyzeHeadless.bat`) | presente e funzionante |
| `native_decompiler` | `Ghidra\Features\Decompiler\os\win_x86_64\decompile.exe` |
| `java` | `java.exe` → Temurin 25.0.5 (deve soddisfare `java.min=25`) |

Dopo il riavvio, la verifica è:

```jsonc
// tools.rea.binary_session({ detail: "summary" })
"analysis_provider_candidates": [ { "provider": { "id": "ghidra" },
  "availability": { "status": "available" } } ]
```

Opzionale (consigliato da REA per non perdere gli edit): impostare
`REA_GHIDRA_CHECKPOINT_DIR` a una directory privata assoluta **prima** di avviare REA.

---

## 8. Ripristino / disinstallazione

```powershell
# variabili
[Environment]::SetEnvironmentVariable("GHIDRA_INSTALL_DIR", $null, "User")
[Environment]::SetEnvironmentVariable("GHIDRA_HEADLESS_MAXMEM", $null, "User")
[Environment]::SetEnvironmentVariable("GHIDRA_MAXMEM", $null, "User")
[Environment]::SetEnvironmentVariable("JAVA_HOME", $null, "User")

# PATH utente: ripristinare il valore salvato
[Environment]::SetEnvironmentVariable("Path",
  (Get-Content "C:\Users\utente\AppData\Local\Temp\opencode\user-path-backup.txt" -Raw).Trim(), "User")

# file
Remove-Item "C:\Users\utente\Tools\ghidra" -Recurse -Force
Remove-Item "C:\Users\utente\Tools\jdk25" -Recurse -Force
Remove-Item "C:\Users\utente\Tools\jdk21" -Recurse -Force   # opzionale, inutilizzato
Remove-Item "C:\Users\utente\AppData\Local\Temp\opencode\ghidra-proj" -Recurse -Force
```

Nessun servizio, nessuna voce di registro oltre a `HKCU\Environment`, nessun file di sistema
modificato. Java 8 di sistema è rimasto intatto: è solo stato scavalcato nel PATH utente.

---

## 9. Spazio su disco

| | Prima | Dopo |
|---|---|---|
| C: libero | 9,38 GB | ~7,4 GB |
| D: libero | 136,92 GB | 136,92 GB (non usato) |

Ghidra estratto ~0,85 GB, JDK 25 ~0,35 GB, JDK 21 ~0,35 GB, ZIP temporanei ~0,78 GB
(eliminabili: `C:\Users\utente\AppData\Local\Temp\opencode\*.zip`).
