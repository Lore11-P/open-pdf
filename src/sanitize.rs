//! Pulizia profonda dei dati nascosti di un PDF.
//!
//! A differenza della redazione (che interviene sul contenuto visibile), questo
//! modulo elimina le informazioni che il documento porta con sé ma non mostra:
//! metadati, XMP, JavaScript, allegati, EXIF nelle immagini JPEG, annotazioni
//! nascoste e strutture non più raggiungibili.
//!
//! I livelli opzionali (`/OCProperties`) vengono deliberatamente conservati:
//! rimuoverli cambierebbe la resa delle pagine.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::HashSet;
use std::path::Path;

/// Contatori delle rimozioni effettuate da [`sanitize_pdf`].
#[derive(Default, Debug, Clone)]
pub struct SanitizeReport {
    /// Voci di metadati rimosse: `/Info`, XMP, `/PieceInfo`, `/Thumb`.
    pub metadata: usize,
    /// Azioni JavaScript, name tree `/JavaScript` e `/XFA` rimossi.
    pub scripts: usize,
    /// Allegati rimossi: `/EmbeddedFiles`, `/FileAttachment`, `/AF`.
    pub attachments: usize,
    /// Immagini ripulite da EXIF/APPn/COM o da metadati nel dizionario.
    pub images: usize,
    /// Dati nascosti: annotazioni invisibili, popup orfani, struttura di accessibilità.
    pub hidden: usize,
    /// Oggetti non più raggiungibili eliminati dal file.
    pub pruned: usize,
}

/// Rimuove i dati nascosti da `src` e salva il risultato in `dest`.
///
/// Se il documento è cifrato serve la password: senza di essa la funzione
/// restituisce un errore esplicito e non tocca `/Encrypt`.
pub fn sanitize_pdf(
    src: &Path,
    dest: &Path,
    password: Option<&str>,
) -> Result<SanitizeReport, String> {
    let mut doc = load(src, password)?;
    let mut report = SanitizeReport::default();

    let root_id = doc
        .trailer
        .get(b"Root")
        .and_then(|object| object.as_reference())
        .map_err(|_| "Catalogo PDF assente: file non valido".to_string())?;

    // 1) Metadati del trailer: /Info non è più raggiungibile dopo la rimozione.
    if doc.trailer.remove(b"Info").is_some() {
        report.metadata += 1;
    }

    sanitize_catalog(&mut doc, root_id, &mut report);

    // 2) Pagine, annotazioni e azioni.
    let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    let mut removed_annots: HashSet<ObjectId> = HashSet::new();
    for page_id in pages {
        removed_annots.extend(sanitize_page(&mut doc, page_id, &mut report));
    }

    // 3) Campi modulo: azioni JavaScript e riferimenti a widget eliminati.
    sanitize_acroform(&mut doc, root_id, &removed_annots, &mut report);

    // 4) Azioni JavaScript rimaste in altre strutture (segnalibri, link, ...).
    sweep_js_actions(&mut doc, &mut report);

    // 5) Immagini: EXIF/APPn/COM nei JPEG e metadati negli XObject.
    sanitize_images(&mut doc, &mut report);

    // 6) Oggetti rimasti orfani dopo le rimozioni.
    report.pruned = doc.prune_objects().len();

    doc.save(dest)
        .map_err(|e| format!("Salvataggio del PDF ripulito non riuscito: {e}"))?;
    Ok(report)
}

fn load(src: &Path, password: Option<&str>) -> Result<Document, String> {
    let doc = match password {
        Some(value) => Document::load_with_password(src, value).map_err(|e| {
            format!("Apertura del PDF non riuscita (password errata o file danneggiato): {e}")
        })?,
        None => Document::load(src).map_err(|e| format!("Apertura del PDF non riuscita: {e}"))?,
    };
    if doc.is_encrypted() {
        return Err(
            "Il PDF è cifrato: per la pulizia dei dati nascosti è necessaria la password."
                .to_string(),
        );
    }
    Ok(doc)
}

fn name_of(object: &Object) -> Option<String> {
    match object {
        Object::Name(bytes) => Some(String::from_utf8_lossy(bytes).to_string()),
        _ => None,
    }
}

fn deref<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Object> {
    match object {
        Object::Reference(id) => doc.get_object(*id).ok(),
        other => Some(other),
    }
}

fn dict_key_name(dict: &Dictionary, key: &[u8]) -> Option<String> {
    dict.get(key).ok().and_then(name_of)
}

