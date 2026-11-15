use crate::engine;
use crate::types::ConvertResult;
use pdfium_render::prelude::*;
use std::io::Write;
use std::path::{Path, PathBuf};

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn strip_tags(xml: &str) -> String {
    let mut out = String::with_capacity(xml.len());
    let mut in_tag = false;
    for ch in xml.chars() {
        match ch {
            '<' => {
                in_tag = true;
            }
            '>' => {
                in_tag = false;
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn pdf_pages_text(path: &Path, password: Option<&str>) -> Result<Vec<String>, String> {
    let doc = engine::open_doc(path, password)?;
    let count = doc.pages().len();
    let mut pages = Vec::with_capacity(count as usize);
    for i in 0..count {
        pages.push(engine::page_text_from_doc(&doc, i as usize)?);
    }
    Ok(pages)
}

pub fn text_to_pdf(dest: &Path, title: &str, content: &str) -> Result<(), String> {
    let pdfium = engine::bind()?;
    let mut doc = pdfium.create_new_pdf().map_err(|e| e.to_string())?;
    let font = doc.fonts_mut().new_built_in(PdfFontBuiltin::Helvetica);
    let bold = doc.fonts_mut().new_built_in(PdfFontBuiltin::HelveticaBold);
    let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    let per_page = 52usize;
    let chunks: Vec<&[String]> = if lines.is_empty() {
        vec![&[][..]]
    } else {
        lines.chunks(per_page).collect()
    };
    for (page_no, chunk) in chunks.iter().enumerate() {
        let mut page = doc
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .map_err(|e| e.to_string())?;
        if page_no == 0 && !title.is_empty() {
            let mut header = PdfPageTextObject::new(&doc, title, bold, PdfPoints::new(20.0))
                .map_err(|e| e.to_string())?;
            header
                .translate(PdfPoints::new(56.0), PdfPoints::new(780.0))
                .map_err(|e| e.to_string())?;
            page.objects_mut().add_text_object(header).map_err(|e| e.to_string())?;
        }
        let mut y = if page_no == 0 && !title.is_empty() { 748.0 } else { 790.0 };
        for line in chunk.iter() {
            if line.trim().is_empty() {
                y -= 11.0;
                continue;
            }
            for piece in wrap_plain(line, 92) {
                if y < 42.0 {
                    break;
                }
                let mut text = PdfPageTextObject::new(&doc, piece, font, PdfPoints::new(10.5))
                    .map_err(|e| e.to_string())?;
                text.translate(PdfPoints::new(56.0), PdfPoints::new(y))
                    .map_err(|e| e.to_string())?;
                page.objects_mut().add_text_object(text).map_err(|e| e.to_string())?;
                y -= 14.0;
            }
        }
        page.regenerate_content().map_err(|e| e.to_string())?;
    }
    doc.save_to_file(dest).map_err(|e| e.to_string())?;
    Ok(())
}

fn wrap_plain(line: &str, max_chars: usize) -> Vec<String> {
    let mut out = vec![];
    let mut current = String::new();
    for word in line.split_whitespace() {
        if current.is_empty() {
            current = word.to_string();
        } else if current.chars().count() + word.chars().count() + 1 <= max_chars {
            current.push(' ');
            current.push_str(word);
        } else {
            out.push(current);
            current = word.to_string();
        }
    }
    if !current.is_empty() {
        out.push(current);
    } else {
        out.push(String::new());
    }
    out
}

pub fn web_to_pdf(url: &str, dest: &Path) -> Result<ConvertResult, String> {
    let edge = engine::edge_path()
        .ok_or("Microsoft Edge non trovato: impossibile convertire pagine web in PDF.")?;
    let url = if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        format!("https://{url}")
    };
    let status = std::process::Command::new(edge)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--no-pdf-header-footer",
            &format!("--print-to-pdf={}", dest.display()),
            &url,
        ])
        .status()
        .map_err(|e| format!("Edge non avviato: {e}"))?;
    if !status.success() || !dest.exists() {
        return Err("Conversione web non riuscita. Verifica la connessione e l'URL.".into());
    }
    let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    Ok(ConvertResult {
        path: dest.to_string_lossy().to_string(),
        message: "Pagina web convertita in PDF con Microsoft Edge.".into(),
        size,
    })
}

/// Motore Office in grado di aprire una determinata estensione.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OfficeKind {
    Word,
    Excel,
    PowerPoint,
}

impl OfficeKind {
    fn key(self) -> &'static str {
        match self {
            OfficeKind::Word => "word",
            OfficeKind::Excel => "excel",
            OfficeKind::PowerPoint => "powerpoint",
        }
    }

    fn prog_id(self) -> &'static str {
        match self {
            OfficeKind::Word => "Word.Application",
            OfficeKind::Excel => "Excel.Application",
            OfficeKind::PowerPoint => "PowerPoint.Application",
        }
    }

    fn label(self) -> &'static str {
        match self {
            OfficeKind::Word => "Microsoft Word",
            OfficeKind::Excel => "Microsoft Excel",
            OfficeKind::PowerPoint => "Microsoft PowerPoint",
        }
    }
}

/// Mappatura estensione -> motore. `None` significa "non Office": si passa a
/// LibreOffice e, in ultima istanza, all'estrazione del solo testo.
fn office_kind_for(path: &Path) -> Option<OfficeKind> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "doc" | "docx" | "docm" | "dot" | "dotx" | "rtf" | "odt" => Some(OfficeKind::Word),
        // CSV resta associato a Excel come nella versione precedente.
        "xls" | "xlsx" | "xlsm" | "xlsb" | "csv" | "ods" => Some(OfficeKind::Excel),
        "ppt" | "pptx" | "pptm" | "odp" => Some(OfficeKind::PowerPoint),
        _ => None,
    }
}

