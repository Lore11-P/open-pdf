//! Redazione irreversibile a livello di content stream.
//!
//! A differenza della rasterizzazione (`pages::redact_pdf`), questo modulo
//! rimuove davvero il testo, le immagini e la grafica che cadono dentro le aree
//! indicate, elimina le annotazioni e i metadati collegati e poi disegna i
//! rettangoli neri in un content stream separato (così nessun clip path
//! precedente può nasconderli). Il contenuto rimosso non resta nel file.
//!
//! Limiti dichiarati (restituiti alla UI in `RedactReport`):
//! - un'immagine coperta solo in parte viene ritagliata e ricodificata: nel file
//!   restano soltanto i pixel non coperti (JPEG per `DCTDecode`, zlib/Flate per i
//!   campioni grezzi). I formati che non sappiamo decodificare (JPX, CCITT,
//!   JBIG2, immagini con maschera di trasparenza) vengono rimossi per intero;
//! - un Form XObject usato più volte viene clonato per ogni uso, così ogni
//!   istanza può essere riscritta in modo indipendente;
//! - le immagini inline vengono rimosse per intero quando intersecano l'area;
//! - le immagini che restano raggiungibili da altre pagine non vengono toccate,
//!   mentre quelle sostituite e non più referenziate vengono svuotate;
//! - i clip path attivi non vengono considerati e più aree sulla stessa
//!   immagine vengono unite nel loro rettangolo di inviluppo: si può rimuovere
//!   qualche pixel in più, mai in meno.

use image::codecs::jpeg::JpegEncoder;
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Default, Clone, Debug, serde::Serialize)]
pub struct RedactReport {
    pub texts: usize,
    /// Immagini rimosse per intero.
    pub images: usize,
    /// Immagini parzialmente coperte, ritagliate e ricodificate.
    pub cropped: usize,
    pub paths: usize,
    pub annotations: usize,
    /// Mantenuto per compatibilità con la UI: i moduli condivisi ora vengono
    /// clonati, quindi il contatore resta a zero.
    pub skipped_forms: usize,
}

impl RedactReport {
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("{} testi", self.texts)];
        if self.images > 0 {
            parts.push(format!("{} immagini", self.images));
        }
        if self.cropped > 0 {
            parts.push(format!("{} immagini parziali", self.cropped));
        }
        if self.paths > 0 {
            parts.push(format!("{} elementi grafici", self.paths));
        }
        if self.annotations > 0 {
            parts.push(format!("{} annotazioni", self.annotations));
        }
        let mut text = format!("Redazione applicata: rimossi {}.", parts.join(", "));
        if self.cropped > 0 {
            text.push_str(&format!(
                " {} immagini sono state ritagliate: nel file restano solo le parti non coperte.",
                self.cropped
            ));
        }
        if self.skipped_forms > 0 {
            text.push_str(&format!(
                " {} moduli condivisi non sono stati modificati: verifica visivamente le aree.",
                self.skipped_forms
            ));
        }
        text
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Rect {
    pub(crate) x0: f64,
    pub(crate) y0: f64,
    pub(crate) x1: f64,
    pub(crate) y1: f64,
}

impl Rect {
    pub(crate) fn intersects(&self, other: &Rect) -> bool {
        self.x0 < other.x1 && other.x0 < self.x1 && self.y0 < other.y1 && other.y0 < self.y1
    }

    pub(crate) fn from_points(points: &[(f64, f64)]) -> Option<Rect> {
        let mut x0 = f64::INFINITY;
        let mut y0 = f64::INFINITY;
        let mut x1 = f64::NEG_INFINITY;
        let mut y1 = f64::NEG_INFINITY;
        for (x, y) in points {
            if !x.is_finite() || !y.is_finite() {
                continue;
            }
            x0 = x0.min(*x);
            y0 = y0.min(*y);
            x1 = x1.max(*x);
            y1 = y1.max(*y);
        }
        if x0 > x1 || y0 > y1 {
            None
        } else {
            Some(Rect { x0, y0, x1, y1 })
        }
    }
}

pub(crate) type Matrix = [f64; 6];
pub(crate) const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

pub(crate) fn apply(m: &Matrix, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

/// Applica `first` e poi `second`.
pub(crate) fn concat(first: &Matrix, second: &Matrix) -> Matrix {
    let [a1, b1, c1, d1, e1, f1] = *first;
    let [a2, b2, c2, d2, e2, f2] = *second;
    [
        a1 * a2 + b1 * c2,
        a1 * b2 + b1 * d2,
        c1 * a2 + d1 * c2,
        c1 * b2 + d1 * d2,
        e1 * a2 + f1 * c2 + e2,
        e1 * b2 + f1 * d2 + f2,
    ]
}

pub(crate) fn invert(m: &Matrix) -> Option<Matrix> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return None;
    }
    Some([
        m[3] / det,
        -m[1] / det,
        -m[2] / det,
        m[0] / det,
        (m[2] * m[5] - m[3] * m[4]) / det,
        (m[1] * m[4] - m[0] * m[5]) / det,
    ])
}

fn map_rect(rect: &Rect, m: &Matrix) -> Option<Rect> {
    let points = [
        apply(m, rect.x0, rect.y0),
        apply(m, rect.x1, rect.y0),
        apply(m, rect.x0, rect.y1),
        apply(m, rect.x1, rect.y1),
    ];
    Rect::from_points(&points)
}

#[derive(Clone)]
pub(crate) struct TextState {
    pub(crate) font_size: f64,
    pub(crate) leading: f64,
    pub(crate) char_spacing: f64,
    pub(crate) word_spacing: f64,
    pub(crate) h_scale: f64,
    pub(crate) font: Option<String>,
}

impl Default for TextState {
    fn default() -> Self {
        TextState {
            font_size: 0.0,
            leading: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            h_scale: 1.0,
            font: None,
        }
    }
}

#[derive(Clone)]
struct GState {
    ctm: Matrix,
    text: TextState,
}

impl Default for GState {
    fn default() -> Self {
        GState {
            ctm: IDENTITY,
            text: TextState::default(),
        }
    }
}

pub(crate) struct FontMetrics {
    pub(crate) first_char: i64,
    pub(crate) widths: Vec<f64>,
    pub(crate) default_width: f64,
    pub(crate) two_byte: bool,
}

impl FontMetrics {
    pub(crate) fn width_of(&self, code: u32) -> f64 {
        if self.two_byte {
            return self
                .widths
                .get(code as usize)
                .copied()
                .unwrap_or(self.default_width);
        }
        let index = code as i64 - self.first_char;
        if index >= 0 {
            if let Some(width) = self.widths.get(index as usize) {
                return *width;
            }
        }
        self.default_width
    }
}

pub(crate) type FontMap = HashMap<String, FontMetrics>;

pub(crate) fn number(obj: &Object) -> Option<f64> {
    match obj {
        Object::Integer(v) => Some(*v as f64),
        Object::Real(v) => Some(*v as f64),
        _ => None,
    }
}

pub(crate) fn name_of(obj: &Object) -> Option<String> {
    match obj {
        Object::Name(bytes) => Some(String::from_utf8_lossy(bytes).to_string()),
        _ => None,
    }
}

pub(crate) fn deref<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Object> {
    match obj {
        Object::Reference(id) => doc.get_object(*id).ok(),
        other => Some(other),
    }
}

pub(crate) fn dict_of<'a>(doc: &'a Document, obj: &'a Object) -> Option<&'a Dictionary> {
    match deref(doc, obj)? {
        Object::Dictionary(dict) => Some(dict),
        _ => None,
    }
}

pub(crate) fn inherited<'a>(doc: &'a Document, start: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut current = Some(start);
    let mut hops = 0;
    while let Some(id) = current {
        hops += 1;
        if hops > 64 {
            break;
        }
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(value) = dict.get(key) {
            return deref(doc, value);
        }
        current = dict.get(b"Parent").ok().and_then(|p| p.as_reference().ok());
    }
    None
}

pub(crate) fn media_box(doc: &Document, page_id: ObjectId) -> (f64, f64, f64, f64) {
    if let Some(Object::Array(values)) = inherited(doc, page_id, b"MediaBox") {
        let n: Vec<f64> = values.iter().filter_map(number).collect();
        if n.len() >= 4 {
            return (n[0], n[1], n[2], n[3]);
        }
    }
    (0.0, 0.0, 595.0, 842.0)
}

pub(crate) fn page_rotation(doc: &Document, page_id: ObjectId) -> i64 {
    inherited(doc, page_id, b"Rotate")
        .and_then(number)
        .unwrap_or(0.0) as i64
}

/// Converte un rettangolo normalizzato (origine in alto a sinistra, dopo la
/// rotazione di visualizzazione) nello spazio utente del PDF.
pub(crate) fn norm_to_user(rect: &[f32; 4], rotation: i64, media: (f64, f64, f64, f64)) -> Rect {
    let (mx0, my0, mx1, my1) = media;
    let width = mx1 - mx0;
    let height = my1 - my0;
    let nx = rect[0] as f64;
    let ny = rect[1] as f64;
    let nw = rect[2] as f64;
    let nh = rect[3] as f64;

    let mut points = Vec::with_capacity(4);
    for (dx, dy) in [(0.0, 0.0), (nw, 0.0), (0.0, nh), (nw, nh)] {
        let du = nx + dx;
        let dv = ny + dy;
        let (u, v) = match rotation.rem_euclid(360) {
            90 => (dv, du),
            180 => (1.0 - du, dv),
            270 => (1.0 - dv, 1.0 - du),
            _ => (du, 1.0 - dv),
        };
        points.push((mx0 + u * width, my0 + v * height));
    }
    Rect::from_points(&points).unwrap_or(Rect {
        x0: 0.0,
        y0: 0.0,
        x1: 0.0,
        y1: 0.0,
    })
}

