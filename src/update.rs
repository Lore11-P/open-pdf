//! Controllo aggiornamenti dalle release GitHub, solo su richiesta dell'utente.

use serde::Serialize;
use std::cmp::Ordering;
use std::path::PathBuf;

const REPO: &str = "Lore11-P/open-pdf";

#[derive(Debug, Serialize)]
pub struct Rilascio {
    pub versione: String,
    pub installer: String,
}

#[derive(Debug, Serialize)]
pub struct Aggiornamento {
    pub attuale: String,
    pub disponibile: bool,
    pub versione: String,
    pub note: String,
    pub installer: Option<String>,
    pub precedente: Option<Rilascio>,
}

/// Confronta due versioni tipo `1.2.3` o `v1.2.3`.
pub fn confronta_versioni(a: &str, b: &str) -> Ordering {
    let numeri = |v: &str| -> Vec<u32> {
        v.trim_start_matches(['v', 'V'])
            .split(|c: char| !c.is_ascii_digit())
            .filter(|parte| !parte.is_empty())
            .map(|parte| parte.parse().unwrap_or(0))
            .collect()
    };
    let (mut x, mut y) = (numeri(a), numeri(b));
    let lunghezza = x.len().max(y.len());
    x.resize(lunghezza, 0);
    y.resize(lunghezza, 0);
    x.cmp(&y)
}

fn installer_di(release: &serde_json::Value) -> Option<String> {
    release["assets"].as_array().and_then(|asset| {
        asset.iter().find_map(|asset| {
            let nome = asset["name"].as_str()?;
            if nome.ends_with("-setup.exe") || nome.ends_with(".msi") {
                asset["browser_download_url"].as_str().map(str::to_string)
            } else {
                None
            }
        })
    })
}

/// Legge l'elenco delle release: la più recente e quella immediatamente precedente
/// alla versione installata (per tornare indietro).
pub fn leggi_release(json: &str, attuale: &str) -> Result<Aggiornamento, String> {
    let valore: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("Risposta GitHub non valida: {e}"))?;
    let elenco = valore
        .as_array()
        .ok_or("Risposta GitHub non valida: attesa una lista di release.")?;
    let mut rilasci: Vec<(String, Option<String>, String)> = Vec::new();
    for release in elenco {
        if release["draft"].as_bool().unwrap_or(false)
            || release["prerelease"].as_bool().unwrap_or(false)
        {
            continue;
        }
        let Some(tag) = release["tag_name"].as_str().filter(|tag| !tag.is_empty()) else {
            continue;
        };
        rilasci.push((
            tag.trim_start_matches(['v', 'V']).to_string(),
            installer_di(release),
            release["body"]
                .as_str()
                .unwrap_or_default()
                .chars()
                .take(800)
                .collect(),
        ));
    }
    let piu_recente = rilasci
        .iter()
        .max_by(|a, b| confronta_versioni(&a.0, &b.0))
        .ok_or("Nessuna release pubblicata.")?;
    let precedente = rilasci
        .iter()
        .filter(|(versione, installer, _)| {
            installer.is_some() && confronta_versioni(versione, attuale) == Ordering::Less
        })
        .max_by(|a, b| confronta_versioni(&a.0, &b.0))
        .map(|(versione, installer, _)| Rilascio {
            versione: versione.clone(),
            installer: installer.clone().unwrap_or_default(),
        });
    Ok(Aggiornamento {
        attuale: attuale.to_string(),
        disponibile: confronta_versioni(&piu_recente.0, attuale) == Ordering::Greater,
        versione: piu_recente.0.clone(),
        note: piu_recente.2.clone(),
        installer: piu_recente.1.clone(),
        precedente,
    })
}

fn esegui_curl(argomenti: &[&str]) -> Result<std::process::Output, String> {
    let output = std::process::Command::new("curl.exe")
        .args(argomenti)
        .output()
        .map_err(|e| format!("curl.exe non disponibile: {e}"))?;
    Ok(output)
}

/// Interroga GitHub: nessun dato locale viene inviato, solo la richiesta HTTP.
pub fn controlla() -> Result<Aggiornamento, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=30");
    let output = esegui_curl(&[
        "-sSL",
        "--max-time",
        "20",
        "-H",
        "Accept: application/vnd.github+json",
        "-H",
        "User-Agent: Open-PDF",
        &url,
    ])?;
    if !output.status.success() {
        return Err("GitHub non raggiungibile: controlla la connessione.".into());
    }
    let corpo = String::from_utf8_lossy(&output.stdout);
    leggi_release(&corpo, env!("CARGO_PKG_VERSION"))
}

