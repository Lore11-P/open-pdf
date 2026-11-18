/* Open PDF · Focus Dark — core */
const HAS_TAURI = !!window.__TAURI__;
const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));
const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
const uid = () => (crypto.randomUUID ? crypto.randomUUID() : 'id-' + Math.random().toString(36).slice(2) + Date.now().toString(36));
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const fmtBytes = (n) => {
  if (!n) return '—';
  const u = ['B', 'KB', 'MB', 'GB'];
  let i = 0, v = n;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v >= 10 || i === 0 ? Math.round(v) : v.toFixed(1)} ${u[i]}`;
};

const ICONS = {
  search: '<circle cx="11" cy="11" r="7"/><path d="m20.5 20.5-4-4"/>',
  volume: '<path d="M11 5 6.5 9H3v6h3.5L11 19z"/><path d="M15.5 8.5a5 5 0 0 1 0 7"/><path d="M18.5 6a9 9 0 0 1 0 12"/>',
  stop: '<rect x="6" y="6" width="12" height="12" rx="2"/>',
  share: '<circle cx="6" cy="12" r="2.5"/><circle cx="17" cy="6" r="2.5"/><circle cx="17" cy="18" r="2.5"/><path d="m8.2 10.8 6.6-3.6M8.2 13.2l6.6 3.6"/>',
  save: '<path d="M5 4h11l3 3v13H5z"/><path d="M8 4v6h8V4"/><path d="M8 15h8v5H8z"/>',
  folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  book: '<path d="M4 5.5A2.5 2.5 0 0 1 6.5 3H20v15.5H6.5A2.5 2.5 0 0 0 4 21z"/><path d="M4 18.5A2.5 2.5 0 0 1 6.5 16H20"/>',
  zoomIn: '<circle cx="11" cy="11" r="7"/><path d="m20.5 20.5-4-4M11 8v6M8 11h6"/>',
  zoomOut: '<circle cx="11" cy="11" r="7"/><path d="m20.5 20.5-4-4M8 11h6"/>',
  fit: '<path d="M9 3H4v5M15 3h5v5M9 21H4v-5M15 21h5v-5"/>',
  left: '<path d="m14.5 6-6 6 6 6"/>',
  right: '<path d="m9.5 6 6 6-6 6"/>',
  rotateL: '<path d="M4 10a8 8 0 1 1 2 8"/><path d="M4 4v6h6"/>',
  rotateR: '<path d="M20 10a8 8 0 1 0-2 8"/><path d="M20 4v6h-6"/>',
  trash: '<path d="M4 7h16M9 7V4h6v3M6.5 7l1 13h9l1-13"/><path d="M10 11v5M14 11v5"/>',
  highlighter: '<path d="m9 14 7.5-7.5a2 2 0 0 1 3 3L12 17l-4 1z"/><path d="M6 21h12"/>',
  underline: '<path d="M7 4v7a5 5 0 0 0 10 0V4"/><path d="M5 21h14"/>',
  sticky: '<path d="M4 5a2 2 0 0 1 2-2h9l5 5v11a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2z"/><path d="M14 3v6h6"/><path d="M8 13h8M8 17h5"/>',
  stamp: '<path d="M8 14h8l1.5 5h-11z"/><path d="M7 19h10v2H7z"/><path d="M10 14V9a2 2 0 1 1 4 0v5"/>',
  pen: '<path d="M4 20c3-1 5-3 6-6l8-8a2.1 2.1 0 0 1 3 3l-8 8c-3 1-5 3-6 6z"/><path d="M14 7l3 3"/>',
  text: '<path d="M5 5h14M12 5v14M8 19h8"/>',
  signature: '<path d="M3 17c3 0 4-8 7-8s2 6 5 6 3-4 6-4"/><path d="M4 21h16"/>',
  pages: '<rect x="4" y="3" width="12" height="15" rx="1.5"/><path d="M8 21h12V8"/><path d="M8 8h5M8 11h5M8 14h3"/>',
  merge: '<path d="M7 4v5a3 3 0 0 0 3 3h4"/><path d="M17 4v5a3 3 0 0 1-3 3"/><path d="M14 12v8M11 17l3 3 3-3"/>',
  split: '<path d="M12 3v6"/><path d="M12 9 7 14v6M12 9l5 5v6"/><path d="M4 17l3-3 3 3M14 17l3-3 3 3"/>',
  compress: '<path d="M12 3v6M12 21v-6"/><path d="m8 7 4 4 4-4M8 17l4-4 4 4"/><path d="M4 12h16"/>',
  redact: '<rect x="4" y="6" width="16" height="12" rx="2"/><path d="M4 12h16"/><path d="M8 17h8"/>',
  image: '<rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="8.5" cy="9" r="1.6"/><path d="m4 17 5-5 4 4 3-3 4 4"/>',
  globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a15 15 0 0 1 0 18M12 3a15 15 0 0 0 0 18"/>',
  office: '<path d="M5 4h9l5 5v11H5z"/><path d="M14 4v5h5"/><path d="M9 13h6M9 17h6"/>',
  export: '<path d="M12 15V3"/><path d="m8 7 4-4 4 4"/><path d="M5 13v7h14v-7"/>',
  scan: '<path d="M4 8V5a1 1 0 0 1 1-1h3M16 4h3a1 1 0 0 1 1 1v3M20 16v3a1 1 0 0 1-1 1h-3M8 20H5a1 1 0 0 1-1-1v-3"/><path d="M7 12h10"/><path d="M9 9h6M9 15h6"/>',
  form: '<rect x="4" y="3" width="16" height="18" rx="2"/><path d="M8 8h8M8 12h8M8 16h5"/>',
  formPlus: '<rect x="3" y="3" width="13" height="18" rx="2"/><path d="M7 8h5M7 12h5M7 16h3"/><path d="M19 14v6M16 17h6"/>',
  signRequest: '<path d="M3 18c3 0 4-9 7-9s2 6 5 6 3-3 6-3"/><path d="M4 21h16"/><circle cx="18" cy="5" r="2.5"/>',
  lock: '<rect x="5" y="10" width="14" height="10" rx="2"/><path d="M8 10V7a4 4 0 0 1 8 0v3"/><circle cx="12" cy="15" r="1.2"/>',
  lockOpen: '<rect x="5" y="10" width="14" height="10" rx="2"/><path d="M8 10V7a4 4 0 0 1 7.5-2"/>',
  compare: '<path d="M7 4v13M17 7v13"/><path d="M4 7h6M7 4l-3 3 3 3"/><path d="M14 17h6M17 14l3 3-3 3"/>',
  metadata: '<circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7.5v.5"/>',
  close: '<path d="M6 6l12 12M18 6 6 18"/>',
  check: '<path d="m5 12 4 4L19 6"/>',
  alert: '<path d="M12 3 2 21h20z"/><path d="M12 9v5M12 17.5v.5"/>',
  info: '<circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7.5v.5"/>',
  copy: '<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V5a1 1 0 0 1 1-1h10"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  up: '<path d="m6 14 6-6 6 6"/>',
  down: '<path d="m6 10 6 6 6-6"/>',
  edit: '<path d="m4 20 4-1 10-10a2.1 2.1 0 0 0-3-3L5 16z"/><path d="M13 6l3 3"/>',
  download: '<path d="M12 3v12"/><path d="m7 10 5 5 5-5"/><path d="M4 21h16"/>',
  grid: '<rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/>',
  drag: '<circle cx="9" cy="6" r="1"/><circle cx="15" cy="6" r="1"/><circle cx="9" cy="12" r="1"/><circle cx="15" cy="12" r="1"/><circle cx="9" cy="18" r="1"/><circle cx="15" cy="18" r="1"/>'
};
function icon(name, cls = '') {
  return `<svg class="${cls}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">${ICONS[name] || ICONS.info}</svg>`;
}

const COLORS = ['#d6c58d', '#9db0b8', '#c1a39a', '#9db3a3', '#a9a5b8', '#c2a17b', '#b98b8b', '#93a7bb', '#ece8df', '#23262b'];
const STAMPS = [['APPROVATO', '#93a99b'], ['BOZZA', '#bfa77a'], ['CONFIDENZIALE', '#bb8e8e'], ['PAGATO', '#93a7bb'], ['DA FIRMARE', '#a6a3b5'], ['REVISIONATO', '#95a9ad']];
const STAMP_COLORS = Object.fromEntries(STAMPS);

const TOOL_META = {
  browse: { name: 'Lettura', icon: 'book', hint: 'Sfoglia il documento, usa lo zoom, seleziona le annotazioni e cerca parole.' },
  edittext: { name: 'Modifica testo', icon: 'edit', hint: "Clicca una riga per selezionarla, doppio clic per modificarla, Shift+clic per aggiungerne altre. Il nuovo testo sostituisce davvero quello originale." },
  highlight: { name: 'Evidenziatore', icon: 'highlighter', hint: 'Trascina sulla pagina per evidenziare una porzione di testo.' },
  underline: { name: 'Sottolineatura', icon: 'underline', hint: 'Trascina sotto una riga per tracciare una sottolineatura.' },
  note: { name: 'Nota adesiva', icon: 'sticky', hint: 'Clicca sulla pagina per applicare una nota con il tuo commento.' },
  stamp: { name: 'Timbro', icon: 'stamp', hint: 'Scegli un timbro qui sotto e clicca sulla pagina.' },
  ink: { name: 'Disegno a mano', icon: 'pen', hint: 'Disegna liberamente con mouse, penna o dito.' },
  text: { name: 'Testo', icon: 'text', hint: 'Clicca nel punto in cui vuoi inserire un blocco di testo.' },
  signature: { name: 'Firma', icon: 'signature', hint: 'Clicca sulla pagina per apporre la firma salvata, oppure creane una nuova.' },
  redact: { name: 'Redazione', icon: 'redact', hint: 'Disegna aree nere sui dati sensibili, poi applica la redazione permanente.' },
  createform: { name: 'Crea modulo', icon: 'formPlus', hint: 'Clicca sulla pagina per posizionare campi compilabili. Scegli il tipo qui sotto.' },
  fill: { name: 'Compila modulo', icon: 'form', hint: 'Modifica i campi del modulo e salva una copia compilata.' }
};

const state = {
  doc: null,
  demo: false,
  page: 0,
  zoom: 1,
  tool: 'browse',
  color: '#d6c58d',
  opacity: 0.35,
  stroke: 0.003,
  fontSize: 0.018,
  stampLabel: 'APPROVATO',
  annotations: [],
  selected: null,
  textLines: [],
  textSelection: [],
  textLinesSeq: 0,
  undo: [],
  redo: [],
  drag: null,
  temp: null,
  search: { query: '', hits: [], active: false },
  thumbs: [],
  demoPages: {},
  signatures: localStorage.getItem('openpdf.signature') || '',
  pendingSignaturePoint: null,
  speaking: false,
  speakTimer: null,
  share: null,
  comments: [],
  shareTimer: null,
  creatingForm: false,
  newFieldKind: 'text',
  newFields: [],
  orgOrder: [],
  orgDeleted: new Set(),
  renderSeq: 0,
  renderTimer: null,
  saveTimer: null,
  formFields: []
};

window.addEventListener('error', (e) => { if (typeof toast === 'function') toast('Errore interfaccia: ' + (e.message || 'sconosciuto'), 'error', 6000); });
window.addEventListener('unhandledrejection', (e) => { if (typeof toast === 'function') toast('Errore: ' + (e.reason?.message || e.reason || 'sconosciuto'), 'error', 6000); });
document.addEventListener('DOMContentLoaded', () => { if (typeof init === 'function') init(); });

async function invoke(cmd, args = {}) {
  if (!HAS_TAURI) return demoInvoke(cmd, args);
  try {
    return await window.__TAURI__.core.invoke(cmd, args);
  } catch (err) {
    const msg = typeof err === 'string' ? err : (err?.message || String(err));
    throw new Error(msg);
  }
}
async function demoInvoke(cmd) {
  if (cmd === 'engine_status') {
    return { pdfium: true, pdfiumPath: 'demo', edge: true, office: true, tesseract: false, ocrWindows: true, version: 'PDFium · WebView2 · OCR Windows' };
  }
  if (cmd === 'pick_pdf' || cmd === 'pick_file' || cmd === 'pick_save') return null;
  if (cmd === 'pick_pdfs' || cmd === 'pick_images') return [];
  throw new Error("Funzione disponibile nell'app desktop Open PDF.");
}

function applyEngineStatus(st) {
  const set = (id, ok, title) => {
    const el = document.getElementById(id);
    if (!el) return;
    el.classList.toggle('ok', !!ok);
    el.classList.toggle('warn', !ok);
    el.title = title || '';
  };
  set('enginePdfium', st.pdfium, st.pdfiumPath || '');
  set('engineOcr', st.ocrWindows, st.ocrWindows ? 'OCR di Windows disponibile' : 'OCR Windows non disponibile');
  set('engineOffice', st.office, st.office ? 'LibreOffice rilevato' : 'LibreOffice non trovato: conversione base');
  const se = $('#statusEngine');
  if (se) se.textContent = st.version || 'Motore PDFium';
  if (!st.pdfium) toast('Motore PDFium non trovato: controlla engines/pdfium.dll', 'error', 7000);
}

/* ---------------- open / info ---------------- */
async function openDialog() {
  try {
    const path = await invoke('pick_pdf');
    if (path) openPath(path);
  } catch (e) { toast(normalizeError(e), 'error'); }
}
async function openPath(path, password) {
  if (!HAS_TAURI) return toast("Apri l'app desktop per selezionare file.", 'warn');
  setStatus('Apertura del documento…', true);
  try {
    const info = await invoke('open_pdf', { path, password: password || null });
    state.demo = false;
    applyOpenInfo(info);
    toast('Documento aperto.');
  } catch (e) {
    const msg = normalizeError(e);
    if (/password/i.test(msg) && !password) {
      const pw = await askPassword();
      if (pw) return openPath(path, pw);
    }
    toast(msg, 'error', 6000);
    setStatus('Apertura non riuscita');
  }
}
function applyOpenInfo(info, annotationsOverride) {
  state.doc = info;
  state.page = clamp(state.page, 0, Math.max(0, info.pageCount - 1));
  state.annotations = annotationsOverride || (Array.isArray(info.annotations) ? info.annotations : []);
  state.formFields = info.formFields || [];
  state.selected = null;
  state.undo = [];
  state.redo = [];
  state.search = { query: '', hits: [], active: false };
  state.newFields = [];
  state.creatingForm = false;
  state.textLines = [];
  state.textSelection = [];
  if (state.share) stopShare(true);
  $('#shell').classList.add('has-doc');
  $('#emptyState').hidden = true;
  $('#pageFrame').hidden = false;
  $('#viewerDock').hidden = false;
  $('#docTitle').textContent = info.originalName || info.name || 'Documento';
  $('#docPath').textContent = truncatePath(info.originalPath || info.path || '', 84);
  $('#viewerSub').textContent = `${info.pageCount} pagine · ${fmtBytes(info.fileSize)}`;
  $('#pageTotal').textContent = info.pageCount;
  $('#pageInput').value = state.page + 1;
  $('#metaPages').textContent = info.pageCount;
  $('#metaSize').textContent = fmtBytes(info.fileSize);
  $('#metaTitle').textContent = info.meta?.title || '—';
  $('#metaAuthor').textContent = info.meta?.author || '—';
  $('#chipSize').textContent = firstPageSize(info);
  state.zoom = 1;
  updateZoomLabel();
  layoutPage();
  renderPage();
  loadThumbnails();
  refreshAnnots();
  renderFieldLayer();
  if (state.formFields.length) toast(`${state.formFields.length} campi modulo rilevati.`, 'info', 4200);
  setStatus(`Pronto · ${info.name || ''}`);
  $('#viewerScroll').scrollTop = 0;
  if (state.tool === 'edittext' && typeof loadTextLines === 'function') setTimeout(loadTextLines, 60);
}
function normalizeError(e) {
  let msg = typeof e === 'string' ? e : (e?.message || String(e));
  return msg.replace(/^Error:\s*/i, '');
}
function truncatePath(p, max) {
  if (!p) return '';
  if (p.length <= max) return p;
  return p.slice(0, 22) + '…' + p.slice(-(max - 24));
}
function firstPageSize(info) {
  const p = info.pages?.[0];
  if (!p) return '—';
  return `${Math.round((p.displayWidth || 595) * 0.3528)} × ${Math.round((p.displayHeight || 842) * 0.3528)} mm`;
}

/* ---------------- rendering ---------------- */
function layoutPage() {
  if (!state.doc) return;
  const info = state.doc.pages?.[state.page] || state.doc.pages?.[0];
  if (!info) return;
  const availW = Math.max(280, $('#viewerScroll').clientWidth - 64);
  const availH = Math.max(300, $('#viewerScroll').clientHeight - 92);
  const fit = Math.min(availW / info.displayWidth, availH / info.displayHeight);
  const width = Math.max(220, Math.round(info.displayWidth * fit * state.zoom));
  $('#pageFrame').style.width = width + 'px';
  renderFieldLayer();
}
function renderPage() {
  if (!state.doc) return;
  layoutPage();
  const seq = ++state.renderSeq;
  $('#pageSpinner').classList.add('show');
  const info = state.doc.pages?.[state.page];
  if (!info) { $('#pageSpinner').classList.remove('show'); return; }
  if (state.demo) {
    const src = demoPage(state.page);
    $('#pageImg').onload = () => {
      if (seq !== state.renderSeq) return;
      $('#pageSpinner').classList.remove('show');
      drawOverlay();
    };
    $('#pageImg').src = src;
    return;
  }
  const cssW = $('#pageFrame').clientWidth || 800;
  const target = clamp(Math.round(cssW * Math.min(2, window.devicePixelRatio || 1)), 640, 3200);
  invoke('render_page', { page: state.page, width: target }).then(res => {
    if (seq !== state.renderSeq) return;
    $('#pageImg').onload = () => {
      if (seq !== state.renderSeq) return;
      $('#pageSpinner').classList.remove('show');
      drawOverlay();
    };
    $('#pageImg').src = 'data:image/png;base64,' + res.png;
    drawOverlay();
  }).catch(e => {
    if (seq !== state.renderSeq) return;
    $('#pageSpinner').classList.remove('show');
    toast('Rendering non riuscito: ' + normalizeError(e), 'error');
  });
}
function debounceRender() {
  clearTimeout(state.renderTimer);
  state.renderTimer = setTimeout(renderPage, 220);
}
function setZoom(z, anchorEvent = null) {
  if (!state.doc) return;
  const next = clamp(z, 0.35, 4);
  if (Math.abs(next - state.zoom) < 0.00005) return;
  const scroll = $('#viewerScroll');
  const frame = $('#pageFrame');
  let anchor = null;
  if (anchorEvent && scroll && frame) {
    const fr = frame.getBoundingClientRect();
    const ax = anchorEvent.clientX - fr.left;
    const ay = anchorEvent.clientY - fr.top;
    if (ax >= 0 && ay >= 0 && ax <= fr.width && ay <= fr.height && fr.width > 0) {
      anchor = { x: ax, y: ay, left: fr.left, top: fr.top, width: fr.width };
    }
  }
  const prevLeft = scroll ? scroll.scrollLeft : 0;
  const prevTop = scroll ? scroll.scrollTop : 0;
  state.zoom = next;
  updateZoomLabel();
  layoutPage();
  if (anchor && scroll) {
    const nr = frame.getBoundingClientRect();
    const ratio = anchor.width > 0 ? nr.width / anchor.width : 1;
    scroll.scrollLeft = prevLeft + (nr.left - anchor.left) + anchor.x * (ratio - 1);
    scroll.scrollTop = prevTop + (nr.top - anchor.top) + anchor.y * (ratio - 1);
  }
  debounceRender();
}
function updateZoomLabel() { $('#zoomLabel').textContent = Math.round(state.zoom * 100) + '%'; }
function gotoPage(p) {
  if (!state.doc) return;
  const next = clamp(p, 0, state.doc.pageCount - 1);
  if (next === state.page) { $('#pageInput').value = next + 1; return; }
  if (state.speaking) stopVoice();
  state.page = next;
  state.selected = null;
  $('#pageInput').value = next + 1;
  updateThumbActive();
  refreshAnnots();
  layoutPage();
  renderPage();
  $('#viewerScroll').scrollTo({ top: 0, behavior: 'smooth' });
  drawOverlay();
  if (typeof updateToolAction === 'function') updateToolAction();
  if (state.tool === 'edittext' && typeof loadTextLines === 'function') loadTextLines();
}

/* ---------------- thumbnails ---------------- */
async function loadThumbnails() {
  if (!state.doc) return;
  const list = $('#thumbList');
  list.innerHTML = '';
  for (let i = 0; i < Math.min(8, state.doc.pageCount); i++) list.insertAdjacentHTML('beforeend', '<div class="thumb-skel"></div>');
  if (state.demo) {
    state.thumbs = Array.from({ length: state.doc.pageCount }, (_, i) => ({ svg: demoPage(i) }));
    renderThumbList();
    return;
  }
  try {
    state.thumbs = await invoke('render_thumbnails', { width: 120 });
    renderThumbList();
  } catch {
    list.innerHTML = '<div class="empty-list">Anteprime non disponibili</div>';
  }
}
function renderThumbList() {
  if (!state.doc) return;
  $('#thumbList').innerHTML = Array.from({ length: state.doc.pageCount }).map((_, i) => {
    const t = state.thumbs[i];
    const src = t ? (t.png ? 'data:image/png;base64,' + t.png : t.svg) : null;
    return `<button class="thumb-card${i === state.page ? ' active' : ''}" data-page="${i}">
      ${src ? `<img src="${src}" alt="Pagina ${i + 1}">` : '<div class="thumb-skel" style="width:100%;height:auto;aspect-ratio:.707"></div>'}
      <span class="num">${i + 1}</span></button>`;
  }).join('');
  $$('#thumbList .thumb-card').forEach(btn => btn.addEventListener('click', () => gotoPage(parseInt(btn.dataset.page, 10))));
}
function updateThumbActive() {
  $$('#thumbList .thumb-card').forEach(c => c.classList.toggle('active', parseInt(c.dataset.page, 10) === state.page));
  const active = $('#thumbList .thumb-card.active');
  if (active) active.scrollIntoView({ block: 'nearest' });
}
async function ensureThumbs() {
  if (!state.thumbs.length && state.doc?.pageCount) await loadThumbnails();
}

/* ---------------- overlay ---------------- */
const SVGNS = 'http://www.w3.org/2000/svg';
function svgEl(tag, attrs = {}, parent) {
  const el = document.createElementNS(SVGNS, tag);
  for (const [k, v] of Object.entries(attrs)) if (v !== undefined && v !== null) el.setAttribute(k, v);
  if (parent) parent.appendChild(el);
  return el;
}
function pageAspect() {
  const p = state.doc?.pages?.[state.page];
  return p ? (p.displayHeight / p.displayWidth) : 1.4142;
}
function drawOverlay() {
  const overlay = $('#overlay');
  if (!overlay) return;
  overlay.innerHTML = '';
  if (!state.doc) return;
  const W = 1000;
  const H = W * pageAspect();
  overlay.setAttribute('viewBox', `0 0 ${W} ${H}`);
  const defs = svgEl('defs', {}, overlay);
  const filter = svgEl('filter', { id: 'softShadow', x: '-30%', y: '-30%', width: '160%', height: '160%' }, defs);
  svgEl('feDropShadow', { dx: 0, dy: 2, stdDeviation: 3, 'flood-color': '#0f172a', 'flood-opacity': .35 }, filter);

  const searchLayer = svgEl('g', { class: 'search-layer' }, overlay);
  if (state.search.active) {
    state.search.hits.filter(h => h.page === state.page).forEach(hit => {
      (hit.rects || []).forEach(r => {
        svgEl('rect', {
          x: r[0] * W, y: r[1] * H, width: Math.max(2, r[2] * W), height: Math.max(2, r[3] * H),
          rx: 2, fill: '#d6c58d', 'fill-opacity': .24, stroke: '#b09a6a', 'stroke-width': 1.1,
          class: 'search-hit-rect'
        }, searchLayer);
      });
    });
  }
  const layer = svgEl('g', { class: 'annot-layer' }, overlay);
  state.annotations.filter(a => a.page === state.page).forEach(ann => renderAnnotation(ann, layer, W, H));
  if (state.temp) renderAnnotation(state.temp, layer, W, H, true);
  if (state.tool === 'edittext' && state.textLines.length) {
    const textLayer = svgEl('g', { class: 'text-layer' }, overlay);
    state.textLines.forEach((line, index) => {
      const selected = state.textSelection.includes(index);
      const rect = svgEl('rect', {
        x: Math.max(0, line.x * W - 1.5),
        y: Math.max(0, line.y * H - 1.5),
        width: Math.max(3, line.w * W + 3),
        height: Math.max(3, line.h * H + 3),
        rx: 2,
        class: 'text-line' + (selected ? ' selected' : '')
      }, textLayer);
      const title = svgEl('title', {}, rect);
      title.textContent = line.text;
      rect.addEventListener('pointerdown', (ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        if (ev.shiftKey) {
          state.textSelection = selected
            ? state.textSelection.filter(i => i !== index)
            : [...state.textSelection, index];
        } else {
          state.textSelection = [index];
        }
        drawOverlay();
        if (typeof updateToolAction === 'function') updateToolAction();
      });
      rect.addEventListener('dblclick', (ev) => {
        ev.preventDefault();
        ev.stopPropagation();
        state.textSelection = [index];
        drawOverlay();
        if (typeof updateToolAction === 'function') updateToolAction();
        if (typeof replaceSelectedText === 'function') replaceSelectedText();
      });
    });
  }
  if (state.selected) {
    const ann = state.annotations.find(a => a.id === state.selected);
    if (ann && ann.page === state.page) {
      const b = annotationBBox(ann);
      const sel = svgEl('g', { class: 'selection' }, overlay);
      svgEl('rect', {
        x: b.x * W - 5, y: b.y * H - 5, width: Math.max(6, b.w * W + 10), height: Math.max(6, b.h * H + 10),
        rx: 3, fill: 'none', stroke: '#cdbf9b', 'stroke-width': 1.4, 'stroke-dasharray': '6 4', opacity: .9
      }, sel);
      [[b.x * W - 5, b.y * H - 5], [b.x * W + b.w * W + 5, b.y * H - 5], [b.x * W - 5, b.y * H + b.h * H + 5], [b.x * W + b.w * W + 5, b.y * H + b.h * H + 5]].forEach(([x, y]) => {
        svgEl('rect', { x: x - 2.5, y: y - 2.5, width: 5, height: 5, rx: 1, fill: '#cdbf9b' }, sel);
      });
    }
  }
}
function annotationBBox(ann) {
  if (ann.type === 'ink' && ann.points?.length) {
    const xs = ann.points.map(p => p[0]);
    const ys = ann.points.map(p => p[1]);
    const minx = Math.min(...xs), maxx = Math.max(...xs), miny = Math.min(...ys), maxy = Math.max(...ys);
    return { x: minx, y: miny, w: maxx - minx, h: maxy - miny };
  }
  return { x: ann.x, y: ann.y, w: ann.w, h: ann.h };
}
function renderAnnotation(ann, parent, W, H, isTemp = false) {
  const g = svgEl('g', { class: 'annot' + (isTemp ? ' temp-annot' : ''), 'data-id': ann.id }, parent);
  g.style.opacity = ann.opacity ?? 1;
  if (state.tool === 'browse' && !isTemp) {
    g.style.pointerEvents = 'all';
    g.style.cursor = 'pointer';
    g.addEventListener('click', (ev) => {
      ev.stopPropagation();
      selectAnnotation(ann.id);
      if (ann.type === 'note') editNote(ann);
    });
  } else g.style.pointerEvents = 'none';
  const x = ann.x * W, y = ann.y * H, w = Math.max(1, ann.w * W), h = Math.max(1, ann.h * H);
  const color = ann.color || '#d6c58d';
  if (ann.type === 'highlight') {
    const r = svgEl('rect', { x, y, width: w, height: h, rx: 2, fill: color, opacity: 1 }, g);
    r.style.mixBlendMode = 'multiply';
  } else if (ann.type === 'redact') {
    svgEl('rect', { x, y, width: w, height: h, fill: '#05070d' }, g);
    if (state.tool === 'redact') svgEl('rect', { x, y, width: w, height: h, fill: 'none', stroke: '#c8b98f', 'stroke-width': 1.2, 'stroke-dasharray': '5 4' }, g);
  } else if (ann.type === 'underline') {
    const th = Math.max(2, (ann.stroke || .003) * W * 1.2);
    svgEl('rect', { x, y: y + h - th, width: w, height: th, rx: 1, fill: color }, g);
  } else if (ann.type === 'ink') {
    if (ann.points?.length > 1) {
      const d = ann.points.map((p, i) => `${i ? 'L' : 'M'}${(p[0] * W).toFixed(2)} ${(p[1] * H).toFixed(2)}`).join(' ');
      svgEl('path', { d, fill: 'none', stroke: color, 'stroke-width': Math.max(1.2, (ann.stroke || .004) * W), 'stroke-linecap': 'round', 'stroke-linejoin': 'round' }, g);
    }
  } else if (ann.type === 'note') {
    svgEl('rect', { x, y, width: w, height: h, rx: 4, fill: '#ded3b4', stroke: '#8c7a50', 'stroke-width': .9, filter: 'url(#softShadow)' }, g);
    svgEl('path', { d: `M${x + w - 14} ${y} L${x + w} ${y + 14} L${x + w - 14} ${y + 14} Z`, fill: '#c8b98f' }, g);
    const label = svgEl('text', { x: x + 8, y: y + 13, fill: '#5c4f35', 'font-size': 8, 'font-family': 'Segoe UI, sans-serif', 'font-weight': 700, 'letter-spacing': 1.2 }, g);
    label.textContent = 'NOTA';
    const fs = Math.max(8, Math.min(13, h * .17));
    const maxChars = Math.max(8, Math.floor((w - 18) / (fs * .52)));
    wrapText(ann.text || '', maxChars).slice(0, 5).forEach((line, i) => {
      const t = svgEl('text', { x: x + 8, y: y + 28 + i * fs * 1.25, fill: '#322b21', 'font-size': fs, 'font-family': 'Segoe UI, sans-serif' }, g);
      t.textContent = line;
    });
  } else if (ann.type === 'stamp') {
    const label = (ann.text || ann.label || 'APPROVATO').toUpperCase();
    const cx = x + w / 2, cy = y + h / 2;
    const sg = svgEl('g', { transform: `rotate(-4 ${cx} ${cy})` }, g);
    svgEl('rect', { x, y, width: w, height: h, rx: 4, fill: color, 'fill-opacity': .07, stroke: color, 'stroke-width': Math.max(1.4, (ann.stroke || .003) * W * 1.3) }, sg);
    svgEl('rect', { x: x + 4, y: y + 4, width: Math.max(1, w - 8), height: Math.max(1, h - 8), rx: 2, fill: 'none', stroke: color, 'stroke-width': .8, opacity: .55 }, sg);
    const t = svgEl('text', {
      x: cx, y: cy + Math.min(h * .11, 8), fill: color, 'font-size': Math.max(8, h * .34),
      'font-family': 'Segoe UI, sans-serif', 'font-weight': 800, 'text-anchor': 'middle', 'letter-spacing': Math.max(1, h * .04)
    }, sg);
    t.textContent = label;
  } else if (ann.type === 'text') {
    const fs = Math.max(6, (ann.fontSize || .018) * H);
    const maxChars = Math.max(6, Math.floor((ann.w * W) / (fs * .52)));
    wrapText(ann.text || '', maxChars).forEach((line, i) => {
      const t = svgEl('text', { x, y: y + fs * (0.95 + i * 1.3), fill: color, 'font-size': fs, 'font-family': 'Segoe UI, sans-serif' }, g);
      t.textContent = line;
    });
  } else if (ann.type === 'link') {
    const fs = Math.max(6, (ann.fontSize || .018) * H);
    const t = svgEl('text', { x, y: y + fs, fill: '#60a5fa', 'font-size': fs, 'font-family': 'Segoe UI, sans-serif', 'text-decoration': 'underline' }, g);
    t.textContent = ann.text;
  } else if (ann.type === 'signature' && ann.dataUrl) {
    svgEl('image', { x, y, width: w, height: h, href: ann.dataUrl, preserveAspectRatio: 'xMidYMid meet' }, g);
  }
  return g;
}
function wrapText(text, maxChars) {
  const out = [];
  String(text || '').split('\n').forEach(raw => {
    if (!raw) { out.push(''); return; }
    let line = '';
    raw.split(/\s+/).forEach(word => {
      if (!line) line = word;
      else if (line.length + 1 + word.length <= maxChars) line += ' ' + word;
      else { out.push(line); line = word; }
    });
    if (line) out.push(line);
  });
  return out.length ? out : [''];
}

/* ---------------- pointer ---------------- */
function normPoint(ev) {
  const rect = $('#overlay').getBoundingClientRect();
  return [clamp((ev.clientX - rect.left) / rect.width, 0, 1), clamp((ev.clientY - rect.top) / rect.height, 0, 1)];
}
function bindOverlay() {
  const overlay = $('#overlay');
  overlay.addEventListener('pointerdown', (ev) => {
    if (!state.doc) return;
    if (state.creatingForm) { if (typeof addFormField === 'function') addFormField(ev); ev.preventDefault(); return; }
    const tool = state.tool;
    if (tool === 'edittext') {
      if (!ev.target.closest('.text-line')) {
        state.textSelection = [];
        drawOverlay();
        if (typeof updateToolAction === 'function') updateToolAction();
      }
      return;
    }
    if (tool === 'browse') {
      if (!ev.target.closest('.annot')) selectAnnotation(null);
      return;
    }
    const p = normPoint(ev);
    if (tool === 'note') { askText('Nuova nota', 'Nota', '', 'Scrivi il contenuto della nota…').then(t => { if (t) addNote(p, t); }); return; }
    if (tool === 'text') { askText('Inserisci testo', 'Testo', '', 'Scrivi il testo da inserire nel PDF…').then(t => { if (t) addText(p, t); }); return; }
    if (tool === 'stamp') { addStamp(p); return; }
    if (tool === 'signature') {
      if (!state.signatures) { state.pendingSignaturePoint = p; openSignatureModal(null); }
      else addSignature(p);
      return;
    }
    if (['highlight', 'underline', 'redact', 'ink'].includes(tool)) {
      state.drag = { tool, start: p, cur: p, points: [p] };
      state.temp = tempFromDrag();
      overlay.setPointerCapture(ev.pointerId);
      drawOverlay();
      ev.preventDefault();
    }
  });
  overlay.addEventListener('pointermove', (ev) => {
    if (!state.drag) return;
    const p = normPoint(ev);
    state.drag.cur = p;
    if (state.drag.tool === 'ink') state.drag.points.push(p);
    state.temp = tempFromDrag();
    drawOverlay();
  });
  const finish = () => {
    if (!state.drag) return;
    const drag = state.drag;
    const [x0, y0] = drag.start;
    const [x1, y1] = drag.cur;
    state.drag = null;
    state.temp = null;
    if (drag.tool === 'ink') {
      if (drag.points.length > 1) {
        const xs = drag.points.map(p => p[0]), ys = drag.points.map(p => p[1]);
        const x = Math.min(...xs), y = Math.min(...ys);
        addAnnotation('ink', { x, y, w: Math.max(...xs) - x, h: Math.max(...ys) - y, points: drag.points, color: state.color, opacity: 1, stroke: state.stroke });
      }
    } else {
      const x = Math.min(x0, x1), y = Math.min(y0, y1);
      const w = Math.abs(x1 - x0), h = Math.abs(y1 - y0);
      if (w > .003 && h > .002) {
        if (drag.tool === 'highlight') addAnnotation('highlight', { x, y, w, h, color: state.color, opacity: state.opacity });
        if (drag.tool === 'underline') addAnnotation('underline', { x, y, w, h: Math.max(h, .003), color: state.color, opacity: 1, stroke: state.stroke });
        if (drag.tool === 'redact') addAnnotation('redact', { x, y, w, h, color: '#05070d', opacity: 1 });
      }
    }
    drawOverlay();
  };
  overlay.addEventListener('pointerup', finish);
  overlay.addEventListener('pointercancel', finish);
}
function tempFromDrag() {
  if (!state.drag) return null;
  const [x0, y0] = state.drag.start;
  const [x1, y1] = state.drag.cur;
  if (state.drag.tool === 'ink') {
    const pts = state.drag.points;
    const xs = pts.map(p => p[0]), ys = pts.map(p => p[1]);
    const x = Math.min(...xs), y = Math.min(...ys);
    return { id: 'temp', page: state.page, type: 'ink', x, y, w: Math.max(...xs) - x, h: Math.max(...ys) - y, points: pts, color: state.color, opacity: .9, stroke: state.stroke };
  }
  return {
    id: 'temp', page: state.page, type: state.drag.tool,
    x: Math.min(x0, x1), y: Math.min(y0, y1), w: Math.abs(x1 - x0), h: Math.abs(y1 - y0),
    color: state.drag.tool === 'redact' ? '#05070d' : state.color,
    opacity: state.drag.tool === 'redact' ? 1 : state.opacity,
    stroke: state.stroke
  };
}

/* ---------------- annotations ---------------- */
function addNote(p, text) {
  const w = .22, h = .1;
  addAnnotation('note', { x: clamp(p[0], .01, .97 - w), y: clamp(p[1], .01, .97 - h), w, h, text, color: '#ded3b4', opacity: 1, fontSize: .011 });
}
function addText(p, text) {
  const fontSize = Math.max(.009, Math.min(.04, state.fontSize));
  const w = .42;
  const lines = wrapText(text, Math.max(8, Math.floor(w * 1000 / (fontSize * 1000 * .52)))).length;
  const h = clamp(fontSize * (0.5 + lines * 1.3), .03, .5);
  addAnnotation('text', { x: clamp(p[0], .01, .97 - w), y: clamp(p[1], .01, .98 - h), w, h, text, color: state.color, opacity: 1, fontSize });
}
function addStamp(p) {
  const w = .27, h = .085;
  const label = state.stampLabel;
  addAnnotation('stamp', {
    x: clamp(p[0] - w / 2, .01, .98 - w), y: clamp(p[1] - h / 2, .01, .98 - h), w, h,
    label, text: label, color: STAMP_COLORS[label] || '#34d399', opacity: .95, stroke: .003
  });
}
function addSignature(p) {
  if (!state.signatures) return openSignatureModal(p);
  const w = .24, h = .085;
  addAnnotation('signature', {
    x: clamp(p[0] - w / 2, .01, .98 - w), y: clamp(p[1] - h / 2, .01, .98 - h), w, h,
    dataUrl: state.signatures, author: 'Firma', opacity: 1
  });
  toast('Firma apposta. Verrà inclusa nel PDF esportato.');
}
function addAnnotation(type, geom, extra = {}) {
  pushHistory();
  const ann = {
    id: uid(), page: state.page, type,
    x: geom.x, y: geom.y, w: geom.w, h: geom.h,
    points: geom.points || [],
    color: geom.color || state.color,
    opacity: geom.opacity ?? state.opacity,
    stroke: geom.stroke ?? state.stroke,
    text: geom.text || '',
    fontSize: geom.fontSize ?? state.fontSize,
    label: geom.label || '',
    dataUrl: geom.dataUrl || '',
    author: geom.author || 'Tu',
    createdAt: new Date().toLocaleString('it-IT'),
    ...extra
  };
  state.annotations.push(ann);
  state.selected = ann.id;
  scheduleSave();
  refreshAnnots();
  drawOverlay();
  if (typeof updateToolAction === 'function') updateToolAction();
}
function pushHistory() {
  state.undo.push(JSON.stringify(state.annotations));
  if (state.undo.length > 60) state.undo.shift();
  state.redo = [];
}
function undo() {
  if (!state.undo.length) return;
  state.redo.push(JSON.stringify(state.annotations));
  state.annotations = JSON.parse(state.undo.pop());
  state.selected = null;
  scheduleSave();
  refreshAnnots();
  drawOverlay();
}
function redo() {
  if (!state.redo.length) return;
  state.undo.push(JSON.stringify(state.annotations));
  state.annotations = JSON.parse(state.redo.pop());
  state.selected = null;
  scheduleSave();
  refreshAnnots();
  drawOverlay();
}
function scheduleSave() {
  clearTimeout(state.saveTimer);
  state.saveTimer = setTimeout(saveAnnotations, 450);
}
async function saveAnnotations() {
  if (!state.doc || state.demo || !state.doc.sessionId) return;
  try { await invoke('save_annotations_cmd', { sessionId: state.doc.sessionId, annotations: state.annotations }); } catch { }
}
function selectAnnotation(id) {
  state.selected = id;
  drawOverlay();
  refreshAnnots();
  if (typeof updateToolAction === 'function') updateToolAction();
}
function deleteSelected() {
  if (!state.selected) return toast("Seleziona prima un'annotazione.", 'warn');
  removeAnnotation(state.selected);
}
function removeAnnotation(id) {
  pushHistory();
  state.annotations = state.annotations.filter(a => a.id !== id);
  if (state.selected === id) state.selected = null;
  scheduleSave();
  refreshAnnots();
  drawOverlay();
  if (typeof updateToolAction === 'function') updateToolAction();
}
function editNote(ann) {
  askText('Modifica nota', 'Nota', ann.text || '', 'Contenuto della nota…').then(t => {
    if (t === null) return;
    pushHistory();
    ann.text = t;
    scheduleSave();
    refreshAnnots();
    drawOverlay();
  });
}
function duplicateSelected() {
  const ann = state.annotations.find(a => a.id === state.selected);
  if (!ann) return;
  pushHistory();
  const copy = JSON.parse(JSON.stringify(ann));
  copy.id = uid();
  copy.x = clamp(copy.x + .02, 0, .96);
  copy.y = clamp(copy.y + .02, 0, .96);
  state.annotations.push(copy);
  state.selected = copy.id;
  scheduleSave();
  refreshAnnots();
  drawOverlay();
}
function refreshAnnots() {
  const total = state.annotations.length;
  const chip = $('#chipAnnots');
  if (chip) chip.textContent = `${total} annotazion${total === 1 ? 'e' : 'i'}`;
  if (!state.doc) {
    $('#annotsList').innerHTML = '<div class="empty-list">Apri un documento per annotarlo.</div>';
    $('#annotsAll').innerHTML = '';
    return;
  }
  const pageAnns = state.annotations.filter(a => a.page === state.page);
  $('#annotsPageLabel').textContent = `Pagina ${state.page + 1} · ${pageAnns.length}`;
  $('#annotsList').innerHTML = pageAnns.length ? pageAnns.map(a => annotItemHtml(a)).join('') : '<div class="empty-list">Nessuna annotazione su questa pagina.<br>Usa gli strumenti a sinistra.</div>';
  const all = [...state.annotations].sort((a, b) => a.page - b.page || String(a.createdAt).localeCompare(String(b.createdAt)));
  $('#annotsAll').innerHTML = all.length ? all.map(a => annotItemHtml(a, true)).join('') : '<div class="empty-list">Nessuna annotazione nel documento.</div>';
  $$('#annotsList .annot-item, #annotsAll .annot-item').forEach(el => {
    el.addEventListener('click', (ev) => {
      if (ev.target.closest('.a-del')) return;
      const ann = state.annotations.find(a => a.id === el.dataset.id);
      if (!ann) return;
      if (ann.page !== state.page) gotoPage(ann.page);
      selectAnnotation(ann.id);
      tab('annots');
    });
    const del = el.querySelector('.a-del');
    if (del) del.addEventListener('click', (ev) => { ev.stopPropagation(); removeAnnotation(el.dataset.id); });
  });
}
function annotItemHtml(ann, showPage = false) {
  const meta = annotationMeta(ann);
  return `<div class="annot-item${state.selected === ann.id ? ' active' : ''}" data-id="${ann.id}">
    <div class="a-ico" style="color:${ann.color || '#c2bfb7'}">${icon(meta.icon)}</div>
    <div class="a-text"><b>${esc(meta.title)}${showPage ? ` <span style="color:var(--muted-2);font-weight:400">· p. ${ann.page + 1}</span>` : ''}</b><span>${esc(meta.detail)}</span></div>
    <button class="a-del" title="Elimina">${icon('trash')}</button>
  </div>`;
}
function annotationMeta(ann) {
  const map = {
    highlight: ['Evidenziazione', 'highlighter'], underline: ['Sottolineatura', 'underline'], note: ['Nota adesiva', 'sticky'],
    stamp: ['Timbro', 'stamp'], ink: ['Disegno a mano', 'pen'], text: ['Testo', 'text'],
    signature: ['Firma', 'signature'], redact: ['Redazione', 'redact'], link: ['Collegamento', 'globe']
  };
  const [title, ic] = map[ann.type] || ['Annotazione', 'highlighter'];
  let detail = '';
  if (['note', 'text', 'link'].includes(ann.type)) detail = ann.text || '';
  else if (ann.type === 'stamp') detail = ann.label || ann.text || 'Timbro';
  else if (ann.type === 'signature') detail = ann.author || 'Firma';
  else if (ann.type === 'redact') detail = 'Area oscurata';
  else detail = ann.color || '';
  return { title, icon: ic, detail: detail.length > 70 ? detail.slice(0, 70) + '…' : detail };
}

/* ---------------- modals / toast ---------------- */
let modalOnClose = null;
function openModal(opts) {
  closeModal();
  modalOnClose = opts.onClose || null;
  const root = $('#modalRoot');
  root.innerHTML = `<div class="modal-backdrop">
    <div class="modal-card${opts.wide ? ' wide' : ''}">
      <div class="modal-head">
        <div class="m-ico">${icon(opts.icon || 'info')}</div>
        <div><div class="modal-title">${esc(opts.title)}</div>${opts.desc ? `<div class="modal-desc">${opts.desc}</div>` : ''}</div>
        <button class="modal-close" title="Chiudi">${icon('close')}</button>
      </div>
      <div class="modal-body">${opts.body || ''}</div>
      ${opts.footer === false ? '' : `<div class="modal-foot">${opts.footer || '<button class="btn ghost" data-close>Chiudi</button>'}</div>`}
    </div>
  </div>`;
  const backdrop = $('.modal-backdrop', root);
  backdrop.addEventListener('mousedown', (e) => { if (e.target === backdrop) closeModal(); });
  $('.modal-close', root).addEventListener('click', closeModal);
  $$('[data-close]', root).forEach(b => b.addEventListener('click', closeModal));
  if (opts.onMount) opts.onMount(backdrop);
  document.addEventListener('keydown', modalEsc);
  return backdrop;
}
function closeModal() {
  const root = $('#modalRoot');
  if (!root) return;
  root.innerHTML = '';
  const cb = modalOnClose;
  modalOnClose = null;
  if (cb) { try { cb(); } catch { } }
  document.removeEventListener('keydown', modalEsc);
}
function modalEsc(e) { if (e.key === 'Escape') closeModal(); }
function askText(title, label, value = '', placeholder = '') {
  return new Promise((resolve) => {
    openModal({
      title, icon: 'edit', desc: 'Il contenuto viene aggiunto sopra la pagina del PDF.',
      body: `<div class="field"><label>${esc(label)}</label><textarea id="askTextArea" placeholder="${esc(placeholder)}">${esc(value)}</textarea></div>`,
      footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="askTextOk">${icon('check')}Aggiungi</button>`,
      onMount: (root) => {
        const area = $('#askTextArea', root);
        setTimeout(() => area.focus(), 80);
        $('#askTextOk', root).addEventListener('click', () => {
          const v = area.value.trim();
          if (!v) { toast('Scrivi qualcosa prima di continuare.', 'warn'); return; }
          modalOnClose = null;
          closeModal();
          resolve(v);
        });
      },
      onClose: () => resolve(null)
    });
  });
}
function askPassword() {
  return new Promise((resolve) => {
    openModal({
      title: 'Documento protetto', icon: 'lock',
      desc: 'Inserisci la password per aprire il PDF. La password non lascia mai questo computer.',
      body: '<div class="field"><label>Password</label><input type="password" id="pwdInput"></div>',
      footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="pwdOk">${icon('lockOpen')}Apri</button>`,
      onMount: (root) => {
        const input = $('#pwdInput', root);
        setTimeout(() => input.focus(), 80);
        const submit = () => { const v = input.value; if (!v) return; modalOnClose = null; closeModal(); resolve(v); };
        $('#pwdOk', root).addEventListener('click', submit);
        input.addEventListener('keydown', e => { if (e.key === 'Enter') submit(); });
      },
      onClose: () => resolve(null)
    });
  });
}
function confirmDialog(title, desc, confirmLabel = 'Conferma', danger = false) {
  return new Promise((resolve) => {
    openModal({
      title, desc: '', icon: danger ? 'alert' : 'info',
      body: `<div class="note-box${danger ? ' danger' : ''}">${desc}</div>`,
      footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn ${danger ? 'ghost danger' : 'primary'}" id="confirmOk">${esc(confirmLabel)}</button>`,
      onMount: (root) => {
        $('#confirmOk', root).addEventListener('click', () => { modalOnClose = null; closeModal(); resolve(true); });
      },
      onClose: () => resolve(false)
    });
  });
}
function toast(message, kind = 'info', duration = 3600) {
  const root = $('#toastRoot');
  if (!root) return;
  const el = document.createElement('div');
  el.className = 'toast ' + kind;
  el.innerHTML = `${icon(kind === 'error' || kind === 'warn' ? 'alert' : 'check', 't-ico')}<span>${esc(message)}</span>`;
  root.appendChild(el);
  setTimeout(() => { el.classList.add('out'); setTimeout(() => el.remove(), 260); }, duration);
}
function setStatus(text, busy = false) {
  const st = $('#statusText');
  if (st) st.textContent = text;
  const dot = $('.status-dot');
  if (dot) dot.style.background = busy ? 'var(--warning)' : 'var(--success)';
}
function copyText(text) {
  if (navigator.clipboard?.writeText) {
    navigator.clipboard.writeText(text).then(() => toast('Copiato negli appunti.')).catch(() => fallbackCopy(text));
  } else fallbackCopy(text);
}
function fallbackCopy(text) {
  const ta = document.createElement('textarea');
  ta.value = text;
  ta.style.position = 'fixed';
  ta.style.opacity = '0';
  document.body.appendChild(ta);
  ta.select();
  try { document.execCommand('copy'); toast('Copiato negli appunti.'); } catch { toast('Impossibile copiare.', 'warn'); }
  ta.remove();
}



