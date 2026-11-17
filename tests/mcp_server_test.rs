//! Verifica che il server MCP risponda al handshake e pubblichi gli strumenti.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn risposta(uscita: &mut impl BufRead) -> String {
    let mut riga = String::new();
    uscita.read_line(&mut riga).expect("lettura della risposta");
    riga
}

#[test]
fn handshake_e_strumenti() {
    let mut figlio = Command::new(env!("CARGO_BIN_EXE_open-pdf-mcp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("avvio del server MCP");
    let mut ingresso = figlio.stdin.take().unwrap();
    let mut uscita = BufReader::new(figlio.stdout.take().unwrap());

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

    let sconosciuto = risposta(&mut uscita);
    assert!(
        sconosciuto.contains("\"code\":-32602"),
        "errore atteso per lo strumento sconosciuto: {sconosciuto}"
    );

    drop(ingresso);
    let _ = figlio.wait();
}
