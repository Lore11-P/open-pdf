/* Open PDF · Focus Dark — strumenti */

/* ---------------- merge / split / compress ---------------- */
async function openMerge() {
  if (!HAS_TAURI) return toast("Disponibile nell'app desktop.", 'warn');
  const local = { files: [] };
  const redraw = (root) => {
    const list = $('#mergeList', root);
    const includeCurrent = $('#mergeCurrent', root)?.checked;
    const items = [
      ...(includeCurrent && state.doc ? [{ path: state.doc.path, name: (state.doc.name || 'Documento') + ' (corrente)', current: true }] : []),
      ...local.files.map(f => ({ path: f, name: f.split(/[\\/]/).pop() }))
    ];
    list.innerHTML = items.length ? items.map((it, i) => `<div class="file-row">${icon('office')}<span title="${esc(it.path)}">${esc(it.name)}</span>${it.current ? '<small class="muted">corrente</small>' : `<button data-remove="${i}">${icon('close')}</button>`}</div>`).join('') : '<div class="empty-list">Nessun file selezionato.</div>';
    $$('[data-remove]', list).forEach(b => b.addEventListener('click', () => {
      const i = parseInt(b.dataset.remove, 10);
      const start = (includeCurrent && state.doc) ? 1 : 0;
      local.files.splice(i - start, 1);
      redraw(root);
    }));
  };
  openModal({
    title: 'Unisci PDF', icon: 'merge',
    desc: 'Crea un nuovo documento unendo più file. Le annotazioni del documento corrente non vengono trasferite.',
    body: `<label class="check-row"><input type="checkbox" id="mergeCurrent" ${state.doc ? 'checked' : ''} ${state.doc ? '' : 'disabled'}> Includi il documento corrente</label>
      <div class="file-drop" id="mergePick">${icon('plus')} Aggiungi file PDF</div>
      <div class="file-list" id="mergeList"></div>
      <div class="note-box">L'ordine di unione è quello mostrato nell'elenco.</div>`,
    footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="mergeApply">${icon('merge')}Unisci</button>`,
    onMount: (root) => {
      $('#mergePick', root).addEventListener('click', async () => {
        try {
          const files = await invoke('pick_pdfs');
          for (const f of files) if (!local.files.includes(f)) local.files.push(f);
          redraw(root);
        } catch (e) { toast(normalizeError(e), 'error'); }
      });
      $('#mergeCurrent', root).addEventListener('change', () => redraw(root));
      redraw(root);
      $('#mergeApply', root).addEventListener('click', async () => {
        const includeCurrent = $('#mergeCurrent', root).checked;
        const paths = [...(includeCurrent && state.doc ? [state.doc.path] : []), ...local.files];
        if (!paths.length) return toast('Aggiungi almeno un PDF.', 'warn');
        try {
          setStatus('Unione dei documenti…', true);
          const info = await invoke('merge_pdfs_cmd', { paths });
          applyOpenInfo(info, []);
          closeModal();
          toast('Documento unito creato.');
        } catch (e) { toast(normalizeError(e), 'error'); }
        setStatus('Pronto');
      });
    }
  });
}

async function openSplit() {
  if (!state.doc) return toast('Apri prima un documento.', 'warn');
  openModal({
    title: 'Dividi / estrai pagine', icon: 'split',
    desc: 'Estrai un intervallo di pagine in un nuovo PDF.',
    body: `<div class="field-row"><div class="field"><label>Da pagina</label><input type="number" id="splitFrom" min="1" max="${state.doc.pageCount}" value="1"></div>
      <div class="field"><label>A pagina</label><input type="number" id="splitTo" min="1" max="${state.doc.pageCount}" value="${state.doc.pageCount}"></div></div>
      <div class="note-box">Verrà creato un nuovo file, senza modificare l'originale.</div>`,
    footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="splitRun">${icon('split')}Estrai</button>`,
    onMount: (root) => {
      $('#splitRun', root).addEventListener('click', async () => {
        const from = parseInt($('#splitFrom', root).value, 10);
        const to = parseInt($('#splitTo', root).value, 10);
        if (!from || !to || to < from) return toast('Intervallo non valido.', 'warn');
        try {
          const dest = await invoke('pick_save', { defaultName: `${baseName()}_pagine_${from}-${to}.pdf`, filters: ['pdf'] });
          if (!dest) return;
          setStatus('Estrazione pagine…', true);
          const res = await invoke('split_pdf_cmd', { dest, from, to });
          closeModal();
          showResult('Pagine estratte', res.message, res.path, true);
        } catch (e) { toast(normalizeError(e), 'error'); }
        setStatus('Pronto');
      });
    }
  });
}

async function openCompress() {
  if (!state.doc) return toast('Apri prima un documento.', 'warn');
  openModal({
    title: 'Comprimi PDF', icon: 'compress',
    desc: 'Riduci il peso del file mantenendo una buona resa visiva.',
    body: `<div class="field"><label>Metodo</label><select id="compMode">
        <option value="lossless">Ottimizza struttura · senza perdita</option>
        <option value="raster">Rasterizza pagine · riduzione forte</option>
      </select></div>
      <div id="rasterOpts" hidden>
        <div class="field"><label>Risoluzione (DPI)</label><div class="range-line"><input type="range" id="compDpi" min="80" max="220" value="130"><span id="compDpiVal">130</span></div></div>
        <div class="field"><label>Qualità JPEG</label><div class="range-line"><input type="range" id="compQuality" min="35" max="90" value="70"><span id="compQualityVal">70</span></div></div>
        <div class="note-box warn">La rasterizzazione elimina il testo selezionabile e i link, ma riduce sensibilmente il peso.</div>
      </div>
      <div class="note-box">Dimensione attuale: <b>${fmtBytes(state.doc.fileSize)}</b></div>`,
    footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="compRun">${icon('compress')}Comprimi</button>`,
    onMount: (root) => {
      const mode = $('#compMode', root);
      const opts = $('#rasterOpts', root);
      const sync = () => { opts.hidden = mode.value !== 'raster'; };
      mode.addEventListener('change', sync); sync();
      $('#compDpi', root).addEventListener('input', e => $('#compDpiVal').textContent = e.target.value);
      $('#compQuality', root).addEventListener('input', e => $('#compQualityVal').textContent = e.target.value);
      $('#compRun', root).addEventListener('click', async () => {
        try {
          const dest = await invoke('pick_save', { defaultName: baseName() + '_compresso.pdf', filters: ['pdf'] });
          if (!dest) return;
          setStatus('Compressione in corso…', true);
          const res = await invoke('compress_pdf', {
            dest, mode: mode.value,
            dpi: parseInt($('#compDpi', root).value, 10),
            quality: parseInt($('#compQuality', root).value, 10)
          });
          closeModal();
          showResult('PDF compresso', res.message, res.path, true);
        } catch (e) { toast(normalizeError(e), 'error'); }
        setStatus('Pronto');
      });
    }
  });
}

