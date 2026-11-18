#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod annotate;
mod collab;
mod convert;
mod engine;
mod image_loader;
mod ocr;
mod pades;
mod pades_ocsp;
mod pades_tsa;
mod pades_verify;
mod pages;
mod pdftext;
mod redact;
mod sanitize;
mod security;
mod signature;
mod state;
mod textedit;
mod types;
mod update;

use state::{AppState, Session};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use tauri::{Manager, State};
use types::*;

/// Evita il lampo della console quando si chiamano reg.exe e curl.exe.
#[cfg(windows)]
const NESSUNA_FINESTRA: u32 = 0x0800_0000;

static STARTUP_PDF: OnceLock<Option<String>> = OnceLock::new();

fn snapshot(state: &State<'_, AppState>) -> Result<(PathBuf, Option<String>, String), String> {
    let session = state.inner.lock().unwrap();
    let path = session
        .working
        .clone()
        .ok_or("Nessun documento aperto. Apri un PDF per iniziare.")?;
    Ok((path, session.password.clone(), session.session_id.clone()))
}

fn info_with_annotations(session: &Session) -> OpenInfo {
    let annotations = state::load_annotations(&session.session_id);
    session.open_info(annotations)
}

fn apply_new_document(state: &State<'_, AppState>, path: &Path, reset: bool) -> Result<OpenInfo, String> {
    let mut session = state.inner.lock().unwrap();
    if reset {
        session.reset();
    }
    session.set_document(path, None, !reset)?;
    Ok(info_with_annotations(&session))
}

fn apply_new_working(
    state: &State<'_, AppState>,
    path: &Path,
    password: Option<String>,
) -> Result<OpenInfo, String> {
    let mut session = state.inner.lock().unwrap();
    session.set_document(path, password, true)?;
    Ok(info_with_annotations(&session))
}

#[tauri::command]
fn engine_status() -> EngineStatus {
    engine::engine_status()
}

