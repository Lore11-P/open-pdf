use serde::{Deserialize, Serialize};

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct DocMeta {
    pub title: String,
    pub author: String,
    pub subject: String,
    pub keywords: String,
    pub creator: String,
    pub producer: String,
    pub creation_date: String,
    pub modification_date: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub index: usize,
    pub width: f32,
    pub height: f32,
    pub rotation: i32,
    pub display_width: f32,
    pub display_height: f32,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RedactResult {
    #[serde(flatten)]
    pub info: OpenInfo,
    pub message: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SignResult {
    #[serde(flatten)]
    pub info: OpenInfo,
    pub message: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SignatureRecord {
    pub field: String,
    pub signer: String,
    pub time: String,
    pub subfilter: String,
    pub covers_all: bool,
    pub digest_ok: bool,
    pub signature_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_valid: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain_complete: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chain_errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attribute_errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chain: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docmdp: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revocation: Option<String>,
}

/// Risposta delle conversioni Office con il messaggio del motore usato.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OfficeResult {
    #[serde(flatten)]
    pub info: OpenInfo,
    pub message: String,
}

/// Risposta della pulizia profonda dei dati nascosti.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SanitizeResult {
    #[serde(flatten)]
    pub info: OpenInfo,
    pub message: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FormFieldInfo {
    pub page: usize,
    pub index: usize,
    pub label: String,
    pub kind: String,
    pub value: String,
    pub checked: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OpenInfo {
    pub session_id: String,
    pub path: String,
    pub name: String,
    pub original_path: String,
    pub original_name: String,
    pub page_count: usize,
    pub pages: Vec<PageInfo>,
    pub meta: DocMeta,
    pub annotations: Vec<Annotation>,
    pub form_fields: Vec<FormFieldInfo>,
    pub file_size: u64,
}

fn default_color() -> String {
    "#ffd60a".into()
}

fn default_opacity() -> f32 {
    0.35
}

fn default_stroke() -> f32 {
    0.003
}

fn default_font_size() -> f32 {
    0.018
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Annotation {
    pub id: String,
    pub page: usize,
    #[serde(rename = "type")]
    pub kind: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub points: Vec<[f32; 2]>,
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    #[serde(default = "default_stroke")]
    pub stroke: f32,
    pub text: String,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    pub label: String,
    pub data_url: String,
    pub author: String,
    pub created_at: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub page: usize,
    pub text: String,
    pub rects: Vec<[f32; 4]>,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RenderResult {
    pub png: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub pdfium: bool,
    pub pdfium_path: String,
    pub edge: bool,
    pub office: bool,
    pub tesseract: bool,
    pub ocr_windows: bool,
    pub version: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ShareComment {
    pub id: String,
    pub author: String,
    pub page: usize,
    pub x: f32,
    pub y: f32,
    pub text: String,
    pub signature: String,
    pub created_at: String,
    pub kind: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ShareInfo {
    pub url: String,
    pub port: u16,
    pub token: String,
    pub page_count: usize,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DiffPage {
    pub page: usize,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: bool,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DiffResult {
    pub pages: Vec<DiffPage>,
    pub added_chars: usize,
    pub removed_chars: usize,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConvertResult {
    pub path: String,
    pub message: String,
    pub size: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FormValue {
    pub page: usize,
    pub index: usize,
    pub value: String,
    pub checked: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct NewField {
    pub page: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub kind: String,
    pub name: String,
}


#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TextLine {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
