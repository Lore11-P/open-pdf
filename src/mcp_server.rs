//! Server MCP (Model Context Protocol) di Open PDF, trasporto stdio.
//!
//! Espone a un agente AI le stesse funzioni dell'applicazione desktop: ogni
//! strumento legge un PDF locale e, quando serve, scrive il risultato in un
//! percorso indicato. Il protocollo è JSON-RPC 2.0 con un messaggio per riga.

// I moduli sono condivisi con l'applicazione: qui ne serve solo una parte
// delle funzioni, quindi il resto resta compilato senza avvisi.
#![allow(dead_code)]

mod annotate;
mod convert;
mod engine;
mod pades;
mod pades_ocsp;
mod pades_tsa;
mod pades_verify;
mod pages;
mod pdftext;
mod redact;
mod sanitize;
mod security;
mod textedit;
mod types;

use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

const VERSIONI: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

fn main() {
    eprintln!("Open PDF MCP {} in ascolto su stdio", env!("CARGO_PKG_VERSION"));
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for riga in stdin.lock().lines() {
        let Ok(riga) = riga else { break };
        let riga = riga.trim();
        if riga.is_empty() {
            continue;
        }
        let risposta = match serde_json::from_str::<Value>(riga) {
            Ok(richiesta) => gestisci(richiesta),
            Err(e) => Some(errore(Value::Null, -32700, &format!("JSON non valido: {e}"))),
        };
        if let Some(risposta) = risposta {
            let _ = writeln!(stdout, "{risposta}");
            let _ = stdout.flush();
        }
    }
}

fn gestisci(richiesta: Value) -> Option<Value> {
    let metodo = richiesta
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let parametri = richiesta.get("params").cloned().unwrap_or(Value::Null);
    // Senza `id` è una notifica: nessuna risposta.
    let id = richiesta.get("id").cloned()?;
    let esito = match metodo.as_str() {
        "initialize" => Ok(inizializza(&parametri)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": elenco_strumenti() })),
        "tools/call" => chiama(&parametri),
        _ => Err((-32601, format!("Metodo non gestito: {metodo}"))),
    };
    Some(match esito {
        Ok(risultato) => json!({"jsonrpc": "2.0", "id": id, "result": risultato}),
        Err((codice, messaggio)) => errore(id, codice, &messaggio),
    })
}

fn errore(id: Value, codice: i64, messaggio: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": codice, "message": messaggio}})
}

fn inizializza(parametri: &Value) -> Value {
    let richiesta = parametri
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let versione = if VERSIONI.contains(&richiesta) {
        richiesta
    } else {
        VERSIONI[0]
    };
    json!({
        "protocolVersion": versione,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {
            "name": "open-pdf",
            "title": "Open PDF",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": "Legge, modifica, redige, pulisce, converte e firma PDF locali. \
Gli indici di pagina partono da 0. Ogni operazione di scrittura richiede un percorso \
di output separato e non tocca mai il file di partenza."
    })
}

fn chiama(parametri: &Value) -> Result<Value, (i64, String)> {
    let nome = parametri
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "Nome dello strumento mancante".to_string()))?;
    let argomenti = parametri
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let definizioni = elenco_strumenti();
    let conosciuto = definizioni
        .as_array()
        .map(|elenco| {
            elenco
                .iter()
                .any(|strumento| strumento["name"].as_str() == Some(nome))
        })
        .unwrap_or(false);
    if !conosciuto {
        return Err((-32602, format!("Strumento sconosciuto: {nome}")));
    }
    Ok(match esegui(nome, &argomenti) {
        Ok(contenuto) => json!({"content": contenuto}),
        Err(messaggio) => json!({
            "content": [{"type": "text", "text": messaggio}],
            "isError": true
        }),
    })
}

// -- Strumenti ---------------------------------------------------------------