/* ---------------- convert ---------------- */
async function openImages() {
  if (!HAS_TAURI) return toast("Disponibile nell'app desktop.", 'warn');
  const local = { images: [] };
  const redraw = (root) => {
    const list = $('#imgList', root);
    list.innerHTML = local.images.length ? local.images.map((p, i) => `<div class="file-row">${icon('image')}<span title="${esc(p)}">${esc(p.split(/[\\/]/).pop())}</span><button data-i="${i}">${icon('close')}</button></div>`).join('') : '<div class="empty-list">Nessuna immagine selezionata.</div>';
    $$('[data-i]', list).forEach(b => b.addEventListener('click', () => { local.images.splice(parseInt(b.dataset.i, 10), 1); redraw(root); }));
  };
  openModal({
    title: 'Immagini → PDF', icon: 'image',
    desc: 'Trasforma foto e scansioni in un PDF. Ogni immagine diventa una pagina.',
    body: `<div class="file-drop" id="imgPick">${icon('plus')} Scegli immagini</div>
      <div class="file-list" id="imgList"></div>
      <label class="check-row"><input type="checkbox" id="imgOcr"> Esegui OCR sulla prima pagina</label>
      <div class="note-box">L'OCR di Windows riconosce il testo in locale. Nessuna immagine viene inviata online.</div>`,
    footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="imgRun">${icon('image')}Crea PDF</button>`,
    onMount: (root) => {
      redraw(root);
      $('#imgPick', root).addEventListener('click', async () => {
        try {
          const files = await invoke('pick_images');
          for (const f of files) if (!local.images.includes(f)) local.images.push(f);
          redraw(root);
        } catch (e) { toast(normalizeError(e), 'error'); }
      });
      $('#imgRun', root).addEventListener('click', async () => {
        if (!local.images.length) return toast("Scegli almeno un'immagine.", 'warn');
        const doOcr = $('#imgOcr', root).checked;
        try {
          setStatus('Creazione PDF dalle immagini…', true);
          const info = await invoke('images_to_pdf_cmd', { images: local.images });
          applyOpenInfo(info, []);
          closeModal();
          toast('PDF creato dalle immagini.');
          if (doOcr) setTimeout(() => runOcrPage(0), 400);
        } catch (e) { toast(normalizeError(e), 'error'); }
        setStatus('Pronto');
      });
    }
  });
}

async function openWeb() {
  if (!HAS_TAURI) return toast("Disponibile nell'app desktop.", 'warn');
  openModal({
    title: 'Web → PDF', icon: 'globe',
    desc: 'Stampa una pagina web in PDF usando Microsoft Edge in locale.',
    body: `<div class="field"><label>Indirizzo web</label><input type="url" id="webUrl" placeholder="https://esempio.it"></div>
      <div class="note-box">Richiede una connessione a Internet per caricare la pagina. Il PDF viene creato sul tuo computer con Edge headless.</div>`,
    footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="webRun">${icon('globe')}Converti</button>`,
    onMount: (root) => {
      setTimeout(() => $('#webUrl', root).focus(), 100);
      $('#webRun', root).addEventListener('click', async () => {
        const url = $('#webUrl', root).value.trim();
        if (!url) return toast('Inserisci un indirizzo web.', 'warn');
        try {
          setStatus('Conversione della pagina web…', true);
          const info = await invoke('web_to_pdf_cmd', { url });
          applyOpenInfo(info, []);
          closeModal();
          toast('Pagina web convertita in PDF.');
        } catch (e) { toast(normalizeError(e), 'error'); }
        setStatus('Pronto');
      });
    }
  });
}

