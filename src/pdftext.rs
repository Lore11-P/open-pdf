//! Modifica del testo esistente nel content stream.
//!
//! A differenza di `textedit::delete_lines` (che rimuove oggetti pdfium e
//! rigenera la pagina), qui il testo viene sostituito davvero: si individua
//! l'operazione che disegna la riga, la si elimina e si inserisce una nuova
//! operazione di testo nella stessa posizione, con lo stesso corpo, colore e
//! matrice. Il risultato è testo PDF reale, selezionabile e ricercabile, non
//! un'annotazione sovrapposta.
//!
//! Scelte dichiarate:
//! - si riusa il font originale solo se è uno dei font standard (Helvetica,
//!   Arial, Times, Courier, Symbol): i font incorporati e sottoinsiemati
//!   potrebbero non contenere i glifi nuovi, quindi si passa a Helvetica;
//! - il testo è codificato in WinAnsi: i caratteri fuori da Latin-1 e dalla
//!   punteggiatura tipografica vengono rifiutati con un messaggio;
//! - si conservano `Tc` (spaziatura caratteri), `Tw` (spaziatura parole) e
//!   `Tz` (scala orizzontale) del run originale; se il nuovo testo viene
//!   ridotto per adattarsi alla larghezza, la scala di adattamento viene
//!   moltiplicata per quella originale;
//! - le righe successive alla prima usano il leading originale (`TL`, o
//!   quello implicito di `TD`/`'`/`"`); se manca si usa 1,2 × corpo. Le righe
//!   che eccedono l'altezza della riga originale sono dichiarate in
//!   [`EditReport::overflow_lines`];
//! - se nessun run della pagina interseca l'area si cercano i Form XObject
//!   (fino a 3 livelli di annidamento). Per modificare una singola istanza si
//!   clona la catena dei Form e si rinomina solo l'operazione `Do`
//!   interessata: le altre copie restano identiche. I font aggiunti finiscono
//!   nelle risorse del Form clonato, non della pagina;
//! - l'allineamento è sempre a sinistra: il blocco parte dalla matrice del run
//!   originale.
//!
//! Limiti residui:
//! - `lopdf::Document::extract_text` non attraversa i Form XObject: dopo una
//!   modifica dentro un Form il testo si vede con pdfium (usato dall'app) ma
//!   non con quell'estrattore. I test verificano il Form sia leggendone il
//!   content stream sia con pdfium.
//! - una selezione che copre più righe viene sostituita da un unico blocco,
//!   allineato alla prima operazione trovata;
//! - i run con font Type0 (CID) non vengono riusati: si passa a Helvetica,
//!   quindi il nuovo testo è Latin-1;
//! - il testo ruotato o inclinato viene misurato sull'ingombro assiale del
//!   run: l'adattamento orizzontale può essere meno preciso.

use crate::redact::{
    apply, concat, content_bytes, deref, dict_of, fonts_for, invert, media_box, name_of,
    norm_to_user, number, page_content_ids, page_resources, page_rotation, text_advance,
    text_bbox, FontMap, Matrix, Rect, TextState, IDENTITY,
};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::HashSet;
use std::path::Path;

/// Livelli massimi di Form XObject attraversati (la pagina non conta).
const MAX_FORM_DEPTH: usize = 3;
/// Corpo di ripiego per run senza `Tf` valido.
const DEFAULT_FONT_SIZE: f64 = 12.0;
/// Riduzione orizzontale minima applicata per far entrare il testo.
const MIN_H_SCALE: f64 = 0.5;

#[derive(Debug, Clone, serde::Serialize)]
pub struct EditReport {
    /// Numero di caratteri contenuti nelle operazioni sostituite.
    pub replaced_chars: usize,
    /// Numero di caratteri del nuovo testo (a capo compresi).
    pub inserted_chars: usize,
    /// Nome risorsa del font usato nel nuovo blocco.
    pub font: String,
    /// `true` se il testo è stato ridotto con `Tz` per stare nella riga.
    pub fitted: bool,
    /// Righe che eccedono l'altezza della riga originale (0 = nessuna).
    pub overflow_lines: usize,
    /// 0 = content stream della pagina, 1-3 = Form XObject annidato.
    pub form_depth: usize,
    /// `true` se il Form modificato è stato clonato per isolare l'istanza.
    pub cloned_form: bool,
}

#[derive(Clone)]
struct EditState {
    ctm: Matrix,
    text: TextState,
    color: [f64; 3],
}

impl Default for EditState {
    fn default() -> Self {
        EditState {
            ctm: IDENTITY,
            text: TextState::default(),
            color: [0.0, 0.0, 0.0],
        }
    }
}

/// Un'operazione che disegna testo, con lo stato in cui viene eseguita.
#[derive(Clone)]
struct Run {
    index: usize,
    bbox: Rect,
    tm: Matrix,
    tlm: Matrix,
    ctm: Matrix,
    font: Option<String>,
    font_size: f64,
    color: [f64; 3],
    char_spacing: f64,
    word_spacing: f64,
    h_scale: f64,
    leading: f64,
    /// Caratteri disegnati dall'operazione (per il report).
    chars: usize,
    /// `true` se l'operazione sta dentro un `BT ... ET`.
    in_text: bool,
}

/// Un'occorrenza di `Do` che richiama un Form XObject.
#[derive(Clone)]
struct FormUse {
    index: usize,
    name: String,
    ctm: Matrix,
}

/// Un passo della catena pagina -> Form -> ... -> Form modificato.
#[derive(Clone)]
struct FormStep {
    /// Nome con cui il Form è richiamato nel contenitore.
    name: String,
    /// Indice dell'operazione `Do` nel contenuto del contenitore.
    op_index: usize,
    /// Identificativo del Form richiamato.
    target_id: ObjectId,
}

struct FormHit {
    chain: Vec<FormStep>,
    hits: Vec<Run>,
    resources: Dictionary,
    fonts: FontMap,
}

struct TextPlan {
    operations: Vec<Operation>,
    font: String,
    fitted: bool,
    overflow_lines: usize,
}

