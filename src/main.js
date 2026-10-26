import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import "./style.css";

const $ = (selector) => document.querySelector(selector);
const files = [];
let activeJob = null;
let busy = false;
let queueRunning = false;
let interactiveTimer = null;
listen("interactive-translation-question", async ({ payload }) => {
  const dialog = $("#interactive-translation-dialog");
  const answer = $("#interactive-question-answer");
  const timer = $("#interactive-question-timer");
  $("#interactive-question-title").textContent = `${payload.section_title} · chunk ${payload.chunk_index + 1}`;
  $("#interactive-question-source").textContent = `Source:\n${payload.source}\n\nSuggested translation:\n${payload.suggestion}`;
  answer.value = payload.suggestion;
  dialog.showModal();
  let remaining = payload.timeout_seconds;
  timer.textContent = `Automatically using the suggestion in ${remaining} seconds.`;
  clearInterval(interactiveTimer);
  interactiveTimer = setInterval(() => {
    remaining -= 1;
    timer.textContent = `Automatically using the suggestion in ${remaining} seconds.`;
    if (remaining <= 0) {
      clearInterval(interactiveTimer);
      dialog.close("skip");
    }
  }, 1000);
  const resolve = async (value) => {
    clearInterval(interactiveTimer);
    if (dialog.open) dialog.close();
    await invoke("answer_interactive_translation", { questionId: payload.id, answer: value });
  };
  $("#interactive-skip").onclick = () => resolve("");
  $("#interactive-use").onclick = (event) => {
    event.preventDefault();
    resolve(answer.value);
  };
  dialog.addEventListener("close", () => {
    if (dialog.returnValue === "skip") resolve("");
  }, { once: true });
});
let settings = {
  ollama_endpoint: "http://localhost:11434",
  default_source_language: "Japanese",
  default_output_format: "epub",
  default_model: "",
  translation_prompt: "",
  dark_mode: false,
  auto_retry: true,
  interactive_translation: false,
};

function setStatus(message, error = false) {
  $("#status-message").textContent = message;
  $("#status-message").classList.toggle("error", error);
}

function libraryBookDirectory(book) {
  return book.files[0]?.replace(/[\\/][^\\/]*$/, "") ?? "";
}

async function downloadLibraryBook(book, messageSelector = "#library-message") {
  if (!book.job_id) {
    $(messageSelector).textContent = "This novel has no completed translation to download.";
    return;
  }
  const job = await invoke("get_job", { jobId: book.job_id });
  const outputFormat = job.output_format || book.output_format || "epub";
  const path = await save({
    defaultPath: `${book.title}-en.${outputFormat}`,
    filters: [{ name: outputFormat.toUpperCase(), extensions: [outputFormat] }],
  });
  if (!path) return;
  await invoke("export_job", { jobId: book.job_id, outputPath: path });
  $(messageSelector).textContent = `Saved ${book.title} to ${path}`;
}

async function showIllustrations(book) {
  const dialog = $("#illustration-dialog");
  const gallery = $("#illustration-gallery");
  const message = $("#illustration-message");
  gallery.replaceChildren();
  message.textContent = "Loading illustrations…";
  dialog.showModal();
  try {
    const illustrations = await invoke("list_library_illustrations", { bookId: book.id });
    if (!illustrations.length) {
      message.textContent = "No embedded illustrations were found. Image extraction is available for EPUB files.";
      return;
    }
    message.textContent = `${illustrations.length} illustration${illustrations.length === 1 ? "" : "s"} in ${book.title}.`;
    for (const illustration of illustrations) {
      const figure = document.createElement("figure");
      figure.className = "illustration-item";
      const image = document.createElement("img");
      image.src = convertFileSrc(illustration.path);
      image.alt = illustration.alt || `Illustration from ${illustration.section_title}`;
      image.loading = "lazy";
      const caption = document.createElement("figcaption");
      caption.textContent = illustration.alt
        ? `${illustration.section_title} · ${illustration.alt}`
        : illustration.section_title;
      figure.append(image, caption);
      gallery.append(figure);
    }
  } catch (error) {
    message.textContent = `Could not load illustrations: ${error}`;
  }
}

async function generateRecommendations() {
  if (!activeJob) return;
  const button = $("#generate-recommendations");
  const list = $("#recommendations-list");
  const message = $("#recommendations-message");
  button.disabled = true;
  button.textContent = "Generating…";
  message.textContent = "Checking the completed translation with your local model…";
  list.replaceChildren();
  try {
    const suggestions = await invoke("generate_job_glossary_suggestions", { jobId: activeJob.id });
    if (!suggestions.length) {
      message.textContent = "No clear glossary terms were found.";
      return;
    }
    message.textContent = `${suggestions.length} suggestions. Add only the terms you want in your global glossary.`;
    for (const suggestion of suggestions) {
      const row = document.createElement("article");
      row.className = "content-card recommendation-card";
      const details = document.createElement("div");
      details.className = "card-details";
      const term = document.createElement("h3");
      term.textContent = `${suggestion.source_term} → ${suggestion.target_term}`;
      const note = document.createElement("p");
      note.textContent = [
        suggestion.note,
        suggestion.character_gender ? `Character gender: ${suggestion.character_gender}` : "",
        suggestion.source_language,
      ].filter(Boolean).join(" · ");
      details.append(term, note);
      const add = document.createElement("button");
      add.type = "button";
      add.className = "button button-secondary";
      add.textContent = "Add to glossary";
      add.addEventListener("click", async () => {
        add.disabled = true;
        try {
          await invoke("save_glossary_entry", {
            id: null,
            sourceLanguage: suggestion.source_language,
            sourceTerm: suggestion.source_term,
            targetTerm: suggestion.target_term,
            note: suggestion.note,
            characterGender: suggestion.character_gender || "",
          });
          add.textContent = "Added";
        } catch (error) {
          add.disabled = false;
          message.textContent = `Could not add "${suggestion.source_term}": ${error}`;
        }
      });
      row.append(details, add);
      list.append(row);
    }
  } catch (error) {
    message.textContent = `Could not generate recommendations: ${error}`;
  } finally {
    button.disabled = false;
    button.textContent = "Generate recommendations";
  }
}

async function loadSettings() {
  try {
    settings = await invoke("get_app_settings");
    $("#settings-endpoint").value = settings.ollama_endpoint;
    $("#settings-prompt").value = settings.translation_prompt;
    $("#settings-language").value = settings.default_source_language;
    $("#settings-output").value = settings.default_output_format;
    $("#settings-dark-mode").checked = settings.dark_mode;
    $("#settings-auto-retry").checked = settings.auto_retry;
    $("#settings-interactive-translation").checked = settings.interactive_translation;
    $("#settings-context-limit").value = settings.context_limit ?? 100;
    $("#source-language").value = "Auto-detect";
    applyTheme(settings.dark_mode);
  } catch (error) {
    $("#settings-message").textContent = `Could not load settings: ${error}`;
  }
}