fn com_app_registered(prog_id: &str) -> bool {
    std::process::Command::new("reg.exe")
        .args(["query", &format!("HKCR\\{prog_id}")])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

const OFFICE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
const LIBREOFFICE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
/// Soglia oltre la quale un PDF è considerato realmente prodotto.
const PDF_MIN_BYTES: u64 = 1024;

/// Codici di uscita concordati con lo script PowerShell.
const EXIT_PROTECTED_VIEW: i32 = 10;
const EXIT_OFFICE_FAILED: i32 = 11;
const EXIT_PDF_MISSING: i32 = 12;
const EXIT_PDF_EMPTY: i32 = 13;

static TEMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Script PowerShell con i parametri espliciti di esportazione. Il testo è
/// interamente ASCII per non dipendere dalla code page di PowerShell 5.1.
const OFFICE_SCRIPT: &str = r##"
param([string]$src, [string]$dst, [string]$kind)
$ErrorActionPreference = 'Stop'
$src = [IO.Path]::GetFullPath($src)
$dst = [IO.Path]::GetFullPath($dst)

function Release-Com($obj) {
  if ($null -ne $obj) {
    try { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($obj) } catch { }
  }
}

function Stop-ComApp($app) {
  if ($null -ne $app) { try { $app.Quit() } catch { } }
  Release-Com $app
  [GC]::Collect()
  [GC]::WaitForPendingFinalizers()
}

function Assert-Pdf([int]$min) {
  if (-not (Test-Path -LiteralPath $dst)) {
    [Console]::Error.WriteLine('PDF non creato')
    exit 12
  }
  $len = (Get-Item -LiteralPath $dst).Length
  if ($len -le $min) {
    [Console]::Error.WriteLine("PDF vuoto o incompleto: $len byte")
    exit 13
  }
}

try {
  switch ($kind) {
    'word' {
      $app = $null
      $doc = $null
      try {
        $app = New-Object -ComObject Word.Application
        $app.Visible = $false
        $app.DisplayAlerts = 0
        try { $app.AutomationSecurity = 3 } catch { }
        try {
          # ConfirmConversions=false, ReadOnly=true, AddToRecentFiles=false, Visible=false
          $doc = $app.Documents.Open($src, $false, $true, $false, '', '', $false, '', '', 0, 65001, $false, $false, 0, $false)
        } catch {
          # Il file puo essere stato aperto in Protected View: chiudi le finestre
          # residue e riprova una volta senza dialoghi prima di dichiarare l'errore.
          $app.DisplayAlerts = 0
          try { foreach ($w in @($app.ProtectedViewWindows)) { $w.Close() } } catch { }
          try {
            $doc = $app.Documents.Open($src, $false, $true, $false, '', '', $false, '', '', 0, 65001, $false, $false, 0, $false)
          } catch {
            [Console]::Error.WriteLine('PROTECTED_VIEW: ' + $_.Exception.Message)
            exit 10
          }
        }
        # ExportFormat=PDF(17), OpenAfterExport=false, OptimizeFor=1, Range=0,
        # Item=0, IncludeDocProps=true, KeepIRM=true, CreateBookmarks=1,
        # DocStructureTags=true, BitmapMissingFonts=true
        $doc.ExportAsFixedFormat($dst, 17, $false, 1, 0, 0, 0, 0, $true, $true, 1, $true, $true, $false)
      } catch {
        $msg = [string]$_.Exception.Message
        if ($msg -match 'Protected View|protected view|0x800A1527|protezione') {
          [Console]::Error.WriteLine('PROTECTED_VIEW: ' + $msg)
          exit 10
        }
        [Console]::Error.WriteLine('WORD_FAIL: ' + $msg)
        exit 11
      } finally {
        if ($null -ne $doc) { try { $doc.Close(0) } catch { } }
        Stop-ComApp $app
      }
    }
    'excel' {
      $app = $null
      $wb = $null
      $empty = @()
      try {
        $app = New-Object -ComObject Excel.Application
        $app.Visible = $false
        $app.DisplayAlerts = $false
        $app.ScreenUpdating = $false
        try { $app.AutomationSecurity = 3 } catch { }
        $wb = $app.Workbooks.Open($src, 0, $true)
        $withContent = 0
        foreach ($ws in @($wb.Worksheets)) {
          if ($ws.Visible -ne -1) { continue }
          $has = $false
          # xlCellTypeConstants = 2, xlCellTypeFormulas = -4123
          foreach ($t in @(2, -4123)) {
            try {
              $found = $ws.UsedRange.SpecialCells($t)
              if ($null -ne $found) { $has = $true }
            } catch { }
          }
          $area = ''
          try { $area = [string]$ws.PageSetup.PrintArea } catch { }
          # Anche immagini, grafici e caselle di testo sono contenuto del foglio.
          if (-not $has) {
            try { if ($ws.Shapes.Count -gt 0) { $has = $true } } catch { }
          }
          # Se l'area di stampa e definita, IgnorePrintAreas = $false la rispetta.
          if ($has) {
            $withContent++
            try {
              $ws.PageSetup.Zoom = $false
              $ws.PageSetup.FitToPagesWide = 1
              $ws.PageSetup.FitToPagesTall = $false
            } catch { }
          } else {
            $empty += $ws
          }
        }
        if ($withContent -eq 0 -and $empty.Count -gt 0) {
          # Nessun foglio con contenuto: esporta almeno il primo foglio visibile.
          $empty = @($empty | Select-Object -Skip 1)
        }
        # I fogli vuoti vengono nascosti solo in memoria: il file non viene salvato.
        foreach ($ws in $empty) { try { $ws.Visible = 2 } catch { } }
        # Type=PDF(0), Quality=0, IncludeDocProperties=true, IgnorePrintAreas=false
        $wb.ExportAsFixedFormat(0, $dst, 0, $true, $false, 0, 0, $false)
      } catch {
        [Console]::Error.WriteLine('EXCEL_FAIL: ' + $_.Exception.Message)
        exit 11
      } finally {
        if ($null -ne $wb) {
          foreach ($ws in $empty) { try { $ws.Visible = -1 } catch { } }
          try { $wb.Close($false) } catch { }
        }
        Stop-ComApp $app
      }
    }
    'powerpoint' {
      $app = $null
      $pres = $null
      try {
        $app = New-Object -ComObject PowerPoint.Application
        try { $app.DisplayAlerts = 1 } catch { }
        $pres = $app.Presentations.Open($src, $true, $false, $false)
        $pres.SaveAs($dst, 32)
      } catch {
        [Console]::Error.WriteLine('POWERPOINT_FAIL: ' + $_.Exception.Message)
        exit 11
      } finally {
        if ($null -ne $pres) { try { $pres.Close() } catch { } }
        Stop-ComApp $app
      }
    }
    default {
      [Console]::Error.WriteLine('Motore Office non riconosciuto: ' + $kind)
      exit 11
    }
  }
} catch {
  [Console]::Error.WriteLine('ERRORE: ' + $_.Exception.Message)
  exit 11
}

Assert-Pdf 1024
exit 0
"##;

/// Errore di automazione Office, classificato per produrre un messaggio utile.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OfficeFailure {
    NotInstalled,
    ProtectedView,
    PasswordProtected,
    Timeout,
    InvalidOutput,
    Failed(String),
}

