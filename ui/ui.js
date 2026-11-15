/* Open PDF · Focus Dark — interfaccia */
const NAV = [
  { title: 'Lettura', items: [
    { id: 'browse', label: 'Documento', icon: 'book', kbd: 'V' },
    { id: 'edittext', label: 'Modifica testo', icon: 'edit', kbd: 'E' },
    { id: 'search', label: 'Cerca nel testo', icon: 'search', kbd: 'Ctrl F' },
    { id: 'voice', label: 'Leggi ad alta voce', icon: 'volume' }
  ]},
  { title: 'Annota', items: [
    { id: 'highlight', label: 'Evidenziatore', icon: 'highlighter', kbd: 'H' },
    { id: 'underline', label: 'Sottolineatura', icon: 'underline', kbd: 'U' },
    { id: 'note', label: 'Nota adesiva', icon: 'sticky', kbd: 'N' },
    { id: 'stamp', label: 'Timbri', icon: 'stamp', kbd: 'S' },
    { id: 'ink', label: 'Disegno a mano', icon: 'pen', kbd: 'P' },
    { id: 'text', label: 'Aggiungi testo', icon: 'text', kbd: 'T' },
    { id: 'signature', label: 'Firma', icon: 'signature', kbd: 'F' }
  ]},
  { title: 'Pagine', items: [
    { id: 'organize', label: 'Organizza pagine', icon: 'pages' },
    { id: 'merge', label: 'Unisci file', icon: 'merge' },
    { id: 'split', label: 'Dividi / estrai', icon: 'split' },
    { id: 'compress', label: 'Comprimi', icon: 'compress' },
    { id: 'redact', label: 'Redazione', icon: 'redact' }
  ]},
  { title: 'Crea e converti', items: [
    { id: 'images', label: 'Immagini → PDF', icon: 'image' },
    { id: 'web', label: 'Web → PDF', icon: 'globe' },
    { id: 'office', label: 'Office → PDF', icon: 'office' },
    { id: 'export', label: 'Esporta PDF', icon: 'export' },
    { id: 'ocr', label: 'Scansione & OCR', icon: 'scan' }
  ]},
  { title: 'Moduli e firme', items: [
    { id: 'fill', label: 'Compila modulo', icon: 'form' },
    { id: 'createform', label: 'Crea modulo', icon: 'formPlus' },
    { id: 'signrequest', label: 'Richiedi firme', icon: 'signRequest' }
  ]},
  { title: 'Sicurezza', items: [
    { id: 'password', label: 'Password & permessi', icon: 'lock' },
    { id: 'compare', label: 'Confronta file', icon: 'compare' },
    { id: 'metadata', label: 'Metadati', icon: 'metadata' }
  ]}
];

async function init() {
  paintStaticIcons();
  buildNav();
  buildSwatches();
  buildStamps();
  buildToolAction();
  bindTopbar();
  bindDock();
  bindTabs();
  bindOverlay();
  bindSearch();
  bindKeyboard();
  bindDragDrop();
  renderSignaturePreview();
  renderToolSettings();
  setTool('browse');
  updateVoiceButton();
  if (!HAS_TAURI) {
    if (!new URLSearchParams(location.search).has('empty')) await loadDemo();
  } else {
    try {
      applyEngineStatus(await invoke('engine_status'));
      const startup = await invoke('startup_file');
      if (startup) await openPath(startup);
    } catch (e) { setStatus('Motore PDF non disponibile', true); }
  }
  window.addEventListener('resize', () => {
    if (!state.doc) return;
    layoutPage();
    clearTimeout(state.renderTimer);
    state.renderTimer = setTimeout(renderPage, 180);
  });
}

function paintStaticIcons() {
  const map = {
    searchIcon: 'search', voiceBtn: 'volume', shareBtn: 'share', openIcon: 'folder',
    openDropIcon: 'folder', dropIcon: 'folder', thumbHeadIcon: 'pages',
    emptyOpenIcon: 'folder', emptyImagesIcon: 'image',
    prevPage: 'left', nextPage: 'right', zoomOut: 'zoomOut', zoomIn: 'zoomIn',
    rotateLeft: 'rotateL', rotateRight: 'rotateR', deleteAnno: 'trash',
    f1: 'volume', f2: 'highlighter', f3: 'scan', f4: 'lock', allToolsIcon: 'grid'
  };
  for (const [id, name] of Object.entries(map)) {
    const el = document.getElementById(id);
    if (el) el.innerHTML = icon(name);
  }
  const save = $('#saveBtn');
  if (save && !save.querySelector('svg')) save.insertAdjacentHTML('afterbegin', icon('save'));
  const ti = $('#toolIcon'); if (ti) ti.innerHTML = icon('book');
  const at = $('#annotsTitleIcon'); if (at) at.innerHTML = icon('highlighter');
  const ct = $('#commentsTitleIcon'); if (ct) ct.innerHTML = icon('share');
}

