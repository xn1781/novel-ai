# Private EPUB reader (GitHub Pages)

An independent mobile/desktop EPUB reader for encrypted books. The desktop translator in the parent project is unaffected. Includes a library, table of contents, saved reading position, and customizable theme, text/background color, font, size, spacing and reading width. Layout options: **Pages**, **Chapter scroll** (one EPUB spine section as a long vertical scroll, with Previous/Next opening the adjacent section), and **Continuous scroll** (epub.js loads adjacent sections as you scroll, for an infinite-scroll experience). EPUBs usually store one chapter per spine section, but some publishers split or combine chapters differently. Switching layouts returns to the current EPUB location.

The library has live search across title, original title, author, synopsis, tags, genres, language and publication status, plus publication/language/tag filters. Book detail pages show covers, author, synopsis and tags. All metadata and covers are inside the **encrypted catalog**, not separately published.

Reading position is saved locally per book and browser, including the current EPUB location (CFI) and exact scroll fraction in Chapter scroll. Continuous scroll resumes by CFI because its total scroll height changes as chapters load/unload. It is flushed when you leave a book or close the tab. Each novel page lists its EPUB reading-order sections with Read / In progress / Unread indicators and Reading order / Newest first / Unread first sorting. A section becomes Read when you scroll near its end in Chapter scroll, scroll forward into the next section in Continuous scroll, or use Next to advance; just opening it does not mark it read. EPUB sections usually match chapters, though some publishers split or combine them. Browser site-data clearing also clears this progress; it does not sync between devices.

Appearance includes locally bundled Open Sans and Merriweather fonts (regular, bold, italic, and bold italic). Their Latin glyphs work offline; for characters not covered by the Latin files, the selected font's system fallback is used.

The top bar, library, and side panels match the reading theme by default. Use **Appearance → Interface colors → Separate interface colors** to set the interface background and text independently of the book page; the choice is saved in this browser. Dark now defaults to a near-black, muted-text palette. Existing saved theme choices remain in effect until you select Dark or reset appearance.

## Important security limits

**GitHub Pages is public, even when the repository is private.** Never put plaintext EPUBs, passphrases, or unencrypted cover art in `reader/public/`, a GitHub Actions secret, or the site source. Only ciphertext is deployed. Anyone can download the encrypted files, and anyone with the passphrase can decrypt them. The site is not an account/login service. Use a strong *unique* passphrase (ideally a randomly generated multi-word phrase); filenames and book titles are also encrypted. File sizes, number of books, and upload timing remain public. Encryption uses PBKDF2-SHA256 (600,000 iterations) and AES-256-GCM with a fresh IV per file.

The browser stores a non-extractable CryptoKey in IndexedDB after the first successful unlock. It normally survives reloads and browser restarts, **not** clearing browser/site data, private/incognito mode, changing device/browser, or browser storage eviction. Choose **Forget this device** to remove it. Someone with access to your unlocked browser can read your books. A malicious browser extension or compromised website build can read decrypted content. Keep your account and site code secure. If you lose your passphrase but still have the original EPUBs, remove `reader/public/books/` and repack with a new passphrase; old published ciphertext remains decryptable with the old passphrase wherever it has been copied, including git history.

## Add books

Requires Node.js 20+ and npm.

1. `cd reader` then `npm install`.
2. Create `reader/books-private/` and place your `.epub` files there (only top-level `.epub` files are read). **This folder is gitignored; do not force-add it.** Back it up elsewhere. Use descriptive filenames for titles displayed in the library. Optional same-stem `.metadata.json` files can contain `title`, `author`, `original_title`, `synopsis`, `source_language`, `publication_status`, and arrays `tags`, `genres`, `raw_tags_en`, `raw_tags_ko`. To add a cover, set `cover_file` to a same-stem `.cover.png`, `.cover.jpg`, `.cover.jpeg`, `.cover.webp`, or `.cover.gif` file next to the EPUB. Metadata and cover are encrypted into the catalog.
3. Set a passphrase in your local terminal; do not put it in source control. In PowerShell:

   ```powershell
   $env:READER_PASSPHRASE = Read-Host 'Library passphrase'
   npm run pack
   Remove-Item Env:READER_PASSPHRASE
   ```

   On macOS/Linux: `read -rs READER_PASSPHRASE; export READER_PASSPHRASE; npm run pack; unset READER_PASSPHRASE`.
4. Run `npm run dev` to preview locally, or `npm run build` to verify the deployable output. Commit `reader/public/books/` (encrypted files only), `reader/package-lock.json`, and the site source, then push. Repeat the pack command with the **same passphrase** when you add/remove/rename books. Repacking creates new IVs and rewrites ciphertext. Renaming a file changes its reading-progress ID.

### Export directly from the desktop Library

For a **Completed** novel, click **Export to web reader** in its Library card and select the `reader/books-private/` folder (create it first). The app creates a translated EPUB plus a matching metadata file and optional cover there. The app must be rebuilt/restarted after installing this feature. You may keep the app running during this export: the command reads a completed translation snapshot and does not change translation state. Actively translating, queued, or incomplete books are not exportable. If you change a novel or its metadata later, export it again. Run `npm run pack` locally afterwards, then commit only `reader/public/books/` and push. **The app exports plaintext into a gitignored staging folder, not directly encrypted files. Do not commit that folder.** Old books in the staging folder remain part of the next pack until removed there.

## Publish

In repository **Settings → Pages → Build and deployment**, select **GitHub Actions**. Push to `main` (or change the workflow branch if needed). The `reader-pages.yml` workflow builds and publishes `reader/dist` at `https://YOUR_USERNAME.github.io/YOUR_REPOSITORY/`. It intentionally has no access to your plaintext EPUBs or passphrase. Pages availability for private repositories depends on your GitHub plan; consult your GitHub Pages settings. For a `username.github.io` repository the site is at the domain root; change `reader/vite.config.js` base to `/` in that special case.

Do not add another Pages deployment workflow for this repository; GitHub Pages has one published site per repository. If the project already has a Pages deployment, combine the workflows rather than deploying both.

EPUBs with JavaScript/DRM or unusual publisher CSS may not render perfectly; this reader disables scripted content and overrides basic typography. Progress and preferences are per browser. Encrypted book downloads are held in memory during reading, so very large EPUBs may use substantial RAM on mobile.