async function openOffice() {
  if (!HAS_TAURI) return toast("Disponibile nell'app desktop.", 'warn');
  try {
    const path = await invoke('pick_file', { filters: ['docx', 'doc', 'xlsx', 'xls', 'pptx', 'ppt', 'odt', 'ods', 'odp', 'rtf', 'csv', 'txt', 'md'] });
    if (!path) return;
    setStatus('Conversione del file Office…', true);
    const info = await invoke('office_to_pdf_cmd', { path });
    applyOpenInfo(info, []);
    toast((info && info.message) || 'Documento Office convertito in PDF.');
  } catch (e) { toast(normalizeError(e), 'error'); }
  setStatus('Pronto');
}

async function openExport() {
  if (!state.doc) return toast('Apri prima un documento.', 'warn');
  openModal({
    title: 'Esporta PDF', icon: 'export',
    desc: 'Crea un documento modificabile con il testo estratto dal PDF.',
    body: `<div class="field"><label>Formato</label><select id="exportKind">
        <option value="docx">Word (.docx)</option>
        <option value="xlsx">Excel (.xlsx)</option>
        <option value="pptx">PowerPoint (.pptx)</option>
        <option value="txt">Testo (.txt)</option>
      </select></div>
      <div class="note-box">La conversione avviene interamente in locale. Se LibreOffice è installato, Open PDF può usarlo per conversioni Office → PDF; per l'esportazione viene creato un documento leggibile con il testo del PDF.</div>`,
    footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="exportRun">${icon('export')}Esporta</button>`,
    onMount: (root) => {
      const kind = $('#exportKind', root);
      $('#exportRun', root).addEventListener('click', async () => {
        try {
          const filters = [kind.value];
          const dest = await invoke('pick_save', { defaultName: baseName() + '.' + kind.value, filters });
          if (!dest) return;
          setStatus('Esportazione in corso…', true);
          const res = await invoke('export_pdf', { kind: kind.value, dest });
          closeModal();
          showResult('Esportazione completata', res.message, res.path, true);
        } catch (e) { toast(normalizeError(e), 'error'); }
        setStatus('Pronto');
      });
    }
  });
}

async function openOcr() {
  openModal({
    title: 'Scansione & OCR', icon: 'scan',
    desc: 'Riconosci il testo da una pagina del PDF o da un\'immagine. Il motore OCR di Windows lavora interamente in locale.',
    body: `<div class="mini-actions">
        <button class="btn ghost" id="ocrPageBtn">${icon('scan')}OCR pagina ${state.page + 1}</button>
        <button class="btn ghost" id="ocrFileBtn">${icon('image')}OCR immagine…</button>
      </div>
      <div class="field" style="margin-top:12px"><label>Testo riconosciuto</label><textarea id="ocrText" readonly placeholder="Il testo riconosciuto apparirà qui…" style="min-height:220px"></textarea></div>
      <div class="mini-actions"><button class="btn ghost small" id="ocrCopy">${icon('copy')}Copia testo</button></div>
      <div class="note-box">Per documenti scansionati: crea prima un PDF da immagini, poi esegui l'OCR sulla pagina.</div>
      <div class="pane-label" style="margin-top:14px">Livello di testo invisibile</div>
      <div class="mini-actions">
        <button class="btn ghost small" id="ocrLayerPage">${icon('scan')}Rendi ricercabile · pagina ${state.page + 1}</button>
        <button class="btn ghost small" id="ocrLayerAll">Tutte le pagine</button>
      </div>
      <div class="note-box">Aggiunge il testo riconosciuto come livello invisibile: il PDF resta identico ma diventa selezionabile e ricercabile. L'operazione può richiedere tempo.</div>`,
    footer: `<button class="btn ghost" data-close>Chiudi</button>`,
    onMount: (root) => {
      const area = $('#ocrText', root);
      $('#ocrPageBtn', root).addEventListener('click', async () => {
        if (!state.doc) return toast('Apri prima un documento.', 'warn');
        area.value = 'Riconoscimento in corso…';
        try {
          area.value = await invoke('ocr_page', { page: state.page });
          toast('OCR completato.');
        } catch (e) { area.value = ''; toast(normalizeError(e), 'error'); }
      });
      $('#ocrFileBtn', root).addEventListener('click', async () => {
        try {
          const path = await invoke('pick_file', { filters: ['png', 'jpg', 'jpeg', 'bmp', 'tif', 'tiff'] });
          if (!path) return;
          area.value = 'Riconoscimento in corso…';
          area.value = await invoke('ocr_file', { path });
          toast('OCR completato.');
        } catch (e) { area.value = ''; toast(normalizeError(e), 'error'); }
      });
      $('#ocrCopy', root).addEventListener('click', () => { if (area.value) copyText(area.value); });
      const makeLayer = async (all) => {
        if (!state.doc) return toast('Apri prima un documento.', 'warn');
        if (all && state.doc.pageCount > 5) {
          const ok = await confirmDialog('Rendere ricercabili tutte le pagine?', `L'OCR verrà eseguito su ${state.doc.pageCount} pagine e può richiedere alcuni minuti. Il documento di lavoro verrà aggiornato con un livello di testo invisibile.`, 'Continua');
          if (!ok) return;
        }
        closeModal();
        try {
          setStatus(all ? 'OCR su tutte le pagine…' : 'OCR della pagina…', true);
          const info = await invoke('make_searchable_cmd', { from: all ? 0 : state.page, to: all ? state.doc.pageCount - 1 : state.page });
          applyOpenInfo(info, state.annotations);
          toast(all ? 'Livello di testo aggiunto a tutte le pagine.' : 'Livello di testo aggiunto alla pagina corrente.');
        } catch (e) { toast(normalizeError(e), 'error'); }
        setStatus('Pronto');
      };
      $('#ocrLayerPage', root).addEventListener('click', () => makeLayer(false));
      $('#ocrLayerAll', root).addEventListener('click', () => makeLayer(true));
    }
  });
}
async function runOcrPage(page) {
  if (!state.doc) return;
  openOcr();
  setTimeout(async () => {
    const area = $('#ocrText');
    if (!area) return;
    area.value = 'Riconoscimento in corso…';
    try { area.value = await invoke('ocr_page', { page }); } catch (e) { area.value = ''; toast(normalizeError(e), 'error'); }
  }, 150);
}