function buildNav() {
  $('#navRoot').innerHTML = NAV.map(group => `
    <div class="nav-group">
      <div class="nav-title">${group.title}</div>
      ${group.items.map(it => `<button class="nav-item" data-action="${it.id}" title="${esc(it.label)}">${icon(it.icon)}<span>${esc(it.label)}</span>${it.kbd ? `<span class="nav-kbd">${it.kbd}</span>` : ''}</button>`).join('')}
    </div>`).join('');
  $$('#navRoot .nav-item').forEach(btn => btn.addEventListener('click', () => handleNav(btn.dataset.action)));
}
function handleNav(action) {
  if (['browse', 'edittext', 'highlight', 'underline', 'note', 'stamp', 'ink', 'text', 'signature', 'redact'].includes(action)) {
    setTool(action);
    return;
  }
  if (action === 'search') return focusSearch();
  if (action === 'voice') return toggleVoice();
  if (action === 'createform') return startCreateForm();
  const actions = {
    organize: openOrganizer, merge: openMerge, split: openSplit, compress: openCompress,
    images: openImages, web: openWeb, office: openOffice, export: openExport, ocr: openOcr,
    fill: openFormFill, signrequest: openSignRequest, password: openPassword,
    compare: openCompare, metadata: openMetadata
  };
  if (actions[action]) actions[action]();
}
function setTool(tool) {
  if (!TOOL_META[tool]) return;
  state.tool = tool;
  $('#viewer').dataset.tool = tool;
  if (tool === 'text') {
    state.color = '#23262b';
    markSwatch();
  } else if (['highlight', 'underline', 'ink'].includes(tool) && state.color === '#23262b') {
    state.color = '#d6c58d';
    markSwatch();
  }
  if (tool !== 'edittext') state.textSelection = [];
  const meta = TOOL_META[tool];
  $$('#navRoot .nav-item').forEach(n => n.classList.toggle('active', n.dataset.action === tool));
  $('#chipTool').textContent = meta.name;
  $('#toolName').textContent = meta.name;
  $('#toolHint').textContent = meta.hint;
  const ti = $('#toolIcon'); if (ti) ti.innerHTML = icon(meta.icon);
  $('#statusTool').textContent = 'Strumento: ' + meta.name;
  if (tool === 'createform') state.creatingForm = true;
  updateToolAction();
  tab('tools');
  drawOverlay();
  if (tool === 'edittext' && state.doc) loadTextLines();
}
function buildToolAction() {
  const pane = $('.tab-pane[data-pane="tools"] .pane-section');
  if (!pane || $('#toolAction')) return;
  const div = document.createElement('div');
  div.id = 'toolAction';
  $('.tool-hint', pane).insertAdjacentElement('afterend', div);
}
function updateToolAction() {
  const box = $('#toolAction');
  if (!box) return;
  const t = state.tool;
  let html = '';
  if (t === 'redact') {
    const n = state.annotations.filter(a => a.type === 'redact').length;
    html = `<div class="note-box danger">Aree di redazione: <b>${n}</b>. Il testo, le immagini e la grafica dentro le aree vengono rimossi dal file, non solo coperti; annotazioni e metadati collegati vengono eliminati.</div>
      <button class="btn primary wide" id="applyRedact" style="margin-top:8px">${icon('redact')}Applica redazione</button>`;
  } else if (t === 'edittext') {
    const count = state.textSelection.length;
    const selected = state.textSelection.map(i => state.textLines[i]).filter(Boolean);
    const preview = selected.map(l => l.text).join(' ').trim();
    html = `<div class="note-box">Seleziona una riga di testo sul documento${count ? ` · <b>${count} selezionata${count === 1 ? '' : 'e'}</b>` : ''}.${preview ? `<br><span style="color:#d8d2c3">“${esc(preview.slice(0, 160))}${preview.length > 160 ? '…' : ''}”</span>` : ''}</div>
      ${state.textLines.length ? `<div class="mini-actions" style="margin-top:8px">
        <button class="btn primary small" id="replaceTextBtn" ${count === 1 ? '' : 'disabled'}>${icon('edit')}Sostituisci…</button>
        <button class="btn ghost small danger" id="deleteTextBtn" ${count ? '' : 'disabled'}>${icon('trash')}Elimina testo</button>
      </div>
      <div class="mini-actions" style="margin-top:6px">
        <button class="btn ghost small" id="selectAllTextBtn">Seleziona tutto</button>
        <button class="btn ghost small" id="clearTextBtn">Deseleziona</button>
        <button class="btn ghost small" id="writeTextBtn">${icon('text')}Scrivi testo</button>
        <button class="btn ghost small" id="textHelpBtn">${icon('info')}Istruzioni</button>
      </div>
      <div class="note-box warn" style="margin-top:8px">La modifica sostituisce davvero il testo nel content stream: stessa posizione, corpo e colore, con Helvetica se il font originale è incorporato o sottoinsiemato. L'eliminazione rimuove l'oggetto di testo che contiene la riga. L'originale resta intatto.</div>` : `<button class="btn ghost wide small" id="ocrForTextBtn">${icon('scan')}Nessun testo selezionabile · Apri OCR</button>`}`;
  } else if (t === 'signature') {
    html = `<button class="btn ghost wide small" id="sigCreateInline">${icon('signature')}Disegna una nuova firma</button>
      <button class="btn primary wide small" id="sigPades" style="margin-top:6px">${icon('check')}Firma digitale PAdES</button>
      <button class="btn ghost wide small" id="sigVerify" style="margin-top:6px">${icon('search')}Verifica firme</button>
      <div class="note-box">La firma PAdES è Baseline B-B: copre l'integrità del file, ma finché il backend non la supporta non include marca temporale né controllo di revoca online.</div>`;
  } else if (t === 'createform') {
    html = `<div class="form-create-bar">
      <button class="btn small ${state.newFieldKind === 'text' ? 'primary' : 'ghost'}" data-fieldkind="text">Campo testo</button>
      <button class="btn small ${state.newFieldKind === 'checkbox' ? 'primary' : 'ghost'}" data-fieldkind="checkbox">Casella</button>
    </div>
    <div class="note-box">Clicca sulla pagina per posizionare i campi. Campi creati: <b>${state.newFields.length}</b>.</div>
    <div class="mini-actions" style="margin-top:8px">
      <button class="btn primary small" id="saveFormFields">${icon('check')}Salva modulo</button>
      <button class="btn ghost small" id="cancelFormFields">Annulla</button>
    </div>`;
  } else if (t === 'browse' && state.selected) {
    html = `<button class="btn ghost wide small" id="duplicateAnno">${icon('copy')}Duplica</button>
      <button class="btn ghost wide small danger" id="removeAnno" style="margin-top:6px">${icon('trash')}Elimina annotazione</button>`;
  } else if (t === 'browse') {
    html = '<div class="note-box">Suggerimento: seleziona un\'annotazione per modificarla, oppure usa <b>Ctrl+Z</b> per annullare.</div>';
  } else {
    html = `<div class="note-box">Opacità <b>${Math.round((state.opacity || 0) * 100)}%</b> · tratto <b>${(state.stroke * 1000).toFixed(1)}</b> · colore <span style="display:inline-block;width:8px;height:8px;border-radius:2px;background:${state.color}"></span></div>`;
  }
  box.innerHTML = html;
  if (t === 'redact') $('#applyRedact')?.addEventListener('click', applyRedactions);
  if (t === 'edittext') {
    $('#replaceTextBtn')?.addEventListener('click', replaceSelectedText);
    $('#deleteTextBtn')?.addEventListener('click', deleteSelectedText);
    $('#selectAllTextBtn')?.addEventListener('click', selectAllText);
    $('#clearTextBtn')?.addEventListener('click', clearTextSelection);
    $('#writeTextBtn')?.addEventListener('click', () => { setTool('text'); toast('Clicca sulla pagina per scrivere il testo.'); });
    $('#textHelpBtn')?.addEventListener('click', showTextEditHelp);
    $('#ocrForTextBtn')?.addEventListener('click', openOcr);
  }
  if (t === 'signature') $('#sigCreateInline')?.addEventListener('click', () => openSignatureModal(null));
  if (t === 'signature') $('#sigPades')?.addEventListener('click', signPades);
  if (t === 'signature') $('#sigVerify')?.addEventListener('click', verifySignatures);
  if (t === 'createform') {
    $$('[data-fieldkind]', box).forEach(b => b.addEventListener('click', () => { state.newFieldKind = b.dataset.fieldkind; updateToolAction(); }));
    $('#saveFormFields')?.addEventListener('click', saveCreatedForm);
    $('#cancelFormFields')?.addEventListener('click', cancelCreateForm);
  }
  if (t === 'browse' && state.selected) {
    $('#duplicateAnno')?.addEventListener('click', duplicateSelected);
    $('#removeAnno')?.addEventListener('click', deleteSelected);
  }
}

function buildSwatches() {
  const box = $('#swatches');
  box.innerHTML = COLORS.map(c => `<button class="swatch" data-color="${c}" style="background:${c}" title="${c}"></button>`).join('');
  $$('.swatch', box).forEach(btn => btn.addEventListener('click', () => {
    state.color = btn.dataset.color;
    markSwatch();
    const selected = state.selected && state.annotations.find(a => a.id === state.selected);
    if (selected && !['note', 'signature', 'redact'].includes(selected.type)) {
      selected.color = state.color;
      scheduleSave(); refreshAnnots();
    }
    drawOverlay();
  }));
  markSwatch();
}
function markSwatch() {
  $$('#swatches .swatch').forEach(s => s.classList.toggle('active', s.dataset.color === state.color));
}
function buildStamps() {
  const box = $('#stampGrid');
  box.innerHTML = STAMPS.map(([label, color]) => `<button class="stamp-opt${state.stampLabel === label ? ' active' : ''}" data-stamp="${label}" style="${state.stampLabel === label ? `color:${color};border-color:${color}66;background:${color}18` : ''}">${label}</button>`).join('');
  $$('.stamp-opt', box).forEach(btn => btn.addEventListener('click', () => {
    state.stampLabel = btn.dataset.stamp;
    state.color = STAMP_COLORS[state.stampLabel] || '#34d399';
    buildStamps();
    setTool('stamp');
  }));
}