#[tauri::command]
async fn pick_pdf() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter("Documenti PDF", &["pdf"])
        .set_title("Apri PDF")
        .pick_file()
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
async fn pick_pdfs() -> Vec<String> {
    rfd::FileDialog::new()
        .add_filter("Documenti PDF", &["pdf"])
        .set_title("Seleziona PDF")
        .pick_files()
        .map(|files| {
            files
                .into_iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tauri::command]
async fn pick_images() -> Vec<String> {
    rfd::FileDialog::new()
        .add_filter("Immagini", &["png", "jpg", "jpeg", "bmp", "gif", "webp", "tif", "tiff"])
        .set_title("Seleziona immagini")
        .pick_files()
        .map(|files| {
            files
                .into_iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[tauri::command]
async fn pick_file(filters: Vec<String>) -> Option<String> {
    let refs: Vec<&str> = filters.iter().map(|s| s.as_str()).collect();
    rfd::FileDialog::new()
        .add_filter("File", &refs)
        .pick_file()
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
async fn pick_certificate(kind: String) -> Option<String> {
    let (title, filters): (&str, &[&str]) = match kind.as_str() {
        "chiave" => ("Scegli la chiave privata (PEM)", &["pem", "key", "txt"]),
        "p12" | "pfx" => ("Scegli l'archivio PKCS#12", &["p12", "pfx"]),
        _ => ("Scegli il certificato (PEM)", &["pem", "crt", "cer", "txt"]),
    };
    rfd::FileDialog::new()
        .add_filter("Certificati e chiavi", filters)
        .set_title(title)
        .pick_file()
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
async fn sign_pdf_pades(
    state: State<'_, AppState>,
    page: usize,
    cert_path: String,
key_path: Option<String>,
password: Option<String>,
format: Option<String>,
visible: Option<bool>,
certification: Option<String>,
tsa_url: Option<String>,
    name: String,
    reason: String,
    location: String,
) -> Result<SignResult, String> {
    let (path, pdf_password, _) = snapshot(&state)?;
    let dest = state::safe_working_path("firmato-pades");

    let formato = format.unwrap_or_else(|| "pem".into());
    let formato = formato.trim().to_ascii_lowercase();
    let credenziali = match formato.as_str() {
        "pfx" | "p12" | "pkcs12" => pades::Credenziali::P12 {
            path: PathBuf::from(&cert_path),
            password: password.unwrap_or_default(),
        },
        "pem" | "" => {
            let chiave = key_path.unwrap_or_default();
            if chiave.trim().is_empty() {
                return Err(
                    "Per la firma con certificato PEM serve anche la chiave privata.".into(),
                );
            }
            pades::Credenziali::Pem {
                cert: PathBuf::from(&cert_path),
                key: PathBuf::from(chiave),
            }
        }
        altro => {
            return Err(format!(
                "Formato delle credenziali non riconosciuto: «{altro}». Valori ammessi: pem, pfx."
            ))
        }
    };

    let livello = match certification
        .as_deref()
        .unwrap_or("none")
        .trim()
        .to_ascii_uppercase()
        .as_str()
    {
        "" | "NONE" => None,
        "P1" => Some(1),
        "P2" => Some(2),
        "P3" => Some(3),
        altro => {
            return Err(format!(
                "Livello di certificazione non valido: «{altro}». Valori ammessi: none, P1, P2, P3."
            ))
        }
    };

    let visibile = visible.unwrap_or(true);
    let marca = tsa_url.filter(|url| !url.trim().is_empty());
    let ha_marca = marca.is_some();
    let opts = pades::SignOptions {
        page_index: page,
        rect: [0.06f32, 0.85, 0.34, 0.09],
        name: name.clone(),
        reason,
        location,
        pdf_password,
        certification: livello,
        visible: visibile,
        tsa_url: marca,
    };

    let dest_for_sign = dest.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        pades::sign_pdf_con(&path, &dest_for_sign, &credenziali, &opts)
    })
    .await
    .map_err(|e| format!("Firma non riuscita: {e}"))??;
    let info = apply_new_document(&state, Path::new(&outcome.path), true)?;

    let mut dettagli = Vec::new();
    if let Some(livello) = livello {
        dettagli.push(format!("certificazione DocMDP P{livello}"));
    }
    if ha_marca {
        dettagli.push("marca temporale RFC 3161".into());
    }
    if !visibile {
        dettagli.push("firma invisibile".into());
    }
    let extra = if dettagli.is_empty() {
        String::new()
    } else {
        format!(" ({})", dettagli.join(", "))
    };
    Ok(SignResult {
        info,
        message: format!(
            "Firma PAdES{extra} applicata da {} nel campo {}. File firmato: {} KB. Usa «Verifica firme» per controllare digest, CMS, catena e copertura.",
            outcome.signer,
            outcome.field,
            (outcome.size + 1023) / 1024
        ),
    })
}

#[tauri::command]
async fn verify_signatures_cmd(
    state: State<'_, AppState>,
check_revocation: Option<bool>,
) -> Result<Vec<SignatureRecord>, String> {
    let (path, password, _) = snapshot(&state)?;
    let revoca = check_revocation.unwrap_or(false);
    let found = tauri::async_runtime::spawn_blocking(move || {
        pades::verify_pdf_con(&path, password.as_deref(), revoca)
    })
    .await
    .map_err(|e| format!("Verifica delle firme non riuscita: {e}"))??;
    Ok(found
        .into_iter()
        .map(|signature| SignatureRecord {
            field: signature.field,
            signer: signature.signer,
            time: signature.time,
            subfilter: signature.subfilter,
            covers_all: signature.covers_all,
            digest_ok: signature.digest_ok,
            signature_ok: signature.signature_ok,
            chain_valid: signature.chain_valid,
            chain_complete: signature.chain_complete,
            trusted: signature.trusted,
            chain_errors: signature.chain_errors,
            attribute_errors: signature.attribute_errors,
            chain: signature.chain,
            valid_from: signature.valid_from,
            valid_to: signature.valid_to,
            docmdp: signature.docmdp,
            timestamp: signature.timestamp,
            revocation: signature.revocation,
        })
        .collect())
}

#[tauri::command]
async fn clean_signature_photo(path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || signature::clean_photo(Path::new(&path)))
        .await
        .map_err(|e| format!("Pulizia della firma non riuscita: {e}"))?
}

#[tauri::command]
async fn pick_signature_photo() -> Option<String> {
    rfd::FileDialog::new()
        .add_filter(
            "Immagini e firme",
            &[
                "jpg", "jpeg", "png", "webp", "avif", "heic", "heif", "gif", "bmp", "tif",
                "tiff", "tga", "ico", "icns", "jxl", "jp2", "jpx", "psd", "psb", "exr", "hdr",
                "dds", "dng", "proraw", "cr2", "cr3", "nef", "arw", "raf", "orf", "rw2", "pef", "srw",
                "3fr", "iiq", "svg", "ai", "eps", "pdf", "cdr", "wmf", "emf", "xcf", "kra",
                "livp", "pvt", "dcm", "dicom",
            ],
        )
        .add_filter("Tutti i file", &["*"])
        .set_title("Scegli una foto della firma")
        .pick_file()
        .map(|path| path.to_string_lossy().to_string())
}

#[tauri::command]
async fn pick_save(default_name: String, filters: Vec<String>) -> Option<String> {
    let refs: Vec<&str> = filters.iter().map(|s| s.as_str()).collect();
    let mut dialog = rfd::FileDialog::new();
    if !refs.is_empty() {
        dialog = dialog.add_filter("File", &refs);
    }
    dialog
        .set_file_name(&default_name)
        .save_file()
        .map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
async fn open_pdf(
    state: State<'_, AppState>,
    path: String,
    password: Option<String>,
) -> Result<OpenInfo, String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err("Il file selezionato non esiste.".into());
    }
    let password = password.filter(|p| !p.is_empty());
    let mut session = state.inner.lock().unwrap();
    session.reset();
    session.set_document(&p, password, false)?;
    Ok(info_with_annotations(&session))
}

#[tauri::command]
async fn close_document(state: State<'_, AppState>) -> Result<(), String> {
    let mut session = state.inner.lock().unwrap();
    session.reset();
    Ok(())
}

#[tauri::command]
async fn current_info(state: State<'_, AppState>) -> Result<Option<OpenInfo>, String> {
    let session = state.inner.lock().unwrap();
    if session.working.is_none() {
        return Ok(None);
    }
    Ok(Some(info_with_annotations(&session)))
}

#[tauri::command]
async fn render_page(
    state: State<'_, AppState>,
    page: usize,
    width: u32,
) -> Result<RenderResult, String> {
    let (path, password, _) = snapshot(&state)?;
    engine::render_page_png(&path, page, width, password.as_deref())
}

#[tauri::command]
async fn render_thumbnails(
    state: State<'_, AppState>,
    width: u32,
) -> Result<Vec<RenderResult>, String> {
    let (path, password, _) = snapshot(&state)?;
    engine::render_thumbnails(&path, width, password.as_deref())
}

#[tauri::command]
async fn get_page_text(state: State<'_, AppState>, page: usize) -> Result<String, String> {
    let (path, password, _) = snapshot(&state)?;
    let text = engine::page_text(&path, page, password.as_deref())?;
    {
        let mut session = state.inner.lock().unwrap();
        session.texts.insert(page, text.clone());
    }
    Ok(text)
}

#[tauri::command]
async fn search_pdf(
    state: State<'_, AppState>,
    query: String,
    match_case: bool,
) -> Result<Vec<SearchHit>, String> {
    if query.trim().is_empty() {
        return Ok(vec![]);
    }
    let (path, password, _) = snapshot(&state)?;
    engine::search_document(&path, &query, match_case, password.as_deref())
}

#[tauri::command]
async fn save_annotations_cmd(
    state: State<'_, AppState>,
    session_id: String,
    annotations: Vec<Annotation>,
) -> Result<(), String> {
    let _ = &state;
    state::save_annotations(&session_id, &annotations)
}

#[tauri::command]
async fn save_copy(state: State<'_, AppState>, dest: String) -> Result<ConvertResult, String> {
    let (path, _, _) = snapshot(&state)?;
    std::fs::copy(&path, &dest).map_err(|e| format!("Salvataggio non riuscito: {e}"))?;
    let size = convert::file_size(Path::new(&dest));
    Ok(ConvertResult {
        path: dest,
        message: "Copia del documento salvata.".into(),
        size,
    })
}

#[tauri::command]
async fn export_annotated(
    state: State<'_, AppState>,
    dest: String,
    annotations: Vec<Annotation>,
) -> Result<ConvertResult, String> {
    let (path, password, _) = snapshot(&state)?;
    annotate::burn_annotations(&path, Path::new(&dest), password.as_deref(), &annotations)?;
    let size = convert::file_size(Path::new(&dest));
    Ok(ConvertResult {
        path: dest,
        message: "PDF con annotazioni esportato.".into(),
        size,
    })
}

#[tauri::command]
async fn speak_page(
    state: State<'_, AppState>,
    page: usize,
    rate: i32,
) -> Result<String, String> {
    let (path, password, _) = snapshot(&state)?;
    let text = engine::page_text(&path, page, password.as_deref())?;
    let mut tts = state.tts.lock().unwrap();
    if let Some(child) = tts.take() {
        ocr::stop(Some(child));
    }
    let child = ocr::speak(&text, rate)?;
    *tts = Some(child);
    Ok(text)
}

#[tauri::command]
async fn stop_speaking(state: State<'_, AppState>) -> Result<(), String> {
    let mut tts = state.tts.lock().unwrap();
    ocr::stop(tts.take());
    Ok(())
}

#[tauri::command]
async fn fill_forms(
    state: State<'_, AppState>,
    values: Vec<FormValue>,
) -> Result<OpenInfo, String> {
    let (path, password, _) = snapshot(&state)?;
    let dest = state::safe_working_path("modulo-compilato");
    engine::fill_form_fields(&path, &dest, password.as_deref(), &values)?;
    apply_new_working(&state, &dest, None)
}

#[tauri::command]
async fn apply_page_order(
    state: State<'_, AppState>,
    order: Vec<usize>,
) -> Result<OpenInfo, String> {
    if order.is_empty() {
        return Err("Seleziona almeno una pagina.".into());
    }
    let (path, password, _) = snapshot(&state)?;
    let dest = state::safe_working_path("pagine");
    pages::copy_pages(&path, &dest, &order, password.as_deref())?;
    apply_new_working(&state, &dest, None)
}

#[tauri::command]
async fn rotate_current_page(
    state: State<'_, AppState>,
    page: usize,
    delta: i32,
) -> Result<OpenInfo, String> {
    let (path, password, _) = snapshot(&state)?;
    let dest = state::safe_working_path("rotazione");
    pages::rotate_page(&path, &dest, page, delta, password.as_deref())?;
    apply_new_working(&state, &dest, None)
}

#[tauri::command]
async fn merge_pdfs_cmd(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<OpenInfo, String> {
    if paths.is_empty() {
        return Err("Seleziona almeno un PDF da unire.".into());
    }
    let dest = state::safe_working_path("unione");
    pages::merge_pdfs(&dest, &paths)?;
    apply_new_document(&state, &dest, true)
}

#[tauri::command]
async fn split_pdf_cmd(
    state: State<'_, AppState>,
    dest: String,
    from: usize,
    to: usize,
) -> Result<ConvertResult, String> {
    let (path, password, _) = snapshot(&state)?;
    if from == 0 || to < from {
        return Err("Intervallo di pagine non valido.".into());
    }
    pages::split_pdf(&path, Path::new(&dest), from - 1, to - 1, password.as_deref())?;
    let size = convert::file_size(Path::new(&dest));
    Ok(ConvertResult {
        path: dest,
        message: format!("Estratte le pagine {from}-{to}."),
        size,
    })
}

#[tauri::command]
async fn compress_pdf(
    state: State<'_, AppState>,
    dest: String,
    mode: String,
    dpi: f32,
    quality: u8,
) -> Result<ConvertResult, String> {
    let (path, password, _) = snapshot(&state)?;
    let (before, after) = if mode == "raster" {
        pages::compress_raster(&path, Path::new(&dest), password.as_deref(), dpi, quality)?
    } else {
        pages::compress_lossless(&path, Path::new(&dest), password.as_deref())?
    };
    let saved = before.saturating_sub(after);
    Ok(ConvertResult {
        path: dest,
        message: format!(
            "Da {:.2} MB a {:.2} MB (risparmio {:.2} MB).",
            before as f64 / 1_048_576.0,
            after as f64 / 1_048_576.0,
            saved as f64 / 1_048_576.0
        ),
        size: after,
    })
}

#[tauri::command]
async fn apply_redactions(
    state: State<'_, AppState>,
    annotations: Vec<Annotation>,
) -> Result<RedactResult, String> {
    let (path, password, _) = snapshot(&state)?;
    let rects: Vec<([f32; 4], usize)> = annotations
        .iter()
        .filter(|a| a.kind == "redact")
        .map(|a| ([a.x, a.y, a.w, a.h], a.page))
        .collect();
    if rects.is_empty() {
        return Err("Disegna almeno un'area di redazione nera sul documento.".into());
    }
    let dest = state::safe_working_path("redatto");
    let message = match redact::redact_pdf(&path, &dest, password.as_deref(), &rects) {
        Ok(report) => report.summary(),
        Err(vector_error) => {
            pages::redact_pdf(&path, &dest, password.as_deref(), &rects, 200.0)?;
            format!(
                "Redazione applicata in modalità raster: la pagina è stata ricostruita come immagine, quindi testo, link e metadati sottostanti non sono più estraibili. Motivo: {vector_error}"
            )
        }
    };
    let info = apply_new_document(&state, &dest, true)?;
    Ok(RedactResult { info, message })
}

#[tauri::command]
async fn images_to_pdf_cmd(
    state: State<'_, AppState>,
    images: Vec<String>,
) -> Result<OpenInfo, String> {
    if images.is_empty() {
        return Err("Seleziona almeno un'immagine.".into());
    }
    let dest = state::safe_working_path("immagini");
    pages::images_to_pdf(&dest, &images)?;
    apply_new_document(&state, &dest, true)
}

#[tauri::command]
async fn web_to_pdf_cmd(state: State<'_, AppState>, url: String) -> Result<OpenInfo, String> {
    let dest = state::safe_working_path("web");
    convert::web_to_pdf(&url, &dest)?;
    apply_new_document(&state, &dest, true)
}

#[tauri::command]
async fn office_to_pdf_cmd(
    state: State<'_, AppState>,
    path: String,
) -> Result<OfficeResult, String> {
    let source = PathBuf::from(&path);
    if !source.exists() {
        return Err("File Office non trovato.".into());
    }
    let dest = state::safe_working_path("office");
    let result = convert::office_to_pdf(&source, &dest)?;
    let info = apply_new_document(&state, &dest, true)?;
    Ok(OfficeResult {
        info,
        message: result.message,
    })
}

#[tauri::command]
async fn export_pdf(
    state: State<'_, AppState>,
    kind: String,
    dest: String,
) -> Result<ConvertResult, String> {
    let (path, password, _) = snapshot(&state)?;
    match kind.as_str() {
        "docx" => convert::export_docx(&path, Path::new(&dest), password.as_deref()),
        "xlsx" => convert::export_xlsx(&path, Path::new(&dest), password.as_deref()),
        "pptx" => convert::export_pptx(&path, Path::new(&dest), password.as_deref()),
        _ => convert::export_text(&path, Path::new(&dest), password.as_deref()),
    }
}

#[tauri::command]
async fn ocr_page(state: State<'_, AppState>, page: usize) -> Result<String, String> {
    let (path, password, _) = snapshot(&state)?;
    ocr::ocr_pdf_page(&path, page, password.as_deref())
}

#[tauri::command]
async fn make_searchable_cmd(
    state: State<'_, AppState>,
    from: usize,
    to: usize,
) -> Result<OpenInfo, String> {
    let (path, password, _) = snapshot(&state)?;
    let dest = state::safe_working_path("ocr-ricercabile");
    ocr::make_searchable(&path, &dest, password.as_deref(), from, to)?;
    apply_new_working(&state, &dest, None)
}

#[tauri::command]
async fn ocr_file(path: String) -> Result<String, String> {
    ocr::ocr_image(Path::new(&path))
}

#[tauri::command]
async fn page_text_lines(
    state: State<'_, AppState>,
    page: usize,
) -> Result<Vec<TextLine>, String> {
    let (path, password, _) = snapshot(&state)?;
    textedit::page_lines(&path, page, password.as_deref())
}

#[tauri::command]
async fn delete_page_text(
    state: State<'_, AppState>,
    page: usize,
    rects: Vec<[f32; 4]>,
) -> Result<OpenInfo, String> {
    if rects.is_empty() {
        return Err("Seleziona almeno una riga di testo.".into());
    }
    let (path, password, _) = snapshot(&state)?;
    let dest = state::safe_working_path("testo-modificato");
    textedit::delete_lines(&path, &dest, password.as_deref(), page, &rects)?;
    apply_new_working(&state, &dest, None)
}

#[tauri::command]
async fn replace_text_in_place(
    state: State<'_, AppState>,
    page: usize,
    rect: [f32; 4],
    text: String,
) -> Result<OpenInfo, String> {
    let (path, password, _) = snapshot(&state)?;
    let dest = state::safe_working_path("testo-modificato");
    pdftext::replace_line(&path, &dest, password.as_deref(), page, rect, &text)?;
    apply_new_working(&state, &dest, None)
}

#[tauri::command]
async fn protect_pdf_cmd(
    state: State<'_, AppState>,
    dest: String,
    user_password: String,
    owner_password: String,
    allow_print: bool,
    allow_copy: bool,
    allow_modify: bool,
    allow_annotate: bool,
    allow_forms: bool,
) -> Result<ConvertResult, String> {
    let (path, password, _) = snapshot(&state)?;
    if user_password.is_empty() && owner_password.is_empty() {
        return Err("Inserisci almeno una password.".into());
    }
    security::protect_pdf(
        &path,
        Path::new(&dest),
        password.as_deref(),
        &user_password,
        &owner_password,
        allow_print,
        allow_copy,
        allow_modify,
        allow_annotate,
        allow_forms,
    )?;
    let size = convert::file_size(Path::new(&dest));
    Ok(ConvertResult {
        path: dest,
        message: "PDF protetto con crittografia AES-128.".into(),
        size,
    })
}

#[tauri::command]
async fn save_metadata(
    state: State<'_, AppState>,
    dest: String,
    title: String,
    author: String,
    subject: String,
    keywords: String,
    creator: String,
) -> Result<ConvertResult, String> {
    let (path, password, _) = snapshot(&state)?;
    security::set_metadata(
        &path,
        Path::new(&dest),
        password.as_deref(),
        &title,
        &author,
        &subject,
        &keywords,
        &creator,
    )?;
    let size = convert::file_size(Path::new(&dest));
    Ok(ConvertResult {
        path: dest,
        message: "Metadati aggiornati.".into(),
        size,
    })
}

#[tauri::command]
async fn scrub_metadata_cmd(
    state: State<'_, AppState>,
    dest: String,
) -> Result<ConvertResult, String> {
    let (path, password, _) = snapshot(&state)?;
    security::scrub_metadata(&path, Path::new(&dest), password.as_deref())?;
    let size = convert::file_size(Path::new(&dest));
    Ok(ConvertResult {
        path: dest,
        message: "Metadati e informazioni nascoste rimossi.".into(),
        size,
    })
}

#[tauri::command]
async fn sanitize_pdf_cmd(state: State<'_, AppState>) -> Result<SanitizeResult, String> {
    let (path, password, _) = snapshot(&state)?;
    let dest = state::safe_working_path("sanitizzato");
    let dest_for_clean = dest.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        sanitize::sanitize_pdf(&path, &dest_for_clean, password.as_deref())
    })
    .await
    .map_err(|e| format!("Pulizia profonda non riuscita: {e}"))??;
    let info = apply_new_document(&state, &dest, true)?;
    let message = format!(
        "Pulizia profonda completata: {} metadati, {} script, {} allegati, {} immagini e {} elementi nascosti rimossi; {} oggetti orfani eliminati.",
        report.metadata, report.scripts, report.attachments, report.images, report.hidden, report.pruned
    );
    Ok(SanitizeResult { info, message })
}

#[tauri::command]
async fn compare_with(state: State<'_, AppState>, second: String) -> Result<DiffResult, String> {
    let (path, password, _) = snapshot(&state)?;
    security::compare_pdfs(&path, Path::new(&second), password.as_deref())
}

#[tauri::command]
async fn open_result(state: State<'_, AppState>, path: String) -> Result<OpenInfo, String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err("Il file non esiste.".into());
    }
    apply_new_document(&state, &p, true)
}

#[tauri::command]
async fn start_share(state: State<'_, AppState>) -> Result<ShareInfo, String> {
    let (path, password, _) = snapshot(&state)?;
    let title = {
        let session = state.inner.lock().unwrap();
        session
            .original
            .as_ref()
            .or(session.working.as_ref())
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Documento PDF".into())
    };
    {
        let mut slot = state.share.lock().unwrap();
        *slot = None;
    }
    let share = collab::start(&path, password.as_deref(), &title)?;
    let info = share.info.clone();
    *state.share.lock().unwrap() = Some(share);
    Ok(info)
}

#[tauri::command]
async fn stop_share(state: State<'_, AppState>) -> Result<(), String> {
    let mut slot = state.share.lock().unwrap();
    *slot = None;
    Ok(())
}

#[tauri::command]
async fn get_share_comments(state: State<'_, AppState>) -> Result<Vec<ShareComment>, String> {
    let slot = state.share.lock().unwrap();
    Ok(slot.as_ref().map(collab::comments).unwrap_or_default())
}

#[tauri::command]
async fn add_share_comment(
    state: State<'_, AppState>,
    mut comment: ShareComment,
) -> Result<(), String> {
    if comment.id.is_empty() {
        comment.id = uuid::Uuid::new_v4().to_string();
    }
    if comment.created_at.is_empty() {
        comment.created_at = chrono::Local::now().format("%d/%m/%Y %H:%M").to_string();
    }
    let slot = state.share.lock().unwrap();
    if let Some(share) = slot.as_ref() {
        collab::add_comment(share, comment);
    }
    Ok(())
}


#[tauri::command]
async fn create_form_fields_cmd(
    state: State<'_, AppState>,
    fields: Vec<NewField>,
) -> Result<OpenInfo, String> {
    if fields.is_empty() {
        return Err("Aggiungi almeno un campo al modulo.".into());
    }
    let (path, password, _) = snapshot(&state)?;
    let dest = state::safe_working_path("modulo-creato");
    security::create_form_fields(&path, &dest, password.as_deref(), &fields)?;
    apply_new_working(&state, &dest, None)
}

#[tauri::command]
async fn tts_status(state: State<'_, AppState>) -> Result<bool, String> {
    let mut tts = state.tts.lock().unwrap();
    if let Some(child) = tts.as_mut() {
        match child.try_wait() {
            Ok(Some(_)) => {
                *tts = None;
                Ok(false)
            }
            Ok(None) => Ok(true),
            Err(_) => {
                *tts = None;
                Ok(false)
            }
        }
    } else {
        Ok(false)
    }
}

#[tauri::command]
async fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("Collegamento non valido.".into());
    }
    std::process::Command::new("cmd.exe")
        .args(["/C", "start", "", &url])
        .spawn()
        .map_err(|e| format!("Impossibile aprire il collegamento: {e}"))?;
    Ok(())
}
#[tauri::command]
fn startup_file() -> Option<String> {
    STARTUP_PDF.get().cloned().flatten()
}

