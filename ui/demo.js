/* Open PDF · Focus Dark — modalità dimostrativa (solo anteprima statica) */
function demoSignature() {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="420" height="140" viewBox="0 0 420 140">
    <path d="M24 92c18-38 31-55 44-55 15 0 6 34 21 34 14 0 17-28 33-28 13 0 6 24 19 24 10 0 17-17 28-17 12 0 10 15 22 15 16 0 25-33 44-33 18 0 5 29 22 29 12 0 21-13 31-13 12 0 15 10 26 10 15 0 27-18 40-18" fill="none" stroke="#3b4654" stroke-width="3.6" stroke-linecap="round"/>
    <path d="M40 112c68-8 149-9 222-4 34 2 64 6 92 12" fill="none" stroke="#3b4654" stroke-width="2" stroke-linecap="round" opacity=".5"/>
  </svg>`;
  return 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(svg);
}

function demoPage(pageIndex) {
  if (state.demoPages[pageIndex]) return state.demoPages[pageIndex];
  const titles = [
    'Proposta di progetto',
    'Obiettivi e risultati attesi',
    'Architettura della soluzione',
    'Piano di lavoro e milestone',
    'Budget e risorse',
    'Sicurezza, privacy e conformità'
  ];
  const subtitle = [
    'Aurora Digital Workspace · 2026',
    'Una esperienza documentale locale, veloce e riservata',
    'Moduli, flussi e integrazioni con i sistemi esistenti',
    'Tempistiche, dipendenze e responsabilità',
    'Investimento complessivo e articolazione dei costi',
    'Tutto resta sul dispositivo, senza inviare dati al cloud'
  ];
  const title = titles[pageIndex] || titles[0];
  const sub = subtitle[pageIndex] || subtitle[0];
  const p1 = pageIndex === 0
    ? 'Il presente documento descrive la proposta per un ambiente di lavoro documentale completamente locale. L\'obiettivo è offrire lettura, annotazione, firma, conversione e controllo dei file PDF senza dipendere da servizi esterni.'
    : 'Questa sezione raccoglie le informazioni principali del progetto, le scelte tecniche adottate e i criteri condivisi con il gruppo di lavoro. Ogni scelta è orientata alla semplicità d\'uso e alla piena sovranità dei dati.';
  const p2 = 'L\'interfaccia utilizza un linguaggio visivo ispirato al vetro: superfici traslucide, profondità morbide e micro-animazioni discrete. Le funzioni principali sono raggiungibili in un solo passaggio, mentre le operazioni avanzate restano disponibili senza appesantire la lettura.';
  const bullets = pageIndex === 0
    ? ['Lettura fluida con ricerca istantanea e sintesi vocale', 'Annotazioni native, timbri e firma a mano libera', 'Organizzazione delle pagine e compressione controllata', 'Moduli compilabili e richiesta di firme in rete locale']
    : ['Esperienza interamente offline e crittografia dei documenti', 'Conversione da e verso i formati Office più diffusi', 'Redazione irreversibile e confronto automatico delle versioni', 'Accessibilità e chiarezza per ogni fase del flusso'];
  const rows = pageIndex === 0
    ? [['Fase', 'Durata', 'Output'], ['Scoperta e allineamento', '2 settimane', 'Requisiti validati'], ['Design e prototipo', '3 settimane', 'Interfaccia approvata'], ['Sviluppo e test', '6 settimane', 'Build stabile'], ['Rilascio locale', '1 settimana', 'Formazione e handover']]
    : [['Attività', 'Responsabile', 'Stato'], ['Analisi dei contenuti', 'Studio Vertex', 'Completata'], ['Prototipo visivo', 'Design team', 'In corso'], ['Verifica sicurezza', 'IT interno', 'Pianificata'], ['Collaudo finale', 'Gruppo utenti', 'Da avviare']];

  const bars = [72, 88, 64, 93, 78, 86];
  const barSvg = bars.map((v, i) => `<rect x="${812 + i * 48}" y="${1270 - v * 2.6}" width="28" height="${v * 2.6}" rx="7" fill="url(#grad)" opacity="${.55 + i * .06}"/>`).join('');
  const bulletSvg = bullets.map((b, i) => `<g><circle cx="150" cy="${760 + i * 44}" r="6" fill="#8d97a2"/><text x="172" y="${767 + i * 44}" font-family="Segoe UI, Arial, sans-serif" font-size="20" fill="#334155">${b}</text></g>`).join('');
  const tableRows = rows.map((r, i) => {
    const y = 1000 + i * 46;
    const head = i === 0;
    return `<rect x="100" y="${y - 28}" width="1040" height="42" rx="8" fill="${head ? '#f1f5f9' : i % 2 ? '#fbfdff' : '#f8fafc'}"/>
      <text x="118" y="${y}" font-family="Segoe UI, Arial, sans-serif" font-size="${head ? 17 : 18}" font-weight="${head ? 700 : 500}" fill="${head ? '#64748b' : '#1e293b'}">${r[0]}</text>
      <text x="620" y="${y}" font-family="Segoe UI, Arial, sans-serif" font-size="${head ? 17 : 18}" font-weight="${head ? 700 : 500}" fill="${head ? '#64748b' : '#1e293b'}">${r[1]}</text>
      <text x="900" y="${y}" font-family="Segoe UI, Arial, sans-serif" font-size="${head ? 17 : 18}" font-weight="${head ? 700 : 500}" fill="${head ? '#64748b' : '#1e293b'}">${r[2]}</text>`;
  }).join('');

  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="1240" height="1754" viewBox="0 0 1240 1754">
  <defs>
    <linearGradient id="grad" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#5f6b78"/><stop offset="1" stop-color="#a9b2ba"/></linearGradient>
    <linearGradient id="soft" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#8d97a2" stop-opacity=".14"/><stop offset="1" stop-color="#a9b2ba" stop-opacity=".03"/></linearGradient>
  </defs>
  <rect width="1240" height="1754" fill="#ffffff"/>
  <rect width="1240" height="14" fill="url(#grad)"/>
  <text x="100" y="86" font-family="Segoe UI, Arial, sans-serif" font-size="15" font-weight="700" letter-spacing="3.2" fill="#5f6b78">OPEN PDF · STUDIO</text>
  <text x="1140" y="86" text-anchor="end" font-family="Segoe UI, Arial, sans-serif" font-size="14" fill="#94a3b8">Documento riservato · ${String(pageIndex + 1).padStart(2, '0')} / 06</text>
  <line x1="100" y1="112" x2="1140" y2="112" stroke="#e2e8f0" stroke-width="1"/>
  <text x="100" y="210" font-family="Segoe UI Variable Display, Segoe UI, Arial, sans-serif" font-size="${pageIndex === 0 ? 64 : 54}" font-weight="700" letter-spacing="-1.6" fill="#0f172a">${title}</text>
  <text x="102" y="258" font-family="Segoe UI, Arial, sans-serif" font-size="23" fill="#64748b">${sub}</text>
  <rect x="100" y="296" width="290" height="6" rx="3" fill="url(#grad)"/>
  <g font-family="Segoe UI, Arial, sans-serif">
    <text x="100" y="352" font-size="15" fill="#94a3b8" letter-spacing="1.4">PREPARATO PER</text>
    <text x="100" y="382" font-size="20" font-weight="600" fill="#334155">Studio Aurora S.r.l.</text>
    <text x="430" y="352" font-size="15" fill="#94a3b8" letter-spacing="1.4">DATA</text>
    <text x="430" y="382" font-size="20" font-weight="600" fill="#334155">12 novembre 2026</text>
    <text x="760" y="352" font-size="15" fill="#94a3b8" letter-spacing="1.4">VERSIONE</text>
    <text x="760" y="382" font-size="20" font-weight="600" fill="#334155">1.0 · Revisione</text>
  </g>
  <rect x="100" y="428" width="1040" height="220" rx="18" fill="url(#soft)" stroke="#ddd6fe"/>
  <text x="132" y="482" font-family="Segoe UI, Arial, sans-serif" font-size="17" font-weight="700" letter-spacing="1.8" fill="#5f6b78">SINTESI ESECUTIVA</text>
  <text x="132" y="522" font-family="Segoe UI, Arial, sans-serif" font-size="21" fill="#1e293b">
    <tspan x="132" dy="0">${p1.slice(0, 78)}</tspan>
    <tspan x="132" dy="32">${p1.slice(78, 156)}</tspan>
    <tspan x="132" dy="32">${p1.slice(156, 232)}</tspan>
    <tspan x="132" dy="32">${p1.slice(232)}</tspan>
  </text>
  <text x="100" y="716" font-family="Segoe UI Variable Display, Segoe UI, Arial, sans-serif" font-size="30" font-weight="700" fill="#0f172a">Ambito e obiettivi</text>
  <text x="100" y="752" font-family="Segoe UI, Arial, sans-serif" font-size="20" fill="#475569">${p2}</text>
  ${bulletSvg}
  <text x="100" y="948" font-family="Segoe UI Variable Display, Segoe UI, Arial, sans-serif" font-size="30" font-weight="700" fill="#0f172a">${pageIndex === 0 ? 'Piano di massima' : 'Attività e responsabilità'}</text>
  ${tableRows}
  <rect x="780" y="1120" width="360" height="240" rx="18" fill="#f8fafc" stroke="#e2e8f0"/>
  <text x="812" y="1160" font-family="Segoe UI, Arial, sans-serif" font-size="15" font-weight="700" letter-spacing="1.4" fill="#64748b">AVANZAMENTO</text>
  ${barSvg}
  <text x="812" y="1338" font-family="Segoe UI, Arial, sans-serif" font-size="15" fill="#94a3b8">Indice di confidenza del piano</text>
  <rect x="100" y="1420" width="520" height="150" rx="18" fill="#ffffff" stroke="#e2e8f0"/>
  <text x="128" y="1462" font-family="Segoe UI, Arial, sans-serif" font-size="15" font-weight="700" letter-spacing="1.4" fill="#64748b">NOTA DEL REVISORE</text>
  <text x="128" y="1502" font-family="Segoe UI, Arial, sans-serif" font-size="19" fill="#334155">La proposta è solida. Allineare il budget</text>
  <text x="128" y="1528" font-family="Segoe UI, Arial, sans-serif" font-size="19" fill="#334155">alla fase due e confermare le date.</text>
  <text x="100" y="1652" font-family="Segoe UI, Arial, sans-serif" font-size="15" fill="#94a3b8">Open PDF · Focus Dark · Documento dimostrativo</text>
  <text x="1140" y="1652" text-anchor="end" font-family="Segoe UI, Arial, sans-serif" font-size="15" fill="#94a3b8">Pagina ${pageIndex + 1} di 6</text>
  <line x1="100" y1="1672" x2="1140" y2="1672" stroke="#e2e8f0"/>
  </svg>`;
  const url = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(svg);
  state.demoPages[pageIndex] = url;
  return url;
}