impl OfficeFailure {
    fn summary(&self, kind: OfficeKind) -> String {
        match self {
            OfficeFailure::NotInstalled => format!("{} non è installato", kind.label()),
            OfficeFailure::ProtectedView => {
                format!("{} ha bloccato il file da Protected View", kind.label())
            }
            OfficeFailure::PasswordProtected => "Il file è protetto da password".to_string(),
            OfficeFailure::Timeout => {
                format!("{} non ha risposto entro 3 minuti", kind.label())
            }
            OfficeFailure::InvalidOutput => {
                format!("{} non ha prodotto un PDF valido", kind.label())
            }
            OfficeFailure::Failed(detail) if detail.is_empty() => {
                format!("{} non ha completato la conversione", kind.label())
            }
            OfficeFailure::Failed(detail) => {
                format!("{} non ha completato la conversione ({detail})", kind.label())
            }
        }
    }

    fn fallback_message(&self, kind: OfficeKind) -> String {
        match self {
            OfficeFailure::NotInstalled => format!(
                "{} non è installato: conversione eseguita con LibreOffice.",
                kind.label()
            ),
            OfficeFailure::ProtectedView => format!(
                "{} ha bloccato il file da Protected View: PDF creato con LibreOffice. Per la massima fedeltà sblocca il file (tasto destro > Proprietà > Sblocca) e riprova.",
                kind.label()
            ),
            OfficeFailure::PasswordProtected => {
                "File protetto da password: PDF creato con LibreOffice.".to_string()
            }
            OfficeFailure::Timeout => format!(
                "{} non ha risposto in tempo: PDF creato con LibreOffice.",
                kind.label()
            ),
            OfficeFailure::InvalidOutput => format!(
                "{} non ha prodotto un PDF valido: PDF creato con LibreOffice.",
                kind.label()
            ),
            OfficeFailure::Failed(_) => format!(
                "Conversione con {} non riuscita: PDF creato con LibreOffice.",
                kind.label()
            ),
        }
    }
}

fn office_failure_from_exit(code: Option<i32>, output: &str) -> OfficeFailure {
    let detail = detail_from_output(output);
    let lower = output.to_lowercase();
    // La password ha la precedenza: Word apre il file in un dialogo bloccato
    // e lo script puo riportare lo stesso codice di Protected View.
    if lower.contains("password") || lower.contains("parola d'ordine") {
        return OfficeFailure::PasswordProtected;
    }
    match code.unwrap_or(-1) {
        EXIT_PROTECTED_VIEW => OfficeFailure::ProtectedView,
        EXIT_PDF_MISSING | EXIT_PDF_EMPTY => OfficeFailure::InvalidOutput,
        EXIT_OFFICE_FAILED | -1 => {
            if lower.contains("protected view") || lower.contains("0x800a1527") {
                OfficeFailure::ProtectedView
            } else {
                OfficeFailure::Failed(detail)
            }
        }
        _ => OfficeFailure::Failed(detail),
    }
}

fn office_to_pdf_via_office(
    path: &Path,
    dest: &Path,
    kind: OfficeKind,
) -> Result<(), OfficeFailure> {
    if !com_app_registered(kind.prog_id()) {
        return Err(OfficeFailure::NotInstalled);
    }
    let script_path = unique_temp_path("openpdf-office2pdf", "ps1");
    std::fs::write(&script_path, OFFICE_SCRIPT)
        .map_err(|e| OfficeFailure::Failed(format!("script non scrivibile: {e}")))?;
    let src = path.to_string_lossy().to_string();
    let dst = dest.to_string_lossy().to_string();
    let script = script_path.to_string_lossy().to_string();
    let mut command = std::process::Command::new("powershell.exe");
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-NoLogo",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        &script,
        "-src",
        &src,
        "-dst",
        &dst,
        "-kind",
        kind.key(),
    ]);
    let outcome = run_with_timeout(&mut command, OFFICE_TIMEOUT);
    let _ = std::fs::remove_file(&script_path);
    let (status, output) = match outcome {
        Ok(result) => result,
        Err(RunFailure::Timeout(_)) => return Err(OfficeFailure::Timeout),
        Err(RunFailure::Other(message)) => return Err(OfficeFailure::Failed(message)),
    };
    if status.success() {
        return check_pdf_output(dest, PDF_MIN_BYTES)
            .map(|_| ())
            .map_err(|_| OfficeFailure::InvalidOutput);
    }
    Err(office_failure_from_exit(status.code(), &output))
}