fn chiave_registro_esiste(chiave: &str) -> bool {
    std::process::Command::new("reg.exe")
        .args(["query", chiave])
        .creation_flags(NESSUNA_FINESTRA)
        .output()
        .map(|uscita| uscita.status.success())
        .unwrap_or(false)
}

fn scrivi_registro(chiave: &str, nome: Option<&str>, valore: &str) -> bool {
    let mut comando = std::process::Command::new("reg.exe");
    comando.args(["add", chiave]);
    match nome {
        Some(nome) => comando.args(["/v", nome]),
        None => comando.args(["/ve"]),
    };
    comando.args(["/t", "REG_SZ", "/d", valore, "/f"]);
    comando
        .creation_flags(NESSUNA_FINESTRA)
        .output()
        .map(|uscita| uscita.status.success())
        .unwrap_or(false)
}

/// Registra Open PDF in «Apri con» di Windows. Scrive solo in HKCU e non cambia
/// l'app predefinita: quella la sceglie l'utente. Restituisce true alla prima volta.
#[tauri::command]
fn associa_pdf() -> Result<bool, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy().to_string();
    let nome = Path::new(&exe)
        .file_name()
        .map(|nome| nome.to_string_lossy().to_string())
        .ok_or("Percorso dell'eseguibile non valido.")?;
    let applicazione = format!("HKCU\\Software\\Classes\\Applications\\{nome}");
    let prima_volta = !chiave_registro_esiste(&applicazione);
    let comando = format!("\"{exe}\" \"%1\"");
    let icona = format!("\"{exe}\",0");
    let prog_id = "HKCU\\Software\\Classes\\OpenPDF.Document";
    let capacita = format!("{prog_id}\\Capabilities");
    let mut ok = true;
    ok &= scrivi_registro(&applicazione, Some("FriendlyAppName"), "Open PDF");
    ok &= scrivi_registro(
        &format!("{applicazione}\\shell\\open\\command"),
        None,
        &comando,
    );
    ok &= scrivi_registro(&format!("{applicazione}\\SupportedTypes"), Some(".pdf"), "");
    ok &= scrivi_registro(prog_id, None, "Documento PDF");
    ok &= scrivi_registro(&format!("{prog_id}\\DefaultIcon"), None, &icona);
    ok &= scrivi_registro(&format!("{prog_id}\\shell\\open\\command"), None, &comando);
    ok &= scrivi_registro(&capacita, Some("ApplicationName"), "Open PDF");
    ok &= scrivi_registro(
        &capacita,
        Some("ApplicationDescription"),
        "Lettore ed editor PDF locale",
    );
    ok &= scrivi_registro(&capacita, Some("ApplicationIcon"), &icona);
    ok &= scrivi_registro(
        &format!("{capacita}\\FileAssociations"),
        Some(".pdf"),
        "OpenPDF.Document",
    );
    ok &= scrivi_registro(
        "HKCU\\Software\\RegisteredApplications",
        Some("Open PDF"),
        "Software\\Classes\\OpenPDF.Document\\Capabilities",
    );
    ok &= scrivi_registro(
        "HKCU\\Software\\Classes\\.pdf\\OpenWithProgids",
        Some("OpenPDF.Document"),
        "",
    );
    if !ok {
        return Err("Windows non ha accettato l'associazione: riavvia Open PDF e riprova.".into());
    }
    Ok(prima_volta)
}

