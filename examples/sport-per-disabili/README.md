# Esempio: Sport per persone con disabilità

Presentazione in 10 slide, 16:9, usata come prova generale della creazione e della
conversione dei documenti.

| File | Cos'è |
| --- | --- |
| `Sport per disabili.pptx` | presentazione modificabile: testi, foto e forme native |
| `Sport per disabili.pdf` | stesso contenuto in PDF, per la distribuzione |
| `crediti.json` | fonte, licenza e autore di ogni foto |

## Contenuto

1. Copertina
2. Diritto: non è «sport speciale»
3. Le grandi manifestazioni (numeri di Parigi 2024)
4. La classificazione spiegata semplice
5. Discipline: dove si gareggia
6. Cosa cambia davvero (benefici)
7. Le barriere che restano
8. Da dove iniziare in Italia
9. Come raccontarlo (linguaggio)
10. Per approfondire, con i crediti delle foto

## Come è stata fatta

Testi, struttura e conversione sono stati preparati con Open PDF; impaginazione,
foto e PDF gemello sono stati rifiniti con uno script Python (`python-pptx` e Edge
headless) fuori dall'app. Il PDF è la fonte grafica, il PPTX la versione modificabile.

Nella prossima versione lo stesso risultato sarà ottenibile dall'agente con
`doc_create` per la struttura e `pdf_export` per la conversione.

## Licenze delle foto

Le immagini sono incorporate nei file e restano dei rispettivi autori. Provenienza e
licenze sono in [`crediti.json`](crediti.json): CC BY-SA 3.0 (Sport the library,
Elizabeth Edmondson, Jsg1224) e CC BY-SA 4.0 (Gail Leenstra), via Wikimedia Commons.
Le foto 04 e 06 sono state scaricate ma non usate nel deck.

Il resto del materiale segue la licenza MIT del progetto.
