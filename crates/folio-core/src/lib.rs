//! Folio PDF engine.
//!
//! PDFium is not thread safe and its documents borrow from the library handle,
//! so everything lives on ONE dedicated worker thread. The rest of the app talks
//! to it through [`Engine`], which ships closures to that thread and waits for
//! the result. Open documents are addressed by a small integer id.
//!
//! Coordinates exchanged with the UI are always "display space": PDF points,
//! origin at the top-left of the page AS DISPLAYED (i.e. after page rotation).
//! This module converts to and from PDF user space internally.

use pdfium_render::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::thread;

pub type Result<T> = std::result::Result<T, String>;

/// Error string the UI looks for to prompt for a password.
pub const PASSWORD_REQUIRED: &str = "PASSWORD_REQUIRED";
/// Error string used when a supplied password was wrong.
pub const PASSWORD_INCORRECT: &str = "PASSWORD_INCORRECT";

const MAX_UNDO_BYTES: usize = 400 * 1024 * 1024;
const MAX_UNDO_STEPS: usize = 30;

fn err<E: std::fmt::Debug>(e: E) -> String {
    format!("{:?}", e)
}

// ───────────────────────────── public data types ─────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct PageInfo {
    /// Displayed width in points.
    pub width: f32,
    /// Displayed height in points.
    pub height: f32,
    /// Page /Rotate value: 0, 90, 180 or 270 (clockwise).
    pub rotation: u16,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocInfo {
    pub id: u32,
    pub path: Option<String>,
    pub title: Option<String>,
    pub page_count: u32,
    pub pages: Vec<PageInfo>,
    pub can_undo: bool,
    /// True when the in-memory document differs from what was last opened or saved.
    pub dirty: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub page: u32,
    pub rects: Vec<Rect>,
}

/// One drawable mark, in display space. Colours are [r, g, b] 0-255.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Annot {
    Highlight { page: u32, rects: Vec<Rect>, color: [u8; 3], opacity: f32 },
    Rect { page: u32, rect: Rect, color: [u8; 3], width: f32 },
    Ink { page: u32, points: Vec<[f32; 2]>, color: [u8; 3], width: f32 },
    Text { page: u32, x: f32, y: f32, text: String, size: f32, color: [u8; 3] },
}

impl Annot {
    fn page(&self) -> u32 {
        match self {
            Annot::Highlight { page, .. }
            | Annot::Rect { page, .. }
            | Annot::Ink { page, .. }
            | Annot::Text { page, .. } => *page,
        }
    }
}

// ───────────────────────── geometry (rotation handling) ──────────────────────

/// Maps between display space (top-left origin, rotated) and PDF user space.
#[derive(Debug, Clone, Copy)]
struct PageMap {
    /// Unrotated page width / height in points.
    w: f32,
    h: f32,
    /// Origin of the page box in user space.
    ox: f32,
    oy: f32,
    /// Clockwise rotation in degrees.
    rot: u16,
}

impl PageMap {
    fn of(page: &PdfPage) -> Result<PageMap> {
        let rot = rotation_degrees(page);
        // page.width()/height() are the DISPLAYED size; swap back for 90/270.
        let (dw, dh) = (page.width().value, page.height().value);
        let (w, h) = if rot == 90 || rot == 270 { (dh, dw) } else { (dw, dh) };
        let (ox, oy) = match page.boundaries().bounding() {
            Ok(b) => (b.bounds.left().value, b.bounds.bottom().value),
            Err(_) => (0.0, 0.0),
        };
        Ok(PageMap { w, h, ox, oy, rot })
    }

    /// Display-space point -> PDF user-space point.
    fn to_pdf(&self, x: f32, y: f32) -> (f32, f32) {
        // First undo the rotation to get unrotated top-left coordinates.
        let (ux, uy) = match self.rot {
            90 => (y, self.h - x),
            180 => (self.w - x, self.h - y),
            270 => (self.w - y, x),
            _ => (x, y),
        };
        (self.ox + ux, self.oy + (self.h - uy))
    }