/* ---------------- forms ---------------- */
async function openFormFill() {
  if (!state.doc) return toast('Apri prima un documento.', 'warn');
  if (!state.formFields.length) {
    openModal({
      title: 'Nessun modulo rilevato', icon: 'form',
      desc: 'Questo PDF non contiene campi compilabili. Puoi crearne uno con Open PDF.',
      body: `<div class="note-box">La creazione di moduli aggiunge campi di testo e caselle di spunta interattive al documento.</div>`,
      footer: `<button class="btn ghost" data-close>Chiudi</button><button class="btn primary" id="goCreateForm">${icon('formPlus')}Crea modulo</button>`,
      onMount: (root) => {
        $('#goCreateForm', root).addEventListener('click', () => { closeModal(); startCreateForm(); });
      }
    });
    return;
  }
  openModal({
    title: 'Compila modulo', icon: 'form',
    desc: 'Modifica i campi e salva una copia compilata del documento.',
    body: state.formFields.map((f, i) => {
      const id = `formField_${i}`;
      if (f.kind === 'checkbox') {
        return `<div class="form-field-row"><label>${esc(f.label)}</label><label class="check-row"><input type="checkbox" id="${id}" data-page="${f.page}" data-index="${f.index}" ${f.checked ? 'checked' : ''}> ${esc(f.value || 'Selezionato')}</label></div>`;
      }
      if (['text', 'combo', 'list'].includes(f.kind)) {
        return `<div class="form-field-row"><label>${esc(f.label)}</label><input type="text" id="${id}" data-page="${f.page}" data-index="${f.index}" value="${esc(f.value || '')}"></div>`;
      }
      return `<div class="form-field-row"><label>${esc(f.label)}</label><small class="muted">${esc(f.kind)} · non modificabile</small></div>`;
    }).join(''),
    footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="fillRun">${icon('check')}Salva compilato</button>`,
    onMount: (root) => {
      $('#fillRun', root).addEventListener('click', async () => {
        const values = [];
        $$('[data-index]', root).forEach(el => {
          const page = parseInt(el.dataset.page, 10);
          const index = parseInt(el.dataset.index, 10);
          if (el.type === 'checkbox') values.push({ page, index, value: '', checked: el.checked });
          else values.push({ page, index, value: el.value, checked: false });
        });
        try {
          setStatus('Salvataggio del modulo…', true);
          const info = await invoke('fill_forms', { values });
          applyOpenInfo(info, state.annotations);
          closeModal();
          toast('Modulo compilato e salvato in una copia di lavoro.');
        } catch (e) { toast(normalizeError(e), 'error'); }
        setStatus('Pronto');
      });
    }
  });
}

function startCreateForm() {
  if (!state.doc) return toast('Apri prima un documento.', 'warn');
  state.creatingForm = true;
  state.newFieldKind = 'text';
  state.newFields = [];
  setTool('createform');
  renderFieldLayer();
  toast('Clicca sulla pagina per posizionare un campo.');
}
function addFormField(ev) {
  const p = normPoint(ev);
  const kind = state.newFieldKind;
  const count = state.newFields.filter(f => f.kind === kind).length + 1;
  const field = kind === 'checkbox'
    ? { page: state.page, x: clamp(p[0], .02, .95), y: clamp(p[1], .02, .95), w: .025, h: .018, kind, name: `Casella_${count}` }
    : { page: state.page, x: clamp(p[0], .02, .9), y: clamp(p[1], .02, .95), w: .28, h: .035, kind, name: `Testo_${count}` };
  state.newFields.push(field);
  renderFieldLayer();
  updateToolAction();
}
function renderFieldLayer() {
  const layer = $('#fieldLayer');
  if (!layer) return;
  if (!state.doc) { layer.innerHTML = ''; return; }
  layer.innerHTML = state.newFields.filter(f => f.page === state.page).map(f =>
    `<div class="new-field" style="left:${f.x * 100}%;top:${f.y * 100}%;width:${f.w * 100}%;height:${f.h * 100}%">${f.kind === 'checkbox' ? '☑' : esc(f.name)}</div>`
  ).join('');
}
async function saveCreatedForm() {
  if (!state.newFields.length) return toast('Aggiungi almeno un campo alla pagina.', 'warn');
  try {
    setStatus('Creazione del modulo interattivo…', true);
    const info = await invoke('create_form_fields_cmd', { fields: state.newFields });
    state.newFields = [];
    state.creatingForm = false;
    applyOpenInfo(info, state.annotations);
    hideFieldLayer();
    setTool('browse');
    toast('Modulo creato. Ora puoi compilarlo con "Compila modulo".');
  } catch (e) { toast(normalizeError(e), 'error'); }
  setStatus('Pronto');
}
function cancelCreateForm() {
  state.newFields = [];
  state.creatingForm = false;
  hideFieldLayer();
  setTool('browse');
}
function hideFieldLayer() {
  const layer = $('#fieldLayer');
  if (layer) layer.innerHTML = '';
}

/* ---------------- security ---------------- */
async function openPassword() {
  if (!state.doc) return toast('Apri prima un documento.', 'warn');
  openModal({
    title: 'Password & permessi', icon: 'lock',
    desc: 'Proteggi una copia del PDF con crittografia AES-128 e limita stampa, copia o modifica.',
    body: `<div class="field-row"><div class="field"><label>Password apertura</label><input type="password" id="secUser" placeholder="Password utente"></div>
      <div class="field"><label>Password proprietario</label><input type="password" id="secOwner" placeholder="Facoltativa"></div></div>
      <label class="check-row"><input type="checkbox" id="secPrint" checked> Consenti stampa</label>
      <label class="check-row"><input type="checkbox" id="secCopy"> Consenti copia del testo</label>
      <label class="check-row"><input type="checkbox" id="secModify"> Consenti modifica contenuti</label>
      <label class="check-row"><input type="checkbox" id="secAnnotate" checked> Consenti annotazioni</label>
      <label class="check-row"><input type="checkbox" id="secForms" checked> Consenti compilazione moduli</label>
      <div class="note-box">Il PDF protetto viene salvato come nuovo file. Conserva la password: non può essere recuperata.</div>`,
    footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="secRun">${icon('lock')}Proteggi PDF</button>`,
    onMount: (root) => {
      $('#secRun', root).addEventListener('click', async () => {
        const user = $('#secUser', root).value;
        const owner = $('#secOwner', root).value;
        if (!user && !owner) return toast('Inserisci almeno una password.', 'warn');
        try {
          const dest = await invoke('pick_save', { defaultName: baseName() + '_protetto.pdf', filters: ['pdf'] });
          if (!dest) return;
          setStatus('Crittografia del documento…', true);
          const res = await invoke('protect_pdf_cmd', {
            dest, userPassword: user, ownerPassword: owner,
            allowPrint: $('#secPrint', root).checked,
            allowCopy: $('#secCopy', root).checked,
            allowModify: $('#secModify', root).checked,
            allowAnnotate: $('#secAnnotate', root).checked,
            allowForms: $('#secForms', root).checked
          });
          closeModal();
          showResult('PDF protetto', res.message, res.path, true);
        } catch (e) { toast(normalizeError(e), 'error'); }
        setStatus('Pronto');
      });
    }
  });
}

