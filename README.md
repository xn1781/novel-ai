# Novel Translator

Looking for the separate encrypted GitHub Pages EPUB reader? See [reader/README.md](reader/README.md).

A local desktop app for translating Korean or Japanese books into English with Ollama. The Tauri shell and document/translation backend are written in Rust; the bundled interface is a small Vite frontend.

## Requirements

- Windows 10/11 with Microsoft Edge WebView2 Runtime
- Rust stable and the MSVC C++ build tools
- Node.js 20+ and npm
- Ollama running locally with at least one installed model

## Run in development

```powershell
npm install
npm run tauri dev
```

The app window opens while that command is running. Leave the terminal open; press `Ctrl+C` there to stop the development app.

## Run the Windows app

Install either the MSI or the EXE installer from `src-tauri\target\release\bundle`, then launch **Novel Translator** from the Windows Start menu. You can also run `src-tauri\target\release\novel-ai-translator.exe` directly. The compiled app does not need a terminal to stay open.

In Ollama, install a model that supports the source language and has enough context for your book. The app discovers models available from `http://localhost:11434`; use **Refresh** after adding a model.

## Use

1. Choose one EPUB or PDF, or select multiple TXT files. TXT files are combined in the displayed order; use the arrows to reorder them.
2. Leave source language on **Auto-detect** to identify Japanese from kana and Korean from Hangul; if the text does not clearly identify either script, the app uses the default language in Settings. You can also select Japanese or Korean manually. Choose the output format (EPUB or TXT), a genre, one or more tones, and any specific translation instructions.
3. Select an installed Ollama model and start translation. Progress is saved after each chunk and incomplete jobs can be loaded from the saved-job list. Use **Remove** beside that list to delete a saved job. After the first chunk, the app estimates remaining time from the selected model's average observed response time and updates it as translation continues.
4. Save the completed translation as EPUB or TXT.

## Library, queue, and glossary

- Library novels have editable tags, genres, author, original title, synopsis and **publication status** (Unknown, Ongoing, Completed, Hiatus or Dropped). Publication status is independent of the automatic translation workflow status. Library search matches these details, both RAW Novelpia tag languages, language and translation status; use the three dropdowns to filter publication status, translation status and source language.
- Use **Add chapters** on a Library novel to paste a titled source-language chapter or append one EPUB/PDF or multiple ordered TXT files. Existing translated chapters and story context are retained. You cannot append while the novel is queued or actively translating. Queue it again to translate just its new chapters; a completed novel with untranslated additions must finish those additions before it can be exported again. Pasted chapters remain in the Library as TXT sources.
- The Library details panel can rename a novel even during translation; **Edit chapter and section titles** can likewise save edits during a running job. These edits are kept separately so the translator cannot overwrite them, and the next EPUB export uses the new titles. A Queue card shows the chapter currently being worked on, chunk progress, estimated remaining time, and accumulated translation/context processing time (not total wall-clock time). Its model picker can change the queued model without resetting progress; during translation it switches at the next untranslated chapter, not in the middle of a chapter.
- Paste a direct `https://www.novelupdates.com/series/.../` link into a Library novel's details and choose **Fetch Novel Updates details** to preview and apply author, synopsis, tags, genres, original title, status and an available cover. Novel Updates tag lists can contain over 40 entries; up to 500 unique tags or genres are retained. Before applying, optionally check **Replace library title** to use Novel Updates' title as the novel's main title; leave it unchecked to preserve your current title. You can still enter metadata manually.
- If Novel Updates returns HTTP 403 to the direct request, **Fetch Novel Updates details** automatically opens a temporary app browser window at the pasted series link and reads the series page when it finishes loading. If the site requires sign-in or verification, complete it in that window; it may still block the embedded browser. Review the extracted details and choose **Apply metadata**. The cover is fetched separately and may be unavailable if image requests are blocked. No saved HTML file is needed.
- **Find RAW** searches the online catalogue at `https://www.raw-fucknovelpia.com/index.php`. Preview synopsis, author, publication status, genres, site tags in both languages and the cover before importing. Import downloads the password-protected ZIP, decrypts it with `taiwanandnorthkorea`, names the contained EPUB after the novel, and keeps an offline cover and metadata in Library when a cover is available. You may optionally save another renamed EPUB to a chosen location; choose a filename that does not already exist. Network access is required for searching/importing and showing search-result covers; the Library cover works offline. Downloads depend on the site's availability and may be rate-limited.
- If the site blocks the app's ZIP download but the novel's **Download RAW ZIP** link works in your browser, use **Download ZIP in browser** and then **Import downloaded ZIP** in that novel's Find RAW details. The app verifies/decrypts the selected ZIP and uses the novel's metadata and cover; this bypasses the site's automated ZIP download limit, though loading the novel page still requires network access. Invalid HTML or incomplete ZIP responses now show a specific error instead of a ZIP parser message.
- In **Find RAW**, typing at least four characters shows up to 12 matching titles from the site's own title list. Choose one with the mouse or the arrow keys and Enter to search for it; Escape dismisses the list. Suggestions are cached for 30 minutes and require the site to be reachable, but ordinary search still works if suggestions are unavailable.
- Expand **Advanced search filters** in Find RAW for the site's author, uploader, translator group, country, year range, status, language, read mode, sorting, image-chapter and audiobook filters. Include/exclude site and Novelpia tags (type a tag and press Enter to add it), choose AND/OR matching for included tags and genres, and select included/excluded genres. Advanced results include a total and support Next page. The site's advanced search also lists non-RAW novels; these can be opened on the site but not imported as RAW EPUBs. The quick search above the advanced form remains separate.

