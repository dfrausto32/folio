'use strict';
/* Folio UI. Talks to the Rust engine through Tauri commands (see src-tauri/src/main.rs).
   Page geometry is always in PDF points, origin top-left of the page as displayed. */

const T = window.__TAURI__;
const invoke = T.core.invoke;
const dialog = T.dialog;
const appWindow = T.window.getCurrentWindow();

const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const PDF_FILTER = [{ name: 'PDF documents', extensions: ['pdf'] }];
const CSS_PX_PER_PT = 96 / 72;
const THUMB_W = 128; // css px

const S = {
  doc: null,            // DocInfo from the engine
  zoom: 1, fit: true,
  tool: 'select',
  color: '#ffd21f', lineW: 2, textSize: 14,
  colors: { highlight: '#ffd21f', box: '#e5252a', pen: '#1c4fd8', text: '#111111' },
  cur: 0,               // current page (0-based)
  selected: new Set(),  // selected thumbnails
  anchor: 0,            // shift-click anchor
  pageEls: [], thumbEls: [],
  gen: 0,               // bumps on every rebuild so stale async work can be ignored
  hits: [], hitIdx: -1, query: '',
  confirmedOverwrite: false,
};

// ───────────────────────────── small helpers ────────────────────────────────

const basename = (p) => p.split(/[\\/]/).pop();
const dirname = (p) => { const i = Math.max(p.lastIndexOf('/'), p.lastIndexOf('\\')); return i < 0 ? '' : p.slice(0, i); };
const stem = (p) => basename(p).replace(/\.pdf$/i, '');
const joinPath = (dir, name) => (dir ? dir + (dir.includes('\\') ? '\\' : '/') : '') + name;
const withPdfExt = (p) => (/\.pdf$/i.test(p) ? p : p + '.pdf');
const clamp = (v, a, b) => Math.min(b, Math.max(a, v));
const errMsg = (e) => (typeof e === 'string' ? e : (e && e.message) || JSON.stringify(e));
const hexToRgb = (h) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));

function el(tag, props = {}, children = []) {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'class') n.className = v;
    else if (k === 'text') n.textContent = v;
    else if (k.startsWith('on')) n.addEventListener(k.slice(2), v);
    else n.setAttribute(k, v);
  }
  for (const c of [].concat(children)) if (c != null) n.append(c);
  return n;
}
const svgEl = (tag, attrs = {}) => {
  const n = document.createElementNS('http://www.w3.org/2000/svg', tag);
  for (const [k, v] of Object.entries(attrs)) n.setAttribute(k, v);
  return n;
};

let toastTimer = 0;
function toast(msg, isError = false, ms = 3500) {
  const t = $('#toast');
  t.textContent = msg;
  t.className = isError ? 'error' : '';
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), isError ? Math.max(ms, 6000) : ms);
}
const setStatus = (s) => ($('#status-text').textContent = s);

// ───────────────────────────── modal dialogs ────────────────────────────────

let modalOpen = false;
/** Show a modal. actions: [{label, value, primary}]. Resolves with the chosen value (null on Esc). */
function modal({ title, body, actions }) {
  return new Promise((resolve) => {
    modalOpen = true;
    $('#modal-title').textContent = title;
    const bodyEl = $('#modal-body');
    bodyEl.replaceChildren(...[].concat(body || []));
    const bar = $('#modal-actions');
    bar.replaceChildren();
    const done = (v) => {
      modalOpen = false;
      $('#modal').hidden = true;
      document.removeEventListener('keydown', onKey, true);
      resolve(v);
    };
    const onKey = (e) => {
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); done(null); }
      else if (e.key === 'Enter' && e.target.tagName !== 'TEXTAREA' && e.target.tagName !== 'BUTTON') {
        const p = actions.find((a) => a.primary);
        if (p) { e.preventDefault(); e.stopPropagation(); done(p.getValue ? p.getValue() : p.value); }
      }
    };
    document.addEventListener('keydown', onKey, true);
    for (const a of actions) {
      bar.append(el('button', {
        class: a.primary ? 'primary' : '', text: a.label,
        onclick: () => done(a.getValue ? a.getValue() : a.value),
      }));
    }
    $('#modal').hidden = false;
    const focusable = bodyEl.querySelector('input, select, textarea') || bar.querySelector('.primary') || bar.lastChild;
    if (focusable) { focusable.focus(); if (focusable.select) focusable.select(); }
  });
}

const confirmDialog = (title, text, okLabel = 'OK', danger = false) =>
  modal({
    title, body: el('p', { text }),
    actions: [{ label: 'Cancel', value: false }, { label: okLabel, value: true, primary: true }],
  }).then((v) => v === true);

function askText({ title, label, value = '', type = 'text', okLabel = 'OK' }) {
  const input = el('input', { type, value });
  return modal({
    title, body: [label ? el('p', { text: label }) : null, input],
    actions: [{ label: 'Cancel', value: null }, { label: okLabel, primary: true, getValue: () => input.value }],
  });
}

// ───────────────────────────── geometry / layout ────────────────────────────

const pxPerPt = () => S.zoom * CSS_PX_PER_PT;
const dpr = () => Math.min(window.devicePixelRatio || 1, 2.5);
const maxPageW = () => Math.max(...S.doc.pages.map((p) => p.width));

function fitZoom() {
  const avail = $('#viewer').clientWidth - 64;
  return clamp(avail / (maxPageW() * CSS_PX_PER_PT), 0.1, 6);
}

function sizePageEl(i) {
  const p = S.doc.pages[i], k = pxPerPt();
  const box = S.pageEls[i].box;
  box.style.width = p.width * k + 'px';
  box.style.height = p.height * k + 'px';
}