async function refreshLibrary() {
  try {
    const books = await invoke("list_library");
    const query = $("#library-search").value.trim().toLowerCase();
    const publicationFilter = $("#library-publication-filter").value;
    const translationFilter = $("#library-translation-filter").value;
    const languageFilter = $("#library-language-filter").value;
    const visibleBooks = books.filter((book) => {
      if (publicationFilter && (book.publication_status || "Unknown") !== publicationFilter) return false;
      if (translationFilter && book.status !== translationFilter) return false;
      if (languageFilter && book.source_language !== languageFilter) return false;
      return !query || [book.title, book.original_title, book.author, book.synopsis, book.source_language,
        book.publication_status, book.status, ...(book.tags || []), ...(book.genres || []),
        ...(book.raw_tags_en || []), ...(book.raw_tags_ko || [])].some((value) => (value || "").toLowerCase().includes(query));
    });
    const list = $("#library-list");
    const openGlossaries = new Set(
      [...list.querySelectorAll("details.book-glossary[open]")].map((section) => section.dataset.bookId),
    );
    list.replaceChildren();
    $("#library-empty").hidden = visibleBooks.length > 0;
    $("#library-empty").textContent = books.length && !visibleBooks.length
      ? "No novels match your search."
      : "Your library is empty. Add an EPUB, PDF, or one or more TXT files to get started.";
    for (const book of visibleBooks) {
      const card = document.createElement("article");
      card.className = "content-card library-card";
      const details = document.createElement("div");
      details.className = "card-details";
      if (book.cover_image) {
        details.classList.add("with-cover");
        const cover = document.createElement("img");
        cover.className = "library-cover";
        cover.src = convertFileSrc(`${libraryBookDirectory(book)}\\${book.cover_image}`);
        cover.alt = `Cover for ${book.title}`;
        details.append(cover);
      }
      const title = document.createElement("h3");
      title.textContent = book.title;
      const meta = document.createElement("p");
      meta.textContent = `${book.source_language} · ${book.publication_status || "Unknown"} publication · ${book.files.length} file${book.files.length === 1 ? "" : "s"}`;
      details.append(title, meta);
      if (book.original_title) { const original = document.createElement("p"); original.textContent = book.original_title; details.append(original); }
      if (book.author) { const author = document.createElement("p"); author.textContent = `Author: ${book.author}`; details.append(author); }
      if (book.synopsis) { const synopsis = document.createElement("p"); synopsis.textContent = book.synopsis; details.append(synopsis); }
      for (const [label, values] of [["Tags", book.tags], ["Genres", book.genres], ["Novelpia EN", book.raw_tags_en], ["Novelpia KO", book.raw_tags_ko]]) {
        if (values?.length) { const row = document.createElement("p"); row.textContent = `${label}: ${values.join(", ")}`; details.append(row); }
      }
      const languageControls = document.createElement("label");
      languageControls.className = "library-language";
      languageControls.textContent = "Source language";
      const languageSelect = document.createElement("select");
      languageSelect.setAttribute("aria-label", `Source language for ${book.title}`);
      languageSelect.add(new Option("Japanese", "Japanese"));
      languageSelect.add(new Option("Korean", "Korean"));
      languageSelect.value = book.source_language;
      languageSelect.disabled = book.status === "Translating";
      languageSelect.addEventListener("change", async () => {
        try {
          await invoke("update_library_source_language", {
            bookId: book.id,
            sourceLanguage: languageSelect.value,
          });
          $("#library-message").textContent = `${book.title} source language updated. Any previous translation progress was cleared.`;
          await Promise.all([refreshLibrary(), refreshQueue()]);
        } catch (error) {
          $("#library-message").textContent = `Could not change source language: ${error}`;
          await refreshLibrary();
        }
      });
      languageControls.append(languageSelect);
      details.append(languageControls);
      const status = document.createElement("span");
      status.className = `status-pill ${book.status.toLowerCase().replaceAll(" ", "-")}`;
      status.textContent = book.status;
      const actions = document.createElement("div");
      actions.className = "card-actions";
      const illustrationsButton = document.createElement("button");
      illustrationsButton.type = "button";
      illustrationsButton.className = "button button-secondary";
      illustrationsButton.textContent = "View illustrations";
      illustrationsButton.addEventListener("click", () => showIllustrations(book));
      const coverButton = document.createElement("button");
      coverButton.type = "button";
      coverButton.className = "button button-secondary";
      coverButton.textContent = book.cover_image ? "Change cover" : "Add cover";
      coverButton.title = "Add a cover image to this novel and its translated EPUB.";
      coverButton.addEventListener("click", async () => {
        try {
          const selected = await open({
            multiple: false,
            title: `Choose a cover for ${book.title}`,
            filters: [{ name: "Cover images", extensions: ["png", "jpg", "jpeg", "gif", "webp"] }],
          });
          if (!selected) return;
          await invoke("set_library_cover_image", { bookId: book.id, sourcePath: selected });
          $("#library-message").textContent = `Cover image saved for ${book.title}.`;
          await refreshLibrary();
        } catch (error) {
          $("#library-message").textContent = `Could not add cover image: ${error}`;
        }
      });
      actions.append(illustrationsButton, coverButton);
      const replaceButton = document.createElement("button");
      replaceButton.type = "button";
      replaceButton.className = "button button-secondary";
      replaceButton.textContent = "Replace source";
      replaceButton.disabled = book.status === "Translating" || book.status === "Queued";
      replaceButton.title = "Replace the imported source files and reset this novel's translation.";
      replaceButton.addEventListener("click", async () => {
        try {
          const selected = await open({
            multiple: true,
            title: `Replace source files for ${book.title}`,
            filters: [{ name: "Book files", extensions: ["epub", "pdf", "txt"] }],
          });
          if (!selected) return;
          const paths = Array.isArray(selected) ? selected : [selected];
          await invoke("replace_library_book", {
            bookId: book.id,
            paths,
            sourceLanguage: book.source_language,
          });
          $("#library-message").textContent = `${book.title} source files replaced. Translation progress was reset; glossary and story context were kept.`;
          await Promise.all([refreshLibrary(), refreshQueue(), refreshSavedJobs()]);
        } catch (error) {
          $("#library-message").textContent = `Could not replace source files: ${error}`;
        }
      });
      actions.append(replaceButton);
      let titlesSection = null;
      let retranslateSection = null;
      if (book.job_id) {
        titlesSection = document.createElement("details");
        titlesSection.className = "book-titles";
        const titlesSummary = document.createElement("summary");
        titlesSummary.textContent = "Edit chapter and section titles";
        const titlesHint = document.createElement("p");
        titlesHint.className = "hint";
        titlesHint.textContent = "Use labels such as Prologue, Side Story, Notice, or Chapter 1. Edits during translation are saved separately and used on the next EPUB export without retranslating.";
        const titlesMessage = document.createElement("p");
        titlesMessage.className = "book-titles-message";
        titlesSection.append(titlesSummary, titlesHint, titlesMessage);
        titlesSection.addEventListener("toggle", async () => {
          if (!titlesSection.open || titlesSection.dataset.loaded) return;
          try {
            const job = await invoke("get_job", { jobId: book.job_id });
            const bulkLabel = document.createElement("label");
            bulkLabel.className = "book-titles-bulk-label";
            bulkLabel.textContent = "Bulk edit — one title per line, in section order";
            const bulkArea = document.createElement("textarea");
            bulkArea.className = "book-titles-bulk";
            bulkArea.rows = Math.min(12, Math.max(4, job.sections.length));
            bulkArea.value = job.sections.map((section) => section.title).join("\n");
            bulkArea.setAttribute("aria-label", `All section titles for ${book.title}`);
            const bulkSave = document.createElement("button");
            bulkSave.type = "button";
            bulkSave.className = "button button-secondary";
            bulkSave.textContent = "Save all titles";
            bulkSave.addEventListener("click", async () => {
              const titles = bulkArea.value.split(/\r?\n/);
              bulkSave.disabled = true;
              try {
                await invoke("update_job_section_titles", { jobId: book.job_id, titles });
                titlesMessage.textContent = `Saved all ${titles.length} section titles.`;
                titlesSection.open = false;
              } catch (error) {
                titlesMessage.textContent = `Could not save all titles: ${error}`;
              } finally {
                bulkSave.disabled = false;
              }
            });
            titlesSection.append(bulkLabel, bulkArea, bulkSave);
            titlesSection.dataset.loaded = "true";
          } catch (error) {
            titlesMessage.textContent = `Could not load section titles: ${error}`;
          }
        });
      }
      if (book.job_id) {
        retranslateSection = document.createElement("details");
        retranslateSection.className = "book-retranslate";
        const retranslateSummary = document.createElement("summary");
        retranslateSummary.textContent = "Edit and retranslate chapters";
        const retranslateHint = document.createElement("p");
        retranslateHint.className = "hint";
        retranslateHint.textContent = "Only the selected chapter will be sent to Ollama. These options apply only to this retranslation.";
        const retranslateControls = document.createElement("div");
        retranslateControls.className = "book-retranslate-controls";
        const chapterSelect = document.createElement("select");
        chapterSelect.multiple = true;
        chapterSelect.size = 6;
        chapterSelect.setAttribute("aria-label", `Chapter to retranslate in ${book.title}`);
        const chapterPreview = document.createElement("pre");
        chapterPreview.className = "book-retranslate-preview";
        chapterPreview.textContent = "Select one or more chapters to preview their current translation.";
        const sourceEditor = document.createElement("textarea");
        sourceEditor.className = "book-retranslate-source";
        sourceEditor.rows = 8;
        sourceEditor.placeholder = "Select exactly one chapter to edit its source content.";
        sourceEditor.disabled = true;
        const saveSourceButton = document.createElement("button");
        saveSourceButton.type = "button";
        saveSourceButton.className = "button button-secondary";
        saveSourceButton.textContent = "Save edited chapter content";
        saveSourceButton.disabled = true;
        const translatedEditor = document.createElement("textarea");
        translatedEditor.className = "book-retranslate-translation";
        translatedEditor.rows = 8;
        translatedEditor.placeholder = "Select exactly one chapter to edit its English translation.";
        translatedEditor.disabled = true;
        const saveTranslatedButton = document.createElement("button");
        saveTranslatedButton.type = "button";
        saveTranslatedButton.className = "button button-secondary";
        saveTranslatedButton.textContent = "Save edited translation";
        saveTranslatedButton.disabled = true;
        const genreInput = document.createElement("input");
        genreInput.type = "text";
        genreInput.maxLength = 120;
        genreInput.placeholder = "Genre for this chapter (optional)";
        genreInput.setAttribute("aria-label", `Genre for retranslation of ${book.title}`);
        const modelSelect = document.createElement("select");
        modelSelect.setAttribute("aria-label", `Ollama model for retranslation of ${book.title}`);
        const toneOptions = document.createElement("div");
        toneOptions.className = "retranslate-tone-options";
        const tones = [
          ["Natural and fluent", "Natural"],
          ["Literary and evocative", "Literary"],
          ["Faithful and precise", "Faithful"],
          ["Warm and intimate", "Warm"],
          ["Tense and dramatic", "Dramatic"],
          ["Light and witty", "Witty"],
        ];
        tones.forEach(([value, label]) => {
          const toneLabel = document.createElement("label");
          toneLabel.className = "tone-chip";
          const checkbox = document.createElement("input");
          checkbox.type = "checkbox";
          checkbox.value = value;
          const text = document.createElement("span");
          text.textContent = label;
          toneLabel.append(checkbox, text);
          toneOptions.append(toneLabel);
        });
        const instructionArea = document.createElement("textarea");
        instructionArea.rows = 4;
        instructionArea.placeholder = "Optional instructions, e.g. preserve this character's gender, use a colder tone, or fix the name spelling.";
        instructionArea.setAttribute("aria-label", `Retranslation instructions for ${book.title}`);
        const interactiveReview = document.createElement("label");
        interactiveReview.className = "interactive-toggle";
        const interactiveCheckbox = document.createElement("input");
        interactiveCheckbox.type = "checkbox";
        interactiveCheckbox.checked = settings.interactive_translation;
        const interactiveText = document.createElement("span");
        interactiveText.textContent = "Ask me to review each selected chapter chunk (30-second auto-skip)";
        interactiveReview.append(interactiveCheckbox, interactiveText);
        const retranslateButton = document.createElement("button");
        retranslateButton.type = "button";
        retranslateButton.className = "button button-secondary";
        retranslateButton.textContent = "Retranslate selected chapter";
        const retranslateMessage = document.createElement("p");
        retranslateMessage.className = "book-retranslate-message";
        retranslateSection.append(
          retranslateSummary,
          retranslateHint,
          retranslateControls,
          chapterPreview,
          sourceEditor,
          saveSourceButton,
          translatedEditor,
          saveTranslatedButton,
          instructionArea,
          interactiveReview,
          retranslateButton,
          retranslateMessage,
        );
        retranslateSection.addEventListener("toggle", async () => {
          if (!retranslateSection.open || chapterSelect.options.length) return;
          try {
            const job = await invoke("get_job", { jobId: book.job_id });
            genreInput.value = job.genre || "";
            const availableModels = [...$("#model").options].map((option) => ({
              value: option.value,
              label: option.textContent,
            })).filter((option) => option.value);
            modelSelect.replaceChildren(...availableModels.map((option) => new Option(option.label, option.value)));
            if (!availableModels.length) {
              const option = new Option(job.model || "Use the novel's model", job.model || "");
              modelSelect.append(option);
            }
            modelSelect.value = job.model || modelSelect.options[0]?.value || "";
            const savedTones = new Set(job.tones || []);
            toneOptions.querySelectorAll("input").forEach((input) => {
              input.checked = savedTones.has(input.value);
            });
            job.sections.forEach((section, index) => {
              const option = document.createElement("option");
              option.value = index;
              option.textContent = `${index + 1}. ${section.title}`;
              chapterSelect.append(option);
            });
            retranslateControls.append(chapterSelect, genreInput, modelSelect, toneOptions);
            const updatePreview = () => {
              const selected = [...chapterSelect.selectedOptions].map((option) => Number(option.value));
              const canEdit = selected.length === 1;
              sourceEditor.disabled = !canEdit;
              saveSourceButton.disabled = !canEdit;
              translatedEditor.disabled = !canEdit;
              saveTranslatedButton.disabled = !canEdit;
              sourceEditor.value = canEdit ? job.sections[selected[0]].text : "";
              translatedEditor.value = canEdit ? job.sections[selected[0]].translated || "" : "";
              chapterPreview.textContent = selected.length
                ? selected.map((index) => {
                  const section = job.sections[index];
                  const translated = section.translated || "(No translation saved)";
                  return `${index + 1}. ${section.title}\n\n${translated.slice(0, 1800)}${translated.length > 1800 ? "\n[…]": ""}`;
                }).join("\n\n━━━━━━━━━━━━━━━━━━━━\n\n")
                : "Select one or more chapters to preview their current translation.";
            };
            chapterSelect.addEventListener("change", updatePreview);
            updatePreview();
            saveSourceButton.addEventListener("click", async () => {
              const selected = [...chapterSelect.selectedOptions].map((option) => Number(option.value));
              if (selected.length !== 1) return;
              saveSourceButton.disabled = true;
              try {
                const updated = await invoke("update_job_section_text", {
                  jobId: book.job_id,
                  sectionIndex: selected[0],
                  text: sourceEditor.value,
                });
                saveTranslatedButton.addEventListener("click", async () => {
                  const selected = [...chapterSelect.selectedOptions].map((option) => Number(option.value));
                  if (selected.length !== 1) return;
                  saveTranslatedButton.disabled = true;
                  try {
                    const updated = await invoke("update_job_section_translation", {
                      jobId: book.job_id,
                      sectionIndex: selected[0],
                      translated: translatedEditor.value,
                    });
                    job.sections = updated.sections;
                    retranslateMessage.textContent = "Edited translation saved. Export the novel again to use it; retranslating this chapter later will replace your edit.";
                    updatePreview();
                    await Promise.all([refreshLibrary(), refreshSavedJobs()]);
                  } catch (error) {
                    retranslateMessage.textContent = `Could not save edited translation: ${error}`;
                  } finally {
                    saveTranslatedButton.disabled = selected.length !== 1;
                  }
                });
                job.sections = updated.sections;
                retranslateMessage.textContent = "Chapter content saved. Its previous translation was cleared; retranslate it to update the output.";
                updatePreview();
                await Promise.all([refreshLibrary(), refreshSavedJobs()]);
              } catch (error) {
                retranslateMessage.textContent = `Could not save chapter content: ${error}`;
              } finally {
                saveSourceButton.disabled = selected.length !== 1;
              }
            });
          } catch (error) {
            retranslateMessage.textContent = `Could not load chapters: ${error}`;
          }
        });
        retranslateButton.addEventListener("click", async () => {
          const selectedIndices = [...chapterSelect.selectedOptions].map((option) => Number(option.value));
          if (!selectedIndices.length) {
            retranslateMessage.textContent = "Choose at least one chapter first.";
            return;
          }
          retranslateButton.disabled = true;
          retranslateButton.textContent = "Retranslating…";
          retranslateMessage.textContent = `${selectedIndices.length} chapter${selectedIndices.length === 1 ? "" : "s"} will be translated sequentially.`;
          try {
            await invoke("retranslate_job_sections", {
              jobId: book.job_id,
              sectionIndices: selectedIndices,
              instructions: instructionArea.value,
              genre: genreInput.value,
              tones: [...toneOptions.querySelectorAll("input:checked")].map((input) => input.value),
              model: modelSelect.value,
              endpoint: settings.ollama_endpoint,
              interactive: interactiveCheckbox.checked,
            });
            retranslateMessage.textContent = "Selected chapters retranslated and saved. Export the novel again to use them in the EPUB.";
            await Promise.all([refreshLibrary(), refreshSavedJobs()]);
          } catch (error) {
            retranslateMessage.textContent = `Could not retranslate chapter: ${error}`;
          } finally {
            retranslateButton.disabled = false;
            retranslateButton.textContent = "Retranslate selected chapter";
          }
        });
        const downloadButton = document.createElement("button");
        downloadButton.type = "button";
        downloadButton.className = "button button-secondary";
        downloadButton.textContent = "Download";
        downloadButton.title = "Save the completed translation as the selected output format.";
        downloadButton.addEventListener("click", async () => {
          try {
            await downloadLibraryBook(book);
          } catch (error) {
            $("#library-message").textContent = `Could not download ${book.title}: ${error}`;
          }
        });
        actions.append(downloadButton);
        const readerButton = document.createElement("button");
        readerButton.type = "button";
        readerButton.className = "button button-secondary";
        readerButton.textContent = "Export to web reader";
        readerButton.title = "Export a completed EPUB and its metadata to reader/books-private, then encrypt it with npm run pack.";
        readerButton.addEventListener("click", async () => {
          try {
            const outputDir = await open({ directory: true, multiple: false, title: "Select reader/books-private (gitignored; never choose reader/public)" });
            if (!outputDir) return;
            const path = await invoke("export_library_to_reader", { bookId: book.id, outputDir });
            $("#library-message").textContent = `Exported to ${path}. Run npm run pack in reader/ to encrypt before committing. Never commit books-private.`;
          } catch (error) {
            $("#library-message").textContent = `Could not export to web reader: ${error}`;
          }
        });
        actions.append(readerButton);
      }
      if (book.cover_image) {
        const removeCoverButton = document.createElement("button");
        removeCoverButton.type = "button";
        removeCoverButton.className = "remove-file";
        removeCoverButton.textContent = "Remove cover";
        removeCoverButton.addEventListener("click", async () => {
          try {
            await invoke("remove_library_cover_image", { bookId: book.id });
            $("#library-message").textContent = `Cover image removed from ${book.title}.`;
            await refreshLibrary();
          } catch (error) {
            $("#library-message").textContent = `Could not remove cover image: ${error}`;
          }
        });
        actions.append(removeCoverButton);
      }
      const translateButton = document.createElement("button");
      translateButton.type = "button";
      translateButton.className = "button button-primary";
      translateButton.textContent = book.status === "Queued" || book.status === "Translating" ? "In queue" : book.status === "Completed" ? "Translate again" : "Translate";
      translateButton.disabled = book.status === "Queued" || book.status === "Translating";
      translateButton.addEventListener("click", async () => {
        try {
          await invoke("enqueue_library_book", {
            bookId: book.id,
            genre: $("#genre").value.trim(),
            tones: [...document.querySelectorAll(".tone-options input:checked")].map((input) => input.value),
            instructions: $("#instructions").value.trim(),
            model: $("#model").value || settings.default_model,
            outputFormat: $("#output-format").value || settings.default_output_format,
          });
          $("#library-message").textContent = `${book.title} added to the translation queue.`;
          await Promise.all([refreshLibrary(), refreshQueue()]);
        } catch (error) {
          $("#library-message").textContent = `Could not queue ${book.title}: ${error}`;
        }
      });
      const removeBook = document.createElement("button");
      removeBook.type = "button";
      removeBook.className = "remove-file";
      removeBook.textContent = "Remove novel";
      removeBook.disabled = book.status === "Translating";
      removeBook.title = "Remove this book and its saved translation from the app. Original files are not deleted.";
      removeBook.addEventListener("click", async () => {
        if (!window.confirm(`Remove "${book.title}" from the library and delete its saved translation from this app? Original files are not deleted.`)) return;
        try {
          await invoke("delete_library_book", { bookId: book.id });
          $("#library-message").textContent = `${book.title} removed.`;
          await Promise.all([refreshLibrary(), refreshQueue(), refreshSavedJobs()]);
        } catch (error) {
          $("#library-message").textContent = `Could not remove novel: ${error}`;
        }
      });
      const restartButton = document.createElement("button");
      restartButton.type = "button";
      restartButton.className = "button button-secondary";
      restartButton.textContent = "Restart from beginning";
      restartButton.disabled = !book.job_id;
      restartButton.title = "Clear the saved translation and translate every section again.";
      restartButton.addEventListener("click", async () => {
        if (!window.confirm(`Restart "${book.title}" from the beginning? Saved translated text and progress will be cleared. Glossary and story context will be kept.`)) return;
        try {
          await invoke("restart_library_translation", { bookId: book.id });
          $("#library-message").textContent = `${book.title} was reset and placed at the front of the queue.`;
          await Promise.all([refreshLibrary(), refreshQueue(), refreshSavedJobs()]);
        } catch (error) {
          $("#library-message").textContent = `Could not restart translation: ${error}`;
        }
      });
      actions.append(status, translateButton, restartButton, removeBook);
      const glossarySection = document.createElement("details");
      glossarySection.className = "book-glossary";
      glossarySection.dataset.bookId = book.id;
      glossarySection.open = openGlossaries.has(book.id);
      const glossarySummary = document.createElement("summary");
      glossarySummary.textContent = `Novel glossary · ${book.glossary.length} terms`;
      glossarySection.append(glossarySummary);

      const glossaryToolbar = document.createElement("div");
      glossaryToolbar.className = "glossary-toolbar";
      const generateButton = document.createElement("button");
      generateButton.type = "button";
      generateButton.className = "button button-secondary";
      generateButton.textContent = "Generate from translation";
      generateButton.disabled = book.status !== "Completed" || !book.job_id;
      generateButton.title = generateButton.disabled
        ? "Complete this novel's translation before generating glossary suggestions."
        : "Ask the local model to suggest terms from the translated text.";
      generateButton.addEventListener("click", async () => {
        generateButton.disabled = true;
        generateButton.textContent = "Generating…";
        const message = document.querySelector(`#book-glossary-message-${CSS.escape(book.id)}`);
        message.textContent = "Reviewing source/translation pairs with the local model…";
        try {
          const result = await invoke("generate_library_glossary", { bookId: book.id });
          await refreshLibrary();
          const updatedMessage = document.querySelector(`#book-glossary-message-${CSS.escape(book.id)}`);
          if (updatedMessage) {
            const warning = result.skipped_batches
              ? ` ${result.skipped_batches} batch${result.skipped_batches === 1 ? "" : "es"} could not be parsed, possibly because of mature content.`
              : "";
            updatedMessage.textContent = result.added
              ? `Added ${result.added} glossary suggestions, including character gender where the translation made it clear. Review and edit them before translating again.${warning}`
              : `No new terms were suggested. Existing glossary terms were left unchanged.${warning}`;
          }
        } catch (error) {
          generateButton.disabled = false;
          generateButton.textContent = "Generate from translation";
          message.textContent = `Could not generate suggestions: ${error}`;
        }
      });
      glossaryToolbar.append(generateButton);
      glossarySection.append(glossaryToolbar);

      const glossaryList = document.createElement("div");
      glossaryList.className = "book-glossary-list";
      if (!book.glossary.length) {
        const empty = document.createElement("p");
        empty.className = "book-glossary-empty";
        empty.textContent = "No novel-specific terms. You can add terms or generate suggestions from a completed translation.";
        glossaryList.append(empty);
      }

      let editingId = "";
      for (const entry of book.glossary) {
        const row = document.createElement("div");
        row.className = "book-glossary-entry";
        const term = document.createElement("span");
        term.className = "book-glossary-term";
        term.textContent = `${entry.source_term} → ${entry.target_term}`;
        const note = document.createElement("span");
        note.className = "book-glossary-note";
          note.textContent = [entry.note, entry.character_gender ? `Gender: ${entry.character_gender}` : ""].filter(Boolean).join(" · ");
        const edit = document.createElement("button");
        edit.type = "button";
        edit.className = "text-button";
        edit.textContent = "Edit";
        edit.addEventListener("click", () => {
          editingId = entry.id;
          sourceInput.value = entry.source_term;
          targetInput.value = entry.target_term;
          noteInput.value = entry.note;
          genderInput.value = entry.character_gender || "";
          saveTerm.textContent = "Save changes";
          cancelEdit.hidden = false;
          sourceInput.focus();
        });
        const remove = document.createElement("button");
        remove.type = "button";
        remove.className = "remove-file";
        remove.textContent = "Delete";
        remove.addEventListener("click", async () => {
          try {
            await invoke("delete_library_glossary_entry", {
              bookId: book.id,
              entryId: entry.id,
            });
            await refreshLibrary();
          } catch (error) {
            document.querySelector(`#book-glossary-message-${CSS.escape(book.id)}`).textContent = `Could not delete term: ${error}`;
          }
        });
        row.append(term, note, edit, remove);
        glossaryList.append(row);
      }
      glossarySection.append(glossaryList);

      const plainTextEditor = document.createElement("details");
      plainTextEditor.className = "plain-glossary-editor";
      const plainTextSummary = document.createElement("summary");
      plainTextSummary.textContent = "Edit glossary as plain text";
      const plainTextHint = document.createElement("p");
      plainTextHint.className = "hint";
      plainTextHint.textContent = "One entry per line: source term, English term, optional note, and optional character gender, separated by tabs. Blank lines and lines starting with # are ignored.";
      const plainTextArea = document.createElement("textarea");
      plainTextArea.rows = 7;
      plainTextArea.setAttribute("aria-label", `Plain-text glossary for ${book.title}`);
      plainTextArea.value = book.glossary
        .map((entry) => [entry.source_term, entry.target_term, entry.note, entry.character_gender].filter((value, index) => index < 2 || value).join("\t"))
        .join("\n");
      const plainTextSave = document.createElement("button");
      plainTextSave.type = "button";
      plainTextSave.className = "button button-secondary";
      plainTextSave.textContent = "Save text glossary";
      plainTextSave.addEventListener("click", async () => {
        try {
          await invoke("save_library_glossary_text", {
            bookId: book.id,
            text: plainTextArea.value,
          });
          $("#library-message").textContent = "Plain-text glossary saved.";
          await refreshLibrary();
        } catch (error) {
          $("#library-message").textContent = `Could not save plain-text glossary: ${error}`;
        }
      });
      plainTextEditor.append(plainTextSummary, plainTextHint, plainTextArea, plainTextSave);
      glossarySection.append(plainTextEditor);

      const form = document.createElement("div");
      form.className = "book-glossary-form";
      const sourceInput = document.createElement("input");
      sourceInput.type = "text";
      sourceInput.maxLength = 160;
      sourceInput.placeholder = book.source_language === "Japanese" ? "Japanese term" : "Korean term";
      sourceInput.setAttribute("aria-label", `${book.source_language} glossary term`);
      const targetInput = document.createElement("input");
      targetInput.type = "text";
      targetInput.maxLength = 160;
      targetInput.placeholder = "Preferred English";
      targetInput.setAttribute("aria-label", "Preferred English translation");
      const noteInput = document.createElement("input");
      noteInput.type = "text";
      noteInput.maxLength = 240;
      noteInput.placeholder = "Note (optional)";
      noteInput.setAttribute("aria-label", "Glossary note");
      const genderInput = document.createElement("input");
      genderInput.type = "text";
      genderInput.maxLength = 80;
      genderInput.placeholder = "Character gender (optional)";
      genderInput.setAttribute("aria-label", "Character gender");
      const saveTerm = document.createElement("button");
      saveTerm.type = "button";
      saveTerm.className = "button button-secondary";
      saveTerm.textContent = "Add term";
      const cancelEdit = document.createElement("button");
      cancelEdit.type = "button";
      cancelEdit.className = "button button-secondary";
      cancelEdit.textContent = "Cancel";
      cancelEdit.hidden = true;
      cancelEdit.addEventListener("click", () => {
        editingId = "";
        sourceInput.value = "";
        targetInput.value = "";
        noteInput.value = "";
        genderInput.value = "";
        saveTerm.textContent = "Add term";
        cancelEdit.hidden = true;
      });
      saveTerm.addEventListener("click", async () => {
        try {
          await invoke("save_library_glossary_entry", {
            bookId: book.id,
            id: editingId || null,
            sourceTerm: sourceInput.value,
            targetTerm: targetInput.value,
            note: noteInput.value,
            characterGender: genderInput.value,
          });
          await refreshLibrary();
          const message = document.querySelector(`#book-glossary-message-${CSS.escape(book.id)}`);
          if (message) message.textContent = "Novel glossary saved.";
        } catch (error) {
          document.querySelector(`#book-glossary-message-${CSS.escape(book.id)}`).textContent = `Could not save glossary term: ${error}`;
        }
      });
      form.append(sourceInput, targetInput, noteInput, genderInput, saveTerm, cancelEdit);
      glossarySection.append(form);
      const glossaryMessage = document.createElement("p");
      glossaryMessage.className = "book-glossary-message";
      glossaryMessage.id = `book-glossary-message-${book.id}`;
      glossaryMessage.setAttribute("role", "status");
      glossarySection.append(glossaryMessage);
      const contextSection = document.createElement("details");
      contextSection.className = "book-context";
      const contextSummary = document.createElement("summary");
      contextSummary.id = `book-context-summary-${book.id}`;
      contextSummary.textContent = `Story context · ${book.context.length} facts`;
      const contextHint = document.createElement("p");
      contextHint.className = "hint";
      contextHint.textContent = "One continuity fact per line. Facts you add are kept alongside facts automatically learned after each translated chapter.";
      const contextArea = document.createElement("textarea");
      contextArea.rows = 6;
      contextArea.className = "story-context-editor";
      contextArea.id = `book-context-editor-${book.id}`;
      contextArea.setAttribute("aria-label", `Story context for ${book.title}`);
      contextArea.placeholder = "For example: Mina and Jae are close friends.\nThe Han family is a noble house.";
      contextArea.value = book.context.join("\n");
      contextArea.disabled = book.status === "Translating";
      const contextSave = document.createElement("button");
      contextSave.type = "button";
      contextSave.className = "button button-secondary";
      contextSave.textContent = "Save story context";
      contextSave.disabled = book.status === "Translating";
      contextSave.addEventListener("click", async () => {
        try {
          const saved = await invoke("save_library_context", {
            bookId: book.id,
            context: contextArea.value.split(/\r?\n/).map((fact) => fact.trim()).filter(Boolean),
          });
          contextArea.value = saved.context.join("\n");
          contextSummary.textContent = `Story context · ${saved.context.length} facts`;
          contextMessage.textContent = "Story context saved.";
        } catch (error) {
          contextMessage.textContent = `Could not save story context: ${error}`;
        }
      });
      const contextMessage = document.createElement("p");
      contextMessage.className = "book-context-message";
      contextMessage.id = `book-context-message-${book.id}`;
      contextMessage.setAttribute("role", "status");
      contextSection.append(contextSummary, contextHint, contextArea, contextSave, contextMessage);
      const metadataPanel = document.createElement("details");
      metadataPanel.className = "book-metadata";
      const metadataSummary = document.createElement("summary");
      metadataSummary.textContent = "Edit tags, publication status and details";
      metadataPanel.append(metadataSummary);
      const novelTitleLabel = document.createElement("label"); novelTitleLabel.textContent = "Novel title";
      const novelTitleInput = document.createElement("input"); novelTitleInput.type = "text"; novelTitleInput.maxLength = 200;
      novelTitleInput.value = book.title;
      novelTitleLabel.append(novelTitleInput);
      const novelTitleSave = document.createElement("button"); novelTitleSave.type = "button";
      novelTitleSave.className = "button button-secondary"; novelTitleSave.textContent = "Save novel title";
      novelTitleSave.addEventListener("click", async () => {
        novelTitleSave.disabled = true;
        try {
          await invoke("update_library_title", { bookId: book.id, title: novelTitleInput.value });
          $("#library-message").textContent = "Novel title updated.";
          await Promise.all([refreshLibrary(), refreshQueue(), refreshSavedJobs()]);
        } catch (error) { $("#library-message").textContent = `Could not rename novel: ${error}`; }
        finally { novelTitleSave.disabled = false; }
      });
      metadataPanel.append(novelTitleLabel, novelTitleSave);
      const nuLabel = document.createElement("label"); nuLabel.textContent = "Novel Updates series link";
      const nuInput = document.createElement("input"); nuInput.type = "url";
      nuInput.placeholder = "https://www.novelupdates.com/series/…/";
      nuInput.value = book.novel_updates_url || "";
      nuLabel.append(nuInput);
      const nuPreview = document.createElement("p"); nuPreview.setAttribute("role", "status");
      const nuFetch = document.createElement("button"); nuFetch.type = "button";
      nuFetch.className = "button button-secondary"; nuFetch.textContent = "Fetch Novel Updates details";
      const loadNuDetails = async () => {
        const url = nuInput.value.trim();
        if (!url) { nuPreview.textContent = "Paste the Novel Updates series link first."; return; }
        nuFetch.disabled = true;
        nuPreview.textContent = "Loading series metadata…";
        try {
          let result;
          try {
            result = await invoke("fetch_novel_updates_metadata", { url });
          } catch (error) {
            if (!String(error).includes("HTTP 403")) throw error;
            nuPreview.textContent = "Novel Updates blocked the direct request. Opening the series page in the app browser…";
            result = await invoke("fetch_novel_updates_in_browser", { url });
          }
          const summary = document.createElement("span");
          summary.textContent = `${result.title} · ${result.tags.length} tags · ${result.genres.length} genres. `;
          const titleChoice = document.createElement("label");
          titleChoice.className = "nu-title-choice";
          const replaceTitle = document.createElement("input");
          replaceTitle.type = "checkbox";
          titleChoice.append(replaceTitle, document.createTextNode(`Replace library title “${book.title}” with “${result.title}”`));
          const apply = document.createElement("button"); apply.type = "button";
          apply.className = "button button-secondary"; apply.textContent = "Apply metadata";
          apply.addEventListener("click", async () => {
            apply.disabled = true;
            try {
              await invoke("apply_novel_updates_metadata", { bookId: book.id, metadata: { ...result, replace_title: replaceTitle.checked } });
              $("#library-message").textContent = `Novel Updates metadata added to ${replaceTitle.checked ? result.title : book.title}.`;
              await Promise.all([refreshLibrary(), refreshQueue(), refreshSavedJobs()]);
            } catch (error) { nuPreview.textContent = `Could not apply metadata: ${error}`; apply.disabled = false; }
          });
          nuPreview.replaceChildren(summary, titleChoice, apply);
        } catch (error) { nuPreview.textContent = `Could not fetch Novel Updates metadata: ${error}`; }
        finally { nuFetch.disabled = false; }
      };
      nuFetch.addEventListener("click", loadNuDetails);
      metadataPanel.append(nuLabel, nuFetch, nuPreview);
      const metadataFields = {};
      for (const [key, label, value] of [
        ["tags", "Tags (comma-separated)", (book.tags || []).join(", ")],
        ["genres", "Genres (comma-separated)", (book.genres || []).join(", ")],
        ["author", "Author", book.author || ""],
        ["original_title", "Original title", book.original_title || ""],
        ["synopsis", "Synopsis", book.synopsis || ""],
      ]) {
        const field = document.createElement("label"); field.textContent = label;
        const input = key === "synopsis" ? document.createElement("textarea") : document.createElement("input");
        if (key !== "synopsis") input.type = "text";
        input.value = value; field.append(input); metadataPanel.append(field); metadataFields[key] = input;
      }
      const publicationLabel = document.createElement("label"); publicationLabel.textContent = "Publication status";
      const publication = document.createElement("select");
      for (const status of ["Unknown", "Ongoing", "Completed", "Hiatus", "Dropped"]) publication.add(new Option(status, status));
      publication.value = book.publication_status || "Unknown";
      publicationLabel.append(publication); metadataPanel.append(publicationLabel);
      const metadataSave = document.createElement("button"); metadataSave.className = "button button-secondary"; metadataSave.type = "button"; metadataSave.textContent = "Save details";
      metadataSave.addEventListener("click", async () => {
        metadataSave.disabled = true;
        try {
          await invoke("update_library_metadata", { bookId: book.id, tags: metadataFields.tags.value.split(","),
            genres: metadataFields.genres.value.split(","), author: metadataFields.author.value,
            originalTitle: metadataFields.original_title.value, synopsis: metadataFields.synopsis.value,
            publicationStatus: publication.value });
          $("#library-message").textContent = `Details saved for ${book.title}.`;
          await refreshLibrary();
        } catch (error) { $("#library-message").textContent = `Could not save details: ${error}`; metadataSave.disabled = false; }
      });
      metadataPanel.append(metadataSave);
      const chapterPanel = document.createElement("details"); chapterPanel.className = "book-metadata";
      const chapterSummary = document.createElement("summary"); chapterSummary.textContent = "Add chapters";
      chapterPanel.append(chapterSummary);
      const chapterTitle = document.createElement("input"); chapterTitle.type = "text"; chapterTitle.placeholder = "Chapter title";
      const chapterText = document.createElement("textarea"); chapterText.rows = 7; chapterText.placeholder = "Paste source-language chapter text";
      const chapterSave = document.createElement("button"); chapterSave.className = "button button-secondary"; chapterSave.type = "button"; chapterSave.textContent = "Add pasted chapter";
      const chapterFiles = document.createElement("button"); chapterFiles.className = "button button-secondary"; chapterFiles.type = "button"; chapterFiles.textContent = "Append EPUB, PDF or TXT";
      const chapterMessage = document.createElement("p"); chapterMessage.setAttribute("role", "status");
      chapterSave.disabled = chapterFiles.disabled = book.status === "Translating" || book.status === "Queued";
      const chapterDone = async (command, args) => {
        try {
          await invoke(command, { bookId: book.id, ...args });
          $("#library-message").textContent = `Chapters added to ${book.title}; existing translated chapters were kept.`;
          await Promise.all([refreshLibrary(), refreshQueue(), refreshSavedJobs()]);
        } catch (error) { chapterMessage.textContent = `Could not append chapters: ${error}`; }
      };
      chapterSave.addEventListener("click", () => chapterDone("append_library_chapter", { title: chapterTitle.value, text: chapterText.value }));
      chapterFiles.addEventListener("click", async () => {
        const chosen = await open({ multiple: true, title: `Append chapters to ${book.title}`, filters: [{ name: "Chapters", extensions: ["epub", "pdf", "txt"] }] });
        if (chosen) await chapterDone("append_library_chapter_files", { paths: Array.isArray(chosen) ? chosen : [chosen] });
      });
      chapterPanel.append(chapterTitle, chapterText, chapterSave, chapterFiles, chapterMessage);
      card.append(details, actions, metadataPanel, chapterPanel, glossarySection, contextSection);
      if (titlesSection) card.append(titlesSection);
      if (retranslateSection) card.append(retranslateSection);
      list.append(card);
    }
    await refreshCompleted(books);
  } catch (error) {
    $("#library-message").textContent = `Could not load library: ${error}`;
  }

  async function refreshCompleted(books = null) {
    try {
      const library = books || await invoke("list_library");
      const completed = library.filter((book) => book.status === "Completed" && book.job_id);
      const list = $("#completed-list");
      list.replaceChildren();
      $("#completed-empty").hidden = completed.length > 0;
      $("#completed-count").textContent = String(completed.length);
      for (const book of completed) {
        const card = document.createElement("article");
        card.className = "content-card completed-card";
        const details = document.createElement("div");
        details.className = "card-details";
        const title = document.createElement("h3");
        title.textContent = book.title;
        const meta = document.createElement("p");
        meta.textContent = `${book.source_language} · Completed`;
        details.append(title, meta);
        const actions = document.createElement("div");
        actions.className = "card-actions";
        const glossaryButton = document.createElement("button");
        glossaryButton.type = "button";
        glossaryButton.className = "button button-secondary";
        glossaryButton.textContent = "Recommend glossary";
        glossaryButton.addEventListener("click", async () => {
          glossaryButton.disabled = true;
          glossaryButton.textContent = "Reviewing…";
          try {
            const result = await invoke("generate_library_glossary", { bookId: book.id });
            const warning = result.skipped_batches
              ? ` ${result.skipped_batches} batch${result.skipped_batches === 1 ? "" : "es"} could not be parsed, possibly because of mature content.`
              : "";
            $("#completed-message").textContent = result.added
              ? `Added ${result.added} glossary recommendations to ${book.title}'s novel glossary.${warning}`
              : `No new glossary recommendations were found for ${book.title}.${warning}`;
            await refreshLibrary();
          } catch (error) {
            $("#completed-message").textContent = `Could not generate glossary recommendations: ${error}`;
            glossaryButton.disabled = false;
            glossaryButton.textContent = "Recommend glossary";
          }
        });
        const download = document.createElement("button");
        download.type = "button";
        download.className = "button button-primary";
        download.textContent = "Download";
        download.addEventListener("click", async () => {
          try {
            await downloadLibraryBook(book, "#completed-message");
          } catch (error) {
            $("#completed-message").textContent = `Could not download ${book.title}: ${error}`;
          }
        });
        actions.append(glossaryButton, download);
        card.append(details, actions);
        list.append(card);
      }
    } catch (error) {
      $("#completed-message").textContent = `Could not load completed novels: ${error}`;
    }
  }
}