fn read_font_metrics(doc: &Document, font: &Dictionary) -> FontMetrics {
    let subtype = font.get(b"Subtype").ok().and_then(name_of).unwrap_or_default();
    let two_byte = subtype == "Type0";

    if two_byte {
        let mut default_width = 1000.0;
        let mut widths: Vec<f64> = Vec::new();
        if let Some(descendant) = font.get(b"DescendantFonts").ok().and_then(|list| match deref(doc, list) {
            Some(Object::Array(items)) => items.first().and_then(|item| dict_of(doc, item)),
            _ => None,
        }) {
            if let Some(value) = descendant.get(b"DW").ok().and_then(number) {
                default_width = value;
            }
            if let Some(Object::Array(w)) = descendant.get(b"W").ok() {
                let mut index = 0usize;
                while index < w.len() {
                    let first = number(&w[index]).unwrap_or(0.0) as usize;
                    let next = w.get(index + 1);
                    match next {
                        Some(Object::Array(list)) => {
                            for (offset, value) in list.iter().enumerate() {
                                let position = first + offset;
                                if position >= widths.len() {
                                    widths.resize(position + 1, default_width);
                                }
                                widths[position] = number(value).unwrap_or(default_width);
                            }
                            index += 2;
                        }
                        Some(value) if w.get(index + 2).is_some() => {
                            let last = number(value).unwrap_or(first as f64) as usize;
                            let width = number(&w[index + 2]).unwrap_or(default_width);
                            if last >= widths.len() {
                                widths.resize(last + 1, default_width);
                            }
                            for position in first..=last {
                                widths[position] = width;
                            }
                            index += 3;
                        }
                        _ => index += 1,
                    }
                }
            }
        }
        return FontMetrics {
            first_char: 0,
            widths,
            default_width,
            two_byte: true,
        };
    }

    let first_char = font.get(b"FirstChar").ok().and_then(number).unwrap_or(0.0) as i64;
    let default_width = font
        .get(b"MissingWidth")
        .ok()
        .and_then(number)
        .or_else(|| {
            let descriptor = font.get(b"FontDescriptor").ok().and_then(|d| dict_of(doc, d))?;
            descriptor.get(b"MissingWidth").ok().and_then(number)
        })
        .unwrap_or(500.0);
    let widths = match font.get(b"Widths").ok() {
        Some(Object::Array(values)) => values
            .iter()
            .map(|v| number(v).unwrap_or(default_width))
            .collect(),
        _ => Vec::new(),
    };
    FontMetrics {
        first_char,
        widths,
        default_width,
        two_byte: false,
    }
}

pub(crate) fn fonts_for(doc: &Document, resources: Option<&Dictionary>) -> FontMap {
    let mut map = FontMap::new();
    let Some(resources) = resources else {
        return map;
    };
    let Some(fonts) = resources.get(b"Font").ok().and_then(|f| dict_of(doc, f)) else {
        return map;
    };
    for (name, value) in fonts.iter() {
        let Some(id) = value.as_reference().ok() else {
            continue;
        };
        if let Ok(font) = doc.get_dictionary(id) {
            map.insert(
                String::from_utf8_lossy(name).to_string(),
                read_font_metrics(doc, font),
            );
        }
    }
    map
}

pub(crate) fn decode_codes(bytes: &[u8], two_byte: bool) -> Vec<u32> {
    if two_byte {
        bytes
            .chunks(2)
            .map(|chunk| {
                if chunk.len() == 2 {
                    ((chunk[0] as u32) << 8) | chunk[1] as u32
                } else {
                    chunk[0] as u32
                }
            })
            .collect()
    } else {
        bytes.iter().map(|b| *b as u32).collect()
    }
}