#[tauri::command]
fn controlla_aggiornamenti() -> Result<update::Aggiornamento, String> {
    update::controlla()
}

/// Scarica e avvia l'installer della release scelta, poi chiude l'app.
#[tauri::command]
fn installa_aggiornamento(url: String, app: tauri::AppHandle) -> Result<(), String> {
    let installer = update::scarica(&url)?;
    std::process::Command::new(&installer)
        .spawn()
        .map_err(|e| format!("Installer non avviato: {e}"))?;
    app.exit(0);
    Ok(())
}

fn main() {
    let startup = std::env::args()
        .skip(1)
        .find(|a| a.to_lowercase().ends_with(".pdf"));
    let _ = STARTUP_PDF.set(startup);
    let app_state = AppState {
        inner: Mutex::new(Session::default()),
        tts: Mutex::new(None),
        share: Mutex::new(None),
    };
    tauri::Builder::default()
        .manage(app_state)
        .setup(|app| {
            if let Ok(resource_dir) = app.path().resource_dir() {
                engine::set_resource_dir(resource_dir);
            }
            if let Some(window) = app.get_webview_window("main") {
                if let Ok(icon) = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png")) {
                    let _ = window.set_icon(icon);
                }
            }
            let _ = engine::init();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            engine_status,
            pick_pdf,
            pick_pdfs,
            pick_images,
            pick_file,
            clean_signature_photo,
            pick_signature_photo,
            pick_certificate,
            sign_pdf_pades,
            verify_signatures_cmd,
            pick_save,
            open_pdf,
            close_document,
            current_info,
            render_page,
            render_thumbnails,
            get_page_text,
            search_pdf,
            save_annotations_cmd,
            save_copy,
            export_annotated,
            speak_page,
            stop_speaking,
            fill_forms,
            apply_page_order,
            rotate_current_page,
            merge_pdfs_cmd,
            split_pdf_cmd,
            compress_pdf,
            apply_redactions,
            images_to_pdf_cmd,
            web_to_pdf_cmd,
            office_to_pdf_cmd,
            export_pdf,
            ocr_page,
            ocr_file,
            make_searchable_cmd,
            page_text_lines,
            delete_page_text,
            replace_text_in_place,
            protect_pdf_cmd,
            save_metadata,
            scrub_metadata_cmd,
            sanitize_pdf_cmd,
            compare_with,
            open_result,
            start_share,
            stop_share,
            get_share_comments,
            add_share_comment,
            create_form_fields_cmd,
            tts_status,
            open_url,
            startup_file,
            associa_pdf,
            controlla_aggiornamenti,
            installa_aggiornamento
        ])
        .run(tauri::generate_context!())
        .expect("Impossibile avviare Open PDF");
}

