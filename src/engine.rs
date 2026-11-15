use crate::types::*;
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

static PDFIUM: OnceLock<Pdfium> = OnceLock::new();
static INIT_ERR: OnceLock<String> = OnceLock::new();
static INIT_LOCK: Mutex<()> = Mutex::new(());
static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();
static DLL_PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn set_resource_dir(path: PathBuf) {
    let _ = RESOURCE_DIR.set(path);
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn dll_candidates() -> Vec<PathBuf> {
    let mut paths = vec![];
    if let Some(p) = DLL_PATH.get() {
        paths.push(p.clone());
    }
    if let Ok(p) = std::env::var("OPENPDF_PDFIUM") {
        paths.push(PathBuf::from(p));
    }
    let exe = exe_dir();
    paths.push(exe.join("pdfium.dll"));
    paths.push(exe.join("engines").join("pdfium.dll"));
    if let Some(res) = RESOURCE_DIR.get() {
        paths.push(res.join("engines").join("pdfium.dll"));
        paths.push(res.join("pdfium.dll"));
    }
    paths.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("engines").join("pdfium.dll"));
    paths.push(PathBuf::from("engines").join("pdfium.dll"));
    paths
}

pub fn bind() -> Result<&'static Pdfium, String> {
    if let Some(p) = PDFIUM.get() {
        return Ok(p);
    }
    if let Some(e) = INIT_ERR.get() {
        return Err(e.clone());
    }
    let _guard = INIT_LOCK.lock().unwrap();
    if let Some(p) = PDFIUM.get() {
        return Ok(p);
    }
    if let Some(e) = INIT_ERR.get() {
        return Err(e.clone());
    }
    let mut last_err = String::new();
    for path in dll_candidates() {
        if !path.exists() {
            continue;
        }
        match Pdfium::bind_to_library(&path) {
            Ok(bindings) => {
                let pdfium = Pdfium::new(bindings);
                let _ = DLL_PATH.set(path.clone());
                let _ = PDFIUM.set(pdfium);
                return Ok(PDFIUM.get().unwrap());
            }
            Err(e) => last_err = format!("{} ({})", path.display(), e),
        }
    }
    let msg = if last_err.is_empty() {
        "Motore PDFium non trovato. Verifica engines/pdfium.dll".to_string()
    } else {
        format!("Impossibile avviare PDFium: {last_err}")
    };
    let _ = INIT_ERR.set(msg.clone());
    Err(msg)
}

pub fn init() -> bool {
    bind().is_ok()
}

pub fn pdfium_path() -> String {
    DLL_PATH
        .get()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn it_err(e: impl std::fmt::Display) -> String {
    let raw = e.to_string();
    if raw.to_lowercase().contains("password") {
        "Il documento Ã¨ protetto da password. Inserisci la password per aprirlo.".into()
    } else {
        format!("Errore PDF: {raw}")
    }
}

pub fn open_doc(path: &Path, password: Option<&str>) -> Result<PdfDocument<'static>, String> {
    let pdfium = bind()?;
    pdfium
        .load_pdf_from_file(path, password)
        .map_err(|e| it_err(e))
}

#[derive(Clone)]
pub struct PageSpace {
    pub x0: f32,
    pub y0: f32,
    pub width: f32,
    pub height: f32,
    pub rotation: i32,
    pub display_width: f32,
    pub display_height: f32,
}

pub fn page_space(page: &PdfPage) -> Result<PageSpace, String> {
    let media = page
        .boundaries()
        .media()
        .map_err(|e| it_err(e))?
        .bounds;
    let width = media.width().value;
    let height = media.height().value;
    let x0 = media.left().value;
    let y0 = media.bottom().value;
    let rotation = match page.rotation().map_err(|e| it_err(e))? {
        PdfPageRenderRotation::None => 0,
        PdfPageRenderRotation::Degrees90 => 90,
        PdfPageRenderRotation::Degrees180 => 180,
        PdfPageRenderRotation::Degrees270 => 270,
    };
    let (display_width, display_height) = if rotation == 90 || rotation == 270 {
        (height, width)
    } else {
        (width, height)
    };
    Ok(PageSpace {
        x0,
        y0,
        width,
        height,
        rotation,
        display_width,
        display_height,
    })
}