// ───────────────────────────── render queue ─────────────────────────────────

const queue = [];
let active = 0;
function schedule(job) {
  queue.push(job);
  pump();
}
function pump() {
  while (active < 2 && queue.length) {
    const job = queue.shift();
    active++;
    job().finally(() => { active--; pump(); });
  }
}

function setImg(img, buf) {
  const url = URL.createObjectURL(new Blob([buf], { type: 'image/png' }));
  const old = img.dataset.url;
  img.onload = () => { if (old) URL.revokeObjectURL(old); };
  img.dataset.url = url;
  img.src = url;
}
function dropImg(img) {
  if (img.dataset.url) URL.revokeObjectURL(img.dataset.url);
  delete img.dataset.url;
  img.removeAttribute('src');
}

/** Render page i (main view or thumbnail) if it is stale, wrong size, or `force`d. */
function ensureImage(rec, i, thumb, force = false) {
  const wantW = Math.round((thumb ? THUMB_W : S.doc.pages[i].width * pxPerPt()) * dpr());
  const key = wantW;
  if (!force && !rec.stale && rec.imgW === wantW && rec.img.dataset.url) return Promise.resolve();
  if (rec.pending && rec.pendingKey === key && !force) return rec.pending;
  const gen = S.gen, id = S.doc.id;
  rec.pendingKey = key;
  rec.pending = new Promise((resolve) => {
    schedule(async () => {
      try {
        if (gen !== S.gen || (!force && !rec.visible)) return;
        const buf = await invoke('render_page', { id, page: i, width: clamp(wantW, 16, 6000) });
        if (gen === S.gen) {
          setImg(rec.img, buf);
          rec.imgW = wantW;
          rec.stale = false;
        }
      } catch (e) {
        console.error('render failed', e);
      } finally {
        rec.pending = null;
        rec.pendingKey = null;
        resolve();
      }
    });
  });
  return rec.pending;
}

const mainObserver = new IntersectionObserver((entries) => {
  for (const en of entries) {
    const rec = en.target._rec; if (!rec) continue;
    rec.visible = en.isIntersecting;
    if (en.isIntersecting) ensureImage(rec, rec.i, false);
    else { dropImg(rec.img); rec.imgW = 0; }
  }
}, { root: $('#viewer'), rootMargin: '1400px 0px' });

const thumbObserver = new IntersectionObserver((entries) => {
  for (const en of entries) {
    const rec = en.target._rec; if (!rec) continue;
    rec.visible = en.isIntersecting;
    if (en.isIntersecting) ensureImage(rec, rec.i, true);
    else { dropImg(rec.img); rec.imgW = 0; }
  }
}, { root: $('#sidebar'), rootMargin: '600px 0px' });

// ───────────────────────────── building the views ───────────────────────────

function rebuild() {
  S.gen++;
  mainObserver.disconnect();
  thumbObserver.disconnect();
  queue.length = 0;
  for (const r of [...S.pageEls, ...S.thumbEls]) if (r) dropImg(r.img);
  S.pageEls = []; S.thumbEls = [];
  const pages = $('#pages'), thumbs = $('#thumbs');
  pages.replaceChildren(); thumbs.replaceChildren();
  S.hits = []; S.hitIdx = -1; updateSearchCount();
  $('#welcome').hidden = !!S.doc;
  if (!S.doc) { syncUi(); return; }

  if (S.fit) S.zoom = fitZoom();
  S.cur = clamp(S.cur, 0, S.doc.page_count - 1);
  for (const s of [...S.selected]) if (s >= S.doc.page_count) S.selected.delete(s);

  S.doc.pages.forEach((p, i) => {
    // main page
    const img = el('img', { class: 'bg', alt: '', draggable: 'false' });
    const svg = svgEl('svg', { class: 'ov', viewBox: `0 0 ${p.width} ${p.height}` });
    svg.append(svgEl('g', { class: 'hits' }), svgEl('g', { class: 'draft' }));
    const box = el('div', { class: 'page', 'data-i': i }, [img, svg]);
    const rec = { i, box, img, svg, visible: false, stale: true, imgW: 0 };
    box._rec = rec;
    S.pageEls.push(rec);
    pages.append(box);
    sizePageEl(i);
    mainObserver.observe(box);

    // thumbnail
    const timg = el('img', { alt: '', draggable: 'false' });
    const tbox = el('div', { class: 'box' }, [timg]);
    tbox.style.width = THUMB_W + 'px';
    tbox.style.height = (THUMB_W * p.height) / p.width + 'px';
    const num = el('span', { class: 'num', text: String(i + 1) });
    const th = el('div', { class: 'thumb', 'data-i': i }, [tbox, num]);
    const trec = { i, box: th, img: timg, visible: false, stale: true, imgW: 0 };
    th._rec = trec;
    S.thumbEls.push(trec);
    thumbs.append(th);
    thumbObserver.observe(th);
  });

  syncUi();
  goToPage(S.cur, false);
  updateThumbStates();
}

function updateThumbStates() {
  S.thumbEls.forEach((t, i) => {
    t.box.classList.toggle('sel', S.selected.has(i));
    t.box.classList.toggle('cur', i === S.cur);
  });
}