/// Analizza il content stream raccogliendo run di testo e richiami a Form.
fn scan_content(ops: &[Operation], fonts: &FontMap) -> (Vec<Run>, Vec<FormUse>) {
    let mut runs = Vec::new();
    let mut forms = Vec::new();
    let mut stack: Vec<EditState> = Vec::new();
    let mut state = EditState::default();
    let mut tm = IDENTITY;
    let mut tlm = IDENTITY;
    let mut in_text = false;

    for (index, op) in ops.iter().enumerate() {
        match op.operator.as_str() {
            "q" => stack.push(state.clone()),
            "Q" => {
                if let Some(previous) = stack.pop() {
                    state = previous;
                }
            }
            "cm" => {
                let values: Vec<f64> = op.operands.iter().filter_map(number).collect();
                if values.len() == 6 {
                    state.ctm = concat(
                        &[
                            values[0], values[1], values[2], values[3], values[4], values[5],
                        ],
                        &state.ctm,
                    );
                }
            }
            "rg" => {
                let values: Vec<f64> = op.operands.iter().filter_map(number).collect();
                if values.len() >= 3 {
                    state.color = [values[0], values[1], values[2]];
                }
            }
            "g" => {
                if let Some(value) = op.operands.first().and_then(number) {
                    state.color = [value, value, value];
                }
            }
            "k" => {
                let values: Vec<f64> = op.operands.iter().filter_map(number).collect();
                if values.len() >= 4 {
                    let (c, m, y, k) = (values[0], values[1], values[2], values[3]);
                    state.color = [
                        (1.0 - c) * (1.0 - k),
                        (1.0 - m) * (1.0 - k),
                        (1.0 - y) * (1.0 - k),
                    ];
                }
            }
            "BT" => {
                tm = IDENTITY;
                tlm = IDENTITY;
                in_text = true;
            }
            "ET" => in_text = false,
            "Tf" => {
                if let Some(name) = op.operands.first().and_then(name_of) {
                    state.text.font = Some(name);
                }
                if let Some(size) = op.operands.get(1).and_then(number) {
                    state.text.font_size = size;
                }
            }
            "TL" => {
                if let Some(value) = op.operands.first().and_then(number) {
                    state.text.leading = value;
                }
            }
            "Tc" => {
                if let Some(value) = op.operands.first().and_then(number) {
                    state.text.char_spacing = value;
                }
            }
            "Tw" => {
                if let Some(value) = op.operands.first().and_then(number) {
                    state.text.word_spacing = value;
                }
            }
            "Tz" => {
                if let Some(value) = op.operands.first().and_then(number) {
                    state.text.h_scale = value / 100.0;
                }
            }
            "Tm" => {
                let values: Vec<f64> = op.operands.iter().filter_map(number).collect();
                if values.len() == 6 {
                    tm = [
                        values[0], values[1], values[2], values[3], values[4], values[5],
                    ];
                    tlm = tm;
                }
            }
            "Td" | "TD" => {
                let tx = op.operands.first().and_then(number).unwrap_or(0.0);
                let ty = op.operands.get(1).and_then(number).unwrap_or(0.0);
                tlm = concat(&[1.0, 0.0, 0.0, 1.0, tx, ty], &tlm);
                tm = tlm;
                if op.operator == "TD" {
                    state.text.leading = -ty;
                }
            }
            "T*" => {
                tlm = concat(&[1.0, 0.0, 0.0, 1.0, 0.0, -state.text.leading], &tlm);
                tm = tlm;
            }
            "Tj" | "TJ" | "'" | "\"" => {
                if op.operator == "\"" {
                    // aw Tw ac Tc: gli spazi valgono per questa sola stringa.
                    if let Some(value) = op.operands.first().and_then(number) {
                        state.text.word_spacing = value;
                    }
                    if let Some(value) = op.operands.get(1).and_then(number) {
                        state.text.char_spacing = value;
                    }
                }
                if op.operator == "'" || op.operator == "\"" {
                    tlm = concat(&[1.0, 0.0, 0.0, 1.0, 0.0, -state.text.leading], &tlm);
                    tm = tlm;
                }
                let operand = if op.operator == "\"" {
                    op.operands.get(2)
                } else {
                    op.operands.first()
                };
                let Some(operand) = operand else {
                    continue;
                };
                let (width, chars) = text_advance(operand, &state.text, fonts);
                if let Some(bbox) = text_bbox(&tm, &state.ctm, width, state.text.font_size) {
                    runs.push(Run {
                        index,
                        bbox,
                        tm,
                        tlm,
                        ctm: state.ctm,
                        font: state.text.font.clone(),
                        font_size: state.text.font_size,
                        color: state.color,
                        char_spacing: state.text.char_spacing,
                        word_spacing: state.text.word_spacing,
                        h_scale: state.text.h_scale,
                        leading: state.text.leading,
                        chars,
                        in_text,
                    });
                }
                tm = concat(&[1.0, 0.0, 0.0, 1.0, width, 0.0], &tm);
            }
            "Do" => {
                if let Some(name) = op.operands.first().and_then(name_of) {
                    forms.push(FormUse {
                        index,
                        name,
                        ctm: state.ctm,
                    });
                }
            }
            _ => {}
        }
    }
    (runs, forms)
}

fn encode_winansi(text: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len());
    for ch in text.chars() {
        let code = ch as u32;
        let byte = match code {
            0x20..=0x7E => code as u8,
            0xA0..=0xFF => code as u8,
            0x20AC => 0x80,
            0x201A => 0x82,
            0x0192 => 0x83,
            0x201E => 0x84,
            0x2026 => 0x85,
            0x2020 => 0x86,
            0x2021 => 0x87,
            0x02C6 => 0x88,
            0x2030 => 0x89,
            0x0160 => 0x8A,
            0x2039 => 0x8B,
            0x0152 => 0x8C,
            0x017D => 0x8E,
            0x2018 => 0x91,
            0x2019 => 0x92,
            0x201C => 0x93,
            0x201D => 0x94,
            0x2022 => 0x95,
            0x2013 => 0x96,
            0x2014 => 0x97,
            0x02DC => 0x98,
            0x2122 => 0x99,
            0x0161 => 0x9A,
            0x203A => 0x9B,
            0x0153 => 0x9C,
            0x017E => 0x9E,
            0x0178 => 0x9F,
            _ => {
                return Err(format!(
                    "Il carattere «{ch}» non è rappresentabile con la codifica WinAnsi usata per la modifica."
                ))
            }
        };
        out.push(byte);
    }
    Ok(out)
}

fn helvetica_width(code: u8) -> f64 {
    const WIDTHS: [u16; 95] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556,
        556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667,
        667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722,
        667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500,
        556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278,
        556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
    ];
    if (32..=126).contains(&code) {
        WIDTHS[(code - 32) as usize] as f64
    } else {
        556.0
    }
}

/// Avanzamento con la width table di Helvetica, con `Tc`, `Tw` e `Tz`.
fn helvetica_advance(
    bytes: &[u8],
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    h_scale: f64,
) -> f64 {
    let mut width = 0.0;
    for code in bytes {
        width += helvetica_width(*code) / 1000.0 * size * h_scale + char_spacing;
        if *code == b' ' {
            width += word_spacing * h_scale;
        }
    }
    width.max(0.0)
}

fn number_object(value: f64) -> Object {
    Object::Real(value as f32)
}

/// Stile con cui viene riscritto il blocco di testo.
struct TextBlockStyle<'a> {
    font: &'a str,
    size: f64,
    tm: &'a Matrix,
    tlm: &'a Matrix,
    color: [f64; 3],
    char_spacing: f64,
    word_spacing: f64,
    h_scale: f64,
    leading: f64,
    in_text: bool,
}

/// Costruisce le operazioni del nuovo blocco.
///
/// Se il run originale stava dentro un `BT ... ET` non si apre un secondo text
/// object (la specifica vieta l'annidamento): si sfrutta quello corrente e, in
/// coda, si ripristinano `Tm`/`Tlm` con `Tm` + `Td`, perché `q`/`Q` non li
/// salvano.
fn text_block(style: &TextBlockStyle, lines: &[Vec<u8>]) -> Vec<Operation> {
    let mut ops = Vec::with_capacity(16 + lines.len() * 2);
    ops.push(Operation::new("q", vec![]));
    ops.push(Operation::new(
        "rg",
        vec![
            number_object(style.color[0]),
            number_object(style.color[1]),
            number_object(style.color[2]),
        ],
    ));
    if !style.in_text {
        ops.push(Operation::new("BT", vec![]));
    }
    ops.push(Operation::new(
        "Tf",
        vec![
            Object::Name(style.font.as_bytes().to_vec()),
            number_object(style.size),
        ],
    ));
    if style.char_spacing.abs() > 1e-9 {
        ops.push(Operation::new(
            "Tc",
            vec![number_object(style.char_spacing)],
        ));
    }
    if style.word_spacing.abs() > 1e-9 {
        ops.push(Operation::new(
            "Tw",
            vec![number_object(style.word_spacing)],
        ));
    }
    if (style.h_scale - 1.0).abs() > 1e-4 {
        ops.push(Operation::new(
            "Tz",
            vec![number_object(style.h_scale * 100.0)],
        ));
    }
    ops.push(Operation::new(
        "Tm",
        vec![
            number_object(style.tm[0]),
            number_object(style.tm[1]),
            number_object(style.tm[2]),
            number_object(style.tm[3]),
            number_object(style.tm[4]),
            number_object(style.tm[5]),
        ],
    ));
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            ops.push(Operation::new(
                "Td",
                vec![number_object(0.0), number_object(-style.leading)],
            ));
        }
        ops.push(Operation::new(
            "Tj",
            vec![Object::String(line.clone(), lopdf::StringFormat::Literal)],
        ));
    }
    if !style.in_text {
        ops.push(Operation::new("ET", vec![]));
    }
    ops.push(Operation::new("Q", vec![]));

    if style.in_text {
        // Ripristina la matrice di testo lasciata dall'operazione originale.
        ops.push(Operation::new(
            "Tm",
            vec![
                number_object(style.tlm[0]),
                number_object(style.tlm[1]),
                number_object(style.tlm[2]),
                number_object(style.tlm[3]),
                number_object(style.tlm[4]),
                number_object(style.tlm[5]),
            ],
        ));
        let det = style.tlm[0] * style.tlm[3] - style.tlm[1] * style.tlm[2];
        if det.abs() > 1e-9 {
            let de = style.tm[4] - style.tlm[4];
            let df = style.tm[5] - style.tlm[5];
            let tx = (de * style.tlm[3] - df * style.tlm[2]) / det;
            let ty = (df * style.tlm[0] - de * style.tlm[1]) / det;
            if tx.abs() > 1e-6 || ty.abs() > 1e-6 {
                ops.push(Operation::new(
                    "Td",
                    vec![number_object(tx), number_object(ty)],
                ));
            }
        }
    }

    ops
}