/// Scarica l'installer solo dal repository ufficiale e verifica che sia un eseguibile.
pub fn scarica(url: &str) -> Result<PathBuf, String> {
    let prefisso = format!("https://github.com/{REPO}/releases/download/");
    if !url.starts_with(&prefisso) {
        return Err("Indirizzo di aggiornamento non riconosciuto.".into());
    }
    let destinazione = std::env::temp_dir().join("open-pdf-aggiornamento.exe");
    let percorso = destinazione.to_string_lossy().to_string();
    let output = esegui_curl(&[
        "-sSL",
        "--max-time",
        "600",
        "-o",
        &percorso,
        url,
    ])?;
    if !output.status.success() {
        return Err("Download dell'aggiornamento non riuscito.".into());
    }
    let dati = std::fs::read(&destinazione).map_err(|e| format!("File scaricato non leggibile: {e}"))?;
    if dati.len() < 1_000_000 || !dati.starts_with(b"MZ") {
        let _ = std::fs::remove_file(&destinazione);
        return Err("Il file scaricato non è un installer valido.".into());
    }
    Ok(destinazione)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confronto_delle_versioni() {
        assert_eq!(confronta_versioni("v1.1.0", "1.0.9"), Ordering::Greater);
        assert_eq!(confronta_versioni("1.0.0", "v1.0.0"), Ordering::Equal);
        assert_eq!(confronta_versioni("1.10.0", "1.9.9"), Ordering::Greater);
        assert_eq!(confronta_versioni("2.0", "2.0.1"), Ordering::Less);
        assert_eq!(confronta_versioni("v1.2", "1.2.0"), Ordering::Equal);
    }

    #[test]
    fn release_letta_e_versioni_scelte() {
        let json = r#"[
            {
                "tag_name": "v1.2.0",
                "body": "Frecce di annulla.",
                "assets": [
                    {"name": "open-pdf-mcp-windows-x64.zip", "browser_download_url": "https://github.com/Lore11-P/open-pdf/releases/download/v1.2.0/open-pdf-mcp-windows-x64.zip"},
                    {"name": "Open.PDF_1.2.0_x64-setup.exe", "browser_download_url": "https://github.com/Lore11-P/open-pdf/releases/download/v1.2.0/Open.PDF_1.2.0_x64-setup.exe"}
                ]
            },
            {
                "tag_name": "v1.1.1",
                "body": "Apri con Windows.",
                "assets": [
                    {"name": "Open.PDF_1.1.1_x64-setup.exe", "browser_download_url": "https://github.com/Lore11-P/open-pdf/releases/download/v1.1.1/Open.PDF_1.1.1_x64-setup.exe"}
                ]
            },
            {
                "tag_name": "v1.0.0",
                "body": "Prima release del server MCP.",
                "assets": [
                    {"name": "open-pdf-mcp-windows-x64.zip", "browser_download_url": "https://github.com/Lore11-P/open-pdf/releases/download/v1.0.0/open-pdf-mcp-windows-x64.zip"}
                ]
            },
            {
                "tag_name": "v9.0.0",
                "draft": true,
                "assets": [
                    {"name": "Open.PDF_9.0.0_x64-setup.exe", "browser_download_url": "https://github.com/Lore11-P/open-pdf/releases/download/v9.0.0/Open.PDF_9.0.0_x64-setup.exe"}
                ]
            }
        ]"#;
        let esito = leggi_release(json, "1.0.0").unwrap();
        assert!(esito.disponibile);
        assert_eq!(esito.attuale, "1.0.0");
        assert_eq!(esito.versione, "1.2.0");
        assert!(esito.installer.as_deref().unwrap().ends_with("Open.PDF_1.2.0_x64-setup.exe"));
        // La 1.0.0 non ha installer: non è una versione a cui tornare.
        assert!(esito.precedente.is_none());

        let aggiornato = leggi_release(json, "1.2.0").unwrap();
        assert!(!aggiornato.disponibile);
        let precedente = aggiornato.precedente.expect("versione precedente mancante");
        assert_eq!(precedente.versione, "1.1.1");
        assert!(precedente.installer.ends_with("Open.PDF_1.1.1_x64-setup.exe"));
    }
}