function syncUi() {
  const has = !!S.doc;
  for (const id of ['btn-save', 'btn-saveas', 'btn-zoom-out', 'btn-zoom-in', 'zoom-label', 'btn-pages', 'page-input', 'search-input'])
    $('#' + id).disabled = !has;
  $('#btn-undo').disabled = !(has && S.doc.can_undo);
  $('#btn-prev').disabled = $('#btn-next').disabled = !(has && S.hits.length);
  $('#page-total').textContent = has ? `/ ${S.doc.page_count}` : '/ 0';
  $('#page-input').max = has ? S.doc.page_count : 1;
  $('#page-input').value = has ? S.cur + 1 : 1;
  $('#zoom-label').textContent = Math.round(S.zoom * 100) + '%';
  updateTitle();
  setStatus(has ? `${basename(S.doc.path || 'Untitled.pdf')}  ·  ${S.doc.page_count} page${S.doc.page_count === 1 ? '' : 's'}` : 'Ready');
}

function updateTitle() {
  const name = S.doc ? basename(S.doc.path || 'Untitled.pdf') : null;
  const title = name ? `${S.doc.dirty ? '● ' : ''}${name} - Folio` : 'Folio';
  document.title = title;
  appWindow.setTitle(title).catch(() => {});
}

/** Apply a fresh DocInfo. mode: 'rebuild' or {pages:[...]} (content of those pages changed). */
async function applyInfo(info, mode) {
  S.doc = info;
  if (mode === 'rebuild') { rebuild(); return; }
  syncUi();
  const jobs = [];
  for (const i of mode.pages) {
    const r = S.pageEls[i], t = S.thumbEls[i];
    if (r) { r.stale = true; jobs.push(ensureImage(r, i, false, true)); }
    if (t) { t.stale = true; if (t.visible) jobs.push(ensureImage(t, i, true, true)); }
  }
  await Promise.all(jobs);
}

// ───────────────────────────── navigation & zoom ────────────────────────────

function goToPage(i, smooth = true) {
  if (!S.doc) return;
  i = clamp(i, 0, S.doc.page_count - 1);
  S.cur = i;
  const v = $('#viewer'), box = S.pageEls[i].box;
  v.scrollTo({ top: box.offsetTop - 14, behavior: smooth ? 'smooth' : 'instant' });
  $('#page-input').value = i + 1;
  updateThumbStates();
  const t = S.thumbEls[i];
  if (t) t.box.scrollIntoView({ block: 'nearest' });
}

let scrollRaf = 0;
$('#viewer').addEventListener('scroll', () => {
  if (scrollRaf) return;
  scrollRaf = requestAnimationFrame(() => {
    scrollRaf = 0;
    if (!S.doc) return;
    const v = $('#viewer');
    const probe = v.scrollTop + v.clientHeight * 0.35;
    let idx = 0;
    for (let i = 0; i < S.pageEls.length; i++) {
      const b = S.pageEls[i].box;
      if (b.offsetTop + b.offsetHeight > probe) { idx = i; break; }
      idx = i;
    }
    if (idx !== S.cur) {
      S.cur = idx;
      $('#page-input').value = idx + 1;
      updateThumbStates();
      const t = S.thumbEls[idx];
      if (t) t.box.scrollIntoView({ block: 'nearest' });
    }
  });
});

let zoomTimer = 0;
function setZoom(z, fit = false) {
  if (!S.doc) return;
  const v = $('#viewer');
  const ratio = v.scrollHeight ? v.scrollTop / v.scrollHeight : 0;
  S.fit = fit;
  S.zoom = clamp(z, 0.1, 6);
  S.doc.pages.forEach((_, i) => sizePageEl(i));
  v.scrollTop = ratio * v.scrollHeight;
  $('#zoom-label').textContent = Math.round(S.zoom * 100) + '%';
  clearTimeout(zoomTimer);
  zoomTimer = setTimeout(() => S.pageEls.forEach((r, i) => r.visible && ensureImage(r, i, false)), 140);
}
const zoomBy = (f) => setZoom(S.zoom * f);

window.addEventListener('resize', () => { if (S.doc && S.fit) setZoom(fitZoom(), true); });
$('#viewer').addEventListener('wheel', (e) => {
  if (!(e.ctrlKey || e.metaKey) || !S.doc) return;
  e.preventDefault();
  zoomBy(e.deltaY < 0 ? 1.1 : 1 / 1.1);
}, { passive: false });

// ───────────────────────────── opening / closing ────────────────────────────

async function confirmDiscard() {
  if (!S.doc || !S.doc.dirty) return true;
  const r = await modal({
    title: 'Unsaved changes',
    body: el('p', { text: `"${basename(S.doc.path || 'Untitled.pdf')}" has changes that have not been saved.` }),
    actions: [
      { label: 'Cancel', value: 'cancel' },
      { label: "Don't save", value: 'discard' },
      { label: 'Save', value: 'save', primary: true },
    ],
  });
  if (r === 'save') return saveDoc();
  return r === 'discard';
}

async function openPath(path) {
  let password = null;
  for (;;) {
    try {
      const info = await invoke('open_pdf', { path, password });
      if (S.doc) invoke('close_doc', { id: S.doc.id }).catch(() => {});
      S.doc = info; S.cur = 0; S.selected.clear(); S.confirmedOverwrite = false;
      S.fit = true;
      rebuild();
      return true;
    } catch (e) {
      const m = errMsg(e);
      if (m === 'PASSWORD_REQUIRED' || m === 'PASSWORD_INCORRECT') {
        const pw = await askText({
          title: 'Password required',
          label: m === 'PASSWORD_INCORRECT' ? 'That password was not correct. Try again:' : `"${basename(path)}" is password protected.`,
          type: 'password', okLabel: 'Open',
        });
        if (pw == null) return false;
        password = pw;
        continue;
      }
      toast(m, true);
      return false;
    }
  }
}