async function openCompare() {
  if (!state.doc) return toast('Apri prima un documento.', 'warn');
  let second;
  try { second = await invoke('pick_file', { filters: ['pdf'] }); } catch (e) { return toast(normalizeError(e), 'error'); }
  if (!second) return;
  try {
    setStatus('Confronto dei documenti…', true);
    const diff = await invoke('compare_with', { second });
    renderDiff(diff, second);
  } catch (e) { toast(normalizeError(e), 'error'); }
  setStatus('Pronto');
}
function renderDiff(diff, second) {
  const changed = diff.pages.filter(p => p.changed);
  openModal({
    title: 'Confronto file', icon: 'compare', wide: true,
    desc: `Confronto tra il documento corrente e ${esc(second.split(/[\\/]/).pop())}.`,
    body: `<div class="diff-summary"><span class="diff-badge">${changed.length} pagine modificate</span><span class="diff-badge add">+ ${diff.addedChars} caratteri</span><span class="diff-badge del">− ${diff.removedChars} caratteri</span></div>
      ${changed.length ? `<div class="diff-list">${changed.map(p => `<div class="diff-page"><h4>Pagina ${p.page}</h4>${p.removed.map(x => `<div class="diff-line del">${esc(x)}</div>`).join('')}${p.added.map(x => `<div class="diff-line add">${esc(x)}</div>`).join('')}</div>`).join('')}</div>` : '<div class="note-box">Nessuna differenza testuale rilevata nelle pagine confrontate.</div>'}
      <div class="note-box">Il confronto analizza il testo estratto. Modifiche puramente grafiche o di layout potrebbero non essere rilevate.</div>`,
    footer: '<button class="btn ghost" data-close>Chiudi</button>'
  });
}