async function importToLibrary() {
  try {
    const selected = await open({
      multiple: true,
      title: "Add novels to your library",
      filters: [{ name: "Book files", extensions: ["epub", "pdf", "txt"] }],
    });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    await invoke("import_library_book", {
      paths,
      sourceLanguage: "Auto-detect",
    });
    $("#library-message").textContent = "Novel added to your library.";
    await refreshLibrary();
  } catch (error) {
    $("#library-message").textContent = `Could not import novel: ${error}`;
  }
}

let rawPage = 1;
let rawQuery = "";
let rawSearchMode = "quick";
let rawAdvancedFilters = null;
const rawGenres = ["academy", "action", "adventure", "fantasy", "horror", "mystery", "romance", "school", "martial", "smut", "adult", "harem", "historical", "scifi", "sliceoflife", "sports", "Uncategorized"];
for (const [containerId, name] of [["#raw-genres-include", "genres_include"], ["#raw-genres-exclude", "genres_exclude"]]) {
  const container = $(containerId);
  rawGenres.forEach((genre, index) => {
    const label = document.createElement("label");
    const box = document.createElement("input"); box.type = "checkbox"; box.name = name; box.value = String(index + 1);
    label.append(box, document.createTextNode(genre)); container.append(label);
  });
}

function addRawTag(editor, value) {
  const tag = value.trim().toLowerCase();
  const pills = editor.querySelector(".raw-filter-pills");
  if (!tag || tag.length > 80 || pills.children.length >= 40 || [...pills.children].some((pill) => pill.dataset.value === tag)) return;
  const pill = document.createElement("span"); pill.className = "raw-filter-pill"; pill.dataset.value = tag;
  const remove = document.createElement("button"); remove.type = "button"; remove.setAttribute("aria-label", `Remove ${tag}`); remove.textContent = "×";
  remove.addEventListener("click", () => pill.remove());
  pill.append(document.createTextNode(tag), remove); pills.append(pill);
}
document.querySelectorAll(".raw-tag-editor").forEach((editor) => {
  const input = editor.querySelector("input");
  input.addEventListener("keydown", (event) => {
    if (event.key === "Enter" || event.key === ",") {
      event.preventDefault(); addRawTag(editor, input.value); input.value = "";
    }
  });
  input.addEventListener("change", () => { addRawTag(editor, input.value); input.value = ""; });
});