function bindTopbar() {
  $('#openBtn').addEventListener('click', openDialog);
  $('#openDrop').addEventListener('click', openDialog);
  $('#emptyOpen').addEventListener('click', openDialog);
  $('#emptyImages').addEventListener('click', openImages);
  $('#saveBtn').addEventListener('click', openSaveModal);
  $('#shareBtn').addEventListener('click', openSignRequest);
  $('#voiceBtn').addEventListener('click', toggleVoice);
  $('#metaEdit').addEventListener('click', openMetadata);
  $('#signCreate').addEventListener('click', () => openSignatureModal(null));
  $('#signPhoto')?.addEventListener('click', () => openSignatureModal(null, 'photo'));
  $('#signClear').addEventListener('click', () => {
    state.signatures = '';
    localStorage.removeItem('openpdf.signature');
    renderSignaturePreview();
  });
  $('#startShareBtn').addEventListener('click', startShare);
  $('#stopShare').addEventListener('click', stopShare);
  $('#copyShare').addEventListener('click', () => copyText($('#shareUrl').value));
  $('#allToolsBtn').addEventListener('click', openAllTools);
}
function bindDock() {
  $('#prevPage').addEventListener('click', () => gotoPage(state.page - 1));
  $('#nextPage').addEventListener('click', () => gotoPage(state.page + 1));
  $('#zoomIn').addEventListener('click', () => setZoom(state.zoom + 0.15));
  $('#zoomOut').addEventListener('click', () => setZoom(state.zoom - 0.15));
  const fitPage = () => {
    setZoom(1);
    $('#viewerScroll').scrollTo({ top: 0, left: 0 });
  };
  $('#zoomFit').addEventListener('click', fitPage);
  $('#zoomLabel').addEventListener('click', fitPage);
  $('#pageInput').addEventListener('change', () => {
    const p = parseInt($('#pageInput').value, 10);
    if (!Number.isNaN(p)) gotoPage(p - 1);
  });
  $('#rotateLeft').addEventListener('click', () => rotateCurrent(-90));
  $('#rotateRight').addEventListener('click', () => rotateCurrent(90));
  $('#deleteAnno').addEventListener('click', deleteSelected);
  const viewer = $('#viewer');
  viewer.addEventListener('wheel', (e) => {
    if (!e.ctrlKey) return;
    e.preventDefault();
    if (!state.doc) return;
    const scale = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? 400 : 1;
    const delta = e.deltaY * scale;
    if (!delta) return;
    const factor = clamp(Math.exp(-delta * 0.0012), 0.72, 1.38);
    setZoom(state.zoom * factor, e);
  }, { passive: false });
  window.addEventListener('wheel', (e) => {
    if (e.ctrlKey) e.preventDefault();
  }, { passive: false, capture: true });
}
function bindTabs() {
  $$('.inspector-tabs .tab').forEach(btn => btn.addEventListener('click', () => tab(btn.dataset.tab)));
}
function tab(name) {
  $$('.inspector-tabs .tab').forEach(t => t.classList.toggle('active', t.dataset.tab === name));
  $$('.tab-pane').forEach(p => p.classList.toggle('active', p.dataset.pane === name));
}
function bindDragDrop() {
  if (HAS_TAURI && window.__TAURI__.event?.listen) {
    window.__TAURI__.event.listen('tauri://drag-drop', (event) => {
      const paths = event?.payload?.paths || [];
      const pdf = paths.find(p => /\.pdf$/i.test(p));
      if (pdf) openPath(pdf);
      else if (paths.length) toast('Trascina un file PDF per aprirlo.', 'warn');
    }).catch(() => {});
  }
}
function renderToolSettings() {
  $('#opacityRange').addEventListener('input', (e) => {
    state.opacity = parseInt(e.target.value, 10) / 100;
    $('#opacityVal').textContent = e.target.value + '%';
    applySelectedProp('opacity', state.opacity);
    drawOverlay();
  });
  $('#strokeRange').addEventListener('input', (e) => {
    state.stroke = parseInt(e.target.value, 10) / 1000;
    $('#strokeVal').textContent = e.target.value;
    applySelectedProp('stroke', state.stroke);
    drawOverlay();
  });
  $('#fontRange').addEventListener('input', (e) => {
    state.fontSize = parseInt(e.target.value, 10) / 1000;
    $('#fontVal').textContent = e.target.value;
    applySelectedProp('fontSize', state.fontSize);
    drawOverlay();
  });
}
function applySelectedProp(key, value) {
  const ann = state.selected && state.annotations.find(a => a.id === state.selected);
  if (!ann) return;
  if (key === 'opacity' && ['note', 'signature', 'redact'].includes(ann.type)) return;
  ann[key] = value;
  scheduleSave();
  drawOverlay();
}