async function openDialog() {
  if (modalOpen) return;
  if (!(await confirmDiscard())) return;
  const p = await dialog.open({ multiple: false, filters: PDF_FILTER });
  if (p) openPath(Array.isArray(p) ? p[0] : p);
}

async function newDoc() {
  if (!(await confirmDiscard())) return;
  try {
    const info = await invoke('new_blank');
    if (S.doc) invoke('close_doc', { id: S.doc.id }).catch(() => {});
    S.doc = info; S.cur = 0; S.selected.clear(); S.fit = true; rebuild();
  } catch (e) { toast(errMsg(e), true); }
}

// ───────────────────────────── saving ───────────────────────────────────────

async function saveAs() {
  if (!S.doc) return false;
  closeTextEdit();
  await chain;
  const def = S.doc.path
    ? joinPath(dirname(S.doc.path), stem(S.doc.path) + ' (edited).pdf')
    : 'Untitled.pdf';
  let p = await dialog.save({ defaultPath: def, filters: PDF_FILTER });
  if (!p) return false;
  p = withPdfExt(p);
  try {
    S.doc = await invoke('save_pdf', { id: S.doc.id, path: p });
    S.confirmedOverwrite = true;
    syncUi();
    toast('Saved ' + basename(p));
    return true;
  } catch (e) { toast(errMsg(e), true); return false; }
}

async function saveDoc() {
  if (!S.doc) return false;
  closeTextEdit();
  await chain;
  if (!S.doc.path) return saveAs();
  if (!S.confirmedOverwrite) {
    const r = await modal({
      title: 'Overwrite the original?',
      body: el('p', { text: 'Saving replaces the original file. Marks you added become part of the pages and cannot be removed afterwards. Save a copy if you want to keep the original untouched.' }),
      actions: [
        { label: 'Cancel', value: 'cancel' },
        { label: 'Save a copy…', value: 'copy' },
        { label: 'Overwrite', value: 'overwrite', primary: true },
      ],
    });
    if (r === 'copy') return saveAs();
    if (r !== 'overwrite') return false;
    S.confirmedOverwrite = true;
  }
  try {
    S.doc = await invoke('save_pdf', { id: S.doc.id, path: S.doc.path });
    syncUi();
    toast('Saved');
    return true;
  } catch (e) { toast(errMsg(e), true); return false; }
}

// ───────────────────────────── undo / edits ─────────────────────────────────

/** Every engine call that changes the document goes through this chain so they run strictly in order. */
let chain = Promise.resolve();
function serial(fn) {
  const next = chain.then(fn, fn);
  chain = next.catch(() => {});
  return next;
}

function runEdit(fn, mode = 'rebuild') {
  if (!S.doc) return Promise.resolve();
  return serial(async () => {
    try {
      const info = await fn();
      await applyInfo(info, mode);
      return info;
    } catch (e) {
      toast(errMsg(e), true);
    }
  });
}

const doUndo = () => S.doc && S.doc.can_undo && runEdit(() => invoke('undo', { id: S.doc.id }));

function targetPages() {
  return S.selected.size ? [...S.selected].sort((a, b) => a - b) : [S.cur];
}

async function pageAction(act) {
  if (!S.doc) return;
  const pages = targetPages();
  const last = pages[pages.length - 1];
  const id = S.doc.id;
  switch (act) {
    case 'rotate-cw': return runEdit(() => invoke('rotate_pages', { id, pages, degrees: 90 }));
    case 'rotate-ccw': return runEdit(() => invoke('rotate_pages', { id, pages, degrees: -90 }));
    case 'delete': {
      if (pages.length >= S.doc.page_count) return toast('A PDF must keep at least one page.', true);
      const ok = await confirmDialog('Delete pages', `Delete ${pages.length === 1 ? 'page ' + (pages[0] + 1) : pages.length + ' pages'}? You can undo this.`, 'Delete');
      if (!ok) return;
      S.selected.clear();
      return runEdit(() => invoke('delete_pages', { id, pages }));
    }
    case 'blank': return runEdit(() => invoke('insert_blank', { id, at: last + 1 }));
    case 'insert-pdf': {
      const p = await dialog.open({ multiple: false, filters: PDF_FILTER });
      if (!p) return;
      return insertPdf(Array.isArray(p) ? p[0] : p, last + 1);
    }
    case 'extract': {
      let out = await dialog.save({ defaultPath: stem(S.doc.path || 'Untitled') + ' (extract).pdf', filters: PDF_FILTER });
      if (!out) return;
      out = withPdfExt(out);
      try { await invoke('extract_pages', { id, pages, outPath: out }); toast(`Saved ${pages.length} page${pages.length === 1 ? '' : 's'} to ${basename(out)}`); }
      catch (e) { toast(errMsg(e), true); }
      return;
    }
    case 'split': {
      const n = await askText({ title: 'Split into several PDFs', label: 'Pages per file:', value: '1', type: 'number', okLabel: 'Choose folder…' });
      const every = parseInt(n, 10);
      if (!every || every < 1) return;
      const dir = await dialog.open({ directory: true, multiple: false });
      if (!dir) return;
      try {
        const files = await invoke('split_every', { id, every, outDir: dir, stem: stem(S.doc.path || 'Untitled') });
        toast(`Created ${files.length} file${files.length === 1 ? '' : 's'} in ${basename(dir)}`);
      } catch (e) { toast(errMsg(e), true); }
      return;
    }
    case 'copy-text': {
      try {
        const text = await invoke('page_text', { id, page: S.cur });
        await navigator.clipboard.writeText(text);
        toast(text.trim() ? `Copied text of page ${S.cur + 1}` : 'That page has no text (it may be a scan).');
      } catch (e) { toast(errMsg(e), true); }
      return;
    }
    case 'print': return printDoc();
  }
}