#[cfg(test)]
mod selftest {
    use super::*;
    use base64::Engine as _;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("openpdf-selftest");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn sample_pdf(name: &str) -> std::path::PathBuf {
        let path = temp(name);
        let text = "Open PDF\nDocumento di prova per annotazioni e ricerca.\nPagina uno.\n\nPagina due con contenuto extra.\n";
        convert::text_to_pdf(&path, "Documento di prova", text).unwrap();
        path
    }

    #[test]
    fn renders_text_and_annotations() {
        assert!(engine::init(), "pdfium init");
        let src = sample_pdf("sample.pdf");
        let (count, pages, meta, fields) = engine::document_info(&src, None).unwrap();
        assert!(count >= 1);
        assert_eq!(pages.len(), count);
        assert!(fields.is_empty());
        assert_eq!(meta.title, "");
        let png = engine::render_page_png(&src, 0, 900, None).unwrap();
        assert!(!png.png.is_empty());
        let text = engine::page_text(&src, 0, None).unwrap();
        assert!(text.contains("Open PDF"), "text was: {text}");

        let mut img = image::RgbaImage::new(30, 12);
        for px in img.pixels_mut() {
            *px = image::Rgba([20, 40, 160, 255]);
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let sig = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
        );

        let annotations = vec![
            Annotation {
                id: "a1".into(),
                page: 0,
                kind: "highlight".into(),
                x: 0.1,
                y: 0.1,
                w: 0.4,
                h: 0.04,
                color: "#ffd60a".into(),
                opacity: 0.35,
                ..Default::default()
            },
            Annotation {
                id: "a2".into(),
                page: 0,
                kind: "underline".into(),
                x: 0.1,
                y: 0.2,
                w: 0.4,
                h: 0.01,
                color: "#22d3ee".into(),
                stroke: 0.004,
                ..Default::default()
            },
            Annotation {
                id: "a3".into(),
                page: 0,
                kind: "ink".into(),
                points: vec![[0.1, 0.3], [0.2, 0.33], [0.3, 0.3], [0.4, 0.36]],
                color: "#f472b6".into(),
                stroke: 0.005,
                opacity: 1.0,
                ..Default::default()
            },
            Annotation {
                id: "a4".into(),
                page: 0,
                kind: "note".into(),
                x: 0.6,
                y: 0.2,
                w: 0.2,
                h: 0.12,
                text: "Nota di prova".into(),
                ..Default::default()
            },
            Annotation {
                id: "a5".into(),
                page: 0,
                kind: "stamp".into(),
                x: 0.62,
                y: 0.5,
                w: 0.24,
                h: 0.08,
                label: "APPROVATO".into(),
                color: "#34d399".into(),
                stroke: 0.004,
                ..Default::default()
            },
            Annotation {
                id: "a6".into(),
                page: 0,
                kind: "text".into(),
                x: 0.15,
                y: 0.6,
                w: 0.5,
                h: 0.1,
                text: "Testo aggiunto con Open PDF".into(),
                font_size: 0.022,
                color: "#1e293b".into(),
                opacity: 1.0,
                ..Default::default()
            },
            Annotation {
                id: "a7".into(),
                page: 0,
                kind: "signature".into(),
                x: 0.6,
                y: 0.72,
                w: 0.2,
                h: 0.08,
                data_url: sig,
                ..Default::default()
            },
        ];
        let out = temp("annotated.pdf");
        annotate::burn_annotations(&src, &out, None, &annotations).unwrap();
        assert!(out.exists());
        let out_png = engine::render_page_png(&out, 0, 900, None).unwrap();
        assert!(!out_png.png.is_empty());
    }