    /// PDF user-space point -> display-space point.
    fn to_display(&self, px: f32, py: f32) -> (f32, f32) {
        let ux = px - self.ox;
        let uy = self.h - (py - self.oy);
        match self.rot {
            90 => (self.h - uy, ux),
            180 => (self.w - ux, self.h - uy),
            270 => (uy, self.w - ux),
            _ => (ux, uy),
        }
    }

    fn rect_to_pdf(&self, r: &Rect) -> PdfRect {
        let (x1, y1) = self.to_pdf(r.x, r.y);
        let (x2, y2) = self.to_pdf(r.x + r.w, r.y + r.h);
        PdfRect::new_from_values(y1.min(y2), x1.min(x2), y1.max(y2), x1.max(x2))
    }

    fn rect_to_display(&self, r: &PdfRect) -> Rect {
        let (x1, y1) = self.to_display(r.left().value, r.bottom().value);
        let (x2, y2) = self.to_display(r.right().value, r.top().value);
        Rect { x: x1.min(x2), y: y1.min(y2), w: (x2 - x1).abs(), h: (y2 - y1).abs() }
    }
}

fn rotation_degrees(page: &PdfPage) -> u16 {
    match page.rotation() {
        Ok(PdfPageRenderRotation::Degrees90) => 90,
        Ok(PdfPageRenderRotation::Degrees180) => 180,
        Ok(PdfPageRenderRotation::Degrees270) => 270,
        _ => 0,
    }
}

fn rotation_from_degrees(d: u16) -> PdfPageRenderRotation {
    match d % 360 {
        90 => PdfPageRenderRotation::Degrees90,
        180 => PdfPageRenderRotation::Degrees180,
        270 => PdfPageRenderRotation::Degrees270,
        _ => PdfPageRenderRotation::None,
    }
}

// ───────────────────────────── worker thread plumbing ────────────────────────

struct Doc {
    doc: PdfDocument<'static>,
    path: Option<String>,
    password: Option<String>,
    /// Snapshots (bytes, version) taken before each edit, oldest first.
    undo: Vec<(Vec<u8>, u64)>,
    /// Unique id of the current content; changes on every edit and on undo.
    version: u64,
    saved_version: u64,
    next_version: u64,
}

impl Doc {
    fn new(doc: PdfDocument<'static>, path: Option<String>, password: Option<String>) -> Doc {
        Doc { doc, path, password, undo: vec![], version: 0, saved_version: 0, next_version: 1 }
    }
}

struct Ctx {
    pdfium: &'static Pdfium,
    docs: HashMap<u32, Doc>,
    next_id: u32,
}

type Job = Box<dyn FnOnce(&mut Ctx) + Send>;

pub struct Engine {
    tx: Mutex<Sender<Job>>,
}

impl Engine {
    /// Start the worker thread and bind to the PDFium library.
    /// `search_dirs` are tried in order for the PDFium shared library, then the system.
    pub fn start(search_dirs: Vec<PathBuf>) -> Result<Engine> {
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();

        thread::Builder::new()
            .name("folio-pdfium".into())
            .spawn(move || {
                let mut failures: Vec<String> = Vec::new();
                let mut bindings = None;
                for d in &search_dirs {
                    let lib = Pdfium::pdfium_platform_library_name_at_path(d);
                    match Pdfium::bind_to_library(&lib) {
                        Ok(b) => {
                            bindings = Some(b);
                            break;
                        }
                        Err(e) => failures.push(format!("{} ({:?})", lib.display(), e)),
                    }
                }
                if bindings.is_none() {
                    match Pdfium::bind_to_system_library() {
                        Ok(b) => bindings = Some(b),
                        Err(e) => failures.push(format!("system library ({:?})", e)),
                    }
                }
                let bindings = match bindings {
                    Some(b) => b,
                    None => {
                        let _ = ready_tx.send(Err(format!(
                            "Could not load the PDFium library. Tried: {}",
                            failures.join("; ")
                        )));
                        return;
                    }
                };
                // Leak so documents can be 'static and stored in a map. Lives for the process.
                let pdfium: &'static Pdfium = Box::leak(Box::new(Pdfium::new(bindings)));
                let mut ctx = Ctx { pdfium, docs: HashMap::new(), next_id: 1 };
                let _ = ready_tx.send(Ok(()));
                while let Ok(job) = rx.recv() {
                    job(&mut ctx);
                }
            })
            .map_err(err)?;

        ready_rx.recv().map_err(err)??;
        Ok(Engine { tx: Mutex::new(tx) })
    }