fn esegui(nome: &str, a: &Value) -> Result<Value, String> {
    match nome {
        "pdf_info" => {
            let percorso = percorso_arg(a, "path")?;
            let password = testo_opzionale(a, "password");
            let (pagine, elenco, meta, campi) =
                engine::document_info(&percorso, password.as_deref())?;
            Ok(testo(json!({
                "path": percorso.display().to_string(),
                "pageCount": pagine,
                "size": std::fs::metadata(&percorso).map(|m| m.len()).unwrap_or(0),
                "pages": elenco,
                "metadata": meta,
                "formFields": campi
            })))
        }

        "pdf_text" => {
            let percorso = percorso_arg(a, "path")?;
            let password = testo_opzionale(a, "password");
            let doc = engine::open_doc(&percorso, password.as_deref())?;
            let totale = doc.pages().len() as usize;
            let da = numero(a, "from", 1).max(1) as usize;
            let a_pagina = numero(a, "to", totale as u64).min(totale as u64) as usize;
            if da > a_pagina {
                return Err("Intervallo non valido: 'from' supera 'to'.".into());
            }
            let mut pagine = Vec::new();
            for indice in (da - 1)..a_pagina {
                pagine.push(json!({
                    "page": indice,
                    "text": engine::page_text_from_doc(&doc, indice)?
                }));
            }
            Ok(testo(json!({"pageCount": totale, "pages": pagine})))
        }

        "pdf_text_lines" => {
            let percorso = percorso_arg(a, "path")?;
            let password = testo_opzionale(a, "password");
            let pagina = numero(a, "page", 0) as usize;
            let righe = textedit::page_lines(&percorso, pagina, password.as_deref())?;
            Ok(testo(json!({"page": pagina, "lines": righe})))
        }

        "pdf_search" => {
            let percorso = percorso_arg(a, "path")?;
            let password = testo_opzionale(a, "password");
            let query = obbligatorio(a, "query")?;
            let casi = booleano(a, "match_case", false);
            let risultati = engine::search_document(&percorso, &query, casi, password.as_deref())?;
            Ok(testo(json!({"hits": risultati})))
        }

        "pdf_render_page" => {
            let percorso = percorso_arg(a, "path")?;
            let password = testo_opzionale(a, "password");
            let pagina = numero(a, "page", 0) as usize;
            let larghezza = numero(a, "width", 1400) as u32;
            let resa =
                engine::render_page_png(&percorso, pagina, larghezza, password.as_deref())?;
            Ok(json!([
                {
                    "type": "text",
                    "text": format!(
                        "Pagina {pagina} (base 0) resa in {}x{} px.",
                        resa.width, resa.height
                    )
                },
                {"type": "image", "data": resa.png, "mimeType": "image/png"}
            ]))
        }

        "pdf_edit_text" => {
            let percorso = percorso_arg(a, "path")?;
            let destinazione = percorso_arg(a, "output")?;
            let password = testo_opzionale(a, "password");
            let pagina = numero(a, "page", 0) as usize;
            let rettangolo = rettangolo(a.get("rect").unwrap_or(&Value::Null)).ok_or(
                "Parametro 'rect' non valido: servono 4 numeri normalizzati [x, y, larghezza, altezza].",
            )?;
            let nuovo = testo_opzionale(a, "text").unwrap_or_default();
            let esito = pdftext::replace_line(
                &percorso,
                &destinazione,
                password.as_deref(),
                pagina,
                rettangolo,
                &nuovo,
            )?;
            Ok(testo(json!({
                "output": destinazione.display().to_string(),
                "report": valore(esito)?
            })))
        }

        "pdf_delete_text" => {
            let percorso = percorso_arg(a, "path")?;
            let destinazione = percorso_arg(a, "output")?;
            let password = testo_opzionale(a, "password");
            let pagina = numero(a, "page", 0) as usize;
            let rettangoli: Vec<[f32; 4]> = a
                .get("rects")
                .and_then(Value::as_array)
                .map(|elenco| elenco.iter().filter_map(rettangolo).collect())
                .unwrap_or_default();
            if rettangoli.is_empty() {
                return Err("Serve almeno un rettangolo in 'rects'.".into());
            }
            let rimossi =
                textedit::delete_lines(&percorso, &destinazione, password.as_deref(), pagina, &rettangoli)?;
            Ok(testo(json!({
                "output": destinazione.display().to_string(),
                "removed": rimossi
            })))
        }

        "pdf_redact" => {
            let percorso = percorso_arg(a, "path")?;
            let destinazione = percorso_arg(a, "output")?;
            let password = testo_opzionale(a, "password");
            let elenco = a
                .get("areas")
                .and_then(Value::as_array)
                .ok_or("Parametro 'areas' mancante: elenco di {page, rect}.")?;
            let mut aree: Vec<([f32; 4], usize)> = Vec::with_capacity(elenco.len());
            for area in elenco {
                let pagina = area
                    .get("page")
                    .and_then(Value::as_u64)
                    .ok_or("Ogni area richiede 'page' (indice da 0).")? as usize;
                let rettangolo = rettangolo(area.get("rect").unwrap_or(&Value::Null)).ok_or(
                    "Ogni area richiede 'rect' con 4 numeri normalizzati [x, y, larghezza, altezza].",
                )?;
                aree.push((rettangolo, pagina));
            }
            if aree.is_empty() {
                return Err("Nessuna area da redigere.".into());
            }
            let esito = redact::redact_pdf(&percorso, &destinazione, password.as_deref(), &aree)?;
            Ok(testo(json!({
                "output": destinazione.display().to_string(),
                "report": valore(esito)?
            })))
        }

        "pdf_sanitize" => {
            let percorso = percorso_arg(a, "path")?;
            let destinazione = percorso_arg(a, "output")?;
            let password = testo_opzionale(a, "password");
            let esito = sanitize::sanitize_pdf(&percorso, &destinazione, password.as_deref())?;
            Ok(testo(json!({
                "output": destinazione.display().to_string(),
                "report": valore(esito)?
            })))
        }

        "pdf_sign_pades" => {
            let percorso = percorso_arg(a, "path")?;
            let destinazione = percorso_arg(a, "output")?;
            let certificato = percorso_arg(a, "certificate")?;
            let credenziali = match testo_opzionale(a, "private_key") {
                Some(chiave) => pades::Credenziali::pem(certificato, PathBuf::from(chiave)),
                None => pades::Credenziali::p12(
                    certificato,
                    testo_opzionale(a, "certificate_password").unwrap_or_default(),
                ),
            };
            let opzioni = pades::SignOptions {
                page_index: numero(a, "page", 0) as usize,
                rect: rettangolo(a.get("rect").unwrap_or(&Value::Null))
                    .unwrap_or([0.62, 0.72, 0.30, 0.13]),
                name: testo_opzionale(a, "name").unwrap_or_default(),
                reason: testo_opzionale(a, "reason").unwrap_or_default(),
                location: testo_opzionale(a, "location").unwrap_or_default(),
                pdf_password: testo_opzionale(a, "password"),
                certification: a
                    .get("docmdp")
                    .and_then(Value::as_u64)
                    .map(|livello| livello as u8),
                visible: booleano(a, "visible", true),
                tsa_url: testo_opzionale(a, "tsa_url"),
            };
            let esito = pades::sign_pdf_con(&percorso, &destinazione, &credenziali, &opzioni)?;
            Ok(testo(json!({
                "output": destinazione.display().to_string(),
                "report": valore(esito)?
            })))
        }

        "pdf_verify_signatures" => {
            let percorso = percorso_arg(a, "path")?;
            let password = testo_opzionale(a, "password");
            let revoca = booleano(a, "check_revocation", false);
            let firme = pades::verify_pdf_con(&percorso, password.as_deref(), revoca)?;
            Ok(testo(json!({"signatures": valore(firme)?})))
        }

        "pdf_from_text" => {
            let destinazione = percorso_arg(a, "output")?;
            let titolo = testo_opzionale(a, "title").unwrap_or_else(|| "Documento".into());
            let contenuto = obbligatorio(a, "text")?;
            convert::text_to_pdf(&destinazione, &titolo, &contenuto)?;
            Ok(testo(json!({"output": destinazione.display().to_string()})))
        }

        "pdf_office_to_pdf" => {
            let percorso = percorso_arg(a, "path")?;
            let destinazione = percorso_arg(a, "output")?;
            let esito = convert::office_to_pdf(&percorso, &destinazione)?;
            Ok(testo(valore(esito)?))
        }

        "pdf_export" => {
            let percorso = percorso_arg(a, "path")?;
            let destinazione = percorso_arg(a, "output")?;
            let password = testo_opzionale(a, "password");
            let formato = obbligatorio(a, "format")?.to_lowercase();
            let esito = match formato.as_str() {
                "text" | "txt" => {
                    convert::export_text(&percorso, &destinazione, password.as_deref())?
                }
                "docx" | "doc" => {
                    convert::export_docx(&percorso, &destinazione, password.as_deref())?
                }
                "xlsx" | "xls" => {
                    convert::export_xlsx(&percorso, &destinazione, password.as_deref())?
                }
                "pptx" | "ppt" => {
                    convert::export_pptx(&percorso, &destinazione, password.as_deref())?
                }
                altro => {
                    return Err(format!(
                        "Formato non supportato: {altro}. Usa text, docx, xlsx o pptx."
                    ))
                }
            };
            Ok(testo(valore(esito)?))
        }

        "pdf_merge" => {
            let destinazione = percorso_arg(a, "output")?;
            let sorgenti = elenco_testo(a, "inputs");
            if sorgenti.is_empty() {
                return Err("'inputs' deve contenere almeno un PDF.".into());
            }
            pages::merge_pdfs(&destinazione, &sorgenti)?;
            Ok(testo(json!({"output": destinazione.display().to_string()})))
        }

        "pdf_extract_pages" => {
            let percorso = percorso_arg(a, "path")?;
            let destinazione = percorso_arg(a, "output")?;
            let password = testo_opzionale(a, "password");
            let pagine = elenco_numeri(a, "pages");
            if pagine.is_empty() {
                return Err("'pages' deve contenere almeno un indice di pagina (base 0).".into());
            }
            pages::copy_pages(&percorso, &destinazione, &pagine, password.as_deref())?;
            Ok(testo(json!({
                "output": destinazione.display().to_string(),
                "pages": pagine
            })))
        }

        "pdf_rotate_page" => {
            let percorso = percorso_arg(a, "path")?;
            let destinazione = percorso_arg(a, "output")?;
            let password = testo_opzionale(a, "password");
            let pagina = numero(a, "page", 0) as usize;
            let gradi = a.get("degrees").and_then(Value::as_i64).unwrap_or(90) as i32;
            pages::rotate_page(&percorso, &destinazione, pagina, gradi, password.as_deref())?;
            Ok(testo(json!({
                "output": destinazione.display().to_string(),
                "page": pagina,
                "degrees": gradi
            })))
        }

        "pdf_compress" => {
            let percorso = percorso_arg(a, "path")?;
            let destinazione = percorso_arg(a, "output")?;
            let password = testo_opzionale(a, "password");
            let modalita = testo_opzionale(a, "mode").unwrap_or_else(|| "lossless".into());
            let (prima, dopo) = match modalita.as_str() {
                "lossless" | "senza perdita" => {
                    pages::compress_lossless(&percorso, &destinazione, password.as_deref())?
                }
                "raster" | "rasterizzazione" => pages::compress_raster(
                    &percorso,
                    &destinazione,
                    password.as_deref(),
                    decimale(a, "dpi", 150.0),
                    numero(a, "quality", 80).min(100) as u8,
                )?,
                altra => {
                    return Err(format!(
                        "Modalità non supportata: {altra}. Usa lossless o raster."
                    ))
                }
            };
            Ok(testo(json!({
                "output": destinazione.display().to_string(),
                "before": prima,
                "after": dopo,
                "savedBytes": prima.saturating_sub(dopo)
            })))
        }

        "pdf_images_to_pdf" => {
            let destinazione = percorso_arg(a, "output")?;
            let immagini = elenco_testo(a, "images");
            if immagini.is_empty() {
                return Err("'images' deve contenere almeno un file immagine.".into());
            }
            pages::images_to_pdf(&destinazione, &immagini)?;
            Ok(testo(json!({"output": destinazione.display().to_string()})))
        }

        "pdf_protect" => {
            let percorso = percorso_arg(a, "path")?;
            let destinazione = percorso_arg(a, "output")?;
            let password = testo_opzionale(a, "password");
            let utente = obbligatorio(a, "user_password")?;
            let proprietario =
                testo_opzionale(a, "owner_password").unwrap_or_else(|| utente.clone());
            security::protect_pdf(
                &percorso,
                &destinazione,
                password.as_deref(),
                &utente,
                &proprietario,
                booleano(a, "allow_print", true),
                booleano(a, "allow_copy", false),
                booleano(a, "allow_modify", false),
                booleano(a, "allow_annotate", true),
                booleano(a, "allow_forms", true),
            )?;
            Ok(testo(json!({"output": destinazione.display().to_string()})))
        }

        altro => Err(format!("Strumento sconosciuto: {altro}")),
    }
}

