use crate::engine;
use crate::types::{DiffPage, DiffResult};
use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
use lopdf::{Document, EncryptionState, EncryptionVersion, Permissions};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

fn load(src: &Path, password: Option<&str>) -> Result<Document, String> {
    match password {
        Some(p) => Document::load_with_password(src, p),
        None => Document::load(src),
    }
    .map_err(|e| format!("Apertura PDF non riuscita: {e}"))
}

pub fn protect_pdf(
    src: &Path,
    dest: &Path,
    password: Option<&str>,
    user_password: &str,
    owner_password: &str,
    allow_print: bool,
    allow_copy: bool,
    allow_modify: bool,
    allow_annotate: bool,
    allow_forms: bool,
) -> Result<(), String> {
    let mut doc = load(src, password)?;
    let mut permissions = Permissions::COPYABLE_FOR_ACCESSIBILITY
        | Permissions::PRINTABLE_IN_HIGH_QUALITY;
    if allow_print {
        permissions |= Permissions::PRINTABLE;
    }
    if allow_copy {
        permissions |= Permissions::COPYABLE;
    }
    if allow_modify {
        permissions |= Permissions::MODIFIABLE;
    }
    if allow_annotate {
        permissions |= Permissions::ANNOTABLE;
    }
    if allow_forms {
        permissions |= Permissions::FILLABLE;
    }
    let owner = if owner_password.is_empty() {
        user_password.to_string()
    } else {
        owner_password.to_string()
    };
    let user = user_password.to_string();
    let filter: Arc<dyn CryptFilter> = Arc::new(Aes128CryptFilter);
    let version = EncryptionVersion::V4 {
        document: &doc,
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: &owner,
        user_password: &user,
        permissions,
    };
    let state = EncryptionState::try_from(version).map_err(|e| format!("Crittografia non riuscita: {e}"))?;
    doc.encrypt(&state)
        .map_err(|e| format!("Crittografia non riuscita: {e}"))?;
    doc.save(dest)
        .map_err(|e| format!("Salvataggio protetto non riuscito: {e}"))?;
    Ok(())
}

pub fn set_metadata(
    src: &Path,
    dest: &Path,
    password: Option<&str>,
    title: &str,
    author: &str,
    subject: &str,
    keywords: &str,
    creator: &str,
) -> Result<(), String> {
    let mut doc = load(src, password)?;
    let info_id = doc
        .trailer
        .get(b"Info")
        .and_then(|o| o.as_reference())
        .ok();
    let id = match info_id {
        Some(id) => id,
        None => {
            let id = doc.add_object(lopdf::Object::Dictionary(lopdf::Dictionary::new()));
            doc.trailer.set("Info", lopdf::Object::Reference(id));
            id
        }
    };
    {
        let info = doc
            .get_object_mut(id)
            .and_then(|o| o.as_dict_mut())
            .map_err(|e| format!("Metadati non accessibili: {e}"))?;
        info.set("Title", lopdf::Object::string_literal(title));
        info.set("Author", lopdf::Object::string_literal(author));
        info.set("Subject", lopdf::Object::string_literal(subject));
        info.set("Keywords", lopdf::Object::string_literal(keywords));
        info.set("Creator", lopdf::Object::string_literal(creator));
        info.set("Producer", lopdf::Object::string_literal("Open PDF"));
    }
    doc.save(dest)
        .map_err(|e| format!("Salvataggio metadati non riuscito: {e}"))?;
    Ok(())
}

pub fn scrub_metadata(src: &Path, dest: &Path, password: Option<&str>) -> Result<(), String> {
    let mut doc = load(src, password)?;
    if let Ok(root_id) = doc.trailer.get(b"Root").and_then(|o| o.as_reference()) {
        if let Ok(root) = doc.get_dictionary_mut(root_id) {
            root.remove(b"Metadata");
        }
    }
    if let Ok(info_id) = doc.trailer.get(b"Info").and_then(|o| o.as_reference()) {
        let _ = doc.delete_object(info_id);
    }
    doc.trailer.remove(b"Info");
    doc.save(dest)
        .map_err(|e| format!("Pulizia metadati non riuscita: {e}"))?;
    Ok(())
}