    #[test]
    fn page_operations_and_security() {
        let a = sample_pdf("ops-a.pdf");
        let b = sample_pdf("ops-b.pdf");
        let merged = temp("merged.pdf");
        pages::merge_pdfs(&merged, &[a.to_string_lossy().to_string(), b.to_string_lossy().to_string()]).unwrap();
        let (count, _, _, _) = engine::document_info(&merged, None).unwrap();
        assert_eq!(count, 2);
        let copy = temp("copy.pdf");
        pages::copy_pages(&merged, &copy, &[1, 0, 0], None).unwrap();
        let (count2, _, _, _) = engine::document_info(&copy, None).unwrap();
        assert_eq!(count2, 3);
        let rotated = temp("rotated.pdf");
        pages::rotate_page(&copy, &rotated, 0, 90, None).unwrap();
        let (_, pages, _, _) = engine::document_info(&rotated, None).unwrap();
        assert_eq!(pages[0].rotation, 90);
        let compressed = temp("compressed.pdf");
        let (_, after) = pages::compress_lossless(&merged, &compressed, None).unwrap();
        assert!(after > 0);
        let redacted = temp("redacted.pdf");
        pages::redact_pdf(&merged, &redacted, None, &[([0.1, 0.1, 0.4, 0.2], 0)], 150.0).unwrap();
        assert!(redacted.exists());
        let protected = temp("protected.pdf");
        security::protect_pdf(&merged, &protected, None, "user", "owner", true, false, false, false, false).unwrap();
        let doc = engine::open_doc(&protected, Some("user")).unwrap();
        assert!(doc.pages().len() >= 1);
        let meta = temp("meta.pdf");
        security::set_metadata(&merged, &meta, None, "Titolo", "Autore", "Soggetto", "kw", "Open PDF").unwrap();
        let (_, _, m, _) = engine::document_info(&meta, None).unwrap();
        assert_eq!(m.title, "Titolo");
        assert_eq!(m.author, "Autore");
    }