// -- Schema degli strumenti --------------------------------------------------

fn strumento(nome: &str, descrizione: &str, proprieta: Value, obbligatori: Value) -> Value {
    json!({
        "name": nome,
        "description": descrizione,
        "inputSchema": {
            "type": "object",
            "properties": proprieta,
            "required": obbligatori
        }
    })
}

fn prop_ingresso(descrizione: &str) -> Value {
    json!({"type": "string", "description": descrizione})
}

fn prop_uscita() -> Value {
    json!({
        "type": "string",
        "description": "Percorso del file da scrivere (viene sovrascritto se esiste)"
    })
}

fn prop_password() -> Value {
    json!({"type": "string", "description": "Password di apertura, se il PDF è cifrato"})
}

fn prop_pagina(descrizione: &str) -> Value {
    json!({"type": "integer", "minimum": 0, "description": descrizione})
}

fn prop_rettangolo(descrizione: &str) -> Value {
    json!({
        "type": "array",
        "items": {"type": "number"},
        "minItems": 4,
        "maxItems": 4,
        "description": descrizione
    })
}

fn elenco_strumenti() -> Value {
    json!([
        strumento(
            "pdf_info",
            "Legge numero di pagine, dimensioni, metadati e campi modulo di un PDF.",
            json!({"path": prop_ingresso("Percorso del PDF"), "password": prop_password()}),
            json!(["path"])
        ),
        strumento(
            "pdf_text",
            "Estrae il testo pagina per pagina (indici da 0).",
            json!({
                "path": prop_ingresso("Percorso del PDF"),
                "from": prop_pagina("Prima pagina da estrarre (base 1, predefinito 1)"),
                "to": prop_pagina("Ultima pagina da estrarre (base 1, predefinito ultima)"),
                "password": prop_password()
            }),
            json!(["path"])
        ),
        strumento(
            "pdf_text_lines",
            "Elenca le righe di testo di una pagina con posizione normalizzata: serve a scegliere il rettangolo per pdf_edit_text o pdf_delete_text.",
            json!({
                "path": prop_ingresso("Percorso del PDF"),
                "page": prop_pagina("Indice della pagina (base 0)"),
                "password": prop_password()
            }),
            json!(["path", "page"])
        ),
        strumento(
            "pdf_search",
            "Cerca una stringa nel documento e restituisce pagina e rettangoli delle occorrenze.",
            json!({
                "path": prop_ingresso("Percorso del PDF"),
                "query": prop_ingresso("Testo da cercare"),
                "match_case": {"type": "boolean", "description": "Distingue maiuscole e minuscole"},
                "password": prop_password()
            }),
            json!(["path", "query"])
        ),
        strumento(
            "pdf_render_page",
            "Renderizza una pagina in PNG e la restituisce come immagine, utile per leggere scansioni o impaginazione.",
            json!({
                "path": prop_ingresso("Percorso del PDF"),
                "page": prop_pagina("Indice della pagina (base 0)"),
                "width": {"type": "integer", "minimum": 160, "maximum": 3200, "description": "Larghezza in pixel (predefinito 1400)"},
                "password": prop_password()
            }),
            json!(["path", "page"])
        ),
        strumento(
            "pdf_edit_text",
            "Sostituisce davvero il testo di una riga nel content stream: il testo resta selezionabile e conserva posizione, corpo e colore.",
            json!({
                "path": prop_ingresso("Percorso del PDF"),
                "output": prop_uscita(),
                "page": prop_pagina("Indice della pagina (base 0)"),
                "rect": prop_rettangolo("Rettangolo della riga [x, y, larghezza, altezza] normalizzato 0-1, origine in alto a sinistra (da pdf_text_lines)"),
                "text": prop_ingresso("Nuovo testo della riga"),
                "password": prop_password()
            }),
            json!(["path", "output", "page", "rect", "text"])
        ),
        strumento(
            "pdf_delete_text",
            "Rimuove il testo che interseca i rettangoli indicati, senza disegnare riquadri neri.",
            json!({
                "path": prop_ingresso("Percorso del PDF"),
                "output": prop_uscita(),
                "page": prop_pagina("Indice della pagina (base 0)"),
                "rects": {
                    "type": "array",
                    "items": prop_rettangolo("Rettangolo normalizzato"),
                    "description": "Rettangoli normalizzati delle righe da eliminare"
                },
                "password": prop_password()
            }),
            json!(["path", "output", "page", "rects"])
        ),
        strumento(
            "pdf_redact",
            "Redazione irreversibile a livello di content stream: rimuove testi, immagini, path e link nelle aree e disegna i riquadri neri.",
            json!({
                "path": prop_ingresso("Percorso del PDF"),
                "output": prop_uscita(),
                "areas": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "page": prop_pagina("Indice della pagina (base 0)"),
                            "rect": prop_rettangolo("Rettangolo normalizzato [x, y, larghezza, altezza]")
                        },
                        "required": ["page", "rect"]
                    },
                    "description": "Aree da oscurare"
                },
                "password": prop_password()
            }),
            json!(["path", "output", "areas"])
        ),
        strumento(
            "pdf_sanitize",
            "Pulizia profonda: metadati, XMP, JavaScript, allegati, EXIF, annotazioni nascoste, struttura di accessibilità e oggetti orfani.",
            json!({
                "path": prop_ingresso("Percorso del PDF"),
                "output": prop_uscita(),
                "password": prop_password()
            }),
            json!(["path", "output"])
        ),
        strumento(
            "pdf_from_text",
            "Crea un PDF impaginato partendo da testo semplice.",
            json!({
                "output": prop_uscita(),
                "text": prop_ingresso("Contenuto del documento"),
                "title": prop_ingresso("Titolo della prima pagina")
            }),
            json!(["output", "text"])
        ),
        strumento(
            "pdf_sign_pades",
            "Firma in formato PAdES Baseline B-B (o B-T con marca temporale RFC 3161) da PKCS#12 o coppia PEM.",
            json!({
                "path": prop_ingresso("Percorso del PDF"),
                "output": prop_uscita(),
                "certificate": prop_ingresso("File PKCS#12/PFX, oppure certificato PEM se si passa private_key"),
                "certificate_password": prop_ingresso("Password dell'archivio PKCS#12"),
                "private_key": prop_ingresso("Chiave privata PEM (se presente, 'certificate' è un PEM)"),
                "page": prop_pagina("Pagina del timbro visibile (base 0, predefinito 0)"),
                "rect": prop_rettangolo("Posizione del timbro [x, y, larghezza, altezza] normalizzata"),
                "name": prop_ingresso("Nome del firmatario mostrato nel timbro"),
                "reason": prop_ingresso("Motivo della firma"),
                "location": prop_ingresso("Luogo della firma"),
                "visible": {"type": "boolean", "description": "Timbro visibile (predefinito true)"},
                "docmdp": {"type": "integer", "minimum": 1, "maximum": 3, "description": "Certificazione DocMDP: 1 nessuna modifica, 2 compilazione moduli, 3 anche annotazioni"},
                "tsa_url": prop_ingresso("URL del servizio di marca temporale RFC 3161 (PAdES B-T)"),
                "password": prop_password()
            }),
            json!(["path", "output", "certificate"])
        ),
        strumento(
            "pdf_verify_signatures",
            "Verifica le firme: digest del documento, CMS, copertura del file, catena, attributi firmati, marca temporale e DocMDP.",
            json!({
                "path": prop_ingresso("Percorso del PDF firmato"),
                "check_revocation": {"type": "boolean", "description": "Interroga anche il responder OCSP (rete)"},
                "password": prop_password()
            }),
            json!(["path"])
        ),
        strumento(
            "pdf_office_to_pdf",
            "Converte Word, Excel o PowerPoint in PDF usando Office locale, LibreOffice o, in ultima istanza, il testo estratto.",
            json!({
                "path": prop_ingresso("Percorso del documento Office"),
                "output": prop_uscita()
            }),
            json!(["path", "output"])
        ),
        strumento(
            "pdf_export",
            "Esporta il PDF in testo, Word, Excel o PowerPoint.",
            json!({
                "path": prop_ingresso("Percorso del PDF"),
                "output": prop_uscita(),
                "format": {"type": "string", "enum": ["text", "docx", "xlsx", "pptx"], "description": "Formato di destinazione"},
                "password": prop_password()
            }),
            json!(["path", "output", "format"])
        ),
        strumento(
            "pdf_merge",
            "Unisce più PDF nell'ordine indicato.",
            json!({
                "output": prop_uscita(),
                "inputs": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Percorsi dei PDF da unire, in ordine"
                }
            }),
            json!(["output", "inputs"])
        ),
        strumento(
            "pdf_extract_pages",
            "Estrae o riordina pagine in un nuovo PDF.",
            json!({
                "path": prop_ingresso("Percorso del PDF"),
                "output": prop_uscita(),
                "pages": {
                    "type": "array",
                    "items": {"type": "integer", "minimum": 0},
                    "description": "Indici delle pagine da copiare, nell'ordine voluto (base 0)"
                },
                "password": prop_password()
            }),
            json!(["path", "output", "pages"])
        ),
        strumento(
            "pdf_rotate_page",
            "Ruota una pagina di 90, 180 o 270 gradi.",
            json!({
                "path": prop_ingresso("Percorso del PDF"),
                "output": prop_uscita(),
                "page": prop_pagina("Indice della pagina (base 0)"),
                "degrees": {"type": "integer", "description": "Rotazione da aggiungere in gradi (predefinito 90)"},
                "password": prop_password()
            }),
            json!(["path", "output", "page"])
        ),
        strumento(
            "pdf_compress",
            "Riduce il peso del PDF senza perdita oppure rasterizzando le pagine a DPI e qualità indicati.",
            json!({
                "path": prop_ingresso("Percorso del PDF"),
                "output": prop_uscita(),
                "mode": {"type": "string", "enum": ["lossless", "raster"], "description": "Modalità di compressione (predefinita lossless)"},
                "dpi": {"type": "number", "description": "Risoluzione della rasterizzazione (predefinita 150)"},
                "quality": {"type": "integer", "minimum": 1, "maximum": 100, "description": "Qualità JPEG della rasterizzazione (predefinita 80)"},
                "password": prop_password()
            }),
            json!(["path", "output"])
        ),
        strumento(
            "pdf_images_to_pdf",
            "Crea un PDF con una pagina per immagine.",
            json!({
                "output": prop_uscita(),
                "images": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Percorsi delle immagini, in ordine"
                }
            }),
            json!(["output", "images"])
        ),
        strumento(
            "pdf_protect",
            "Aggiunge password di apertura e permessi con AES-128.",
            json!({
                "path": prop_ingresso("Percorso del PDF"),
                "output": prop_uscita(),
                "user_password": prop_ingresso("Password richiesta per aprire il documento"),
                "owner_password": prop_ingresso("Password del proprietario (predefinita: quella utente)"),
                "allow_print": {"type": "boolean", "description": "Consenti la stampa (predefinito true)"},
                "allow_copy": {"type": "boolean", "description": "Consenti copia del testo (predefinito false)"},
                "allow_modify": {"type": "boolean", "description": "Consenti modifica (predefinito false)"},
                "allow_annotate": {"type": "boolean", "description": "Consenti annotazioni (predefinito true)"},
                "allow_forms": {"type": "boolean", "description": "Consenti compilazione moduli (predefinito true)"},
                "password": prop_password()
            }),
            json!(["path", "output", "user_password"])
        )
    ])
}