function collectRawAdvancedFilters() {
  const form = $("#raw-advanced-form");
  const value = (name) => form.elements.namedItem(name).value.trim();
  const tags = (name) => [...form.querySelector(`.raw-tag-editor[data-raw-filter="${name}"] .raw-filter-pills`).children].map((pill) => pill.dataset.value);
  const genres = (name) => [...form.querySelectorAll(`input[name="${name}"]:checked`)].map((input) => input.value);
  return {
    query: value("query"), author: value("author"), uploader: value("uploader"),
    translator_group: value("translator_group"), country: value("country"),
    year_from: value("year_from"), year_to: value("year_to"), status: value("status"),
    language: value("language"), view_lang: value("view_lang"), read_only: value("read_only"), sort: value("sort"),
    has_images: form.elements.namedItem("has_images").checked,
    has_audiobook: form.elements.namedItem("has_audiobook").checked,
    tag_mode: value("tag_mode"), tags_include: tags("tags_include"), tags_exclude: tags("tags_exclude"),
    source_tag_mode: value("source_tag_mode"), source_tags_include: tags("source_tags_include"),
    source_tags_exclude: tags("source_tags_exclude"), genre_mode: value("genre_mode"),
    genres_include: genres("genres_include"), genres_exclude: genres("genres_exclude"),
  };
}
let rawSuggestionTimer = null;
let rawSuggestionRequest = 0;
let rawSuggestionIndex = -1;