/// Numero di voci in un name tree, usato solo per il rapporto.
fn name_tree_entries(doc: &Document, object: &Object) -> usize {
    fn walk(doc: &Document, object: &Object, depth: usize) -> usize {
        if depth > 16 {
            return 0;
        }
        let Some(object) = deref(doc, object) else {
            return 0;
        };
        let Object::Dictionary(dict) = object else {
            return 0;
        };
        let mut count = 0;
        if let Ok(Object::Array(items)) = dict.get(b"Names") {
            count += items.len() / 2;
        }
        if let Ok(Object::Array(kids)) = dict.get(b"Kids") {
            for kid in kids {
                count += walk(doc, kid, depth + 1);
            }
        }
        count
    }
    walk(doc, object, 0).max(1)
}

fn array_len(doc: &Document, object: &Object) -> usize {
    match deref(doc, object) {
        Some(Object::Array(items)) => items.len().max(1),
        _ => 1,
    }
}

fn is_js_action(doc: &Document, object: &Object, depth: usize) -> bool {
    if depth > 16 {
        return false;
    }
    let Some(object) = deref(doc, object) else {
        return false;
    };
    match object {
        Object::Dictionary(dict) => {
            if dict_key_name(dict, b"S").as_deref() == Some("JavaScript") {
                return true;
            }
            if dict.has(b"JS") {
                return true;
            }
            dict.get(b"Next")
                .map(|next| is_js_action(doc, next, depth + 1))
                .unwrap_or(false)
        }
        Object::Array(items) => items
            .iter()
            .any(|item| is_js_action(doc, item, depth + 1)),
        _ => false,
    }
}

/// Rimuove `/OpenAction`, `/A` e le voci `/AA` che eseguono JavaScript.
fn strip_js_actions(doc: &mut Document, holder: &mut Dictionary) -> usize {
    let mut removed = 0;

    if let Some(action) = holder.get(b"OpenAction").ok().cloned() {
        if is_js_action(doc, &action, 0) {
            holder.remove(b"OpenAction");
            removed += 1;
        }
    }
    if let Some(action) = holder.get(b"A").ok().cloned() {
        if is_js_action(doc, &action, 0) {
            holder.remove(b"A");
            removed += 1;
        }
    }

    let Some(value) = holder.get(b"AA").ok().cloned() else {
        return removed;
    };
    let (target_id, mut events) = match &value {
        Object::Dictionary(dict) => (None, dict.clone()),
        Object::Reference(id) => match doc.get_dictionary(*id).cloned() {
            Ok(dict) => (Some(*id), dict),
            Err(_) => return removed,
        },

        _ => return removed,
    };

    let keys: Vec<Vec<u8>> = events.iter().map(|(key, _)| key.clone()).collect();
    for key in keys {
        let drop = events
            .get(&key)
            .map(|action| is_js_action(doc, action, 0))
            .unwrap_or(false);
        if drop {
            events.remove(&key);
            removed += 1;
        }
    }

    if events.is_empty() {
        holder.remove(b"AA");
        // Se gli eventi erano un dizionario indiretto, va svuotato: altrimenti
        // resterebbe raggiungibile con dentro il codice JavaScript.
        if let Some(id) = target_id {
            if let Ok(target) = doc.get_dictionary_mut(id) {
                *target = Dictionary::new();
            }
        }
    } else {
        match target_id {
            Some(id) => {
                if let Ok(target) = doc.get_dictionary_mut(id) {
                    *target = events;
                }
            }
            None => holder.set("AA", Object::Dictionary(events)),
        }
    }
    removed
}

fn sanitize_catalog(doc: &mut Document, root_id: ObjectId, report: &mut SanitizeReport) {
    let mut catalog = match doc.get_dictionary(root_id).cloned() {
        Ok(dict) => dict,
        Err(_) => return,
    };

    for key in [b"Metadata".as_slice(), b"PieceInfo", b"Thumb"] {
        if catalog.remove(key).is_some() {
            report.metadata += 1;
        }
    }
    // La struttura di accessibilità sparisce: il documento resta leggibile ma
    // non più navigabile da uno screen reader.
    for key in [b"StructTreeRoot".as_slice(), b"MarkInfo"] {
        if catalog.remove(key).is_some() {
            report.hidden += 1;
        }
    }
    if let Some(af) = catalog.remove(b"AF") {
        report.attachments += array_len(doc, &af);
    }
    if let Some(files) = catalog.remove(b"EmbeddedFiles") {
        report.attachments += array_len(doc, &files);
    }
    if catalog.remove(b"JavaScript").is_some() {
        report.scripts += 1;
    }

    let names = catalog.get(b"Names").ok().cloned();
    report.scripts += strip_js_actions(doc, &mut catalog);

    if let Ok(target) = doc.get_dictionary_mut(root_id) {
        *target = catalog;
    }

    if let Some(names) = names {
        sanitize_names(doc, root_id, names, report);
    }
}

