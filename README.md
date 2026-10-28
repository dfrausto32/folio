# Folio

A fast, private PDF viewer and editor for Windows and Linux. No account, no cloud, no subscription:
your files never leave your computer.

Folio renders with PDFium (the engine inside Chrome) and is built with Tauri and Rust, so the app is
small and starts quickly.

## What it does

- **Read**: smooth scrolling, zoom (Ctrl + mouse wheel), fit to width, page thumbnails, password-protected files
- **Search**: find text across the whole document with highlighted matches (Ctrl+F, Enter / Shift+Enter to step)
- **Mark up**: highlight, box, freehand pen and text. Marks are written into the page when you save
- **Sign**: draw, type or import a picture of your signature (press S), keep it for next time, click a page to place it, then drag and resize it before pressing Place. Works on rotated pages and can be undone
- **Pages**: rotate, delete, insert blank pages, insert pages from another PDF, drag thumbnails to reorder
- **Split and merge**: extract pages to a new file, split into several files, merge many PDFs into one
- **Safe**: full undo for every change, saves never destroy the original if something goes wrong, a prompt before closing with unsaved work
- **Print**, **copy page text**, dark and light themes, drag and drop, "Open with" support

## Install

### Option A: download a release (easiest, nothing to compile)

1. Open the **Releases** page of this repository and download `Folio-windows-x64.zip` (Windows) or `Folio-linux-x64.tar.gz` (Linux).
2. Unpack it and run the installer inside:
   - Windows: double-click `install.cmd`
   - Linux: `./install.sh`
3. Delete the download. Folio stays installed.

**"Windows protected your PC" or "publisher could not be verified"?** Folio is not code-signed yet (a signing
certificate costs money), so Windows warns about any new unsigned download. To avoid the warning, right-click
`Folio-windows-x64.zip`, choose Properties, tick **Unblock**, press OK, and only then unpack it. If the blue
SmartScreen box still appears, choose **More info** then **Run anyway**. The installer clears the same mark on the
installed copy, so Folio itself starts without a warning afterwards.

### Option B: build from source and leave nothing behind

```bash
git clone https://github.com/<you>/folio.git
cd folio
```

Windows (PowerShell):

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -RemoveSource
```

Linux:

```bash
./install.sh --remove-source
```

The installer checks for the tools it needs (Rust, the C++ build tools on Windows, WebKitGTK on Linux), offers to
install anything missing, downloads PDFium, builds Folio, installs it, creates the shortcuts, and with
`-RemoveSource` / `--remove-source` deletes the cloned folder when it is finished. The cloned folder holds the build
output (a few GB), so removing it is what keeps your computer clean. Rust itself stays installed
(`rustup self uninstall` removes it if you do not want it).

Where things go:

| | Windows | Linux |
|---|---|---|
| Program | `%LOCALAPPDATA%\Programs\Folio` | `~/.local/share/folio` |
| Menu entry | Start Menu | Applications menu |
| Desktop shortcut | yes (skip with `-NoDesktopShortcut`) | yes (skip with `--no-desktop-shortcut`) |
| Uninstall | Settings > Apps > Folio | `~/.local/share/folio/uninstall.sh` |

No administrator rights are needed. To make Folio your default PDF app, right-click a PDF, choose
Open with, pick Folio, and choose Always.

## Your own icon

The icon set lives in `src-tauri/icons`. To redraw it or use your own artwork:

```bash
pip install pillow
python3 scripts/make_icon.py                 # the built-in design (edit draw_builtin() to change it)
python3 scripts/make_icon.py my-logo.png     # or any square PNG, 512 px or larger
```

Then rebuild or re-run the installer.

## Publishing this to GitHub (so you can download it anywhere)

```bash
git init -b main
git add .
git commit -m "Folio 0.1.0"
git remote add origin https://github.com/<you>/folio.git
git push -u origin main

git tag v0.1.0
git push origin v0.1.0
```

Pushing the tag starts `.github/workflows/release.yml`, which builds Windows and Linux packages and attaches
them to a new GitHub Release. After a few minutes that page is your download link. The `ci.yml` workflow runs the
engine tests against real PDFium on every push.

## Development

```bash
# one-time: fetch PDFium for your platform into vendor/pdfium (see install.sh for the URL pattern)
export FOLIO_PDFIUM_DIR=$PWD/vendor/pdfium/lib          # Windows: vendor\pdfium\bin

cargo test -p folio-core        # engine tests: rendering, search geometry on rotated pages, edit and undo
cargo run -p folio -- tests/fixtures/sample.pdf
```

Layout:

```
crates/folio-core/   PDF engine: PDFium on one worker thread, undo history, page edits, annotation baking
src-tauri/           the desktop app: window, native dialogs, command bridge to the engine
src/                 the interface (plain HTML, CSS and JavaScript, no build step)
scripts/make_icon.py icon generator
install.* uninstall.*  installers for Windows (ps1/cmd) and Linux (sh)
```

PDFium releases are pinned (`chromium/7543`) because the Rust bindings target a specific PDFium API level.
When you bump `pdfium-render`, change the tag in `install.sh`, `install.ps1` and both workflows together.

## Known limits (v0.1)

- Marks are baked into the page on save. You can undo them before saving, but not edit them afterwards.
- Highlight is a drawn rectangle, not text-selection based. There is no text selection or copy-by-drag yet (use "Copy text of this page").
- Signatures are pictures placed on the page (like signing a printout and scanning it). They are not cryptographic digital signatures, so they do not prove who signed or detect later changes. Saved signatures are kept in the app's local storage on this computer and are removed by the uninstaller.
- No form filling, cryptographic digital signatures, OCR, or editing of existing text.
- Merging password-protected files is not supported yet.
- Added text uses a built-in Latin font. Characters outside Latin-1 become `?`.
- Printing uses the system print dialog via the web view. It works on Windows; the Linux web view may not offer it.
- One document per window.

## License

MIT, see `LICENSE`. Third-party components are listed in `THIRD-PARTY-NOTICES.md`.