async function loadRawAdvancedTagOptions() {
  try {
    const { tags, source_tags } = await invoke("get_raw_advanced_options");
    for (const [id, options] of [["#raw-tag-options", tags], ["#raw-source-tag-options", source_tags]]) {
      const list = $(id);
      const fragment = document.createDocumentFragment();
      for (const value of options) {
        const option = document.createElement("option"); option.value = value; fragment.append(option);
      }
      list.replaceChildren(fragment);
    }
  } catch (error) {
    // Tag inputs still accept custom values when the site's options cannot be loaded.
  }
}
$("#raw-advanced").addEventListener("toggle", () => {
  if (!$("#raw-advanced").open || $("#raw-advanced").dataset.optionsLoaded) return;
  $("#raw-advanced").dataset.optionsLoaded = "true";
  loadRawAdvancedTagOptions();
});

let advancedTitleTimer = null;
let advancedTitleRequest = 0;
const advancedTitleInput = $("#raw-advanced-form input[name='query']");
advancedTitleInput.addEventListener("input", () => {
  clearTimeout(advancedTitleTimer);
  const query = advancedTitleInput.value.trim();
  const request = ++advancedTitleRequest;
  const list = $("#raw-advanced-title-options");
  list.replaceChildren();
  if ([...query].length < 4) return;
  advancedTitleTimer = setTimeout(async () => {
    try {
      const titles = await invoke("suggest_raw_titles", { query });
      if (request !== advancedTitleRequest || advancedTitleInput.value.trim() !== query) return;
      const fragment = document.createDocumentFragment();
      for (const title of titles) { const option = document.createElement("option"); option.value = title; fragment.append(option); }
      list.replaceChildren(fragment);
    } catch (error) { /* Advanced search remains available without suggestions. */ }
  }, 250);
});

function closeRawSuggestions() {
  const list = $("#raw-suggestions");
  list.hidden = true;
  list.replaceChildren();
  rawSuggestionIndex = -1;
  $("#raw-search-query").setAttribute("aria-expanded", "false");
  $("#raw-search-query").removeAttribute("aria-activedescendant");
}

function renderRawSuggestions(titles) {
  const list = $("#raw-suggestions");
  list.replaceChildren();
  rawSuggestionIndex = -1;
  titles.forEach((title, index) => {
    const option = document.createElement("button");
    option.type = "button";
    option.id = `raw-suggestion-${index}`;
    option.className = "raw-suggestion";
    option.setAttribute("role", "option");
    option.setAttribute("aria-selected", "false");
    option.textContent = title;
    option.addEventListener("mousedown", (event) => event.preventDefault());
    option.addEventListener("click", () => {
      $("#raw-search-query").value = title;
      rawSuggestionRequest++;
      closeRawSuggestions();
      $("#raw-search-query").focus();
      rawQuery = title;
      rawSearchMode = "quick";
      searchRaw(1);
    });
    list.append(option);
  });
  list.hidden = titles.length === 0;
  $("#raw-search-query").setAttribute("aria-expanded", String(titles.length > 0));
}

function scheduleRawSuggestions() {
  clearTimeout(rawSuggestionTimer);
  const request = ++rawSuggestionRequest;
  const query = $("#raw-search-query").value.trim();
  closeRawSuggestions();
  if ([...query].length < 4) return;
  rawSuggestionTimer = setTimeout(async () => {
    try {
      const titles = await invoke("suggest_raw_titles", { query });
      if (request === rawSuggestionRequest && document.activeElement === $("#raw-search-query") && query === $("#raw-search-query").value.trim()) {
        renderRawSuggestions(titles);
      }
    } catch (error) {
      // Suggestions are optional: a failed request should not prevent a normal search.
      if (request === rawSuggestionRequest) closeRawSuggestions();
    }
  }, 250);
}