fn sanitize_names(doc: &mut Document, root_id: ObjectId, names: Object, report: &mut SanitizeReport) {
    let (target_id, mut names) = match &names {
        Object::Dictionary(dict) => (None, dict.clone()),
        Object::Reference(id) => match doc.get_dictionary(*id).cloned() {
            Ok(dict) => (Some(*id), dict),
            Err(_) => return,
        },
        _ => return,
    };

    if let Some(tree) = names.remove(b"JavaScript") {
        report.scripts += name_tree_entries(doc, &tree);
    }
    if let Some(tree) = names.remove(b"EmbeddedFiles") {
        report.attachments += name_tree_entries(doc, &tree);
    }

    match target_id {
        Some(id) => {
            if let Ok(target) = doc.get_dictionary_mut(id) {
                *target = names;
            }
        }
        None => {
            if let Ok(catalog) = doc.get_dictionary_mut(root_id) {
                if names.is_empty() {
                    catalog.remove(b"Names");
                } else {
                    catalog.set("Names", Object::Dictionary(names));
                }
            }
        }
    }
}

fn sanitize_acroform(
    doc: &mut Document,
    root_id: ObjectId,
    removed_annots: &HashSet<ObjectId>,
    report: &mut SanitizeReport,
) {
    let Some(value) = doc
        .get_dictionary(root_id)
        .ok()
        .and_then(|catalog| catalog.get(b"AcroForm").ok().cloned())
    else {
        return;
    };

    let (target_id, mut acro) = match &value {
        Object::Dictionary(dict) => (None, dict.clone()),
        Object::Reference(id) => match doc.get_dictionary(*id).cloned() {
            Ok(dict) => (Some(*id), dict),
            Err(_) => return,
        },
        _ => return,
    };

    if acro.remove(b"XFA").is_some() {
        report.scripts += 1;
    }
    let fields = acro.get(b"Fields").ok().cloned();
    report.scripts += strip_js_actions(doc, &mut acro);

    let mut removed_fields = removed_annots.clone();
    match fields {
        Some(Object::Array(mut list)) => {
            clean_fields(doc, &mut list, &mut removed_fields, report);
            acro.set("Fields", Object::Array(list));
        }
        Some(Object::Reference(id)) => {
            let mut list = doc
                .get_object(id)
                .ok()
                .and_then(|object| object.as_array().ok())
                .cloned();
            if let Some(list) = list.as_mut() {
                clean_fields(doc, list, &mut removed_fields, report);
                if let Ok(Some(target)) = doc
                    .get_object_mut(id)
                    .map(|object| object.as_array_mut().ok())
                {
                    *target = list.clone();
                }
            }
        }
        _ => {}
    }

    // L'ordine di calcolo non deve puntare a campi eliminati.
    if let Some(Object::Array(mut order)) = acro.get(b"CO").ok().cloned() {
        order.retain(|item| match item {
            Object::Reference(id) => !removed_fields.contains(id),
            _ => true,
        });
        if order.is_empty() {
            acro.remove(b"CO");
        } else {
            acro.set("CO", Object::Array(order));
        }
    }

    match target_id {
        Some(id) => {
            if let Ok(target) = doc.get_dictionary_mut(id) {
                *target = acro;
            }
        }
        None => {
            if let Ok(catalog) = doc.get_dictionary_mut(root_id) {
                catalog.set("AcroForm", Object::Dictionary(acro));
            }
        }
    }
}