function demoAnnotations() {
  return [
    {
      id: 'demo-h1', page: 0, type: 'highlight', x: .083, y: .108, w: .42, h: .032,
      points: [], color: '#d6c58d', opacity: .38, stroke: .003, text: '', fontSize: .018, label: '', dataUrl: '', author: 'Tu', createdAt: '12/11/2026, 09:14'
    },
    {
      id: 'demo-u1', page: 0, type: 'underline', x: .084, y: .146, w: .38, h: .004,
      points: [], color: '#a9b2ba', opacity: 1, stroke: .004, text: '', fontSize: .018, label: '', dataUrl: '', author: 'Tu', createdAt: '12/11/2026, 09:15'
    },
    {
      id: 'demo-n1', page: 0, type: 'note', x: .685, y: .148, w: .235, h: .1,
      points: [], color: '#fde68a', opacity: 1, stroke: .003, text: 'Allineare il budget alla fase 2 e condividere il piano con il team.', fontSize: .011, label: '', dataUrl: '', author: 'Giulia', createdAt: '12/11/2026, 09:22'
    },
    {
      id: 'demo-s1', page: 0, type: 'stamp', x: .66, y: .345, w: .255, h: .07,
      points: [], color: '#93a99b', opacity: .95, stroke: .0035, text: 'APPROVATO', fontSize: .018, label: 'APPROVATO', dataUrl: '', author: 'Tu', createdAt: '12/11/2026, 09:30'
    },
    {
      id: 'demo-t1', page: 0, type: 'text', x: .086, y: .565, w: .38, h: .032,
      points: [], color: '#0f172a', opacity: 1, stroke: .003, text: 'Aggiornare la timeline entro venerdì.', fontSize: .017, label: '', dataUrl: '', author: 'Tu', createdAt: '12/11/2026, 09:34'
    },
    {
      id: 'demo-i1', page: 0, type: 'ink', x: .11, y: .745, w: .24, h: .045,
      points: [[.11, .77], [.14, .75], [.17, .775], [.2, .755], [.24, .785], [.29, .75], [.35, .775]],
      color: '#b7a49d', opacity: 1, stroke: .0045, text: '', fontSize: .018, label: '', dataUrl: '', author: 'Tu', createdAt: '12/11/2026, 09:36'
    },
    {
      id: 'demo-sig1', page: 0, type: 'signature', x: .62, y: .72, w: .22, h: .075,
      points: [], color: '#3b4654', opacity: 1, stroke: .003, text: '', fontSize: .018, label: '', dataUrl: demoSignature(), author: 'Studio Vertex', createdAt: '12/11/2026, 09:41'
    }
  ];
}