function selectRawSuggestion(index) {
  const options = [...$("#raw-suggestions").children];
  rawSuggestionIndex = index;
  options.forEach((option, position) => {
    option.classList.toggle("active", position === index);
    option.setAttribute("aria-selected", String(position === index));
  });
  if (index >= 0) {
    $("#raw-search-query").setAttribute("aria-activedescendant", options[index].id);
    options[index].scrollIntoView({ block: "nearest" });
  } else {
    $("#raw-search-query").removeAttribute("aria-activedescendant");
  }
}

async function searchRaw(page = 1) {
  const message = $("#raw-message");
  message.textContent = "Searching RAW catalogue…";
  $("#raw-next").hidden = true;
  const mode = rawSearchMode;
  const query = rawQuery;
  const filters = rawAdvancedFilters;
  try {
    const data = mode === "advanced"
      ? await invoke("search_raw_advanced", { filters, page })
      : await invoke("search_raw_novels", { query, page });
    if (mode !== rawSearchMode || (mode === "quick" && query !== rawQuery) || (mode === "advanced" && filters !== rawAdvancedFilters)) return;
    rawPage = page;
    $("#raw-results").replaceChildren();
    $("#raw-details").hidden = true;
    for (const item of data.results) {
      const card = document.createElement("article"); card.className = "content-card raw-card";
      const cover = document.createElement("img"); cover.className = "library-cover"; cover.alt = `Cover for ${item.title}`;
      if (item.cover_url) cover.src = item.cover_url;
      const info = document.createElement("div"); info.className = "card-details";
      const title = document.createElement("h3"); title.textContent = item.title;
      const subtitle = document.createElement("p"); subtitle.textContent = item.original_title;
      const meta = document.createElement("p"); meta.textContent = `${item.publication_status} · ${item.tags.join(", ")} · ${item.raw_tags_en.join(", ")}`;
      const button = document.createElement("button"); button.type = "button"; button.className = "button button-secondary";
      button.textContent = item.is_raw ? "Details / import" : "Read on site";
      if (item.is_raw) button.addEventListener("click", () => showRawDetails(item.url));
      else button.addEventListener("click", () => window.open(item.url, "_blank", "noopener,noreferrer"));
      info.append(title, subtitle, meta); card.append(cover, info, button); $("#raw-results").append(card);
    }
    message.textContent = data.total == null
      ? `${data.results.length} result(s) on page ${page}.`
      : `${data.total} result(s) · page ${page} (${data.results.length} shown). Non-RAW books can be opened on the site.`;
    $("#raw-next").hidden = !data.next_page;
    $("#raw-next").dataset.page = data.next_page || "";
  } catch (error) { message.textContent = `RAW search failed: ${error}`; }
}
async function showRawDetails(url) {
  const panel = $("#raw-details"); panel.hidden = false; panel.replaceChildren();
  $("#raw-message").textContent = "Loading novel details…";
  try {
    const book = await invoke("fetch_raw_novel_details", { novelUrl: url });
    if (book.cover_url) { const cover = document.createElement("img"); cover.className = "raw-detail-cover"; cover.src = book.cover_url; cover.alt = `Cover for ${book.title}`; panel.append(cover); }
    const heading = document.createElement("h3"); heading.textContent = book.title;
    const original = document.createElement("p"); original.textContent = book.original_title;
    const info = document.createElement("p"); info.textContent = `${book.publication_status} · Author: ${book.author || "Unknown"}`;
    const synopsis = document.createElement("p"); synopsis.textContent = book.synopsis || "No synopsis available.";
    panel.append(heading, original, info, synopsis);
    for (const [name, items] of [["Tags", book.tags], ["Novelpia EN", book.raw_tags_en], ["Novelpia KO", book.raw_tags_ko], ["Genres", book.genres]]) {
      const row = document.createElement("p"); row.textContent = `${name}: ${items.join(", ") || "—"}`; panel.append(row);
    }
    const importButton = document.createElement("button"); importButton.className = "button button-primary"; importButton.type = "button"; importButton.textContent = "Import to Library";
    const saveCopy = document.createElement("button"); saveCopy.className = "button button-secondary"; saveCopy.type = "button"; saveCopy.textContent = "Import and save EPUB copy";
    const localButton = document.createElement("button"); localButton.className = "button button-secondary"; localButton.type = "button"; localButton.textContent = "Import downloaded ZIP";
    const browserButton = document.createElement("button"); browserButton.className = "button button-secondary"; browserButton.type = "button"; browserButton.textContent = "Download ZIP in browser";
    browserButton.addEventListener("click", () => window.open(book.download_url, "_blank", "noopener,noreferrer"));
    browserButton.disabled = !book.download_url;
    const importBook = async (copy, localZip = false) => {
      let optionalOutputPath = null;
      if (copy) { optionalOutputPath = await save({ defaultPath: `${book.title.replace(/[<>:"/\\|?*]/g, "_")}.epub`, filters: [{ name: "EPUB", extensions: ["epub"] }] }); if (!optionalOutputPath) return; }
      let localZipPath = null;
      if (localZip) {
        localZipPath = await open({ multiple: false, title: `Choose the downloaded RAW ZIP for ${book.title}`, filters: [{ name: "ZIP archive", extensions: ["zip"] }] });
        if (!localZipPath) return;
      }
      importButton.disabled = saveCopy.disabled = localButton.disabled = true;
      $("#raw-message").textContent = localZip ? "Decrypting your downloaded RAW ZIP…" : "Downloading and decrypting RAW EPUB…";
      try {
        await invoke("import_raw_novel", { novelUrl: book.url, optionalOutputPath, localZipPath });
        $("#raw-message").textContent = `Imported ${book.title} into Library${copy ? " and saved EPUB copy" : ""}.`;
        await refreshLibrary();
      } catch (error) { $("#raw-message").textContent = `RAW import failed: ${error}`; }
      finally { importButton.disabled = saveCopy.disabled = localButton.disabled = false; }
    };
    importButton.addEventListener("click", () => importBook(false));
    saveCopy.addEventListener("click", () => importBook(true));
    localButton.addEventListener("click", () => importBook(false, true));
    panel.append(importButton, saveCopy, browserButton, localButton);
    $("#raw-message").textContent = "Details loaded.";
    panel.scrollIntoView({ behavior: "smooth", block: "nearest" });
  } catch (error) { $("#raw-message").textContent = `Could not load RAW details: ${error}`; }
}

async function refreshQueue() {
  try {
    const jobs = await invoke("list_queue");
    const list = $("#queue-list");
    list.replaceChildren();
    const activeJobs = jobs.filter((job) => job.queue_status !== "Completed");
    $("#queue-empty").hidden = activeJobs.length > 0;
    const unfinished = activeJobs.length;
    $("#queue-count").textContent = String(unfinished);
    for (const job of activeJobs) {
      const card = document.createElement("article");
      card.className = "content-card queue-card";
      card.id = `queue-job-${job.id}`;
      const details = document.createElement("div");
      details.className = "queue-details";
      const title = document.createElement("h3");
      title.textContent = job.title;
      const meta = document.createElement("p");
      meta.textContent = `${job.model || "No model selected"} · ${job.source_language}`;
      meta.id = `queue-model-${job.id}`;
      const currentIndex = job.queue_status === "Translating" && Number.isInteger(job.current_section_index)
        ? job.current_section_index : job.sections.findIndex((section) => section.translated === null);
      const chapterInfo = document.createElement("p");
      chapterInfo.id = `queue-chapter-${job.id}`;
      chapterInfo.textContent = currentIndex < 0 ? "All chapters translated" : `${job.queue_status === "Translating" ? "Working on" : "Next"} chapter ${currentIndex + 1} of ${job.sections.length}: ${job.sections[currentIndex].title}`;
      const timeInfo = document.createElement("p");
      timeInfo.id = `queue-time-${job.id}`;
      timeInfo.textContent = `Accumulated translation and context time: ${formatDuration(job.elapsed_translation_millis || 0)}`;
      const progressText = document.createElement("p");
      progressText.className = "queue-progress-text";
      progressText.id = `queue-progress-text-${job.id}`;
      progressText.textContent = `${job.completed_chunks} of ${job.total_chunks} chunks · ${formatEta(estimateJobSeconds(job))}`;
      const track = document.createElement("div");
      track.className = "progress-track queue-progress-track";
      const fill = document.createElement("div");
      fill.className = "progress-fill";
      fill.id = `queue-progress-${job.id}`;
      fill.style.width = `${job.total_chunks ? Math.round(job.completed_chunks / job.total_chunks * 100) : 0}%`;
      track.append(fill);
      details.append(title, meta, chapterInfo, timeInfo, progressText, track);
      if (job.context_update_failures?.length) {
        const contextWarning = document.createElement("p");
        contextWarning.className = "context-warning";
        contextWarning.textContent = `Story context needs retry: ${job.context_update_failures.join(" · ")}`;
        details.append(contextWarning);
      }
      const status = document.createElement("span");
      status.className = `status-pill ${(job.queue_status || "Queued").toLowerCase()}`;
      status.textContent = job.queue_status || "Queued";
      const actions = document.createElement("div");
      actions.className = "card-actions";
      actions.append(status);
      const modelSelect = document.createElement("select");
      modelSelect.setAttribute("aria-label", `Queue model for ${job.title}`);
      modelSelect.className = "queue-model-select";
      const modelNames = new Set([...$("#model").options].map((option) => option.value).filter(Boolean));
      if (job.model) modelNames.add(job.model);
      for (const name of modelNames) modelSelect.add(new Option(name, name));
      modelSelect.value = job.model;
      modelSelect.disabled = !modelNames.size;
      modelSelect.addEventListener("change", async () => {
        modelSelect.disabled = true;
        try {
          await invoke("set_queue_model", { jobId: job.id, model: modelSelect.value });
          const selectedModel = modelSelect.value;
          $("#queue-message").textContent = job.queue_status === "Translating"
            ? `Model change for ${job.title} will apply at the next chapter boundary.`
            : `Model for ${job.title} updated without clearing progress.`;
          meta.textContent = job.queue_status === "Translating"
            ? `${job.model} · ${job.source_language} · Next chapter: ${selectedModel}`
            : `${selectedModel} · ${job.source_language}`;
        } catch (error) {
          modelSelect.value = job.model;
          $("#queue-message").textContent = `Could not change model: ${error}`;
        } finally { modelSelect.disabled = false; }
      });
      actions.append(modelSelect);
      card.append(details, actions);
      list.append(card);
    }
    for (const job of activeJobs.filter((entry) => entry.queue_status === "Translating")) {
      try {
        const nextModel = await invoke("get_queue_model_override", { jobId: job.id });
        const label = document.getElementById(`queue-model-${job.id}`);
        const dropdown = document.querySelector(`#queue-job-${CSS.escape(job.id)} .queue-model-select`);
        if (nextModel && label && dropdown) {
          label.textContent = `${job.model} · ${job.source_language} · Next chapter: ${nextModel}`;
          if (![...dropdown.options].some((option) => option.value === nextModel)) dropdown.add(new Option(nextModel, nextModel));
          dropdown.value = nextModel;
        }
      } catch (error) { /* A model override is optional; queue progress remains visible. */ }
    }
    if (queueRunning) {
      const active = activeJobs.find((job) => job.queue_status === "Translating");
      if (active) {
        const chapter = document.getElementById(`queue-chapter-${active.id}`);
        if (chapter && active.current_section_index === null) chapter.textContent = "Starting translation…";
      }
    }
    $("#start-queue").disabled = queueRunning || unfinished === 0;
  } catch (error) {
    $("#queue-message").textContent = `Could not load queue: ${error}`;
  }
}