pub fn office_to_pdf(path: &Path, dest: &Path) -> Result<ConvertResult, String> {
    if !path.exists() {
        return Err("File di origine non trovato.".into());
    }
    // Un PDF rimasto da una conversione precedente non deve sembrare un successo.
    let _ = std::fs::remove_file(dest);
    let kind = office_kind_for(path);
    let mut notes: Vec<String> = Vec::new();

    if let Some(kind) = kind {
        match office_to_pdf_via_office(path, dest, kind) {
            Ok(()) => {
                return Ok(convert_result(
                    dest,
                    format!("Conversione fedele eseguita con {}.", kind.label()),
                ));
            }
            Err(failure) => {
                notes.push(format!("{}.", failure.summary(kind)));
                let _ = std::fs::remove_file(dest);
                // Secondo tentativo: LibreOffice anche quando Office è presente
                // ma la conversione è fallita, non solo quando manca.
                match libreoffice_to_pdf(path, dest) {
                    Ok(size) => {
                        return Ok(ConvertResult {
                            path: dest.to_string_lossy().to_string(),
                            message: failure.fallback_message(kind),
                            size,
                        });
                    }
                    Err(e) => notes.push(e),
                }
            }
        }
    } else {
        // Estensioni non Office: comportamento invariato (LibreOffice, poi testo).
        match libreoffice_to_pdf(path, dest) {
            Ok(size) => {
                return Ok(ConvertResult {
                    path: dest.to_string_lossy().to_string(),
                    message: "Conversione eseguita con LibreOffice.".into(),
                    size,
                });
            }
            Err(e) => notes.push(e),
        }
    }

    let _ = std::fs::remove_file(dest);
    match extract_office_text(path) {
        Ok(text) => {
            let title = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            text_to_pdf(dest, &title, &text)?;
            let message = if notes.is_empty() {
                "Creato un PDF con il solo testo estratto dal file.".to_string()
            } else {
                format!(
                    "{} Creato un PDF con il solo testo estratto dal file.",
                    notes.join(" ")
                )
            };
            Ok(convert_result(dest, message))
        }
        Err(text_error) => Err(final_error(kind, &notes, &text_error)),
    }
}

fn convert_result(dest: &Path, message: String) -> ConvertResult {
    ConvertResult {
        path: dest.to_string_lossy().to_string(),
        message,
        size: file_size(dest),
    }
}

fn final_error(kind: Option<OfficeKind>, notes: &[String], text_error: &str) -> String {
    let mut message = String::from("Conversione non riuscita.");
    if let Some(kind) = kind {
        message.push_str(&format!(" Motore preferito: {}.", kind.label()));
    }
    if !notes.is_empty() {
        message.push(' ');
        message.push_str(&notes.join(" "));
    }
    message.push_str(&format!(
        " Estrazione del testo non riuscita: {}.",
        text_error.trim_end_matches('.')
    ));
    message.push_str(" Se il file è protetto da password, rimuovi la protezione o salva una copia senza password; se è bloccato da Protected View, usa tasto destro sul file > Proprietà > Sblocca.");
    message
}

/// Percorso temporaneo univoco (nessuna directory viene creata).
fn unique_temp_path(prefix: &str, extension: &str) -> PathBuf {
    let seq = TEMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{nanos}-{seq}.{extension}",
        std::process::id()
    ))
}

/// Converte un percorso Windows in URI `file:///`, codificando spazi e
/// caratteri non ASCII: LibreOffice interpreta `-env:UserInstallation` come URI.
fn path_to_file_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let raw = raw.trim_start_matches('/');
    let mut encoded = String::with_capacity(raw.len() + 8);
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/'
            | b':' => encoded.push(byte as char),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    format!("file:///{encoded}")
}

/// Argomenti LibreOffice: profilo isolato e nessun ripristino di sessione.
fn libreoffice_args(profile_uri: &str, out_dir: &Path, src: &Path) -> Vec<String> {
    vec![
        "--headless".to_string(),
        "--norestore".to_string(),
        "--nolockcheck".to_string(),
        "--nodefault".to_string(),
        "--nofirststartwizard".to_string(),
        format!("-env:UserInstallation={profile_uri}"),
        "--convert-to".to_string(),
        "pdf".to_string(),
        "--outdir".to_string(),
        out_dir.to_string_lossy().to_string(),
        src.to_string_lossy().to_string(),
    ]
}

fn libreoffice_to_pdf(path: &Path, dest: &Path) -> Result<u64, String> {
    let soffice =
        engine::office_path().ok_or_else(|| "LibreOffice non installato.".to_string())?;
    let work_dir = unique_temp_path("openpdf-lo", "dir");
    let profile_dir = work_dir.join("profile");
    let out_dir = work_dir.join("out");

    let result = (|| -> Result<u64, String> {
        std::fs::create_dir_all(&profile_dir)
            .map_err(|e| format!("Cartella temporanea LibreOffice non creata: {e}"))?;
        std::fs::create_dir_all(&out_dir)
            .map_err(|e| format!("Cartella temporanea LibreOffice non creata: {e}"))?;
        let profile_uri = path_to_file_uri(&profile_dir);
        let args = libreoffice_args(&profile_uri, &out_dir, path);
        let mut command = std::process::Command::new(&soffice);
        command.args(&args);
        let (status, output) =
            run_with_timeout(&mut command, LIBREOFFICE_TIMEOUT).map_err(|e| e.message().to_string())?;
        let produced = match find_produced_pdf(&out_dir, path) {
            Some(produced) => produced,
            None => {
                let detail = detail_from_output(&output);
                let stato = status
                    .code()
                    .map(|c| format!(" (codice {c})"))
                    .unwrap_or_default();
                return Err(if detail.is_empty() {
                    format!("LibreOffice non ha prodotto il PDF{stato}.")
                } else {
                    format!("LibreOffice non ha prodotto il PDF{stato}: {detail}")
                });
            }
        };
        let size = check_pdf_output(&produced, PDF_MIN_BYTES)
            .map_err(|e| format!("LibreOffice: {e}"))?;
        if produced != dest {
            std::fs::copy(&produced, dest)
                .map_err(|e| format!("PDF di LibreOffice non copiato: {e}"))?;
        }
        Ok(size)
    })();

    let _ = std::fs::remove_dir_all(&work_dir);
    result
}

fn find_produced_pdf(out_dir: &Path, source: &Path) -> Option<PathBuf> {
    if let Some(stem) = source.file_stem() {
        let candidate = out_dir.join(format!("{}.pdf", stem.to_string_lossy()));
        if candidate.exists() {
            return Some(candidate);
        }
    }
    std::fs::read_dir(out_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|p| {
            p.extension()
                .map(|e| e.to_string_lossy().eq_ignore_ascii_case("pdf"))
                .unwrap_or(false)
        })
}