async function openMetadata() {
  if (!state.doc) return toast('Apri prima un documento.', 'warn');
  const m = state.doc.meta || {};
  openModal({
    title: 'Metadati del documento', icon: 'metadata',
    desc: 'Aggiorna titolo, autore e informazioni descrittive in una nuova copia del PDF.',
    body: `<div class="field"><label>Titolo</label><input type="text" id="mdTitle" value="${esc(m.title || '')}"></div>
      <div class="field-row"><div class="field"><label>Autore</label><input type="text" id="mdAuthor" value="${esc(m.author || '')}"></div>
      <div class="field"><label>Creatore</label><input type="text" id="mdCreator" value="${esc(m.creator || '')}"></div></div>
      <div class="field"><label>Soggetto</label><input type="text" id="mdSubject" value="${esc(m.subject || '')}"></div>
      <div class="field"><label>Parole chiave</label><input type="text" id="mdKeywords" value="${esc(m.keywords || '')}"></div>
      <div class="note-box">I metadati fanno parte del file PDF e possono essere rimossi definitivamente con <b>Pulisci metadati</b>. La <b>Pulizia profonda</b> elimina anche script, allegati, XMP, EXIF delle immagini, annotazioni nascoste e strutture orfane: il documento viene riscritto e riaperto.</div>
      <div class="mini-actions" style="margin-top:10px"><button class="btn ghost small" id="mdScrub">${icon('redact')}Pulisci metadati</button><button class="btn ghost small" id="mdDeep">${icon('redact')}Pulizia profonda</button></div>`,
    footer: `<button class="btn ghost" data-close>Annulla</button><button class="btn primary" id="mdRun">${icon('check')}Salva metadati</button>`,
    onMount: (root) => {
      $('#mdRun', root).addEventListener('click', async () => {
        try {
          const dest = await invoke('pick_save', { defaultName: baseName() + '_metadati.pdf', filters: ['pdf'] });
          if (!dest) return;
          setStatus('Aggiornamento metadati…', true);
          const res = await invoke('save_metadata', {
            dest,
            title: $('#mdTitle', root).value,
            author: $('#mdAuthor', root).value,
            subject: $('#mdSubject', root).value,
            keywords: $('#mdKeywords', root).value,
            creator: $('#mdCreator', root).value
          });
          closeModal();
          showResult('Metadati aggiornati', res.message, res.path, true);
        } catch (e) { toast(normalizeError(e), 'error'); }
        setStatus('Pronto');
      });
      $('#mdScrub', root).addEventListener('click', async () => {
        try {
          const dest = await invoke('pick_save', { defaultName: baseName() + '_pulito.pdf', filters: ['pdf'] });
          if (!dest) return;
          const res = await invoke('scrub_metadata_cmd', { dest });
          closeModal();
          showResult('Metadati rimossi', res.message, res.path, true);
        } catch (e) { toast(normalizeError(e), 'error'); }
      });
      $('#mdDeep', root).addEventListener('click', async () => {
        try {
          setStatus('Pulizia profonda…', true);
          const res = await invoke('sanitize_pdf_cmd');
          closeModal();
          if (res && res.pageCount) applyOpenInfo(res, []);
          toast((res && res.message) || 'Pulizia profonda completata.', 'info', 7000);
        } catch (e) { toast(normalizeError(e), 'error', 9000); }
        setStatus('Pronto');
      });
    }
  });
}