async function insertPdf(path, at) {
  let password = null;
  for (;;) {
    try {
      const info = await invoke('insert_pdf', { id: S.doc.id, other: path, password, at });
      await applyInfo(info, 'rebuild');
      return;
    } catch (e) {
      const m = errMsg(e);
      if (m === 'PASSWORD_REQUIRED' || m === 'PASSWORD_INCORRECT') {
        const pw = await askText({ title: 'Password required', label: `"${basename(path)}" is password protected.`, type: 'password', okLabel: 'Insert' });
        if (pw == null) return;
        password = pw; continue;
      }
      toast(m, true); return;
    }
  }
}

// ───────────────────────────── merge ────────────────────────────────────────

async function mergeDialog() {
  const first = await dialog.open({ multiple: true, filters: PDF_FILTER });
  if (!first) return;
  let files = [].concat(first);
  const listEl = el('div', { class: 'file-list' });
  const draw = () => {
    listEl.replaceChildren(...files.map((f, i) => el('div', { class: 'file-row' }, [
      el('span', { class: 'name', text: `${i + 1}. ${basename(f)}`, title: f }),
      el('button', { text: '↑', title: 'Move up', onclick: () => { if (i > 0) { [files[i - 1], files[i]] = [files[i], files[i - 1]]; draw(); } } }),
      el('button', { text: '↓', title: 'Move down', onclick: () => { if (i < files.length - 1) { [files[i + 1], files[i]] = [files[i], files[i + 1]]; draw(); } } }),
      el('button', { text: '✕', title: 'Remove', onclick: () => { files.splice(i, 1); draw(); } }),
    ])));
  };
  draw();
  const addBtn = el('button', {
    text: 'Add more…',
    onclick: async () => { const more = await dialog.open({ multiple: true, filters: PDF_FILTER }); if (more) { files = files.concat(more); draw(); } },
  });
  const r = await modal({
    title: 'Merge PDFs',
    body: [el('p', { text: 'Files are combined top to bottom.' }), listEl, el('div', { class: 'row' }, [addBtn])],
    actions: [{ label: 'Cancel', value: false }, { label: 'Merge…', value: true, primary: true }],
  });
  if (!r || !files.length) return;
  let out = await dialog.save({ defaultPath: 'Merged.pdf', filters: PDF_FILTER });
  if (!out) return;
  out = withPdfExt(out);
  try {
    setStatus('Merging…');
    const n = await invoke('merge_pdfs', { inputs: files, outPath: out });
    syncUi();
    const open = await confirmDialog('Merged', `Created ${basename(out)} with ${n} pages. Open it now?`, 'Open');
    if (open && (await confirmDiscard())) openPath(out);
  } catch (e) {
    syncUi();
    const m = errMsg(e);
    toast(m === 'PASSWORD_REQUIRED' ? 'One of the files is password protected, and merging protected files is not supported yet.' : m, true);
  }
}

// ───────────────────────────── printing ─────────────────────────────────────

async function printDoc() {
  const root = $('#print-root');
  const urls = [];
  try {
    root.replaceChildren();
    for (let i = 0; i < S.doc.page_count; i++) {
      setStatus(`Preparing page ${i + 1} of ${S.doc.page_count} for printing…`);
      const buf = await invoke('render_page', { id: S.doc.id, page: i, width: 1654 }); // ~150 dpi on A4
      const url = URL.createObjectURL(new Blob([buf], { type: 'image/png' }));
      urls.push(url);
      const img = el('img', { src: url });
      root.append(img);
      await img.decode().catch(() => {});
    }
    syncUi();
    window.print();
  } catch (e) {
    toast('Printing is not available here: ' + errMsg(e), true);
  } finally {
    setTimeout(() => { root.replaceChildren(); urls.forEach((u) => URL.revokeObjectURL(u)); syncUi(); }, 4000);
  }
}

// ───────────────────────────── search ───────────────────────────────────────

function updateSearchCount() {
  const c = $('#search-count');
  if (!S.query) c.textContent = '';
  else if (!S.hits.length) c.textContent = 'No matches';
  else c.textContent = `${S.hitIdx + 1} of ${S.hits.length}`;
  const has = S.hits.length > 0;
  $('#btn-prev').disabled = $('#btn-next').disabled = !has;
}

function drawHits() {
  for (const r of S.pageEls) r.svg.querySelector('.hits').replaceChildren();
  S.hits.forEach((h, n) => {
    const g = S.pageEls[h.page]?.svg.querySelector('.hits');
    if (!g) return;
    for (const r of h.rects) {
      g.append(svgEl('rect', { class: 'hit' + (n === S.hitIdx ? ' current' : ''), x: r.x, y: r.y, width: r.w, height: r.h, rx: 1.5 }));
    }
  });
}

async function runSearch() {
  if (!S.doc) return;
  S.query = $('#search-input').value.trim();
  if (!S.query) { S.hits = []; S.hitIdx = -1; drawHits(); updateSearchCount(); return; }
  try {
    setStatus('Searching…');
    S.hits = await invoke('search', { id: S.doc.id, query: S.query, matchCase: $('#match-case').checked });
    S.hitIdx = S.hits.length ? Math.max(0, S.hits.findIndex((h) => h.page >= S.cur)) : -1;
    syncUi();
    drawHits();
    updateSearchCount();
    if (S.hitIdx >= 0) revealHit();
  } catch (e) { toast(errMsg(e), true); }
}