setInterval(() => {
  if ($("#tab-queue").classList.contains("active") && !document.querySelector(".queue-model-select:focus")) {
    refreshQueue();
  }
}, 5000);

async function startQueue() {
  if (queueRunning) return;
  queueRunning = true;
  $("#start-queue").disabled = true;
  try {
    const jobs = await invoke("list_queue");
    if (jobs.some((job) => job.queue_status !== "Completed" && !job.model)) {
      $("#queue-message").textContent = "Choose a default Ollama model in Settings before starting queued novels.";
      return;
    }
    for (const queued of jobs) {
      if (queued.queue_status === "Completed") continue;
      try {
        await invoke("update_queue_status", { jobId: queued.id, status: "Translating" });
        await refreshQueue();
        await invoke("translate_job", { jobId: queued.id, endpoint: settings.ollama_endpoint, interactive: false });
        await invoke("update_queue_status", { jobId: queued.id, status: "Completed" });
      } catch (error) {
        await invoke("update_queue_status", { jobId: queued.id, status: "Failed" });
        $("#queue-message").textContent = `${queued.title} stopped: ${error}`;
      }
      await Promise.all([refreshQueue(), refreshLibrary()]);
    }
    $("#queue-message").textContent = "Queue processing finished.";
  } catch (error) {
    $("#queue-message").textContent = `Could not start queue: ${error}`;
  } finally {
    queueRunning = false;
    await refreshQueue();
  }
}

function renderGlossary(entries) {
  const list = $("#glossary-list");
  list.replaceChildren();
  $("#glossary-empty").hidden = entries.length > 0;
  for (const entry of entries) {
    const card = document.createElement("article");
    card.className = "content-card glossary-card";
    const details = document.createElement("div");
    details.className = "card-details";
    const terms = document.createElement("h3");
    terms.textContent = `${entry.source_term}  →  ${entry.target_term}`;
    const meta = document.createElement("p");
    meta.textContent = `${entry.source_language}${entry.note ? ` · ${entry.note}` : ""}${entry.character_gender ? ` · Gender: ${entry.character_gender}` : ""}`;
    details.append(terms, meta);
    const actions = document.createElement("div");
    actions.className = "card-actions";
    const edit = document.createElement("button");
    edit.type = "button";
    edit.className = "button button-secondary";
    edit.textContent = "Edit";
    edit.addEventListener("click", () => {
      $("#glossary-id").value = entry.id;
      $("#glossary-language").value = entry.source_language;
      $("#glossary-source").value = entry.source_term;
      $("#glossary-target").value = entry.target_term;
      $("#glossary-note").value = entry.note;
      $("#glossary-gender").value = entry.character_gender || "";
      $("#save-glossary").textContent = "Save changes";
      $("#cancel-glossary-edit").hidden = false;
    });
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "button button-secondary";
    remove.textContent = "Delete";
    remove.addEventListener("click", async () => {
      try {
        renderGlossary(await invoke("delete_glossary_entry", { id: entry.id }));
        $("#glossary-message").textContent = "Glossary term deleted.";
      } catch (error) {
        $("#glossary-message").textContent = `Could not delete term: ${error}`;
      }
    });
    actions.append(edit, remove);
    card.append(details, actions);
    list.append(card);
  }
}

async function refreshGlossary() {
  try {
    renderGlossary(await invoke("list_glossary"));
  } catch (error) {
    $("#glossary-message").textContent = `Could not load glossary: ${error}`;
  }
}

function resetGlossaryForm() {
  $("#glossary-id").value = "";
  $("#glossary-source").value = "";
  $("#glossary-target").value = "";
  $("#glossary-note").value = "";
  $("#glossary-gender").value = "";
  $("#save-glossary").textContent = "Add term";
  $("#cancel-glossary-edit").hidden = true;
}

async function saveGlossary() {
  try {
    const entries = await invoke("save_glossary_entry", {
      id: $("#glossary-id").value || null,
      sourceLanguage: $("#glossary-language").value,
      sourceTerm: $("#glossary-source").value,
      targetTerm: $("#glossary-target").value,
      note: $("#glossary-note").value,
      characterGender: $("#glossary-gender").value,
    });
    renderGlossary(entries);
    resetGlossaryForm();
    $("#glossary-message").textContent = "Glossary saved. New translations will use these terms.";
  } catch (error) {
    $("#glossary-message").textContent = `Could not save glossary term: ${error}`;
  }
}

async function exportGlossary() {
    try {
      const entries = await invoke("list_glossary");
      if (!entries.length) {
        $("#glossary-message").textContent = "Add at least one glossary term before exporting.";
        return;
      }
      const text = entries
        .map((entry) => [entry.source_term, entry.target_term, entry.note, entry.character_gender].filter((value, index) => index < 2 || value).join("\t"))
        .join("\n");
      const path = await save({
        defaultPath: "novel-translator-glossary.txt",
        filters: [{ name: "Plain text", extensions: ["txt"] }],
      });
      if (!path) return;
      await invoke("write_text_file", { path, text });
      $("#glossary-message").textContent = `Glossary exported to ${path}`;
    } catch (error) {
      $("#glossary-message").textContent = `Could not export glossary: ${error}`;
  }
}

async function saveSettings() {
  try {
    settings = await invoke("update_app_settings", {
      ollamaEndpoint: $("#settings-endpoint").value.trim(),
      defaultSourceLanguage: $("#settings-language").value,
      defaultOutputFormat: $("#settings-output").value,
      defaultModel: $("#settings-model").value,
      autoRetry: $("#settings-auto-retry").checked,
      interactiveTranslation: $("#settings-interactive-translation").checked,
      translationPrompt: $("#settings-prompt").value,
      darkMode: $("#settings-dark-mode").checked,
      contextLimit: Number($("#settings-context-limit").value),
    });
    $("#output-format").value = settings.default_output_format;
    $("#settings-message").textContent = "Settings saved.";
    applyTheme(settings.dark_mode);
    await refreshModels();
  } catch (error) {
    $("#settings-message").textContent = `Could not save settings: ${error}`;
  }
}

function applyTheme(dark) {
  document.documentElement.dataset.theme = dark ? "dark" : "light";
  $("#theme-toggle").textContent = dark ? "☀" : "☾";
  const label = dark ? "Switch to light mode" : "Switch to dark mode";
  $("#theme-toggle").setAttribute("aria-label", label);
  $("#theme-toggle").title = label;
}

async function toggleTheme() {
  $("#settings-dark-mode").checked = !$("#settings-dark-mode").checked;
  await saveSettings();
}

function activateTab(name) {
  document.querySelectorAll(".tab-button").forEach((button) => {
    const selected = button.dataset.tab === name;
    button.classList.toggle("active", selected);
    button.setAttribute("aria-selected", String(selected));
  });
  document.querySelectorAll(".tab-panel").forEach((panel) => {
    panel.classList.toggle("active", panel.id === `tab-${name}`);
  });
  if (name === "library") refreshLibrary();
  if (name === "queue") refreshQueue();
  if (name === "completed") refreshCompleted();
  if (name === "glossary") refreshGlossary();
}

function setBusy(value) {
  busy = value;
  $("#translate").disabled = value;
  $("#choose-files").disabled = value;
  $("#browse-files").disabled = value;
  $("#refresh-models").disabled = value;
}

function renderFiles() {
  const list = $("#file-list");
  list.replaceChildren();
  files.forEach((path, index) => {
    const row = document.createElement("div");
    row.className = "file-row";
    const name = document.createElement("span");
    name.textContent = path.split(/[\\/]/).pop();
    const controls = document.createElement("div");
    controls.className = "file-controls";
    const move = (label, offset, disabled) => {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "reorder-file";
      button.textContent = label;
      button.disabled = disabled;
      button.setAttribute("aria-label", `${offset < 0 ? "Move up" : "Move down"} ${name.textContent}`);
      button.addEventListener("click", () => {
        [files[index], files[index + offset]] = [files[index + offset], files[index]];
        renderFiles();
      });
      return button;
    };
    controls.append(move("↑", -1, index === 0), move("↓", 1, index === files.length - 1));
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "remove-file";
    remove.textContent = "Remove";
    remove.setAttribute("aria-label", `Remove ${name.textContent}`);
    remove.addEventListener("click", () => {
      files.splice(index, 1);
      renderFiles();
    });
    controls.append(remove);
    row.append(name, controls);
    list.append(row);
  });
  $("#drop-zone").classList.toggle("has-files", files.length > 0);
}

async function chooseFiles() {
  if (busy) return;
  try {
    const selected = await open({
      multiple: true,
      title: "Choose source files",
      filters: [{ name: "Book files", extensions: ["epub", "pdf", "txt"] }],
    });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    for (const path of paths) if (!files.includes(path)) files.push(path);
    renderFiles();
    activeJob = null;
    $("#export").hidden = true;
    $("#recommendations-panel").hidden = true;
    $("#recommendations-panel").hidden = true;
  } catch (error) {
    setStatus(`Could not open the file picker: ${error}`, true);
  }
}

async function refreshModels() {
  const status = $("#ollama-status");
  status.textContent = "Checking Ollama…";
  try {
    const previous = $("#model").value || settings.default_model;
    const models = await invoke("list_ollama_models", { endpoint: settings.ollama_endpoint });
    const select = $("#model");
    const defaultSelect = $("#settings-model");
    select.replaceChildren();
    defaultSelect.replaceChildren(new Option("No default model", ""));
    if (!models.length) {
      select.add(new Option("No models installed in Ollama", ""));
      $("#ollama-status").textContent = `Ollama is running at ${settings.ollama_endpoint}, but no models were found.`;
      status.textContent = "Ollama is running, but no models were found. Pull a model in Ollama, then refresh.";
      return;
    }
    for (const model of models) {
      select.add(new Option(model, model));
      defaultSelect.add(new Option(model, model));
    }
    if (models.includes(previous)) select.value = previous;
    if (models.includes(settings.default_model)) defaultSelect.value = settings.default_model;
    status.textContent = `${models.length} local model${models.length === 1 ? "" : "s"} available`;
  } catch (error) {
    $("#model").replaceChildren(new Option("Ollama unavailable", ""));
    $("#settings-model").replaceChildren(new Option("Ollama unavailable", ""));
    status.textContent = `Could not reach Ollama at ${settings.ollama_endpoint} — ${error}`;
    setStatus("Start Ollama, then refresh the model list.", true);
  }
}

async function refreshSavedJobs() {
  try {
    const jobs = await invoke("list_jobs");
    const select = $("#saved-jobs");
    select.replaceChildren();
    for (const job of jobs) {
      const label = `${job.title} · ${job.completed_sections}/${job.section_count} translated`;
      select.add(new Option(label, job.id));
    }
    $("#job-list-wrap").hidden = jobs.length === 0;
    $("#delete-saved-job").disabled = jobs.length === 0;
  } catch (error) {
    setStatus(`Could not load saved jobs: ${error}`, true);
  }
}

