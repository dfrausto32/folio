# Third-party notices

Folio is built on the following open source components.

| Component | Used for | License |
|---|---|---|
| [PDFium](https://pdfium.googlesource.com/pdfium/) | Rendering, text search, page editing | BSD-3-Clause / Apache-2.0 |
| [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) | Prebuilt PDFium libraries downloaded by the installers | MIT |
| [pdfium-render](https://github.com/ajrcarey/pdfium-render) | Rust bindings for PDFium | MIT / Apache-2.0 |
| [Tauri](https://tauri.app) and plugins | Application shell and native dialogs | MIT / Apache-2.0 |
| [image](https://github.com/image-rs/image) | PNG encoding of rendered pages | MIT / Apache-2.0 |
| [base64](https://github.com/marshallpierce/rust-base64) | Decoding signature pictures sent by the UI | MIT / Apache-2.0 |
| [Dancing Script](https://github.com/impallari/DancingScript) and [Caveat](https://github.com/googlefonts/caveat) fonts | Typed signatures (`src/fonts`) | SIL Open Font License 1.1 (texts in `src/fonts`) |
| [serde](https://serde.rs) | Data exchange between UI and engine | MIT / Apache-2.0 |

The PDFium library that ships beside the Folio program carries its own license files
(see the `licenses` folder of the pdfium-binaries download). Transitive Rust dependencies are
listed in `Cargo.lock`; run `cargo install cargo-about` and `cargo about generate` for a full report.
