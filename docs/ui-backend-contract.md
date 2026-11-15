# Contratto UI ↔ backend (da rispettare nell'integrazione)

Preso dalla UI aggiornata (`ui/ui.js`). L'agente di integrazione (main.rs /
types.rs) deve accettare questi payload; la UI ha comunque un fallback al
payload storico per la firma.

## Firma PAdES — `sign_pdf_pades`

Payload nuovo (tutti i campi possono arrivare):

```json
{
  "page": 0,
  "certPath": "C:\\...\\cert.pem | C:\\...\\firma.pfx",
  "keyPath": "C:\\...\\key.pem | ''",
  "password": "'' | password pfx",
  "format": "pem | pfx",
  "visible": true,
  "certification": "none | P1 | P2 | P3",
  "tsaUrl": "'' | https://tsa.esempio.it/tsr",
  "name": "Mario Rossi",
  "reason": "Approvazione",
  "location": "Roma"
}
```

Fallback usato se il backend rifiuta gli argomenti nuovi (solo PEM):

```json
{ "page": 0, "certPath": "...", "keyPath": "...", "name": "...", "reason": "...", "location": "..." }
```

`pick_certificate` deve accettare anche `kind: "p12"` (filtri `.p12`, `.pfx`).

Risposta: `SignResult` piatta (`#[serde(flatten)]` di `OpenInfo` + `message`)
oppure `{ info, message }`: la UI accetta entrambe. Se manca `pageCount` la UI
mostra solo il messaggio e non applica lo stato.

## Verifica firme — `verify_signatures_cmd`

Parametro opzionale: `{ "checkRevocation": true }` attiva le richieste OCSP. Senza
il parametro o con `false` la verifica resta offline.

La UI accetta un array o `{ signatures: [...] }`. Campi sempre gestiti:

- `field`, `signer`, `time`, `subfilter`
- `digestOk`, `signatureOk`, `coversAll`

Campi extra opzionali (mostrati se presenti, tipi tolleranti):

- `chainValid` (bool)
- `revocation` (stringa o oggetto con `status`/`state`/`label`/`level`)
- `timestamp` (stringa o oggetto)
- `validFrom`, `validTo` (stringhe)
- `docmdp` (numero o stringa)

## Modifica testo — `replace_text_in_place`

```json
{ "page": 0, "rect": [x, y, w, h], "text": "nuovo testo, anche multi-riga" }
```

## Redazione — `apply_redactions`

Invariato: `{ "annotations": [...] }`. La UI mostra `message` della risposta
(`RedactResult`) in un modal di riepilogo.

## Pulizia profonda — `sanitize_pdf_cmd`

Nessun argomento. Riscrive il documento aperto, lo riapre e restituisce
`OpenInfo` appiattito più `message` con il conteggio di metadati, script,
allegati, immagini, elementi nascosti e oggetti orfani rimossi.

## Note per l'integrazione

- `sign_pdf_pades` deve restare chiamabile anche con il payload storico: se si
  aggiungono parametri, usare `Option<T>` con `#[serde(default)]` e ignorare i
  campi assenti.
- `certification: "none"` deve significare nessuna DocMDP.
- Le risposte di `replace_text_in_place` e `apply_redactions` devono continuare
  a includere tutti i campi di `OpenInfo` (la UI li applica allo stato).