pub fn compare_pdfs(
    first: &Path,
    second: &Path,
    password: Option<&str>,
) -> Result<DiffResult, String> {
    let doc_a = engine::open_doc(first, password)?;
    let doc_b = engine::open_doc(second, password)?;
    let count_a = doc_a.pages().len() as usize;
    let count_b = doc_b.pages().len() as usize;
    let count = count_a.max(count_b);
    let mut pages = Vec::new();
    let mut total_added = 0usize;
    let mut total_removed = 0usize;
    for i in 0..count {
        let a = if i < count_a {
            engine::page_text_from_doc(&doc_a, i).unwrap_or_default()
        } else {
            String::new()
        };
        let b = if i < count_b {
            engine::page_text_from_doc(&doc_b, i).unwrap_or_default()
        } else {
            String::new()
        };
        if a == b {
            continue;
        }
        let diff = similar::TextDiff::from_lines(&a, &b);
        let mut added = Vec::new();
        let mut removed = Vec::new();
        for change in diff.iter_all_changes() {
            let value = change.value().trim().to_string();
            if value.is_empty() {
                continue;
            }
            match change.tag() {
                similar::ChangeTag::Insert => added.push(value),
                similar::ChangeTag::Delete => removed.push(value),
                similar::ChangeTag::Equal => {}
            }
        }
        total_added += added.iter().map(|s| s.len()).sum::<usize>();
        total_removed += removed.iter().map(|s| s.len()).sum::<usize>();
        added.truncate(80);
        removed.truncate(80);
        pages.push(DiffPage {
            page: i + 1,
            changed: !added.is_empty() || !removed.is_empty(),
            added,
            removed,
        });
        if pages.len() >= 100 {
            break;
        }
    }
    Ok(DiffResult {
        pages,
        added_chars: total_added,
        removed_chars: total_removed,
    })
}

pub fn remove_password(src: &Path, dest: &Path, password: &str) -> Result<(), String> {
    let mut doc = Document::load_with_password(src, password)
        .map_err(|e| format!("Password non valida o file non leggibile: {e}"))?;
    doc.decrypt(password)
        .map_err(|e| format!("Decrittografia non riuscita: {e}"))?;
    doc.save(dest)
        .map_err(|e| format!("Salvataggio non riuscito: {e}"))?;
    Ok(())
}



fn num(v: f32) -> lopdf::Object {
    if (v.fract()).abs() < 0.0001 {
        lopdf::Object::Integer(v.round() as i64)
    } else {
        lopdf::Object::Real(v)
    }
}

fn ref_array(doc: &Document, value: Option<&lopdf::Object>) -> Vec<lopdf::Object> {
    match value {
        Some(lopdf::Object::Array(items)) => items.clone(),
        Some(lopdf::Object::Reference(id)) => doc
            .get_object(*id)
            .ok()
            .and_then(|o| o.as_array().ok())
            .cloned()
            .unwrap_or_default(),
        _ => vec![],
    }
}