/// Pulisce ricorsivamente i campi modulo, eliminando i widget già rimossi.
fn clean_fields(
    doc: &mut Document,
    fields: &mut Vec<Object>,
    removed_fields: &mut HashSet<ObjectId>,
    report: &mut SanitizeReport,
) {
    fields.retain(|field| {
        if let Object::Reference(id) = field {
            if removed_fields.contains(id) {
                return false;
            }
        }
        // Un campo-unitore nascosto non è visibile nel modulo: va eliminato
        // anche quando la pagina non lo elenca in /Annots.
        let hidden = dict_of(doc, field)
            .map(|dict| annotation_hidden(&dict))
            .unwrap_or(false);
        if hidden {
            report.hidden += 1;
            if let Object::Reference(id) = field {
                removed_fields.insert(*id);
            }
        }
        !hidden
    });

    for index in 0..fields.len() {
        let field = fields[index].clone();
        let Some(mut dict) = dict_of(doc, &field) else {
            continue;
        };
        report.scripts += strip_js_actions(doc, &mut dict);

        if let Some(kids) = dict.get(b"Kids").ok().cloned() {
            match kids {
                Object::Array(mut list) => {
                    clean_fields(doc, &mut list, removed_fields, report);
                    dict.set("Kids", Object::Array(list));
                }
                Object::Reference(id) => {
                    let mut list = doc
                        .get_object(id)
                        .ok()
                        .and_then(|object| object.as_array().ok())
                        .cloned();
                    if let Some(list) = list.as_mut() {
                        clean_fields(doc, list, removed_fields, report);
                        if let Ok(Some(target)) = doc
                            .get_object_mut(id)
                            .map(|object| object.as_array_mut().ok())
                        {
                            *target = list.clone();
                        }
                    }
                }
                _ => {}
            }
        }

        match &field {
            Object::Reference(id) => {
                if let Ok(target) = doc.get_dictionary_mut(*id) {
                    *target = dict;
                }
            }
            Object::Dictionary(_) => fields[index] = Object::Dictionary(dict),
            _ => {}
        }
    }
}

/// Passata finale su tutti i dizionari: intercetta le azioni JavaScript che
/// non vivono nel catalogo, nelle pagine o nei campi (per esempio i segnalibri).
fn sweep_js_actions(doc: &mut Document, report: &mut SanitizeReport) {
    let ids: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter_map(|(id, object)| {
            let dict = match object {
                Object::Dictionary(dict) => dict,
                Object::Stream(stream) => &stream.dict,
                _ => return None,
            };
            (dict.has(b"A") || dict.has(b"AA") || dict.has(b"OpenAction")).then_some(*id)
        })
        .collect();

    for id in ids {
        let mut dict = match doc.get_object(id) {
            Ok(Object::Dictionary(dict)) => dict.clone(),
            Ok(Object::Stream(stream)) => stream.dict.clone(),
            _ => continue,
        };
        let removed = strip_js_actions(doc, &mut dict);
        if removed == 0 {
            continue;
        }
        report.scripts += removed;
        match doc.get_object_mut(id) {
            Ok(Object::Dictionary(target)) => *target = dict,
            Ok(Object::Stream(stream)) => stream.dict = dict,
            _ => {}
        }
    }
}

fn dict_of(doc: &Document, object: &Object) -> Option<Dictionary> {
    match deref(doc, object)? {
        Object::Dictionary(dict) => Some(dict.clone()),
        _ => None,
    }
}

fn annotation_hidden(dict: &Dictionary) -> bool {
    let flags = dict
        .get(b"F")
        .ok()
        .and_then(|value| value.as_i64().ok())
        .unwrap_or(0);
    if flags & 2 != 0 {
        return true;
    }
    dict.get(b"Hidden")
        .ok()
        .and_then(|value| value.as_bool().ok())
        .unwrap_or(false)
        || dict
            .get(b"F")
            .ok()
            .and_then(|value| value.as_bool().ok())
            .unwrap_or(false)
}

fn sanitize_page(
    doc: &mut Document,
    page_id: ObjectId,
    report: &mut SanitizeReport,
) -> Vec<ObjectId> {
    let mut page = match doc.get_dictionary(page_id).cloned() {
        Ok(dict) => dict,
        Err(_) => return Vec::new(),
    };

    for key in [b"Metadata".as_slice(), b"PieceInfo", b"Thumb"] {
        if page.remove(key).is_some() {
            report.metadata += 1;
        }
    }
    // Il riferimento alla struttura di accessibilità non ha più senso una
    // volta eliminato /StructTreeRoot.
    page.remove(b"StructParents");
    if let Some(af) = page.remove(b"AF") {
        report.attachments += array_len(doc, &af);
    }
    report.scripts += strip_js_actions(doc, &mut page);

    let annots = page.get(b"Annots").ok().cloned();
    if let Ok(target) = doc.get_dictionary_mut(page_id) {
        *target = page;
    }

    match annots {
        Some(value) => sanitize_annotations(doc, page_id, value, report),
        None => Vec::new(),
    }
}