// -- Argomenti ---------------------------------------------------------------

fn testo(valore: Value) -> Value {
    json!([{"type": "text", "text": valore.to_string()}])
}

fn valore<T: serde::Serialize>(dato: T) -> Result<Value, String> {
    serde_json::to_value(dato).map_err(|e| format!("Serializzazione non riuscita: {e}"))
}

fn percorso_arg(a: &Value, chiave: &str) -> Result<PathBuf, String> {
    Ok(PathBuf::from(obbligatorio(a, chiave)?))
}

fn obbligatorio(a: &Value, chiave: &str) -> Result<String, String> {
    testo_opzionale(a, chiave)
        .filter(|valore| !valore.trim().is_empty())
        .ok_or_else(|| format!("Parametro obbligatorio mancante: {chiave}"))
}

fn testo_opzionale(a: &Value, chiave: &str) -> Option<String> {
    a.get(chiave)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|valore| !valore.is_empty())
}

fn numero(a: &Value, chiave: &str, predefinito: u64) -> u64 {
    a.get(chiave)
        .and_then(Value::as_u64)
        .unwrap_or(predefinito)
}

fn decimale(a: &Value, chiave: &str, predefinito: f32) -> f32 {
    a.get(chiave)
        .and_then(Value::as_f64)
        .map(|valore| valore as f32)
        .unwrap_or(predefinito)
}

fn booleano(a: &Value, chiave: &str, predefinito: bool) -> bool {
    a.get(chiave)
        .and_then(Value::as_bool)
        .unwrap_or(predefinito)
}

fn rettangolo(valore: &Value) -> Option<[f32; 4]> {
    let elenco = valore.as_array()?;
    if elenco.len() != 4 {
        return None;
    }
    let mut rettangolo = [0f32; 4];
    for (indice, voce) in elenco.iter().enumerate() {
        rettangolo[indice] = voce.as_f64()? as f32;
    }
    Some(rettangolo)
}

fn elenco_testo(a: &Value, chiave: &str) -> Vec<String> {
    a.get(chiave)
        .and_then(Value::as_array)
        .map(|elenco| {
            elenco
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn elenco_numeri(a: &Value, chiave: &str) -> Vec<usize> {
    a.get(chiave)
        .and_then(Value::as_array)
        .map(|elenco| {
            elenco
                .iter()
                .filter_map(Value::as_u64)
                .map(|valore| valore as usize)
                .collect()
        })
        .unwrap_or_default()
}
