//! Integration tests against a real PDFium library.
//! Point FOLIO_PDFIUM_DIR at the folder containing libpdfium.so / pdfium.dll.
//! Tests are skipped (with a notice) when the library is not available.

use folio_core::{Annot, Engine, Rect};
use std::path::PathBuf;
use std::sync::OnceLock;

// PDFium can only be initialised once per process, so every test shares one engine.
static ENGINE: OnceLock<Option<Engine>> = OnceLock::new();

fn engine() -> Option<&'static Engine> {
    ENGINE
        .get_or_init(|| {
            let dir = std::env::var("FOLIO_PDFIUM_DIR").ok().map(PathBuf::from)?;
            Some(Engine::start(vec![dir]).expect("engine should start"))
        })
        .as_ref()
}

macro_rules! need_engine {
    () => {
        match engine() {
            Some(e) => e,
            None => {
                eprintln!("FOLIO_PDFIUM_DIR not set; skipping");
                return;
            }
        }
    };
}

fn tmp(name: &str) -> String {
    let dir = std::env::temp_dir().join("folio-tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name).to_string_lossy().to_string()
}

fn text_at(page: u32, x: f32, y: f32, s: &str) -> Annot {
    Annot::Text { page, x, y, text: s.into(), size: 24.0, color: [0, 0, 0] }
}

/// Fraction of non-white pixels inside a display-space rect of a rendered page.
fn ink_fraction(png: &[u8], page_w: f32, page_h: f32, r: &Rect) -> f32 {
    let img = image::load_from_memory(png).unwrap().to_rgb8();
    let sx = img.width() as f32 / page_w;
    let sy = img.height() as f32 / page_h;
    let (x0, y0) = ((r.x * sx) as u32, (r.y * sy) as u32);
    let (x1, y1) = (((r.x + r.w) * sx) as u32, ((r.y + r.h) * sy) as u32);
    let (mut dark, mut total) = (0u32, 0u32);
    for y in y0..y1.min(img.height()) {
        for x in x0..x1.min(img.width()) {
            total += 1;
            let p = img.get_pixel(x, y);
            if (p[0] as u32 + p[1] as u32 + p[2] as u32) < 600 {
                dark += 1;
            }
        }
    }
    if total == 0 { 0.0 } else { dark as f32 / total as f32 }
}

#[test]
fn blank_text_save_reopen_search() {
    let e = need_engine!();
    let info = e.new_blank().unwrap();
    assert_eq!(info.page_count, 1);
    let info = e.add_annotations(info.id, vec![text_at(0, 100.0, 100.0, "Hello Folio")]).unwrap();
    assert!(info.can_undo);

    let path = tmp("hello.pdf");
    e.save(info.id, path.clone()).unwrap();
    e.close(info.id).unwrap();

    let doc = e.open(path, None).unwrap();
    assert!(e.page_text(doc.id, 0).unwrap().contains("Hello Folio"));
    let hits = e.search(doc.id, "folio".into(), false).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(e.search(doc.id, "folio".into(), true).unwrap().is_empty(), "match-case must work");
}

/// The core promise of the geometry code: a search hit rectangle must sit on top of
/// the rendered ink, on pages of every rotation.
#[test]
fn search_rects_land_on_rendered_text_for_all_rotations() {
    let e = need_engine!();
    for rot in [0, 90, 180, 270] {
        let doc = e.new_blank().unwrap();
        let id = doc.id;
        // Place the text away from the centre so mirrored/rotated mistakes can't pass by luck.
        e.add_annotations(id, vec![text_at(0, 80.0, 150.0, "NEEDLE")]).unwrap();
        let info = if rot == 0 { e.info(id).unwrap() } else { e.rotate_pages(id, vec![0], rot).unwrap() };
        assert_eq!(info.pages[0].rotation as i32, rot);

        let hits = e.search(id, "NEEDLE".into(), true).unwrap();
        assert_eq!(hits.len(), 1, "rot {}", rot);
        let r = hits[0].rects[0];
        let p = &info.pages[0];
        assert!(r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= p.width + 1.0 && r.y + r.h <= p.height + 1.0,
            "rot {} rect {:?} outside page {}x{}", rot, r, p.width, p.height);

        let png = e.render(id, 0, 600).unwrap();
        let inside = ink_fraction(&png, p.width, p.height, &r);
        assert!(inside > 0.08, "rot {}: hit rect {:?} should cover text, ink fraction {}", rot, r, inside);
        // And a mirrored location (the classic bug) must be mostly empty.
        let mirrored = Rect { x: p.width - r.x - r.w, y: p.height - r.y - r.h, w: r.w, h: r.h };
        if (mirrored.x - r.x).abs() > r.w * 2.0 || (mirrored.y - r.y).abs() > r.h * 2.0 {
            assert!(ink_fraction(&png, p.width, p.height, &mirrored) < inside / 4.0, "rot {} not mirrored", rot);
        }
    }
}

/// Annotations drawn on a rotated page must land where the user drew them (display space).
#[test]
fn annotations_on_rotated_page_land_in_display_space() {
    let e = need_engine!();
    for rot in [0, 90, 180, 270] {
        let doc = e.new_blank().unwrap();
        let id = doc.id;
        let info = if rot == 0 { e.info(id).unwrap() } else { e.rotate_pages(id, vec![0], rot).unwrap() };
        let p = info.pages[0].clone();
        let target = Rect { x: 40.0, y: 60.0, w: 90.0, h: 30.0 };
        e.add_annotations(id, vec![Annot::Highlight { page: 0, rects: vec![target], color: [0, 0, 0], opacity: 1.0 }]).unwrap();
        let png = e.render(id, 0, 600).unwrap();
        assert!(ink_fraction(&png, p.width, p.height, &target) > 0.9, "rot {} highlight not where drawn", rot);
        let far = Rect { x: p.width - 130.0, y: p.height - 90.0, w: 90.0, h: 30.0 };
        assert!(ink_fraction(&png, p.width, p.height, &far) < 0.05, "rot {} stray ink", rot);
    }
}

#[test]
fn page_operations_and_undo() {
    let e = need_engine!();
    let doc = e.new_blank().unwrap();
    let id = doc.id;
    let info = e.insert_blank(id, 1).unwrap();
    assert_eq!(info.page_count, 2);
    let info = e.insert_blank(id, 2).unwrap();
    assert_eq!(info.page_count, 3);

    // Label pages so we can track order: 0="A", 1="B", 2="C".
    e.add_annotations(id, vec![text_at(0, 50.0, 50.0, "AAAA"), text_at(1, 50.0, 50.0, "BBBB"), text_at(2, 50.0, 50.0, "CCCC")]).unwrap();
    let order = |e: &Engine| -> String {
        (0..3).map(|i| e.page_text(id, i).unwrap().trim().chars().next().unwrap()).collect()
    };
    assert_eq!(order(&e), "ABC");

    e.move_page(id, 0, 2).unwrap();
    assert_eq!(order(&e), "BCA");
    e.undo(id).unwrap();
    assert_eq!(order(&e), "ABC");

    e.delete_pages(id, vec![1]).unwrap();
    assert_eq!(e.info(id).unwrap().page_count, 2);
    e.undo(id).unwrap();
    assert_eq!(e.info(id).unwrap().page_count, 3);

    assert!(e.delete_pages(id, vec![0, 1, 2]).is_err(), "must keep one page");
    assert_eq!(e.info(id).unwrap().page_count, 3, "failed edit leaves doc untouched");
}

#[test]
fn extract_split_merge_and_insert() {
    let e = need_engine!();
    let doc = e.new_blank().unwrap();
    let id = doc.id;
    e.insert_blank(id, 1).unwrap();
    e.insert_blank(id, 2).unwrap();
    e.insert_blank(id, 3).unwrap();
    e.insert_blank(id, 4).unwrap(); // 5 pages
    let labels = ["P1", "P2", "P3", "P4", "P5"];
    let annots: Vec<Annot> = labels.iter().enumerate().map(|(i, l)| text_at(i as u32, 50.0, 50.0, l)).collect();
    e.add_annotations(id, annots).unwrap();
    let src = tmp("five.pdf");
    e.save(id, src.clone()).unwrap();

    // extract pages 2 and 4 (0-based 1 and 3) in that order
    let ex = tmp("extract.pdf");
    e.extract(id, vec![3, 1], ex.clone()).unwrap();
    let d = e.open(ex, None).unwrap();
    assert_eq!(d.page_count, 2);
    assert!(e.page_text(d.id, 0).unwrap().contains("P4"));
    assert!(e.page_text(d.id, 1).unwrap().contains("P2"));

    // split every 2 -> 3 files (2,2,1)
    let dir = std::env::temp_dir().join("folio-tests").join("split");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let files = e.split_every(id, 2, dir.to_string_lossy().to_string(), "five".into()).unwrap();
    assert_eq!(files.len(), 3);
    let counts: Vec<u32> = files.iter().map(|f| e.open(f.clone(), None).unwrap().page_count).collect();
    assert_eq!(counts, vec![2, 2, 1]);

    // merge two files
    let merged = tmp("merged.pdf");
    let n = e.merge(vec![(files[0].clone(), None), (files[2].clone(), None)], merged.clone()).unwrap();
    assert_eq!(n, 3);

    // insert another pdf into an open doc at position 1
    let target = e.new_blank().unwrap();
    let info = e.insert_pdf(target.id, src, None, 1).unwrap();
    assert_eq!(info.page_count, 6);
    assert!(e.page_text(target.id, 1).unwrap().contains("P1"));
}

#[test]
fn errors_are_helpful() {
    let e = need_engine!();
    let missing = e.open("/definitely/not/here.pdf".into(), None).unwrap_err();
    assert!(missing.contains("Could not open"), "{}", missing);
    let bad = tmp("notapdf.pdf");
    std::fs::write(&bad, b"this is not a pdf").unwrap();
    assert!(e.open(bad, None).unwrap_err().contains("not a valid PDF"));
}

#[test]
fn render_returns_png_of_requested_width() {
    let e = need_engine!();
    let d = e.new_blank().unwrap();
    let png = e.render(d.id, 0, 400).unwrap();
    assert_eq!(&png[..4], &[0x89, b'P', b'N', b'G']);
    let img = image::load_from_memory(&png).unwrap();
    assert_eq!(img.width(), 400);
}

#[test]
fn dirty_flag_tracks_saves_and_undo() {
    let e = need_engine!();
    let d = e.new_blank().unwrap();
    let id = d.id;
    assert!(!d.dirty);
    let info = e.insert_blank(id, 1).unwrap();
    assert!(info.dirty);
    let info = e.undo(id).unwrap();
    assert!(!info.dirty, "undoing back to the starting point is clean again");
    e.insert_blank(id, 1).unwrap();
    let saved = e.save(id, tmp("dirty.pdf")).unwrap();
    assert!(!saved.dirty);
    let info = e.undo(id).unwrap();
    assert!(info.dirty, "undo after a save diverges from the saved file");
    let info = e.insert_blank(id, 1).unwrap();
    assert!(info.dirty, "re-doing equivalent work must not look clean");
}

#[test]
fn password_protected_documents() {
    let e = need_engine!();
    let path = format!("{}/../../tests/fixtures/encrypted.pdf", env!("CARGO_MANIFEST_DIR"));
    assert_eq!(e.open(path.clone(), None).unwrap_err(), folio_core::PASSWORD_REQUIRED);
    assert_eq!(e.open(path.clone(), Some("wrong".into())).unwrap_err(), folio_core::PASSWORD_INCORRECT);
    let doc = e.open(path, Some("secret".into())).unwrap();
    assert_eq!(doc.page_count, 4);
    assert!(e.page_text(doc.id, 0).unwrap().contains("Sample document"));
    // Edits and undo reload the document from snapshots; that must keep working with a password.
    let info = e.rotate_pages(doc.id, vec![0], 90).unwrap();
    assert_eq!(info.pages[0].rotation, 90);
    let info = e.undo(doc.id).unwrap();
    assert_eq!(info.pages[0].rotation, 0);
}

#[test]
fn real_world_fixture_opens_searches_and_renders() {
    let e = need_engine!();
    let path = format!("{}/../../tests/fixtures/sample.pdf", env!("CARGO_MANIFEST_DIR"));
    let doc = e.open(path, None).unwrap();
    assert_eq!(doc.page_count, 4);
    assert_eq!(doc.title.as_deref(), Some("Folio sample"));
    // page 3 is landscape
    assert!(doc.pages[2].width > doc.pages[2].height);
    let hits = e.search(doc.id, "lazy dog".into(), false).unwrap();
    assert_eq!(hits.len(), 4 * 14, "14 matching lines on each of 4 pages");
    let png = e.render(doc.id, 2, 800).unwrap();
    assert_eq!(image::load_from_memory(&png).unwrap().width(), 800);
}