- Add novels to **Library** to copy them into the app's local data directory. Their source language is detected automatically and can be changed on each library card. Use **View illustrations** to browse embedded EPUB chapter images, or add a PNG, JPEG, GIF, or WebP cover image; covers appear in the Library and are included in that novel's translated EPUB. Removing a library novel deletes its app copy and associated saved translation, but never deletes the original files. Library entries show their translation state and can be added to the sequential queue. When queueing a library novel, the current translation-tab genre, tones, instructions, model, and output format are used.
- **Queue** displays each queued novel's status, chunk progress, and estimated remaining time. Use **Start queue** to process unfinished jobs one at a time. Failed or interrupted jobs remain available to resume.
- Add source-to-English terms in **Glossary**, separately for Japanese and Korean. Character entries can also store an explicit gender, which is included as an authoritative instruction for the model and can be corrected at any time. Each novel also has its own editable glossary in the Library, either through individual term controls or as tab-separated plain text (`source term<TAB>English term<TAB>optional note<TAB>optional character gender`). Once a novel is translated, choose **Generate from translation** in its Library card to add conservative term suggestions to its novel glossary. In the Translate tab, **Generate recommendations** displays suggestions from the completed translation; add them to the global glossary individually. Existing novel glossary terms are not overwritten by generation. A novel's terms take precedence over matching global glossary entries when that novel is translated again.
- The global Glossary can be exported as tab-separated plain text for backup or editing in another application. Completed Library novels also have a **Recommend glossary** action that runs the same source/translation analysis even when the translation was started from the Library.
- Each novel's **Story context** in the Library stores continuity facts, such as character relationships, family affiliations, titles, and worldbuilding. Enter or edit one fact per line; after translating a chapter, the local model reviews the existing context and newly observed facts, removes one-off details and duplicates, and retains the configured number of concise facts (100 by default) for later chapter prompts. Context extraction failures do not discard translated text and are reported so extraction can be retried by running the translation again.
- Use the Library search field to find novels without scrolling. Completed novels are removed from the active Queue and appear in **Completed**, with **Download** actions available directly in both the Library card and Completed tab.
- **Settings** stores the Ollama endpoint, default language, output format, model, theme, and whether to automatically retry transient translation failures. Retries are enabled by default and are limited to three retries with increasing delays; connection failures, HTTP 429/5xx responses, and incomplete model responses are retried. Other HTTP errors fail immediately. Translation requests disable Ollama's separate thinking output so it does not consume the response instead of the visible translation. The header moon/sun button toggles dark mode. The editable main prompt supports `{source_language}`, `{genre}`, `{tones}`, `{instructions}`, `{context}`, and `{glossary}` placeholders. If `{context}` is omitted from a custom prompt, the app appends novel context automatically.