    #[test]
    fn searches_and_exports_office() {
        let src = sample_pdf("export-src.pdf");
        let hits = engine::search_document(&src, "prova", false, None).unwrap();
        assert!(!hits.is_empty());
        let docx = temp("export.docx");
        convert::export_docx(&src, &docx, None).unwrap();
        let xlsx = temp("export.xlsx");
        convert::export_xlsx(&src, &xlsx, None).unwrap();
        let pptx = temp("export.pptx");
        convert::export_pptx(&src, &pptx, None).unwrap();
        for (path, entry) in [(&docx, "word/document.xml"), (&xlsx, "xl/workbook.xml"), (&pptx, "ppt/presentation.xml")] {
            let file = std::fs::File::open(path).unwrap();
            let mut archive = zip::ZipArchive::new(file).unwrap();
            assert!(archive.by_name(entry).is_ok(), "missing {entry}");
        }
    }

    #[test]
    fn creates_and_fills_forms() {
        let src = sample_pdf("form-src.pdf");
        let form = temp("form.pdf");
        let fields = vec![
            NewField { page: 0, x: 0.1, y: 0.2, w: 0.3, h: 0.03, kind: "text".into(), name: "Nome".into() },
            NewField { page: 0, x: 0.1, y: 0.3, w: 0.03, h: 0.02, kind: "checkbox".into(), name: "Accetto".into() },
        ];
        security::create_form_fields(&src, &form, None, &fields).unwrap();
        let (_, _, _, found) = engine::document_info(&form, None).unwrap();
        assert!(found.len() >= 2, "found {} fields", found.len());
        let filled = temp("form-filled.pdf");
        let values = vec![FormValue { page: 0, index: found[0].index, value: "Mario Rossi".into(), checked: false }];
        engine::fill_form_fields(&form, &filled, None, &values).unwrap();
        let (_, _, _, still) = engine::document_info(&filled, None).unwrap();
        assert!(!still.is_empty());
    }