function demoInfo() {
  return {
    sessionId: 'demo-session',
    path: 'C:\\Users\\utente\\Documents\\Aurora\\Proposta_Progetto_2026.pdf',
    name: 'Proposta_Progetto_2026.pdf',
    pageCount: 6,
    pages: Array.from({ length: 6 }, (_, i) => ({
      index: i, width: 595, height: 842, rotation: 0, displayWidth: 595, displayHeight: 842
    })),
    meta: {
      title: 'Proposta di progetto · Aurora Digital Workspace',
      author: 'Studio Vertex',
      subject: 'Ambiente documentale locale',
      keywords: 'PDF, focus dark, locale, produttività',
      creator: 'Open PDF',
      producer: 'Open PDF 1.0',
      creationDate: '12/11/2026',
      modificationDate: '12/11/2026'
    },
    annotations: demoAnnotations(),
    formFields: [],
    fileSize: 1843200
  };
}

async function loadDemo() {
  state.demo = true;
  applyOpenInfo(demoInfo());
  applyEngineStatus({ pdfium: true, pdfiumPath: 'demo', edge: true, office: true, tesseract: false, ocrWindows: true, version: 'PDFium · WebView2 · OCR Windows' });
  setStatus("Modalità dimostrativa · apri l'app per usare i tuoi file");
  toast('Anteprima dimostrativa. Apri un PDF per iniziare.', 'info', 4200);
}