pub(crate) fn text_advance(object: &Object, state: &TextState, fonts: &FontMap) -> (f64, usize) {
    let metrics = state.font.as_ref().and_then(|name| fonts.get(name));
    let two_byte = metrics.map(|m| m.two_byte).unwrap_or(false);
    let font_size = state.font_size;
    let mut width = 0.0;
    let mut count = 0usize;

    let add_string = |bytes: &[u8], width: &mut f64, count: &mut usize| {
        for code in decode_codes(bytes, two_byte) {
            let unit = metrics.map(|m| m.width_of(code)).unwrap_or(500.0);
            *width += unit / 1000.0 * font_size * state.h_scale + state.char_spacing;
            if code == 32 {
                *width += state.word_spacing * state.h_scale;
            }
            *count += 1;
        }
    };

    match object {
        Object::String(bytes, _) => add_string(bytes, &mut width, &mut count),
        Object::Array(items) => {
            for item in items {
                match item {
                    Object::String(bytes, _) => add_string(bytes, &mut width, &mut count),
                    other => {
                        if let Some(adjust) = number(other) {
                            width -= adjust / 1000.0 * font_size * state.h_scale;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    (width.max(0.0), count)
}

pub(crate) fn text_bbox(tm: &Matrix, ctm: &Matrix, width: f64, font_size: f64) -> Option<Rect> {
    let ascent = font_size * 0.85;
    let descent = -font_size * 0.25;
    let mut points = Vec::with_capacity(4);
    for (x, y) in [(0.0, descent), (width, descent), (0.0, ascent), (width, ascent)] {
        let (tx, ty) = apply(tm, x, y);
        points.push(apply(ctm, tx, ty));
    }
    Rect::from_points(&points)
}

fn bbox_of_rect_array(values: &[Object]) -> Option<Rect> {
    let n: Vec<f64> = values.iter().filter_map(number).collect();
    if n.len() < 4 {
        return None;
    }
    Rect::from_points(&[(n[0], n[1]), (n[2], n[3])])
}

fn extend(bbox: &mut Option<Rect>, points: &[(f64, f64)]) {
    let Some(extent) = Rect::from_points(points) else {
        return;
    };
    *bbox = Some(match *bbox {
        None => extent,
        Some(existing) => Rect {
            x0: existing.x0.min(extent.x0),
            y0: existing.y0.min(extent.y0),
            x1: existing.x1.max(extent.x1),
            y1: existing.y1.max(extent.y1),
        },
    });
}

fn int_of(obj: &Object) -> Option<i64> {
    match obj {
        Object::Integer(v) => Some(*v),
        Object::Real(v) => Some(*v as i64),
        _ => None,
    }
}

/// Rettangolo in coordinate immagine: origine in alto a sinistra, riga 0 = prima
/// riga di campioni del flusso.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PixelRect {
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
}

impl PixelRect {
    fn width(&self) -> u32 {
        self.x1 - self.x0
    }

    fn height(&self) -> u32 {
        self.y1 - self.y0
    }

    fn covers_all(&self, width: u32, height: u32) -> bool {
        self.x0 == 0 && self.y0 == 0 && self.x1 >= width && self.y1 >= height
    }
}

const UNIT_SQUARE: Rect = Rect {
    x0: 0.0,
    y0: 0.0,
    x1: 1.0,
    y1: 1.0,
};

/// Campione decodificato di un XObject immagine.
enum Sample {
    /// Campioni grezzi (8 bit per componente) e numero di componenti per pixel.
    Raw { data: Vec<u8>, components: usize },
    /// Immagine JPEG già decodificata.
    Jpeg(image::DynamicImage),
}

fn color_components(doc: &Document, dict: &Dictionary) -> Option<usize> {
    let space = deref(doc, dict.get(b"ColorSpace").ok()?)?;
    match space {
        Object::Name(name) => match name.as_slice() {
            b"DeviceGray" | b"CalGray" => Some(1),
            b"DeviceRGB" | b"CalRGB" => Some(3),
            b"DeviceCMYK" => Some(4),
            _ => None,
        },
        Object::Array(items) => {
            let kind = items.first().and_then(name_of)?;
            match kind.as_str() {
                "CalGray" => Some(1),
                "CalRGB" => Some(3),
                "ICCBased" => {
                    let stream = deref(doc, items.get(1)?)?.as_stream().ok()?;
                    stream.dict.get(b"N").ok().and_then(int_of).map(|n| n as usize)
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Decodifica il campione dell'immagine nei soli formati che sappiamo
/// ricodificare senza alterare i colori.
fn decode_samples(doc: &Document, stream: &Stream) -> Result<Sample, String> {
    let dict = &stream.dict;
    if matches!(dict.get(b"ImageMask"), Ok(Object::Boolean(true))) {
        return Err("immagine stencil non ritagliabile".to_string());
    }
    if dict.has(b"SMask") || dict.has(b"Mask") {
        return Err("immagine con canale di trasparenza".to_string());
    }
    let width = dict.get(b"Width").ok().and_then(int_of).unwrap_or(0);
    let height = dict.get(b"Height").ok().and_then(int_of).unwrap_or(0);
    if width <= 0 || height <= 0 {
        return Err("dimensioni dell'immagine non valide".to_string());
    }
    let filters: Vec<String> = stream
        .filters()
        .map(|list| {
            list.iter()
                .map(|filter| String::from_utf8_lossy(filter).to_string())
                .collect()
        })
        .unwrap_or_default();
    match filters.last().map(String::as_str) {
        Some("JPXDecode") => Err("immagine JPEG 2000 non supportata".to_string()),
        Some("CCITTFaxDecode") => Err("immagine CCITT non supportata".to_string()),
        Some("JBIG2Decode") => Err("immagine JBIG2 non supportata".to_string()),
        Some("DCTDecode") => {
            if filters.len() != 1 {
                return Err("catena di filtri JPEG non supportata".to_string());
            }
            if dict.has(b"Decode") {
                return Err("immagine JPEG con /Decode personalizzato".to_string());
            }
            let pixels = image::load_from_memory(&stream.content)
                .map_err(|e| format!("Immagine JPEG non decodificabile: {e}"))?;
            if pixels.width() as i64 != width || pixels.height() as i64 != height {
                return Err("dimensioni JPEG incoerenti con il dizionario".to_string());
            }
            match pixels.color() {
                image::ColorType::L8 | image::ColorType::Rgb8 => Ok(Sample::Jpeg(pixels)),
                other => Err(format!("spazio colore JPEG non supportato: {other:?}")),
            }
        }
        _ => {
            let components = color_components(doc, dict)
                .ok_or_else(|| "spazio colore non supportato".to_string())?;
            let bits = dict
                .get(b"BitsPerComponent")
                .ok()
                .and_then(int_of)
                .unwrap_or(8);
            if bits != 8 {
                return Err(format!("{bits} bit per componente non supportati"));
            }
            let expected = width as usize * height as usize * components;
            let data = stream
                .decompressed_content_with_limit(
                    expected.saturating_add(expected / 16).saturating_add(65_536),
                )
                .map_err(|e| format!("Campione dell'immagine non decodificabile: {e}"))?;
            if data.len() < expected {
                return Err(format!(
                    "campione incompleto: {} byte invece di {expected}",
                    data.len()
                ));
            }
            Ok(Sample::Raw { data, components })
        }
    }
}

/// Sotto-rettangoli non coperti: sopra, sotto, sinistra e destra.
fn crop_rects(cover: PixelRect, width: u32, height: u32) -> Vec<PixelRect> {
    let mut rects = Vec::with_capacity(4);
    let mut push = |x0: u32, y0: u32, x1: u32, y1: u32| {
        let x0 = x0.min(width);
        let y0 = y0.min(height);
        let x1 = x1.min(width);
        let y1 = y1.min(height);
        if x1 > x0 && y1 > y0 {
            rects.push(PixelRect { x0, y0, x1, y1 });
        }
    };
    push(0, 0, width, cover.y0);
    push(0, cover.y1, width, height);
    push(0, cover.y0, cover.x0, cover.y1);
    push(cover.x1, cover.y0, width, cover.y1);
    rects
}

/// Area coperta dalle redazioni, espressa in pixel dell'immagine.
fn covered_pixels(ctm: &Matrix, areas: &[Rect], width: u32, height: u32) -> Option<PixelRect> {
    let inverse = invert(ctm)?;
    let mut unit: Option<Rect> = None;
    for area in areas {
        let corners = [
            apply(&inverse, area.x0, area.y0),
            apply(&inverse, area.x1, area.y0),
            apply(&inverse, area.x0, area.y1),
            apply(&inverse, area.x1, area.y1),
        ];
        let Some(mapped) = Rect::from_points(&corners) else {
            continue;
        };
        let clipped = Rect {
            x0: mapped.x0.max(0.0),
            y0: mapped.y0.max(0.0),
            x1: mapped.x1.min(1.0),
            y1: mapped.y1.min(1.0),
        };
        if clipped.x1 <= clipped.x0 || clipped.y1 <= clipped.y0 {
            continue;
        }
        unit = Some(match unit {
            None => clipped,
            Some(existing) => Rect {
                x0: existing.x0.min(clipped.x0),
                y0: existing.y0.min(clipped.y0),
                x1: existing.x1.max(clipped.x1),
                y1: existing.y1.max(clipped.y1),
            },
        });
    }
    let unit = unit?;
    let w = width as f64;
    let h = height as f64;
    let x0 = (unit.x0 * w).floor().clamp(0.0, w) as u32;
    let x1 = (unit.x1 * w).ceil().clamp(0.0, w) as u32;
    let y0 = ((1.0 - unit.y1) * h).floor().clamp(0.0, h) as u32;
    let y1 = ((1.0 - unit.y0) * h).ceil().clamp(0.0, h) as u32;
    if x1 <= x0 || y1 <= y0 {
        None
    } else {
        Some(PixelRect { x0, y0, x1, y1 })
    }
}

/// Costruisce il nuovo XObject immagine con i soli pixel del ritaglio.
fn build_crop_stream(
    base: &Dictionary,
    sample: &Sample,
    rect: PixelRect,
    width: u32,
) -> Result<Stream, String> {
    let mut dict = base.clone();
    for key in [
        b"Filter".as_slice(),
        b"DecodeParms",
        b"DL",
        b"Metadata",
        b"PieceInfo",
        b"StructParent",
        b"SMask",
        b"Mask",
        b"Name",
        b"Alternates",
        b"OPI",
        b"PostScript",
    ] {
        dict.remove(key);
    }
    dict.set("Type", Object::Name(b"XObject".to_vec()));
    dict.set("Subtype", Object::Name(b"Image".to_vec()));
    dict.set("Width", rect.width() as i64);
    dict.set("Height", rect.height() as i64);
    dict.set("BitsPerComponent", 8i64);

    match sample {
        Sample::Raw { data, components } => {
            let components = *components;
            let stride = width as usize * components;
            let mut pixels =
                Vec::with_capacity(rect.width() as usize * rect.height() as usize * components);
            for row in rect.y0..rect.y1 {
                let start = row as usize * stride + rect.x0 as usize * components;
                let end = start + rect.width() as usize * components;
                if end > data.len() {
                    return Err("campione dell'immagine troncato".to_string());
                }
                pixels.extend_from_slice(&data[start..end]);
            }
            let mut stream = Stream::new(dict, pixels);
            stream
                .compress()
                .map_err(|e| format!("Compressione del ritaglio non riuscita: {e}"))?;
            Ok(stream)
        }
        Sample::Jpeg(pixels) => {
            let gray = pixels.color() == image::ColorType::L8;
            let cropped = pixels.crop_imm(rect.x0, rect.y0, rect.width(), rect.height());
            dict.remove(b"Decode");
            dict.set(
                "ColorSpace",
                Object::Name(if gray {
                    b"DeviceGray".to_vec()
                } else {
                    b"DeviceRGB".to_vec()
                }),
            );
            let (buffer, color) = if gray {
                (
                    cropped.into_luma8().into_raw(),
                    image::ExtendedColorType::L8,
                )
            } else {
                (
                    cropped.into_rgb8().into_raw(),
                    image::ExtendedColorType::Rgb8,
                )
            };
            let mut bytes = Vec::new();
            let mut encoder = JpegEncoder::new_with_quality(&mut bytes, 90);
            encoder
                .encode(&buffer, rect.width(), rect.height(), color)
                .map_err(|e| format!("Ricodifica JPEG non riuscita: {e}"))?;
            dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
            Ok(Stream::new(dict, bytes).with_compression(false))
        }
    }
}

/// Riscrittura di un Form XObject modificato sul posto.
struct FormEdit {
    content: Vec<u8>,
    resources: Option<Dictionary>,
}

struct ProcessOutput {
    ops: Vec<Operation>,
    /// Dizionario `/Resources` aggiornato: presente solo se sono stati creati
    /// nuovi XObject (immagini ritagliate o cloni di moduli).
    resources: Option<Dictionary>,
}

struct Processor<'a> {
    doc: &'a Document,
    report: RedactReport,
    modified_forms: &'a mut HashMap<ObjectId, FormEdit>,
    touched_forms: &'a mut HashSet<ObjectId>,
    /// Oggetti sostituiti da una copia redatta: se non restano raggiungibili
    /// vengono svuotati a fine lavorazione.
    replaced: &'a mut HashSet<ObjectId>,
    pending: Vec<(ObjectId, Object)>,
    next_id: u32,
    name_seq: usize,
}

impl<'a> Processor<'a> {
    fn alloc_id(&mut self) -> ObjectId {
        self.next_id += 1;
        (self.next_id, 0)
    }

    fn unique_name(&mut self, xobjects: &Dictionary, prefix: &str) -> String {
        loop {
            self.name_seq += 1;
            let name = format!("{prefix}{}", self.name_seq);
            if !xobjects.has(name.as_bytes()) {
                return name;
            }
        }
    }
}

impl<'a> Processor<'a> {
    fn process(
        &mut self,
        ops: Vec<Operation>,
        areas: &[Rect],
        resources: Option<&Dictionary>,
        fonts: &FontMap,
        depth: usize,
    ) -> Result<ProcessOutput, String> {
        let usage = count_form_usage(self.doc, &ops, resources);
        let mut xobjects: Dictionary = resources
            .and_then(|r| r.get(b"XObject").ok())
            .and_then(|x| dict_of(self.doc, x))
            .cloned()
            .unwrap_or_default();
        let mut resources_dirty = false;
        let mut out: Vec<Operation> = Vec::with_capacity(ops.len() + 8);
        let mut stack: Vec<GState> = Vec::new();
        let mut state = GState::default();
        let mut tm = IDENTITY;
        let mut tlm = IDENTITY;
        let mut path_bbox: Option<Rect> = None;

        for op in ops {
            match op.operator.as_str() {
                "q" => {
                    stack.push(state.clone());
                    out.push(op);
                }
                "Q" => {
                    if let Some(previous) = stack.pop() {
                        state = previous;
                    }
                    out.push(op);
                }
                "cm" => {
                    let values: Vec<f64> = op.operands.iter().filter_map(number).collect();
                    if values.len() == 6 {
                        let m = [
                            values[0], values[1], values[2], values[3], values[4], values[5],
                        ];
                        state.ctm = concat(&m, &state.ctm);
                    }
                    out.push(op);
                }
                "BT" => {
                    tm = IDENTITY;
                    tlm = IDENTITY;
                    out.push(op);
                }
                "Tf" => {
                    if let Some(name) = op.operands.first().and_then(name_of) {
                        state.text.font = Some(name);
                    }
                    if let Some(size) = op.operands.get(1).and_then(number) {
                        state.text.font_size = size;
                    }
                    out.push(op);
                }
                "TL" => {
                    if let Some(value) = op.operands.first().and_then(number) {
                        state.text.leading = value;
                    }
                    out.push(op);
                }
                "Tc" => {
                    if let Some(value) = op.operands.first().and_then(number) {
                        state.text.char_spacing = value;
                    }
                    out.push(op);
                }
                "Tw" => {
                    if let Some(value) = op.operands.first().and_then(number) {
                        state.text.word_spacing = value;
                    }
                    out.push(op);
                }
                "Tz" => {
                    if let Some(value) = op.operands.first().and_then(number) {
                        state.text.h_scale = value / 100.0;
                    }
                    out.push(op);
                }
                "Tm" => {
                    let values: Vec<f64> = op.operands.iter().filter_map(number).collect();
                    if values.len() == 6 {
                        tm = [
                            values[0], values[1], values[2], values[3], values[4], values[5],
                        ];
                        tlm = tm;
                    }
                    out.push(op);
                }
                "Td" | "TD" => {
                    let tx = op.operands.first().and_then(number).unwrap_or(0.0);
                    let ty = op.operands.get(1).and_then(number).unwrap_or(0.0);
                    tlm = concat(&[1.0, 0.0, 0.0, 1.0, tx, ty], &tlm);
                    tm = tlm;
                    if op.operator == "TD" {
                        state.text.leading = -ty;
                    }
                    out.push(op);
                }
                "T*" => {
                    tlm = concat(&[1.0, 0.0, 0.0, 1.0, 0.0, -state.text.leading], &tlm);
                    tm = tlm;
                    out.push(op);
                }
                "Tj" | "TJ" | "'" | "\"" => {
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
                        out.push(op);
                        continue;
                    };
                    let (width, count) = text_advance(operand, &state.text, fonts);
                    let bbox = text_bbox(&tm, &state.ctm, width, state.text.font_size);
                    let hit = bbox
                        .map(|b| areas.iter().any(|area| area.intersects(&b)))
                        .unwrap_or(false);
                    if hit {
                        self.report.texts += count.max(1);
                    } else {
                        out.push(op.clone());
                    }
                    tm = concat(&[1.0, 0.0, 0.0, 1.0, width, 0.0], &tm);
                }
                "m" | "l" => {
                    let x = op.operands.first().and_then(number).unwrap_or(0.0);
                    let y = op.operands.get(1).and_then(number).unwrap_or(0.0);
                    let point = apply(&state.ctm, x, y);
                    extend(&mut path_bbox, &[point]);
                    out.push(op);
                }
                "c" | "v" | "y" => {
                    let values: Vec<f64> = op.operands.iter().filter_map(number).collect();
                    let mut points = vec![];
                    for pair in values.chunks(2) {
                        if pair.len() == 2 {
                            points.push(apply(&state.ctm, pair[0], pair[1]));
                        }
                    }
                    extend(&mut path_bbox, &points);
                    out.push(op);
                }
                "re" => {
                    let values: Vec<f64> = op.operands.iter().filter_map(number).collect();
                    if values.len() == 4 {
                        let (x, y) = apply(&state.ctm, values[0], values[1]);
                        let (x2, y2) =
                            apply(&state.ctm, values[0] + values[2], values[1] + values[3]);
                        extend(&mut path_bbox, &[(x, y), (x2, y2)]);
                    }
                    out.push(op);
                }
                "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                    let hit = path_bbox
                        .map(|b| areas.iter().any(|area| area.intersects(&b)))
                        .unwrap_or(false);
                    if hit {
                        self.report.paths += 1;
                    } else {
                        out.push(op);
                    }
                    path_bbox = None;
                }
                "n" => {
                    path_bbox = None;
                    out.push(op);
                }
                "BI" => {
                    // Immagine inline: occupa il quadrato unitario trasformato dalla CTM.
                    let bbox = map_rect(
                        &Rect {
                            x0: 0.0,
                            y0: 0.0,
                            x1: 1.0,
                            y1: 1.0,
                        },
                        &state.ctm,
                    );
                    let hit = bbox
                        .map(|b| areas.iter().any(|area| area.intersects(&b)))
                        .unwrap_or(false);
                    if hit {
                        self.report.images += 1;
                    } else {
                        out.push(op);
                    }
                }
                "Do" => {
                    let Some(name) = op.operands.first().and_then(name_of) else {
                        out.push(op);
                        continue;
                    };
                    let entry = xobjects.get(name.as_bytes()).ok().cloned();
                    let Some(xobject) = entry.as_ref().and_then(|v| v.as_reference().ok()) else {
                        out.push(op);
                        continue;
                    };
                    let doc = self.doc;
                    let Ok(object) = doc.get_object(xobject) else {
                        out.push(op);
                        continue;
                    };
                    let Some(Object::Stream(stream)) = deref(doc, object) else {
                        out.push(op);
                        continue;
                    };
                    let subtype = stream
                        .dict
                        .get(b"Subtype")
                        .ok()
                        .and_then(name_of)
                        .unwrap_or_default();
                    match subtype.as_str() {
                        "Image" => {
                            let bbox = map_rect(&UNIT_SQUARE, &state.ctm);
                            let hit = bbox
                                .map(|b| areas.iter().any(|area| area.intersects(&b)))
                                .unwrap_or(false);
                            if !hit {
                                out.push(op);
                                continue;
                            }
                            let width = stream.dict.get(b"Width").ok().and_then(int_of).unwrap_or(0);
                            let height = stream.dict.get(b"Height").ok().and_then(int_of).unwrap_or(0);
                            if width <= 0 || height <= 0 {
                                out.push(op);
                                continue;
                            }
                            let (width, height) = (width as u32, height as u32);
                            let Some(cover) = covered_pixels(&state.ctm, areas, width, height) else {
                                out.push(op);
                                continue;
                            };
                            if cover.covers_all(width, height) {
                                self.report.images += 1;
                                self.replaced.insert(xobject);
                                continue;
                            }
                            match self.crop_image(stream, &cover, width, height, &mut xobjects) {
                                Ok(pieces) if !pieces.is_empty() => {
                                    self.report.cropped += 1;
                                    self.replaced.insert(xobject);
                                    resources_dirty = true;
                                    for (piece, matrix) in pieces {
                                        out.push(Operation::new("q", vec![]));
                                        out.push(Operation::new(
                                            "cm",
                                            matrix
                                                .iter()
                                                .map(|value| Object::Real(*value as f32))
                                                .collect(),
                                        ));
                                        out.push(Operation::new(
                                            "Do",
                                            vec![Object::Name(piece.into_bytes())],
                                        ));
                                        out.push(Operation::new("Q", vec![]));
                                    }
                                }
                                _ => {
                                    self.report.images += 1;
                                    self.replaced.insert(xobject);
                                }
                            }
                        }
                        "Form" => {
                            let uses = usage.get(&xobject).copied().unwrap_or(0);
                            let clone = uses > 1 || self.touched_forms.contains(&xobject);
                            match self.process_form(
                                xobject,
                                stream,
                                areas,
                                &state.ctm,
                                depth,
                                clone,
                                &mut xobjects,
                            )? {
                                Some(new_name) => {
                                    resources_dirty = true;
                                    out.push(Operation::new(
                                        "Do",
                                        vec![Object::Name(new_name.into_bytes())],
                                    ));
                                }
                                None => out.push(op),
                            }
                        }
                        _ => out.push(op),
                    }
                }
                _ => out.push(op),
            }
        }

        if !self.replaced.is_empty() {
            let mut used: HashSet<String> = HashSet::new();
            for operation in &out {
                if operation.operator == "Do" {
                    if let Some(name) = operation.operands.first().and_then(name_of) {
                        used.insert(name);
                    }
                }
            }
            let stale: Vec<Vec<u8>> = xobjects
                .iter()
                .filter_map(|(name, value)| {
                    let id = value.as_reference().ok()?;
                    let name = String::from_utf8_lossy(name).to_string();
                    if self.replaced.contains(&id) && !used.contains(&name) {
                        Some(name.into_bytes())
                    } else {
                        None
                    }
                })
                .collect();
            for name in stale {
                xobjects.remove(&name);
                resources_dirty = true;
            }
        }

        let resources = if resources_dirty {
            let mut updated = resources.cloned().unwrap_or_default();
            updated.set("XObject", Object::Dictionary(xobjects));
            Some(updated)
        } else {
            None
        };
        Ok(ProcessOutput { ops: out, resources })
    }

    /// Crea una copia ritagliata del campione per ogni sotto-rettangolo non
    /// coperto e restituisce nome risorsa e matrice `cm` locale.
    fn crop_image(
        &mut self,
        stream: &Stream,
        cover: &PixelRect,
        width: u32,
        height: u32,
        xobjects: &mut Dictionary,
    ) -> Result<Vec<(String, Matrix)>, String> {
        let sample = decode_samples(self.doc, stream)?;
        let mut prepared = Vec::new();
        for rect in crop_rects(*cover, width, height) {
            prepared.push((build_crop_stream(&stream.dict, &sample, rect, width)?, rect));
        }
        let mut pieces = Vec::with_capacity(prepared.len());
        for (cropped, rect) in prepared {
            let id = self.alloc_id();
            self.pending.push((id, Object::Stream(cropped)));
            let name = self.unique_name(xobjects, "RedactImg");
            xobjects.set(name.as_bytes().to_vec(), Object::Reference(id));
            let local = [
                rect.width() as f64 / width as f64,
                0.0,
                0.0,
                rect.height() as f64 / height as f64,
                rect.x0 as f64 / width as f64,
                1.0 - rect.y1 as f64 / height as f64,
            ];
            pieces.push((name, local));
        }
        Ok(pieces)
    }

    fn process_form(
        &mut self,
        id: ObjectId,
        stream: &Stream,
        areas: &[Rect],
        ctm: &Matrix,
        depth: usize,
        clone: bool,
        xobjects: &mut Dictionary,
    ) -> Result<Option<String>, String> {
        if depth >= 4 {
            return Ok(None);
        }
        let matrix = stream
            .dict
            .get(b"Matrix")
            .ok()
            .and_then(|m| deref(self.doc, m))
            .and_then(|m| m.as_array().ok())
            .map(|values| {
                let n: Vec<f64> = values.iter().filter_map(number).collect();
                if n.len() == 6 {
                    [n[0], n[1], n[2], n[3], n[4], n[5]]
                } else {
                    IDENTITY
                }
            })
            .unwrap_or(IDENTITY);
        let effective = concat(&matrix, ctm);
        let Some(inverse) = invert(&effective) else {
            return Ok(None);
        };
        let mapped: Vec<Rect> = areas.iter().filter_map(|a| map_rect(a, &inverse)).collect();
        if mapped.is_empty() {
            return Ok(None);
        }
        let data = stream
            .decompressed_content()
            .map_err(|e| format!("Form XObject non leggibile: {e}"))?;
        let content = Content::decode(&data)
            .map_err(|e| format!("Form XObject non interpretabile: {e}"))?;
        let form_resources = stream
            .dict
            .get(b"Resources")
            .ok()
            .and_then(|r| dict_of(self.doc, r));
        let form_fonts = fonts_for(self.doc, form_resources);
        let processed = self.process(
            content.operations,
            &mapped,
            form_resources,
            &form_fonts,
            depth + 1,
        )?;
        let encoded = Content {
            operations: processed.ops,
        }
        .encode()
        .map_err(|e| format!("Form XObject non serializzabile: {e}"))?;

        if !clone {
            self.modified_forms.insert(
                id,
                FormEdit {
                    content: encoded,
                    resources: processed.resources,
                },
            );
            self.touched_forms.insert(id);
            return Ok(None);
        }

        let new_id = self.alloc_id();
        let mut dict = stream.dict.clone();
        dict.remove(b"Filter");
        dict.remove(b"DecodeParms");
        dict.remove(b"DL");
        if let Some(resources) = processed.resources {
            dict.set("Resources", Object::Dictionary(resources));
        }
        self.pending
            .push((new_id, Object::Stream(Stream::new(dict, encoded))));
        self.replaced.insert(id);
        let name = self.unique_name(xobjects, "RedactForm");
        xobjects.set(name.as_bytes().to_vec(), Object::Reference(new_id));
        Ok(Some(name))
    }
}

pub(crate) fn page_resources<'a>(doc: &'a Document, page_id: ObjectId) -> Option<&'a Dictionary> {
    inherited(doc, page_id, b"Resources").and_then(|r| dict_of(doc, r))
}

pub(crate) fn page_content_ids(doc: &Document, page_id: ObjectId) -> Vec<ObjectId> {
    doc.get_page_contents(page_id)
}

pub(crate) fn content_bytes(doc: &Document, ids: &[ObjectId]) -> Result<Vec<u8>, String> {
    let mut data = Vec::new();
    for id in ids {
        let stream = doc
            .get_object(*id)
            .ok()
            .and_then(|o| o.as_stream().ok())
            .ok_or_else(|| "Content stream non trovato.".to_string())?;
        data.extend(
            stream
                .decompressed_content()
                .map_err(|e| format!("Content stream non decomprimibile: {e}"))?,
        );
        data.push(b'\n');
    }
    Ok(data)
}

fn count_form_usage(
    doc: &Document,
    ops: &[Operation],
    resources: Option<&Dictionary>,
) -> HashMap<ObjectId, usize> {
    let xobjects = resources
        .and_then(|r| r.get(b"XObject").ok())
        .and_then(|x| dict_of(doc, x));
    let mut usage = HashMap::new();
    for op in ops {
        if op.operator != "Do" {
            continue;
        }
        let Some(name) = op.operands.first().and_then(name_of) else {
            continue;
        };
        let Some(id) = xobjects
            .and_then(|x| x.get(name.as_bytes()).ok())
            .and_then(|v| v.as_reference().ok())
        else {
            continue;
        };
        *usage.entry(id).or_insert(0) += 1;
    }
    usage
}

fn add_black_rects(doc: &mut Document, page_id: ObjectId, areas: &[Rect], base: Vec<ObjectId>) {
    if areas.is_empty() {
        return;
    }
    let mut ops = Vec::with_capacity(areas.len() * 4);
    for area in areas {
        ops.push(Operation::new("q", vec![]));
        ops.push(Operation::new(
            "rg",
            vec![Object::Real(0.0), Object::Real(0.0), Object::Real(0.0)],
        ));
        ops.push(Operation::new(
            "re",
            vec![
                Object::Real(area.x0 as f32),
                Object::Real(area.y0 as f32),
                Object::Real((area.x1 - area.x0).max(0.0) as f32),
                Object::Real((area.y1 - area.y0).max(0.0) as f32),
            ],
        ));
        ops.push(Operation::new("f", vec![]));
        ops.push(Operation::new("Q", vec![]));
    }
    let Ok(bytes) = (Content { operations: ops }).encode() else {
        return;
    };
    let mut dict = Dictionary::new();
    dict.set("Length", bytes.len() as i64);
    let new_id = doc.add_object(Object::Stream(Stream::new(dict, bytes)));
    let mut contents = base
        .into_iter()
        .map(Object::Reference)
        .collect::<Vec<Object>>();
    contents.push(Object::Reference(new_id));
    if let Ok(page) = doc.get_dictionary_mut(page_id) {
        page.set("Contents", Object::Array(contents));
    }
}

fn remove_annotations(doc: &mut Document, page_id: ObjectId, areas: &[Rect], report: &mut RedactReport) {
    let annots = match doc.get_dictionary(page_id).ok().and_then(|p| p.get(b"Annots").ok().cloned()) {
        Some(annots) => annots,
        None => return,
    };
    let list = match annots {
        Object::Array(items) => items,
        Object::Reference(id) => match doc.get_object(id) {
            Ok(Object::Array(items)) => items.clone(),
            _ => return,
        },
        _ => return,
    };
    let mut keep = Vec::with_capacity(list.len());
    let mut removed = 0usize;
    for item in list {
        let keep_item = match item.as_reference() {
            Ok(id) => match doc.get_dictionary(id) {
                Ok(dict) => {
                    let rect = dict
                        .get(b"Rect")
                        .ok()
                        .and_then(|r| r.as_array().ok())
                        .and_then(|values| bbox_of_rect_array(values));
                    match rect {
                        Some(rect) => {
                            let hit = areas.iter().any(|area| area.intersects(&rect));
                            if hit {
                                removed += 1;
                            }
                            !hit
                        }
                        None => true,
                    }
                }
                Err(_) => true,
            },
            Err(_) => true,
        };
        if keep_item {
            keep.push(item);
        }
    }
    if removed > 0 {
        report.annotations += removed;
        if let Ok(page) = doc.get_dictionary_mut(page_id) {
            page.set("Annots", Object::Array(keep));
        }
    }
}

fn strip_metadata(doc: &mut Document) {
    let info = doc.trailer.get(b"Info").ok().and_then(|o| o.as_reference().ok());
    if let Some(id) = info {
        let _ = doc.delete_object(id);
    }
    doc.trailer.remove(b"Info");

    let Some(root_id) = doc.trailer.get(b"Root").ok().and_then(|o| o.as_reference().ok()) else {
        return;
    };
    let mut to_delete = Vec::new();
    if let Ok(root) = doc.get_dictionary(root_id) {
        for key in [b"Metadata".as_slice(), b"PieceInfo", b"OpenAction", b"AA"] {
            if let Ok(Object::Reference(id)) = root.get(key) {
                to_delete.push(*id);
            }
        }
    }
    for id in to_delete {
        let _ = doc.delete_object(id);
    }
    if let Ok(root) = doc.get_dictionary_mut(root_id) {
        for key in [
            b"Metadata".as_slice(),
            b"StructTreeRoot",
            b"MarkInfo",
            b"PieceInfo",
            b"OpenAction",
            b"AA",
        ] {
            root.remove(key);
        }
        if let Ok(names) = root.get_mut(b"Names") {
            if let Ok(names) = names.as_dict_mut() {
                names.remove(b"EmbeddedFiles");
            }
        }
    }
    for (_, page_id) in doc.get_pages() {
        if let Ok(page) = doc.get_dictionary_mut(page_id) {
            page.remove(b"Thumb");
            page.remove(b"AA");
        }
    }
}

/// Rimuove i metadati opzionali dagli XObject immagine che sopravvivono.
fn scrub_image_metadata(doc: &mut Document) {
    for object in doc.objects.values_mut() {
        let Object::Stream(stream) = object else {
            continue;
        };
        if stream.dict.get(b"Subtype").ok().and_then(name_of).as_deref() != Some("Image") {
            continue;
        }
        stream.dict.remove(b"Metadata");
        stream.dict.remove(b"PieceInfo");
        stream.dict.remove(b"StructParent");
    }
}

fn collect_references(object: &Object, out: &mut Vec<ObjectId>) {
    match object {
        Object::Reference(id) => out.push(*id),
        Object::Array(items) => {
            for item in items {
                collect_references(item, out);
            }
        }
        Object::Dictionary(dict) => {
            for (_, value) in dict.iter() {
                collect_references(value, out);
            }
        }
        Object::Stream(stream) => {
            for (_, value) in stream.dict.iter() {
                collect_references(value, out);
            }
        }
        _ => {}
    }
}

/// Immagine 1x1 grigia usata al posto dei campioni rimossi.
fn blank_image() -> Stream {
    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"XObject".to_vec()));
    dict.set("Subtype", Object::Name(b"Image".to_vec()));
    dict.set("Width", 1i64);
    dict.set("Height", 1i64);
    dict.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
    dict.set("BitsPerComponent", 8i64);
    Stream::new(dict, vec![0u8]).with_compression(false)
}

/// Svuota gli oggetti sostituiti che non sono più raggiungibili dal catalogo:
/// nel file devono restare solo le copie redatte oppure, se un'altra pagina li
/// usa ancora, gli originali intatti.
fn scrub_replaced_objects(doc: &mut Document, replaced: &HashSet<ObjectId>) {
    if replaced.is_empty() {
        return;
    }
    let Some(root) = doc
        .trailer
        .get(b"Root")
        .ok()
        .and_then(|object| object.as_reference().ok())
    else {
        return;
    };
    let mut reachable: HashSet<ObjectId> = HashSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !reachable.insert(id) {
            continue;
        }
        let Some(object) = doc.objects.get(&id) else {
            continue;
        };
        collect_references(object, &mut stack);
    }
    if reachable.is_empty() {
        return;
    }
    for id in replaced {
        if reachable.contains(id) {
            continue;
        }
        let Some(object) = doc.objects.get(id) else {
            continue;
        };
        let replacement = match object {
            Object::Stream(stream) => {
                let subtype = stream.dict.get(b"Subtype").ok().and_then(name_of);
                match subtype.as_deref() {
                    Some("Image") => Object::Stream(blank_image()),
                    Some("Form") => {
                        let mut dict = stream.dict.clone();
                        dict.remove(b"Filter");
                        dict.remove(b"DecodeParms");
                        dict.remove(b"DL");
                        Object::Stream(Stream::new(dict, Vec::new()))
                    }
                    _ => continue,
                }
            }
            _ => continue,
        };
        doc.objects.insert(*id, replacement);
    }
}

/// Dice se un content stream è referenziato da altre pagine oltre a quella che
/// stiamo riscrivendo: in quel caso non va cancellato, altrimenti si romperebbero
/// le pagine che lo condividono.
pub(crate) fn stream_shared_with_other_pages(
    doc: &Document,
    current_page: ObjectId,
    id: ObjectId,
) -> bool {
    for (_, page_id) in doc.get_pages() {
        if page_id == current_page {
            continue;
        }
        let Ok(page) = doc.get_dictionary(page_id) else {
            continue;
        };
        match page.get(b"Contents") {
            Ok(Object::Reference(other)) if *other == id => return true,
            Ok(Object::Array(items)) => {
                if items.iter().any(|item| item.as_reference().ok() == Some(id)) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

pub fn redact_pdf(
    src: &Path,
    dest: &Path,
    password: Option<&str>,
    rects: &[([f32; 4], usize)],
) -> Result<RedactReport, String> {
    let mut doc = match password {
        Some(p) => Document::load_with_password(src, p),
        None => Document::load(src),
    }
    .map_err(|e| format!("Apertura del PDF non riuscita: {e}"))?;

    let pages = doc.get_pages();
    let mut by_page: HashMap<usize, Vec<Rect>> = HashMap::new();
    for (rect, page_index) in rects {
        let Some(page_id) = pages.get(&(*page_index as u32 + 1)).copied() else {
            continue;
        };
        let media = media_box(&doc, page_id);
        let rotation = page_rotation(&doc, page_id);
        let area = norm_to_user(rect, rotation, media);
        if (area.x1 - area.x0).abs() < 0.5 || (area.y1 - area.y0).abs() < 0.5 {
            continue;
        }
        by_page.entry(*page_index).or_default().push(area);
    }

    let mut report = RedactReport::default();
    let mut modified_forms: HashMap<ObjectId, FormEdit> = HashMap::new();
    let mut touched_forms: HashSet<ObjectId> = HashSet::new();
    let mut replaced: HashSet<ObjectId> = HashSet::new();

    for (page_index, areas) in by_page.iter() {
        let Some(page_id) = pages.get(&(*page_index as u32 + 1)).copied() else {
            continue;
        };
        let resources = page_resources(&doc, page_id).cloned();
        let fonts = fonts_for(&doc, resources.as_ref());
        let ids = page_content_ids(&doc, page_id);
        let data = content_bytes(&doc, &ids)?;
        if data.is_empty() {
            add_black_rects(&mut doc, page_id, areas, ids);
            remove_annotations(&mut doc, page_id, areas, &mut report);
            continue;
        }
        let content = Content::decode(&data)
            .map_err(|e| format!("Contenuto della pagina {} non interpretabile: {e}", page_index + 1))?;
        let (processed, new_resources, pending, next_id) = {
            let mut processor = Processor {
                doc: &doc,
                report: RedactReport::default(),
                modified_forms: &mut modified_forms,
                touched_forms: &mut touched_forms,
                replaced: &mut replaced,
                pending: Vec::new(),
                next_id: doc.max_id,
                name_seq: 0,
            };
            let outcome =
                processor.process(content.operations, areas, resources.as_ref(), &fonts, 0)?;
            report.texts += processor.report.texts;
            report.images += processor.report.images;
            report.cropped += processor.report.cropped;
            report.paths += processor.report.paths;
            report.skipped_forms += processor.report.skipped_forms;
            (
                outcome.ops,
                outcome.resources,
                std::mem::take(&mut processor.pending),
                processor.next_id,
            )
        };
        for (id, object) in pending {
            doc.objects.insert(id, object);
        }
        doc.max_id = doc.max_id.max(next_id);
        let bytes = Content {
            operations: processed,
        }
        .encode()
        .map_err(|e| format!("Riscrittura del contenuto non riuscita: {e}"))?;
        let mut dict = Dictionary::new();
        dict.set("Length", bytes.len() as i64);
        let new_id = doc.add_object(Object::Stream(Stream::new(dict, bytes)));
        if let Ok(page) = doc.get_dictionary_mut(page_id) {
            page.set("Contents", Object::Reference(new_id));
            if let Some(resources) = new_resources {
                page.set("Resources", Object::Dictionary(resources));
            }
        }
        for old in ids {
            if !stream_shared_with_other_pages(&doc, page_id, old) {
                let _ = doc.delete_object(old);
            }
        }
        add_black_rects(&mut doc, page_id, areas, vec![new_id]);
        remove_annotations(&mut doc, page_id, areas, &mut report);
    }

    for (id, edit) in modified_forms {
        if let Ok(Object::Stream(stream)) = doc.get_object_mut(id) {
            stream.dict.remove(b"Filter");
            stream.dict.remove(b"DecodeParms");
            stream.dict.remove(b"DL");
            if let Some(resources) = edit.resources {
                stream.dict.set("Resources", Object::Dictionary(resources));
            }
            stream.set_content(edit.content);
        }
    }

    strip_metadata(&mut doc);
    scrub_image_metadata(&mut doc);
    scrub_replaced_objects(&mut doc, &replaced);
    doc.save(dest)
        .map_err(|e| format!("Salvataggio del PDF redatto non riuscito: {e}"))?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_sample() -> Vec<u8> {
        let content = b"BT /F1 12 Tf 72 700 Td (SEGRETO) Tj ET\nBT /F1 12 Tf 72 500 Td (PUBBLICO) Tj ET\n";
        let objects: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R /Annots [6 0 R] >>".to_vec(),
            {
                let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
                stream.extend_from_slice(content);
                stream.extend_from_slice(b"endstream");
                stream
            },
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /MissingWidth 500 >>".to_vec(),
            b"<< /Type /Annot /Subtype /Text /Rect [70 690 220 720] /Contents (nota riservata) >>".to_vec(),
            b"<< /Title (Documento riservato) /Author (Mario Rossi) >>".to_vec(),
        ];

        let mut file: Vec<u8> = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(file.len());
            file.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
            file.extend_from_slice(body);
            file.extend_from_slice(b"\nendobj\n");
        }
        let xref = file.len();
        file.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in &offsets {
            file.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        file.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R /Info 7 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        file
    }

    #[test]
    fn removes_text_inside_area_and_keeps_the_rest() {
        let dir = std::env::temp_dir();
        let src = dir.join("openpdf-redact-src.pdf");
        let dest = dir.join("openpdf-redact-dest.pdf");
        std::fs::write(&src, build_sample()).unwrap();

        // Copre la riga "SEGRETO" (y 660..720) lasciando "PUBBLICO" (y 500) intatto.
        let rects = [([0.10f32, 0.14, 0.55, 0.09], 0usize)];
        let report = redact_pdf(&src, &dest, None, &rects).unwrap();
        assert!(report.texts >= 1, "nessun testo rimosso");
        assert!(report.annotations >= 1, "annotazione non rimossa");

        let doc = Document::load(&dest).unwrap();
        let text = doc.extract_text(&[1]).unwrap();
        assert!(!text.contains("SEGRETO"), "testo riservato ancora presente: {text:?}");
        assert!(text.contains("PUBBLICO"), "testo non riservato perso: {text:?}");

        let page_id = *doc.get_pages().get(&1).unwrap();
        let page = doc.get_dictionary(page_id).unwrap();
        if let Ok(Object::Array(annots)) = page.get(b"Annots") {
            assert!(annots.is_empty(), "annotazioni non rimosse");
        }
        assert!(doc.trailer.get(b"Info").is_err(), "Info non rimossa");
    }

    #[test]
    fn maps_normalized_rect_with_rotation() {
        let media = (0.0, 0.0, 600.0, 800.0);
        let upright = norm_to_user(&[0.0, 0.0, 1.0, 1.0], 0, media);
        assert_eq!((upright.x0, upright.y0, upright.x1, upright.y1), (0.0, 0.0, 600.0, 800.0));

        let rotated = norm_to_user(&[0.0, 0.0, 0.5, 1.0], 90, media);
        assert!((rotated.x1 - rotated.x0 - 600.0).abs() < 0.01);
        assert!((rotated.y1 - rotated.y0 - 400.0).abs() < 0.01);
    }

    #[test]
    fn keeps_shared_content_stream_on_other_pages() {
        let content = b"BT /F1 12 Tf 72 700 Td (SEGRETO) Tj ET\nBT /F1 12 Tf 72 500 Td (PUBBLICO) Tj ET\n";
        let objects: Vec<Vec<u8>> = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 6 0 R >> >> /Contents 4 0 R >>".to_vec(),
            {
                let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
                stream.extend_from_slice(content);
                stream.extend_from_slice(b"endstream");
                stream
            },
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 6 0 R >> >> /Contents 4 0 R >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /MissingWidth 500 >>".to_vec(),
        ];
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

        let dir = std::env::temp_dir();
        let src = dir.join("openpdf-redact-shared-src.pdf");
        let dest = dir.join("openpdf-redact-shared-dest.pdf");
        std::fs::write(&src, file).unwrap();
        let rects = [([0.10f32, 0.14, 0.55, 0.09], 0usize)];
        redact_pdf(&src, &dest, None, &rects).unwrap();

        let doc = Document::load(&dest).unwrap();
        let first = doc.extract_text(&[1]).unwrap();
        let second = doc.extract_text(&[2]).unwrap();
        assert!(!first.contains("SEGRETO"), "redazione non applicata alla pagina 1");
        assert!(
            second.contains("SEGRETO"),
            "lo stream condiviso è stato cancellato anche dalla pagina 2: {second:?}"
        );
        assert!(second.contains("PUBBLICO"));
    }

    #[test]
    fn redacts_a_real_pdf_when_available() {
        let src = std::path::Path::new(r"C:\Users\utente\Documents\toutorial.pdf");
        if !src.exists() {
            return;
        }
        let dest = std::env::temp_dir().join("openpdf-redact-real.pdf");
        let report = redact_pdf(src, &dest, None, &[([0.1f32, 0.1, 0.3, 0.2], 0)]).unwrap();
        assert!(report.texts + report.images + report.paths > 0, "nulla rimosso");
        let doc = Document::load(&dest).unwrap();
        assert!(doc.get_pages().len() >= 1);
        let page = doc.get_pages().values().next().copied().unwrap();
        assert!(!doc.get_page_contents(page).is_empty());
    }

    #[derive(Clone, Copy)]
    enum ImageFilter {
        Flate,
        Jpeg,
    }

    fn name(value: &str) -> Object {
        Object::Name(value.as_bytes().to_vec())
    }

    fn rectangle(values: [i64; 4]) -> Object {
        Object::Array(values.iter().map(|value| Object::Integer(*value)).collect())
    }

    fn helvetica(doc: &mut Document) -> ObjectId {
        let mut font = Dictionary::new();
        font.set("Type", name("Font"));
        font.set("Subtype", name("Type1"));
        font.set("BaseFont", name("Helvetica"));
        font.set("MissingWidth", 500i64);
        doc.add_object(font)
    }

    /// Pagina 300x300 con un'immagine 200x200 disegnata da (50, 100).
    fn image_document(filter: ImageFilter, tag: &str) -> (std::path::PathBuf, ObjectId) {
        let width = 200u32;
        let height = 200u32;
        let mut raw = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                raw.push((x % 256) as u8);
                raw.push((y % 256) as u8);
                raw.push(96u8);
            }
        }

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = helvetica(&mut doc);

        let mut image = Dictionary::new();
        image.set("Type", name("XObject"));
        image.set("Subtype", name("Image"));
        image.set("Width", width as i64);
        image.set("Height", height as i64);
        image.set("ColorSpace", name("DeviceRGB"));
        image.set("BitsPerComponent", 8i64);
        // Metadati opzionali che la redazione deve togliere.
        image.set("StructParent", 7i64);
        image.set("PieceInfo", Object::Dictionary(Dictionary::new()));
        image.set("Metadata", Object::Dictionary(Dictionary::new()));
        let stream = match filter {
            ImageFilter::Flate => {
                let mut stream = Stream::new(image, raw);
                stream.compress().unwrap();
                stream
            }
            ImageFilter::Jpeg => {
                let picture = image::RgbImage::from_raw(width, height, raw).unwrap();
                let mut bytes = Vec::new();
                let mut encoder = JpegEncoder::new_with_quality(&mut bytes, 92);
                encoder
                    .encode(
                        picture.as_raw(),
                        width,
                        height,
                        image::ExtendedColorType::Rgb8,
                    )
                    .unwrap();
                image.set("Filter", name("DCTDecode"));
                Stream::new(image, bytes).with_compression(false)
            }
        };
        let image_id = doc.add_object(stream);

        let page_content = "q 200 0 0 200 50 100 cm /Im1 Do Q\nBT /F1 12 Tf 20 20 Td (PUBBLICO) Tj ET\n";
        let content_id = doc.add_object(Stream::new(
            Dictionary::new(),
            page_content.as_bytes().to_vec(),
        ));

        let mut xobjects = Dictionary::new();
        xobjects.set("Im1", Object::Reference(image_id));
        let mut fonts = Dictionary::new();
        fonts.set("F1", Object::Reference(font_id));
        let mut resources = Dictionary::new();
        resources.set("XObject", Object::Dictionary(xobjects));
        resources.set("Font", Object::Dictionary(fonts));

        let mut page = Dictionary::new();
        page.set("Type", name("Page"));
        page.set("Parent", Object::Reference(pages_id));
        page.set("MediaBox", rectangle([0, 0, 300, 300]));
        page.set("Resources", Object::Dictionary(resources));
        page.set("Contents", Object::Reference(content_id));
        let page_id = doc.add_object(page);

        let mut pages = Dictionary::new();
        pages.set("Type", name("Pages"));
        pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        pages.set("Count", 1i64);
        doc.objects.insert(pages_id, Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.set("Type", name("Catalog"));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(catalog);
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let file = format!("openpdf-redact-image-{tag}.pdf");
        let path = std::env::temp_dir().join(file);
        doc.save(&path).unwrap();
        (path, image_id)
    }

    /// Rettangoli in spazio utente delle immagini effettivamente disegnate.
    fn drawn_image_boxes(doc: &Document, page_id: ObjectId) -> Vec<Rect> {
        let content = content_bytes(doc, &page_content_ids(doc, page_id)).unwrap();
        let decoded = Content::decode(&content).unwrap();
        let Some(resources) = page_resources(doc, page_id) else {
            return Vec::new();
        };
        let Some(xobjects) = resources.get(b"XObject").ok().and_then(|x| x.as_dict().ok()) else {
            return Vec::new();
        };
        let mut ctm = IDENTITY;
        let mut stack: Vec<Matrix> = Vec::new();
        let mut boxes = Vec::new();
        for operation in decoded.operations {
            match operation.operator.as_str() {
                "q" => stack.push(ctm),
                "Q" => {
                    if let Some(previous) = stack.pop() {
                        ctm = previous;
                    }
                }
                "cm" => {
                    let values: Vec<f64> = operation.operands.iter().filter_map(number).collect();
                    if values.len() == 6 {
                        ctm = concat(
                            &[values[0], values[1], values[2], values[3], values[4], values[5]],
                            &ctm,
                        );
                    }
                }
                "Do" => {
                    let Some(name) = operation.operands.first().and_then(name_of) else {
                        continue;
                    };
                    let Some(id) = xobjects
                        .get(name.as_bytes())
                        .ok()
                        .and_then(|value| value.as_reference().ok())
                    else {
                        continue;
                    };
                    let Ok(stream) = doc.get_object(id).and_then(|object| object.as_stream()) else {
                        continue;
                    };
                    if stream.dict.get(b"Subtype").ok().and_then(name_of).as_deref()
                        != Some("Image")
                    {
                        continue;
                    }
                    if let Some(bbox) = map_rect(&UNIT_SQUARE, &ctm) {
                        boxes.push(bbox);
                    }
                }
                _ => {}
            }
        }
        boxes
    }

    fn page_image_sizes(doc: &Document, page_id: ObjectId) -> Vec<(u32, u32)> {
        let Some(resources) = page_resources(doc, page_id) else {
            return Vec::new();
        };
        let Some(xobjects) = resources.get(b"XObject").ok().and_then(|x| x.as_dict().ok()) else {
            return Vec::new();
        };
        let mut sizes = Vec::new();
        for (_, value) in xobjects.iter() {
            let Ok(id) = value.as_reference() else {
                continue;
            };
            let Ok(stream) = doc.get_object(id).and_then(|object| object.as_stream()) else {
                continue;
            };
            let width = stream.dict.get(b"Width").ok().and_then(int_of).unwrap_or(0);
            let height = stream.dict.get(b"Height").ok().and_then(int_of).unwrap_or(0);
            sizes.push((width.max(0) as u32, height.max(0) as u32));
        }
        sizes
    }

    fn assert_partial_crop(filter: ImageFilter, tag: &str) {
        let (src, image_id) = image_document(filter, tag);
        let dest = src.with_extension("redacted.pdf");

        // Copre il quarto in alto a sinistra dell'immagine.
        let rects = [([0.1667f32, 0.0, 0.3333, 0.3333], 0usize)];
        let report = redact_pdf(&src, &dest, None, &rects).unwrap();
        assert_eq!(report.cropped, 1, "immagine non ritagliata: {report:?}");
        assert_eq!(report.images, 0, "immagine rimossa per intero: {report:?}");

        let doc = Document::load(&dest).unwrap();
        let page_id = *doc.get_pages().get(&1).unwrap();
        let text = doc.extract_text(&[1]).unwrap();
        assert!(text.contains("PUBBLICO"), "resto della pagina perso: {text:?}");

        let sizes = page_image_sizes(&doc, page_id);
        assert_eq!(sizes.len(), 2, "attesi due ritagli, trovati {sizes:?}");
        for (width, height) in &sizes {
            assert!(
                *width < 200 || *height < 200,
                "ritaglio della dimensione originale: {width}x{height}"
            );
        }

        // I ritagli devono coprire esattamente la parte non redatta e non
        // devono sporgere dentro l'area coperta.
        let area = Rect {
            x0: 50.0,
            y0: 200.0,
            x1: 150.0,
            y1: 300.0,
        };
        let boxes = drawn_image_boxes(&doc, page_id);
        let mut drawn = 0.0;
        for bbox in &boxes {
            assert!(
                !bbox.intersects(&area),
                "ritaglio ancora sotto l'area redatta: {bbox:?}"
            );
            drawn += (bbox.x1 - bbox.x0) * (bbox.y1 - bbox.y0);
        }
        assert!(
            (drawn - 30_000.0).abs() < 1.0,
            "superficie non coperta errata: {drawn}"
        );

        // I pixel coperti non devono restare nel file: l'oggetto originale,
        // non più raggiungibile, viene svuotato.
        let original = doc.get_object(image_id).unwrap().as_stream().unwrap();
        let width = original.dict.get(b"Width").ok().and_then(int_of).unwrap_or(0);
        assert_eq!(width, 1, "campione originale ancora presente nel file");

        // Metadati opzionali rimossi dalle immagini che sopravvivono.
        for object in doc.objects.values() {
            let Object::Stream(stream) = object else {
                continue;
            };
            if stream.dict.get(b"Subtype").ok().and_then(name_of).as_deref() != Some("Image") {
                continue;
            }
            for key in [b"Metadata".as_slice(), b"PieceInfo", b"StructParent"] {
                assert!(
                    stream.dict.get(key).is_err(),
                    "metadato {:?} ancora presente",
                    String::from_utf8_lossy(key)
                );
            }
        }
    }

    #[test]
    fn crops_a_partially_covered_flate_image() {
        assert_partial_crop(ImageFilter::Flate, "flate-crop");
    }

    #[test]
    fn crops_a_partially_covered_jpeg_image() {
        assert_partial_crop(ImageFilter::Jpeg, "jpeg-crop");
    }

    #[test]
    fn removes_an_image_covered_entirely() {
        let (src, image_id) = image_document(ImageFilter::Flate, "flate-full");
        let dest = src.with_extension("full.pdf");
        let rects = [([0.0f32, 0.0, 1.0, 1.0], 0usize)];
        let report = redact_pdf(&src, &dest, None, &rects).unwrap();
        assert_eq!(report.images, 1, "immagine non rimossa: {report:?}");
        assert_eq!(report.cropped, 0, "immagine ritagliata invece che rimossa");

        let doc = Document::load(&dest).unwrap();
        let page_id = *doc.get_pages().get(&1).unwrap();
        let content = content_bytes(&doc, &page_content_ids(&doc, page_id)).unwrap();
        let content = String::from_utf8_lossy(&content);
        assert!(!content.contains(" Do"), "immagine ancora disegnata: {content}");
        assert!(
            page_image_sizes(&doc, page_id).is_empty(),
            "immagine ancora referenziata dalle risorse"
        );
        let original = doc.get_object(image_id).unwrap().as_stream().unwrap();
        assert_eq!(
            original.dict.get(b"Width").ok().and_then(int_of).unwrap_or(0),
            1,
            "campione originale ancora presente nel file"
        );
    }

    /// Pagina 300x300 con un Form XObject disegnato due volte nella stessa posizione.
    fn shared_form_document() -> (std::path::PathBuf, ObjectId) {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = helvetica(&mut doc);

        let mut form_fonts = Dictionary::new();
        form_fonts.set("F1", Object::Reference(font_id));
        let mut form_resources = Dictionary::new();
        form_resources.set("Font", Object::Dictionary(form_fonts));

        let mut form = Dictionary::new();
        form.set("Type", name("XObject"));
        form.set("Subtype", name("Form"));
        form.set("FormType", 1i64);
        form.set("BBox", rectangle([0, 0, 200, 200]));
        form.set("Resources", Object::Dictionary(form_resources));
        let form_id = doc.add_object(Stream::new(
            form,
            b"BT /F1 12 Tf 10 100 Td (RISERVATO) Tj ET\n".to_vec(),
        ));

        let page_content = "q 1 0 0 1 50 100 cm /Fm1 Do Q\nq 1 0 0 1 50 100 cm /Fm1 Do Q\n";
        let content_id = doc.add_object(Stream::new(
            Dictionary::new(),
            page_content.as_bytes().to_vec(),
        ));

        let mut xobjects = Dictionary::new();
        xobjects.set("Fm1", Object::Reference(form_id));
        let mut resources = Dictionary::new();
        resources.set("XObject", Object::Dictionary(xobjects));

        let mut page = Dictionary::new();
        page.set("Type", name("Page"));
        page.set("Parent", Object::Reference(pages_id));
        page.set("MediaBox", rectangle([0, 0, 300, 300]));
        page.set("Resources", Object::Dictionary(resources));
        page.set("Contents", Object::Reference(content_id));
        let page_id = doc.add_object(page);

        let mut pages = Dictionary::new();
        pages.set("Type", name("Pages"));
        pages.set("Kids", Object::Array(vec![Object::Reference(page_id)]));
        pages.set("Count", 1i64);
        doc.objects.insert(pages_id, Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.set("Type", name("Catalog"));
        catalog.set("Pages", Object::Reference(pages_id));
        let catalog_id = doc.add_object(catalog);
        doc.trailer.set("Root", Object::Reference(catalog_id));

        let path = std::env::temp_dir().join("openpdf-redact-form.pdf");
        doc.save(&path).unwrap();
        (path, form_id)
    }

    #[test]
    fn clones_and_redacts_a_shared_form_xobject() {
        let (src, form_id) = shared_form_document();
        let dest = src.with_extension("redacted.pdf");

        // Copre la riga di testo dentro il modulo.
        let rects = [([0.18f32, 0.29, 0.30, 0.06], 0usize)];
        let report = redact_pdf(&src, &dest, None, &rects).unwrap();
        assert_eq!(report.skipped_forms, 0, "modulo saltato: {report:?}");
        assert!(report.texts >= 2, "entrambe le istanze devono essere redatte");

        let doc = Document::load(&dest).unwrap();
        let page_id = *doc.get_pages().get(&1).unwrap();
        let content = content_bytes(&doc, &page_content_ids(&doc, page_id)).unwrap();
        let decoded = Content::decode(&content).unwrap();
        let names: Vec<String> = decoded
            .operations
            .iter()
            .filter(|op| op.operator == "Do")
            .filter_map(|op| op.operands.first().and_then(name_of))
            .collect();
        assert_eq!(names.len(), 2, "attesi due disegni del modulo: {names:?}");
        assert_ne!(names[0], names[1], "il modulo non è stato clonato: {names:?}");

        let resources = page_resources(&doc, page_id).unwrap();
        let xobjects = resources.get(b"XObject").unwrap().as_dict().unwrap();
        for resource_name in &names {
            let id = xobjects
                .get(resource_name.as_bytes())
                .unwrap()
                .as_reference()
                .unwrap();
            let stream = doc.get_object(id).unwrap().as_stream().unwrap();
            let body = stream.decompressed_content().unwrap();
            assert!(
                !String::from_utf8_lossy(&body).contains("RISERVATO"),
                "testo riservato ancora nel clone {resource_name}"
            );
        }

        let original = doc.get_object(form_id).unwrap().as_stream().unwrap();
        let body = original.decompressed_content().unwrap();
        assert!(
            !String::from_utf8_lossy(&body).contains("RISERVATO"),
            "il modulo originale conserva ancora il testo"
        );
    }
}