Library files, translation jobs, glossary, and settings remain in the application's local data directory. By default, translation requests go to `http://localhost:11434`. If you change the Ollama address to a remote host in Settings, book text is sent to that host.

## Current format and fidelity limits

- EPUB imports extract XHTML/HTML chapters and referenced PNG, JPEG, GIF, WebP, and SVG images. Chapters are currently ordered by a natural sort of archive paths rather than the EPUB spine; the generated EPUB retains chapter text breaks and embeds extracted images, but is rebuilt with a simple stylesheet and places images after their translated chapter text. Source styling, exact image placement, and other book metadata are not retained.
- PDF imports extract readable text only. Scanned pages require OCR, and embedded PDF images/page layout are not retained.
- TXT line breaks are retained. Input files with unsupported encodings or malformed archives report an import error.
- Translation proceeds one extracted section at a time. Completed sections are persisted and resumable; an interrupted request may need to be retried for its current section.
- Long novels remain resumable across hundreds of chapters. Story context is curated after each chapter, with up to three translation chunks analyzed per Ollama request, limited by the Settings retention value, and additionally bounded in each translation prompt so context cannot consume the model's entire input window. Glossary recommendations are intentionally an on-demand operation because they review the complete source/translation pair and may take many Ollama requests for a large novel.
- Story-context analysis ignores explicit/R19 material entirely. It does not quote or summarize sexual content; when a section has no non-explicit continuity facts, the context extractor returns no facts and translation continues normally.
- If the local model refuses or returns malformed JSON during optional context analysis, that section is treated as having no new context instead of being marked for repeated retries. Existing context is preserved, and the translation remains unaffected.
- Library EPUB exports place the cover, title page, and colophon before the chapters, include a navigation document, preserve labels such as Prologue, and normalize filenames such as `chapter001` to `Chapter 1`. Use **Replace source** on a Library novel to import an updated EPUB/PDF/TXT source without deleting the Library entry; its glossary, context, output format, and cover are retained while translation progress is reset.
- Library novels include **Edit chapter and section titles**. Rename any section to labels such as `Side Story`, `Notice`, or `Prologue`; the custom title is used in the EPUB heading and navigation without retranslating the section.
- The title editor also supports bulk editing: paste one title per line in section order and choose **Save all titles**. This is useful for quickly labeling groups of imported notice or side-story sections.
- Completed Library novels include **Retranslate selected chapters**. Choose one or more sections, preview their current translations, select a model/genre/tones, and enter optional chapter-specific instructions. Only the selected sections are sent to Ollama; other sections remain unchanged. Export the novel again afterward to include the replacements.
- Settings include a global **interactive translation review** preference. Manual Translate-tab jobs and selected-chapter retranslations can use it (with a per-operation checkbox), allowing the model suggestion to be edited before saving, skipped unchanged, or auto-skipped after 30 seconds. Queue jobs never use interactive review, so long novels remain unattended.
- Library jobs let you edit a selected chapter's source content before retranslating. Saving edited content clears only that chapter's old translation and marks the saved job for retry; the other chapters remain intact.
- The same panel lets you edit a saved chapter's translated English directly. This changes export output without making another Ollama request; retranslating that chapter later will replace the manual edit.
- Output formats in this initial version are EPUB and TXT.

The app does not send documents to a remote translation service. Requests go to the Ollama service at `localhost:11434`.