    fn run<R: Send + 'static>(&self, f: impl FnOnce(&mut Ctx) -> Result<R> + Send + 'static) -> Result<R> {
        let (rtx, rrx) = mpsc::channel();
        let job: Job = Box::new(move |ctx| {
            // A panic inside PDFium glue must not kill the worker thread.
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(ctx)))
                .unwrap_or_else(|_| Err("internal error in PDF engine".to_string()));
            let _ = rtx.send(r);
        });
        self.tx.lock().map_err(err)?.send(job).map_err(err)?;
        rrx.recv().map_err(err)?
    }

    // ─────────────────────────────── documents ───────────────────────────────

    pub fn open(&self, path: String, password: Option<String>) -> Result<DocInfo> {
        self.run(move |ctx| {
            let doc = load_file(ctx.pdfium, &path, password.as_deref())?;
            let id = ctx.next_id;
            ctx.next_id += 1;
            ctx.docs.insert(id, Doc::new(doc, Some(path), password));
            doc_info(ctx, id)
        })
    }

    /// Create a new empty document with one blank A4-ish page.
    pub fn new_blank(&self) -> Result<DocInfo> {
        self.run(move |ctx| {
            let mut doc = ctx.pdfium.create_new_pdf().map_err(err)?;
            doc.pages_mut()
                .create_page_at_start(PdfPagePaperSize::a4())
                .map_err(err)?;
            let id = ctx.next_id;
            ctx.next_id += 1;
            ctx.docs.insert(id, Doc::new(doc, None, None));
            doc_info(ctx, id)
        })
    }

    pub fn info(&self, id: u32) -> Result<DocInfo> {
        self.run(move |ctx| doc_info(ctx, id))
    }

    pub fn close(&self, id: u32) -> Result<()> {
        self.run(move |ctx| {
            ctx.docs.remove(&id);
            Ok(())
        })
    }

    // ─────────────────────────────── rendering ───────────────────────────────

    /// Render one page to PNG bytes, `width_px` wide (aspect ratio preserved).
    pub fn render(&self, id: u32, page: u32, width_px: u32) -> Result<Vec<u8>> {
        self.run(move |ctx| {
            let d = get_doc(ctx, id)?;
            let p = d.doc.pages().get(page as i32).map_err(err)?;
            let width = width_px.clamp(16, 6000) as i32;
            let cfg = PdfRenderConfig::new()
                .set_target_width(width)
                .set_maximum_height(width.saturating_mul(6))
                .set_clear_color(PdfColor::WHITE)
                .render_form_data(true)
                .render_annotations(true);
            let bmp = p.render_with_config(&cfg).map_err(err)?;
            let rgb = bmp.as_image().map_err(err)?.to_rgb8();
            let mut out = Vec::with_capacity(rgb.len() / 4);
            let enc = image::codecs::png::PngEncoder::new_with_quality(
                &mut out,
                image::codecs::png::CompressionType::Fast,
                image::codecs::png::FilterType::Sub,
            );
            use image::ImageEncoder;
            enc.write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
                .map_err(err)?;
            Ok(out)
        })
    }

    // ───────────────────────────────── text ──────────────────────────────────

    pub fn page_text(&self, id: u32, page: u32) -> Result<String> {
        self.run(move |ctx| {
            let d = get_doc(ctx, id)?;
            let p = d.doc.pages().get(page as i32).map_err(err)?;
            let text = p.text().map_err(err)?.all();
            Ok(text)
        })
    }

    pub fn search(&self, id: u32, query: String, match_case: bool) -> Result<Vec<SearchHit>> {
        self.run(move |ctx| {
            let d = get_doc(ctx, id)?;
            let mut hits = Vec::new();
            if query.trim().is_empty() {
                return Ok(hits);
            }
            let opts = PdfSearchOptions::new().match_case(match_case);
            for (i, p) in d.doc.pages().iter().enumerate() {
                let map = PageMap::of(&p)?;
                let text = match p.text() {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let search = match text.search(&query, &opts) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                while let Some(segments) = search.find_next() {
                    let rects: Vec<Rect> =
                        segments.iter().map(|s| map.rect_to_display(&s.bounds())).collect();
                    if !rects.is_empty() {
                        hits.push(SearchHit { page: i as u32, rects });
                    }
                    if hits.len() > 5000 {
                        return Ok(hits);
                    }
                }
            }
            Ok(hits)
        })
    }

    // ───────────────────────── structural editing (undoable) ─────────────────

    pub fn rotate_pages(&self, id: u32, pages: Vec<u32>, clockwise_degrees: i32) -> Result<DocInfo> {
        self.edit(id, move |_, d| {
            for idx in pages {
                let mut p = d.doc.pages().get(idx as i32).map_err(err)?;
                let cur = rotation_degrees(&p) as i32;
                let new = ((cur + clockwise_degrees) % 360 + 360) % 360;
                p.set_rotation(rotation_from_degrees(new as u16));
            }
            Ok(())
        })
    }

    pub fn delete_pages(&self, id: u32, pages: Vec<u32>) -> Result<DocInfo> {
        self.edit(id, move |_, d| {
            let count = d.doc.pages().len() as usize;
            let mut uniq: Vec<u32> = pages;
            uniq.sort_unstable();
            uniq.dedup();
            if uniq.len() >= count {
                return Err("A PDF must keep at least one page.".into());
            }
            // Delete from the end so indices stay valid.
            for idx in uniq.into_iter().rev() {
                d.doc.pages().get(idx as i32).map_err(err)?.delete().map_err(err)?;
            }
            Ok(())
        })
    }

    /// Move page `from` so that it ends up at index `to` (index in the final order).
    pub fn move_page(&self, id: u32, from: u32, to: u32) -> Result<DocInfo> {
        self.edit(id, move |ctx, d| {
            let n = d.doc.pages().len() as u32;
            if from >= n || to >= n {
                return Err("page index out of range".into());
            }
            if from == to {
                return Ok(());
            }
            let bytes = d.doc.save_to_bytes().map_err(err)?;
            let src = ctx.pdfium.load_pdf_from_byte_vec(bytes, d.password.as_deref()).map_err(err)?;
            d.doc.pages().get(from as i32).map_err(err)?.delete().map_err(err)?;
            d.doc
                .pages_mut()
                .copy_page_from_document(&src, from as i32, to as i32)
                .map_err(err)?;
            Ok(())
        })
    }

    pub fn insert_blank(&self, id: u32, at: u32) -> Result<DocInfo> {
        self.edit(id, move |_, d| {
            let n = d.doc.pages().len() as u32;
            let at = at.min(n);
            // Match the size of the neighbouring page when there is one.
            let size = if n > 0 {
                let neighbour = if at > 0 { at - 1 } else { 0 };
                let p = d.doc.pages().get(neighbour as i32).map_err(err)?;
                PdfPagePaperSize::Custom(PdfPoints::new(p.width().value), PdfPoints::new(p.height().value))
            } else {
                PdfPagePaperSize::a4()
            };
            d.doc.pages_mut().create_page_at_index(size, at as i32).map_err(err)?;
            Ok(())
        })
    }

    /// Insert every page of another PDF at `at`.
    pub fn insert_pdf(&self, id: u32, other: String, other_password: Option<String>, at: u32) -> Result<DocInfo> {
        self.edit(id, move |ctx, d| {
            let src = load_file(ctx.pdfium, &other, other_password.as_deref())?;
            let n = d.doc.pages().len() as u32;
            let at = at.min(n);
            let range = src.pages().as_range_inclusive();
            d.doc.pages_mut().copy_page_range_from_document(&src, range, at as i32).map_err(err)?;
            Ok(())
        })
    }

    /// Bake marks into the page content. Undoable like any other edit.
    pub fn add_annotations(&self, id: u32, annots: Vec<Annot>) -> Result<DocInfo> {
        self.edit(id, move |_, d| apply_annots(&mut d.doc, &annots))
    }

    pub fn undo(&self, id: u32) -> Result<DocInfo> {
        self.run(move |ctx| {
            let pdfium = ctx.pdfium;
            let d = get_doc_mut(ctx, id)?;
            let (bytes, version) = d.undo.pop().ok_or("Nothing to undo")?;
            d.doc = pdfium.load_pdf_from_byte_vec(bytes, d.password.as_deref()).map_err(err)?;
            d.version = version;
            doc_info(ctx, id)
        })
    }

    // ─────────────────────────── saving and exporting ────────────────────────

    /// Save the current state to `path`. Updates the document's remembered path.
    pub fn save(&self, id: u32, path: String) -> Result<DocInfo> {
        self.run(move |ctx| {
            {
                let d = get_doc_mut(ctx, id)?;
                write_atomically(&mut d.doc, &path)?;
                d.path = Some(path);
                d.saved_version = d.version;
            }
            doc_info(ctx, id)
        })
    }

    /// Write a new file containing only `pages` (0-based, in the order given).
    pub fn extract(&self, id: u32, pages: Vec<u32>, out_path: String) -> Result<()> {
        self.run(move |ctx| {
            let d = get_doc(ctx, id)?;
            let mut out = ctx.pdfium.create_new_pdf().map_err(err)?;
            copy_pages(&d.doc, &pages, &mut out)?;
            write_atomically(&mut out, &out_path)
        })
    }

    /// Split into files of `every` pages each: `<stem>-part1.pdf`, ... Returns written paths.
    pub fn split_every(&self, id: u32, every: u32, out_dir: String, stem: String) -> Result<Vec<String>> {
        self.run(move |ctx| {
            let d = get_doc(ctx, id)?;
            let n = d.doc.pages().len() as u32;
            let every = every.max(1);
            let mut written = Vec::new();
            let mut start = 0;
            let mut part = 1;
            while start < n {
                let end = (start + every).min(n);
                let pages: Vec<u32> = (start..end).collect();
                let mut out = ctx.pdfium.create_new_pdf().map_err(err)?;
                copy_pages(&d.doc, &pages, &mut out)?;
                let p = Path::new(&out_dir).join(format!("{}-part{}.pdf", stem, part));
                let p = p.to_string_lossy().to_string();
                write_atomically(&mut out, &p)?;
                written.push(p);
                start = end;
                part += 1;
            }
            Ok(written)
        })
    }

    /// Merge files in order into `out_path`. Independent of any open document.
    pub fn merge(&self, inputs: Vec<(String, Option<String>)>, out_path: String) -> Result<u32> {
        self.run(move |ctx| {
            if inputs.is_empty() {
                return Err("Nothing to merge".into());
            }
            let mut out = ctx.pdfium.create_new_pdf().map_err(err)?;
            for (path, pw) in &inputs {
                let src = load_file(ctx.pdfium, path, pw.as_deref())?;
                let at = out.pages().len();
                let range = src.pages().as_range_inclusive();
                out.pages_mut().copy_page_range_from_document(&src, range, at).map_err(err)?;
            }
            let count = out.pages().len() as u32;
            write_atomically(&mut out, &out_path)?;
            Ok(count)
        })
    }

    /// Run an undoable edit: snapshot first, restore the snapshot if the edit fails.
    fn edit(
        &self,
        id: u32,
        f: impl FnOnce(&mut Ctx, &mut Doc) -> Result<()> + Send + 'static,
    ) -> Result<DocInfo> {
        self.run(move |ctx| {
            // Temporarily take the doc out so `f` can borrow ctx (for pdfium) and doc together.
            let mut d = ctx.docs.remove(&id).ok_or("Document is not open")?;
            let snapshot = d.doc.save_to_bytes().map_err(err)?;
            let result = f(ctx, &mut d);
            match result {
                Ok(()) => {
                    d.undo.push((snapshot, d.version));
                    d.version = d.next_version;
                    d.next_version += 1;
                    trim_undo(&mut d.undo);
                    ctx.docs.insert(id, d);
                    doc_info(ctx, id)
                }
                Err(e) => {
                    // Roll back whatever partial change was made.
                    if let Ok(restored) =
                        ctx.pdfium.load_pdf_from_byte_vec(snapshot, d.password.as_deref())
                    {
                        d.doc = restored;
                    }
                    ctx.docs.insert(id, d);
                    Err(e)
                }
            }
        })
    }
}