fn sanitize_annotations(
    doc: &mut Document,
    page_id: ObjectId,
    annots: Object,
    report: &mut SanitizeReport,
) -> Vec<ObjectId> {
    let list = match &annots {
        Object::Array(items) => items.clone(),
        Object::Reference(id) => doc
            .get_object(*id)
            .ok()
            .and_then(|object| object.as_array().ok())
            .cloned()
            .unwrap_or_default(),
        _ => return Vec::new(),
    };

    // Prima passata: allegati e annotazioni nascoste. I popup si decidono dopo,
    // perché un popup è orfano anche quando la sua annotazione madre viene
    // rimossa per altri motivi.
    let mut remove: HashSet<usize> = HashSet::new();
    for (index, annotation) in list.iter().enumerate() {
        let Some(dict) = dict_of(doc, annotation) else {
            continue;
        };
        let subtype = dict_key_name(&dict, b"Subtype");
        if subtype.as_deref() == Some("FileAttachment") {
            report.attachments += 1;
            remove.insert(index);
        } else if annotation_hidden(&dict) {
            report.hidden += 1;
            remove.insert(index);
        }
    }

    // I popup sono raggiungibili solo tramite /Popup di un'annotazione superstite.
    let mut referenced_popups: HashSet<ObjectId> = HashSet::new();
    for (index, annotation) in list.iter().enumerate() {
        if remove.contains(&index) {
            continue;
        }
        let Some(dict) = dict_of(doc, annotation) else {
            continue;
        };
        if dict_key_name(&dict, b"Subtype").as_deref() == Some("Popup") {
            continue;
        }
        if let Ok(Object::Reference(id)) = dict.get(b"Popup") {
            referenced_popups.insert(*id);
        }
    }
    for (index, annotation) in list.iter().enumerate() {
        if remove.contains(&index) {
            continue;
        }
        let Some(dict) = dict_of(doc, annotation) else {
            continue;
        };
        if dict_key_name(&dict, b"Subtype").as_deref() != Some("Popup") {
            continue;
        }
        let referenced = matches!(annotation, Object::Reference(id) if referenced_popups.contains(id));
        if !referenced {
            report.hidden += 1;
            remove.insert(index);
        }
    }

    let removed_ids: Vec<ObjectId> = list
        .iter()
        .enumerate()
        .filter(|(index, _)| remove.contains(index))
        .filter_map(|(_, annotation)| annotation.as_reference().ok())
        .collect();
    let removed_set: HashSet<ObjectId> = removed_ids.iter().copied().collect();

    let mut keep = Vec::with_capacity(list.len().saturating_sub(remove.len()));
    for (index, annotation) in list.into_iter().enumerate() {
        if remove.contains(&index) {
            continue;
        }
        let Some(mut dict) = dict_of(doc, &annotation) else {
            keep.push(annotation);
            continue;
        };

        report.scripts += strip_js_actions(doc, &mut dict);

        // Un'annotazione superstite non deve puntare a un popup o a una
        // risposta eliminati: il riferimento li terrebbe in vita nel file.
        for key in [b"Popup".as_slice(), b"IRT"] {
            if let Ok(Object::Reference(target)) = dict.get(key) {
                if removed_set.contains(target) {
                    dict.remove(key);
                }
            }
        }

        match &annotation {
            Object::Reference(id) => {
                if let Ok(target) = doc.get_dictionary_mut(*id) {
                    *target = dict;
                }
                keep.push(annotation);
            }
            Object::Dictionary(_) => keep.push(Object::Dictionary(dict)),
            _ => keep.push(annotation),
        }
    }

    if let Ok(page) = doc.get_dictionary_mut(page_id) {
        if keep.is_empty() {
            page.remove(b"Annots");
        } else {
            page.set("Annots", Object::Array(keep));
        }
    }
    removed_ids
}

fn is_dct(stream: &Stream) -> bool {
    match stream.dict.get(b"Filter") {
        Ok(Object::Name(name)) => name.as_slice() == b"DCTDecode",
        Ok(Object::Array(filters)) => filters
            .iter()
            .any(|filter| name_of(filter).as_deref() == Some("DCTDecode")),
        _ => false,
    }
}