pub fn document_info(
    path: &Path,
    password: Option<&str>,
) -> Result<(usize, Vec<PageInfo>, DocMeta, Vec<FormFieldInfo>), String> {
    let doc = open_doc(path, password)?;
    let count = doc.pages().len() as usize;
    let mut pages = Vec::with_capacity(count);
    for i in 0..count {
        let page = doc.pages().get(i as i32).map_err(|e| it_err(e))?;
        let space = page_space(&page)?;
        pages.push(PageInfo {
            index: i,
            width: space.width,
            height: space.height,
            rotation: space.rotation,
            display_width: space.display_width,
            display_height: space.display_height,
        });
    }
    let meta = read_meta(&doc);
    let fields = read_form_fields(&doc);
    Ok((count, pages, meta, fields))
}

fn read_meta(doc: &PdfDocument) -> DocMeta {
    let mut meta = DocMeta::default();
    for tag in doc.metadata().iter() {
        let value = tag.value().to_string();
        match tag.tag_type() {
            PdfDocumentMetadataTagType::Title => meta.title = value,
            PdfDocumentMetadataTagType::Author => meta.author = value,
            PdfDocumentMetadataTagType::Subject => meta.subject = value,
            PdfDocumentMetadataTagType::Keywords => meta.keywords = value,
            PdfDocumentMetadataTagType::Creator => meta.creator = value,
            PdfDocumentMetadataTagType::Producer => meta.producer = value,
            PdfDocumentMetadataTagType::CreationDate => meta.creation_date = value,
            PdfDocumentMetadataTagType::ModificationDate => meta.modification_date = value,
        }
    }
    meta
}

fn field_kind(field: &PdfFormField) -> &'static str {
    match field.field_type() {
        PdfFormFieldType::Text => "text",
        PdfFormFieldType::Checkbox => "checkbox",
        PdfFormFieldType::RadioButton => "radio",
        PdfFormFieldType::ComboBox => "combo",
        PdfFormFieldType::ListBox => "list",
        PdfFormFieldType::Signature => "signature",
        PdfFormFieldType::PushButton => "button",
        PdfFormFieldType::Unknown => "unknown",
    }
}

fn field_value(field: &PdfFormField) -> (String, bool) {
    if let Some(f) = field.as_text_field() {
        return (f.value().unwrap_or_default(), false);
    }
    if let Some(f) = field.as_checkbox_field() {
        return (String::new(), f.is_checked().unwrap_or(false));
    }
    if let Some(f) = field.as_combo_box_field() {
        return (f.value().unwrap_or_default(), false);
    }
    if let Some(f) = field.as_list_box_field() {
        return (f.value().unwrap_or_default(), false);
    }
    if let Some(f) = field.as_radio_button_field() {
        return (f.group_value().unwrap_or_default(), f.is_checked().unwrap_or(false));
    }
    (String::new(), false)
}

fn read_form_fields(doc: &PdfDocument) -> Vec<FormFieldInfo> {
    let mut out = vec![];
    let _ = doc.form();
    for (page_idx, page) in doc.pages().iter().enumerate() {
        let mut idx = 0usize;
        for annotation in page.annotations().iter() {
            if let Some(field) = annotation.as_form_field() {
                let (value, checked) = field_value(field);
                out.push(FormFieldInfo {
                    page: page_idx,
                    index: idx,
                    label: format!("Pagina {} Â· Campo {}", page_idx + 1, idx + 1),
                    kind: field_kind(field).to_string(),
                    value,
                    checked,
                });
            }
            idx += 1;
        }
    }
    out
}