function revealHit() {
  const h = S.hits[S.hitIdx];
  if (!h) return;
  const box = S.pageEls[h.page].box, v = $('#viewer');
  const y = h.rects[0].y * pxPerPt();
  v.scrollTo({ top: box.offsetTop + y - v.clientHeight / 3, behavior: 'smooth' });
  S.cur = h.page;
  $('#page-input').value = h.page + 1;
  updateThumbStates();
  drawHits();
  updateSearchCount();
}

function stepHit(d) {
  if (!S.hits.length) return;
  S.hitIdx = (S.hitIdx + d + S.hits.length) % S.hits.length;
  revealHit();
}

// ───────────────────────────── annotation tools ─────────────────────────────

function setTool(t) {
  closeTextEdit();
  S.tool = t;
  document.body.dataset.tool = t;
  $$('.tool').forEach((b) => b.classList.toggle('active', b.dataset.tool === t));
  const sel = $('#size');
  sel.replaceChildren();
  const opts = t === 'text' ? [10, 12, 14, 18, 24, 36] : [1, 2, 3, 5, 8];
  const cur = t === 'text' ? S.textSize : S.lineW;
  for (const o of opts) sel.append(el('option', { value: o, text: t === 'text' ? o + ' pt text' : o + ' pt line', ...(o === cur ? { selected: '' } : {}) }));
  sel.disabled = t === 'select' || t === 'highlight';
  if (S.colors[t]) $('#color').value = S.colors[t];
  S.color = $('#color').value;
}

function pagePoint(rec, e) {
  const r = rec.svg.getBoundingClientRect();
  const p = S.doc.pages[rec.i];
  return [clamp(((e.clientX - r.left) / r.width) * p.width, 0, p.width), clamp(((e.clientY - r.top) / r.height) * p.height, 0, p.height)];
}

let drawing = null;
let pendingText = null;
$('#pages').addEventListener('pointerdown', (e) => {
  if (!S.doc || S.tool === 'select' || e.button !== 0) return;
  const boxEl = e.target.closest('.page');
  if (!boxEl || e.target.closest('.text-edit')) { return; }
  e.preventDefault();
  closeTextEdit();
  const rec = boxEl._rec;
  const pt = pagePoint(rec, e);
  if (S.tool === 'text') { pendingText = { rec, pt }; return; }
  const color = S.color;
  let shape;
  if (S.tool === 'highlight') shape = svgEl('rect', { fill: color, 'fill-opacity': 0.4 });
  else if (S.tool === 'box') shape = svgEl('rect', { fill: 'none', stroke: color, 'stroke-width': S.lineW });
  else shape = svgEl('polyline', { fill: 'none', stroke: color, 'stroke-width': S.lineW, 'stroke-linecap': 'round', 'stroke-linejoin': 'round' });
  rec.svg.querySelector('.draft').append(shape);
  drawing = { rec, start: pt, pts: [pt], shape, tool: S.tool, color, lineW: S.lineW };
  updateShape();
  boxEl.setPointerCapture(e.pointerId);
});

function updateShape() {
  const d = drawing, [x0, y0] = d.start, [x1, y1] = d.pts[d.pts.length - 1];
  if (d.tool === 'pen') d.shape.setAttribute('points', d.pts.map((p) => p.join(',')).join(' '));
  else {
    d.shape.setAttribute('x', Math.min(x0, x1)); d.shape.setAttribute('y', Math.min(y0, y1));
    d.shape.setAttribute('width', Math.abs(x1 - x0)); d.shape.setAttribute('height', Math.abs(y1 - y0));
  }
}

$('#pages').addEventListener('pointermove', (e) => {
  if (!drawing) return;
  // The browser may merge several moves into one event; use all of them so fast strokes stay smooth.
  const events = (e.getCoalescedEvents && e.getCoalescedEvents()) || [];
  for (const ev of events.length ? events : [e]) {
    const pt = pagePoint(drawing.rec, ev);
    if (drawing.tool === 'pen') {
      const last = drawing.pts[drawing.pts.length - 1];
      if (Math.hypot(pt[0] - last[0], pt[1] - last[1]) < 0.8) continue;
      drawing.pts.push(pt);
    } else drawing.pts = [pt];
  }
  updateShape();
});

// Clicking anywhere outside an open text box finishes it.
document.addEventListener('pointerdown', (e) => {
  if (textEdit && !e.target.closest('.text-edit')) closeTextEdit();
}, true);

async function finishDraw(e) {
  if (pendingText) { const { rec, pt } = pendingText; pendingText = null; if (e.type === 'pointerup') startText(rec, pt); return; }
  if (!drawing) return;
  const d = drawing; drawing = null;
  try { d.rec.box.releasePointerCapture(e.pointerId); } catch (_) { /* already released */ }
  const [x0, y0] = d.start, [x1, y1] = d.pts[d.pts.length - 1];
  const rect = { x: Math.min(x0, x1), y: Math.min(y0, y1), w: Math.abs(x1 - x0), h: Math.abs(y1 - y0) };
  const rgb = hexToRgb(d.color);
  let annot = null;
  if (d.tool === 'highlight' && rect.w > 2 && rect.h > 2) annot = { kind: 'highlight', page: d.rec.i, rects: [rect], color: rgb, opacity: 0.4 };
  else if (d.tool === 'box' && rect.w > 2 && rect.h > 2) annot = { kind: 'rect', page: d.rec.i, rect, color: rgb, width: d.lineW };
  else if (d.tool === 'pen' && d.pts.length >= 2) annot = { kind: 'ink', page: d.rec.i, points: d.pts, color: rgb, width: d.lineW };
  if (!annot) { d.shape.remove(); return; }
  await commitAnnot(annot, d.rec.i);
  d.shape.remove();
}
$('#pages').addEventListener('pointerup', finishDraw);
$('#pages').addEventListener('pointercancel', finishDraw);