fn mapped_rect(rect: &Rect, matrix: &Matrix) -> Option<Rect> {
    Rect::from_points(&[
        apply(matrix, rect.x0, rect.y0),
        apply(matrix, rect.x1, rect.y0),
        apply(matrix, rect.x0, rect.y1),
        apply(matrix, rect.x1, rect.y1),
    ])
}

fn xobject_id(
    doc: &Document,
    resources: Option<&Dictionary>,
    name: &str,
) -> Option<ObjectId> {
    resources
        .and_then(|r| r.get(b"XObject").ok())
        .and_then(|x| dict_of(doc, x))
        .and_then(|x| x.get(name.as_bytes()).ok())
        .and_then(|v| v.as_reference().ok())
}

fn form_stream(doc: &Document, id: ObjectId) -> Option<Stream> {
    let stream = doc.get_object(id).ok()?.as_stream().ok()?;
    let subtype = stream
        .dict
        .get(b"Subtype")
        .ok()
        .and_then(name_of)
        .unwrap_or_default();
    if subtype == "Form" {
        Some(stream.clone())
    } else {
        None
    }
}

fn form_matrix(doc: &Document, stream: &Stream) -> Matrix {
    let Some(entry) = stream.dict.get(b"Matrix").ok() else {
        return IDENTITY;
    };
    let Some(Object::Array(values)) = deref(doc, entry) else {
        return IDENTITY;
    };
    let numbers: Vec<f64> = values.iter().filter_map(number).collect();
    if numbers.len() == 6 {
        [
            numbers[0], numbers[1], numbers[2], numbers[3], numbers[4], numbers[5],
        ]
    } else {
        IDENTITY
    }
}

/// Cerca il testo dentro `form_id`, con l'area già mappata nello spazio del Form.
fn find_in_form(
    doc: &Document,
    page_id: ObjectId,
    form_id: ObjectId,
    area: &Rect,
    chain: Vec<FormStep>,
    depth: usize,
) -> Option<FormHit> {
    let stream = form_stream(doc, form_id)?;
    let data = stream.decompressed_content().ok()?;
    let content = Content::decode(&data).ok()?;
    let resources = stream
        .dict
        .get(b"Resources")
        .ok()
        .and_then(|value| dict_of(doc, value))
        .or_else(|| page_resources(doc, page_id));
    let fonts = scan_fonts(doc, resources);
    let (runs, uses) = scan_content(&content.operations, &fonts);
    let hits: Vec<Run> = runs
        .iter()
        .filter(|run| run.bbox.intersects(area))
        .cloned()
        .collect();
    if !hits.is_empty() {
        return Some(FormHit {
            chain,
            hits,
            resources: resources.cloned().unwrap_or_default(),
            fonts,
        });
    }

    if depth >= MAX_FORM_DEPTH {
        return None;
    }

    for usage in uses {
        let Some(child_id) = xobject_id(doc, resources, &usage.name) else {
            continue;
        };
        let Some(child) = form_stream(doc, child_id) else {
            continue;
        };
        let matrix = form_matrix(doc, &child);
        let effective = concat(&matrix, &usage.ctm);
        let Some(inverse) = invert(&effective) else {
            continue;
        };
        let Some(mapped) = mapped_rect(area, &inverse) else {
            continue;
        };
        let mut nested = chain.clone();
        nested.push(FormStep {
            name: usage.name.clone(),
            op_index: usage.index,
            target_id: child_id,
        });
        if let Some(hit) = find_in_form(doc, page_id, child_id, &mapped, nested, depth + 1) {
            return Some(hit);
        }
    }
    None
}

/// Copia un sotto-dizionario (Font, XObject) per non condividere le modifiche.
fn clone_subdict(doc: &mut Document, parent: &mut Dictionary, key: &[u8]) {
    let base = parent
        .get(key)
        .ok()
        .and_then(|value| dict_of(doc, value))
        .cloned();
    if let Some(base) = base {
        let id = doc.add_object(Object::Dictionary(base));
        parent.set(key.to_vec(), Object::Reference(id));
    }
}

/// Clona un Form XObject con risorse proprie e restituisce (form, risorse).
fn clone_form(
    doc: &mut Document,
    form_id: ObjectId,
    page_id: ObjectId,
) -> Result<(ObjectId, ObjectId), String> {
    let stream = form_stream(doc, form_id)
        .ok_or_else(|| "Form XObject non trovato nel documento.".to_string())?;
    let data = stream
        .decompressed_content()
        .map_err(|e| format!("Form XObject non leggibile: {e}"))?;
    let mut dict = stream.dict.clone();
    dict.remove(b"Filter");
    dict.remove(b"DecodeParms");
    dict.remove(b"Length");

    // Un Form senza Resources eredita quelle della pagina: se ne copia una
    // versione propria, altrimenti i nuovi font finirebbero nella pagina.
    let mut resources = stream
        .dict
        .get(b"Resources")
        .ok()
        .and_then(|value| dict_of(doc, value))
        .cloned()
        .or_else(|| page_resources(doc, page_id).cloned())
        .unwrap_or_default();
    clone_subdict(doc, &mut resources, b"Font");
    clone_subdict(doc, &mut resources, b"XObject");
    let resources_id = doc.add_object(Object::Dictionary(resources));
    dict.set("Resources", Object::Reference(resources_id));

    let new_id = doc.add_object(Object::Stream(Stream::new(dict, data)));
    Ok((new_id, resources_id))
}

fn form_operations(doc: &Document, form_id: ObjectId) -> Result<Vec<Operation>, String> {
    let stream = form_stream(doc, form_id)
        .ok_or_else(|| "Form XObject non trovato nel documento.".to_string())?;
    let data = stream
        .decompressed_content()
        .map_err(|e| format!("Form XObject non leggibile: {e}"))?;
    let content = Content::decode(&data)
        .map_err(|e| format!("Form XObject non interpretabile: {e}"))?;
    Ok(content.operations)
}

fn set_form_operations(
    doc: &mut Document,
    form_id: ObjectId,
    operations: Vec<Operation>,
) -> Result<(), String> {
    let bytes = Content { operations }
        .encode()
        .map_err(|e| format!("Riscrittura del Form XObject non riuscita: {e}"))?;
    let object = doc
        .get_object_mut(form_id)
        .map_err(|e| format!("Form XObject non aggiornabile: {e}"))?;
    let stream = object
        .as_stream_mut()
        .map_err(|_| "Form XObject non aggiornabile: oggetto non stream.".to_string())?;
    stream.set_plain_content(bytes);
    Ok(())
}

fn ensure_owned_resources(
    doc: &mut Document,
    owner_id: ObjectId,
    fallback: Option<&Dictionary>,
) -> Result<ObjectId, String> {
    let base = doc
        .get_dictionary(owner_id)
        .ok()
        .and_then(|dict| dict.get(b"Resources").ok())
        .and_then(|value| dict_of(doc, value))
        .cloned()
        .or_else(|| fallback.cloned())
        .unwrap_or_default();
    let id = doc.add_object(Object::Dictionary(base));
    doc.get_dictionary_mut(owner_id)
        .map_err(|e| format!("Risorse non aggiornabili: {e}"))?
        .set("Resources", Object::Reference(id));
    Ok(id)
}

/// Copia un sotto-dizionario delle risorse e lo rende indiretto.
fn ensure_owned_subdict(
    doc: &mut Document,
    resources_id: ObjectId,
    key: &[u8],
) -> Result<ObjectId, String> {
    let base = doc
        .get_dictionary(resources_id)
        .ok()
        .and_then(|dict| dict.get(key).ok())
        .and_then(|value| dict_of(doc, value))
        .cloned()
        .unwrap_or_default();
    let id = doc.add_object(Object::Dictionary(base));
    doc.get_dictionary_mut(resources_id)
        .map_err(|e| format!("Risorse non aggiornabili: {e}"))?
        .set(key.to_vec(), Object::Reference(id));
    Ok(id)
}