    #[test]
    fn renders_and_compares() {
        let a = sample_pdf("cmp-a.pdf");
        let b = sample_pdf("cmp-b.pdf");
        let diff = security::compare_pdfs(&a, &b, None).unwrap();
        assert!(diff.added_chars + diff.removed_chars == 0);
    }
}


#[cfg(test)]
mod ocr_manual_test {
    #[test]
    fn ocr_layer_is_searchable() {
        let image = std::path::Path::new(r"C:\Users\utente\AppData\Local\Temp\opencode\ocr-test.png");
        if !image.exists() {
            return;
        }
        let base = std::env::temp_dir().join("openpdf-ocr-selftest");
        std::fs::create_dir_all(&base).unwrap();
        let scan = base.join("scan.pdf");
        super::pages::images_to_pdf(&scan, &[image.to_string_lossy().to_string()]).unwrap();
        let searchable = base.join("scan-searchable.pdf");
        let pages_done = super::ocr::make_searchable(&scan, &searchable, None, 0, 0).unwrap();
        assert_eq!(pages_done, 1);
        let hits = super::engine::search_document(&searchable, "OCR", false, None).unwrap();
        assert!(!hits.is_empty(), "OCR layer not searchable");
    }
}

#[cfg(test)]
mod share_test {
    use std::io::{Read, Write};
    #[test]
    fn share_server_serves_review_page() {
        let src = std::env::temp_dir().join("openpdf-selftest").join("sample.pdf");
        if !src.exists() {
            return;
        }
        let share = super::collab::start(&src, None, "Test revisione").unwrap();
        let port = share.info.port;
        let token = share.info.token.clone();
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let request = format!("GET /?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.contains("Revisione"), "response was: {}", &response[..response.len().min(400)]);
        assert!(response.contains("__TOKEN__") == false);
        drop(share);
    }
}

#[cfg(test)]
mod textedit_test {
    #[test]
    fn deletes_existing_text_lines() {
        let base = std::env::temp_dir().join("openpdf-selftest");
        std::fs::create_dir_all(&base).unwrap();
        let src = base.join("edit-src.pdf");
        super::convert::text_to_pdf(&src, "Titolo di prova", "Documento di prova\nSeconda riga da mantenere.").unwrap();
        let lines = super::textedit::page_lines(&src, 0, None).unwrap();
        assert!(!lines.is_empty());
        let line = lines.iter().find(|l| l.text.contains("Documento")).expect("line not found");
        let dest = base.join("edit-dest.pdf");
        let removed = super::textedit::delete_lines(&src, &dest, None, 0, &[[line.x, line.y, line.w, line.h]]).unwrap();
        assert!(removed > 0);
        let after = super::engine::page_text(&dest, 0, None).unwrap();
        assert!(!after.contains("Documento di prova"), "testo ancora presente: {after}");
    }
}