pub fn create_form_fields(
    src: &Path,
    dest: &Path,
    password: Option<&str>,
    fields: &[crate::types::NewField],
) -> Result<(), String> {
    use lopdf::{dictionary, Dictionary, Object};

    let mut doc = load(src, password)?;
    let spaces: Vec<crate::engine::PageSpace> = {
        let pdoc = engine::open_doc(src, password)?;
        let count = pdoc.pages().len();
        let mut spaces = Vec::with_capacity(count as usize);
        for i in 0..count {
            let page = pdoc.pages().get(i).map_err(|e| e.to_string())?;
            spaces.push(engine::page_space(&page)?);
        }
        spaces
    };

    let root_id = doc
        .trailer
        .get(b"Root")
        .and_then(|o| o.as_reference())
        .map_err(|e| format!("Catalogo PDF non trovato: {e}"))?;
    let pages = doc.get_pages();

    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let font_ref = Object::Reference(font_id);

    let existing_acro = {
        let catalog = doc.get_dictionary(root_id).map_err(|e| e.to_string())?;
        match catalog.get(b"AcroForm").ok() {
            Some(Object::Reference(id)) => Some((Some(*id), None)),
            Some(Object::Dictionary(d)) => Some((None, Some(d.clone()))),
            _ => None,
        }
    };

    let (acro_id, mut acro_fields) = match existing_acro {
        Some((Some(id), _)) => {
            let fields = doc
                .get_dictionary(id)
                .ok()
                .and_then(|d| d.get(b"Fields").ok())
                .map(|o| ref_array(&doc, Some(o)))
                .unwrap_or_default();
            (id, fields)
        }
        Some((None, Some(dict))) => {
            let fields = dict
                .get(b"Fields")
                .ok()
                .map(|o| ref_array(&doc, Some(o)))
                .unwrap_or_default();
            let id = doc.add_object(Object::Dictionary(dict));
            doc.get_dictionary_mut(root_id)
                .map_err(|e| e.to_string())?
                .set("AcroForm", Object::Reference(id));
            (id, fields)
        }
        _ => {
            let id = doc.add_object(dictionary! {
                "Fields" => Object::Array(vec![]),
                "NeedAppearances" => true,
            });
            doc.get_dictionary_mut(root_id)
                .map_err(|e| e.to_string())?
                .set("AcroForm", Object::Reference(id));
            (id, vec![])
        }
    };

    for (index, field) in fields.iter().enumerate() {
        let Some(page_id) = pages.get(&((field.page + 1) as u32)) else {
            continue;
        };
        let space = match spaces.get(field.page) {
            Some(s) => s,
            None => continue,
        };
        let (x1, y1) = crate::annotate::norm_to_page(space, field.x, field.y);
        let (x2, y2) = crate::annotate::norm_to_page(
            space,
            (field.x + field.w).clamp(0.0, 1.0),
            (field.y + field.h).clamp(0.0, 1.0),
        );
        let left = x1.min(x2);
        let right = x1.max(x2);
        let bottom = y1.min(y2);
        let top = y1.max(y2);
        let rect = Object::Array(vec![num(left), num(bottom), num(right), num(top)]);
        let name = if field.name.trim().is_empty() {
            format!("Campo_{}", index + 1)
        } else {
            field.name.clone()
        };

        let mut annot = Dictionary::new();
        annot.set("Type", "Annot");
        annot.set("Subtype", "Widget");
        annot.set("T", Object::string_literal(name));
        annot.set("Rect", rect);
        annot.set("P", Object::Reference(*page_id));
        annot.set("F", 4);
        if field.kind == "checkbox" {
            annot.set("FT", "Btn");
            annot.set("V", "Off");
            annot.set("AS", "Off");
            annot.set(
                "MK",
                dictionary! {
                    "BC" => Object::Array(vec![num(0.0), num(0.0), num(0.0)]),
                    "BG" => Object::Array(vec![num(1.0), num(1.0), num(1.0)]),
                },
            );
        } else {
            annot.set("FT", "Tx");
            annot.set("V", Object::string_literal(""));
            annot.set("DA", Object::string_literal("/Helv 12 Tf 0 0 0 rg"));
            annot.set("Q", 0);
            annot.set(
                "MK",
                dictionary! {
                    "BC" => Object::Array(vec![num(0.2), num(0.2), num(0.2)]),
                    "BG" => Object::Array(vec![num(1.0), num(1.0), num(1.0)]),
                },
            );
        }
        let annot_id = doc.add_object(Object::Dictionary(annot));
        acro_fields.push(Object::Reference(annot_id));

        let mut annots = {
            let page = doc.get_dictionary(*page_id).map_err(|e| e.to_string())?;
            ref_array(&doc, page.get(b"Annots").ok())
        };
        annots.push(Object::Reference(annot_id));
        doc.get_dictionary_mut(*page_id)
            .map_err(|e| e.to_string())?
            .set("Annots", Object::Array(annots));
    }

    {
        let acro = doc.get_dictionary_mut(acro_id).map_err(|e| e.to_string())?;
        acro.set("Fields", Object::Array(acro_fields));
        acro.set("DA", Object::string_literal("/Helv 0 Tf 0 g"));
        acro.set("NeedAppearances", true);
        acro.set(
            "DR",
            dictionary! {
                "Font" => dictionary! {
                    "Helv" => font_ref,
                },
            },
        );
    }

    doc.save(dest)
        .map_err(|e| format!("Creazione modulo non riuscita: {e}"))?;
    Ok(())
}