fn font_uses_winansi(font: &Dictionary) -> bool {
    if font.get(b"Subtype").ok().and_then(name_of).as_deref() == Some("Type0") {
        return false;
    }
    let encoding = font
        .get(b"Encoding")
        .ok()
        .and_then(name_of)
        .unwrap_or_else(|| "WinAnsiEncoding".into());
    matches!(
        encoding.as_str(),
        "WinAnsiEncoding" | "StandardEncoding"
    )
}

/// Aggiunge Helvetica alle risorse indicate e restituisce il nome risorsa.
fn ensure_font_in(
    doc: &mut Document,
    resources_id: ObjectId,
    base_font: &str,
) -> Result<String, String> {
    let fonts_id = ensure_owned_subdict(doc, resources_id, b"Font")?;
    let mut existing: Option<String> = None;
    if let Ok(fonts) = doc.get_dictionary(fonts_id) {
        for (name, value) in fonts.iter() {
            let Some(id) = value.as_reference().ok() else {
                continue;
            };
            let Ok(font) = doc.get_dictionary(id) else {
                continue;
            };
            if font.get(b"BaseFont").ok().and_then(name_of).as_deref() == Some(base_font)
                && font_uses_winansi(font)
            {
                existing = Some(String::from_utf8_lossy(name).to_string());
                break;
            }
        }
    }
    if let Some(name) = existing {
        return Ok(name);
    }

    let mut font = Dictionary::new();
    font.set("Type", Object::Name(b"Font".to_vec()));
    font.set("Subtype", Object::Name(b"Type1".to_vec()));
    font.set("BaseFont", Object::Name(base_font.as_bytes().to_vec()));
    font.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
    let font_id = doc.add_object(Object::Dictionary(font));

    let mut name = String::from("OPDFHelvetica");
    let mut counter = 2;
    while doc
        .get_dictionary(fonts_id)
        .map(|fonts| fonts.get(name.as_bytes()).is_ok())
        .unwrap_or(false)
    {
        name = format!("OPDFHelvetica{counter}");
        counter += 1;
    }
    doc.get_dictionary_mut(fonts_id)
        .map_err(|e| format!("Risorse font non aggiornabili: {e}"))?
        .set(name.as_bytes().to_vec(), Object::Reference(font_id));
    Ok(name)
}

/// Metriche per la scansione.
///
/// I base-14 Helvetica/Arial senza `/Widths` usano la tabella anche per il
/// testo esistente: altrimenti l'ingombro sarebbe 500/1000 per glifo e la
/// ricerca dell'area e l'adattamento risulterebbero sbagliati.
fn scan_fonts(doc: &Document, resources: Option<&Dictionary>) -> FontMap {
    let mut fonts = fonts_for(doc, resources);
    let Some(resources) = resources else {
        return fonts;
    };
    for (name, metrics) in fonts.iter_mut() {
        if metrics.two_byte || !metrics.widths.is_empty() {
            continue;
        }
        let Some(base) = font_base(doc, resources, name) else {
            continue;
        };
        if base.contains("Helvetica") || base.contains("Arial") {
            metrics.first_char = 32;
            metrics.widths = (32..=126u8).map(helvetica_width).collect();
            metrics.default_width = 556.0;
        }
    }
    fonts
}

/// Nome del BaseFont di una risorsa font, se disponibile.
fn font_base(doc: &Document, resources: &Dictionary, name: &str) -> Option<String> {
    let fonts = resources
        .get(b"Font")
        .ok()
        .and_then(|value| dict_of(doc, value))?;
    let font = fonts
        .get(name.as_bytes())
        .ok()
        .and_then(|value| dict_of(doc, value))?;
    font.get(b"BaseFont").ok().and_then(name_of)
}

fn font_is_standard(doc: &Document, resources: Option<&Dictionary>, name: &str) -> bool {
    let Some(fonts) = resources
        .and_then(|r| r.get(b"Font").ok())
        .and_then(|f| dict_of(doc, f))
    else {
        return false;
    };
    let Some(font) = fonts
        .get(name.as_bytes())
        .ok()
        .and_then(|f| dict_of(doc, f))
    else {
        return false;
    };
    if !font_uses_winansi(font) {
        return false;
    }
    let base = font
        .get(b"BaseFont")
        .ok()
        .and_then(name_of)
        .unwrap_or_default();
    [
        "Helvetica", "Arial", "Times", "Courier", "Symbol", "ZapfDingbats",
    ]
    .iter()
    .any(|family| base.contains(family))
}

/// Decidi se misurare con la width table di Helvetica o con le metriche del PDF.
///
/// La tabella si usa quando il font l'abbiamo aggiunto noi, oppure quando il
/// font riusato non dichiara `/Widths` ed è Helvetica/Arial (i 14 font standard
/// possono ometterle).
fn uses_helvetica_table(
    doc: &Document,
    resources: &Dictionary,
    fonts: &FontMap,
    name: &str,
    added: bool,
) -> bool {
    if added {
        return true;
    }
    let Some(metrics) = fonts.get(name) else {
        return false;
    };
    if !metrics.widths.is_empty() {
        return false;
    }
    font_base(doc, resources, name)
        .map(|base| base.contains("Helvetica") || base.contains("Arial"))
        .unwrap_or(false)
}

/// Prepara il nuovo content stream, aggiungendo il font alle risorse indicate.
fn plan_text_edit(
    doc: &mut Document,
    resources_id: ObjectId,
    resources: &Dictionary,
    fonts: &FontMap,
    operations: Vec<Operation>,
    hits: &[Run],
    new_text: &str,
) -> Result<TextPlan, String> {
    let anchor = hits
        .iter()
        .min_by_key(|run| run.index)
        .ok_or("Testo non modificabile.")?;
    let size = if anchor.font_size.abs() < 0.01 {
        DEFAULT_FONT_SIZE
    } else {
        anchor.font_size
    };

    let encoded_lines: Vec<Vec<u8>> = new_text
        .split('\n')
        .map(encode_winansi)
        .collect::<Result<_, _>>()?;

    let reuse = anchor
        .font
        .as_deref()
        .filter(|name| font_is_standard(doc, Some(resources), name))
        .map(|name| name.to_string());
    let added = reuse.is_none();
    let font_name = match reuse {
        Some(name) => name,
        None => ensure_font_in(doc, resources_id, "Helvetica")?,
    };

    let mut state = TextState::default();
    state.font = Some(font_name.clone());
    state.font_size = size;
    state.char_spacing = anchor.char_spacing;
    state.word_spacing = anchor.word_spacing;
    state.h_scale = anchor.h_scale;

    let table = uses_helvetica_table(doc, resources, fonts, &font_name, added);
    let new_width = encoded_lines
        .iter()
        .map(|line| {
            if table {
                helvetica_advance(
                    line,
                    size,
                    anchor.char_spacing,
                    anchor.word_spacing,
                    anchor.h_scale,
                )
            } else {
                text_advance(
                    &Object::String(line.clone(), lopdf::StringFormat::Literal),
                    &state,
                    fonts,
                )
                .0
            }
        })
        .fold(0.0f64, f64::max);

    let old_left = hits
        .iter()
        .map(|run| run.bbox.x0)
        .fold(f64::INFINITY, f64::min);
    let old_right = hits
        .iter()
        .map(|run| run.bbox.x1)
        .fold(f64::NEG_INFINITY, f64::max);
    let old_width = (old_right - old_left).max(0.0);
    let tm_scale = (anchor.tm[0] * anchor.tm[0] + anchor.tm[1] * anchor.tm[1])
        .sqrt()
        .max(0.01);
    let ctm_scale = (anchor.ctm[0] * anchor.ctm[0] + anchor.ctm[1] * anchor.ctm[1])
        .sqrt()
        .max(0.01);
    let old_text_width = old_width / (tm_scale * ctm_scale);

    let fit = if new_width > old_text_width * 1.01 && old_text_width > 0.5 && new_width > 0.0 {
        (old_text_width / new_width).clamp(MIN_H_SCALE, 1.0)
    } else {
        1.0
    };
    let fitted = fit < 0.999;
    let h_scale = anchor.h_scale * fit;

    let tm_scale_y = (anchor.tm[2] * anchor.tm[2] + anchor.tm[3] * anchor.tm[3])
        .sqrt()
        .max(0.01);
    let ctm_scale_y = (anchor.ctm[2] * anchor.ctm[2] + anchor.ctm[3] * anchor.ctm[3])
        .sqrt()
        .max(0.01);
    let old_height = (anchor.bbox.y1 - anchor.bbox.y0).max(0.0) / (tm_scale_y * ctm_scale_y);
    let leading = if anchor.leading > 0.01 {
        anchor.leading
    } else {
        size * 1.2
    };
    let capacity = if leading > 0.01 {
        ((old_height / leading).round() as usize).max(1)
    } else {
        1
    };
    let overflow_lines = encoded_lines.len().saturating_sub(capacity);

    let style = TextBlockStyle {
        font: &font_name,
        size,
        tm: &anchor.tm,
        tlm: &anchor.tlm,
        color: anchor.color,
        char_spacing: anchor.char_spacing,
        word_spacing: anchor.word_spacing,
        h_scale,
        leading,
        in_text: anchor.in_text,
    };
    let block = text_block(&style, &encoded_lines);

    let hit_indices: HashSet<usize> = hits.iter().map(|run| run.index).collect();
    let mut output: Vec<Operation> = Vec::with_capacity(operations.len() + block.len());
    for (index, operation) in operations.into_iter().enumerate() {
        if index == anchor.index {
            output.extend(block.iter().cloned());
        }
        if hit_indices.contains(&index) {
            continue;
        }
        output.push(operation);
    }

    Ok(TextPlan {
        operations: output,
        font: font_name,
        fitted,
        overflow_lines,
    })
}

