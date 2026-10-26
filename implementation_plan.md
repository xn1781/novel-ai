# Implementation Plan

Add editable novel metadata and richer Library search, allow ongoing novels to gain chapters without losing completed translations, and discover/import password-protected RAW EPUBs from the specified site.

The application is a Tauri 2 desktop app with a vanilla JavaScript/Vite frontend, Rust backend, JSON Library and job storage, and existing EPUB/PDF/TXT ingestion. Publication status must remain separate from the automatically maintained translation workflow status. RAW downloads are handled only in Rust, with fixed-host validation, offline cover storage and optional separately saved renamed EPUB.

## Types

- Extend `LibraryBook` in `c:\Users\Ucer\Documents\GitHub\novel-ai\src-tauri\src\main.rs` with serde-defaulted `tags: Vec<String>`, `publication_status: String` (Unknown/Ongoing/Completed/Hiatus/Dropped), `synopsis: String`, `author: String`, `original_title: String`, `genres: Vec<String>`, `raw_source_url: Option<String>`, `raw_tags_en: Vec<String>`, `raw_tags_ko: Vec<String>`.
- Keep `LibraryBook.status` and `Job.queue_status` for translation state; existing `Section` fields represent newly appended chapters.
- Add serializable `RawSearchResult`, `RawSearchPage`, and `RawNovelDetails` structs describing cards, pagination, metadata, download URL and cover. Do not persist download tokens.

## Files

- Modify `c:\Users\Ucer\Documents\GitHub\novel-ai\src-tauri\src\main.rs`: metadata CRUD, append commands and shared helpers, RAW HTML parsing/search/download/import, tests and Tauri command registration.
- Modify `c:\Users\Ucer\Documents\GitHub\novel-ai\src\main.js`: Library search/filter/metadata/chapter controls and paginated RAW search/detail/import UI.
- Modify `c:\Users\Ucer\Documents\GitHub\novel-ai\index.html`: search filters and RAW discovery tab.
- Modify `c:\Users\Ucer\Documents\GitHub\novel-ai\src\style.css`: responsive metadata/RAW/cover/chapter styles.
- Modify `c:\Users\Ucer\Documents\GitHub\novel-ai\src-tauri\Cargo.toml` and lockfile only if ZIP AES is needed; otherwise reuse the current zip crate.
- Modify `c:\Users\Ucer\Documents\GitHub\novel-ai\README.md`: user instructions and limitations. No moves or deletions.

## Functions

- Add `update_library_metadata(app, book_id, tags, publication_status, synopsis, author, original_title, genres)` in `main.rs`, with limits, normalization and validation. Modify `import_library_book` and `replace_library_book` to initialize/preserve metadata and attribution; modify `refreshLibrary()` in `main.js` to search title, native title, author, synopsis, tags, genres, source language, publication and translation states, and render editor and explicit filters.
- Add `append_library_chapter(app, book_id, title, text)` and `append_library_chapter_files(app, book_id, paths)` and shared append logic: validate first, persist source copies, preserve old translated sections, update counts and queue/completed states, reject active jobs, handle EPUB images without collisions and ensure illustrations/exports work.
- Add `search_raw_novels(query, page)` to fetch/parse fixed-host SSR results and pagination; `fetch_raw_novel_details(novel_url)` to parse title/native title, author, description, status, cover, both groups of tags, genres and current download link; `import_raw_novel(app, novel_url, optional_output_path)` to refetch the token, fetch bounded ZIP, decrypt with `taiwanandnorthkorea`, safely select/rename EPUB, import into Library, save metadata and offline cover, and optionally save renamed EPUB elsewhere. Add testable parsing, validation, ZIP and normalization helpers. Modify frontend to render safely with `textContent` and handle pagination, preview and optional output choice.

## Classes

No new classes; use existing Rust structs and plain JavaScript functions.

## Dependencies

Reuse `reqwest`, `scraper`, `zip`, Tauri and JavaScript dependencies. Enable `zip/aes-crypto` only if the RAW archive requires it. No new frontend framework or database. Keep remote HTTP/image access in Rust; frontend uses local asset URLs.

## Testing

- Add Rust tests for old metadata defaults, normalization/status validation, HTML fixtures and pagination, URL restrictions, ZIP bad password/malformed/path-traversal/size cases, chapter append state and export preservation.
- Manually exercise metadata/search, chapter paste/file import, RAW search and preview, import, optional save, and offline covers where possible.
- Run `cargo fmt --check`, `cargo test`, `node --check src/main.js`, `npm run build`; baseline: 29 Rust tests pass and frontend build succeeds.

## Implementation Order

1. Persist compatible metadata and build Library search/edit UI.
2. Implement pasted/file chapter append and job/asset state handling with tests and UI.
3. Implement and test fixed-host RAW search and detail parsers.
4. Implement bounded ZIP decrypt/import and local covers/optional save.
5. Complete RAW UI, documentation, and end-to-end validation.