/// Verifica che il PDF esista e superi la soglia minima: un file assente o
/// vuoto non deve mai essere dichiarato come conversione riuscita.
fn check_pdf_output(path: &Path, min_bytes: u64) -> Result<u64, String> {
    let meta =
        std::fs::metadata(path).map_err(|_| "Il PDF non è stato creato.".to_string())?;
    if meta.len() <= min_bytes {
        return Err(format!(
            "Il PDF prodotto è vuoto o incompleto ({} byte).",
            meta.len()
        ));
    }
    Ok(meta.len())
}

fn detail_from_output(output: &str) -> String {
    let line = output
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .last()
        .unwrap_or("");
    let mut text = line;
    for prefix in [
        "PROTECTED_VIEW:",
        "WORD_FAIL:",
        "EXCEL_FAIL:",
        "POWERPOINT_FAIL:",
        "ERRORE:",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim();
            break;
        }
    }
    truncate_chars(text, 240)
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push_str("...");
    out
}

/// Esito dell'avvio di un processo esterno: distingue il timeout dagli altri
/// errori per non etichettare come "Office bloccato" un avvio fallito.
enum RunFailure {
    Timeout(String),
    Other(String),
}

impl RunFailure {
    fn message(&self) -> &str {
        match self {
            RunFailure::Timeout(message) | RunFailure::Other(message) => message,
        }
    }
}

/// Avvia il processo con output su file temporaneo, attende al massimo `limit`
/// e in caso di timeout termina l'albero di processi prima di restituire
/// l'errore. Il file evita che un processo figlio rimasto attivo tenga aperti
/// i pipe e blocchi l'attesa oltre il timeout.
fn run_with_timeout(
    command: &mut std::process::Command,
    limit: std::time::Duration,
) -> Result<(std::process::ExitStatus, String), RunFailure> {
    use std::process::Stdio;

    let log_path = unique_temp_path("openpdf-run", "log");
    let log_out = std::fs::File::create(&log_path)
        .map_err(|e| RunFailure::Other(format!("File di log non creato: {e}")))?;
    let log_err = match log_out.try_clone() {
        Ok(handle) => handle,
        Err(e) => {
            let _ = std::fs::remove_file(&log_path);
            return Err(RunFailure::Other(format!("File di log non duplicato: {e}")));
        }
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_out))
        .stderr(Stdio::from(log_err));
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = std::fs::remove_file(&log_path);
            return Err(RunFailure::Other(format!("Processo non avviato: {e}")));
        }
    };

    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() > limit {
                    terminate_child(&mut child);
                    let detail = read_log(&log_path);
                    let _ = std::fs::remove_file(&log_path);
                    return Err(RunFailure::Timeout(format!(
                        "Conversione interrotta dopo {} secondi.{}",
                        limit.as_secs(),
                        if detail.is_empty() {
                            String::new()
                        } else {
                            format!(" {detail}")
                        }
                    )));
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            Err(e) => {
                terminate_child(&mut child);
                let _ = std::fs::remove_file(&log_path);
                return Err(RunFailure::Other(format!("Processo interrotto: {e}")));
            }
        }
    };
    let output = read_log(&log_path);
    let _ = std::fs::remove_file(&log_path);
    Ok((status, output))
}

fn read_log(path: &Path) -> String {
    std::fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
        .unwrap_or_default()
}

fn terminate_child(child: &mut std::process::Child) {
    use std::process::Stdio;
    let pid = child.id().to_string();
    let taskkill = |extra: &[&str]| {
        let mut args = vec!["/PID", pid.as_str(), "/T"];
        args.extend_from_slice(extra);
        let _ = std::process::Command::new("taskkill.exe")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    };
    // Prima chiusura gentile dell'albero, poi terminazione forzata.
    taskkill(&[]);
    std::thread::sleep(std::time::Duration::from_millis(800));
    if !matches!(child.try_wait(), Ok(Some(_))) {
        taskkill(&["/F"]);
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn extract_office_text(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("File Office non valido: {e}"))?;
    let mut out = String::new();
    let mut names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .collect();
    names.sort();
    for name in names {
        let is_content = (name == "word/document.xml")
            || name.starts_with("xl/sharedStrings")
            || name.starts_with("xl/worksheets/sheet")
            || (name.starts_with("ppt/slides/slide") && name.ends_with(".xml"));
        if !is_content {
            continue;
        }
        if let Ok(mut f) = zip.by_name(&name) {
            let mut xml = String::new();
            use std::io::Read;
            if f.read_to_string(&mut xml).is_ok() {
                let text = strip_tags(&xml);
                if !text.trim().is_empty() {
                    out.push_str(&text);
                    out.push('\n');
                }
            }
        }
    }
    if out.trim().is_empty() {
        return Err("Nessun testo estraibile dal file Office.".into());
    }
    Ok(out)
}

fn write_zip<F>(dest: &Path, build: F) -> Result<(), String>
where
    F: FnOnce(&mut zip::ZipWriter<std::fs::File>) -> Result<(), String>,
{
    let file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut writer = zip::ZipWriter::new(file);
    build(&mut writer)?;
    writer.finish().map_err(|e| e.to_string())?;
    Ok(())
}

fn add_zip_file(
    writer: &mut zip::ZipWriter<std::fs::File>,
    name: &str,
    data: &str,
) -> Result<(), String> {
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer.start_file(name, options).map_err(|e| e.to_string())?;
    writer.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn export_docx(src: &Path, dest: &Path, password: Option<&str>) -> Result<ConvertResult, String> {
    let pages = pdf_pages_text(src, password)?;
    let mut body = String::new();
    for (i, page) in pages.iter().enumerate() {
        for line in page.lines() {
            body.push_str(&format!(
                "<w:p><w:r><w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
                xml_escape(line)
            ));
        }
        if i + 1 < pages.len() {
            body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>");
        }
    }
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1134\" w:right=\"1134\" w:bottom=\"1134\" w:left=\"1134\"/></w:sectPr></w:body></w:document>"
    );
    write_zip(dest, |z| {
        add_zip_file(z, "[Content_Types].xml", "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/></Types>")?;
        add_zip_file(z, "_rels/.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>")?;
        add_zip_file(z, "word/document.xml", &document)
    })?;
    let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    Ok(ConvertResult {
        path: dest.to_string_lossy().to_string(),
        message: "Documento Word creato con il testo estratto dal PDF.".into(),
        size,
    })
}

pub fn export_xlsx(src: &Path, dest: &Path, password: Option<&str>) -> Result<ConvertResult, String> {
    let pages = pdf_pages_text(src, password)?;
    let sheet_count = pages.len().max(1);
    write_zip(dest, |z| {
        let mut ct = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/>");
        for i in 1..=sheet_count {
            ct.push_str(&format!("<Override PartName=\"/xl/worksheets/sheet{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>"));
        }
        ct.push_str("</Types>");
        add_zip_file(z, "[Content_Types].xml", &ct)?;
        add_zip_file(z, "_rels/.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>")?;
        let mut wb = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><workbook xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"><sheets>");
        for i in 1..=sheet_count {
            wb.push_str(&format!("<sheet name=\"Pagina {i}\" sheetId=\"{i}\" r:id=\"rId{i}\"/>"));
        }
        wb.push_str("</sheets></workbook>");
        add_zip_file(z, "xl/workbook.xml", &wb)?;
        let mut rels = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">");
        for i in 1..=sheet_count {
            rels.push_str(&format!("<Relationship Id=\"rId{i}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet{i}.xml\"/>"));
        }
        rels.push_str(&format!("<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/>", sheet_count + 1));
        rels.push_str("</Relationships>");
        add_zip_file(z, "xl/_rels/workbook.xml.rels", &rels)?;
        add_zip_file(z, "xl/styles.xml", "<?xml version=\"1.0\" encoding=\"UTF-8\"?><styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts><fills count=\"1\"><fill><patternFill patternType=\"none\"/></fill></fills><borders count=\"1\"><border/></borders><cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs><cellXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/></cellXfs></styleSheet>")?;
        for (idx, page) in pages.iter().enumerate() {
            let mut sheet = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><worksheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><sheetData>");
            for (row_idx, line) in page.lines().enumerate() {
                sheet.push_str(&format!(
                    "<row r=\"{}\"><c r=\"A{}\" t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c></row>",
                    row_idx + 1,
                    row_idx + 1,
                    xml_escape(line)
                ));
            }
            sheet.push_str("</sheetData></worksheet>");
            add_zip_file(z, &format!("xl/worksheets/sheet{}.xml", idx + 1), &sheet)?;
        }
        Ok(())
    })?;
    let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    Ok(ConvertResult {
        path: dest.to_string_lossy().to_string(),
        message: "Cartella Excel creata con il testo del PDF (una pagina per foglio).".into(),
        size,
    })
}