/* ---------------- signature ---------------- */
function renderSignaturePreview() {
  const box = $('#signPreview');
  if (!box) return;
  if (state.signatures) box.innerHTML = `<img src="${state.signatures}" alt="Firma">`;
  else box.innerHTML = '<span>Nessuna firma salvata</span>';
}
function openSignatureModal(pendingPoint, initialTab = 'draw') {
  if (pendingPoint) state.pendingSignaturePoint = pendingPoint;
  const startTab = ['draw', 'type', 'photo'].includes(initialTab) ? initialTab : 'draw';
  openModal({
    title: 'La tua firma',
    desc: 'Disegna con mouse, penna o dito, scrivi il tuo nome o importa una foto della firma. La firma resta salvata solo su questo computer.',
    icon: 'signature',
    body: `<div class="sig-tabs">
        <button class="sig-tab${startTab === 'draw' ? ' active' : ''}" data-sig="draw">Disegna</button>
        <button class="sig-tab${startTab === 'type' ? ' active' : ''}" data-sig="type">Scrivi</button>
        <button class="sig-tab${startTab === 'photo' ? ' active' : ''}" data-sig="photo">Foto</button>
      </div>
      <div id="sigDrawWrap"${startTab === 'draw' ? '' : ' hidden'}><canvas class="signature-pad" id="sigPad"></canvas>
        <div class="mini-actions" style="margin-top:8px"><button class="btn ghost small" id="sigClearPad">Pulisci</button></div>
      </div>
      <div id="sigTypeWrap"${startTab === 'type' ? '' : ' hidden'}>
        <div class="field"><label>Nome o firma</label><input type="text" id="sigName" placeholder="Mario Rossi" maxlength="40"></div>
        <div class="sig-typed" id="sigTypedPreview">Mario Rossi</div>
      </div>
      <div id="sigPhotoWrap"${startTab === 'photo' ? '' : ' hidden'}>
        <div class="file-drop" id="sigPhotoPick">${icon('plus')} Scegli foto della firma</div>
        <div class="sig-photo-status note-box" id="sigPhotoStatus" hidden></div>
        <div class="sig-photo-frame" id="sigPhotoFrame" hidden><img id="sigPhotoPreview" alt="Anteprima firma"></div>
        <div class="note-box">Meglio inchiostro scuro su carta chiara. La pulizia rimuove sfondo, ombre e piccoli segni; i segni forti possono restare. Sono accettati JPEG, PNG, WebP, HEIC, AVIF, TIFF, RAW e altri formati: la disponibilità dipende dai codec Windows e dai convertitori locali.</div>
      </div>
      <div class="note-box">La firma viene apposta come immagine sul PDF e accompagnata da autore e data. Per firme con valore legale certificato serve un provider di firma digitale.</div>`,
    footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="sigSave">${icon('check')}Salva firma</button>`,
    onMount: (root) => {
      const pad = $('#sigPad', root);
      const saveBtn = $('#sigSave', root);
      let mode = startTab;
      let drawing = false;
      let ctx = null;
      let padReady = false;
      let photoData = '';
      let cleaning = false;

      const setup = () => {
        if (!pad) return;
        const r = pad.getBoundingClientRect();
        pad.width = Math.max(600, Math.round(r.width * 2));
        pad.height = Math.round((r.height || 190) * 2);
        ctx = pad.getContext('2d');
        ctx.setTransform(2, 0, 0, 2, 0, 0);
        ctx.lineWidth = 2.4;
        ctx.lineCap = 'round';
        ctx.lineJoin = 'round';
        ctx.strokeStyle = '#111827';
        padReady = true;
      };
      const applyMode = () => {
        $$('.sig-tab', root).forEach(x => x.classList.toggle('active', x.dataset.sig === mode));
        $('#sigDrawWrap').hidden = mode !== 'draw';
        $('#sigTypeWrap').hidden = mode !== 'type';
        $('#sigPhotoWrap').hidden = mode !== 'photo';
        if (mode === 'draw' && !padReady) setup();
      };
      if (mode === 'draw') setup();
      applyMode();

      pad?.addEventListener('pointerdown', (e) => {
        if (!ctx) setup();
        drawing = true;
        const r = pad.getBoundingClientRect();
        ctx.beginPath();
        ctx.moveTo(e.clientX - r.left, e.clientY - r.top);
        pad.setPointerCapture(e.pointerId);
      });
      pad?.addEventListener('pointermove', (e) => {
        if (!drawing) return;
        const r = pad.getBoundingClientRect();
        ctx.lineTo(e.clientX - r.left, e.clientY - r.top);
        ctx.stroke();
      });
      pad?.addEventListener('pointerup', () => { drawing = false; });
      $('#sigClearPad', root)?.addEventListener('click', setup);
      const typed = $('#sigName', root);
      typed?.addEventListener('input', () => { $('#sigTypedPreview').textContent = typed.value || 'Mario Rossi'; });
      $$('.sig-tab', root).forEach(b => b.addEventListener('click', () => {
        mode = b.dataset.sig;
        applyMode();
      }));

      $('#sigPhotoPick', root)?.addEventListener('click', async () => {
        if (cleaning) return;
        try {
          const path = await invoke('pick_signature_photo');
          if (!path) return;
          const frame = $('#sigPhotoFrame', root);
          const status = $('#sigPhotoStatus', root);
          const preview = $('#sigPhotoPreview', root);
          photoData = '';
          frame.hidden = true;
          preview.removeAttribute('src');
          status.hidden = false;
          status.className = 'sig-photo-status note-box';
          status.textContent = 'Pulizia della firma in corso…';
          cleaning = true;
          saveBtn.disabled = true;
          try {
            photoData = await invoke('clean_signature_photo', { path });
            preview.src = photoData;
            frame.hidden = false;
            status.textContent = "Firma pulita. Controlla l'anteprima prima di salvare.";
          } catch (e) {
            status.className = 'sig-photo-status note-box warn';
            status.textContent = normalizeError(e);
            toast(normalizeError(e), 'error', 6000);
          } finally {
            cleaning = false;
            saveBtn.disabled = false;
          }
        } catch (e) { toast(normalizeError(e), 'error'); }
      });

      $('#sigSave', root)?.addEventListener('click', () => {
        if (cleaning) return;
        let data = '';
        if (mode === 'draw') {
          if (!hasInk(pad)) { toast('Disegna la firma prima di salvare.', 'warn'); return; }
          data = pad.toDataURL('image/png');
        } else if (mode === 'type') {
          const value = (typed?.value || '').trim();
          if (!value) { toast('Scrivi il tuo nome o la tua firma.', 'warn'); return; }
          const c = document.createElement('canvas');
          c.width = 760; c.height = 240;
          const g = c.getContext('2d');
          g.fillStyle = '#111827';
          g.font = '72px "Segoe Script", "Segoe UI", cursive';
          g.textAlign = 'center'; g.textBaseline = 'middle';
          g.fillText(value, c.width / 2, c.height / 2, c.width - 60);
          data = c.toDataURL('image/png');
        } else {
          if (!photoData) { toast('Scegli una foto della firma prima di salvare.', 'warn'); return; }
          data = photoData;
        }
        state.signatures = data;
        localStorage.setItem('openpdf.signature', data);
        renderSignaturePreview();
        const pending = state.pendingSignaturePoint;
        state.pendingSignaturePoint = null;
        modalOnClose = null;
        closeModal();
        if (pending) addSignature(pending);
        else toast('Firma salvata.');
      });
    },
    onClose: () => { state.pendingSignaturePoint = null; }
  });
}
function hasInk(canvas) {
  try {
    const ctx = canvas.getContext('2d');
    const data = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
    for (let i = 3; i < data.length; i += 4) if (data[i] > 0) return true;
    return false;
  } catch { return true; }
}

/* ---------------- search ---------------- */
function bindSearch() {
  const input = $('#searchInput');
  input.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') runSearch();
    if (e.key === 'Escape') { closeSearch(); input.blur(); }
  });
  input.addEventListener('input', () => {
    if (!input.value) { state.search = { query: '', hits: [], active: false }; drawOverlay(); closeSearch(); }
  });
  document.addEventListener('click', (e) => { if (!e.target.closest('#searchBox')) closeSearch(); });
}
function focusSearch() {
  const input = $('#searchInput');
  input.focus(); input.select();
}
function closeSearch() { $('#searchPopover').classList.remove('open'); }
async function runSearch() {
  const query = $('#searchInput').value.trim();
  if (!state.doc) return toast('Apri un documento per cercare.', 'warn');
  if (!query) { state.search = { query: '', hits: [], active: false }; drawOverlay(); closeSearch(); return; }
  try {
    setStatus('Ricerca in corso…', true);
    const hits = state.demo ? demoSearch(query) : await invoke('search_pdf', { query, matchCase: false });
    state.search = { query, hits, active: true };
    renderSearchPopover();
    if (hits.length) {
      gotoPage(hits[0].page);
      drawOverlay();
    }
    setStatus(hits.length ? `${hits.length} risultati · “${query}”` : 'Nessun risultato');
  } catch (e) { toast(normalizeError(e), 'error'); setStatus('Pronto'); }
}
function renderSearchPopover() {
  const pop = $('#searchPopover');
  const q = state.search.query;
  const hits = state.search.hits;
  pop.innerHTML = `<div class="search-head">${hits.length} risultat${hits.length === 1 ? 'o' : 'i'} · “${esc(q)}”</div>` +
    (hits.length ? hits.slice(0, 40).map((h, i) => {
      let marked = esc(h.text || '');
      try {
        const safe = esc(q).replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
        marked = marked.replace(new RegExp('(' + safe + ')', 'ig'), '<b>$1</b>');
      } catch { }
      return `<button class="search-hit" data-hit="${i}"><span class="pg">p. ${h.page + 1}</span><span class="snip">${marked}</span></button>`;
    }).join('') : '<div class="search-empty">Nessuna occorrenza nel documento.</div>');
  pop.classList.add('open');
  $$('.search-hit', pop).forEach(btn => btn.addEventListener('click', () => {
    const h = hits[parseInt(btn.dataset.hit, 10)];
    if (!h) return;
    gotoPage(h.page);
    drawOverlay();
    closeSearch();
  }));
}

/* ---------------- voice ---------------- */
async function toggleVoice() {
  if (!state.doc) return toast('Apri un documento per ascoltarlo.', 'warn');
  if (state.speaking) return stopVoice();
  try {
    setStatus('Preparazione della voce…', true);
    await invoke('speak_page', { page: state.page, rate: 0 });
    state.speaking = true;
    updateVoiceButton();
    setStatus(`Lettura vocale · pagina ${state.page + 1}`);
    clearInterval(state.speakTimer);
    state.speakTimer = setInterval(async () => {
      if (!HAS_TAURI) return;
      try {
        const active = await invoke('tts_status');
        if (!active) stopVoice(false);
      } catch { }
    }, 1400);
  } catch (e) {
    toast('Lettura vocale non disponibile: ' + normalizeError(e), 'error');
    setStatus('Pronto');
  }
}
async function stopVoice(withCommand = true) {
  state.speaking = false;
  clearInterval(state.speakTimer);
  updateVoiceButton();
  if (withCommand && HAS_TAURI) { try { await invoke('stop_speaking'); } catch { } }
  setStatus('Pronto');
}
function updateVoiceButton() {
  const btn = $('#voiceBtn');
  if (!btn) return;
  btn.innerHTML = state.speaking ? icon('stop') : icon('volume');
  btn.classList.toggle('active', state.speaking);
  btn.title = state.speaking ? 'Ferma la lettura' : 'Leggi ad alta voce';
}

/* ---------------- save / result ---------------- */
function baseName() {
  const n = state.doc?.originalName || state.doc?.name || 'documento.pdf';
  return n.replace(/\.pdf$/i, '');
}
function openSaveModal() {
  if (!state.doc) return toast('Apri prima un documento.', 'warn');
  openModal({
    title: 'Salva PDF', icon: 'save',
    desc: 'Il file originale non viene mai sovrascritto: scegli dove salvare la nuova copia.',
    body: `<button class="btn primary wide" id="saveAnnotated" style="justify-content:flex-start">${icon('save')}Salva con annotazioni e firme</button>
      <button class="btn ghost wide" id="savePlain" style="margin-top:8px;justify-content:flex-start">${icon('folder')}Salva copia del documento di lavoro</button>
      <div class="note-box">Le annotazioni vengono impresse nel PDF mantenendo il contenuto originale. La copia di lavoro include le modifiche a pagine e moduli.</div>`,
    footer: '<button class="btn ghost" data-close>Annulla</button>',
    onMount: (root) => {
      $('#saveAnnotated', root).addEventListener('click', () => { closeModal(); saveAnnotated(); });
      $('#savePlain', root).addEventListener('click', () => { closeModal(); savePlain(); });
    }
  });
}
async function saveAnnotated() {
  try {
    const dest = await invoke('pick_save', { defaultName: baseName() + '_annotato.pdf', filters: ['pdf'] });
    if (!dest) return;
    await saveAnnotations();
    setStatus('Esportazione annotazioni…', true);
    const res = await invoke('export_annotated', { dest, annotations: state.annotations });
    setStatus('Pronto');
    showResult('PDF annotato salvato', res.message, res.path, true);
  } catch (e) { setStatus('Pronto'); toast(normalizeError(e), 'error'); }
}
async function savePlain() {
  try {
    const dest = await invoke('pick_save', { defaultName: baseName() + '.pdf', filters: ['pdf'] });
    if (!dest) return;
    const res = await invoke('save_copy', { dest });
    showResult('Copia salvata', res.message, res.path, true);
  } catch (e) { toast(normalizeError(e), 'error'); }
}
function showResult(title, message, path, openable = false) {
  openModal({
    title, icon: 'check', desc: message,
    body: `<div class="field"><label>File</label><input type="text" readonly value="${esc(path || '')}"></div>
      <div class="note-box">Percorso locale. Il documento è disponibile anche dopo la chiusura di Open PDF.</div>`,
    footer: `${openable && path ? `<button class="btn primary" id="openResultBtn">${icon('folder')}Apri risultato</button>` : ''}<button class="btn ghost" data-close>Chiudi</button>`,
    onMount: (root) => {
      $('#openResultBtn', root)?.addEventListener('click', async () => {
        try {
          const info = await invoke('open_result', { path });
          applyOpenInfo(info);
          closeModal();
          toast('Documento aperto.');
        } catch (e) { toast(normalizeError(e), 'error'); }
      });
    }
  });
}
/* Alcune risposte del backend sono piatte (campi di OpenInfo in radice), altre
   annidano l'oggetto sotto `info`. Accetta entrambe e ignora risposte parziali. */
function docInfoFrom(res) {
  if (!res || typeof res !== 'object') return null;
  if (res.info && typeof res.info === 'object' && typeof res.info.pageCount === 'number') return res.info;
  if (typeof res.pageCount === 'number') return res;
  return null;
}

/* ---------------- pages ---------------- */
async function rotateCurrent(delta) {
  if (!state.doc) return;
  const old = JSON.parse(JSON.stringify(state.annotations));
  const page = state.page;
  try {
    setStatus('Rotazione pagina…', true);
    const info = await invoke('rotate_current_page', { page, delta });
    const remapped = old.map(a => a.page === page ? rotateAnnotation(a, delta) : a);
    applyOpenInfo(info, remapped);
    await saveAnnotations();
    toast('Pagina ruotata.');
  } catch (e) { toast(normalizeError(e), 'error'); }
  setStatus('Pronto');
}
function rotateAnnotation(ann, delta) {
  const d = ((delta % 360) + 360) % 360;
  if (!d) return ann;
  const rp = (p) => d === 90 ? [1 - p[1], p[0]] : d === 180 ? [1 - p[0], 1 - p[1]] : [p[1], 1 - p[0]];
  const copy = JSON.parse(JSON.stringify(ann));
  if (copy.type === 'ink' && copy.points?.length) {
    copy.points = copy.points.map(rp);
    const xs = copy.points.map(p => p[0]), ys = copy.points.map(p => p[1]);
    copy.x = Math.min(...xs); copy.y = Math.min(...ys);
    copy.w = Math.max(...xs) - copy.x; copy.h = Math.max(...ys) - copy.y;
  } else {
    const corners = [[copy.x, copy.y], [copy.x + copy.w, copy.y], [copy.x + copy.w, copy.y + copy.h], [copy.x, copy.y + copy.h]].map(rp);
    const xs = corners.map(p => p[0]), ys = corners.map(p => p[1]);
    copy.x = Math.min(...xs); copy.y = Math.min(...ys);
    copy.w = Math.max(...xs) - copy.x; copy.h = Math.max(...ys) - copy.y;
  }
  return copy;
}
async function openOrganizer() {
  if (!state.doc) return toast('Apri prima un documento.', 'warn');
  await ensureThumbs();
  state.orgOrder = Array.from({ length: state.doc.pageCount }, (_, i) => i);
  state.orgDeleted = new Set();
  openModal({
    title: 'Organizza pagine', wide: true, icon: 'pages',
    desc: 'Trascina le miniature per riordinare, elimina o ripristina le pagine. Le modifiche vengono applicate a una copia di lavoro.',
    body: `<div class="org-toolbar"><span class="note-box" style="margin:0">Pagine totali: <b>${state.doc.pageCount}</b></span></div>
      <div class="organizer-grid" id="orgGrid"></div>
      ${state.doc.pageCount > 120 ? '<div class="note-box warn">Anteprime limitate alle prime 120 pagine; tutte le pagine possono comunque essere riordinate.</div>' : ''}`,
    footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="applyOrder">${icon('check')}Applica</button>`,
    onMount: (root) => {
      renderOrgGrid(root);
      $('#applyOrder', root).addEventListener('click', applyOrganizer);
    }
  });
}
function renderOrgGrid(root = $('#modalRoot')) {
  const grid = $('#orgGrid', root);
  if (!grid) return;
  grid.innerHTML = state.orgOrder.map((pageIndex, pos) => {
    const t = state.thumbs[pageIndex];
    const src = t ? (t.png ? 'data:image/png;base64,' + t.png : t.svg) : null;
    const deleted = state.orgDeleted.has(pageIndex);
    return `<div class="org-card${deleted ? ' deleted' : ''}" draggable="true" data-pos="${pos}">
      ${src ? `<img src="${src}" alt="Pagina ${pageIndex + 1}">` : '<div class="thumb-skel" style="width:100%;height:auto;aspect-ratio:.707"></div>'}
      <div class="org-num">Pagina ${pageIndex + 1}</div>
      <div class="org-actions"><button data-act="del" title="${deleted ? 'Ripristina' : 'Elimina'}">${icon(deleted ? 'rotateL' : 'close')}</button></div>
    </div>`;
  }).join('');
  $$('.org-card', grid).forEach(card => {
    card.addEventListener('dragstart', () => card.classList.add('dragging'));
    card.addEventListener('dragend', () => { card.classList.remove('dragging'); $$('.org-card', grid).forEach(c => c.classList.remove('drag-over')); });
    card.addEventListener('dragover', (e) => { e.preventDefault(); card.classList.add('drag-over'); });
    card.addEventListener('dragleave', () => card.classList.remove('drag-over'));
    card.addEventListener('drop', (e) => {
      e.preventDefault();
      const dragging = $('.org-card.dragging', grid);
      if (!dragging || dragging === card) return;
      const from = parseInt(dragging.dataset.pos, 10);
      const to = parseInt(card.dataset.pos, 10);
      const item = state.orgOrder.splice(from, 1)[0];
      state.orgOrder.splice(to, 0, item);
      renderOrgGrid(root);
    });
    $('[data-act="del"]', card)?.addEventListener('click', (e) => {
      e.stopPropagation();
      const pageIndex = state.orgOrder[parseInt(card.dataset.pos, 10)];
      if (state.orgDeleted.has(pageIndex)) state.orgDeleted.delete(pageIndex);
      else state.orgDeleted.add(pageIndex);
      renderOrgGrid(root);
    });
  });
}
async function applyOrganizer() {
  const order = state.orgOrder.filter(i => !state.orgDeleted.has(i));
  if (!order.length) return toast('Devi mantenere almeno una pagina.', 'warn');
  const old = JSON.parse(JSON.stringify(state.annotations));
  try {
    setStatus('Riorganizzazione delle pagine…', true);
    const info = await invoke('apply_page_order', { order });
    const map = new Map();
    order.forEach((oldPage, newPage) => { if (!map.has(oldPage)) map.set(oldPage, newPage); });
    const remapped = old.filter(a => map.has(a.page)).map(a => ({ ...a, page: map.get(a.page) }));
    applyOpenInfo(info, remapped);
    await saveAnnotations();
    closeModal();
    toast('Pagine aggiornate.');
  } catch (e) { toast(normalizeError(e), 'error'); }
  setStatus('Pronto');
}
async function applyRedactions() {
  const rects = state.annotations.filter(a => a.type === 'redact');
  if (!rects.length) return toast("Disegna almeno un'area nera di redazione.", 'warn');
  const ok = await confirmDialog('Applicare la redazione?', `Verranno rimosse ${rects.length} aree di contenuto: il testo e gli elementi grafici sotto le aree vengono cancellati dal file (non solo coperti), insieme alle annotazioni e ai metadati collegati. L'operazione non è annullabile.`, 'Redigi definitivamente', true);
  if (!ok) return;
  try {
    setStatus('Applicazione della redazione…', true);
    const res = await invoke('apply_redactions', { annotations: state.annotations });
    const info = docInfoFrom(res);
    if (info) applyOpenInfo(info, []);
    const message = (res && res.message) || (info && info.message) || 'Redazione applicata: il contenuto sotto le aree è stato rimosso definitivamente.';
    showRedactionSummary(message, rects.length);
  } catch (e) { toast(normalizeError(e), 'error'); }
  setStatus('Pronto');
}

