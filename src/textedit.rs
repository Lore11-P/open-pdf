use crate::engine;
use crate::types::TextLine;
use pdfium_render::prelude::*;
use std::path::Path;

pub fn page_lines(
    path: &Path,
    page_index: usize,
    password: Option<&str>,
) -> Result<Vec<TextLine>, String> {
    let doc = engine::open_doc(path, password)?;
    let page = doc.pages().get(page_index as i32).map_err(|e| e.to_string())?;
    let space = engine::page_space(&page)?;
    let text = page.text().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for segment in text.segments().iter() {
        let value = segment.text();
        if value.trim().is_empty() {
            continue;
        }
        let rect = engine::rect_to_norm(&space, &segment.bounds());
        if rect[2] <= 0.0001 || rect[3] <= 0.0001 {
            continue;
        }
        out.push(TextLine {
            text: value,
            x: rect[0],
            y: rect[1],
            w: rect[2],
            h: rect[3],
        });
        if out.len() >= 2500 {
            break;
        }
    }
    Ok(out)
}

fn overlap(a: [f32; 4], b: [f32; 4]) -> f32 {
    let left = a[0].max(b[0]);
    let top = a[1].max(b[1]);
    let right = (a[0] + a[2]).min(b[0] + b[2]);
    let bottom = (a[1] + a[3]).min(b[1] + b[3]);
    if right <= left || bottom <= top {
        0.0
    } else {
        (right - left) * (bottom - top)
    }
}

pub fn delete_lines(
    src: &Path,
    dest: &Path,
    password: Option<&str>,
    page_index: usize,
    rects: &[[f32; 4]],
) -> Result<usize, String> {
    let mut doc = engine::open_doc(src, password)?;
    let remove = {
        let page = doc.pages().get(page_index as i32).map_err(|e| e.to_string())?;
        let space = engine::page_space(&page)?;
        let count = page.objects().len();
        let mut remove = Vec::new();
        for index in 0..count {
            let object = match page.objects().get(index) {
                Ok(o) => o,
                Err(_) => continue,
            };
            if object.object_type() != PdfPageObjectType::Text {
                continue;
            }
            let bounds = match object.bounds() {
                Ok(b) => engine::rect_to_norm(&space, &b.to_rect()),
                Err(_) => continue,
            };
            let obj_area = (bounds[2] * bounds[3]).max(0.000001);
            for target in rects {
                let inter = overlap(bounds, *target);
                if inter <= 0.0 {
                    continue;
                }
                let target_area = (target[2] * target[3]).max(0.000001);
                if inter / target_area > 0.16 || inter / obj_area > 0.62 {
                    remove.push(index);
                    break;
                }
            }
        }
        remove
    };
    if remove.is_empty() {
        return Err("Nessun testo modificabile trovato in quest'area. Il contenuto potrebbe essere un'immagine o un oggetto raggruppato: usa la redazione per rimuoverlo.".into());
    }
    let removed = remove.len();
    {
        let mut page = doc.pages_mut().get(page_index as i32).map_err(|e| e.to_string())?;
        for index in remove.into_iter().rev() {
            page.objects_mut()
                .remove_object_at_index(index)
                .map_err(|e| e.to_string())?;
        }
        page.regenerate_content().map_err(|e| e.to_string())?;
    }
    doc.save_to_file(dest).map_err(|e| e.to_string())?;
    Ok(removed)
}