fn ppt_slide_xml(page_text: &str, page_no: usize) -> String {
    let mut paragraphs = String::new();
    for line in page_text.lines().take(26) {
        paragraphs.push_str(&format!(
            "<a:p><a:r><a:rPr lang=\"it-IT\" sz=\"1400\" dirty=\"0\"/><a:t>{}</a:t></a:r></a:p>",
            xml_escape(line)
        ));
    }
    if paragraphs.is_empty() {
        paragraphs.push_str("<a:p><a:r><a:rPr lang=\"it-IT\" sz=\"1400\"/><a:t> </a:t></a:r></a:p>");
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><p:sld xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Testo pagina {page_no}\"/><p:cNvSpPr txBox=\"1\"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x=\"609600\" y=\"365760\"/><a:ext cx=\"10972800\" cy=\"6096000\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:noFill/><a:ln><a:noFill/></a:ln></p:spPr><p:txBody><a:bodyPr wrap=\"square\" rtlCol=\"0\"><a:normAutofit/></a:bodyPr><a:lstStyle/>{paragraphs}</p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"
    )
}

pub fn export_pptx(src: &Path, dest: &Path, password: Option<&str>) -> Result<ConvertResult, String> {
    let pages = pdf_pages_text(src, password)?;
    let count = pages.len().max(1);
    let theme = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"OpenPDF\"><a:themeElements><a:clrScheme name=\"OpenPDF\"><a:dk1><a:sysClr val=\"windowText\" lastClr=\"000000\"/></a:dk1><a:lt1><a:sysClr val=\"window\" lastClr=\"FFFFFF\"/></a:lt1><a:dk2><a:srgbClr val=\"1F2937\"/></a:dk2><a:lt2><a:srgbClr val=\"F3F4F6\"/></a:lt2><a:accent1><a:srgbClr val=\"7C3AED\"/></a:accent1><a:accent2><a:srgbClr val=\"22D3EE\"/></a:accent2><a:accent3><a:srgbClr val=\"F472B6\"/></a:accent3><a:accent4><a:srgbClr val=\"34D399\"/></a:accent4><a:accent5><a:srgbClr val=\"FBBF24\"/></a:accent5><a:accent6><a:srgbClr val=\"60A5FA\"/></a:accent6><a:hlink><a:srgbClr val=\"2563EB\"/></a:hlink><a:folHlink><a:srgbClr val=\"7C3AED\"/></a:folHlink></a:clrScheme><a:fontScheme name=\"OpenPDF\"><a:majorFont><a:latin typeface=\"Segoe UI\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:majorFont><a:minorFont><a:latin typeface=\"Segoe UI\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:minorFont></a:fontScheme><a:fmtScheme name=\"OpenPDF\"><a:fillStyleLst><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln><a:ln><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln><a:ln><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>";
    let layout = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><p:sldLayout xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" type=\"blank\" preserve=\"1\"><p:cSld name=\"Blank\"><p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>";
    let master = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><p:sldMaster xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"><p:cSld><p:bg><p:bgRef idx=\"1001\"><a:schemeClr val=\"bg1\"/></p:bgRef></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"0\" cy=\"0\"/></a:xfrm></p:grpSpPr></p:spTree></p:cSld><p:clrMap bg1=\"lt1\" tx1=\"dk1\" bg2=\"lt2\" tx2=\"dk2\" accent1=\"accent1\" accent2=\"accent2\" accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"/><p:sldLayoutIdLst><p:sldLayoutId id=\"2147483649\" r:id=\"rId1\"/></p:sldLayoutIdLst><p:txStyles><p:titleStyle/><p:bodyStyle/><p:otherStyle/></p:txStyles></p:sldMaster>";
    write_zip(dest, |z| {
        let mut ct = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml\"/><Override PartName=\"/ppt/slideMasters/slideMaster1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml\"/><Override PartName=\"/ppt/slideLayouts/slideLayout1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml\"/><Override PartName=\"/ppt/theme/theme1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.theme+xml\"/>");
        for i in 1..=count {
            ct.push_str(&format!("<Override PartName=\"/ppt/slides/slide{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>"));
        }
        ct.push_str("</Types>");
        add_zip_file(z, "[Content_Types].xml", &ct)?;
        add_zip_file(z, "_rels/.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"ppt/presentation.xml\"/></Relationships>")?;
        add_zip_file(z, "ppt/theme/theme1.xml", theme)?;
        add_zip_file(z, "ppt/slideMasters/slideMaster1.xml", master)?;
        add_zip_file(z, "ppt/slideMasters/_rels/slideMaster1.xml.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout\" Target=\"../slideLayouts/slideLayout1.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme\" Target=\"../theme/theme1.xml\"/></Relationships>")?;
        add_zip_file(z, "ppt/slideLayouts/slideLayout1.xml", layout)?;
        add_zip_file(z, "ppt/slideLayouts/_rels/slideLayout1.xml.rels", "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster\" Target=\"../slideMasters/slideMaster1.xml\"/></Relationships>")?;
        let mut pres = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><p:presentation xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"><p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rId1\"/></p:sldMasterIdLst><p:sldIdLst>");
        for i in 1..=count {
            pres.push_str(&format!("<p:sldId id=\"{}\" r:id=\"rId{}\"/>", 255 + i, i + 1));
        }
        pres.push_str("</p:sldIdLst><p:sldSz cx=\"12192000\" cy=\"6858000\"/><p:notesSz cx=\"6858000\" cy=\"9144000\"/></p:presentation>");
        add_zip_file(z, "ppt/presentation.xml", &pres)?;
        let mut pres_rels = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster\" Target=\"slideMasters/slideMaster1.xml\"/>");
        for i in 1..=count {
            pres_rels.push_str(&format!("<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{i}.xml\"/>", i + 1));
        }
        pres_rels.push_str(&format!("<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme\" Target=\"theme/theme1.xml\"/>", count + 2));
        pres_rels.push_str("</Relationships>");
        add_zip_file(z, "ppt/_rels/presentation.xml.rels", &pres_rels)?;
        for i in 1..=count {
            let text = pages.get(i - 1).cloned().unwrap_or_default();
            add_zip_file(z, &format!("ppt/slides/slide{i}.xml"), &ppt_slide_xml(&text, i))?;
            add_zip_file(z, &format!("ppt/slides/_rels/slide{i}.xml.rels"), "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout\" Target=\"../slideLayouts/slideLayout1.xml\"/></Relationships>")?;
        }
        Ok(())
    })?;
    let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    Ok(ConvertResult {
        path: dest.to_string_lossy().to_string(),
        message: "Presentazione PowerPoint creata con il testo del PDF.".into(),
        size,
    })
}

