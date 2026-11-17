//! Verifica che il server MCP risponda al handshake, pubblichi gli strumenti e crei documenti.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

fn avvia() -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut figlio = Command::new(env!("CARGO_BIN_EXE_open-pdf-mcp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("avvio del server MCP");
    let ingresso = figlio.stdin.take().unwrap();
    let uscita = BufReader::new(figlio.stdout.take().unwrap());
    (figlio, ingresso, uscita)
}

fn risposta(uscita: &mut impl BufRead) -> String {
    let mut riga = String::new();
    uscita.read_line(&mut riga).expect("lettura della risposta");
    riga
}

#[test]
fn handshake_e_strumenti() {
    let (mut figlio, mut ingresso, mut uscita) = avvia();

    let richieste = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}}"#,
        // Una notifica non deve produrre risposte.
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"strumento_inesistente","arguments":{}}}"#,
    ];
    for richiesta in richieste {
        writeln!(ingresso, "{richiesta}").unwrap();
    }
    ingresso.flush().unwrap();

    let apertura = risposta(&mut uscita);
    assert!(
        apertura.contains("\"protocolVersion\":\"2025-06-18\""),
        "risposta di apertura inattesa: {apertura}"
    );

    let elenco = risposta(&mut uscita);
    let strumenti = elenco.matches("\"inputSchema\"").count();
    assert!(strumenti >= 15, "strumenti pubblicati: {strumenti}");
    assert!(
        elenco.contains("\"name\":\"pdf_sign_pades\"") && elenco.contains("\"name\":\"pdf_redact\""),
        "strumenti attesi assenti dall'elenco"
    );
    assert!(
        elenco.contains("\"name\":\"doc_create\""),
        "doc_create assente dall'elenco"
    );

    let sconosciuto = risposta(&mut uscita);
    assert!(
        sconosciuto.contains("\"code\":-32602"),
        "errore atteso per lo strumento sconosciuto: {sconosciuto}"
    );

    drop(ingresso);
    let _ = figlio.wait();
}

#[test]
fn doc_create_scrive_testo_e_html() {
    let cartella = std::env::temp_dir().join(format!("openpdf-mcp-{}", std::process::id()));
    std::fs::create_dir_all(&cartella).unwrap();
    let (mut figlio, mut ingresso, mut uscita) = avvia();

    writeln!(
        ingresso,
        "{}",
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}
        })
    )
    .unwrap();

    for (id, nome) in [(2, "nota.txt"), (3, "nota.html")] {
        writeln!(
            ingresso,
            "{}",
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {
                    "name": "doc_create",
                    "arguments": {
                        "output": cartella.join(nome).to_string_lossy(),
                        "title": "Promemoria",
                        "content": "## Sezione\n- primo punto\n- secondo punto"
                    }
                }
            })
        )
        .unwrap();
    }
    ingresso.flush().unwrap();
    assert!(risposta(&mut uscita).contains("\"protocolVersion\""));

    for _ in 0..2 {
        let esito = risposta(&mut uscita);
        assert!(!esito.contains("\"isError\""), "errore inatteso: {esito}");
    }

    let testo = std::fs::read_to_string(cartella.join("nota.txt")).unwrap();
    assert!(testo.starts_with("Promemoria\n\n"), "testo: {testo}");
    assert!(testo.contains("Sezione\n• primo punto"), "testo: {testo}");

    let html = std::fs::read_to_string(cartella.join("nota.html")).unwrap();
    assert!(html.contains("<h1>Promemoria</h1>"), "html: {html}");
    assert!(html.contains("<li>primo punto</li>"), "html: {html}");

    drop(ingresso);
    let _ = figlio.wait();
    let _ = std::fs::remove_dir_all(&cartella);
}