function demoSearch(query) {
  const q = query.toLowerCase();
  const hits = [];
  const snippets = [
    'Proposta di progetto · Aurora Digital Workspace · 2026',
    'Il presente documento descrive la proposta per un ambiente di lavoro documentale completamente locale.',
    'Lettura fluida con ricerca istantanea e sintesi vocale. Annotazioni native, timbri e firma a mano libera.'
  ];
  snippets.forEach((text, i) => {
    if (text.toLowerCase().includes(q) || i === 0) {
      const rects = [[.083, .10 + i * .03, .34, .024], [.085, .13 + i * .03, .42, .024]];
      hits.push({ page: i, text, rects });
    }
  });
  return hits;
}


function demoTextLines(page) {
  if (page !== 0) {
    return [
      { text: ['Proposta di progetto','Obiettivi e risultati attesi','Architettura della soluzione','Piano di lavoro e milestone','Budget e risorse','Sicurezza, privacy e conformità'][page] || 'Documento di prova', x: .083, y: .108, w: .42, h: .035 },
      { text: 'Open PDF · Focus Dark · anteprima documento', x: .083, y: .155, w: .42, h: .016 }
    ];
  }
  return [
    { text: 'Proposta di progetto', x: .083, y: .108, w: .42, h: .035 },
    { text: 'Aurora Digital Workspace · 2026', x: .083, y: .143, w: .35, h: .016 },
    { text: 'Studio Aurora S.r.l.', x: .083, y: .198, w: .18, h: .016 },
    { text: '12 novembre 2026', x: .347, y: .198, w: .14, h: .016 },
    { text: '1.0 · Revisione', x: .613, y: .198, w: .12, h: .016 },
    { text: 'SINTESI ESECUTIVA', x: .106, y: .265, w: .16, h: .016 },
    { text: 'Il presente documento descrive la proposta per un ambiente di lavoro documentale', x: .106, y: .286, w: .58, h: .018 },
    { text: 'completamente locale. L\'obiettivo è offrire lettura, annotazione, firma, conversione', x: .106, y: .304, w: .58, h: .018 },
    { text: 'e controllo dei file PDF senza dipendere da servizi esterni.', x: .106, y: .322, w: .38, h: .018 },
    { text: 'Ambito e obiettivi', x: .083, y: .405, w: .22, h: .025 },
    { text: 'Lettura fluida con ricerca istantanea e sintesi vocale', x: .12, y: .43, w: .4, h: .016 },
    { text: 'Annotazioni native, timbri e firma a mano libera', x: .12, y: .457, w: .36, h: .016 },
    { text: 'Organizzazione delle pagine e compressione controllata', x: .12, y: .484, w: .4, h: .016 },
    { text: 'Moduli compilabili e richiesta di firme in rete locale', x: .12, y: .51, w: .38, h: .016 },
    { text: 'Piano di massima', x: .083, y: .535, w: .2, h: .025 },
    { text: 'Scoperta e allineamento', x: .09, y: .565, w: .17, h: .016 },
    { text: 'Design e prototipo', x: .09, y: .591, w: .15, h: .016 },
    { text: 'Sviluppo e test', x: .09, y: .618, w: .13, h: .016 },
    { text: 'Rilascio locale', x: .09, y: .644, w: .12, h: .016 }
  ];
}