function showRedactionSummary(message, count) {
  openModal({
    title: 'Riepilogo redazione',
    icon: 'redact',
    desc: 'La redazione è stata applicata alla copia di lavoro.',
    body: `<div class="note-box">${esc(message || 'Redazione applicata.').replace(/\n/g, '<br>')}</div>
      <div class="result-line">${icon('redact')}Aree redatte: <b>${count}</b></div>
      <div class="note-box warn">Le aree sono state rimosse dal contenuto, non solo coperte: testo, immagini e annotazioni sottostanti non sono più estraibili. L'operazione non è annullabile e il file originale resta intatto.</div>`,
    footer: '<button class="btn primary" data-close>Chiudi</button>'
  });
}

function sigStatusBadge(ok, label) {
  const color = ok === true ? '#34d399' : ok === false ? '#fb7185' : '#9aa4c4';
  const bg = ok === true ? 'rgba(52,211,153,.14)' : ok === false ? 'rgba(251,113,133,.14)' : 'rgba(255,255,255,.06)';
  const border = ok === true ? 'rgba(52,211,153,.38)' : ok === false ? 'rgba(251,113,133,.38)' : 'rgba(255,255,255,.16)';
  return `<span style="display:inline-block;padding:2px 9px;border-radius:999px;font-size:9.5px;font-weight:700;letter-spacing:.03em;color:${color};background:${bg};border:1px solid ${border};white-space:nowrap">${esc(label)}</span>`;
}
function sigExtraText(value) {
  if (value === undefined || value === null || value === '') return '';
  if (value === true) return 'sì';
  if (value === false) return 'no';
  if (typeof value === 'object') {
    const label = value.status ?? value.state ?? value.label ?? value.message ?? value.result ?? value.detail;
    if (label !== undefined) return sigExtraText(label);
    try { return JSON.stringify(value); } catch { return String(value); }
  }
  return String(value);
}
function sigRevocationState(value) {
  if (value === true) return true;
  if (value === false) return null;
  const v = String(value ?? '').toLowerCase();
  if (/non revocat|not revoked|not revocat/.test(v)) return true;
  if (/(revoked|invalid|bad|revocat|scadut)/.test(v)) return false;
  if (/(good|valid|ok|none|nessuna|valida)/.test(v)) return true;
  return null;
}
function sigDocmdpLabel(value) {
  if (value === undefined || value === null || value === '') return '';
  const v = String(typeof value === 'object' ? (value.level ?? value.type ?? value.status ?? '') : value).toUpperCase();
  if (v === 'P1' || v === '1') return 'P1 · nessuna modifica consentita';
  if (v === 'P2' || v === '2') return 'P2 · solo compilazione moduli';
  if (v === 'P3' || v === '3') return 'P3 · annotazioni e moduli consentiti';
  if (v === 'NONE' || v === '0' || v === 'NESSUNA') return 'Nessuna (non certificato)';
  return sigExtraText(value);
}
function sigRow(label, ok, value) {
  const color = ok === false ? '#fecdd3' : ok === true ? '#a7f3d0' : '#d8dcef';
  return `<div style="display:flex;justify-content:space-between;align-items:baseline;gap:14px;padding:4px 0;border-top:1px solid rgba(255,255,255,.055);font-size:10.5px">
    <span style="color:#9aa4c4">${esc(label)}</span>
    <span style="color:${color};text-align:right">${esc(value)}</span>
  </div>`;
}
function signatureCard(s) {
  s = s && typeof s === 'object' ? s : {};
  const checks = [];
  if (typeof s.digestOk === 'boolean') checks.push(s.digestOk);
  if (typeof s.signatureOk === 'boolean') checks.push(s.signatureOk);
  if (typeof s.chainValid === 'boolean') checks.push(s.chainValid);
  const ok = checks.length ? checks.every(Boolean) : null;
  const rows = [];
  if (typeof s.digestOk === 'boolean') rows.push(sigRow('Integrità del documento', s.digestOk, s.digestOk ? 'valida' : 'non valida'));
  if (typeof s.signatureOk === 'boolean') rows.push(sigRow('Firma CMS', s.signatureOk, s.signatureOk ? 'valida' : 'non valida'));
  if (typeof s.coversAll === 'boolean') rows.push(sigRow('Copertura del file', s.coversAll, s.coversAll ? 'intero documento' : 'parziale'));
  if (typeof s.chainValid === 'boolean') rows.push(sigRow('Catena certificati', s.chainValid, s.chainValid ? 'valida' : 'non valida'));
  if (s.revocation !== undefined && s.revocation !== null) {
    rows.push(sigRow('Revoca del certificato', sigRevocationState(s.revocation), sigExtraText(s.revocation) || 'non verificata'));
  }
  if (s.timestamp !== undefined && s.timestamp !== null) {
    const tsLabel = s.timestamp === true ? 'presente' : s.timestamp === false ? 'assente' : (sigExtraText(s.timestamp) || 'non verificata');
    rows.push(sigRow('Marca temporale', s.timestamp === true ? true : null, tsLabel));
  }
  if (s.validFrom || s.validTo) {
    rows.push(sigRow('Validità certificato', null, `${sigExtraText(s.validFrom) || '—'} → ${sigExtraText(s.validTo) || '—'}`));
  }
  if (s.docmdp !== undefined && s.docmdp !== null && s.docmdp !== '') {
    rows.push(sigRow('Certificazione DocMDP', null, sigDocmdpLabel(s.docmdp)));
  }
  const meta = [
    s.field ? `campo ${esc(s.field)}` : '',
    s.subfilter ? esc(s.subfilter) : '',
    s.time ? esc(s.time) : ''
  ].filter(Boolean).join(' · ') || 'firma digitale';
  return `<div style="border:1px solid ${ok === false ? 'rgba(251,113,133,.3)' : 'rgba(255,255,255,.1)'};border-radius:12px;padding:11px 13px;margin-bottom:10px;background:rgba(255,255,255,.022)">
    <div style="display:flex;align-items:center;gap:9px;flex-wrap:wrap">
      <b style="font-size:12px;color:#eef2ff">${esc(s.signer || 'Firmatario')}</b>
      ${sigStatusBadge(ok, ok === true ? 'Firma valida' : ok === false ? 'Firma non valida' : 'Non verificata')}
    </div>
    <div style="font-size:10px;color:#9aa4c4;margin:3px 0 7px">${meta}</div>
    ${rows.join('')}
  </div>`;
}
async function verifySignatures(online = false) {
  if (!state.doc) return toast('Apri un PDF per verificare le firme.', 'warn');
  if (state.demo) return toast('La verifica richiede un PDF reale, non il documento dimostrativo.', 'warn');
  try {
    setStatus(online ? 'Verifica delle firme e della revoca online…' : 'Verifica delle firme…', true);
    const res = await invoke('verify_signatures_cmd', { checkRevocation: !!online });
    setStatus('Pronto');
    const list = Array.isArray(res) ? res : Array.isArray(res?.signatures) ? res.signatures : [];
    const body = list.length
      ? `<div class="result-line">${icon('signature')}Firme trovate: <b>${list.length}</b></div>` + list.map(signatureCard).join('') + `<div class="note-box">La verifica controlla in locale digest, firma CMS, catena dei certificati, attributi firmati, marca temporale e certificazione DocMDP.${online ? ' La revoca è stata interrogata via OCSP.' : ' Per la revoca serve una connessione: usa «Controlla revoca online».'}</div>`
      : '<div class="note-box">Nessuna firma digitale trovata. Le firme-immagine non sono firme crittografiche e non vengono verificate.</div>';
    openModal({
      title: 'Firme digitali',
      desc: 'Controllo crittografico delle firme presenti nel documento aperto.',
      icon: 'check',
      body,
      footer: `<button class="btn ghost" id="sigRevoke">${icon('globe')}Controlla revoca online</button><button class="btn ghost" data-close>Chiudi</button>`,
      onMount: (root) => {
        $('#sigRevoke', root)?.addEventListener('click', () => {
          closeModal();
          verifySignatures(true);
        });
      }
    });
  } catch (e) {
    setStatus('Pronto');
    toast(normalizeError(e), 'error');
  }
}