/* ---------------- collaboration ---------------- */
async function openSignRequest() {
  if (!state.doc) return toast('Apri prima un documento.', 'warn');
  if (!state.share) await startShare();
  tab('comments');
  if (!state.share) return;
  openModal({
    title: 'Richiedi firme', icon: 'signRequest',
    desc: 'Condividi un collegamento privato nella rete locale. I destinatari possono commentare e firmare dal browser.',
    body: `<div class="field"><label>Link di revisione</label><div class="share-link"><input id="reqUrl" readonly value="${esc(state.share.url)}"><button class="btn ghost small" id="reqCopy">Copia</button></div></div>
      <div class="mini-actions"><button class="btn primary" id="reqOpen">${icon('globe')}Apri nel browser</button><button class="btn ghost" id="reqStop">Termina sessione</button></div>
      <div class="note-box">Funziona tra dispositivi sulla stessa rete Wi-Fi/LAN. La prima volta Windows Firewall chiederà il permesso: scegli <b>Consenti</b> per la rete privata. Per ricevere firme da remoto serve una VPN o un server dedicato. Il collegamento include un token casuale.</div>`,
    footer: '<button class="btn ghost" data-close>Chiudi</button>',
    onMount: (root) => {
      $('#reqCopy', root).addEventListener('click', () => copyText(state.share?.url || ''));
      $('#reqOpen', root).addEventListener('click', () => { if (state.share) invoke('open_url', { url: state.share.url }).catch(e => toast(normalizeError(e), 'error')); });
      $('#reqStop', root).addEventListener('click', () => { stopShare(); closeModal(); });
    }
  });
}
async function startShare() {
  if (!state.doc) return;
  try {
    setStatus('Avvio del server di revisione locale…', true);
    const info = await invoke('start_share');
    state.share = info;
    updateShareUI();
    renderComments();
    clearInterval(state.shareTimer);
    state.shareTimer = setInterval(pollComments, 2500);
    tab('comments');
    toast('Link di revisione pronto. Aprilo da un dispositivo sulla stessa rete.', 'info', 5000);
    setStatus('Revisione locale attiva');
  } catch (e) { toast(normalizeError(e), 'error'); setStatus('Pronto'); }
}
function stopShare(silent = false) {
  clearInterval(state.shareTimer);
  if (state.share && !silent && HAS_TAURI) invoke('stop_share').catch(() => { });
  if (state.share && silent && HAS_TAURI) invoke('stop_share').catch(() => { });
  state.share = null;
  state.comments = [];
  updateShareUI();
  renderComments();
}
async function pollComments() {
  if (!state.share) return;
  try {
    state.comments = await invoke('get_share_comments');
    renderComments();
  } catch { }
}
function updateShareUI() {
  const live = !!state.share;
  const empty = $('#shareEmpty');
  const liveBox = $('#shareLive');
  const start = $('#startShareBtn');
  if (!empty || !liveBox) return;
  empty.hidden = live;
  liveBox.hidden = !live;
  if (start) start.style.display = live ? 'none' : '';
  if (live) {
    $('#shareUrl').value = state.share.url;
    $('#shareStats').textContent = `${state.comments.length} comment${state.comments.length === 1 ? 'o' : 'i'}`;
  }
}
function renderComments() {
  const list = $('#commentsList');
  if (!list) return;
  if (!state.share) {
    list.innerHTML = '';
    return;
  }
  const items = state.comments;
  list.innerHTML = items.length ? items.slice().reverse().map(c => {
    const initial = (c.author || 'O').trim().charAt(0).toUpperCase();
    const sig = c.kind === 'signature' || c.signature;
    return `<div class="comment-item">
      <header><span class="avatar">${esc(initial)}</span><b>${esc(c.author || 'Ospite')}</b><span class="pg-tag">p. ${(c.page || 0) + 1}</span><span class="time">${esc(c.createdAt || '')}</span></header>
      ${c.text ? `<p>${esc(c.text)}</p>` : ''}
      ${sig && c.signature ? `<img class="sig-thumb" src="${c.signature}" alt="Firma di ${esc(c.author || '')}">` : ''}
      ${sig && c.signature ? `<button class="btn ghost small apply-sig" data-sig-apply="${esc(c.id)}">${icon('signature')}Applica firma</button>` : ''}
    </div>`;
  }).join('') : '<div class="empty-list">In attesa di commenti e firme…</div>';
  if (state.share) $('#shareStats').textContent = `${items.length} comment${items.length === 1 ? 'o' : 'i'}`;
  $$('[data-sig-apply]', list).forEach(btn => btn.addEventListener('click', () => {
    const c = state.comments.find(x => x.id === btn.dataset.sigApply);
    if (!c) return;
    if (state.page !== c.page) gotoPage(c.page);
    addAnnotation('signature', {
      x: clamp(c.x - .12, .01, .98 - .24), y: clamp(c.y - .042, .01, .98 - .085), w: .24, h: .085,
      dataUrl: c.signature, author: c.author || 'Firma', opacity: 1
    }, { page: c.page });
    toast(`Firma di ${c.author || 'Ospite'} applicata alla pagina ${c.page + 1}.`);
  }));
}