function commitAnnot(annot, pageIdx) {
  return runEdit(() => invoke('add_annotations', { id: S.doc.id, annots: [annot] }), { pages: [pageIdx] });
}

let textEdit = null;
function startText(rec, pt) {
  const k = pxPerPt(), size = S.textSize;
  const ta = el('textarea', { class: 'text-edit', rows: '1', spellcheck: 'false' });
  ta.style.left = pt[0] * k + 'px';
  ta.style.top = pt[1] * k + 'px';
  ta.style.fontSize = size * k + 'px';
  ta.style.color = S.color;
  const grow = () => { ta.style.height = 'auto'; ta.style.height = ta.scrollHeight + 'px'; ta.style.width = Math.max(80, ta.scrollWidth) + 'px'; };
  ta.addEventListener('input', grow);
  ta.addEventListener('keydown', (ev) => {
    if (ev.key === 'Escape') { ev.preventDefault(); closeTextEdit(false); }
    else if ((ev.ctrlKey || ev.metaKey) && ev.key === 'Enter') { ev.preventDefault(); closeTextEdit(true); }
    ev.stopPropagation();
  });
  rec.box.append(ta);
  textEdit = { ta, rec, pt, size, color: S.color };
  grow();
  setTimeout(() => ta.focus(), 0); // after the click's default focus handling has finished
}

function closeTextEdit(commit = true) {
  if (!textEdit) return;
  const { ta, rec, pt, size, color } = textEdit;
  textEdit = null;
  const text = ta.value.replace(/\s+$/, '');
  ta.remove();
  if (commit && text) commitAnnot({ kind: 'text', page: rec.i, x: pt[0], y: pt[1], text, size, color: hexToRgb(color) }, rec.i);
}

// ───────────────────────────── thumbnails: select + drag to reorder ─────────

const thumbsEl = $('#thumbs');
let tdrag = null;

thumbsEl.addEventListener('pointerdown', (e) => {
  const th = e.target.closest('.thumb');
  if (!th || e.button !== 0) return;
  tdrag = { i: +th.dataset.i, x: e.clientX, y: e.clientY, moving: false, drop: 0, ctrl: e.ctrlKey || e.metaKey, shift: e.shiftKey };
  thumbsEl.setPointerCapture(e.pointerId);
});

thumbsEl.addEventListener('pointermove', (e) => {
  if (!tdrag) return;
  if (!tdrag.moving && Math.hypot(e.clientX - tdrag.x, e.clientY - tdrag.y) > 6) {
    tdrag.moving = true;
    document.body.classList.add('dragging-thumb');
    tdrag.line = el('div', { class: 'drop-line' });
    thumbsEl.append(tdrag.line);
  }
  if (!tdrag.moving) return;
  // Insertion index = number of thumbnails whose midpoint is above the pointer.
  let drop = 0;
  S.thumbEls.forEach((t, i) => { const r = t.box.getBoundingClientRect(); if (e.clientY > r.top + r.height / 2) drop = i + 1; });
  tdrag.drop = drop;
  const base = thumbsEl.getBoundingClientRect();
  let y;
  if (drop >= S.thumbEls.length) { const r = S.thumbEls[S.thumbEls.length - 1].box.getBoundingClientRect(); y = r.bottom + 5; }
  else { const r = S.thumbEls[drop].box.getBoundingClientRect(); y = r.top - 8; }
  tdrag.line.style.top = y - base.top + 'px';
});

async function endThumbDrag(e) {
  if (!tdrag) return;
  const d = tdrag; tdrag = null;
  document.body.classList.remove('dragging-thumb');
  if (d.line) d.line.remove();
  try { thumbsEl.releasePointerCapture(e.pointerId); } catch (_) { /* ignore */ }
  if (d.moving) {
    const to = d.drop > d.i ? d.drop - 1 : d.drop;
    if (to !== d.i && e.type === 'pointerup') {
      S.cur = to; S.selected.clear(); S.selected.add(to);
      await runEdit(() => invoke('move_page', { id: S.doc.id, from: d.i, to }));
    }
    return;
  }
  // plain click: selection
  if (d.shift) {
    const [a, b] = [Math.min(S.anchor, d.i), Math.max(S.anchor, d.i)];
    S.selected = new Set(Array.from({ length: b - a + 1 }, (_, n) => a + n));
  } else if (d.ctrl) {
    S.selected.has(d.i) ? S.selected.delete(d.i) : S.selected.add(d.i);
    S.anchor = d.i;
  } else {
    S.selected = new Set([d.i]);
    S.anchor = d.i;
  }
  goToPage(d.i);
}
thumbsEl.addEventListener('pointerup', endThumbDrag);
thumbsEl.addEventListener('pointercancel', endThumbDrag);

// ───────────────────────────── wiring ───────────────────────────────────────

