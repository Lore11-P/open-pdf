use crate::types::*;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Mutex;
use uuid::Uuid;

pub struct Session {
    pub session_id: String,
    pub original: Option<PathBuf>,
    pub working: Option<PathBuf>,
    pub password: Option<String>,
    pub page_count: usize,
    pub pages: Vec<PageInfo>,
    pub meta: DocMeta,
    pub form_fields: Vec<FormFieldInfo>,
    pub texts: HashMap<usize, String>,
    pub file_size: u64,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            original: None,
            working: None,
            password: None,
            page_count: 0,
            pages: vec![],
            meta: DocMeta::default(),
            form_fields: vec![],
            texts: HashMap::new(),
            file_size: 0,
        }
    }
}

impl Session {
    pub fn reset(&mut self) {
        *self = Session::default();
    }

    pub fn set_document(&mut self, path: &Path, password: Option<String>, keep_session: bool) -> Result<(), String> {
        let info = crate::engine::document_info(path, password.as_deref())?;
        if !keep_session || self.session_id.is_empty() {
            let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            self.session_id = format!("doc-{:016x}", stable_hash(&canonical.to_string_lossy()));
        }
        self.working = Some(path.to_path_buf());
        if !keep_session {
            self.original = Some(path.to_path_buf());
        }
        self.password = password;
        self.page_count = info.0;
        self.pages = info.1;
        self.meta = info.2;
        self.form_fields = info.3;
        self.texts.clear();
        self.file_size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        Ok(())
    }

    pub fn open_info(&self, annotations: Vec<Annotation>) -> OpenInfo {
        OpenInfo {
            session_id: self.session_id.clone(),
            path: self
                .working
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            name: self
                .working
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Documento".into()),
            original_path: self
                .original
                .as_ref()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            original_name: self
                .original
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            page_count: self.page_count,
            pages: self.pages.clone(),
            meta: self.meta.clone(),
            annotations,
            form_fields: self.form_fields.clone(),
            file_size: self.file_size,
        }
    }
}

pub struct AppState {
    pub inner: Mutex<Session>,
    pub tts: Mutex<Option<Child>>,
    pub share: Mutex<Option<crate::collab::Share>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Session::default()),
            tts: Mutex::new(None),
            share: Mutex::new(None),
        }
    }
}

pub fn app_data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("OpenPDF");
    let _ = fs::create_dir_all(dir.join("annotations"));
    let _ = fs::create_dir_all(dir.join("working"));
    dir
}

pub fn annotations_file(session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() {
        return None;
    }
    Some(app_data_dir().join("annotations").join(format!("{session_id}.json")))
}

pub fn load_annotations(session_id: &str) -> Vec<Annotation> {
    let Some(path) = annotations_file(session_id) else {
        return vec![];
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<Annotation>>(&s).ok())
        .unwrap_or_default()
}

pub fn save_annotations(session_id: &str, annotations: &[Annotation]) -> Result<(), String> {
    let Some(path) = annotations_file(session_id) else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let json = serde_json::to_string_pretty(annotations).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn stable_hash(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

pub fn copy_to_working(src: &Path, tag: &str) -> Result<PathBuf, String> {
    let dir = app_data_dir().join("working");
    let _ = fs::create_dir_all(&dir);
    let name = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "document".into());
    let dest = dir.join(format!(
        "{}-{}-{}.pdf",
        sanitize(&name),
        tag,
        &Uuid::new_v4().to_string()[..8]
    ));
    fs::copy(src, &dest).map_err(|e| format!("Impossibile creare la copia di lavoro: {e}"))?;
    Ok(dest)
}

pub fn safe_working_path(name: &str) -> PathBuf {
    let dir = app_data_dir().join("working");
    let _ = fs::create_dir_all(&dir);
    dir.join(format!(
        "{}-{}.pdf",
        sanitize(name),
        &Uuid::new_v4().to_string()[..8]
    ))
}

pub fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