fn add_xobject(
    doc: &mut Document,
    resources_id: ObjectId,
    child: ObjectId,
) -> Result<String, String> {
    let xobjects_id = ensure_owned_subdict(doc, resources_id, b"XObject")?;
    let mut counter = 1;
    let name = loop {
        let candidate = format!("OPDFEdit{counter}");
        let taken = doc
            .get_dictionary(xobjects_id)
            .map(|dict| dict.get(candidate.as_bytes()).is_ok())
            .unwrap_or(false);
        if !taken {
            break candidate;
        }
        counter += 1;
    };
    doc.get_dictionary_mut(xobjects_id)
        .map_err(|e| format!("Risorse XObject non aggiornabili: {e}"))?
        .set(name.as_bytes().to_vec(), Object::Reference(child));
    Ok(name)
}

fn replace_do_operand(
    operations: &mut [Operation],
    index: usize,
    expected: &str,
    new_name: &str,
) -> Result<(), String> {
    let operation = operations.get_mut(index).ok_or_else(|| {
        "L'operazione del Form XObject non è più presente: riprova.".to_string()
    })?;
    if operation.operator != "Do" {
        return Err(
            "Il contenuto è cambiato rispetto all'analisi: impossibile aggiornare il Form XObject."
                .to_string(),
        );
    }
    let current = operation
        .operands
        .first()
        .and_then(name_of)
        .unwrap_or_default();
    if current != expected {
        return Err(
            "Il Form XObject non corrisponde all'istanza selezionata: riapri il PDF e riprova."
                .to_string(),
        );
    }
    operation.operands = vec![Object::Name(new_name.as_bytes().to_vec())];
    Ok(())
}

/// Identificativi di content stream usati anche da altre pagine: non vanno cancellati.
fn shared_content_ids(
    doc: &Document,
    page_id: ObjectId,
    ids: &[ObjectId],
) -> HashSet<ObjectId> {
    let wanted: HashSet<ObjectId> = ids.iter().copied().collect();
    let mut shared = HashSet::new();
    for (_, other) in doc.get_pages() {
        if other == page_id {
            continue;
        }
        for id in doc.get_page_contents(other) {
            if wanted.contains(&id) {
                shared.insert(id);
            }
        }
    }
    shared
}

fn write_page_content(
    doc: &mut Document,
    page_id: ObjectId,
    old_ids: &[ObjectId],
    operations: Vec<Operation>,
) -> Result<(), String> {
    let bytes = Content { operations }
        .encode()
        .map_err(|e| format!("Riscrittura del contenuto non riuscita: {e}"))?;
    let new_id = doc.add_object(Object::Stream(Stream::new(Dictionary::new(), bytes)));
    doc.get_dictionary_mut(page_id)
        .map_err(|e| format!("Pagina non aggiornabile: {e}"))?
        .set("Contents", Object::Reference(new_id));
    let shared = shared_content_ids(doc, page_id, old_ids);
    for old in old_ids {
        if !shared.contains(old) {
            let _ = doc.delete_object(*old);
        }
    }
    Ok(())
}

fn rewrite_page_do(
    doc: &mut Document,
    page_id: ObjectId,
    ids: &[ObjectId],
    index: usize,
    expected: &str,
    new_name: &str,
) -> Result<(), String> {
    let data = content_bytes(doc, ids)?;
    let mut content = Content::decode(&data)
        .map_err(|e| format!("Contenuto della pagina non interpretabile: {e}"))?;
    replace_do_operand(&mut content.operations, index, expected, new_name)?;
    write_page_content(doc, page_id, ids, content.operations)
}

fn rewrite_form_do(
    doc: &mut Document,
    form_id: ObjectId,
    index: usize,
    expected: &str,
    new_name: &str,
) -> Result<(), String> {
    let mut operations = form_operations(doc, form_id)?;
    replace_do_operand(&mut operations, index, expected, new_name)?;
    set_form_operations(doc, form_id, operations)
}

fn no_text_error() -> String {
    "Nessuna operazione di testo trovata in quest'area, né nella pagina né nei Form XObject (massimo 3 livelli): il contenuto può essere un'immagine o un oggetto grafico. Usa OCR o la redazione."
        .to_string()
}