pub fn fill_form_fields(
    path: &Path,
    dest: &Path,
    password: Option<&str>,
    values: &[FormValue],
) -> Result<(), String> {
    let mut doc = open_doc(path, password)?;
    let _ = doc.form();
    for v in values {
        let mut page = doc.pages_mut().get(v.page as i32).map_err(|e| it_err(e))?;
        let annotations = page.annotations_mut();
        let mut annotation = annotations.get(v.index).map_err(|e| it_err(e))?;
        if let Some(field) = annotation.as_form_field_mut() {
            if let Some(text) = field.as_text_field_mut() {
                text.set_value(&v.value).map_err(|e| it_err(e))?;
            } else if let Some(check) = field.as_checkbox_field_mut() {
                check.set_checked(v.checked).map_err(|e| it_err(e))?;
            }
        }
    }
    doc.save_to_file(dest).map_err(|e| it_err(e))?;
    Ok(())
}

pub fn render_page_png(
    path: &Path,
    page_index: usize,
    width: u32,
    password: Option<&str>,
) -> Result<RenderResult, String> {
    let doc = open_doc(path, password)?;
    let page = doc.pages().get(page_index as i32).map_err(|e| it_err(e))?;
    let target = width.clamp(160, 3200) as i32;
    let config = PdfRenderConfig::new().set_target_width(target);
    let bitmap = page.render_with_config(&config).map_err(|e| it_err(e))?;
    let w = bitmap.width();
    let h = bitmap.height();
    let image = bitmap.as_image().map_err(|e| it_err(e))?;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(RenderResult {
        png: B64.encode(bytes.into_inner()),
        width: w as u32,
        height: h as u32,
    })
}

pub fn render_page_image(
    doc: &PdfDocument,
    page_index: usize,
    width: u32,
) -> Result<image::DynamicImage, String> {
    let page = doc.pages().get(page_index as i32).map_err(|e| it_err(e))?;
    let config = PdfRenderConfig::new().set_target_width(width.clamp(160, 4000) as i32);
    let bitmap = page.render_with_config(&config).map_err(|e| it_err(e))?;
    bitmap.as_image().map_err(|e| it_err(e))
}

pub fn render_thumbnails(
    path: &Path,
    max_width: u32,
    password: Option<&str>,
) -> Result<Vec<RenderResult>, String> {
    let doc = open_doc(path, password)?;
    let count = doc.pages().len();
    let limit = count.min(120);
    let mut out = Vec::with_capacity(limit as usize);
    for i in 0..limit {
        let page = doc.pages().get(i).map_err(|e| it_err(e))?;
        let config = PdfRenderConfig::new().set_target_width(max_width as i32);
        let bitmap = page.render_with_config(&config).map_err(|e| it_err(e))?;
        let w = bitmap.width();
        let h = bitmap.height();
        let image = bitmap.as_image().map_err(|e| it_err(e))?;
        let mut bytes = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut bytes, image::ImageFormat::Png)
            .map_err(|e| e.to_string())?;
        out.push(RenderResult {
            png: B64.encode(bytes.into_inner()),
            width: w as u32,
            height: h as u32,
        });
    }
    Ok(out)
}

pub fn page_text(path: &Path, page_index: usize, password: Option<&str>) -> Result<String, String> {
    let doc = open_doc(path, password)?;
    let page = doc.pages().get(page_index as i32).map_err(|e| it_err(e))?;
    let text = page.text().map_err(|e| it_err(e))?;
    Ok(text.all())
}

pub fn page_text_from_doc(doc: &PdfDocument, page_index: usize) -> Result<String, String> {
    let page = doc.pages().get(page_index as i32).map_err(|e| it_err(e))?;
    let text = page.text().map_err(|e| it_err(e))?;
    Ok(text.all())
}

pub fn to_norm(space: &PageSpace, x: f32, y: f32) -> (f32, f32) {
    let u = ((x - space.x0) / space.width).clamp(-1.0, 2.0);
    let v = ((y - space.y0) / space.height).clamp(-1.0, 2.0);
    match space.rotation {
        90 => (v, u),
        180 => (1.0 - u, v),
        270 => (1.0 - v, 1.0 - u),
        _ => (u, 1.0 - v),
    }
}