// ─────────────────────────────── helpers ─────────────────────────────────────

fn get_doc(ctx: &Ctx, id: u32) -> Result<&Doc> {
    ctx.docs.get(&id).ok_or_else(|| "Document is not open".to_string())
}

fn get_doc_mut(ctx: &mut Ctx, id: u32) -> Result<&mut Doc> {
    ctx.docs.get_mut(&id).ok_or_else(|| "Document is not open".to_string())
}

fn trim_undo(undo: &mut Vec<(Vec<u8>, u64)>) {
    while undo.len() > MAX_UNDO_STEPS {
        undo.remove(0);
    }
    let mut total: usize = undo.iter().map(|(b, _)| b.len()).sum();
    while total > MAX_UNDO_BYTES && undo.len() > 1 {
        total -= undo.remove(0).0.len();
    }
}

/// Documents are always loaded from memory rather than from a path. PDFium would otherwise keep
/// the file open, which on Windows blocks replacing it when the user saves over the original.
fn load_file(pdfium: &'static Pdfium, path: &str, password: Option<&str>) -> Result<PdfDocument<'static>> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not open '{}': {}", path, e))?;
    match pdfium.load_pdf_from_byte_vec(bytes, password) {
        Ok(d) => Ok(d),
        Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)) => {
            Err(if password.map_or(true, |p| p.is_empty()) {
                PASSWORD_REQUIRED.to_string()
            } else {
                PASSWORD_INCORRECT.to_string()
            })
        }
        Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::FormatError)) => {
            Err(format!("'{}' is not a valid PDF file.", path))
        }
        Err(e) => Err(format!("Could not open '{}': {:?}", path, e)),
    }
}