$('#btn-open').onclick = openDialog;
$('#btn-open-big').onclick = openDialog;
$('#btn-save').onclick = saveDoc;
$('#btn-saveas').onclick = saveAs;
$('#btn-undo').onclick = doUndo;
$('#btn-zoom-in').onclick = () => zoomBy(1.2);
$('#btn-zoom-out').onclick = () => zoomBy(1 / 1.2);
$('#zoom-label').onclick = () => setZoom(fitZoom(), true);
$('#btn-merge').onclick = mergeDialog;
$('#page-input').addEventListener('change', (e) => goToPage((parseInt(e.target.value, 10) || 1) - 1));
$('#btn-next').onclick = () => stepHit(1);
$('#btn-prev').onclick = () => stepHit(-1);
$('#match-case').addEventListener('change', () => S.query && runSearch());
$('#search-input').addEventListener('keydown', (e) => {
  if (e.key !== 'Enter') return;
  const q = e.target.value.trim();
  if (q && q === S.query && S.hits.length) stepHit(e.shiftKey ? -1 : 1);
  else runSearch();
});
$('#search-input').addEventListener('input', (e) => { if (!e.target.value) runSearch(); });

$$('.tool').forEach((b) => b.addEventListener('click', () => setTool(b.dataset.tool)));
$('#color').addEventListener('input', (e) => { S.color = e.target.value; if (S.colors[S.tool]) S.colors[S.tool] = S.color; });
$('#size').addEventListener('change', (e) => { const v = +e.target.value; if (S.tool === 'text') S.textSize = v; else S.lineW = v; });

const menu = $('#menu-pages');
$('#btn-pages').addEventListener('click', (e) => { e.stopPropagation(); menu.hidden = !menu.hidden; });
document.addEventListener('click', () => (menu.hidden = true));
menu.addEventListener('click', (e) => {
  const b = e.target.closest('button[data-act]');
  if (!b) return;
  menu.hidden = true;
  pageAction(b.dataset.act);
});

function applyTheme(t) {
  document.documentElement.dataset.theme = t;
  try { localStorage.setItem('folio-theme', t); } catch (_) { /* storage unavailable */ }
}
$('#btn-theme').onclick = () => applyTheme(document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark');
applyTheme((() => { try { return localStorage.getItem('folio-theme'); } catch (_) { return null; } })()
  || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'));

window.addEventListener('keydown', (e) => {
  if (modalOpen) return;
  const mod = e.ctrlKey || e.metaKey;
  const k = e.key.toLowerCase();
  const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement && document.activeElement.tagName);
  if (mod) {
    if (k === 'o') { e.preventDefault(); openDialog(); }
    else if (k === 's') { e.preventDefault(); e.shiftKey ? saveAs() : saveDoc(); }
    else if (k === 'z' && !typing) { e.preventDefault(); doUndo(); }
    else if (k === 'f') { e.preventDefault(); const s = $('#search-input'); if (!s.disabled) { s.focus(); s.select(); } }
    else if (k === '=' || k === '+') { e.preventDefault(); zoomBy(1.2); }
    else if (k === '-') { e.preventDefault(); zoomBy(1 / 1.2); }
    else if (k === '0') { e.preventDefault(); S.doc && setZoom(fitZoom(), true); }
    else if (k === 'p') { e.preventDefault(); S.doc && printDoc(); }
    return;
  }
  if (typing) return;
  const tools = { v: 'select', h: 'highlight', b: 'box', p: 'pen', t: 'text' };
  if (tools[k]) { setTool(tools[k]); return; }
  if (e.key === 'Escape') { S.selected.clear(); updateThumbStates(); setTool('select'); }
  else if (e.key === 'Home') goToPage(0);
  else if (e.key === 'End' && S.doc) goToPage(S.doc.page_count - 1);
  else if (e.key === 'F3') { e.preventDefault(); stepHit(e.shiftKey ? -1 : 1); }
});

// Drag and drop of files onto the window (Tauri delivers native paths).
T.webview.getCurrentWebview().onDragDropEvent(async (ev) => {
  const t = ev.payload.type;
  if (t === 'enter' || t === 'over') document.body.classList.add('drop-hover');
  else document.body.classList.remove('drop-hover');
  if (t !== 'drop') return;
  const pdfs = (ev.payload.paths || []).filter((p) => /\.pdf$/i.test(p));
  if (!pdfs.length) { toast('Drop a PDF file to open it.', true); return; }
  if (!S.doc) { openPath(pdfs[0]); return; }
  const choice = await modal({
    title: `Add ${basename(pdfs[0])}${pdfs.length > 1 ? ` and ${pdfs.length - 1} more` : ''}`,
    body: el('p', { text: 'Open it in place of the current document, or insert its pages after the current page?' }),
    actions: [
      { label: 'Cancel', value: null },
      { label: 'Insert pages', value: 'insert' },
      { label: 'Open', value: 'open', primary: true },
    ],
  });
  if (choice === 'open') { if (await confirmDiscard()) openPath(pdfs[0]); }
  else if (choice === 'insert') { let at = S.cur + 1; for (const p of pdfs) { await insertPdf(p, at); at = S.cur + 1; } }
});

// Ask before closing the window with unsaved changes.
appWindow.onCloseRequested(async (event) => {
  if (!S.doc || !S.doc.dirty) return;
  event.preventDefault();
  if (await confirmDiscard()) appWindow.destroy();
});

// ───────────────────────────── startup ──────────────────────────────────────

(async function start() {
  setTool('select');
  try {
    await invoke('engine_status');
  } catch (e) {
    await modal({
      title: 'Folio could not start its PDF engine',
      body: [el('p', { text: errMsg(e) }), el('p', { text: 'Re-run the Folio installer to restore the PDFium library next to the app.' })],
      actions: [{ label: 'Close', value: true, primary: true }],
    });
    appWindow.destroy();
    return;
  }
  const files = await invoke('initial_files').catch(() => []);
  if (files.length) await openPath(files[0]);
  else rebuild();
})();