pub fn replace_line(
    src: &Path,
    dest: &Path,
    password: Option<&str>,
    page_index: usize,
    rect: [f32; 4],
    new_text: &str,
) -> Result<EditReport, String> {
    if new_text.trim().is_empty() {
        return Err("Il nuovo testo è vuoto: usa «Elimina testo» per rimuovere la riga.".into());
    }
    // Il testo arriva dalla UI: accetta anche i capo riga Windows.
    let normalized = new_text.replace("\r\n", "\n").replace('\r', "\n");
    let new_text = normalized.as_str();
    let mut doc = match password {
        Some(p) => Document::load_with_password(src, p),
        None => Document::load(src),
    }
    .map_err(|e| format!("Apertura del PDF non riuscita: {e}"))?;
    if doc.is_encrypted() {
        return Err("La modifica del testo di PDF cifrati non è supportata.".into());
    }

    let pages = doc.get_pages();
    let page_id = *pages
        .get(&(page_index as u32 + 1))
        .ok_or("Pagina non trovata.")?;
    let area = norm_to_user(&rect, page_rotation(&doc, page_id), media_box(&doc, page_id));

    let ids = page_content_ids(&doc, page_id);
    let data = content_bytes(&doc, &ids)?;
    let page_operations = if data.is_empty() {
        Vec::new()
    } else {
        Content::decode(&data)
            .map_err(|e| format!("Contenuto della pagina non interpretabile: {e}"))?
            .operations
    };
    let page_res = page_resources(&doc, page_id).cloned();
    let page_fonts = scan_fonts(&doc, page_res.as_ref());
    let (runs, uses) = scan_content(&page_operations, &page_fonts);
    let hits: Vec<Run> = runs
        .iter()
        .filter(|run| run.bbox.intersects(&area))
        .cloned()
        .collect();

    if !hits.is_empty() {
        let resources = page_res.clone().unwrap_or_default();
        let resources_id = ensure_owned_resources(&mut doc, page_id, page_res.as_ref())?;
        let plan = plan_text_edit(
            &mut doc,
            resources_id,
            &resources,
            &page_fonts,
            page_operations,
            &hits,
            new_text,
        )?;
        write_page_content(&mut doc, page_id, &ids, plan.operations)?;
        doc.save(dest)
            .map_err(|e| format!("Salvataggio del PDF modificato non riuscito: {e}"))?;
        return Ok(EditReport {
            replaced_chars: hits.iter().map(|run| run.chars).sum(),
            inserted_chars: new_text.chars().count(),
            font: plan.font,
            fitted: plan.fitted,
            overflow_lines: plan.overflow_lines,
            form_depth: 0,
            cloned_form: false,
        });
    }

    // Nessun testo in pagina: cerca nei Form XObject richiamati dalla pagina.
    let mut found: Option<FormHit> = None;
    for usage in &uses {
        let Some(form_id) = xobject_id(&doc, page_res.as_ref(), &usage.name) else {
            continue;
        };
        let Some(stream) = form_stream(&doc, form_id) else {
            continue;
        };
        let matrix = form_matrix(&doc, &stream);
        let effective = concat(&matrix, &usage.ctm);
        let Some(inverse) = invert(&effective) else {
            continue;
        };
        let Some(mapped) = mapped_rect(&area, &inverse) else {
            continue;
        };
        let chain = vec![FormStep {
            name: usage.name.clone(),
            op_index: usage.index,
            target_id: form_id,
        }];
        if let Some(hit) = find_in_form(&doc, page_id, form_id, &mapped, chain, 1) {
            found = Some(hit);
            break;
        }
    }
    let Some(hit) = found else {
        return Err(no_text_error());
    };

    // Clona tutta la catena: l'istanza modificata non deve alterare le altre.
    let mut new_ids = Vec::with_capacity(hit.chain.len());
    let mut resources_ids = Vec::with_capacity(hit.chain.len());
    for step in &hit.chain {
        let (new_id, resources_id) = clone_form(&mut doc, step.target_id, page_id)?;
        new_ids.push(new_id);
        resources_ids.push(resources_id);
    }

    let target_new = *new_ids
        .last()
        .ok_or("Form XObject non modificabile: catena vuota.")?;
    let target_resources = *resources_ids
        .last()
        .ok_or("Form XObject non modificabile: risorse assenti.")?;
    let target_operations = form_operations(&doc, target_new)?;
    let plan = plan_text_edit(
        &mut doc,
        target_resources,
        &hit.resources,
        &hit.fonts,
        target_operations,
        &hit.hits,
        new_text,
    )?;
    set_form_operations(&mut doc, target_new, plan.operations)?;

    // Risale la catena: ogni contenitore clonato richiama il figlio clonato con
    // un nome nuovo, così solo questa istanza cambia.
    if hit.chain.len() > 1 {
        for level in (1..hit.chain.len()).rev() {
            let parent_id = new_ids[level - 1];
            let parent_resources = resources_ids[level - 1];
            let child_id = new_ids[level];
            let name = add_xobject(&mut doc, parent_resources, child_id)?;
            rewrite_form_do(
                &mut doc,
                parent_id,
                hit.chain[level].op_index,
                &hit.chain[level].name,
                &name,
            )?;
        }
    }

    let current_page_resources = page_resources(&doc, page_id).cloned();
    let page_resources_id =
        ensure_owned_resources(&mut doc, page_id, current_page_resources.as_ref())?;
    let top_name = add_xobject(&mut doc, page_resources_id, new_ids[0])?;
    rewrite_page_do(
        &mut doc,
        page_id,
        &ids,
        hit.chain[0].op_index,
        &hit.chain[0].name,
        &top_name,
    )?;

    doc.save(dest)
        .map_err(|e| format!("Salvataggio del PDF modificato non riuscito: {e}"))?;
    Ok(EditReport {
        replaced_chars: hit.hits.iter().map(|run| run.chars).sum(),
        inserted_chars: new_text.chars().count(),
        font: plan.font,
        fitted: plan.fitted,
        overflow_lines: plan.overflow_lines,
        form_depth: hit.chain.len(),
        cloned_form: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream_object(content: &[u8]) -> Vec<u8> {
        let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        stream.extend_from_slice(content);
        stream.extend_from_slice(b"endstream");
        stream
    }

    fn form_object(content: &[u8], matrix: Option<&str>) -> Vec<u8> {
        let matrix = matrix
            .map(|value| format!(" /Matrix {value}"))
            .unwrap_or_default();
        let mut stream = format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 500 200]{matrix} /Resources << /Font << /F1 5 0 R >> >> /Length {} >>\nstream\n",
            content.len()
        )
        .into_bytes();
        stream.extend_from_slice(content);
        stream.extend_from_slice(b"endstream");
        stream
    }

    /// Costruisce un PDF minimo con una xref valida.
    fn build_pdf(objects: Vec<Vec<u8>>) -> Vec<u8> {
        let mut file: Vec<u8> = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(file.len());
            file.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            file.extend_from_slice(body);
            file.extend_from_slice(b"\nendobj\n");
        }
        let xref = file.len();
        file.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in &offsets {
            file.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        file.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        file
    }

    fn sample_pdf() -> Vec<u8> {
        let content =
            b"BT /F1 12 Tf 72 700 Td (SEGRETO) Tj ET\nBT /F1 12 Tf 72 500 Td (PUBBLICO) Tj ET\n";
        build_pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
            stream_object(content),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /MissingWidth 500 /Encoding /WinAnsiEncoding >>".to_vec(),
        ])
    }

    fn spacing_pdf() -> Vec<u8> {
        let content = b"BT /F1 12 Tf 2 Tc 6 Tw 90 Tz 72 700 Td (SEGRETO PDF) Tj ET\n";
        build_pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
            stream_object(content),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /MissingWidth 500 /Encoding /WinAnsiEncoding >>".to_vec(),
        ])
    }

    fn leading_pdf() -> Vec<u8> {
        let content = b"BT /F1 12 Tf 16 TL 72 700 Td (PRIMA RIGA) Tj ET\n";
        build_pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
            stream_object(content),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /MissingWidth 500 /Encoding /WinAnsiEncoding >>".to_vec(),
        ])
    }

    fn form_pdf(font: &[u8], page_content: &[u8]) -> Vec<u8> {
        build_pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 5 0 R >> /XObject << /Fm1 6 0 R >> >> /Contents 4 0 R >>".to_vec(),
            stream_object(page_content),
            font.to_vec(),
            form_object(
                b"BT /F1 12 Tf 0 0 Td (SEGRETO) Tj ET\n",
                Some("[1 0 0 1 50 600]"),
            ),
        ])
    }

    const HELVETICA: &[u8] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /MissingWidth 500 /Encoding /WinAnsiEncoding >>";
    const CUSTOM_SANS: &[u8] = b"<< /Type /Font /Subtype /Type1 /BaseFont /CustomSans /MissingWidth 500 /Encoding /WinAnsiEncoding >>";

    fn page_operations(doc: &Document, page_id: ObjectId) -> Vec<Operation> {
        let data = doc.get_page_content(page_id);
        Content::decode(&data)
            .expect("contenuto decodificabile")
            .operations
    }

    fn operand(operation: &Operation, index: usize) -> Option<f64> {
        operation.operands.get(index).and_then(number)
    }

    fn last_value(operations: &[Operation], operator: &str) -> Option<f64> {
        operations
            .iter()
            .rev()
            .find(|operation| operation.operator == operator)
            .and_then(|operation| operand(operation, 0))
    }

    fn font_bases(doc: &Document, resources: &Dictionary) -> Vec<String> {
        let Some(fonts) = resources
            .get(b"Font")
            .ok()
            .and_then(|value| dict_of(doc, value))
        else {
            return Vec::new();
        };
        fonts
            .iter()
            .filter_map(|(_, value)| {
                value
                    .as_reference()
                    .ok()
                    .and_then(|id| doc.get_dictionary(id).ok())
                    .and_then(|font| font.get(b"BaseFont").ok().and_then(name_of))
            })
            .collect()
    }

    fn first_do_target(doc: &Document, page_id: ObjectId) -> Option<ObjectId> {
        let operations = page_operations(doc, page_id);
        let name = operations
            .iter()
            .find(|operation| operation.operator == "Do")?
            .operands
            .first()
            .and_then(name_of)?;
        xobject_id(doc, page_resources(doc, page_id), &name)
    }

    /// Estrae il testo seguendo anche i Form XObject.
    ///
    /// `lopdf::Document::extract_text` legge solo il content stream della
    /// pagina: per verificare una modifica dentro un Form serve questo
    /// attraversamento esplicito.
    fn extract_text_with_forms(doc: &Document, page_id: ObjectId) -> String {
        let mut text = String::new();
        let data = doc.get_page_content(page_id);
        if let Ok(content) = Content::decode(&data) {
            collect_text(
                doc,
                page_resources(doc, page_id),
                &content.operations,
                0,
                &mut text,
            );
        }
        text
    }

    fn collect_text(
        doc: &Document,
        resources: Option<&Dictionary>,
        operations: &[Operation],
        depth: usize,
        out: &mut String,
    ) {
        for operation in operations {
            match operation.operator.as_str() {
                "Tj" => push_string(out, operation.operands.first()),
                "TJ" => {
                    if let Some(Object::Array(items)) = operation.operands.first() {
                        for item in items {
                            push_string(out, Some(item));
                        }
                    }
                }
                "Do" => {
                    if depth >= MAX_FORM_DEPTH {
                        continue;
                    }
                    let Some(name) = operation.operands.first().and_then(name_of) else {
                        continue;
                    };
                    let Some(id) = xobject_id(doc, resources, &name) else {
                        continue;
                    };
                    let Some(stream) = form_stream(doc, id) else {
                        continue;
                    };
                    let Ok(bytes) = stream.decompressed_content() else {
                        continue;
                    };
                    let Ok(content) = Content::decode(&bytes) else {
                        continue;
                    };
                    let nested = stream
                        .dict
                        .get(b"Resources")
                        .ok()
                        .and_then(|value| dict_of(doc, value))
                        .or(resources);
                    collect_text(doc, nested, &content.operations, depth + 1, out);
                    out.push('\n');
                }
                _ => {}
            }
        }
    }

    fn push_string(out: &mut String, object: Option<&Object>) {
        if let Some(Object::String(bytes, _)) = object {
            out.push_str(&String::from_utf8_lossy(bytes));
        }
    }

    #[test]
    fn replaces_text_in_place_and_keeps_the_rest() {
        let dir = std::env::temp_dir();
        let src = dir.join("openpdf-edit-src.pdf");
        let dest = dir.join("openpdf-edit-dest.pdf");
        std::fs::write(&src, sample_pdf()).unwrap();

        // La riga "SEGRETO" sta intorno a y = 700, x = 72.
        let rect = [0.10f32, 0.155, 0.30, 0.035];
        let report = replace_line(&src, &dest, None, 0, rect, "RISERVATO").unwrap();
        assert!(report.inserted_chars > 0);
        assert_eq!(report.form_depth, 0);
        assert!(!report.cloned_form);

        let doc = Document::load(&dest).unwrap();
        let text = doc.extract_text(&[1]).unwrap();
        assert!(
            !text.contains("SEGRETO"),
            "testo vecchio ancora presente: {text:?}"
        );
        assert!(
            text.contains("RISERVATO"),
            "testo nuovo assente: {text:?}"
        );
        assert!(
            text.contains("PUBBLICO"),
            "testo non toccato perso: {text:?}"
        );
    }

    #[test]
    fn keeps_char_word_spacing_and_horizontal_scaling() {
        let dir = std::env::temp_dir();
        let src = dir.join("openpdf-spacing-src.pdf");
        let dest = dir.join("openpdf-spacing-dest.pdf");
        std::fs::write(&src, spacing_pdf()).unwrap();

        let report = replace_line(
            &src,
            &dest,
            None,
            0,
            [0.10f32, 0.15, 0.30, 0.04],
            "NUOVO",
        )
        .unwrap();
        assert!(!report.fitted, "un testo più corto non deve essere ridotto");

        let doc = Document::load(&dest).unwrap();
        let page_id = doc.get_pages()[&1];
        let operations = page_operations(&doc, page_id);
        assert_eq!(last_value(&operations, "Tc"), Some(2.0), "Tc perso");
        assert_eq!(last_value(&operations, "Tw"), Some(6.0), "Tw perso");
        let tz = last_value(&operations, "Tz").expect("Tz assente");
        assert!((tz - 90.0).abs() < 0.01, "Tz atteso 90, trovato {tz}");
        assert!(doc.extract_text(&[1]).unwrap().contains("NUOVO"));
    }

    #[test]
    fn combines_fit_scaling_with_the_original_horizontal_scaling() {
        let dir = std::env::temp_dir();
        let src = dir.join("openpdf-spacing-fit-src.pdf");
        let dest = dir.join("openpdf-spacing-fit-dest.pdf");
        std::fs::write(&src, spacing_pdf()).unwrap();

        let report = replace_line(
            &src,
            &dest,
            None,
            0,
            [0.10f32, 0.15, 0.30, 0.04],
            "RISERVATO PDF",
        )
        .unwrap();
        assert!(report.fitted, "il testo più largo deve essere ridotto");

        let doc = Document::load(&dest).unwrap();
        let page_id = doc.get_pages()[&1];
        let operations = page_operations(&doc, page_id);
        assert_eq!(last_value(&operations, "Tc"), Some(2.0), "Tc perso");
        assert_eq!(last_value(&operations, "Tw"), Some(6.0), "Tw perso");
        let tz = last_value(&operations, "Tz").expect("Tz assente");
        // Larghezza originale 104,8, nuova 118,4: fattore 0,885 sul Tz 90.
        assert!(
            (tz - 79.7).abs() < 0.5,
            "Tz adattato atteso ~79,7, trovato {tz}"
        );
        assert!(tz < 90.0, "la riduzione deve partire dal Tz originale: {tz}");
    }

    #[test]
    fn edits_text_inside_a_form_xobject() {
        let dir = std::env::temp_dir();
        let src = dir.join("openpdf-form-src.pdf");
        let dest = dir.join("openpdf-form-dest.pdf");
        std::fs::write(&src, form_pdf(HELVETICA, b"q /Fm1 Do Q\n")).unwrap();

        let report = replace_line(
            &src,
            &dest,
            None,
            0,
            [0.07f32, 0.27, 0.15, 0.025],
            "MODULO NUOVO",
        )
        .unwrap();
        assert_eq!(report.form_depth, 1, "la modifica deve avvenire nel Form");
        assert!(report.cloned_form);
        assert_eq!(report.inserted_chars, "MODULO NUOVO".chars().count());

        let doc = Document::load(&dest).unwrap();
        let page_id = doc.get_pages()[&1];
        let text = extract_text_with_forms(&doc, page_id);
        assert!(
            !text.contains("SEGRETO"),
            "testo vecchio ancora presente: {text:?}"
        );
        assert!(
            text.contains("MODULO NUOVO"),
            "testo nuovo assente: {text:?}"
        );

        // `lopdf::Document::extract_text` non attraversa i Form XObject: la
        // verifica end-to-end usa pdfium, lo stesso motore dell'app.
        let edited = crate::engine::open_doc(&dest, None).unwrap();
        let page = edited.pages().get(0).unwrap();
        let rendered = page.text().unwrap().all();
        assert!(
            rendered.contains("MODULO NUOVO") && !rendered.contains("SEGRETO"),
            "pdfium non vede la modifica: {rendered:?}"
        );

        // Il Form originale non deve essere stato toccato: la modifica vive nel clone.
        let original = doc.get_object((6, 0)).and_then(Object::as_stream).unwrap();
        let original_data = original.decompressed_content().unwrap();
        let original_text = String::from_utf8_lossy(&original_data);
        assert!(
            original_text.contains("SEGRETO"),
            "il Form originale è stato modificato: {original_text:?}"
        );
    }

    #[test]
    fn edits_only_the_selected_instance_of_a_shared_form() {
        let dir = std::env::temp_dir();
        let src = dir.join("openpdf-form-twice-src.pdf");
        let dest = dir.join("openpdf-form-twice-dest.pdf");
        let page_content = b"q 1 0 0 1 0 0 cm /Fm1 Do Q\nq 1 0 0 1 0 -300 cm /Fm1 Do Q\n";
        std::fs::write(&src, form_pdf(HELVETICA, page_content)).unwrap();

        // Il primo Form è a y = 600, il secondo a y = 300: si modifica solo il primo.
        replace_line(
            &src,
            &dest,
            None,
            0,
            [0.07f32, 0.27, 0.15, 0.025],
            "MODULO NUOVO",
        )
        .unwrap();

        let doc = Document::load(&dest).unwrap();
        let page_id = doc.get_pages()[&1];
        let text = extract_text_with_forms(&doc, page_id);
        assert_eq!(
            text.matches("SEGRETO").count(),
            1,
            "la seconda istanza deve restare intatta: {text:?}"
        );
        assert_eq!(
            text.matches("MODULO NUOVO").count(),
            1,
            "una sola istanza deve cambiare: {text:?}"
        );

        let operations = page_operations(&doc, page_id);
        let do_names: Vec<String> = operations
            .iter()
            .filter(|operation| operation.operator == "Do")
            .filter_map(|operation| operation.operands.first().and_then(name_of))
            .collect();
        assert_eq!(do_names.len(), 2);
        assert_ne!(
            do_names[0], do_names[1],
            "l'istanza modificata deve avere un nome proprio: {do_names:?}"
        );
    }

    #[test]
    fn edits_text_in_a_nested_form_xobject() {
        let dir = std::env::temp_dir();
        let src = dir.join("openpdf-form-nested-src.pdf");
        let dest = dir.join("openpdf-form-nested-dest.pdf");

        let outer_content = b"q /FmInner Do Q\n";
        let inner_content = b"BT /F1 12 Tf 0 0 Td (SEGRETO) Tj ET\n";
        let mut outer = format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 500 200] /Matrix [1 0 0 1 0 0] /Resources << /XObject << /FmInner 7 0 R >> >> /Length {} >>\nstream\n",
            outer_content.len()
        )
        .into_bytes();
        outer.extend_from_slice(outer_content);
        outer.extend_from_slice(b"endstream");
        let mut inner = format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 500 200] /Matrix [1 0 0 1 50 600] /Resources << /Font << /F1 5 0 R >> >> /Length {} >>\nstream\n",
            inner_content.len()
        )
        .into_bytes();
        inner.extend_from_slice(inner_content);
        inner.extend_from_slice(b"endstream");

        let pdf = build_pdf(vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 5 0 R >> /XObject << /FmOuter 6 0 R >> >> /Contents 4 0 R >>".to_vec(),
            stream_object(b"q /FmOuter Do Q\n"),
            HELVETICA.to_vec(),
            outer,
            inner,
        ]);
        std::fs::write(&src, pdf).unwrap();

        let report = replace_line(
            &src,
            &dest,
            None,
            0,
            [0.07f32, 0.27, 0.15, 0.025],
            "MODULO NUOVO",
        )
        .unwrap();
        assert_eq!(report.form_depth, 2, "due livelli di Form");

        let doc = Document::load(&dest).unwrap();
        let page_id = doc.get_pages()[&1];
        let text = extract_text_with_forms(&doc, page_id);
        assert!(!text.contains("SEGRETO"), "testo vecchio ancora presente: {text:?}");
        assert!(text.contains("MODULO NUOVO"), "testo nuovo assente: {text:?}");

        // Anche il Form interno originale resta com'era.
        let original = doc.get_object((7, 0)).and_then(Object::as_stream).unwrap();
        let data = original.decompressed_content().unwrap();
        assert!(
            String::from_utf8_lossy(&data).contains("SEGRETO"),
            "il Form annidato originale è stato modificato"
        );
    }

    #[test]
    fn puts_the_new_font_in_the_form_resources() {
        let dir = std::env::temp_dir();
        let src = dir.join("openpdf-form-font-src.pdf");
        let dest = dir.join("openpdf-form-font-dest.pdf");
        std::fs::write(&src, form_pdf(CUSTOM_SANS, b"q /Fm1 Do Q\n")).unwrap();

        replace_line(
            &src,
            &dest,
            None,
            0,
            [0.07f32, 0.27, 0.15, 0.025],
            "MODULO",
        )
        .unwrap();

        let doc = Document::load(&dest).unwrap();
        let page_id = doc.get_pages()[&1];
        let page_bases = font_bases(&doc, page_resources(&doc, page_id).unwrap());
        assert!(
            page_bases.iter().all(|base| !base.contains("Helvetica")),
            "il font del Form non deve finire nella pagina: {page_bases:?}"
        );

        let form_id = first_do_target(&doc, page_id).expect("Form richiamato dalla pagina");
        let stream = doc.get_object(form_id).unwrap().as_stream().unwrap();
        let resources = stream
            .dict
            .get(b"Resources")
            .ok()
            .and_then(|value| dict_of(&doc, value))
            .unwrap();
        let form_bases = font_bases(&doc, resources);
        assert!(
            form_bases.iter().any(|base| base.contains("Helvetica")),
            "Helvetica deve stare nelle risorse del Form: {form_bases:?}"
        );
    }

    #[test]
    fn uses_the_original_leading_for_new_lines() {
        let dir = std::env::temp_dir();
        let src = dir.join("openpdf-leading-src.pdf");
        let dest = dir.join("openpdf-leading-dest.pdf");
        std::fs::write(&src, leading_pdf()).unwrap();

        let report = replace_line(
            &src,
            &dest,
            None,
            0,
            [0.10f32, 0.15, 0.30, 0.04],
            "UNO\nDUE",
        )
        .unwrap();
        assert_eq!(report.overflow_lines, 1, "una riga in più rispetto alla originale");

        let doc = Document::load(&dest).unwrap();
        let page_id = doc.get_pages()[&1];
        let operations = page_operations(&doc, page_id);
        let moved = operations.iter().any(|operation| {
            operation.operator == "Td"
                && operand(operation, 0) == Some(0.0)
                && operand(operation, 1) == Some(-16.0)
        });
        assert!(moved, "il leading originale (16) non è stato riusato");
        let text = doc.extract_text(&[1]).unwrap();
        assert!(text.contains("UNO") && text.contains("DUE"), "{text:?}");
    }

    #[test]
    fn falls_back_to_one_point_two_times_the_body() {
        let dir = std::env::temp_dir();
        let src = dir.join("openpdf-leading-default-src.pdf");
        let dest = dir.join("openpdf-leading-default-dest.pdf");
        std::fs::write(&src, sample_pdf()).unwrap();

        let report = replace_line(
            &src,
            &dest,
            None,
            0,
            [0.10f32, 0.155, 0.30, 0.035],
            "UNO\nDUE",
        )
        .unwrap();
        assert_eq!(report.overflow_lines, 1);

        let doc = Document::load(&dest).unwrap();
        let page_id = doc.get_pages()[&1];
        let operations = page_operations(&doc, page_id);
        let moved = operations.iter().any(|operation| {
            operation.operator == "Td"
                && operand(operation, 0) == Some(0.0)
                && operand(operation, 1)
                    .map(|value| (value + 14.4).abs() < 0.01)
                    .unwrap_or(false)
        });
        assert!(moved, "il leading di ripiego (1,2 × 12 = 14,4) non è stato usato");
    }

    #[test]
    fn rejects_unsupported_characters() {
        let error = encode_winansi("漢字").unwrap_err();
        assert!(error.contains("WinAnsi"));
    }

    #[test]
    fn rejects_empty_text() {
        let dir = std::env::temp_dir();
        let error = replace_line(
            &dir.join("openpdf-vuoto-src.pdf"),
            &dir.join("openpdf-vuoto-dest.pdf"),
            None,
            0,
            [0.0, 0.0, 0.1, 0.1],
            "   ",
        )
        .unwrap_err();
        assert!(error.contains("vuoto"), "messaggio inatteso: {error}");
    }

    #[test]
    fn edits_text_in_a_real_pdf_when_available() {
        let src = Path::new(r"C:\Users\utente\Documents\toutorial.pdf");
        if !src.exists() {
            return;
        }
        let doc = crate::engine::open_doc(src, None).unwrap();
        let page = doc.pages().get(0).unwrap();
        let space = crate::engine::page_space(&page).unwrap();
        let text = page.text().unwrap();
        let segments = text.segments();
        let segment = segments
            .iter()
            .find(|segment| segment.text().trim().chars().count() >= 4)
            .expect("nessun segmento di testo");
        let rect = crate::engine::rect_to_norm(&space, &segment.bounds());
        drop(doc);

        let dest = std::env::temp_dir().join("openpdf-edit-real.pdf");
        let report = replace_line(
            src,
            &dest,
            None,
            0,
            rect,
            "TESTO MODIFICATO",
        )
        .unwrap();
        assert_eq!(report.inserted_chars, 16);

        let edited = crate::engine::open_doc(&dest, None).unwrap();
        let page = edited.pages().get(0).unwrap();
        let value = page.text().unwrap().all();
        assert!(
            value.contains("TESTO MODIFICATO"),
            "il nuovo testo non è nel PDF: {value:?}"
        );
    }
}