fn doc_info(ctx: &Ctx, id: u32) -> Result<DocInfo> {
    let d = get_doc(ctx, id)?;
    let mut pages = Vec::new();
    for p in d.doc.pages().iter() {
        pages.push(PageInfo { width: p.width().value, height: p.height().value, rotation: rotation_degrees(&p) });
    }
    let title = d
        .doc
        .metadata()
        .get(PdfDocumentMetadataTagType::Title)
        .map(|t| t.value().to_string())
        .filter(|t| !t.trim().is_empty());
    Ok(DocInfo {
        id,
        path: d.path.clone(),
        title,
        page_count: pages.len() as u32,
        pages,
        can_undo: !d.undo.is_empty(),
        dirty: d.version != d.saved_version,
    })
}

fn copy_pages(src: &PdfDocument, pages: &[u32], out: &mut PdfDocument) -> Result<()> {
    if pages.is_empty() {
        return Err("No pages selected".into());
    }
    let n = src.pages().len() as u32;
    for (k, &p) in pages.iter().enumerate() {
        if p >= n {
            return Err(format!("page {} is out of range", p + 1));
        }
        out.pages_mut().copy_page_from_document(src, p as i32, k as i32).map_err(err)?;
    }
    Ok(())
}

/// Save to a temp file beside the target, then rename, so a failed save never destroys the original.
fn write_atomically(doc: &mut PdfDocument, path: &str) -> Result<()> {
    let target = Path::new(path);
    let tmp = target.with_extension("pdf.folio-tmp");
    doc.save_to_file(&tmp).map_err(|e| format!("Could not write file: {:?}", e))?;
    std::fs::rename(&tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Could not replace '{}': {}", path, e)
    })
}