/* Riconosce gli errori di argomenti sconosciuti/mancanti, così la firma può
   riprovare una volta con il payload storico se il backend è più vecchio. */
function isPadesArgsError(message) {
  const m = String(message || '').toLowerCase();
  if (!m) return false;
  if (/failed to deserialize|invalid type|invalid value|invalid args|unknown field|unknown argument|unknown parameter|missing (required )?(field|argument|key)|unexpected (field|argument)/.test(m)) return true;
  return /\b(format|visible|certification|password|docmdp)\b/.test(m) && /(unknown|missing|invalid|unsupported|non riconosciut|non supportat|unexpected)/.test(m);
}

async function signPades() {
  if (!state.doc) return toast('Apri un PDF per firmarlo.', 'warn');
  if (state.demo) return toast('La firma digitale richiede un PDF reale.', 'warn');
  let format = 'pem';
  let certPath = '';
  let keyPath = '';
  let pfxPath = '';
  let working = false;
  const certHints = {
    none: 'Nessuna certificazione: la firma copre il contenuto attuale e il documento resta modificabile.',
    P2: 'Certificazione P2: dopo la firma sono consentite solo la compilazione dei campi modulo e le firme.',
    P1: "Certificazione P1: nessuna modifica è consentita dopo la firma. Le eventuali aree di firma non ancora compilate resteranno bloccate.",
    P3: 'Certificazione P3: sono consentite annotazioni, compilazione dei moduli e ulteriori firme.'
  };
  openModal({
    title: 'Firma digitale PAdES',
    desc: 'Scegli il formato del certificato, il livello di certificazione DocMDP e le informazioni del firmatario.',
    icon: 'signature',
    wide: true,
    body: `
      <div class="sig-tabs">
        <button class="sig-tab active" type="button" data-pades-format="pem">PEM · certificato + chiave</button>
        <button class="sig-tab" type="button" data-pades-format="pfx">PFX / P12 · file unico</button>
      </div>
      <div id="padesPemWrap">
        <button class="btn ghost wide small" type="button" id="padesPickCert" style="justify-content:flex-start">${icon('folder')}Scegli certificato (.pem / .crt)</button>
        <div class="note-box warn" id="padesCertBox" style="margin-top:6px">Nessun certificato selezionato</div>
        <button class="btn ghost wide small" type="button" id="padesPickKey" style="justify-content:flex-start;margin-top:6px">${icon('lock')}Scegli chiave privata (.pem / .key)</button>
        <div class="note-box warn" id="padesKeyBox" style="margin-top:6px">Nessuna chiave selezionata</div>
      </div>
      <div id="padesPfxWrap" hidden>
        <button class="btn ghost wide small" type="button" id="padesPickP12" style="justify-content:flex-start">${icon('folder')}Scegli file PFX / P12</button>
        <div class="note-box warn" id="padesP12Box" style="margin-top:6px">Nessun file selezionato</div>
        <div class="field" style="margin:10px 0 0"><label>Password del file</label><input type="password" id="padesPfxPassword" placeholder="Password del certificato" autocomplete="off"></div>
      </div>
      <label class="check-row"><input type="checkbox" id="padesVisible" checked> Mostra timbro visibile sulla pagina</label>
      <div class="field"><label>Certificazione DocMDP</label>
        <select id="padesCertification">
          <option value="none">Nessuna</option>
          <option value="P2">Solo moduli (P2)</option>
          <option value="P1">Vieta modifiche (P1)</option>
          <option value="P3">Consenti annotazioni (P3)</option>
        </select>
        <div class="note-box" id="padesCertHint" style="margin-top:6px">${esc(certHints.none)}</div>
      </div>
      <div class="field"><label>Marca temporale RFC 3161 (facoltativa)</label><input id="padesTsa" type="url" placeholder="https://tsa.esempio.it/tsr" autocomplete="off">
        <div class="note-box" style="margin-top:6px">Se inserisci l'indirizzo di un servizio di marca temporale (TSA) la firma diventa PAdES Baseline B-T. Lascia vuoto per firmare senza marca.</div>
      </div>
      <div class="field-row three">
        <div class="field" style="margin:0"><label>Firmatario</label><input id="padesName" type="text" maxlength="80" placeholder="Mario Rossi"></div>
        <div class="field" style="margin:0"><label>Motivo</label><input id="padesReason" type="text" maxlength="120" placeholder="Approvazione del documento"></div>
        <div class="field" style="margin:0"><label>Luogo</label><input id="padesLocation" type="text" maxlength="80" placeholder="Roma"></div>
      </div>
      <div class="note-box warn">La firma è PAdES Baseline B-B, oppure B-T se indichi una TSA (SHA-256 e signingCertificateV2). La verifica del trust store di Windows e la revoca online non sono automatiche: non è una firma qualificata eIDAS. Il PDF viene riscritto: le firme già presenti non restano valide.</div>`,
    footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="padesGo">${icon('signature')}Firma</button>`,
    onMount: (root) => {
      const setPath = (boxId, path) => {
        const box = $('#' + boxId, root);
        if (!box) return;
        box.textContent = path ? truncatePath(path, 64) : 'Nessun file selezionato';
        box.title = path || '';
        box.classList.toggle('warn', !path);
      };
      const setFormat = (next) => {
        format = next === 'pfx' ? 'pfx' : 'pem';
        $$('[data-pades-format]', root).forEach(b => b.classList.toggle('active', b.dataset.padesFormat === format));
        const pem = $('#padesPemWrap', root);
        const pfx = $('#padesPfxWrap', root);
        if (pem) pem.hidden = format !== 'pem';
        if (pfx) pfx.hidden = format !== 'pfx';
      };
      $$('[data-pades-format]', root).forEach(b => b.addEventListener('click', () => setFormat(b.dataset.padesFormat)));
      const pick = async (kind, boxId, assign) => {
        try {
          const p = await invoke('pick_certificate', { kind });
          if (!p) return;
          assign(p);
          setPath(boxId, p);
        } catch (e) { toast(normalizeError(e), 'error'); }
      };
      $('#padesPickCert', root)?.addEventListener('click', () => pick('certificato', 'padesCertBox', p => { certPath = p; }));
      $('#padesPickKey', root)?.addEventListener('click', () => pick('chiave', 'padesKeyBox', p => { keyPath = p; }));
      $('#padesPickP12', root)?.addEventListener('click', () => pick('p12', 'padesP12Box', p => { pfxPath = p; }));
      $('#padesCertification', root)?.addEventListener('change', (e) => {
        const hint = $('#padesCertHint', root);
        if (hint) hint.textContent = certHints[e.target.value] || certHints.none;
      });

      const go = $('#padesGo', root);
      go?.addEventListener('click', async () => {
        if (working) return;
        const pfx = format === 'pfx';
        if (pfx && !pfxPath) return toast('Scegli il file PFX / P12 da usare per la firma.', 'warn');
        if (!pfx && !certPath) return toast('Scegli il certificato PEM.', 'warn');
        if (!pfx && !keyPath) return toast('Scegli la chiave privata PEM.', 'warn');
        const name = ($('#padesName', root)?.value || '').trim() || 'Firmatario';
        const reasonValue = ($('#padesReason', root)?.value || '').trim();
        const locationValue = ($('#padesLocation', root)?.value || '').trim();
        const visible = !!$('#padesVisible', root)?.checked;
        const certification = $('#padesCertification', root)?.value || 'none';
        const payload = {
          page: state.page,
          certPath: pfx ? pfxPath : certPath,
          keyPath: pfx ? '' : keyPath,
          password: pfx ? ($('#padesPfxPassword', root)?.value || '') : '',
          format: pfx ? 'pfx' : 'pem',
          visible,
          certification,
          tsaUrl: ($('#padesTsa', root)?.value || '').trim(),
          name,
          reason: reasonValue,
          location: locationValue
        };
        working = true;
        go.disabled = true;
        setStatus('Firma in corso…', true);
        try {
          let res;
          let fallback = false;
          try {
            res = await invoke('sign_pdf_pades', payload);
          } catch (e) {
            const msg = normalizeError(e);
            if (!isPadesArgsError(msg)) throw e;
            if (pfx) throw new Error(msg + ' Il backend attuale non sembra supportare i file PFX/P12: usa il formato PEM (certificato + chiave).');
            res = await invoke('sign_pdf_pades', {
              page: payload.page,
              certPath: payload.certPath,
              keyPath: payload.keyPath,
              name: payload.name,
              reason: payload.reason,
              location: payload.location
            });
            fallback = true;
          }
          closeModal();
          const info = docInfoFrom(res);
          if (info) applyOpenInfo(info, []);
          toast((res && res.message) || 'Documento firmato.');
          if (fallback) toast('Il backend non supporta ancora formato, timbro visibile o DocMDP: la firma è stata applicata con le impostazioni base.', 'warn', 6500);
        } catch (e) {
          toast(normalizeError(e), 'error', 9000);
        }
        working = false;
        if (go.isConnected) go.disabled = false;
        setStatus('Pronto');
      });
    }
  });
}

/* ---------------- edit existing text ---------------- */
async function loadTextLines() {
  if (!state.doc) return;
  const seq = ++state.textLinesSeq;
  state.textSelection = [];
  if (state.demo) {
    state.textLines = typeof demoTextLines === 'function' ? demoTextLines(state.page) : [];
    drawOverlay();
    updateToolAction();
    return;
  }
  try {
    const lines = await invoke('page_text_lines', { page: state.page });
    if (seq !== state.textLinesSeq) return;
    state.textLines = Array.isArray(lines) ? lines : [];
    drawOverlay();
    updateToolAction();
  } catch (e) {
    state.textLines = [];
    drawOverlay();
    updateToolAction();
    toast('Testo non selezionabile: ' + normalizeError(e), 'warn', 6000);
  }
}
function selectedTextLines() {
  return state.textSelection.map(i => state.textLines[i]).filter(Boolean);
}
function selectAllText() {
  state.textSelection = state.textLines.map((_, i) => i);
  drawOverlay();
  updateToolAction();
}
function clearTextSelection() {
  state.textSelection = [];
  drawOverlay();
  updateToolAction();
}
async function deleteSelectedText() {
  const selected = selectedTextLines();
  if (!selected.length) return toast('Seleziona prima una o più righe di testo.', 'warn');
  if (state.demo) return toast("L'eliminazione del testo è disponibile nell'app desktop.", 'warn');
  const ok = await confirmDialog(
    'Eliminare il testo selezionato?',
    `Verranno eliminate ${selected.length} righe dalla copia di lavoro. Se il PDF raggruppa più righe nello stesso oggetto, potrebbero essere rimosse anche righe adiacenti. L'originale non viene modificato.`,
    'Elimina testo',
    true
  );
  if (!ok) return;
  try {
    setStatus('Eliminazione del testo…', true);
    const rects = selected.map(l => [l.x, l.y, l.w, l.h]);
    const info = await invoke('delete_page_text', { page: state.page, rects });
    applyOpenInfo(info, state.annotations);
    toast('Testo eliminato dalla copia di lavoro.');
  } catch (e) {
    toast(normalizeError(e), 'error', 8000);
  }
  setStatus('Pronto');
}
function showTextEditHelp() {
  openModal({
    title: 'Modificare il testo',
    icon: 'info',
    desc: 'La modifica agisce sul contenuto reale del PDF, non su una copertura.',
    body: `<div class="note-box">
        <b>Doppio clic</b> su una riga per modificarla subito.<br>
        <b>Shift + clic</b> per aggiungere o togliere righe dalla selezione multipla.<br>
        <b>Clic singolo</b> seleziona una sola riga.<br>
        <b>Ctrl+Enter</b> conferma la modifica nel riquadro di testo.<br>
        Con il documento attivo, <b>Ctrl+A</b> seleziona tutte le righe della pagina ed <b>Esc</b> deseleziona.
      </div>
      <div class="note-box warn">Limiti noti: il testo sostitutivo usa Helvetica quando il font originale è incorporato o sottoinsiemato; i caratteri CJK (cinese, giapponese, coreano) non sono supportati; il testo più lungo della riga viene ridotto per rientrare nell'area disponibile.</div>`,
    footer: '<button class="btn ghost" data-close>Ho capito</button>'
  });
}

function askReplacementText(line) {
  return new Promise((resolve) => {
    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      modalOnClose = null;
      closeModal();
      resolve(value);
    };
    openModal({
      title: 'Modifica testo',
      icon: 'edit',
      desc: 'Il nuovo testo sostituisce davvero la riga nel PDF: resta selezionabile e ricercabile.',
      body: `<div class="field"><label>Nuovo testo</label><textarea id="replaceTextArea" rows="5" placeholder="Testo da inserire nella riga…"></textarea></div>
        <div class="note-box" id="replaceTextMeta"></div>
        <div class="note-box warn">Se il font originale è incorporato o sottoinsiemato viene usato Helvetica; i caratteri CJK (cinese, giapponese, coreano) non sono supportati; il testo più lungo della riga viene ridotto per rientrare nello spazio disponibile.</div>`,
      footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="replaceTextOk">${icon('check')}Sostituisci</button>`,
      onMount: (root) => {
        const area = $('#replaceTextArea', root);
        const meta = $('#replaceTextMeta', root);
        if (!area) return;
        area.value = line.text || '';
        const updateMeta = () => {
          const value = area.value || '';
          const rows = value ? value.split('\n').length : 0;
          if (meta) {
            meta.textContent = `${value.length} caratteri · ${rows} riga${rows === 1 ? '' : 'e'} · riga originale: “${(line.text || '').slice(0, 70)}${(line.text || '').length > 70 ? '…' : ''}”`;
            meta.classList.toggle('warn', value.length > 400);
          }
        };
        area.addEventListener('input', updateMeta);
        updateMeta();
        setTimeout(() => { area.focus(); area.select(); }, 80);
        const submit = () => {
          const value = area.value.replace(/\r\n/g, '\n');
          if (!value.trim()) { toast('Il nuovo testo è vuoto. Usa «Elimina testo» per rimuovere la riga.', 'warn'); return; }
          finish(value);
        };
        $('#replaceTextOk', root)?.addEventListener('click', submit);
        area.addEventListener('keydown', (e) => {
          if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); submit(); }
        });
      },
      onClose: () => {
        if (settled) return;
        settled = true;
        resolve(null);
      }
    });
  });
}

async function replaceSelectedText() {
  const line = selectedTextLines()[0];
  if (!line) return toast('Seleziona una riga da sostituire.', 'warn');
  if (state.demo) return toast("La sostituzione del testo è disponibile nell'app desktop.", 'warn');
  const replacement = await askReplacementText(line);
  if (replacement === null) return;
  if (!replacement.trim()) return toast('Il nuovo testo è vuoto. Usa «Elimina testo» per rimuovere la riga.', 'warn');
  try {
    setStatus('Modifica del testo…', true);
    const res = await invoke('replace_text_in_place', {
      page: state.page,
      rect: [line.x, line.y, line.w, line.h],
      text: replacement
    });
    const info = docInfoFrom(res);
    if (info) applyOpenInfo(info, state.annotations);
    else await loadTextLines();
    toast((res && res.message) || 'Testo modificato nel PDF: il contenuto è reale, selezionabile e ricercabile.');
  } catch (e) {
    toast(normalizeError(e), 'error', 9000);
  }
  setStatus('Pronto');
}

/* ---------------- keyboard ---------------- */
function bindKeyboard() {
  window.addEventListener('keydown', (e) => {
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName) || e.target.isContentEditable;
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'o') { e.preventDefault(); openDialog(); return; }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') { e.preventDefault(); openSaveModal(); return; }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') { e.preventDefault(); focusSearch(); return; }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') { e.preventDefault(); focusSearch(); return; }
    if ((e.ctrlKey || e.metaKey) && (e.key === '+' || e.key === '=')) {
      e.preventDefault();
      if (state.doc) setZoom(state.zoom + 0.15);
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.key === '-') {
      e.preventDefault();
      if (state.doc) setZoom(state.zoom - 0.15);
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.key === '0') {
      e.preventDefault();
      if (state.doc) setZoom(1);
      return;
    }
    if (typing) return;
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a' && state.tool === 'edittext') {
      e.preventDefault();
      selectAllText();
      return;
    }
    const keyTool = { v: 'browse', e: 'edittext', h: 'highlight', u: 'underline', n: 'note', s: 'stamp', p: 'ink', t: 'text', f: 'signature', r: 'redact' };
    if (!e.ctrlKey && !e.metaKey && !e.altKey && keyTool[e.key.toLowerCase()]) { setTool(keyTool[e.key.toLowerCase()]); return; }
    if (e.key === 'Escape') {
      if ($('#modalRoot').children.length) closeModal();
      else if (state.tool === 'edittext' && state.textSelection.length) clearTextSelection();
      else selectAnnotation(null);
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'z') { e.preventDefault(); undo(); return; }
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'y') { e.preventDefault(); redo(); return; }
    if (e.key === 'Delete' || e.key === 'Backspace') {
      if (state.tool === 'edittext' && state.textSelection.length) { e.preventDefault(); deleteSelectedText(); }
      else if (state.selected) { e.preventDefault(); deleteSelected(); }
      return;
    }
    if (e.key === 'Enter' && state.tool === 'edittext' && state.textSelection.length === 1) {
      e.preventDefault();
      replaceSelectedText();
      return;
    }
    if (e.key === '+' || e.key === '=') { if (state.doc) setZoom(state.zoom + .1); return; }
    if (e.key === '-') { if (state.doc) setZoom(state.zoom - .1); return; }
    if (e.key === 'PageDown' || e.key === 'ArrowRight') { if (state.doc) gotoPage(state.page + 1); return; }
    if (e.key === 'PageUp' || e.key === 'ArrowLeft') { if (state.doc) gotoPage(state.page - 1); return; }
  });
}