/// Riscrive un flusso JPEG senza segmenti APPn e COM, lasciando intatti i
/// dati entropici: i pixel non vengono ricodificati.
fn strip_jpeg_markers(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }

    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(&data[..2]);
    let mut cursor = 2usize;
    let mut removed = 0usize;

    while cursor < data.len() {
        if data[cursor] != 0xFF {
            return None;
        }
        let mut marker_pos = cursor;
        while marker_pos < data.len() && data[marker_pos] == 0xFF {
            marker_pos += 1;
        }
        if marker_pos >= data.len() {
            return None;
        }
        let marker = data[marker_pos];

        // Da SOS in poi ci sono i dati dell'immagine: si copia il resto.
        if marker == 0xDA {
            out.extend_from_slice(&data[cursor..]);
            return (removed > 0).then_some(out);
        }

        // Marker senza lunghezza (TEM, RSTn, SOI, EOI).
        if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            out.extend_from_slice(&data[cursor..=marker_pos]);
            cursor = marker_pos + 1;
            continue;
        }

        if marker_pos + 2 >= data.len() {
            return None;
        }
        let length = u16::from_be_bytes([data[marker_pos + 1], data[marker_pos + 2]]) as usize;
        if length < 2 {
            return None;
        }
        let end = marker_pos + 1 + length;
        if end > data.len() {
            return None;
        }

        if (0xE0..=0xEF).contains(&marker) || marker == 0xFE {
            removed += 1;
        } else {
            out.extend_from_slice(&data[cursor..end]);
        }
        cursor = end;
    }

    None
}