fn color(c: [u8; 3], alpha: u8) -> PdfColor {
    PdfColor::new(c[0], c[1], c[2], alpha)
}

fn apply_annots(doc: &mut PdfDocument<'static>, annots: &[Annot]) -> Result<()> {
    let font = doc.fonts_mut().helvetica();
    for a in annots {
        let mut page = doc.pages().get(a.page() as i32).map_err(err)?;
        let map = PageMap::of(&page)?;
        match a {
            Annot::Highlight { rects, color: c, opacity, .. } => {
                let alpha = (opacity.clamp(0.05, 1.0) * 255.0) as u8;
                for r in rects {
                    let obj = PdfPagePathObject::new_rect(doc, map.rect_to_pdf(r), None, None, Some(color(*c, alpha)))
                        .map_err(err)?;
                    page.objects_mut().add_path_object(obj).map_err(err)?;
                }
            }
            Annot::Rect { rect, color: c, width, .. } => {
                let obj = PdfPagePathObject::new_rect(
                    doc,
                    map.rect_to_pdf(rect),
                    Some(color(*c, 255)),
                    Some(PdfPoints::new(width.max(0.25))),
                    None,
                )
                .map_err(err)?;
                page.objects_mut().add_path_object(obj).map_err(err)?;
            }
            Annot::Ink { points, color: c, width, .. } => {
                if points.len() < 2 {
                    continue;
                }
                let (sx, sy) = map.to_pdf(points[0][0], points[0][1]);
                let mut path = PdfPagePathObject::new(
                    doc,
                    PdfPoints::new(sx),
                    PdfPoints::new(sy),
                    Some(color(*c, 255)),
                    Some(PdfPoints::new(width.max(0.25))),
                    None,
                )
                .map_err(err)?;
                for pt in &points[1..] {
                    let (x, y) = map.to_pdf(pt[0], pt[1]);
                    path.line_to(PdfPoints::new(x), PdfPoints::new(y)).map_err(err)?;
                }
                path.set_line_cap(PdfPageObjectLineCap::Round).map_err(err)?;
                path.set_line_join(PdfPageObjectLineJoin::Round).map_err(err)?;
                page.objects_mut().add_path_object(path).map_err(err)?;
            }
            Annot::Text { x, y, text, size, color: c, .. } => {
                let size = size.clamp(4.0, 200.0);
                let theta = -(map.rot as f32).to_radians(); // counter-rotate so text reads upright
                let (sin, cos) = theta.sin_cos();
                for (i, line) in text.lines().enumerate() {
                    if line.is_empty() {
                        continue;
                    }
                    // Display-space baseline of this line: top of box + ascent + i * leading.
                    let bx = *x;
                    let by = *y + size * 0.94 + (i as f32) * size * 1.2;
                    let (px, py) = map.to_pdf(bx, by);
                    let mut obj = PdfPageTextObject::new(doc, sanitize(line), font, PdfPoints::new(size))
                        .map_err(err)?;
                    obj.set_fill_color(color(*c, 255)).map_err(err)?;
                    // Created at the origin; rotate then translate into place.
                    obj.transform(cos, sin, -sin, cos, px, py)
                        .map_err(err)?;
                    page.objects_mut().add_text_object(obj).map_err(err)?;
                }
            }
        }
        page.regenerate_content().map_err(err)?;
    }
    Ok(())
}

/// Built-in Helvetica covers Latin-1; swap anything outside it for '?'.
fn sanitize(s: &str) -> String {
    s.chars().map(|c| if (c as u32) < 256 { c } else { '?' }).collect()
}
