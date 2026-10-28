// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use folio_core::{Annot, DocInfo, Engine, SearchHit};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{ipc::Response, State};

/// Holds the PDF engine, or the reason it could not start (shown in the UI).
struct AppState {
    engine: Result<Arc<Engine>, String>,
}

impl AppState {
    fn engine(&self) -> Result<Arc<Engine>, String> {
        self.engine.clone()
    }
}

/// Run an engine call off the async runtime's worker threads.
async fn with_engine<R, F>(state: &State<'_, AppState>, f: F) -> Result<R, String>
where
    R: Send + 'static,
    F: FnOnce(&Engine) -> folio_core::Result<R> + Send + 'static,
{
    let engine = state.engine()?;
    tauri::async_runtime::spawn_blocking(move || f(&engine))
        .await
        .map_err(|e| e.to_string())?
}

// ───────────────────────────── commands ──────────────────────────────────────

/// PDF paths passed on the command line (file association, "Open with", drag onto the icon).
#[tauri::command]
fn initial_files() -> Vec<String> {
    std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .filter(|a| std::path::Path::new(a).is_file())
        .collect()
}

#[tauri::command]
fn engine_status(state: State<'_, AppState>) -> Result<(), String> {
    state.engine().map(|_| ())
}

#[tauri::command]
async fn open_pdf(state: State<'_, AppState>, path: String, password: Option<String>) -> Result<DocInfo, String> {
    with_engine(&state, move |e| e.open(path, password)).await
}

#[tauri::command]
async fn new_blank(state: State<'_, AppState>) -> Result<DocInfo, String> {
    with_engine(&state, |e| e.new_blank()).await
}

#[tauri::command]
async fn close_doc(state: State<'_, AppState>, id: u32) -> Result<(), String> {
    with_engine(&state, move |e| e.close(id)).await
}

#[tauri::command]
async fn render_page(state: State<'_, AppState>, id: u32, page: u32, width: u32) -> Result<Response, String> {
    let png = with_engine(&state, move |e| e.render(id, page, width)).await?;
    Ok(Response::new(png))
}

#[tauri::command]
async fn page_text(state: State<'_, AppState>, id: u32, page: u32) -> Result<String, String> {
    with_engine(&state, move |e| e.page_text(id, page)).await
}

#[tauri::command]
async fn search(state: State<'_, AppState>, id: u32, query: String, match_case: bool) -> Result<Vec<SearchHit>, String> {
    with_engine(&state, move |e| e.search(id, query, match_case)).await
}

#[tauri::command]
async fn rotate_pages(state: State<'_, AppState>, id: u32, pages: Vec<u32>, degrees: i32) -> Result<DocInfo, String> {
    with_engine(&state, move |e| e.rotate_pages(id, pages, degrees)).await
}

#[tauri::command]
async fn delete_pages(state: State<'_, AppState>, id: u32, pages: Vec<u32>) -> Result<DocInfo, String> {
    with_engine(&state, move |e| e.delete_pages(id, pages)).await
}

#[tauri::command]
async fn move_page(state: State<'_, AppState>, id: u32, from: u32, to: u32) -> Result<DocInfo, String> {
    with_engine(&state, move |e| e.move_page(id, from, to)).await
}

#[tauri::command]
async fn insert_blank(state: State<'_, AppState>, id: u32, at: u32) -> Result<DocInfo, String> {
    with_engine(&state, move |e| e.insert_blank(id, at)).await
}

#[tauri::command]
async fn insert_pdf(
    state: State<'_, AppState>,
    id: u32,
    other: String,
    password: Option<String>,
    at: u32,
) -> Result<DocInfo, String> {
    with_engine(&state, move |e| e.insert_pdf(id, other, password, at)).await
}

#[tauri::command]
async fn add_annotations(state: State<'_, AppState>, id: u32, annots: Vec<Annot>) -> Result<DocInfo, String> {
    with_engine(&state, move |e| e.add_annotations(id, annots)).await
}

#[tauri::command]
async fn undo(state: State<'_, AppState>, id: u32) -> Result<DocInfo, String> {
    with_engine(&state, move |e| e.undo(id)).await
}

#[tauri::command]
async fn save_pdf(state: State<'_, AppState>, id: u32, path: String) -> Result<DocInfo, String> {
    with_engine(&state, move |e| e.save(id, path)).await
}

#[tauri::command]
async fn extract_pages(state: State<'_, AppState>, id: u32, pages: Vec<u32>, out_path: String) -> Result<(), String> {
    with_engine(&state, move |e| e.extract(id, pages, out_path)).await
}

#[tauri::command]
async fn split_every(
    state: State<'_, AppState>,
    id: u32,
    every: u32,
    out_dir: String,
    stem: String,
) -> Result<Vec<String>, String> {
    with_engine(&state, move |e| e.split_every(id, every, out_dir, stem)).await
}

#[tauri::command]
async fn merge_pdfs(state: State<'_, AppState>, inputs: Vec<String>, out_path: String) -> Result<u32, String> {
    let inputs = inputs.into_iter().map(|p| (p, None)).collect();
    with_engine(&state, move |e| e.merge(inputs, out_path)).await
}

// ───────────────────────────── startup ───────────────────────────────────────

/// Where to look for the PDFium shared library: beside the executable first
/// (that is where the installers put it), then a few development fallbacks.
fn pdfium_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(d) = std::env::var("FOLIO_PDFIUM_DIR") {
        dirs.push(PathBuf::from(d));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.to_path_buf());
            dirs.push(dir.join("lib"));
            dirs.push(dir.join("pdfium"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("vendor").join("pdfium").join(pdfium_subdir()));
    }
    dirs
}

fn pdfium_subdir() -> &'static str {
    if cfg!(windows) { "bin" } else { "lib" }
}

fn main() {
    let engine = Engine::start(pdfium_search_dirs()).map(Arc::new);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState { engine })
        .invoke_handler(tauri::generate_handler![
            initial_files,
            engine_status,
            open_pdf,
            new_blank,
            close_doc,
            render_page,
            page_text,
            search,
            rotate_pages,
            delete_pages,
            move_page,
            insert_blank,
            insert_pdf,
            add_annotations,
            undo,
            save_pdf,
            extract_pages,
            split_every,
            merge_pdfs,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Folio");
}