pub fn export_text(src: &Path, dest: &Path, password: Option<&str>) -> Result<ConvertResult, String> {
    let pages = pdf_pages_text(src, password)?;
    let mut text = String::new();
    for (i, page) in pages.iter().enumerate() {
        text.push_str(&format!("\n\n===== Pagina {} =====\n\n", i + 1));
        text.push_str(page);
    }
    std::fs::write(dest, text).map_err(|e| e.to_string())?;
    let size = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    Ok(ConvertResult {
        path: dest.to_string_lossy().to_string(),
        message: "Testo esportato.".into(),
        size,
    })
}

pub fn add_watermark(src: &Path, dest: &Path, text: &str, password: Option<&str>) -> Result<(), String> {
    let mut doc = engine::open_doc(src, password)?;
    let bold = doc.fonts_mut().new_built_in(PdfFontBuiltin::HelveticaBold);
    let count = doc.pages().len();
    for i in 0..count {
        let mut page = doc.pages_mut().get(i).map_err(|e| e.to_string())?;
        let w = page.width().value;
        let h = page.height().value;
        let mut obj = PdfPageTextObject::new(&doc, text, bold, PdfPoints::new(54.0))
            .map_err(|e| e.to_string())?;
        obj.set_fill_color(PdfColor::new(150, 150, 160, 70))
            .map_err(|e| e.to_string())?;
        obj.rotate_clockwise_degrees(35.0).map_err(|e| e.to_string())?;
        obj.translate(PdfPoints::new(w * 0.12), PdfPoints::new(h * 0.42))
            .map_err(|e| e.to_string())?;
        page.objects_mut().add_text_object(obj).map_err(|e| e.to_string())?;
        page.regenerate_content().map_err(|e| e.to_string())?;
    }
    doc.save_to_file(dest).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

pub fn normalize_out_dir(dest: &Path) -> PathBuf {
    dest.parent().map(|p| p.to_path_buf()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mappatura_estensioni_sul_motore() {
        assert_eq!(office_kind_for(Path::new("documento.docx")), Some(OfficeKind::Word));
        assert_eq!(office_kind_for(Path::new("RELAZIONE.DOC")), Some(OfficeKind::Word));
        assert_eq!(office_kind_for(Path::new("nota.rtf")), Some(OfficeKind::Word));
        assert_eq!(office_kind_for(Path::new("foglio.xlsx")), Some(OfficeKind::Excel));
        assert_eq!(office_kind_for(Path::new("macro.xlsm")), Some(OfficeKind::Excel));
        // CSV resta associato a Excel come nella versione precedente.
        assert_eq!(office_kind_for(Path::new("dati.csv")), Some(OfficeKind::Excel));
        assert_eq!(office_kind_for(Path::new("slide.pptx")), Some(OfficeKind::PowerPoint));
        assert_eq!(office_kind_for(Path::new("mostra.odp")), Some(OfficeKind::PowerPoint));
        assert_eq!(office_kind_for(Path::new("letto.txt")), None);
        assert_eq!(office_kind_for(Path::new("archivio.zip")), None);
        assert_eq!(office_kind_for(Path::new("senza_estensione")), None);
    }

    #[test]
    fn script_powershell_contiene_i_parametri_attesi() {
        let script = OFFICE_SCRIPT;
        // Word: apertura esplicita e senza dialoghi.
        assert!(script.contains("Documents.Open($src, $false, $true, $false, '', '', $false, '', '', 0, 65001, $false, $false, 0, $false)"));
        assert!(script.contains("$doc.ExportAsFixedFormat($dst, 17, $false, 1, 0, 0, 0, 0, $true, $true, 1, $true, $true, $false)"));
        assert!(script.contains("$app.Visible = $false"));
        assert!(script.contains("$app.DisplayAlerts = 0"));
        // Excel: niente aree di stampa ignorate e adattamento in larghezza.
        assert!(script.contains("$wb.ExportAsFixedFormat(0, $dst, 0, $true, $false, 0, 0, $false)"));
        assert!(script.contains("$ws.PageSetup.FitToPagesWide = 1"));
        assert!(script.contains("PageSetup.PrintArea"));
        // PowerPoint.
        assert!(script.contains("Presentations.Open($src, $true, $false, $false)"));
        assert!(script.contains("$pres.SaveAs($dst, 32)"));
        // Rilascio COM e gestione Protected View.
        assert!(script.contains("$doc.Close(0)"));
        assert!(script.contains("$app.Quit()"));
        assert!(script.contains("ReleaseComObject"));
        assert!(script.contains("PROTECTED_VIEW"));
        assert!(script.contains("Assert-Pdf 1024"));
    }

    #[test]
    fn comando_libreoffice_usa_profilo_isolato() {
        let args = libreoffice_args(
            "file:///C:/Temp/openpdf-lo/profile",
            Path::new(r"C:\Temp\out"),
            Path::new(r"C:\Temp\documento.docx"),
        );
        for atteso in ["--headless", "--norestore", "--nolockcheck", "--convert-to"] {
            assert!(args.iter().any(|a| a == atteso), "argomento mancante: {atteso}");
        }
        let profilo = args
            .iter()
            .find(|a| a.starts_with("-env:UserInstallation="))
            .expect("profilo isolato mancante");
        assert_eq!(
            profilo,
            "-env:UserInstallation=file:///C:/Temp/openpdf-lo/profile"
        );
        let convert_index = args
            .iter()
            .position(|a| a == "--convert-to")
            .expect("--convert-to mancante");
        assert_eq!(args[convert_index + 1], "pdf");
        assert_eq!(args.last().map(String::as_str), Some(r"C:\Temp\documento.docx"));
    }

    #[test]
    fn percorso_profilo_in_uri_file() {
        assert_eq!(
            path_to_file_uri(Path::new(r"C:\Temp\open pdf\profile")),
            "file:///C:/Temp/open%20pdf/profile"
        );
        assert_eq!(
            path_to_file_uri(Path::new(r"C:\Temp\openpdf\profile")),
            "file:///C:/Temp/openpdf/profile"
        );
    }

    #[test]
    fn pdf_assente_o_vuoto_restituisce_errore() {
        let assente = unique_temp_path("openpdf-test-assente", "pdf");
        let _ = std::fs::remove_file(&assente);
        assert!(check_pdf_output(&assente, PDF_MIN_BYTES).is_err());

        let vuoto = unique_temp_path("openpdf-test-vuoto", "pdf");
        std::fs::write(&vuoto, b"").expect("file di prova");
        assert!(check_pdf_output(&vuoto, PDF_MIN_BYTES).is_err());

        let incompleto = unique_temp_path("openpdf-test-incompleto", "pdf");
        std::fs::write(&incompleto, vec![0u8; 512]).expect("file di prova");
        assert!(check_pdf_output(&incompleto, PDF_MIN_BYTES).is_err());

        let valido = unique_temp_path("openpdf-test-valido", "pdf");
        std::fs::write(&valido, vec![0u8; 2048]).expect("file di prova");
        assert_eq!(check_pdf_output(&valido, PDF_MIN_BYTES).unwrap(), 2048);

        let _ = std::fs::remove_file(&vuoto);
        let _ = std::fs::remove_file(&incompleto);
        let _ = std::fs::remove_file(&valido);
    }

    #[test]
    fn origine_assente_restituisce_errore_senza_com() {
        let assente = unique_temp_path("openpdf-test-origine-assente", "docx");
        let _ = std::fs::remove_file(&assente);
        let dest = unique_temp_path("openpdf-test-origine-dest", "pdf");
        let errore = office_to_pdf(&assente, &dest).expect_err("il file assente deve fallire");
        assert!(errore.contains("non trovato"));
    }

    #[test]
    fn errori_office_classificati() {
        assert_eq!(
            office_failure_from_exit(Some(EXIT_PROTECTED_VIEW), ""),
            OfficeFailure::ProtectedView
        );
        assert_eq!(
            office_failure_from_exit(Some(EXIT_OFFICE_FAILED), "The password is incorrect"),
            OfficeFailure::PasswordProtected
        );
        assert_eq!(
            office_failure_from_exit(Some(EXIT_OFFICE_FAILED), "COM error 0x800A1527"),
            OfficeFailure::ProtectedView
        );
        assert_eq!(
            office_failure_from_exit(Some(EXIT_PDF_MISSING), ""),
            OfficeFailure::InvalidOutput
        );
        assert!(matches!(
            office_failure_from_exit(Some(EXIT_OFFICE_FAILED), "errore generico"),
            OfficeFailure::Failed(_)
        ));
    }

    #[test]
    fn messaggi_distinguono_i_casi() {
        assert!(OfficeFailure::NotInstalled
            .fallback_message(OfficeKind::Word)
            .contains("non è installato"));
        assert!(OfficeFailure::ProtectedView
            .fallback_message(OfficeKind::Word)
            .contains("Protected View"));
        assert!(OfficeFailure::Failed("x".into())
            .summary(OfficeKind::Excel)
            .contains("Microsoft Excel"));
        assert!(OfficeFailure::PasswordProtected
            .summary(OfficeKind::PowerPoint)
            .contains("password"));
    }
}