pub fn rect_to_norm(space: &PageSpace, rect: &PdfRect) -> [f32; 4] {
    let left = rect.left().value;
    let right = rect.right().value;
    let bottom = rect.bottom().value;
    let top = rect.top().value;
    let corners = [
        to_norm(space, left, bottom),
        to_norm(space, right, bottom),
        to_norm(space, right, top),
        to_norm(space, left, top),
    ];
    let minx = corners.iter().map(|c| c.0).fold(f32::INFINITY, f32::min);
    let maxx = corners.iter().map(|c| c.0).fold(f32::NEG_INFINITY, f32::max);
    let miny = corners.iter().map(|c| c.1).fold(f32::INFINITY, f32::min);
    let maxy = corners.iter().map(|c| c.1).fold(f32::NEG_INFINITY, f32::max);
    [minx, miny, (maxx - minx).max(0.0), (maxy - miny).max(0.0)]
}

pub fn search_document(
    path: &Path,
    query: &str,
    match_case: bool,
    password: Option<&str>,
) -> Result<Vec<SearchHit>, String> {
    let doc = open_doc(path, password)?;
    let count = doc.pages().len();
    let mut hits = Vec::new();
    let needle = query.to_lowercase();
    for i in 0..count {
        if hits.len() >= 300 {
            break;
        }
        let page = doc.pages().get(i).map_err(|e| it_err(e))?;
        let space = page_space(&page)?;
        let text = page.text().map_err(|e| it_err(e))?;
        let all = text.all();
        if all.is_empty() {
            continue;
        }
        let searchable = if match_case {
            all.clone()
        } else {
            all.to_lowercase()
        };
        if !searchable.contains(&needle) {
            continue;
        }
        let options = PdfSearchOptions::new().match_case(match_case);
        if let Ok(search) = text.search(query, &options) {
            let mut iter = search.iter(PdfSearchDirection::SearchForward);
            while let Some(segments) = iter.next() {
                let mut rects = vec![];
                let mut label = String::new();
                for segment in segments.iter() {
                    label.push_str(&segment.text());
                    rects.push(rect_to_norm(&space, &segment.bounds()));
                }
                let idx = searchable.find(&needle).unwrap_or(0);
                let start = idx.saturating_sub(50);
                let end = (idx + query.len() + 70).min(all.len());
                let snippet = all
                    .chars()
                    .skip(start)
                    .take(end - start)
                    .collect::<String>()
                    .replace('\n', " ")
                    .trim()
                    .to_string();
                hits.push(SearchHit {
                    page: i as usize,
                    text: if snippet.is_empty() { label } else { snippet },
                    rects,
                });
                if hits.len() >= 300 {
                    break;
                }
            }
        };
    }
    Ok(hits)
}

pub fn find_executable(names: &[&str]) -> Option<PathBuf> {
    for name in names {
        if let Ok(out) = std::process::Command::new("where.exe").arg(name).output() {
            if out.status.success() {
                if let Some(line) = String::from_utf8_lossy(&out.stdout).lines().next() {
                    let p = PathBuf::from(line.trim());
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
}

pub fn edge_path() -> Option<PathBuf> {
    let candidates = [
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ];
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
}

pub fn office_path() -> Option<PathBuf> {
    find_executable(&["soffice.exe", "soffice"])
}

pub fn tesseract_path() -> Option<PathBuf> {
    find_executable(&["tesseract.exe", "tesseract"])
}

pub fn engine_status() -> EngineStatus {
    let pdfium_ok = bind().is_ok();
    let edge = edge_path().is_some();
    let office = office_path().is_some();
    let tesseract = tesseract_path().is_some();
    let ocr = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", "Get-Command powershell.exe | Out-Null; [Windows.Media.Ocr.OcrEngine,Windows.Foundation,ContentType=WindowsRuntime] | Out-Null; 'ok'"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    EngineStatus {
        pdfium: pdfium_ok,
        pdfium_path: pdfium_path(),
        edge,
        office,
        tesseract,
        ocr_windows: ocr,
        version: format!(
            "PDFium {} Â· WebView2 Â· OCR {}",
            if pdfium_ok { "attivo" } else { "assente" },
            if ocr { "Windows" } else { "limitatO" }
        ),
    }
}