async function deleteSavedJob() {
  const id = $("#saved-jobs").value;
  if (!id) return;
  const title = $("#saved-jobs").selectedOptions[0]?.textContent ?? "this saved translation";
  if (!window.confirm(`Remove ${title} and its saved progress from the app?`)) return;
  try {
    await invoke("delete_saved_job", { jobId: id });
    if (activeJob?.id === id) {
      activeJob = null;
      $("#export").hidden = true;
      $("#recommendations-panel").hidden = true;
    }
    await Promise.all([refreshSavedJobs(), refreshLibrary(), refreshQueue()]);
    setStatus("Saved translation removed.");
  } catch (error) {
    setStatus(`Could not remove saved translation: ${error}`, true);
  }
}

async function loadJob() {
  const id = $("#saved-jobs").value;
  if (!id) return;
  try {
    activeJob = await invoke("get_job", { jobId: id });
    $("#recommendations-list").replaceChildren();
    $("#recommendations-message").textContent = "";
    $("#recommendations-panel").hidden = activeJob.completed_sections !== activeJob.section_count;
    $("#generate-recommendations").disabled = false;
    files.splice(0, files.length);
    renderFiles();
    $("#source-language").value = activeJob.source_language;
    $("#output-format").value = activeJob.output_format;
    $("#genre").value = activeJob.genre ?? "";
    $("#instructions").value = activeJob.instructions ?? "";
    document.querySelectorAll(".tone-options input").forEach((input) => {
      input.checked = activeJob.tones.includes(input.value);
    });
    if (activeJob.model) $("#model").value = activeJob.model;
    updateProgress(activeJob.completed_chunks, activeJob.total_chunks, estimateJobSeconds(activeJob));
    $("#export").hidden = activeJob.completed_sections !== activeJob.section_count;
    setStatus(activeJob.completed_sections === activeJob.section_count ? "Translation complete. Save your translated book." : "Saved translation loaded. Continue when ready.");
  } catch (error) {
    setStatus(`Could not load this job: ${error}`, true);
  }
}

function estimateJobSeconds(job) {
  const completed = job.completed_chunks ?? 0;
  const total = job.total_chunks ?? 0;
  if (completed >= total) return total ? 0 : null;
  if (!completed || !job.elapsed_translation_millis) return null;
  return Math.ceil((job.elapsed_translation_millis * (total - completed) / completed) / 1000);
}

function formatEta(seconds) {
  if (seconds === null) return "ETA appears after the first chunk";
  if (seconds === 0) return "Complete";
  if (seconds < 60) return "Under 1 min left";
  const minutes = Math.ceil(seconds / 60);
  if (minutes < 60) return `About ${minutes} min left`;
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return remainingMinutes ? `About ${hours} hr ${remainingMinutes} min left` : `About ${hours} hr left`;
}

function formatDuration(milliseconds) {
  const seconds = Math.floor(milliseconds / 1000);
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return `${hours ? `${hours}h ` : ""}${minutes}m ${seconds % 60}s`;
}

function updateProgress(completed, total, remainingSeconds = null) {
  const percent = total ? Math.round((completed / total) * 100) : 0;
  $("#progress-fill").style.width = `${percent}%`;
  $("#progress-label").textContent = total ? `${completed} of ${total} chunks · ${percent}%` : "Ready when you are";
  $("#eta-label").textContent = formatEta(remainingSeconds);
}

async function translate() {
  if (busy) return;
  if (!files.length && !activeJob) {
    setStatus("Choose an EPUB, PDF, or TXT file first.", true);
    return;
  }
  if (!$("#model").value) {
    setStatus("Select an installed Ollama model before translating.", true);
    return;
  }
  setBusy(true);
  $("#export").hidden = true;
  try {
    if (!activeJob) {
      activeJob = await invoke("create_job", {
        paths: files,
        sourceLanguage: $("#source-language").value,
        outputFormat: $("#output-format").value,
        genre: $("#genre").value.trim(),
        tones: [...document.querySelectorAll(".tone-options input:checked")].map((input) => input.value),
        instructions: $("#instructions").value.trim(),
        model: $("#model").value,
      });
      $("#source-language").value = activeJob.source_language;
    } else {
      activeJob = await invoke("update_job_settings", {
        jobId: activeJob.id,
        sourceLanguage: $("#source-language").value,
        outputFormat: $("#output-format").value,
        genre: $("#genre").value.trim(),
        tones: [...document.querySelectorAll(".tone-options input:checked")].map((input) => input.value),
        instructions: $("#instructions").value.trim(),
        model: $("#model").value,
      });
    }
    activeJob = await invoke("translate_job", { jobId: activeJob.id, endpoint: settings.ollama_endpoint, interactive: $("#interactive-translation")?.checked ?? false });
    updateProgress(activeJob.completed_chunks, activeJob.total_chunks, estimateJobSeconds(activeJob));
    $("#export").hidden = false;
    $("#recommendations-panel").hidden = false;
    $("#recommendations-list").replaceChildren();
    $("#recommendations-message").textContent = "";
    setStatus(activeJob.context_update_failures?.length
      ? `Translation complete, but story context extraction needs retry: ${activeJob.context_update_failures.join(" · ")}`
      : "Translation complete. Save your translated book.");
    await refreshSavedJobs();
  } catch (error) {
    setStatus(`Translation stopped: ${error}. Your completed sections are saved; resume this job to continue.`, true);
    await refreshSavedJobs();
  } finally {
    setBusy(false);
  }
}

async function exportBook() {
  if (!activeJob) return;
  const extension = activeJob.output_format;
  const path = await save({
    defaultPath: `${activeJob.title}-en.${extension}`,
    filters: [{ name: extension.toUpperCase(), extensions: [extension] }],
  });
  if (!path) return;
  try {
    await invoke("export_job", { jobId: activeJob.id, outputPath: path });
    setStatus(`Saved translated book to ${path}`);
  } catch (error) {
    setStatus(`Could not save the translated book: ${error}`, true);
  }
}

$("#choose-files").addEventListener("click", chooseFiles);
$("#browse-files").addEventListener("click", (event) => { event.stopPropagation(); chooseFiles(); });
$("#drop-zone").addEventListener("click", (event) => {
  if (event.target.closest("button")) return;
  chooseFiles();
});
$("#drop-zone").addEventListener("keydown", (event) => {
  if (event.key === "Enter" || event.key === " ") chooseFiles();
});
$("#refresh-models").addEventListener("click", refreshModels);
$("#load-job").addEventListener("click", loadJob);
$("#delete-saved-job").addEventListener("click", deleteSavedJob);
$("#translate").addEventListener("click", translate);
$("#export").addEventListener("click", exportBook);
$("#generate-recommendations").addEventListener("click", generateRecommendations);
$("#close-illustrations").addEventListener("click", () => $("#illustration-dialog").close());

document.querySelectorAll(".tab-button").forEach((button) => {
  button.addEventListener("click", () => activateTab(button.dataset.tab));
});
$("#import-library").addEventListener("click", importToLibrary);
$("#library-search").addEventListener("input", () => refreshLibrary());
for (const id of ["#library-publication-filter", "#library-translation-filter", "#library-language-filter"]) {
  $(id).addEventListener("change", refreshLibrary);
}
$("#raw-search-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const selected = $("#raw-suggestions").children[rawSuggestionIndex];
  if (selected && !$("#raw-suggestions").hidden) $("#raw-search-query").value = selected.textContent;
  clearTimeout(rawSuggestionTimer);
  rawSuggestionRequest++;
  closeRawSuggestions();
  rawQuery = $("#raw-search-query").value.trim();
  rawSearchMode = "quick";
  searchRaw(1);
});
$("#raw-advanced-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = $("#raw-advanced-form");
  if (!form.reportValidity()) return;
  for (const editor of form.querySelectorAll(".raw-tag-editor")) {
    const input = editor.querySelector("input");
    if (input.value.trim()) { addRawTag(editor, input.value); input.value = ""; }
  }
  rawAdvancedFilters = collectRawAdvancedFilters();
  rawSearchMode = "advanced";
  searchRaw(1);
});
$("#raw-advanced-reset").addEventListener("click", () => {
  $("#raw-advanced-form").reset();
  document.querySelectorAll(".raw-filter-pills").forEach((pills) => pills.replaceChildren());
  clearTimeout(advancedTitleTimer);
  advancedTitleRequest++;
  $("#raw-advanced-title-options").replaceChildren();
  rawAdvancedFilters = collectRawAdvancedFilters();
  rawSearchMode = "advanced";
  searchRaw(1);
});
$("#raw-search-query").addEventListener("input", scheduleRawSuggestions);
$("#raw-search-query").addEventListener("focus", scheduleRawSuggestions);
$("#raw-search-query").addEventListener("keydown", (event) => {
  const options = $("#raw-suggestions").children;
  if (event.key === "Escape") {
    clearTimeout(rawSuggestionTimer);
    rawSuggestionRequest++;
    closeRawSuggestions();
  } else if (!$("#raw-suggestions").hidden && options.length && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
    event.preventDefault();
    const direction = event.key === "ArrowDown" ? 1 : -1;
    selectRawSuggestion(rawSuggestionIndex < 0
      ? (direction > 0 ? 0 : options.length - 1)
      : (rawSuggestionIndex + direction + options.length) % options.length);
  }
});
$("#raw-search-query").addEventListener("blur", () => {
  clearTimeout(rawSuggestionTimer);
  rawSuggestionRequest++;
  closeRawSuggestions();
});
$("#raw-next").addEventListener("click", () => searchRaw(Number($("#raw-next").dataset.page) || rawPage + 1));
$("#start-queue").addEventListener("click", startQueue);
$("#save-glossary").addEventListener("click", saveGlossary);
$("#export-glossary").addEventListener("click", exportGlossary);
$("#cancel-glossary-edit").addEventListener("click", resetGlossaryForm);
$("#save-settings").addEventListener("click", saveSettings);
$("#theme-toggle").addEventListener("click", toggleTheme);

listen("translation-progress", (event) => {
  const { job_id, completed, total, estimated_remaining_seconds, message } = event.payload;
  if (activeJob?.id === job_id) {
    updateProgress(completed, total, estimated_remaining_seconds);
    setStatus(message);
  }
  const fill = document.querySelector(`#queue-progress-${CSS.escape(job_id)}`);
  if (fill) {
    fill.style.width = `${total ? Math.round(completed / total * 100) : 0}%`;
    $(`#queue-progress-text-${CSS.escape(job_id)}`).textContent = `${completed} of ${total} chunks · ${formatEta(estimated_remaining_seconds)}`;
  }
}).catch((error) => {
  setStatus(`Could not subscribe to translation progress: ${error}`, true);
});

listen("story-context-updated", (event) => {
  const { book_id, context, added } = event.payload;
  const editor = document.querySelector(`#book-context-editor-${CSS.escape(book_id)}`);
  const summary = document.querySelector(`#book-context-summary-${CSS.escape(book_id)}`);
  const message = document.querySelector(`#book-context-message-${CSS.escape(book_id)}`);
  if (editor) editor.value = context.join("\n");
  if (summary) summary.textContent = `Story context · ${context.length} facts`;
  if (message) {
    message.textContent = added
      ? `Added ${added} story context fact${added === 1 ? "" : "s"} from the translated chapter.`
      : "Chapter reviewed; no new story context facts were needed.";
  }
}).catch((error) => {
  setStatus(`Could not subscribe to story context updates: ${error}`, true);
});

Promise.all([loadSettings(), refreshSavedJobs()]).then(async () => {
  await refreshModels();
  await Promise.all([refreshLibrary(), refreshQueue()]);
});