/* ---------------- command palette ---------------- */
function openAllTools() {
  const extras = [
    { title: 'Azioni', items: [
      { id: 'open', label: 'Apri PDF', icon: 'folder' },
      { id: 'save', label: 'Salva PDF', icon: 'save' },
      { id: 'voice', label: 'Leggi ad alta voce', icon: 'volume' },
      { id: 'signrequest', label: 'Richiedi firme', icon: 'signRequest' }
    ]},
    { title: 'Sistema', items: [
      { id: 'updates', label: 'Aggiornamenti', icon: 'download' }
    ]}
  ];
  const groups = [...NAV, ...extras];
  openModal({
    title: 'Tutti gli strumenti', wide: true, icon: 'grid',
    desc: 'Ogni funzione di Open PDF in un unico punto. Tutto viene eseguito in locale.',
    body: `<div class="all-tools-grid">${groups.map(g => `<div class="all-tools-group"><h4>${esc(g.title)}</h4>${g.items.map(it => `<button data-command="${it.id}">${icon(it.icon)}<span>${esc(it.label)}</span></button>`).join('')}</div>`).join('')}</div>`,
    footer: '<button class="btn ghost" data-close>Chiudi</button>',
    onMount: (root) => {
      $$('[data-command]', root).forEach(btn => btn.addEventListener('click', () => {
        const cmd = btn.dataset.command;
        closeModal();
        if (cmd === 'open') return openDialog();
        if (cmd === 'save') return openSaveModal();
        if (cmd === 'voice') return toggleVoice();
        if (cmd === 'updates') return openUpdater();
        handleNav(cmd);
      }));
    }
  });
}