fn sanitize_images(doc: &mut Document, report: &mut SanitizeReport) {
    for object in doc.objects.values_mut() {
        let Object::Stream(stream) = object else {
            continue;
        };
        if dict_key_name(&stream.dict, b"Subtype").as_deref() != Some("Image") {
            continue;
        }

        let mut cleaned = false;
        for key in [b"Metadata".as_slice(), b"PieceInfo", b"Alternates"] {
            if stream.dict.remove(key).is_some() {
                cleaned = true;
            }
        }
        stream.dict.remove(b"StructParent");

        if is_dct(stream) {
            if let Some(stripped) = strip_jpeg_markers(&stream.content) {
                stream.set_content(stripped);
                cleaned = true;
            }
        }

        if cleaned {
            report.images += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::{dictionary, Stream};

    fn stream_body(dict: &str, data: &[u8]) -> Vec<u8> {
        let mut body = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        body
    }

    fn build_pdf(objects: &[Vec<u8>], info: Option<u32>) -> Vec<u8> {
        let mut file = b"%PDF-1.5\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(file.len());
            file.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            file.extend_from_slice(body);
            file.extend_from_slice(b"\nendobj\n");
        }

        let xref = file.len();
        file.extend_from_slice(
            format!(
                "xref\n0 {}\n0000000000 65535 f \n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        for offset in &offsets {
            file.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        let info_part = info
            .map(|id| format!(" /Info {id} 0 R"))
            .unwrap_or_default();
        file.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R{info_part} >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        file
    }

    fn hidden_pdf() -> Vec<u8> {
        let content = b"BT /F1 12 Tf 72 700 Td (VISIBILE) Tj ET\n";
        let objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R /Metadata 6 0 R /OpenAction 7 0 R /Names << /JavaScript 8 0 R /EmbeddedFiles 9 0 R >> /AcroForm 10 0 R /StructTreeRoot 11 0 R /MarkInfo << /Marked true >> /AF [12 0 R] >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R /Annots [14 0 R 15 0 R] >>".to_vec(),
            stream_body("", content),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /MissingWidth 500 >>".to_vec(),
            stream_body(
                "/Type /Metadata /Subtype /XML",
                b"<?xpacket begin=''?><x:xmpmeta>XMPSEGRETO</x:xmpmeta><?xpacket end='w'?>",
            ),
            b"<< /S /JavaScript /JS (app.alert('JSSEGRETO')) >>".to_vec(),
            b"<< /Names [(JSTREESEGRETO) 7 0 R] >>".to_vec(),
            b"<< /Names [(ALLEGATOSEGRETO.txt) 12 0 R] >>".to_vec(),
            b"<< /Fields [18 0 R] /XFA 16 0 R >>".to_vec(),
            b"<< /Type /StructTreeRoot /K [] >>".to_vec(),
            b"<< /Type /Filespec /F (allegato.txt) /UF (ALLEGATOSEGRETO.txt) /EF << /F 13 0 R >> >>".to_vec(),
            stream_body("/Type /EmbeddedFile", b"PAYLOADSEGRETO"),
            b"<< /Type /Annot /Subtype /Text /Rect [10 10 20 20] /F 2 /Contents (NASCUSTO) >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [30 30 40 40] /F 4 /Contents (VISIBILE) >>".to_vec(),
            stream_body("/Type /XFA /Subtype /XML", b"<xfa>XFASEGRETO</xfa>"),
            b"<< /Title (TitoloSegreto) /Author (AutoreSegreto) /Producer (Open PDF) >>".to_vec(),
            b"<< /FT /Tx /T (campo) /AA << /K << /S /JavaScript /JS (app.alert('CAMPOJS')) >> >> >>".to_vec(),
        ];
        build_pdf(&objects, Some(17))
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(name)
    }

    #[test]
    fn removes_metadata_scripts_and_attachments() {
        let src = temp_path("openpdf-sanitize-hidden-src.pdf");
        let dest = temp_path("openpdf-sanitize-hidden-dest.pdf");
        std::fs::write(&src, hidden_pdf()).unwrap();

        let report = sanitize_pdf(&src, &dest, None).unwrap();
        assert!(report.metadata >= 2, "metadati non rimossi: {report:?}");
        assert!(report.scripts >= 3, "script non rimossi: {report:?}");
        assert!(report.attachments >= 1, "allegati non rimossi: {report:?}");
        assert!(report.hidden >= 3, "dati nascosti non rimossi: {report:?}");
        assert!(report.pruned > 0, "oggetti orfani non eliminati: {report:?}");

        let bytes = std::fs::read(&dest).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        for marker in [
            "XMPSEGRETO",
            "JSSEGRETO",
            "JSTREESEGRETO",
            "ALLEGATOSEGRETO",
            "PAYLOADSEGRETO",
            "XFASEGRETO",
            "AutoreSegreto",
            "CAMPOJS",
        ] {
            assert!(!text.contains(marker), "marcatore ancora presente: {marker}");
        }

        let doc = Document::load(&dest).unwrap();
        assert_eq!(doc.get_pages().len(), 1);
        let extracted = doc.extract_text(&[1]).unwrap();
        assert!(
            extracted.contains("VISIBILE"),
            "contenuto visibile perso: {extracted:?}"
        );

        let page_id = *doc.get_pages().get(&1).unwrap();
        let annots = doc
            .get_dictionary(page_id)
            .unwrap()
            .get(b"Annots")
            .and_then(Object::as_array)
            .unwrap();
        assert_eq!(annots.len(), 1, "annotazione nascosta non rimossa");
    }

    #[test]
    fn strips_exif_from_a_dct_image() {
        let width = 32u32;
        let height = 24u32;
        let mut raw = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                raw.push((x * 7) as u8);
                raw.push((y * 9) as u8);
                raw.push(64u8);
            }
        }
        let picture = image::RgbImage::from_raw(width, height, raw).unwrap();
        let mut jpeg = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90);
        encoder
            .encode(
                picture.as_raw(),
                width,
                height,
                image::ExtendedColorType::Rgb8,
            )
            .unwrap();

        // Inserisce un APP1 "Exif" subito dopo SOI.
        let payload = b"Exif\x00\x00MM\x00*\x00\x00\x00\x08\x00\x00\x00\x00";
        let mut with_exif = Vec::new();
        with_exif.extend_from_slice(&jpeg[..2]);
        with_exif.push(0xFF);
        with_exif.push(0xE1);
        with_exif.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
        with_exif.extend_from_slice(payload);
        with_exif.extend_from_slice(&jpeg[2..]);

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let image_id = {
            let mut dict = Dictionary::new();
            dict.set("Type", "XObject");
            dict.set("Subtype", "Image");
            dict.set("Width", width as i64);
            dict.set("Height", height as i64);
            dict.set("ColorSpace", "DeviceRGB");
            dict.set("BitsPerComponent", 8i64);
            dict.set("Filter", "DCTDecode");
            let stream = Stream::new(dict, with_exif).with_compression(false);
            doc.add_object(stream)
        };
        let content_id = doc.add_object(Stream::new(
            Dictionary::new(),
            b"q 32 0 0 24 10 10 cm /Im1 Do Q\n".to_vec(),
        ));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => Object::Reference(pages_id),
            "MediaBox" => Object::Array(vec![0.into(), 0.into(), 100.into(), 100.into()]),
            "Resources" => dictionary! { "XObject" => dictionary! { "Im1" => Object::Reference(image_id) } },
            "Contents" => Object::Reference(content_id),
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => Object::Array(vec![Object::Reference(page_id)]),
                "Count" => 1i64,
            }),
        );
        let catalog_id = doc.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => Object::Reference(pages_id),
        });
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let src = temp_path("openpdf-sanitize-exif-src.pdf");
        let dest = temp_path("openpdf-sanitize-exif-dest.pdf");
        doc.save(&src).unwrap();

        let report = sanitize_pdf(&src, &dest, None).unwrap();
        assert_eq!(report.images, 1, "immagine non ripulita: {report:?}");

        let cleaned = Document::load(&dest).unwrap();
        let image = cleaned
            .objects
            .values()
            .filter_map(|object| object.as_stream().ok())
            .find(|stream| dict_key_name(&stream.dict, b"Subtype").as_deref() == Some("Image"))
            .expect("immagine assente dopo la pulizia");
        assert!(
            !image
                .content
                .windows(4)
                .any(|window| window == b"Exif"),
            "segmento EXIF ancora presente"
        );
        assert_eq!(&image.content[..2], &[0xFF, 0xD8]);
        let decoded = image::load_from_memory(&image.content).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (width, height));
    }

    fn annotations_pdf() -> Vec<u8> {
        let content = b"BT /F1 12 Tf 72 700 Td (VISIBILE) Tj ET\n";
        let objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R /Annots [6 0 R 7 0 R 8 0 R] >>".to_vec(),
            stream_body("", content),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /MissingWidth 500 >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [10 10 20 20] /F 2 /Contents (NASCUSTO) >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [30 30 40 40] /F 4 /Contents (VISIBILE) >>".to_vec(),
            b"<< /Type /Annot /Subtype /Popup /Rect [50 50 60 60] /Contents (POPUPORFANO) >>".to_vec(),
        ];
        build_pdf(&objects, None)
    }

    #[test]
    fn removes_hidden_and_orphan_annotations_only() {
        let src = temp_path("openpdf-sanitize-annots-src.pdf");
        let dest = temp_path("openpdf-sanitize-annots-dest.pdf");
        std::fs::write(&src, annotations_pdf()).unwrap();

        let report = sanitize_pdf(&src, &dest, None).unwrap();
        assert_eq!(report.hidden, 2, "conteggio dati nascosti errato: {report:?}");

        let doc = Document::load(&dest).unwrap();
        let page_id = *doc.get_pages().get(&1).unwrap();
        let annots = doc
            .get_dictionary(page_id)
            .unwrap()
            .get(b"Annots")
            .and_then(Object::as_array)
            .unwrap();
        assert_eq!(annots.len(), 1, "attese una sola annotazione visibile");
        let kept = annots[0].as_reference().unwrap();
        let dict = doc.get_dictionary(kept).unwrap();
        assert_eq!(
            dict.get(b"Contents").and_then(Object::as_str).unwrap(),
            b"VISIBILE"
        );
        let bytes = std::fs::read(&dest).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains("POPUPORFANO"),
            "popup orfano ancora presente"
        );
    }

    #[test]
    fn a_clean_document_reports_no_changes() {
        let content = b"BT /F1 12 Tf 72 700 Td (PULITO) Tj ET\n";
        let objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
            stream_body("", content),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /MissingWidth 500 >>".to_vec(),
        ];
        let src = temp_path("openpdf-sanitize-clean-src.pdf");
        let dest = temp_path("openpdf-sanitize-clean-dest.pdf");
        std::fs::write(&src, build_pdf(&objects, None)).unwrap();

        let report = sanitize_pdf(&src, &dest, None).unwrap();
        assert_eq!(report.metadata, 0, "metadati rimossi: {report:?}");
        assert_eq!(report.scripts, 0, "script rimossi: {report:?}");
        assert_eq!(report.attachments, 0, "allegati rimossi: {report:?}");
        assert_eq!(report.images, 0, "immagini rimossi: {report:?}");
        assert_eq!(report.hidden, 0, "dati nascosti rimossi: {report:?}");
        assert_eq!(report.pruned, 0, "oggetti eliminati: {report:?}");

        let doc = Document::load(&dest).unwrap();
        assert_eq!(doc.get_pages().len(), 1);
        let extracted = doc.extract_text(&[1]).unwrap();
        assert!(extracted.contains("PULITO"), "contenuto perso: {extracted:?}");
    }
}
