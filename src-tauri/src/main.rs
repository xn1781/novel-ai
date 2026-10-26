#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    collections::HashMap,
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context, Result};
use reqwest::{Client, Url};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use tauri::{
    webview::PageLoadEvent, AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder,
};
use uuid::Uuid;
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

static INTERACTIVE_ANSWERS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

#[derive(Clone, Serialize, Deserialize)]
struct Section {
    title: String,
    text: String,
    translated: Option<String>,
    #[serde(default)]
    translated_chunks: Vec<String>,
    images: Vec<ImageAsset>,
    #[serde(default = "context_update_complete")]
    context_updated: bool,
}

fn context_update_complete() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize)]
struct ImageAsset {
    file_name: String,
    alt: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Job {
    id: String,
    title: String,
    source_language: String,
    output_format: String,
    genre: String,
    tones: Vec<String>,
    instructions: String,
    model: String,
    sections: Vec<Section>,
    completed_sections: usize,
    section_count: usize,
    #[serde(default)]
    completed_chunks: usize,
    #[serde(default)]
    total_chunks: usize,
    #[serde(default)]
    elapsed_translation_millis: u64,
    #[serde(default)]
    current_section_index: Option<usize>,
    #[serde(default)]
    library_id: Option<String>,
    #[serde(default)]
    queue_status: Option<String>,
    #[serde(default)]
    queue_order: usize,
    #[serde(default = "default_translation_prompt")]
    translation_prompt: String,
    #[serde(default)]
    glossary: Vec<GlossaryEntry>,
    #[serde(default)]
    context: Vec<String>,
    #[serde(default)]
    context_update_failures: Vec<String>,
    #[serde(default = "default_context_limit")]
    context_limit: usize,
}

#[derive(Default, Serialize, Deserialize)]
struct JobOverrides {
    #[serde(default)]
    titles: HashMap<usize, String>,
    #[serde(default)]
    pending_model: Option<String>,
    #[serde(default)]
    novel_title: Option<String>,
    #[serde(default)]
    current_section_index: Option<usize>,
}

fn job_overrides_path(app: &AppHandle, id: &str) -> Result<PathBuf> {
    let id = Uuid::parse_str(id)
        .context("invalid translation job identifier")?
        .to_string();
    Ok(jobs_dir(app)?.join(format!("{id}.overrides.json")))
}

fn load_job_overrides(app: &AppHandle, id: &str) -> Result<JobOverrides> {
    let path = job_overrides_path(app, id)?;
    if !path.exists() {
        return Ok(JobOverrides::default());
    }
    serde_json::from_slice(&fs::read(path)?).context("reading queued job edits")
}

fn save_job_overrides(app: &AppHandle, id: &str, overrides: &JobOverrides) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(overrides)?;
    fs::write(job_overrides_path(app, id)?, bytes).context("saving queued job edits")
}

fn apply_job_titles(job: &mut Job, overrides: &JobOverrides) {
    if let Some(title) = &overrides.novel_title {
        job.title = title.clone();
    }
    for (&index, title) in &overrides.titles {
        if let Some(section) = job.sections.get_mut(index) {
            section.title = title.clone();
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct LibraryBook {
    id: String,
    title: String,
    files: Vec<String>,
    source_language: String,
    status: String,
    job_id: Option<String>,
    #[serde(default)]
    glossary: Vec<GlossaryEntry>,
    #[serde(default)]
    cover_image: Option<String>,
    #[serde(default)]
    context: Vec<String>,
    #[serde(default = "default_library_output_format")]
    output_format: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default = "default_publication_status")]
    publication_status: String,
    #[serde(default)]
    synopsis: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    original_title: String,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    raw_source_url: Option<String>,
    #[serde(default)]
    raw_tags_en: Vec<String>,
    #[serde(default)]
    raw_tags_ko: Vec<String>,
    #[serde(default)]
    novel_updates_url: Option<String>,
}

#[derive(Serialize)]
struct NovelUpdatesMetadata {
    url: String,
    title: String,
    original_title: String,
    author: String,
    synopsis: String,
    tags: Vec<String>,
    genres: Vec<String>,
    publication_status: String,
    cover_url: Option<String>,
}

fn validate_novel_updates_url(input: &str) -> Result<Url, String> {
    let url =
        Url::parse(input.trim()).map_err(|error| format!("Invalid Novel Updates URL: {error}"))?;
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("www.novelupdates.com" | "novelupdates.com")
        )
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return Err("Use a direct HTTPS Novel Updates series link.".into());
    }
    let slug = url
        .path()
        .strip_prefix("/series/")
        .unwrap_or("")
        .trim_end_matches('/');
    if slug.is_empty()
        || !slug
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err("Use a direct Novel Updates /series/name/ link.".into());
    }
    Ok(url)
}

fn parse_novel_updates_metadata(html: &str, url: &Url) -> Result<NovelUpdatesMetadata, String> {
    let page = Html::parse_document(html);
    let root = page.root_element();
    let title = select_text(root, "div.seriestitlenu, h4.seriestitlenu, h1.entry-title");
    if title.is_empty() {
        return Err("Novel Updates did not return a readable series page (it may require a browser or block automated requests).".into());
    }
    let synopsis = select_text(
        root,
        "#editdescription, .series-synopsis, [itemprop='description']",
    );
    let tags = labels(root, "#showtags a, #showtags span");
    let genres = labels(root, "#seriesgenre a, .series-genres a");
    let authors = labels(root, "#showauthors a, .series-authors a");
    let original_title = select_text(root, "#editassociated, .associated-names");
    let status_text = select_text(root, "#editstatus, .series-status");
    let cover = select_attr(
        root,
        "#seriesimg img, .seriesimg img, meta[property='og:image']",
        "src",
    );
    let cover = if cover.is_empty() {
        select_attr(root, "meta[property='og:image']", "content")
    } else {
        cover
    };
    let cover_url = (!cover.is_empty())
        .then(|| url.join(&cover).ok())
        .flatten()
        .filter(|url| {
            url.scheme() == "https"
                && url.port().is_none()
                && matches!(
                    url.host_str(),
                    Some("www.novelupdates.com" | "novelupdates.com")
                        | Some("cdn.novelupdates.com")
                )
        })
        .map(|url| url.to_string());
    Ok(NovelUpdatesMetadata {
        url: url.to_string(),
        title,
        original_title,
        author: authors.join(", "),
        synopsis,
        tags,
        genres,
        publication_status: if status_text.to_lowercase().contains("complete") {
            "Completed".into()
        } else if status_text.to_lowercase().contains("hiatus") {
            "Hiatus".into()
        } else if status_text.to_lowercase().contains("drop") {
            "Dropped".into()
        } else if status_text.to_lowercase().contains("ongoing") {
            "Ongoing".into()
        } else {
            "Unknown".into()
        },
        cover_url,
    })
}

#[tauri::command]
async fn fetch_novel_updates_metadata(url: String) -> Result<NovelUpdatesMetadata, String> {
    let url = validate_novel_updates_url(&url)?;
    let client = Client::builder().timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/124.0.0.0 Safari/537.36")
        .build().map_err(|error| error.to_string())?;
    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|error| format!("Novel Updates request failed: {error}"))?;
    if response.status() == reqwest::StatusCode::FORBIDDEN {
        return Err("Novel Updates refused the direct metadata request (HTTP 403). Open the page in the app browser to fetch it.".into());
    }
    let response = response
        .error_for_status()
        .map_err(|error| format!("Novel Updates request failed: {error}"))?;
    if response
        .content_length()
        .is_some_and(|size| size > 4 * 1024 * 1024)
    {
        return Err("Novel Updates page is too large.".into());
    }
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("Novel Updates page is too large.".into());
    }
    parse_novel_updates_metadata(&String::from_utf8_lossy(&bytes), &url)
}

#[tauri::command]
async fn fetch_novel_updates_in_browser(
    app: AppHandle,
    url: String,
) -> Result<NovelUpdatesMetadata, String> {
    let url = validate_novel_updates_url(&url)?;
    let expected = url.clone();
    let label = format!("novel-updates-{}", Uuid::new_v4());
    let (sender, receiver) = std::sync::mpsc::sync_channel::<String>(1);
    let window = WebviewWindowBuilder::new(&app, label, WebviewUrl::External(url.clone()))
        .title("Novel Updates — finish loading the series page")
        .inner_size(950.0, 780.0)
        .on_navigation(|next| {
            next.scheme() == "https"
                && matches!(
                    next.host_str(),
                    Some("www.novelupdates.com" | "novelupdates.com")
                )
        })
        .on_page_load(move |webview, payload| {
            if payload.event() != PageLoadEvent::Finished
                || payload.url().path() != expected.path()
                || !matches!(
                    payload.url().host_str(),
                    Some("www.novelupdates.com" | "novelupdates.com")
                )
            {
                return;
            }
            let sender = sender.clone();
            // No IPC is enabled for the remote window. Only the HTML string is returned to Rust.
            let _ = webview.eval_with_callback("document.querySelector('div.seriestitlenu, h4.seriestitlenu, h1.entry-title') ? document.documentElement.outerHTML : null", move |value| {
                if value != "null" && value.len() <= 8 * 1024 * 1024 {
                    let _ = sender.try_send(value);
                }
            });
        })
        .build()
        .map_err(|error| format!("Opening Novel Updates browser: {error}"))?;
    let result = tokio::task::spawn_blocking(move || receiver.recv_timeout(Duration::from_secs(120)))
        .await.map_err(|error| error.to_string())?
        .map_err(|_| "Novel Updates did not load a readable series page in 2 minutes. The embedded browser may also be blocked.".to_string());
    let _ = window.close();
    let html = result?;
    let html: String = serde_json::from_str(&html)
        .map_err(|_| "Could not read the Novel Updates browser page.".to_string())?;
    parse_novel_updates_metadata(&html, &url)
}

#[tauri::command]
async fn apply_novel_updates_metadata(
    app: AppHandle,
    book_id: String,
    metadata: NovelUpdatesMetadataInput,
) -> Result<LibraryBook, String> {
    let url = validate_novel_updates_url(&metadata.url)?;
    let mut book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    // Validate merged metadata before persisting the title override or any other changes.
    let tags = normalize_labels(
        book.tags
            .iter()
            .cloned()
            .chain(metadata.tags.iter().cloned())
            .collect(),
    )?;
    let genres = normalize_labels(
        book.genres
            .iter()
            .cloned()
            .chain(metadata.genres.iter().cloned())
            .collect(),
    )?;
    book.novel_updates_url = Some(url.to_string());
    if metadata.replace_title {
        let title = metadata.title.trim();
        if title.is_empty() || title.chars().count() > 200 {
            return Err("Novel title must be 1–200 characters.".into());
        }
        if let Some(job_id) = &book.job_id {
            let mut overrides =
                load_job_overrides(&app, job_id).map_err(|error| error.to_string())?;
            overrides.novel_title = Some(title.to_string());
            save_job_overrides(&app, job_id, &overrides).map_err(|error| error.to_string())?;
        }
        book.title = title.to_string();
    }
    if !metadata.original_title.trim().is_empty() {
        book.original_title = metadata.original_title.trim().into();
    }
    if !metadata.author.trim().is_empty() {
        book.author = metadata.author.trim().into();
    }
    if !metadata.synopsis.trim().is_empty() {
        book.synopsis = metadata.synopsis.trim().into();
    }
    book.tags = tags;
    book.genres = genres;
    if metadata.publication_status != "Unknown" {
        book.publication_status = metadata.publication_status;
    }
    if let Some(image_url) = metadata.cover_url.as_deref() {
        let image_url = Url::parse(image_url)
            .map_err(|error| format!("Invalid Novel Updates cover: {error}"))?;
        if image_url.scheme() != "https"
            || image_url.port().is_some()
            || !matches!(
                image_url.host_str(),
                Some("www.novelupdates.com" | "novelupdates.com" | "cdn.novelupdates.com")
            )
        {
            return Err("Novel Updates cover must be hosted by Novel Updates.".into());
        }
        let client = Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| error.to_string())?;
        if let Ok(response) = client.get(image_url).send().await {
            if response.status().is_success()
                && response
                    .content_length()
                    .is_none_or(|size| size <= 5 * 1024 * 1024)
            {
                if let Ok(bytes) = response.bytes().await {
                    if bytes.len() <= 5 * 1024 * 1024 {
                        let extension = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
                            Some("png")
                        } else if bytes.starts_with(b"\xff\xd8\xff") {
                            Some("jpg")
                        } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
                            Some("webp")
                        } else {
                            None
                        };
                        if let Some(extension) = extension {
                            let directory = library_dir(&app)
                                .map_err(|error| error.to_string())?
                                .join(&book.id);
                            fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
                            let name = format!("novel-updates-cover.{extension}");
                            fs::write(directory.join(&name), bytes)
                                .map_err(|error| error.to_string())?;
                            book.cover_image = Some(name);
                        }
                    }
                }
            }
        }
    }
    save_library_book(&app, &book).map_err(|error| error.to_string())?;
    Ok(book)
}

#[derive(Deserialize)]
struct NovelUpdatesMetadataInput {
    url: String,
    title: String,
    original_title: String,
    author: String,
    synopsis: String,
    tags: Vec<String>,
    genres: Vec<String>,
    publication_status: String,
    replace_title: bool,
    cover_url: Option<String>,
}

fn default_publication_status() -> String {
    "Unknown".into()
}

fn default_library_output_format() -> String {
    "epub".into()
}

fn default_context_limit() -> usize {
    100
}

#[derive(Serialize)]
struct LibraryIllustration {
    path: String,
    section_title: String,
    alt: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct GlossaryEntry {
    id: String,
    source_language: String,
    source_term: String,
    target_term: String,
    note: String,
    #[serde(default)]
    character_gender: String,
}

#[derive(Deserialize)]
struct GlossarySuggestion {
    source_term: String,
    target_term: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    character_gender: String,
}

#[derive(Deserialize)]
struct ContextSuggestion {
    fact: String,
}

#[derive(Clone, Serialize)]
struct StoryContextUpdate {
    book_id: String,
    context: Vec<String>,
    added: usize,
}

const MAX_CONTEXT_FACTS: usize = 200;
const MAX_CONTEXT_PROMPT_CHARS: usize = 12_000;
const MAX_CONTEXT_REVIEW_CHARS: usize = 24_000;

#[derive(Serialize)]
struct GlossaryGenerationResult {
    glossary: Vec<GlossaryEntry>,
    added: usize,
    skipped_batches: usize,
}

#[derive(Clone, Serialize, Deserialize)]
struct AppSettings {
    ollama_endpoint: String,
    default_source_language: String,
    default_output_format: String,
    default_model: String,
    #[serde(default = "default_auto_retry")]
    auto_retry: bool,
    #[serde(default)]
    interactive_translation: bool,
    #[serde(default)]
    dark_mode: bool,
    #[serde(default = "default_translation_prompt")]
    translation_prompt: String,
    #[serde(default = "default_context_limit")]
    context_limit: usize,
}

fn default_auto_retry() -> bool {
    true
}

fn default_translation_prompt() -> String {
    "You are a professional literary translator. Translate the user's {source_language} text into natural English. Preserve paragraph breaks, line breaks, and meaning. Do not summarize, omit content, explain the translation, or add commentary. Return only the translation.{genre}{tones}{instructions}{context}{glossary}".to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            ollama_endpoint: "http://localhost:11434".to_string(),
            default_source_language: "Japanese".to_string(),
            default_output_format: "epub".to_string(),
            default_model: String::new(),
            auto_retry: default_auto_retry(),
            interactive_translation: false,
            dark_mode: false,
            translation_prompt: default_translation_prompt(),
            context_limit: default_context_limit(),
        }
    }
}

#[derive(Clone, Serialize)]
struct Progress {
    job_id: String,
    completed: usize,
    total: usize,
    estimated_remaining_seconds: Option<u64>,
    message: String,
}

#[derive(Deserialize)]
struct OllamaTags {
    models: Vec<OllamaModel>,
}

#[derive(Deserialize)]
struct OllamaModel {
    name: String,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
    think: bool,
    options: ChatOptions,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: String,
}

#[derive(Serialize)]
struct ChatOptions {
    temperature: f32,
}

#[derive(Deserialize)]
struct ChatOutput {
    content: String,
    #[serde(default)]
    thinking: Option<String>,
}

#[derive(Deserialize)]
struct ChatResponse {
    message: ChatOutput,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    eval_count: Option<u64>,
}

fn empty_translation_reason(response: &ChatResponse, section_index: usize) -> String {
    let mut details = Vec::new();
    if let Some(reason) = &response.done_reason {
        details.push(format!("finish reason: {reason}"));
    }
    if let Some(count) = response.eval_count {
        details.push(format!("{count} generated tokens"));
    }
    if response
        .message
        .thinking
        .as_deref()
        .is_some_and(|thinking| !thinking.trim().is_empty())
    {
        details.push("the model returned separate thinking text but no translation".into());
    }
    if details.is_empty() {
        details.push("the model returned no visible message content".into());
    }
    format!(
        "Ollama returned an empty translation for section {} ({})",
        section_index + 1,
        details.join(", ")
    )
}

fn jobs_dir(app: &AppHandle) -> Result<PathBuf> {
    let directory = app.path().app_data_dir()?.join("jobs");
    fs::create_dir_all(&directory).context("creating the saved-jobs directory")?;
    Ok(directory)
}

fn library_dir(app: &AppHandle) -> Result<PathBuf> {
    let directory = app.path().app_data_dir()?.join("library");
    fs::create_dir_all(&directory).context("creating the library directory")?;
    Ok(directory)
}

fn data_file(app: &AppHandle, name: &str) -> Result<PathBuf> {
    let directory = app.path().app_data_dir()?;
    fs::create_dir_all(&directory).context("creating the application data directory")?;
    Ok(directory.join(name))
}

fn save_settings(app: &AppHandle, settings: &AppSettings) -> Result<()> {
    let data = serde_json::to_vec_pretty(settings).context("serializing application settings")?;
    fs::write(data_file(app, "settings.json")?, data).context("saving application settings")
}

fn load_settings(app: &AppHandle) -> Result<AppSettings> {
    let path = data_file(app, "settings.json")?;
    if !path.exists() {
        return Ok(AppSettings::default());
    }
    serde_json::from_slice(&fs::read(path).context("reading application settings")?)
        .context("decoding application settings")
}

fn glossary_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join("glossary.json"))
}

fn load_glossary(app: &AppHandle) -> Result<Vec<GlossaryEntry>> {
    let path = glossary_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    serde_json::from_slice(&fs::read(path).context("reading glossary")?)
        .context("decoding glossary")
}

fn save_glossary(app: &AppHandle, entries: &[GlossaryEntry]) -> Result<()> {
    let data = serde_json::to_vec_pretty(entries).context("serializing glossary")?;
    fs::write(glossary_path(app)?, data).context("saving glossary")
}

fn queue_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_data_dir()?.join("queue.json"))
}

fn load_queue(app: &AppHandle) -> Result<Vec<String>> {
    let path = queue_path(app)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    serde_json::from_slice(&fs::read(path).context("reading translation queue")?)
        .context("decoding translation queue")
}

fn save_queue(app: &AppHandle, queue: &[String]) -> Result<()> {
    let data = serde_json::to_vec_pretty(queue).context("serializing translation queue")?;
    fs::write(queue_path(app)?, data).context("saving translation queue")
}

fn library_book_path(app: &AppHandle, id: &str) -> Result<PathBuf> {
    let canonical_id = Uuid::parse_str(id)
        .context("invalid library book identifier")?
        .to_string();
    Ok(library_dir(app)?.join(format!("{canonical_id}.json")))
}

fn save_library_book(app: &AppHandle, book: &LibraryBook) -> Result<()> {
    let data = serde_json::to_vec_pretty(book).context("serializing library book")?;
    fs::write(library_book_path(app, &book.id)?, data).context("saving library book")
}

fn load_library_book(app: &AppHandle, id: &str) -> Result<LibraryBook> {
    let path = library_book_path(app, id)?;
    serde_json::from_slice(&fs::read(path).context("reading library book")?)
        .context("decoding library book")
}

fn job_path(app: &AppHandle, id: &str) -> Result<PathBuf> {
    let canonical_id = Uuid::parse_str(id)
        .context("invalid translation job identifier")?
        .to_string();
    Ok(jobs_dir(app)?.join(format!("{canonical_id}.json")))
}

fn save_job(app: &AppHandle, job: &Job) -> Result<()> {
    let mut job = job.clone();
    let overrides = load_job_overrides(app, &job.id)?;
    apply_job_titles(&mut job, &overrides);
    let data = serde_json::to_vec_pretty(&job).context("serializing translation job")?;
    fs::write(job_path(app, &job.id)?, data).context("saving translation progress")
}

fn load_job(app: &AppHandle, id: &str) -> Result<Job> {
    let path = job_path(app, id)?;
    let bytes = fs::read(path).context("reading saved translation job")?;
    let mut job: Job = serde_json::from_slice(&bytes).context("decoding saved translation job")?;
    let overrides = load_job_overrides(app, id)?;
    apply_job_titles(&mut job, &overrides);
    if job.queue_status.as_deref() == Some("Translating") {
        if let Some(index) = overrides.current_section_index {
            job.current_section_index = Some(index);
        }
    }
    let (completed, total) = job_chunk_counts(&job);
    job.completed_chunks = completed;
    job.total_chunks = total;
    Ok(job)
}

fn clean_text(text: &str) -> String {
    text.replace('\u{00a0}', " ")
        .replace("\r\n", "\n")
        .trim()
        .to_string()
}

fn natural_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    let (mut left_index, mut right_index) = (0, 0);
    let left_bytes = left.as_bytes();
    let right_bytes = right.as_bytes();
    while left_index < left_bytes.len() && right_index < right_bytes.len() {
        if left_bytes[left_index].is_ascii_digit() && right_bytes[right_index].is_ascii_digit() {
            let (left_start, right_start) = (left_index, right_index);
            while left_index < left_bytes.len() && left_bytes[left_index].is_ascii_digit() {
                left_index += 1;
            }
            while right_index < right_bytes.len() && right_bytes[right_index].is_ascii_digit() {
                right_index += 1;
            }
            let left_number = left[left_start..left_index].trim_start_matches('0');
            let right_number = right[right_start..right_index].trim_start_matches('0');
            let ordering = left_number
                .len()
                .cmp(&right_number.len())
                .then_with(|| left_number.cmp(right_number))
                .then_with(|| (left_index - left_start).cmp(&(right_index - right_start)));
            if ordering != std::cmp::Ordering::Equal {
                return ordering;
            }
        } else {
            let ordering = left_bytes[left_index]
                .to_ascii_lowercase()
                .cmp(&right_bytes[right_index].to_ascii_lowercase());
            if ordering != std::cmp::Ordering::Equal {
                return ordering;
            }
            left_index += 1;
            right_index += 1;
        }
    }
    left_bytes.len().cmp(&right_bytes.len())
}

fn split_translation_chunks(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut offset = 0;
    while offset < text.len() {
        let remaining = &text[offset..];
        if remaining.chars().count() <= max_chars {
            chunks.push(remaining.to_string());
            break;
        }
        let limit = remaining
            .char_indices()
            .nth(max_chars)
            .map(|(index, _)| index)
            .unwrap_or(remaining.len());
        let split = remaining[..limit]
            .rfind('\n')
            .or_else(|| remaining[..limit].rfind(' '))
            .filter(|index| *index > 0)
            .map(|index| index + 1)
            .unwrap_or(limit);
        chunks.push(remaining[..split].to_string());
        offset += split;
    }
    chunks
}

fn detect_source_language(text: &str, fallback: &str) -> String {
    let mut japanese = 0;
    let mut korean = 0;
    for character in text.chars() {
        match character as u32 {
            0x3040..=0x30ff | 0x31f0..=0x31ff => japanese += 1,
            0xac00..=0xd7af | 0x1100..=0x11ff | 0x3130..=0x318f => korean += 1,
            _ => {}
        }
    }
    if korean >= 8 && korean > japanese * 2 {
        "Korean".into()
    } else if japanese >= 4 && japanese * 2 >= korean {
        "Japanese".into()
    } else if korean >= 4 && japanese == 0 {
        "Korean".into()
    } else {
        fallback.to_string()
    }
}

fn job_chunk_counts(job: &Job) -> (usize, usize) {
    let mut completed = 0;
    let mut total = 0;
    for section in &job.sections {
        let section_chunks = split_translation_chunks(&section.text, 3000).len();
        total += section_chunks;
        completed += if section.translated.is_some() {
            section_chunks
        } else {
            section.translated_chunks.len()
        };
    }
    (completed, total)
}

fn estimate_remaining_seconds(completed: usize, total: usize, elapsed_millis: u64) -> Option<u64> {
    let remaining = total.saturating_sub(completed);
    if remaining == 0 {
        return Some(0);
    }
    if completed == 0 || elapsed_millis == 0 {
        return None;
    }
    let remaining_millis = u128::from(elapsed_millis) * remaining as u128 / completed as u128;
    Some(((remaining_millis + 999) / 1000).min(u64::MAX as u128) as u64)
}

const MAX_TRANSLATION_RETRIES: usize = 3;

fn should_retry_http_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
}

async fn report_translation_retry(
    window: &tauri::Window,
    job: &Job,
    retry_number: usize,
    reason: &str,
) -> Result<(), String> {
    window
        .emit(
            "translation-progress",
            Progress {
                job_id: job.id.clone(),
                completed: job.completed_chunks,
                total: job.total_chunks,
                estimated_remaining_seconds: estimate_remaining_seconds(
                    job.completed_chunks,
                    job.total_chunks,
                    job.elapsed_translation_millis,
                ),
                message: format!(
                    "Ollama request failed ({reason}). Retrying in {}s (retry {retry_number}/{MAX_TRANSLATION_RETRIES})…",
                    2u64.pow(retry_number as u32)
                ),
            },
        )
        .map_err(|error| format!("reporting automatic retry: {error}"))?;
    tokio::time::sleep(Duration::from_secs(2u64.pow(retry_number as u32))).await;
    Ok(())
}

fn build_translation_prompt(
    template: &str,
    source_language: &str,
    genre: &str,
    tones: &[String],
    instructions: &str,
    context: &str,
    glossary: &str,
) -> String {
    let genre = if genre.is_empty() {
        String::new()
    } else {
        format!(" The story genre is: {genre}.")
    };
    let tones = if tones.is_empty() {
        String::new()
    } else {
        format!(" Apply these translation tones: {}.", tones.join(", "))
    };
    let instructions = if instructions.is_empty() {
        String::new()
    } else {
        format!(" Follow these additional instructions: {instructions}")
    };
    let context = if context.is_empty() {
        String::new()
    } else {
        let context = bound_context_text(context, MAX_CONTEXT_PROMPT_CHARS);
        format!(
            " Established story context (use for continuity; do not contradict or restate it unless the source does):\n{context}"
        )
    };
    let glossary = if glossary.is_empty() {
        String::new()
    } else {
        format!(
            " Apply these glossary terms consistently, using the specified English translation whenever the source term appears:\n{glossary}"
        )
    };
    let has_context_placeholder = template.contains("{context}");
    let prompt = template
        .replace("{source_language}", source_language)
        .replace("{genre}", &genre)
        .replace("{tones}", &tones)
        .replace("{instructions}", &instructions)
        .replace("{context}", &context)
        .replace("{glossary}", &glossary);
    if !has_context_placeholder {
        format!("{prompt}{context}")
    } else {
        prompt
    }
}

fn translated_with_source_breaks(source_chunks: &[String], translated_chunks: &[String]) -> String {
    let mut output = String::new();
    for (index, translated) in translated_chunks.iter().enumerate() {
        if index > 0 {
            let source_chunk = &source_chunks[index - 1];
            let suffix = source_chunk
                .chars()
                .rev()
                .take_while(|character| character.is_whitespace())
                .collect::<String>();
            output.extend(suffix.chars().rev());
        }

        output.push_str(translated.trim());
    }

    output
}

fn without_duplicate_export_heading<'a>(title: &str, translated: &'a str) -> &'a str {
    let Some(first_line_end) = translated.find(['\r', '\n']) else {
        return translated;
    };
    let first_line = translated[..first_line_end].trim();
    let normalize = |value: &str| {
        value
            .chars()
            .filter(|character| character.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    if !first_line.is_empty() && normalize(first_line) == normalize(title) {
        return translated[first_line_end..].trim_start_matches(['\r', '\n']);
    } else {
        return translated;
    }
}

fn export_section_heading<'a>(section_title: &str, translated: &'a str) -> (String, &'a str) {
    let translated = without_duplicate_export_heading(section_title, translated);
    let trimmed = translated.trim_start();
    let Some(line_end) = trimmed.find(['\r', '\n']) else {
        return (section_title.to_string(), translated);
    };
    let first_line = trimmed[..line_end].trim();
    if first_line.is_empty() || first_line.chars().count() > 160 {
        return (section_title.to_string(), translated);
    }
    let normalized_title = first_line
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    let normalized_section = section_title
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if normalized_title == normalized_section {
        return (
            section_title.to_string(),
            trimmed[line_end..].trim_start_matches(['\r', '\n']),
        );
    }
    (
        format!("{section_title} — {first_line}"),
        trimmed[line_end..].trim_start_matches(['\r', '\n']),
    )
}

fn html_text(bytes: &[u8]) -> Result<String> {
    let html = std::str::from_utf8(bytes).context("reading EPUB chapter as UTF-8")?;
    let document = Html::parse_document(html);
    let selector =
        Selector::parse("body").map_err(|error| anyhow!("invalid HTML selector: {error}"))?;
    let root = document
        .select(&selector)
        .next()
        .unwrap_or_else(|| document.root_element());
    let mut text = String::new();
    append_html_text(root, &mut text);
    Ok(clean_text(&text))
}

fn append_html_text(element: ElementRef<'_>, output: &mut String) {
    for child in element.children() {
        let Some(child_element) = ElementRef::wrap(child) else {
            if let Some(text) = child.value().as_text() {
                output.push_str(text);
            }
            continue;
        };
        let tag = child_element.value().name();
        if tag == "br" {
            output.push('\n');
            continue;
        }
        let block = matches!(
            tag,
            "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "li" | "blockquote" | "pre"
        );
        if block && !output.is_empty() {
            while output.ends_with('\n') {
                output.pop();
            }
            output.push_str("\n\n");
        }
        append_html_text(child_element, output);
        if block {
            output.push_str("\n\n");
        }
    }
}

fn read_source(path: &Path, assets_dir: &Path) -> Result<Vec<Section>> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match extension.as_str() {
        "txt" => {
            let text =
                fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            let text = clean_text(&text);
            if text.is_empty() {
                return Err(anyhow!("{} contains no readable text", path.display()));
            }
            Ok(vec![Section {
                title: source_section_title(path, "Chapter"),
                text,
                translated: None,
                translated_chunks: Vec::new(),
                images: Vec::new(),
                context_updated: false,
            }])
        }
        "pdf" => {
            let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            let text = clean_text(
                &pdf_extract::extract_text_from_mem(&bytes)
                    .context("extracting readable text from PDF")?,
            );
            if text.is_empty() {
                return Err(anyhow!(
                    "{} contains no extractable text; scanned PDFs require OCR",
                    path.display()
                ));
            }
            Ok(vec![Section {
                title: source_section_title(path, "Document"),
                text,
                translated: None,
                translated_chunks: Vec::new(),
                images: Vec::new(),
                context_updated: false,
            }])
        }
        "epub" => read_epub(path, assets_dir),
        _ => Err(anyhow!("unsupported source file: {}", path.display())),
    }
}

fn source_section_title(path: &Path, fallback: &str) -> String {
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(fallback);
    // Library source copies are named 000_<original name> or 0000_<appended name>.
    stem.split_once('_')
        .filter(|(prefix, rest)| {
            !rest.is_empty()
                && (prefix.len() == 3 || prefix.len() == 4)
                && prefix.bytes().all(|b| b.is_ascii_digit())
        })
        .map(|(_, title)| title)
        .unwrap_or(stem)
        .to_string()
}

fn epub_source_key(path: &Path) -> String {
    let key = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("source")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(90)
        .collect::<String>();
    if key.is_empty() {
        "source".into()
    } else {
        key
    }
}

fn read_epub(path: &Path, assets_dir: &Path) -> Result<Vec<Section>> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut archive = ZipArchive::new(Cursor::new(bytes)).context("opening EPUB archive")?;
    let mut names = Vec::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index).context("reading EPUB entry")?;
        let name = file.name().to_string();
        if file.enclosed_name().is_none() {
            return Err(anyhow!("EPUB contains an unsafe entry path"));
        }
        let lower = name.to_ascii_lowercase();
        if !file.is_dir()
            && (lower.ends_with(".xhtml") || lower.ends_with(".html") || lower.ends_with(".htm"))
        {
            names.push(name);
        }
    }
    names.sort_by(|left, right| natural_cmp(left, right));
    let mut sections = Vec::new();
    let mut image_index = 0;
    let image_selector =
        Selector::parse("img[src]").map_err(|error| anyhow!("invalid image selector: {error}"))?;
    for name in names {
        let mut entry = archive
            .by_name(&name)
            .with_context(|| format!("opening EPUB chapter {name}"))?;
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading EPUB chapter {name}"))?;
        let text = html_text(&bytes)
            .with_context(|| format!("extracting text from EPUB chapter {name}"))?;
        let chapter_html = std::str::from_utf8(&bytes).context("reading EPUB chapter as UTF-8")?;
        let html_document = Html::parse_document(chapter_html);
        let references = html_document
            .select(&image_selector)
            .filter_map(|image| {
                Some((
                    image.value().attr("src")?.to_string(),
                    image.value().attr("alt").unwrap_or("").to_string(),
                ))
            })
            .collect::<Vec<_>>();
        drop(entry);
        let mut images = Vec::new();
        for (reference, alt) in references {
            let Some(asset_path) = resolve_epub_asset(&name, &reference) else {
                continue;
            };
            let Ok(mut asset) = archive.by_name(&asset_path) else {
                continue;
            };
            let extension = Path::new(&asset_path)
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("bin")
                .to_ascii_lowercase();
            if !matches!(
                extension.as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg"
            ) {
                continue;
            }
            // Scope extracted images to their EPUB so later chapter imports cannot overwrite them.
            let source_key = epub_source_key(path);
            let file_name = format!("{source_key}-image-{:04}.{}", image_index + 1, extension);
            let mut image_bytes = Vec::new();
            asset
                .read_to_end(&mut image_bytes)
                .context("reading EPUB image")?;
            fs::create_dir_all(assets_dir).context("creating extracted EPUB image directory")?;
            fs::write(assets_dir.join(&file_name), image_bytes)
                .context("saving extracted EPUB image")?;
            images.push(ImageAsset { file_name, alt });
            image_index += 1;
        }
        if !text.is_empty() {
            let title = Path::new(&name)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("Chapter");
            sections.push(Section {
                title: display_section_title(title),
                text,
                translated: None,
                translated_chunks: Vec::new(),
                images,
                context_updated: false,
            });
        }
    }
    if sections.is_empty() {
        return Err(anyhow!(
            "no readable XHTML/HTML chapters were found in this EPUB"
        ));
    }

    Ok(sections)
}

fn display_section_title(title: &str) -> String {
    let lower = title.to_ascii_lowercase();
    let digits = lower
        .strip_prefix("chapter")
        .or_else(|| lower.strip_prefix("ch"))
        .map(|value| value.trim_start_matches([' ', '_', '-']));
    if let Some(digits) = digits {
        if !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit()) {
            if let Ok(number) = digits.parse::<u32>() {
                return format!("Chapter {number}");
            }
        }
    }
    title.to_string()
}

fn resolve_epub_asset(chapter: &str, reference: &str) -> Option<String> {
    if reference.starts_with('/') || reference.contains("://") {
        return None;
    }
    let relative = reference.split(['?', '#']).next().unwrap_or_default();
    let mut components = chapter.split('/').collect::<Vec<_>>();
    components.pop();
    for component in relative.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            value => components.push(value),
        }
    }
    (!components.is_empty()).then(|| components.join("/"))
}

#[tauri::command]
async fn list_ollama_models(endpoint: String) -> Result<Vec<String>, String> {
    let url = endpoint_url(&endpoint, "api/tags").map_err(|error| error.to_string())?;
    let client = Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("connecting to Ollama: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Ollama returned HTTP {}", response.status()));
    }
    let mut tags: OllamaTags = response
        .json()
        .await
        .map_err(|error| format!("reading Ollama model list: {error}"))?;
    tags.models
        .sort_by(|left, right| left.name.cmp(&right.name));
    Ok(tags.models.into_iter().map(|model| model.name).collect())
}

fn endpoint_url(endpoint: &str, path: &str) -> Result<Url> {
    let mut url = Url::parse(endpoint).context("invalid Ollama endpoint URL")?;
    let mut segments = url
        .path_segments_mut()
        .map_err(|_| anyhow!("invalid Ollama endpoint URL"))?;
    segments.pop_if_empty();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        segments.push(segment);
    }
    drop(segments);
    Ok(url)
}

#[tauri::command]
fn create_job(
    app: AppHandle,
    paths: Vec<String>,
    source_language: String,
    output_format: String,
    genre: String,
    tones: Vec<String>,
    instructions: String,
    model: String,
) -> Result<Job, String> {
    create_job_from_paths(
        &app,
        paths,
        source_language,
        output_format,
        genre,
        tones,
        instructions,
        model,
        None,
        None,
        0,
    )
}

fn create_job_from_paths(
    app: &AppHandle,
    paths: Vec<String>,
    source_language: String,
    output_format: String,
    genre: String,
    tones: Vec<String>,
    instructions: String,
    model: String,
    library_id: Option<String>,
    queue_status: Option<String>,
    queue_order: usize,
) -> Result<Job, String> {
    validate_input_paths(&paths).map_err(|error| error.to_string())?;
    if !matches!(
        source_language.as_str(),
        "Korean" | "Japanese" | "Auto-detect"
    ) {
        return Err("Source language must be Korean, Japanese, or Auto-detect.".into());
    }
    if !matches!(output_format.as_str(), "epub" | "txt") {
        return Err("Output format must be EPUB or TXT.".into());
    }
    let id = Uuid::new_v4().to_string();
    let assets_dir = jobs_dir(app)
        .map_err(|error| error.to_string())?
        .join(&id)
        .join("assets");
    let mut sections = Vec::new();
    for path in &paths {
        sections
            .extend(read_source(Path::new(path), &assets_dir).map_err(|error| error.to_string())?);
    }
    if sections
        .iter()
        .all(|section| section.text.trim().is_empty())
    {
        return Err("The selected files contain no readable text.".into());
    }
    let settings =
        load_settings(app).map_err(|error| format!("Loading application settings: {error}"))?;
    let context_limit = settings.context_limit.clamp(1, MAX_CONTEXT_FACTS);
    let detected_language = detect_source_language(
        &sections
            .iter()
            .map(|section| section.text.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        &settings.default_source_language,
    );
    let source_language = if source_language == "Auto-detect" {
        detected_language
    } else {
        source_language
    };
    let title = paths
        .first()
        .and_then(|path| Path::new(path).file_stem())
        .and_then(|value| value.to_str())
        .unwrap_or("Translated book")
        .to_string();
    let count = sections.len();
    let total_chunks = sections
        .iter()
        .map(|section| split_translation_chunks(&section.text, 3000).len())
        .sum();
    let translation_prompt = settings.translation_prompt;
    let glossary = load_glossary(app)
        .map_err(|error| format!("Loading glossary: {error}"))?
        .into_iter()
        .filter(|entry| entry.source_language == source_language)
        .collect();
    let context = library_id
        .as_deref()
        .map(|book_id| load_library_book(app, book_id).map(|book| book.context))
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let job = Job {
        id,
        title,
        source_language,
        output_format,
        genre,
        tones,
        instructions,
        model,
        sections,
        completed_sections: 0,
        section_count: count,
        completed_chunks: 0,
        total_chunks,
        elapsed_translation_millis: 0,
        current_section_index: None,
        library_id,
        queue_status,
        queue_order,
        translation_prompt,
        glossary,
        context,
        context_update_failures: Vec::new(),
        context_limit,
    };
    save_job(app, &job).map_err(|error| error.to_string())?;
    Ok(job)
}

fn validate_input_paths(paths: &[String]) -> Result<()> {
    if paths.is_empty() {
        return Err(anyhow!("Choose at least one source file."));
    }
    for path in paths {
        let extension = Path::new(path)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(extension.as_str(), "epub" | "pdf" | "txt") {
            return Err(anyhow!("Unsupported source file: {}", path));
        }
    }
    if paths.len() > 1
        && paths.iter().any(|path| {
            Path::new(path)
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                != "txt"
        })
    {
        return Err(anyhow!(
            "Choose one EPUB or PDF, or combine multiple TXT files."
        ));
    }
    Ok(())
}

#[tauri::command]
fn get_app_settings(app: AppHandle) -> Result<AppSettings, String> {
    load_settings(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn update_app_settings(
    app: AppHandle,
    ollama_endpoint: String,
    default_source_language: String,
    default_output_format: String,
    default_model: String,
    auto_retry: bool,
    interactive_translation: bool,
    translation_prompt: String,
    dark_mode: bool,
    context_limit: usize,
) -> Result<AppSettings, String> {
    let endpoint =
        Url::parse(&ollama_endpoint).map_err(|error| format!("Invalid Ollama URL: {error}"))?;
    if !matches!(endpoint.scheme(), "http" | "https") || endpoint.host_str().is_none() {
        return Err("Ollama address must be an HTTP or HTTPS URL.".into());
    }
    if !matches!(default_source_language.as_str(), "Japanese" | "Korean") {
        return Err("Default source language must be Japanese or Korean.".into());
    }
    if !matches!(default_output_format.as_str(), "epub" | "txt") {
        return Err("Default output must be EPUB or TXT.".into());
    }
    if translation_prompt.trim().is_empty() {
        return Err("Translation prompt cannot be empty.".into());
    }
    if !(1..=MAX_CONTEXT_FACTS).contains(&context_limit) {
        return Err(format!(
            "Story context facts to keep must be between 1 and {MAX_CONTEXT_FACTS}."
        ));
    }
    let settings = AppSettings {
        ollama_endpoint: ollama_endpoint.trim_end_matches('/').to_string(),
        default_source_language,
        default_output_format,
        default_model,
        auto_retry,
        interactive_translation,
        translation_prompt: translation_prompt.trim().to_string(),
        dark_mode,
        context_limit,
    };
    save_settings(&app, &settings).map_err(|error| error.to_string())?;
    Ok(settings)
}

#[tauri::command]
fn write_text_file(path: String, text: String) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("Choose a destination file.".into());
    }
    fs::write(Path::new(&path), text).map_err(|error| format!("Writing text file: {error}"))
}

#[tauri::command]
fn list_library(app: AppHandle) -> Result<Vec<LibraryBook>, String> {
    let directory = library_dir(&app).map_err(|error| error.to_string())?;
    let mut books = Vec::new();
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry.path().extension().and_then(|value| value.to_str()) == Some("json") {
            let book: LibraryBook =
                serde_json::from_slice(&fs::read(entry.path()).map_err(|error| error.to_string())?)
                    .map_err(|error| format!("Reading library entry: {error}"))?;
            books.push(book);
        }
    }
    books.sort_by(|left, right| left.title.to_lowercase().cmp(&right.title.to_lowercase()));
    Ok(books)
}

fn normalize_labels(labels: Vec<String>) -> Result<Vec<String>, String> {
    const MAX_LABELS: usize = 500;
    // Count unique, nonempty labels rather than raw entries: source sites may repeat tags.
    if labels.len() > 2_000 {
        return Err("Too many tag or genre entries (maximum 2,000 before deduplication).".into());
    }
    let mut output = Vec::new();
    for label in labels {
        let value = label.split_whitespace().collect::<Vec<_>>().join(" ");
        if value.chars().count() > 80 {
            return Err("Tags and genres must be at most 80 characters.".into());
        }
        if !value.is_empty()
            && !output
                .iter()
                .any(|item: &String| item.to_lowercase() == value.to_lowercase())
        {
            if output.len() >= MAX_LABELS {
                return Err(format!("Use at most {MAX_LABELS} unique tags or genres."));
            }
            output.push(value);
        }
    }
    Ok(output)
}

#[tauri::command]
fn update_library_metadata(
    app: AppHandle,
    book_id: String,
    tags: Vec<String>,
    publication_status: String,
    synopsis: String,
    author: String,
    original_title: String,
    genres: Vec<String>,
) -> Result<LibraryBook, String> {
    if !matches!(
        publication_status.as_str(),
        "Unknown" | "Ongoing" | "Completed" | "Hiatus" | "Dropped"
    ) {
        return Err("Invalid publication status.".into());
    }
    if synopsis.chars().count() > 10000
        || author.chars().count() > 200
        || original_title.chars().count() > 200
    {
        return Err("Novel metadata is too long.".into());
    }
    let mut book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    book.tags = normalize_labels(tags)?;
    book.genres = normalize_labels(genres)?;
    book.publication_status = publication_status;
    book.synopsis = synopsis.trim().into();
    book.author = author.trim().into();
    book.original_title = original_title.trim().into();
    save_library_book(&app, &book).map_err(|error| error.to_string())?;
    Ok(book)
}

#[tauri::command]
fn update_library_title(
    app: AppHandle,
    book_id: String,
    title: String,
) -> Result<LibraryBook, String> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 200 {
        return Err("Novel title must be 1–200 characters.".into());
    }
    let mut book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    if let Some(job_id) = &book.job_id {
        let mut overrides = load_job_overrides(&app, job_id).map_err(|error| error.to_string())?;
        overrides.novel_title = Some(title.to_string());
        save_job_overrides(&app, job_id, &overrides).map_err(|error| error.to_string())?;
        if let Ok(mut job) = load_job(&app, job_id) {
            if job.queue_status.as_deref() != Some("Translating") {
                job.title = title.to_string();
                save_job(&app, &job).map_err(|error| error.to_string())?;
            }
        }
    }
    book.title = title.into();
    save_library_book(&app, &book).map_err(|error| error.to_string())?;
    Ok(book)
}

#[tauri::command]
fn save_library_context(
    app: AppHandle,
    book_id: String,
    context: Vec<String>,
) -> Result<LibraryBook, String> {
    let mut book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    if let Some(job_id) = &book.job_id {
        let job_file = job_path(&app, job_id).map_err(|error| error.to_string())?;
        if job_file.exists() {
            let job = load_job(&app, job_id).map_err(|error| error.to_string())?;
            if job.queue_status.as_deref() == Some("Translating") {
                return Err(
                    "Wait for the active translation to finish before editing story context."
                        .into(),
                );
            }
        }
    }
    let context = normalize_story_context(context)?;
    book.context = context.clone();
    if let Some(job_id) = &book.job_id {
        let path = job_path(&app, job_id).map_err(|error| error.to_string())?;
        if path.exists() {
            let mut job = load_job(&app, job_id).map_err(|error| error.to_string())?;
            job.context = context;
            save_job(&app, &job).map_err(|error| error.to_string())?;
        }
    }
    save_library_book(&app, &book).map_err(|error| error.to_string())?;
    Ok(book)
}

fn normalize_story_context(context: Vec<String>) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for (index, fact) in context.into_iter().enumerate() {
        let fact = fact.split_whitespace().collect::<Vec<_>>().join(" ");
        if fact.is_empty() {
            continue;
        }
        if normalized.len() == MAX_CONTEXT_FACTS {
            return Err(format!(
                "A novel can have at most {MAX_CONTEXT_FACTS} context facts."
            ));
        }
        if fact.chars().count() > 500 {
            return Err(format!(
                "Context fact {} is longer than 500 characters.",
                index + 1
            ));
        }
        if normalized
            .iter()
            .any(|existing: &String| existing.to_lowercase() == fact.to_lowercase())
        {
            return Err(format!("Context fact {} is duplicated.", index + 1));
        }
        normalized.push(fact);
    }
    Ok(normalized)
}

#[tauri::command]
fn import_library_book(
    app: AppHandle,
    paths: Vec<String>,
    source_language: String,
) -> Result<LibraryBook, String> {
    validate_input_paths(&paths).map_err(|error| error.to_string())?;
    if !matches!(
        source_language.as_str(),
        "Korean" | "Japanese" | "Auto-detect"
    ) {
        return Err("Source language must be Korean, Japanese, or Auto-detect.".into());
    }
    let settings = load_settings(&app).map_err(|error| error.to_string())?;
    let id = Uuid::new_v4().to_string();
    let book_dir = library_dir(&app)
        .map_err(|error| error.to_string())?
        .join(&id);
    fs::create_dir_all(&book_dir).map_err(|error| format!("Creating library folder: {error}"))?;
    let detection_assets = book_dir.join("language-detection-assets");
    let mut source_text = String::new();
    for source in &paths {
        let sections = read_source(Path::new(source), &detection_assets)
            .map_err(|error| format!("Detecting source language: {error}"))?;
        for section in sections {
            source_text.push_str(&section.text);
            source_text.push('\n');
        }
    }
    if detection_assets.exists() {
        fs::remove_dir_all(&detection_assets)
            .map_err(|error| format!("Cleaning temporary language-detection files: {error}"))?;
    }
    let detected_language = detect_source_language(&source_text, &settings.default_source_language);
    let source_language = if source_language == "Auto-detect" {
        detected_language
    } else {
        source_language
    };
    let mut stored_paths = Vec::new();
    for (index, source) in paths.iter().enumerate() {
        let source = Path::new(source);
        let filename = source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("book.txt");
        let target = book_dir.join(format!("{index:03}_{filename}"));
        fs::copy(source, &target)
            .map_err(|error| format!("Importing {}: {error}", source.display()))?;
        stored_paths.push(target.to_string_lossy().to_string());
    }
    let title = Path::new(&paths[0])
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Novel")
        .to_string();
    let book = LibraryBook {
        id,
        title,
        files: stored_paths,
        source_language,
        status: "Not translated".into(),
        job_id: None,
        glossary: Vec::new(),
        cover_image: None,
        context: Vec::new(),
        output_format: default_library_output_format(),
        tags: Vec::new(),
        publication_status: default_publication_status(),
        synopsis: String::new(),
        author: String::new(),
        original_title: String::new(),
        genres: Vec::new(),
        raw_source_url: None,
        raw_tags_en: Vec::new(),
        raw_tags_ko: Vec::new(),
        novel_updates_url: None,
    };
    save_library_book(&app, &book).map_err(|error| error.to_string())?;
    Ok(book)
}

#[tauri::command]
fn replace_library_book(
    app: AppHandle,
    book_id: String,
    paths: Vec<String>,
    source_language: String,
) -> Result<LibraryBook, String> {
    let existing = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    if matches!(existing.status.as_str(), "Queued" | "Translating") {
        return Err(
            "Remove the novel from the active queue before replacing its source files.".into(),
        );
    }
    let glossary = existing.glossary;
    let context = existing.context;
    let output_format = existing.output_format;
    let metadata = (
        existing.tags,
        existing.publication_status,
        existing.synopsis,
        existing.author,
        existing.original_title,
        existing.genres,
        existing.raw_source_url,
        existing.raw_tags_en,
        existing.raw_tags_ko,
        existing.novel_updates_url,
    );
    let cover = existing.cover_image.as_ref().and_then(|file_name| {
        let path = library_dir(&app).ok()?.join(&book_id).join(file_name);
        Some((file_name.clone(), fs::read(path).ok()?))
    });
    delete_library_book(app.clone(), book_id)?;
    let mut replacement = import_library_book(app.clone(), paths, source_language)?;
    replacement.glossary = glossary;
    replacement.context = context;
    replacement.output_format = output_format;
    (
        replacement.tags,
        replacement.publication_status,
        replacement.synopsis,
        replacement.author,
        replacement.original_title,
        replacement.genres,
        replacement.raw_source_url,
        replacement.raw_tags_en,
        replacement.raw_tags_ko,
        replacement.novel_updates_url,
    ) = metadata;
    if let Some((file_name, bytes)) = cover {
        let directory = library_dir(&app)
            .map_err(|error| error.to_string())?
            .join(&replacement.id);
        fs::write(directory.join(&file_name), bytes)
            .map_err(|error| format!("Restoring cover image: {error}"))?;
        replacement.cover_image = Some(file_name);
    }
    save_library_book(&app, &replacement).map_err(|error| error.to_string())?;
    Ok(replacement)
}

fn reconcile_appended_sections(job: &mut Job, sections: Vec<Section>) -> Result<(), String> {
    if matches!(job.queue_status.as_deref(), Some("Queued" | "Translating")) {
        return Err("Wait until the novel leaves the active queue before adding chapters.".into());
    }
    job.sections.extend(sections);
    job.section_count = job.sections.len();
    job.completed_sections = job
        .sections
        .iter()
        .filter(|section| section.translated.is_some())
        .count();
    (job.completed_chunks, job.total_chunks) = job_chunk_counts(job);
    job.queue_status = None;
    Ok(())
}

fn append_sources(
    app: &AppHandle,
    book_id: &str,
    sources: Vec<(String, Vec<u8>)>,
) -> Result<LibraryBook, String> {
    let mut book = load_library_book(app, book_id).map_err(|error| error.to_string())?;
    let mut job = book
        .job_id
        .as_deref()
        .map(|id| load_job(app, id).map_err(|e| e.to_string()))
        .transpose()?;
    if job
        .as_ref()
        .is_some_and(|job| matches!(job.queue_status.as_deref(), Some("Queued" | "Translating")))
    {
        return Err("Wait until the novel leaves the active queue before adding chapters.".into());
    }
    let directory = library_dir(app)
        .map_err(|error| error.to_string())?
        .join(&book.id);
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let mut staged = Vec::new();
    let mut new_sections = Vec::new();
    let mut temporary_assets = Vec::new();
    let mut staged_asset_keys = Vec::new();
    let assets_dir = library_dir(app)
        .map_err(|e| e.to_string())?
        .join(&book.id)
        .join("illustrations");
    let result: Result<(), String> = (|| {
        for (offset, (name, bytes)) in sources.iter().enumerate() {
            let name = Path::new(name)
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or("Invalid chapter filename.")?;
            let ext = Path::new(name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !matches!(ext.as_str(), "txt" | "pdf" | "epub") {
                return Err("Choose EPUB, PDF or TXT chapters.".into());
            }
            let destination = directory.join(format!("{:04}_{}", book.files.len() + offset, name));
            let key = epub_source_key(&destination);
            if ext == "epub" {
                if staged_asset_keys.contains(&key)
                    || book
                        .files
                        .iter()
                        .any(|file| epub_source_key(Path::new(file)) == key)
                {
                    return Err("This EPUB filename would overwrite existing chapter images; rename the EPUB and retry.".into());
                }
                staged_asset_keys.push(key);
            }
            fs::write(&destination, bytes).map_err(|e| format!("Saving chapter: {e}"))?;
            staged.push(destination.clone());
            let mut sections = read_source(&destination, &assets_dir).map_err(|e| e.to_string())?;
            temporary_assets.extend(sections.iter().flat_map(|section| {
                section
                    .images
                    .iter()
                    .map(|image| assets_dir.join(&image.file_name))
            }));
            new_sections.append(&mut sections);
        }
        if new_sections.is_empty() {
            return Err("No readable chapters found.".into());
        }
        Ok(())
    })();
    if let Err(error) = result {
        for path in staged {
            let _ = fs::remove_file(path);
        }
        for path in temporary_assets {
            let _ = fs::remove_file(path);
        }
        return Err(error);
    }
    if let Some(ref mut job) = job {
        reconcile_appended_sections(job, new_sections)?;
        if let Err(error) = save_job(app, job) {
            for path in &staged {
                let _ = fs::remove_file(path);
            }
            for path in &temporary_assets {
                let _ = fs::remove_file(path);
            }
            return Err(error.to_string());
        }
        book.status = "Not translated".into();
    }
    book.files
        .extend(staged.iter().map(|p| p.to_string_lossy().to_string()));
    save_library_book(app, &book).map_err(|e| e.to_string())?;
    Ok(book)
}

#[tauri::command]
fn append_library_chapter(
    app: AppHandle,
    book_id: String,
    title: String,
    text: String,
) -> Result<LibraryBook, String> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 160 {
        return Err("Enter a chapter title of at most 160 characters.".into());
    }
    if text.trim().is_empty() {
        return Err("Chapter content cannot be empty.".into());
    }
    let filename = safe_epub_name(title).trim_end_matches(".epub").to_string();
    let book = append_sources(
        &app,
        &book_id,
        vec![(format!("{filename}.txt"), text.into_bytes())],
    )?;
    if let Some(job_id) = &book.job_id {
        let mut job = load_job(&app, job_id).map_err(|e| e.to_string())?;
        if let Some(section) = job.sections.last_mut() {
            section.title = title.to_string();
        }
        save_job(&app, &job).map_err(|e| e.to_string())?;
    }
    Ok(book)
}

#[tauri::command]
fn append_library_chapter_files(
    app: AppHandle,
    book_id: String,
    paths: Vec<String>,
) -> Result<LibraryBook, String> {
    validate_input_paths(&paths).map_err(|e| e.to_string())?;
    let sources = paths
        .iter()
        .map(|path| {
            let file = Path::new(path);
            Ok((
                file.file_name()
                    .and_then(|n| n.to_str())
                    .ok_or("Invalid filename.")?
                    .to_string(),
                fs::read(file).map_err(|e| e.to_string())?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    append_sources(&app, &book_id, sources)
}

const RAW_HOST: &str = "www.raw-fucknovelpia.com";
const RAW_PASSWORD: &[u8] = b"taiwanandnorthkorea";
const MAX_RAW_BYTES: usize = 120 * 1024 * 1024;
static RAW_TITLE_CACHE: OnceLock<Mutex<Option<(Instant, Vec<String>)>>> = OnceLock::new();

fn parse_raw_titles(html: &str) -> Result<Vec<String>, String> {
    let start = html
        .find("const titles = [")
        .ok_or_else(|| "RAW title suggestions are unavailable on this page.".to_string())?;
    let json = &html[start + "const titles = ".len()..];
    let end = json
        .find(";\n    if (!Array.isArray(titles)")
        .or_else(|| json.find(";\r\n    if (!Array.isArray(titles)"))
        .ok_or_else(|| "RAW title suggestions have an unexpected format.".to_string())?;
    let values: Vec<serde_json::Value> = serde_json::from_str(&json[..end])
        .map_err(|error| format!("Reading RAW title suggestions: {error}"))?;
    Ok(values
        .into_iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect())
}

fn match_raw_titles(titles: &[String], query: &str) -> Vec<String> {
    let query = query.trim().to_lowercase();
    if query.chars().count() < 4 {
        return Vec::new();
    }
    titles
        .iter()
        .filter(|title| title.to_lowercase().contains(&query))
        .take(12)
        .cloned()
        .collect()
}

#[tauri::command]
async fn suggest_raw_titles(query: String) -> Result<Vec<String>, String> {
    if query.chars().count() > 200 {
        return Err("Search text must be at most 200 characters.".into());
    }
    if query.trim().chars().count() < 4 {
        return Ok(Vec::new());
    }
    let cache = RAW_TITLE_CACHE.get_or_init(|| Mutex::new(None));
    {
        let cached = cache
            .lock()
            .map_err(|_| "RAW title cache is unavailable.")?;
        if let Some((updated, titles)) = cached.as_ref() {
            if updated.elapsed() < Duration::from_secs(30 * 60) {
                return Ok(match_raw_titles(titles, &query));
            }
        }
    }
    let url = Url::parse("https://www.raw-fucknovelpia.com/index.php?view=classic&view_lang=en")
        .map_err(|error| error.to_string())?;
    let bytes = raw_get(&raw_client()?, url, 8 * 1024 * 1024).await?;
    let html =
        std::str::from_utf8(&bytes).map_err(|error| format!("Reading RAW search page: {error}"))?;
    let titles = parse_raw_titles(html)?;
    let suggestions = match_raw_titles(&titles, &query);
    *cache
        .lock()
        .map_err(|_| "RAW title cache is unavailable.")? = Some((Instant::now(), titles));
    Ok(suggestions)
}

#[derive(Clone, Serialize)]
struct RawSearchResult {
    url: String,
    is_raw: bool,
    title: String,
    original_title: String,
    cover_url: String,
    publication_status: String,
    tags: Vec<String>,
    raw_tags_en: Vec<String>,
    raw_tags_ko: Vec<String>,
}
#[derive(Serialize)]
struct RawSearchPage {
    results: Vec<RawSearchResult>,
    page: usize,
    next_page: Option<usize>,
    total: Option<usize>,
}
#[derive(Serialize)]
struct RawNovelDetails {
    url: String,
    title: String,
    original_title: String,
    cover_url: String,
    publication_status: String,
    synopsis: String,
    author: String,
    tags: Vec<String>,
    raw_tags_en: Vec<String>,
    raw_tags_ko: Vec<String>,
    genres: Vec<String>,
    download_url: String,
}

fn raw_url(value: &str, kind: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|e| format!("Invalid RAW URL: {e}"))?;
    if url.scheme() != "https"
        || (url.host_str() != Some(RAW_HOST)
            && !(kind == "cover" && url.host_str() == Some("assets.raw-fucknovelpia.com")))
        || url.port().is_some()
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("RAW URL must use the official HTTPS host.".into());
    }
    let valid = match kind {
        "novel" => {
            url.path().strip_prefix("/novel/raw-").is_some_and(|id| {
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            }) && url.query().is_none()
        }
        "download" => {
            url.path() == "/download.php"
                && url.query_pairs().any(|(k, v)| {
                    k == "slug"
                        && v.strip_prefix("raw-").is_some_and(|id| {
                            !id.is_empty()
                                && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                        })
                })
        }
        "cover" => {
            url.host_str() == Some("assets.raw-fucknovelpia.com")
                && url.path().starts_with("/covers/")
                && url.query().is_none()
        }
        _ => false,
    };
    if !valid {
        return Err("Unexpected RAW URL path.".into());
    }
    Ok(url)
}

fn select_text(node: ElementRef<'_>, selector: &str) -> String {
    Selector::parse(selector)
        .ok()
        .and_then(|s| node.select(&s).next())
        .map(|el| {
            el.text()
                .collect::<Vec<_>>()
                .join(" ")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}
fn select_attr(node: ElementRef<'_>, selector: &str, attr: &str) -> String {
    Selector::parse(selector)
        .ok()
        .and_then(|s| node.select(&s).next())
        .and_then(|el| el.value().attr(attr))
        .unwrap_or("")
        .to_string()
}
fn labels(node: ElementRef<'_>, selector: &str) -> Vec<String> {
    Selector::parse(selector)
        .ok()
        .map(|s| {
            node.select(&s)
                .map(|el| el.text().collect::<String>().trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}
fn raw_absolute(base: &Url, link: &str, kind: &str) -> String {
    base.join(link)
        .ok()
        .and_then(|url| raw_url(url.as_str(), kind).ok())
        .map(|url| url.to_string())
        .unwrap_or_default()
}
fn raw_status(value: &str) -> String {
    let word = value
        .split(|c: char| !c.is_alphabetic())
        .find(|s| !s.is_empty())
        .unwrap_or("");
    match word.to_ascii_lowercase().as_str() {
        "ongoing" => "Ongoing",
        "completed" | "complete" => "Completed",
        "hiatus" => "Hiatus",
        "dropped" => "Dropped",
        _ => "Unknown",
    }
    .into()
}
fn parse_raw_results(html: &str, url: &Url, page: usize) -> RawSearchPage {
    let doc = Html::parse_document(html);
    let mut results = Vec::new();
    if let Ok(selector) = Selector::parse(".grid .card") {
        for card in doc.select(&selector) {
            let link = raw_absolute(url, &select_attr(card, ".card-title-link", "href"), "novel");
            if link.is_empty()
                || results
                    .iter()
                    .any(|item: &RawSearchResult| item.url == link)
            {
                continue;
            }
            results.push(RawSearchResult {
                url: link,
                is_raw: true,
                title: select_text(card, ".book-title"),
                original_title: select_text(card, ".book-subtitle"),
                cover_url: raw_absolute(url, &select_attr(card, ".cover img", "src"), "cover"),
                publication_status: raw_status(&select_text(card, ".carousel-meta")),
                tags: labels(card, ".info > .tags a"),
                raw_tags_en: labels(card, ".source-tag-set[data-lang='en'] a"),
                raw_tags_ko: labels(card, ".source-tag-set[data-lang='ko'] a"),
            });
        }
    }
    let next_page = Selector::parse(".pagination a").ok().and_then(|selector| {
        doc.select(&selector)
            .find(|a| a.text().collect::<String>().trim() == "Next ›")
            .and_then(|a| a.value().attr("href"))
            .and_then(|href| url.join(href).ok())
            .and_then(|next| {
                next.query_pairs()
                    .find(|(k, _)| k == "page")
                    .and_then(|(_, v)| v.parse::<usize>().ok())
            })
    });
    RawSearchPage {
        results,
        page,
        next_page,
        total: None,
    }
}

#[derive(Deserialize)]
struct RawAdvancedSearch {
    query: String,
    author: String,
    uploader: String,
    translator_group: String,
    country: String,
    year_from: String,
    year_to: String,
    status: String,
    language: String,
    view_lang: String,
    read_only: String,
    sort: String,
    has_images: bool,
    has_audiobook: bool,
    tag_mode: String,
    tags_include: Vec<String>,
    tags_exclude: Vec<String>,
    source_tag_mode: String,
    source_tags_include: Vec<String>,
    source_tags_exclude: Vec<String>,
    genre_mode: String,
    genres_include: Vec<String>,
    genres_exclude: Vec<String>,
}

#[derive(Serialize)]
struct RawAdvancedOptions {
    tags: Vec<String>,
    source_tags: Vec<String>,
}

fn parse_raw_advanced_options(html: &str) -> RawAdvancedOptions {
    let document = Html::parse_document(html);
    let root = document.root_element();
    let values = |selector: &str| -> Vec<String> {
        Selector::parse(selector)
            .ok()
            .map(|selector| {
                root.select(&selector)
                    .filter_map(|option| option.value().attr("value"))
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    RawAdvancedOptions {
        tags: values("#all-tags option[value]"),
        source_tags: values("#all-source-tags option[value]"),
    }
}

#[tauri::command]
async fn get_raw_advanced_options() -> Result<RawAdvancedOptions, String> {
    let url = Url::parse("https://www.raw-fucknovelpia.com/search.php").unwrap();
    let bytes = raw_get(&raw_client()?, url, 8 * 1024 * 1024).await?;
    Ok(parse_raw_advanced_options(&String::from_utf8_lossy(&bytes)))
}

const RAW_GENRES: &[(&str, &str)] = &[
    ("1", "academy"),
    ("2", "action"),
    ("3", "adventure"),
    ("4", "fantasy"),
    ("5", "horror"),
    ("6", "mystery"),
    ("7", "romance"),
    ("8", "school"),
    ("9", "martial"),
    ("10", "smut"),
    ("11", "adult"),
    ("12", "harem"),
    ("13", "historical"),
    ("14", "scifi"),
    ("15", "sliceoflife"),
    ("16", "sports"),
    ("17", "Uncategorized"),
];

fn raw_advanced_url(filters: &RawAdvancedSearch, page: usize) -> Result<Url, String> {
    if page == 0 || page > 10000 {
        return Err("Invalid search page.".into());
    }
    for field in [
        &filters.query,
        &filters.author,
        &filters.uploader,
        &filters.translator_group,
        &filters.country,
    ] {
        if field.chars().count() > 200 {
            return Err("Search text must be at most 200 characters.".into());
        }
    }
    for (value, options) in [
        (
            filters.status.as_str(),
            &["", "ongoing", "completed", "hiatus", "dropped"][..],
        ),
        (
            filters.language.as_str(),
            &["", "en", "es", "ko", "ja", "zh"],
        ),
        (filters.view_lang.as_str(), &["en", "ko"]),
        (filters.read_only.as_str(), &["any", "yes", "no"]),
        (
            filters.sort.as_str(),
            &["newest", "oldest", "title", "year_desc", "year_asc"],
        ),
        (filters.tag_mode.as_str(), &["and", "or"]),
        (filters.source_tag_mode.as_str(), &["and", "or"]),
        (filters.genre_mode.as_str(), &["and", "or"]),
    ] {
        if !options.contains(&value) {
            return Err("Invalid advanced search option.".into());
        }
    }
    let year = |s: &str| -> Result<Option<u32>, String> {
        if s.is_empty() {
            return Ok(None);
        }
        let value = s
            .parse::<u32>()
            .map_err(|_| "Years must be non-negative whole numbers.".to_string())?;
        if value > 9999 {
            return Err("Year is out of range.".into());
        }
        Ok(Some(value))
    };
    let from = year(&filters.year_from)?;
    let to = year(&filters.year_to)?;
    if from.zip(to).is_some_and(|(from, to)| from > to) {
        return Err("Year from cannot exceed year to.".into());
    }
    for values in [
        &filters.tags_include,
        &filters.tags_exclude,
        &filters.source_tags_include,
        &filters.source_tags_exclude,
        &filters.genres_include,
        &filters.genres_exclude,
    ] {
        if values.len() > 40 {
            return Err("Too many search filters.".into());
        }
    }
    for value in filters
        .tags_include
        .iter()
        .chain(&filters.tags_exclude)
        .chain(&filters.source_tags_include)
        .chain(&filters.source_tags_exclude)
    {
        if value.trim().is_empty() || value.chars().count() > 80 {
            return Err("Tag filters must be 1–80 characters.".into());
        }
    }
    for id in filters.genres_include.iter().chain(&filters.genres_exclude) {
        if !RAW_GENRES.iter().any(|(valid, _)| valid == id) {
            return Err("Invalid genre filter.".into());
        }
    }
    let mut url = Url::parse("https://www.raw-fucknovelpia.com/search.php").unwrap();
    let mut pairs = url.query_pairs_mut();
    pairs.append_pair("view_lang", &filters.view_lang);
    for (key, value) in [
        ("q", filters.query.as_str()),
        ("author", &filters.author),
        ("uploader", &filters.uploader),
        ("translator_group", &filters.translator_group),
        ("country", &filters.country),
        ("year_from", &filters.year_from),
        ("year_to", &filters.year_to),
        ("status", &filters.status),
        ("language", &filters.language),
        ("read_only", &filters.read_only),
        ("sort", &filters.sort),
        ("tag_mode", &filters.tag_mode),
        ("source_tag_mode", &filters.source_tag_mode),
        ("genre_mode", &filters.genre_mode),
    ] {
        if !value.is_empty() {
            pairs.append_pair(key, value.trim());
        }
    }
    if filters.has_images {
        pairs.append_pair("has_images", "1");
    }
    if filters.has_audiobook {
        pairs.append_pair("has_audiobook", "1");
    }
    for (key, values) in [
        ("tags_include[]", &filters.tags_include),
        ("tags_exclude[]", &filters.tags_exclude),
        ("source_tags_include[]", &filters.source_tags_include),
        ("source_tags_exclude[]", &filters.source_tags_exclude),
        ("genres_include[]", &filters.genres_include),
        ("genres_exclude[]", &filters.genres_exclude),
    ] {
        for value in values {
            pairs.append_pair(key, value.trim());
        }
    }
    if page > 1 {
        pairs.append_pair("page", &page.to_string());
    }
    drop(pairs);
    Ok(url)
}

fn parse_raw_advanced_results(html: &str, url: &Url, page: usize) -> RawSearchPage {
    let document = Html::parse_document(html);
    let root = document.root_element();
    let mut results = Vec::new();
    if let Ok(selector) = Selector::parse(".grid .card-book") {
        for card in root.select(&selector) {
            let link = select_attr(card, ":scope > a[href]", "href");
            let Ok(candidate) = url.join(&link) else {
                continue;
            };
            let title = select_text(card, ".title");
            if title.is_empty() {
                continue;
            }
            if candidate.scheme() != "https"
                || candidate.host_str() != Some(RAW_HOST)
                || candidate.port().is_some()
                || candidate.query().is_some()
                || !candidate.path().starts_with("/novel/")
            {
                continue;
            }
            let slug = &candidate.path()["/novel/".len()..];
            if slug.is_empty() || !slug.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
                continue;
            }
            let meta = select_text(card, ".meta");
            results.push(RawSearchResult {
                is_raw: raw_url(candidate.as_str(), "novel").is_ok(),
                url: candidate.to_string(),
                title,
                original_title: select_text(card, ".title-sub"),
                cover_url: raw_absolute(url, &select_attr(card, ".cover img", "src"), "cover"),
                publication_status: raw_status(&meta),
                tags: labels(card, ".tag-group:not([data-source-tag-group]) .tag"),
                raw_tags_en: labels(
                    card,
                    "[data-source-tag-group] .source-tag-set[data-lang='en'] .tag",
                ),
                raw_tags_ko: labels(
                    card,
                    "[data-source-tag-group] .source-tag-set[data-lang='ko'] .tag",
                ),
            });
        }
    }
    let next_page = Selector::parse(".pagination a").ok().and_then(|selector| {
        root.select(&selector)
            .find(|a| a.text().collect::<String>().trim() == "Next ›")
            .and_then(|a| a.value().attr("href"))
            .and_then(|href| url.join(href).ok())
            .filter(|next| {
                next.scheme() == "https"
                    && next.host_str() == Some(RAW_HOST)
                    && next.path() == "/search.php"
            })
            .and_then(|next| {
                next.query_pairs()
                    .find(|(key, _)| key == "page")
                    .and_then(|(_, value)| value.parse::<usize>().ok())
            })
    });
    let total = select_text(root, ".result-count")
        .strip_prefix("Results:")
        .and_then(|count| count.trim().replace(',', "").parse::<usize>().ok());
    RawSearchPage {
        results,
        page,
        next_page,
        total,
    }
}

#[tauri::command]
async fn search_raw_advanced(
    filters: RawAdvancedSearch,
    page: usize,
) -> Result<RawSearchPage, String> {
    let url = raw_advanced_url(&filters, page)?;
    let bytes = raw_get(&raw_client()?, url.clone(), 8 * 1024 * 1024).await?;
    Ok(parse_raw_advanced_results(
        &String::from_utf8_lossy(&bytes),
        &url,
        page,
    ))
}
fn parse_raw_details(html: &str, url: &Url) -> Result<RawNovelDetails, String> {
    let doc = Html::parse_document(html);
    let root = doc.root_element();
    let title = select_text(root, ".hero-title");
    if title.is_empty() {
        return Err("RAW novel page has no title.".into());
    }
    let mut author = String::new();
    let mut publication_status = "Unknown".to_string();
    if let Ok(selector) = Selector::parse(".info-list li") {
        for li in root.select(&selector) {
            let text = li.text().collect::<String>();
            if let Some(value) = text.trim().strip_prefix("Author:") {
                author = value.trim().to_string();
            }
            if let Some(value) = text.trim().strip_prefix("Status:") {
                publication_status = raw_status(value.trim());
            }
        }
    }
    let cover = select_attr(root, "meta[property='og:image']", "content");
    let cover_url = Url::parse(&cover)
        .ok()
        .filter(|u| {
            u.scheme() == "https"
                && u.host_str() == Some("assets.raw-fucknovelpia.com")
                && u.path().starts_with("/covers/")
        })
        .map(|u| u.to_string())
        .unwrap_or_default();
    let download_url = raw_absolute(
        url,
        &select_attr(root, ".novel-download-link", "href"),
        "download",
    );
    Ok(RawNovelDetails {
        url: url.to_string(),
        title,
        original_title: select_text(root, ".hero-subtitle"),
        cover_url,
        publication_status,
        synopsis: select_text(root, ".description"),
        author,
        tags: labels(root, ".tag-group:not([data-source-tag-group]) .tags a"),
        raw_tags_en: labels(
            root,
            "[data-source-tag-group] .source-tag-set[data-lang='en'] a",
        ),
        raw_tags_ko: labels(
            root,
            "[data-source-tag-group] .source-tag-set[data-lang='ko'] a",
        ),
        genres: labels(root, ".genres .genre-pill"),
        download_url,
    })
}

fn raw_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| e.to_string())
}
async fn raw_get(client: &Client, url: Url, max_bytes: usize) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("RAW request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("RAW server returned an error: {e}"))?;
    if response
        .content_length()
        .is_some_and(|len| len > max_bytes as u64)
    {
        return Err("RAW response is too large.".into());
    }
    let mut response = response;
    let mut data = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
        if data.len().saturating_add(chunk.len()) > max_bytes {
            return Err("RAW response is too large.".into());
        }
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}

#[tauri::command]
async fn search_raw_novels(query: String, page: usize) -> Result<RawSearchPage, String> {
    if page == 0 || page > 10000 || query.chars().count() > 200 {
        return Err("Invalid search or page.".into());
    }
    let mut url = Url::parse("https://www.raw-fucknovelpia.com/index.php").unwrap();
    url.query_pairs_mut()
        .append_pair("q", query.trim())
        .append_pair("view", "classic")
        .append_pair("view_lang", "en")
        .append_pair("sort", "newest")
        .append_pair("page", &page.to_string());
    let bytes = raw_get(&raw_client()?, url.clone(), 8 * 1024 * 1024).await?;
    Ok(parse_raw_results(
        &String::from_utf8_lossy(&bytes),
        &url,
        page,
    ))
}
async fn get_raw_details(client: &Client, novel_url: &str) -> Result<RawNovelDetails, String> {
    let url = raw_url(novel_url, "novel")?;
    let bytes = raw_get(client, url.clone(), 4 * 1024 * 1024).await?;
    parse_raw_details(&String::from_utf8_lossy(&bytes), &url)
}
#[tauri::command]
async fn fetch_raw_novel_details(novel_url: String) -> Result<RawNovelDetails, String> {
    get_raw_details(&raw_client()?, &novel_url).await
}

fn validate_raw_zip_response(bytes: &[u8], content_type: Option<&str>) -> Result<(), String> {
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return Ok(());
    }
    let preview = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_lowercase();
    if preview.contains("too many downloads") || preview.contains("rate limit") {
        return Err(
            "The RAW site is limiting downloads from your IP. Wait a few minutes before retrying."
                .into(),
        );
    }
    if content_type.is_some_and(|value| value.to_ascii_lowercase().contains("html"))
        || preview.contains("<html")
        || preview.contains("<!doctype html")
    {
        return Err("The RAW site returned a web page instead of a ZIP. Open the novel's download link in your browser or retry later.".into());
    }
    Err(
        "The RAW site did not return a ZIP archive. Retry later or download it in your browser."
            .into(),
    )
}

async fn download_raw_zip(
    client: &Client,
    download_url: &str,
    novel_url: &str,
) -> Result<Vec<u8>, String> {
    let url = raw_url(download_url, "download")?;
    let referer = raw_url(novel_url, "novel")?;
    let response = client
        .get(url)
        .header(reqwest::header::REFERER, referer.as_str())
        .header(
            reqwest::header::ACCEPT,
            "application/zip,application/octet-stream,*/*",
        )
        .send()
        .await
        .map_err(|e| format!("RAW download failed: {e}"))?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(
            "The RAW site is limiting downloads from your IP. Wait a few minutes before retrying."
                .into(),
        );
    }
    let response = response
        .error_for_status()
        .map_err(|e| format!("RAW download failed: {e}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RAW_BYTES as u64)
    {
        return Err("RAW ZIP is too large.".into());
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("Reading RAW download: {e}"))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RAW_BYTES {
            return Err("RAW ZIP is too large.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    validate_raw_zip_response(&bytes, content_type.as_deref())?;
    Ok(bytes)
}

fn decrypted_epub(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    validate_raw_zip_response(&bytes, None)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|e| {
        format!(
            "The RAW download is not a complete ZIP archive ({e}). Try downloading again later."
        )
    })?;
    if archive.len() > 1000 {
        return Err("RAW ZIP contains too many files.".into());
    }
    let mut epubs = Vec::new();
    for index in 0..archive.len() {
        let file = archive.by_index_raw(index).map_err(|e| e.to_string())?;
        if file.enclosed_name().is_none() {
            return Err("RAW ZIP contains an unsafe filename.".into());
        }
        if !file.is_dir() && file.name().to_ascii_lowercase().ends_with(".epub") {
            if file
                .enclosed_name()
                .is_some_and(|path| path.components().count() != 1)
            {
                return Err("RAW EPUB must be at the ZIP root.".into());
            }
            if file.size() > MAX_RAW_BYTES as u64 {
                return Err("RAW EPUB is too large.".into());
            }
            epubs.push(index);
        }
    }
    if epubs.len() != 1 {
        return Err("Expected exactly one EPUB in the RAW ZIP.".into());
    }
    // Some archives use traditional ZIP encryption; AES is enabled for newer archives.
    let entry = archive
        .by_index_decrypt(epubs[0], RAW_PASSWORD)
        .map_err(|e| {
            format!("Cannot decrypt RAW EPUB (password or encryption unsupported): {e}")
        })?;
    let mut epub = Vec::new();
    entry
        .take(MAX_RAW_BYTES as u64 + 1)
        .read_to_end(&mut epub)
        .map_err(|e| format!("Reading decrypted EPUB: {e}"))?;
    if epub.len() > MAX_RAW_BYTES {
        return Err("RAW EPUB is too large.".into());
    }
    let mut inner = ZipArchive::new(Cursor::new(&epub))
        .map_err(|e| format!("Extracted file is not an EPUB: {e}"))?;
    let mut mime = String::new();
    inner
        .by_name("mimetype")
        .map_err(|_| "Extracted EPUB has no mimetype.".to_string())?
        .take(64)
        .read_to_string(&mut mime)
        .map_err(|e| e.to_string())?;
    if mime.trim() != "application/epub+zip" {
        return Err("Extracted EPUB has invalid mimetype.".into());
    }
    Ok(epub)
}
fn safe_epub_name(title: &str) -> String {
    let name = title
        .chars()
        .map(|ch| {
            if ch.is_control() || "<>:\"/\\|?*".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>()
        .trim_matches(|c: char| c == '.' || c.is_whitespace())
        .chars()
        .take(120)
        .collect::<String>();
    format!("{}.epub", if name.is_empty() { "RAW Novel" } else { &name })
}

#[tauri::command]
async fn import_raw_novel(
    app: AppHandle,
    novel_url: String,
    optional_output_path: Option<String>,
    local_zip_path: Option<String>,
) -> Result<LibraryBook, String> {
    let client = raw_client()?;
    let details = get_raw_details(&client, &novel_url).await?;
    if details.download_url.is_empty() {
        return Err("No RAW download is available for this novel.".into());
    }
    if list_library(app.clone())?
        .iter()
        .any(|book| book.raw_source_url.as_deref() == Some(&details.url))
    {
        return Err("This RAW novel is already in your library.".into());
    }
    let zip = if let Some(path) = local_zip_path {
        let path = Path::new(&path);
        if !path.is_file()
            || path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_none_or(|ext| !ext.eq_ignore_ascii_case("zip"))
        {
            return Err("Choose a downloaded RAW ZIP file.".into());
        }
        if fs::metadata(path).map_err(|e| e.to_string())?.len() > MAX_RAW_BYTES as u64 {
            return Err("RAW ZIP is too large.".into());
        }
        fs::read(path).map_err(|e| format!("Reading downloaded RAW ZIP: {e}"))?
    } else {
        download_raw_zip(&client, &details.download_url, &details.url).await?
    };
    let epub = decrypted_epub(zip)?;
    let cover = if !details.cover_url.is_empty() {
        let url = raw_url(&details.cover_url, "cover")?;
        raw_get(&client, url, 20 * 1024 * 1024).await.ok()
    } else {
        None
    };
    let name = safe_epub_name(&details.title);
    let raw_tags = details.tags.clone();
    if let Some(output) = &optional_output_path {
        let path = Path::new(output);
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("epub"))
        {
            return Err("Choose an EPUB destination.".into());
        }
        if path.exists() {
            return Err("The EPUB destination already exists; choose another filename.".into());
        }
    }
    let temp = library_dir(&app)
        .map_err(|e| e.to_string())?
        .join(format!("{}.epub", Uuid::new_v4()));
    fs::write(&temp, &epub).map_err(|e| e.to_string())?;
    let imported = import_library_book(
        app.clone(),
        vec![temp.to_string_lossy().to_string()],
        "Auto-detect".into(),
    );
    let _ = fs::remove_file(&temp);
    let mut book = imported?;
    let result: Result<(), String> = (|| {
        let dir = library_dir(&app).map_err(|e| e.to_string())?.join(&book.id);
        let target = dir.join(format!("000_{name}"));
        fs::rename(&book.files[0], &target).map_err(|e| e.to_string())?;
        book.files[0] = target.to_string_lossy().to_string();
        book.title = details.title;
        book.original_title = details.original_title;
        book.publication_status = details.publication_status;
        book.tags = normalize_labels(raw_tags)?;
        book.genres = normalize_labels(details.genres)?;
        book.raw_tags_en = normalize_labels(details.raw_tags_en)?;
        book.raw_tags_ko = normalize_labels(details.raw_tags_ko)?;
        book.author = details.author;
        book.synopsis = details.synopsis;
        book.raw_source_url = Some(details.url);
        if let Some(bytes) = cover {
            let extension = if details.cover_url.to_lowercase().ends_with(".png") {
                "png"
            } else if details.cover_url.to_lowercase().ends_with(".webp") {
                "webp"
            } else {
                "jpg"
            };
            fs::write(dir.join(format!("cover.{extension}")), bytes).map_err(|e| e.to_string())?;
            book.cover_image = Some(format!("cover.{extension}"));
        }
        save_library_book(&app, &book).map_err(|e| e.to_string())?;
        if let Some(output) = optional_output_path {
            fs::write(output, epub).map_err(|e| format!("Saving EPUB copy: {e}"))?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = delete_library_book(app, book.id);
        return Err(error);
    }
    Ok(book)
}

#[tauri::command]
fn list_library_illustrations(
    app: AppHandle,
    book_id: String,
) -> Result<Vec<LibraryIllustration>, String> {
    let book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    let job = book
        .job_id
        .as_deref()
        .and_then(|id| load_job(&app, id).ok());
    let cache_dir = library_dir(&app)
        .map_err(|error| error.to_string())?
        .join(&book.id)
        .join("illustrations");
    let mut illustrations = Vec::new();
    let mut section_offset = 0;
    for source in &book.files {
        let sections = read_source(Path::new(source), &cache_dir)
            .map_err(|error| format!("Extracting illustrations: {error}"))?;
        let section_count = sections.len();
        for (section_index, section) in sections.into_iter().enumerate() {
            for image in section.images {
                let path = cache_dir.join(&image.file_name);
                let source_title = section.title.clone();
                illustrations.push(LibraryIllustration {
                    path: path.to_string_lossy().to_string(),
                    section_title: job
                        .as_ref()
                        .and_then(|job| job.sections.get(section_offset + section_index))
                        .map(|section| section.title.clone())
                        .unwrap_or(source_title),
                    alt: image.alt,
                });
            }
        }
        section_offset += section_count;
    }
    Ok(illustrations)
}

#[tauri::command]
fn set_library_cover_image(
    app: AppHandle,
    book_id: String,
    source_path: String,
) -> Result<LibraryBook, String> {
    let mut book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    let source = Path::new(&source_path);
    if !source.is_file() {
        return Err("Choose an existing image file for the cover.".into());
    }
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
        return Err("Cover images must be PNG, JPEG, GIF, or WebP files.".into());
    }
    let metadata = fs::metadata(source).map_err(|error| format!("Reading cover image: {error}"))?;
    if metadata.len() > 20 * 1024 * 1024 {
        return Err("Cover images must be 20 MB or smaller.".into());
    }
    let book_dir = library_dir(&app)
        .map_err(|error| error.to_string())?
        .join(&book.id);
    let file_name = format!("cover.{extension}");
    let destination = book_dir.join(&file_name);
    let same_file = source
        .canonicalize()
        .ok()
        .zip(destination.canonicalize().ok())
        .is_some_and(|(source, destination)| source == destination);
    if !same_file {
        fs::copy(source, &destination).map_err(|error| format!("Copying cover image: {error}"))?;
    }
    if let Some(previous) = &book.cover_image {
        if previous != &file_name {
            let previous_path = book_dir.join(previous);
            if previous_path.exists() {
                fs::remove_file(previous_path)
                    .map_err(|error| format!("Replacing previous cover image: {error}"))?;
            }
        }
    }
    book.cover_image = Some(file_name);
    save_library_book(&app, &book).map_err(|error| error.to_string())?;
    Ok(book)
}

#[tauri::command]
fn remove_library_cover_image(app: AppHandle, book_id: String) -> Result<LibraryBook, String> {
    let mut book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    if let Some(file_name) = book.cover_image.take() {
        let book_dir = library_dir(&app)
            .map_err(|error| error.to_string())?
            .join(&book.id);
        let relative_path = Path::new(&file_name);
        if relative_path.file_name().and_then(|name| name.to_str()) != Some(file_name.as_str()) {
            return Err("Saved cover path is invalid.".into());
        }
        let path = book_dir.join(relative_path);
        if path.exists() {
            fs::remove_file(path).map_err(|error| format!("Removing cover image: {error}"))?;
        }
    }
    save_library_book(&app, &book).map_err(|error| error.to_string())?;
    Ok(book)
}

fn parse_glossary_suggestions(response: &str) -> Result<Vec<GlossarySuggestion>> {
    let start = response
        .find('[')
        .ok_or_else(|| anyhow!("model response did not contain a JSON array"))?;
    let end = response
        .rfind(']')
        .ok_or_else(|| anyhow!("model response contained an incomplete JSON array"))?;
    if end < start {
        return Err(anyhow!("model response contained an invalid JSON array"));
    }

    serde_json::from_str(&response[start..=end]).context("parsing generated glossary terms")
}

fn parse_context_suggestions(response: &str) -> Result<Vec<ContextSuggestion>> {
    let start = response
        .find('[')
        .ok_or_else(|| anyhow!("model response did not contain a JSON array"))?;
    let end = response
        .rfind(']')
        .ok_or_else(|| anyhow!("model response contained an incomplete JSON array"))?;
    if end < start {
        return Err(anyhow!("model response contained an invalid JSON array"));
    }
    serde_json::from_str(&response[start..=end]).context("parsing story context facts")
}

#[cfg(test)]
fn merge_story_context(existing: &mut Vec<String>, suggestions: Vec<ContextSuggestion>) -> usize {
    let mut known = existing
        .iter()
        .map(|fact| {
            fact.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .collect::<std::collections::HashSet<_>>();
    let before = existing.len();
    for suggestion in suggestions {
        let fact = suggestion
            .fact
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if fact.is_empty() || fact.chars().count() > 500 || existing.len() >= MAX_CONTEXT_FACTS {
            continue;
        }

        let key = fact.to_lowercase();
        if known.insert(key) {
            existing.push(fact);
        }
    }
    existing.len() - before
}

fn context_lines(facts: &[String]) -> String {
    let mut output = String::new();
    for (index, fact) in facts.iter().take(MAX_CONTEXT_FACTS).enumerate() {
        let line = format!("{}. {}\n", index + 1, fact);
        if output.chars().count() + line.chars().count() > MAX_CONTEXT_REVIEW_CHARS {
            break;
        }
        output.push_str(&line);
    }
    output.trim_end().to_string()
}

fn bound_context_text(context: &str, limit: usize) -> String {
    if context.chars().count() <= limit {
        return context.to_string();
    }
    let mut bounded = context.chars().take(limit).collect::<String>();
    if let Some(index) = bounded.rfind('\n') {
        bounded.truncate(index);
    }
    format!(
        "{bounded}\n[Additional lower-priority context omitted to preserve translation capacity.]"
    )
}

async fn collect_glossary_suggestions(
    app: &AppHandle,
    job: &Job,
) -> Result<(Vec<GlossaryEntry>, usize), String> {
    if job.completed_sections != job.section_count
        || job
            .sections
            .iter()
            .any(|section| section.translated.is_none())
    {
        return Err("Finish translating this novel before generating its glossary.".into());
    }
    let settings = load_settings(app).map_err(|error| error.to_string())?;
    let model = if job.model.trim().is_empty() {
        settings.default_model
    } else {
        job.model.clone()
    };
    if model.trim().is_empty() {
        return Err(
            "Set a default Ollama model in Settings before generating glossary terms.".into(),
        );
    }
    let endpoint = settings.ollama_endpoint;
    let client = Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|error| error.to_string())?;
    let mut suggestions = std::collections::BTreeMap::new();
    let mut skipped_batches = 0;
    for (section_index, section) in job.sections.iter().enumerate() {
        let source_chunks = split_translation_chunks(&section.text, 3000);
        if source_chunks.len() != section.translated_chunks.len() {
            return Err(format!(
                "Cannot pair source and translation chunks in section {}.",
                section_index + 1
            ));
        }
        for (batch_index, source_batch) in source_chunks.chunks(1).enumerate() {
            let first = batch_index;
            let examples = source_batch
                .iter()
                .enumerate()
                .map(|(offset, source)| {
                    format!(
                        "SOURCE:\n{}\nENGLISH TRANSLATION:\n{}",
                        source,
                        section.translated_chunks[first + offset]
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n---\n\n");
            let request = ChatRequest {
                model: &model,
                messages: vec![
                    ChatMessage {
                        role: "system",
                        content: format!(
                            "You extract a conservative glossary from parallel {} and English literary text. Mature or R19 content may appear in the input. Do not describe, quote, summarize, or analyze sexual content; ignore those scenes and extract only names, places, titles, forms of address, and story-specific terms needed for translation continuity. Return only a valid JSON array, with objects shaped like {{\"source_term\":\"...\",\"target_term\":\"...\",\"note\":\"...\",\"character_gender\":\"\"}}. Include every recurring named character that appears in non-explicit paired text, as well as important places, titles, forms of address, and story-specific terms, when the English rendering is clear. For each clearly identified character, set character_gender to exactly one concise value such as male, female, nonbinary, or unknown. This gender field is important for resolving Korean and Japanese pronouns and forms of address. Do not guess a gender from a name alone; use explicit source text, pronouns, titles, or the established English translation. Leave character_gender empty for non-characters. Do not include ordinary vocabulary or invent terms. Keep source_term exactly in {} script and target_term as it appears in English.",
                            job.source_language, job.source_language
                        ),
                    },
                    ChatMessage {
                        role: "user",
                        content: examples,
                    },
                ],
                stream: false,
                think: false,
                options: ChatOptions { temperature: 0.1 },
            };
            let response = client
                .post(endpoint_url(&endpoint, "api/chat").map_err(|error| error.to_string())?)
                .json(&request)
                .send()
                .await
                .map_err(|error| format!("Ollama glossary request failed: {error}"))?;
            if !response.status().is_success() {
                let status = response.status();
                let detail = response.text().await.unwrap_or_else(|error| {
                    format!("unable to read Ollama error response: {error}")
                });
                return Err(format!(
                    "Ollama returned HTTP {status} while generating glossary terms: {}",
                    detail.trim()
                ));
            }
            let output: ChatResponse = response
                .json()
                .await
                .map_err(|error| format!("Reading generated glossary response: {error}"))?;
            let batch_suggestions = match parse_glossary_suggestions(&output.message.content) {
                Ok(suggestions) => suggestions,
                Err(_) => {
                    skipped_batches += 1;
                    continue;
                }
            };
            for suggestion in batch_suggestions {
                let source_term = suggestion.source_term.trim();
                let target_term = suggestion.target_term.trim();
                if source_term.is_empty() || target_term.is_empty() {
                    continue;
                }
                suggestions
                    .entry(source_term.to_lowercase())
                    .or_insert_with(|| GlossaryEntry {
                        id: Uuid::new_v4().to_string(),
                        source_language: job.source_language.clone(),
                        source_term: source_term.to_string(),
                        target_term: target_term.to_string(),
                        note: suggestion.note.trim().to_string(),
                        character_gender: suggestion.character_gender.trim().to_string(),
                    });
            }
        }
    }
    let suggestions = suggestions.into_values().collect::<Vec<_>>();
    if suggestions.is_empty() && skipped_batches > 0 {
        return Err(
            "The model refused or returned invalid glossary data for every batch. R19 content may have triggered refusals; try a less restrictive local model or add terms manually.".into(),
        );
    }
    Ok((suggestions, skipped_batches))
}

#[tauri::command]
async fn generate_job_glossary_suggestions(
    app: AppHandle,
    job_id: String,
) -> Result<Vec<GlossaryEntry>, String> {
    let job = load_job(&app, &job_id).map_err(|error| error.to_string())?;
    Ok(collect_glossary_suggestions(&app, &job).await?.0)
}

#[tauri::command]
async fn generate_library_glossary(
    app: AppHandle,
    book_id: String,
) -> Result<GlossaryGenerationResult, String> {
    let mut book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    let job_id = book
        .job_id
        .as_deref()
        .ok_or_else(|| "Translate this novel before generating a glossary.".to_string())?;
    let job = load_job(&app, job_id).map_err(|error| error.to_string())?;
    let (suggestions, skipped_batches) = collect_glossary_suggestions(&app, &job).await?;
    let before = book.glossary.len();
    for suggestion in suggestions {
        if !book.glossary.iter().any(|existing| {
            existing
                .source_term
                .eq_ignore_ascii_case(&suggestion.source_term)
        }) {
            book.glossary.push(suggestion);
        }
    }
    let added = book.glossary.len() - before;
    save_library_book(&app, &book).map_err(|error| error.to_string())?;
    Ok(GlossaryGenerationResult {
        glossary: book.glossary,
        added,
        skipped_batches,
    })
}

#[tauri::command]
fn save_library_glossary_text(
    app: AppHandle,
    book_id: String,
    text: String,
) -> Result<LibraryBook, String> {
    let mut book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    book.glossary = parse_library_glossary_text(&text, &book.source_language, &book.glossary)?;
    save_library_book(&app, &book).map_err(|error| error.to_string())?;
    Ok(book)
}

fn parse_library_glossary_text(
    text: &str,
    source_language: &str,
    existing: &[GlossaryEntry],
) -> Result<Vec<GlossaryEntry>, String> {
    let mut entries = Vec::new();
    for (line_number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split('\t').map(str::trim).collect::<Vec<_>>();
        if !(2..=4).contains(&fields.len()) || fields[0].is_empty() || fields[1].is_empty() {
            return Err(format!(
                "Line {} must contain source term, English term, optional note, and optional character gender separated by tabs.",
                line_number + 1
            ));
        }
        if fields[0].chars().count() > 160
            || fields[1].chars().count() > 160
            || fields.get(2).is_some_and(|note| note.chars().count() > 240)
            || fields
                .get(3)
                .is_some_and(|gender| gender.chars().count() > 80)
        {
            return Err(format!(
                "Glossary entry on line {} is too long.",
                line_number + 1
            ));
        }
        if entries
            .iter()
            .any(|entry: &GlossaryEntry| entry.source_term.eq_ignore_ascii_case(fields[0]))
        {
            return Err(format!(
                "Source term on line {} is duplicated.",
                line_number + 1
            ));
        }
        let id = existing
            .iter()
            .find(|entry| entry.source_term.eq_ignore_ascii_case(fields[0]))
            .map(|entry| entry.id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        entries.push(GlossaryEntry {
            id,
            source_language: source_language.to_string(),
            source_term: fields[0].to_string(),
            target_term: fields[1].to_string(),
            note: fields.get(2).copied().unwrap_or_default().to_string(),
            character_gender: fields.get(3).copied().unwrap_or_default().to_string(),
        });
    }
    Ok(entries)
}

#[tauri::command]
fn update_library_source_language(
    app: AppHandle,
    book_id: String,
    source_language: String,
) -> Result<LibraryBook, String> {
    if !matches!(source_language.as_str(), "Japanese" | "Korean") {
        return Err("Source language must be Japanese or Korean.".into());
    }
    let mut book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    if book.source_language == source_language {
        return Ok(book);
    }
    book.source_language = source_language.clone();
    for entry in &mut book.glossary {
        entry.source_language = source_language.clone();
    }
    if let Some(job_id) = &book.job_id {
        let mut job = load_job(&app, job_id).map_err(|error| error.to_string())?;
        if job.queue_status.as_deref() == Some("Translating") {
            return Err(
                "Wait for the active translation to finish before changing its source language."
                    .into(),
            );
        }
        job.source_language = source_language.clone();
        job.glossary = load_glossary(&app)
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|entry| entry.source_language == source_language)
            .collect();
        job.glossary = merge_glossary(job.glossary, &book.glossary);
        for section in &mut job.sections {
            section.translated = None;
            section.translated_chunks.clear();
            section.context_updated = false;
        }
        job.completed_sections = 0;
        job.completed_chunks = 0;
        job.total_chunks = job_chunk_counts(&job).1;
        job.elapsed_translation_millis = 0;
        job.context_update_failures.clear();
        if job.queue_status.as_deref() == Some("Completed") {
            job.queue_status = None;
            book.status = "Not translated".into();
            let mut queue = load_queue(&app).map_err(|error| error.to_string())?;
            queue.retain(|queued_id| queued_id != job_id);
            save_queue(&app, &queue).map_err(|error| error.to_string())?;
        }
        save_job(&app, &job).map_err(|error| error.to_string())?;
    }
    save_library_book(&app, &book).map_err(|error| error.to_string())?;
    Ok(book)
}

fn remove_job_storage(app: &AppHandle, job_id: &str) -> Result<(), String> {
    let mut queue = load_queue(app).map_err(|error| error.to_string())?;
    queue.retain(|queued_id| queued_id != job_id);
    save_queue(app, &queue).map_err(|error| error.to_string())?;
    let job_file = job_path(app, job_id).map_err(|error| error.to_string())?;
    if job_file.exists() {
        fs::remove_file(job_file)
            .map_err(|error| format!("Deleting saved translation: {error}"))?;
    }
    let overrides_file = job_overrides_path(app, job_id).map_err(|error| error.to_string())?;
    if overrides_file.exists() {
        fs::remove_file(overrides_file)
            .map_err(|error| format!("Deleting queued job edits: {error}"))?;
    }
    let assets = jobs_dir(app).map_err(|error| error.to_string())?.join(
        Uuid::parse_str(job_id)
            .map_err(|error| error.to_string())?
            .to_string(),
    );
    if assets.exists() {
        fs::remove_dir_all(assets)
            .map_err(|error| format!("Deleting saved translation assets: {error}"))?;
    }
    Ok(())
}

#[tauri::command]
fn delete_saved_job(app: AppHandle, job_id: String) -> Result<(), String> {
    let job = load_job(&app, &job_id).map_err(|error| error.to_string())?;
    if job.queue_status.as_deref() == Some("Translating") {
        return Err("Wait for the active translation to finish before deleting it.".into());
    }
    if let Some(book_id) = job.library_id {
        let mut book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
        if book.job_id.as_deref() == Some(&job_id) {
            book.job_id = None;
            book.status = "Not translated".into();
            save_library_book(&app, &book).map_err(|error| error.to_string())?;
        }
    }
    remove_job_storage(&app, &job_id)
}

#[tauri::command]
fn delete_library_book(app: AppHandle, book_id: String) -> Result<(), String> {
    let book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    if let Some(job_id) = &book.job_id {
        let job_file = job_path(&app, job_id).map_err(|error| error.to_string())?;
        if job_file.exists() {
            let job = load_job(&app, job_id).map_err(|error| error.to_string())?;
            if job.queue_status.as_deref() == Some("Translating") {
                return Err(
                    "Wait for the active translation to finish before removing this novel.".into(),
                );
            }
            remove_job_storage(&app, job_id)?;
        }
    }
    let metadata = library_book_path(&app, &book_id).map_err(|error| error.to_string())?;
    if metadata.exists() {
        fs::remove_file(metadata)
            .map_err(|error| format!("Removing novel from library: {error}"))?;
    }
    let files = library_dir(&app).map_err(|error| error.to_string())?.join(
        Uuid::parse_str(&book_id)
            .map_err(|error| error.to_string())?
            .to_string(),
    );
    if files.exists() {
        fs::remove_dir_all(files)
            .map_err(|error| format!("Removing copied novel files: {error}"))?;
    }
    Ok(())
}

#[tauri::command]
fn save_library_glossary_entry(
    app: AppHandle,
    book_id: String,
    id: Option<String>,
    source_term: String,
    target_term: String,
    note: String,
    character_gender: String,
) -> Result<LibraryBook, String> {
    let mut book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    let source_term = source_term.trim().to_string();
    let target_term = target_term.trim().to_string();
    if source_term.is_empty() || target_term.is_empty() {
        return Err("Both source and English terms are required.".into());
    }
    if book.glossary.iter().any(|entry| {
        entry.source_term.eq_ignore_ascii_case(&source_term)
            && Some(entry.id.as_str()) != id.as_deref()
    }) {
        return Err("This source term is already in this novel's glossary.".into());
    }
    let entry = GlossaryEntry {
        id: id.unwrap_or_else(|| Uuid::new_v4().to_string()),
        source_language: book.source_language.clone(),
        source_term,
        target_term,
        note: note.trim().to_string(),
        character_gender: character_gender.trim().to_string(),
    };
    if let Some(index) = book
        .glossary
        .iter()
        .position(|existing| existing.id == entry.id)
    {
        book.glossary[index] = entry;
    } else {
        book.glossary.push(entry);
    }
    save_library_book(&app, &book).map_err(|error| error.to_string())?;
    Ok(book)
}

#[tauri::command]
fn delete_library_glossary_entry(
    app: AppHandle,
    book_id: String,
    entry_id: String,
) -> Result<LibraryBook, String> {
    let mut book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    book.glossary.retain(|entry| entry.id != entry_id);
    save_library_book(&app, &book).map_err(|error| error.to_string())?;
    Ok(book)
}

fn merge_glossary(
    mut global: Vec<GlossaryEntry>,
    per_book: &[GlossaryEntry],
) -> Vec<GlossaryEntry> {
    for local in per_book {
        global.retain(|entry| {
            entry.source_language != local.source_language
                || !entry.source_term.eq_ignore_ascii_case(&local.source_term)
        });
        global.push(local.clone());
    }
    global
}

#[tauri::command]
fn enqueue_library_book(
    app: AppHandle,
    book_id: String,
    genre: String,
    tones: Vec<String>,
    instructions: String,
    model: String,
    output_format: String,
) -> Result<Job, String> {
    let mut book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    let settings = load_settings(&app).map_err(|error| error.to_string())?;
    let mut queue = load_queue(&app).map_err(|error| error.to_string())?;
    let output_format = if matches!(output_format.as_str(), "epub" | "txt") {
        output_format
    } else {
        settings.default_output_format.clone()
    };
    book.output_format = output_format.clone();
    let model = if model.trim().is_empty() {
        settings.default_model.clone()
    } else {
        model
    };
    if let Some(existing_id) = &book.job_id {
        if let Ok(job) = load_job(&app, existing_id) {
            if matches!(job.queue_status.as_deref(), Some("Queued" | "Translating")) {
                return Ok(job);
            }
            if job
                .sections
                .iter()
                .any(|section| section.translated.is_none())
            {
                let mut resumed = load_job(&app, existing_id).map_err(|e| e.to_string())?;
                // A new chapter must not invalidate completed chapters when Translate-tab settings change.
                resumed.output_format = output_format.clone();
                if !model.is_empty() {
                    resumed.model = model.clone();
                }
                resumed.queue_status = Some("Queued".into());
                resumed.queue_order = queue.len();
                save_job(&app, &resumed).map_err(|e| e.to_string())?;
                if !queue.contains(existing_id) {
                    queue.push(existing_id.clone());
                }
                save_queue(&app, &queue).map_err(|e| e.to_string())?;
                book.status = "Queued".into();
                save_library_book(&app, &book).map_err(|e| e.to_string())?;
                return Ok(resumed);
            }
            if job.queue_status.as_deref() == Some("Failed") {
                update_job_settings(
                    app.clone(),
                    existing_id.clone(),
                    book.source_language.clone(),
                    output_format,
                    genre,
                    tones,
                    instructions,
                    model,
                )?;
                let job = update_queue_status(app.clone(), existing_id.clone(), "Queued".into())?;
                book.status = "Queued".into();
                save_library_book(&app, &book).map_err(|error| error.to_string())?;
                return Ok(job);
            }
        }
    }
    let mut job = create_job_from_paths(
        &app,
        book.files.clone(),
        book.source_language.clone(),
        output_format,
        genre,
        tones,
        instructions,
        model,
        Some(book.id.clone()),
        Some("Queued".into()),
        queue.len(),
    )?;
    job.glossary = merge_glossary(job.glossary, &book.glossary);
    job.context = book.context.clone();
    job.title = book.title.clone();
    save_job(&app, &job).map_err(|error| error.to_string())?;
    queue.push(job.id.clone());
    save_queue(&app, &queue).map_err(|error| error.to_string())?;
    book.status = "Queued".into();
    book.job_id = Some(job.id.clone());
    save_library_book(&app, &book).map_err(|error| error.to_string())?;
    Ok(job)
}

#[tauri::command]
fn list_queue(app: AppHandle) -> Result<Vec<Job>, String> {
    let ids = load_queue(&app).map_err(|error| error.to_string())?;
    let mut jobs = Vec::new();
    let mut active_ids = Vec::new();
    for id in ids {
        let path = job_path(&app, &id).map_err(|error| error.to_string())?;
        if path.exists() {
            let job = load_job(&app, &id).map_err(|error| error.to_string())?;
            if job.queue_status.as_deref() != Some("Completed") {
                active_ids.push(id);
                jobs.push(job);
            }
        }
    }
    save_queue(&app, &active_ids).map_err(|error| error.to_string())?;
    jobs.sort_by_key(|job| job.queue_order);
    Ok(jobs)
}

#[tauri::command]
fn update_queue_status(app: AppHandle, job_id: String, status: String) -> Result<Job, String> {
    if !matches!(
        status.as_str(),
        "Queued" | "Translating" | "Failed" | "Completed"
    ) {
        return Err("Invalid queue status.".into());
    }

    let mut job = load_job(&app, &job_id).map_err(|error| error.to_string())?;
    job.queue_status = Some(status.clone());
    if status != "Translating" {
        job.current_section_index = None;
        let mut overrides = load_job_overrides(&app, &job_id).map_err(|error| error.to_string())?;
        overrides.current_section_index = None;
        save_job_overrides(&app, &job_id, &overrides).map_err(|error| error.to_string())?;
    }
    save_job(&app, &job).map_err(|error| error.to_string())?;
    if status == "Completed" {
        let mut queue = load_queue(&app).map_err(|error| error.to_string())?;
        queue.retain(|queued_id| queued_id != &job_id);
        save_queue(&app, &queue).map_err(|error| error.to_string())?;
    }
    if let Some(book_id) = &job.library_id {
        let mut book = load_library_book(&app, book_id).map_err(|error| error.to_string())?;
        book.status = status;
        save_library_book(&app, &book).map_err(|error| error.to_string())?;
    }
    Ok(job)
}

#[tauri::command]
fn restart_library_translation(app: AppHandle, book_id: String) -> Result<Job, String> {
    let book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    let job_id = book
        .job_id
        .as_deref()
        .ok_or_else(|| "This novel has not been queued yet.".to_string())?;
    let mut job = load_job(&app, job_id).map_err(|error| error.to_string())?;
    for section in &mut job.sections {
        section.translated = None;
        section.translated_chunks.clear();
        section.context_updated = false;
    }
    job.completed_sections = 0;
    job.completed_chunks = 0;
    job.total_chunks = job_chunk_counts(&job).1;
    job.elapsed_translation_millis = 0;
    job.current_section_index = None;
    job.context_update_failures.clear();
    job.queue_status = Some("Queued".into());
    save_job(&app, &job).map_err(|error| error.to_string())?;
    let mut queue = load_queue(&app).map_err(|error| error.to_string())?;
    queue.retain(|id| id != job_id);
    queue.insert(0, job_id.to_string());
    save_queue(&app, &queue).map_err(|error| error.to_string())?;
    let mut book = book;
    book.status = "Queued".into();
    save_library_book(&app, &book).map_err(|error| error.to_string())?;
    Ok(job)
}

#[tauri::command]
fn list_glossary(app: AppHandle) -> Result<Vec<GlossaryEntry>, String> {
    load_glossary(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn save_glossary_entry(
    app: AppHandle,
    id: Option<String>,
    source_language: String,
    source_term: String,
    target_term: String,
    note: String,
    character_gender: String,
) -> Result<Vec<GlossaryEntry>, String> {
    if !matches!(source_language.as_str(), "Japanese" | "Korean") {
        return Err("Glossary source language must be Japanese or Korean.".into());
    }
    let source_term = source_term.trim().to_string();
    let target_term = target_term.trim().to_string();
    if source_term.is_empty() || target_term.is_empty() {
        return Err("Both source and English terms are required.".into());
    }
    let mut entries = load_glossary(&app).map_err(|error| error.to_string())?;
    let entry = GlossaryEntry {
        id: id.unwrap_or_else(|| Uuid::new_v4().to_string()),
        source_language,
        source_term,
        target_term,
        note: note.trim().to_string(),
        character_gender: character_gender.trim().to_string(),
    };
    if let Some(index) = entries.iter().position(|existing| existing.id == entry.id) {
        entries[index] = entry;
    } else {
        entries.push(entry);
    }
    save_glossary(&app, &entries).map_err(|error| error.to_string())?;
    Ok(entries)
}

#[tauri::command]
fn delete_glossary_entry(app: AppHandle, id: String) -> Result<Vec<GlossaryEntry>, String> {
    let mut entries = load_glossary(&app).map_err(|error| error.to_string())?;
    entries.retain(|entry| entry.id != id);
    save_glossary(&app, &entries).map_err(|error| error.to_string())?;
    Ok(entries)
}

#[tauri::command]
fn list_jobs(app: AppHandle) -> Result<Vec<JobSummary>, String> {
    let directory = jobs_dir(&app).map_err(|error| error.to_string())?;
    let mut jobs = Vec::new();
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .file_name()
            .to_string_lossy()
            .ends_with(".overrides.json")
        {
            continue;
        }
        if entry.path().extension().and_then(|value| value.to_str()) == Some("json") {
            let bytes = fs::read(entry.path()).map_err(|error| error.to_string())?;
            let job: Job = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            jobs.push(JobSummary {
                id: job.id,
                title: job.title,
                completed_sections: job.completed_sections,
                section_count: job.section_count,
            });
        }
    }
    jobs.sort_by(|left, right| left.title.to_lowercase().cmp(&right.title.to_lowercase()));
    Ok(jobs)
}

#[derive(Serialize)]
struct JobSummary {
    id: String,
    title: String,
    completed_sections: usize,
    section_count: usize,
}

#[tauri::command]
fn get_job(app: AppHandle, job_id: String) -> Result<Job, String> {
    load_job(&app, &job_id).map_err(|error| error.to_string())
}

#[tauri::command]
async fn set_queue_model(app: AppHandle, job_id: String, model: String) -> Result<Job, String> {
    let mut job = load_job(&app, &job_id).map_err(|error| error.to_string())?;
    if job.library_id.is_none()
        || !matches!(
            job.queue_status.as_deref(),
            Some("Queued" | "Translating" | "Failed")
        )
    {
        return Err("Only library queue jobs can change models here.".into());
    }
    let model = model.trim();
    if model.is_empty() || model.chars().count() > 200 {
        return Err("Select a valid model.".into());
    }
    let settings = load_settings(&app).map_err(|error| error.to_string())?;
    let endpoint =
        endpoint_url(&settings.ollama_endpoint, "api/tags").map_err(|error| error.to_string())?;
    let client = Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|error| error.to_string())?;
    let models: OllamaTags = client
        .get(endpoint)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    if !models
        .models
        .iter()
        .any(|available| available.name == model)
    {
        return Err("The selected model is not installed on the configured Ollama server.".into());
    }
    if job.queue_status.as_deref() == Some("Translating") {
        let mut overrides = load_job_overrides(&app, &job_id).map_err(|error| error.to_string())?;
        overrides.pending_model = Some(model.to_string());
        save_job_overrides(&app, &job_id, &overrides).map_err(|error| error.to_string())?;
    } else {
        job.model = model.to_string();
        save_job(&app, &job).map_err(|error| error.to_string())?;
    }
    Ok(job)
}

#[tauri::command]
fn get_queue_model_override(app: AppHandle, job_id: String) -> Result<Option<String>, String> {
    load_job(&app, &job_id).map_err(|error| error.to_string())?;
    Ok(load_job_overrides(&app, &job_id)
        .map_err(|error| error.to_string())?
        .pending_model)
}

#[tauri::command]
fn update_job_section_title(
    app: AppHandle,
    job_id: String,
    section_index: usize,
    title: String,
) -> Result<Job, String> {
    let mut job = load_job(&app, &job_id).map_err(|error| error.to_string())?;
    let title = title.trim();
    if title.is_empty() {
        return Err("Section title cannot be empty.".into());
    }
    if title.chars().count() > 160 {
        return Err("Section title cannot exceed 160 characters.".into());
    }
    let section = job
        .sections
        .get_mut(section_index)
        .ok_or_else(|| "Section no longer exists.".to_string())?;
    section.title = title.to_string();
    let mut overrides = load_job_overrides(&app, &job_id).map_err(|error| error.to_string())?;
    overrides.titles.insert(section_index, title.to_string());
    save_job_overrides(&app, &job_id, &overrides).map_err(|error| error.to_string())?;
    if job.queue_status.as_deref() != Some("Translating") {
        save_job(&app, &job).map_err(|error| error.to_string())?;
    }
    Ok(job)
}

#[tauri::command]
fn update_job_section_titles(
    app: AppHandle,
    job_id: String,
    titles: Vec<String>,
) -> Result<Job, String> {
    let mut job = load_job(&app, &job_id).map_err(|error| error.to_string())?;
    if titles.len() != job.sections.len() {
        return Err(format!(
            "Expected one title for each of the {} sections.",
            job.sections.len()
        ));
    }
    for (index, (section, title)) in job.sections.iter_mut().zip(titles).enumerate() {
        let title = title.trim();
        if title.is_empty() {
            return Err(format!("Title {} cannot be empty.", index + 1));
        }
        if title.chars().count() > 160 {
            return Err(format!("Title {} cannot exceed 160 characters.", index + 1));
        }
        section.title = title.to_string();
    }
    let mut overrides = load_job_overrides(&app, &job_id).map_err(|error| error.to_string())?;
    overrides.titles = job
        .sections
        .iter()
        .enumerate()
        .map(|(index, section)| (index, section.title.clone()))
        .collect();
    save_job_overrides(&app, &job_id, &overrides).map_err(|error| error.to_string())?;
    if job.queue_status.as_deref() != Some("Translating") {
        save_job(&app, &job).map_err(|error| error.to_string())?;
    }
    Ok(job)
}

#[tauri::command]
fn update_job_section_text(
    app: AppHandle,
    job_id: String,
    section_index: usize,
    text: String,
) -> Result<Job, String> {
    let mut job = load_job(&app, &job_id).map_err(|error| error.to_string())?;
    if matches!(job.queue_status.as_deref(), Some("Queued" | "Translating")) {
        return Err("Wait until the novel is no longer in the translation queue.".into());
    }
    if text.trim().is_empty() {
        return Err("Chapter content cannot be empty.".into());
    }
    let section = job
        .sections
        .get_mut(section_index)
        .ok_or_else(|| "Section no longer exists.".to_string())?;
    section.text = text;
    section.translated = None;
    section.translated_chunks.clear();
    section.context_updated = false;
    job.completed_sections = job
        .sections
        .iter()
        .filter(|section| section.translated.is_some())
        .count();
    job.completed_chunks = job_chunk_counts(&job).0;
    job.total_chunks = job_chunk_counts(&job).1;
    job.queue_status = Some("Failed".into());
    save_job(&app, &job).map_err(|error| error.to_string())?;
    if let Some(book_id) = &job.library_id {
        let mut book = load_library_book(&app, book_id).map_err(|error| error.to_string())?;
        book.status = "Not translated".into();
        save_library_book(&app, &book).map_err(|error| error.to_string())?;
    }
    Ok(job)
}

#[tauri::command]
fn update_job_section_translation(
    app: AppHandle,
    job_id: String,
    section_index: usize,
    translated: String,
) -> Result<Job, String> {
    let mut job = load_job(&app, &job_id).map_err(|error| error.to_string())?;
    if matches!(job.queue_status.as_deref(), Some("Queued" | "Translating")) {
        return Err("Wait until the novel is no longer in the translation queue.".into());
    }
    if translated.trim().is_empty() {
        return Err("Translated chapter content cannot be empty.".into());
    }
    let section = job
        .sections
        .get_mut(section_index)
        .ok_or_else(|| "Section no longer exists.".to_string())?;
    section.translated = Some(translated);
    section.context_updated = false;
    save_job(&app, &job).map_err(|error| error.to_string())?;
    Ok(job)
}

#[tauri::command]
fn update_job_settings(
    app: AppHandle,
    job_id: String,
    source_language: String,
    output_format: String,
    genre: String,
    tones: Vec<String>,
    instructions: String,
    model: String,
) -> Result<Job, String> {
    if !matches!(
        source_language.as_str(),
        "Korean" | "Japanese" | "Auto-detect"
    ) {
        return Err("Source language must be Korean, Japanese, or Auto-detect.".into());
    }
    if !matches!(output_format.as_str(), "epub" | "txt") {
        return Err("Output format must be EPUB or TXT.".into());
    }
    let mut job = load_job(&app, &job_id).map_err(|error| error.to_string())?;
    let settings = load_settings(&app).map_err(|error| error.to_string())?;
    let context_limit = settings.context_limit.clamp(1, MAX_CONTEXT_FACTS);
    let source_language = if source_language == "Auto-detect" {
        detect_source_language(
            &job.sections
                .iter()
                .map(|section| section.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            &settings.default_source_language,
        )
    } else {
        source_language
    };
    let mut glossary = load_glossary(&app)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter(|entry| entry.source_language == source_language)
        .collect::<Vec<_>>();
    if let Some(book_id) = &job.library_id {
        let book = load_library_book(&app, book_id).map_err(|error| error.to_string())?;
        glossary = merge_glossary(glossary, &book.glossary);
    }
    let translation_settings_changed = job.source_language != source_language
        || job.genre != genre
        || job.tones != tones
        || job.instructions != instructions
        || job.model != model
        || job.translation_prompt != settings.translation_prompt
        || job.glossary != glossary;
    if translation_settings_changed {
        for section in &mut job.sections {
            section.translated = None;
            section.translated_chunks.clear();
            section.context_updated = false;
        }
        job.completed_sections = 0;
        job.completed_chunks = 0;
        job.elapsed_translation_millis = 0;
        job.context_update_failures.clear();
        job.translation_prompt = settings.translation_prompt;
        job.glossary = glossary;
    }
    job.source_language = source_language;
    job.output_format = output_format;
    job.genre = genre;
    job.tones = tones;
    job.instructions = instructions;
    job.model = model;
    job.context_limit = context_limit;
    save_job(&app, &job).map_err(|error| error.to_string())?;
    Ok(job)
}

async fn generate_section_context(
    client: &Client,
    endpoint: &str,
    job: &Job,
    section_index: usize,
    window: &tauri::Window,
    max_retries: usize,
) -> Result<Vec<ContextSuggestion>, String> {
    let section = job
        .sections
        .get(section_index)
        .ok_or_else(|| format!("Section {} no longer exists.", section_index + 1))?;
    let source_chunks = split_translation_chunks(&section.text, 3000);
    if source_chunks.len() != section.translated_chunks.len() {
        return Err(format!(
            "Source and translated chunks do not align for section {}.",
            section_index + 1
        ));
    }
    let system = "Extract only non-explicit, durable story facts that may matter in later chapters, especially character relationships, family ties, identities, titles, affiliations, and established worldbuilding. Ignore sexual, erotic, pornographic, or other R19 material completely: do not analyze it, describe it, quote it, or turn it into a fact. If the text contains no relevant non-explicit continuity fact, return []. Do not infer uncertain facts, include plot summaries, or repeat existing facts. Return only a JSON array in the exact shape [{\"fact\":\"...\"}].";
    let existing = if job.context.is_empty() {
        "(none)".to_string()
    } else {
        bound_context_text(&context_lines(&job.context), MAX_CONTEXT_PROMPT_CHARS)
    };
    let mut suggestions = Vec::new();
    for (batch_index, source_batch) in source_chunks.chunks(3).enumerate() {
        let first = batch_index * 3;
        let examples = source_batch
            .iter()
            .enumerate()
            .map(|(offset, source)| {
                format!(
                    "CHUNK {}:\nSOURCE:\n{}\n\nENGLISH TRANSLATION:\n{}",
                    first + offset + 1,
                    source,
                    section.translated_chunks[first + offset]
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");
        let request = ChatRequest {
            model: &job.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: system.into(),
                },
                ChatMessage {
                    role: "user",
                    content: format!(
                        "Chapter: {}\nChunks {}–{} of {}\nExisting story context:\n{}\n\n{}",
                        section.title,
                        first + 1,
                        first + source_batch.len(),
                        source_chunks.len(),
                        existing,
                        examples
                    ),
                },
            ],
            stream: false,
            think: false,
            options: ChatOptions { temperature: 0.1 },
        };
        let request_url = endpoint_url(endpoint, "api/chat").map_err(|error| error.to_string())?;
        let mut result = None;
        let mut last_failure = None;
        for attempt in 0..=max_retries {
            match client.post(request_url.clone()).json(&request).send().await {
                Ok(response) if response.status().is_success() => {
                    match response.json::<ChatResponse>().await {
                        Ok(response) => {
                            match parse_context_suggestions(&response.message.content) {
                                Ok(parsed) => {
                                    result = Some(parsed);
                                    break;
                                }
                                Err(error) => {
                                    let _ = error;
                                    result = Some(Vec::new());
                                    break;
                                }
                            }
                        }
                        Err(error) => {
                            let reason = format!(
                                "reading context response for section {}: {error}",
                                section_index + 1
                            );
                            last_failure = Some(reason.clone());
                            if attempt == max_retries {
                                break;
                            }
                            report_translation_retry(window, job, attempt + 1, &reason).await?;
                        }
                    }
                }
                Ok(response) => {
                    let status = response.status();
                    let reason = format!(
                        "Ollama returned HTTP {status} while extracting context for section {}.",
                        section_index + 1
                    );
                    if !should_retry_http_status(status) || attempt == max_retries {
                        return Err(reason);
                    }
                    last_failure = Some(reason.clone());
                    report_translation_retry(window, job, attempt + 1, &reason).await?;
                }
                Err(error) => {
                    let reason = format!(
                        "Ollama request failed while extracting context for section {}: {error}",
                        section_index + 1
                    );
                    last_failure = Some(reason.clone());
                    if attempt == max_retries {
                        break;
                    }
                    report_translation_retry(window, job, attempt + 1, &reason).await?;
                }
            }
        }
        suggestions.extend(result.ok_or_else(|| {
            last_failure.unwrap_or_else(|| {
                format!(
                    "Context extraction failed for section {}.",
                    section_index + 1
                )
            })
        })?);
    }
    Ok(suggestions)
}

async fn update_section_story_context(
    app: &AppHandle,
    client: &Client,
    endpoint: &str,
    job: &mut Job,
    section_index: usize,
    window: &tauri::Window,
    max_retries: usize,
) -> Result<usize, String> {
    let title = job.sections[section_index].title.clone();
    let candidates =
        generate_section_context(client, endpoint, job, section_index, window, max_retries).await?;
    let mut combined = candidates
        .into_iter()
        .map(|suggestion| suggestion.fact)
        .collect::<Vec<_>>();
    combined.extend(job.context.clone());
    let curated =
        curate_story_context(client, endpoint, job, &combined, window, max_retries).await?;
    let before = job.context.len();
    let retained = if curated.is_empty() && !job.context.is_empty() {
        job.context
            .iter()
            .cloned()
            .map(|fact| ContextSuggestion { fact })
            .collect()
    } else {
        curated
    };
    job.context = normalize_story_context(
        retained
            .into_iter()
            .take(job.context_limit.clamp(1, MAX_CONTEXT_FACTS))
            .map(|suggestion| suggestion.fact)
            .collect(),
    )?;
    let added = job.context.len().saturating_sub(before);
    job.sections[section_index].context_updated = true;
    job.context_update_failures
        .retain(|failure| !failure.starts_with(&format!("{title}: ")));
    if let Some(book_id) = &job.library_id {
        let mut book = load_library_book(app, book_id).map_err(|error| error.to_string())?;
        book.context = job.context.clone();
        save_library_book(app, &book).map_err(|error| error.to_string())?;
    }

    async fn curate_story_context(
        client: &Client,
        endpoint: &str,
        job: &Job,
        candidates: &[String],
        window: &tauri::Window,
        max_retries: usize,
    ) -> Result<Vec<ContextSuggestion>, String> {
        let context_limit = job.context_limit.clamp(1, MAX_CONTEXT_FACTS);
        let request = ChatRequest {
            model: &job.model,
            messages: vec![
                ChatMessage {
                    role: "system",
                    content: format!(
                        "You maintain a compact continuity bible for a long novel. Review the existing facts and the newly observed facts below. Keep only non-explicit durable facts likely to matter later: important relationships, identities, family or faction membership, titles, stable traits, rules of the world, unresolved long-term plot facts, and terminology that affects continuity. Ignore sexual, erotic, pornographic, or other R19 material completely; never quote, describe, or analyze it. Remove one-off actions, temporary emotions, scene details, redundant wording, and facts that are not useful later. Resolve duplicates and contradictions by keeping the most recently supported wording. Return at most {context_limit} concise facts, each no longer than 500 characters, as only a JSON array shaped like [{{\"fact\":\"...\"}}]. Do not add facts that are not present in the input."
                    ),
                },
                ChatMessage {
                    role: "user",
                    content: format!(
                        "EXISTING AND NEW FACTS:\n{}",
                        context_lines(candidates)
                    ),
                },
            ],
            stream: false,
            think: false,
            options: ChatOptions { temperature: 0.0 },
        };
        let request_url = endpoint_url(endpoint, "api/chat").map_err(|error| error.to_string())?;
        let mut last_failure = None;
        for attempt in 0..=max_retries {
            match client.post(request_url.clone()).json(&request).send().await {
                Ok(response) if response.status().is_success() => {
                    match response.json::<ChatResponse>().await {
                        Ok(response) => {
                            match parse_context_suggestions(&response.message.content) {
                                Ok(suggestions) => return Ok(suggestions),
                                Err(error) => {
                                    let _ = error;
                                    return Ok(job
                                        .context
                                        .iter()
                                        .cloned()
                                        .map(|fact| ContextSuggestion { fact })
                                        .collect());
                                }
                            }
                        }
                        Err(error) => {
                            last_failure = Some(format!("reading curated story context: {error}"));
                        }
                    }
                }
                Ok(response) => {
                    let status = response.status();
                    let reason =
                        format!("Ollama returned HTTP {status} while curating story context.");
                    if !should_retry_http_status(status) {
                        return Err(reason);
                    }
                    last_failure = Some(reason);
                }
                Err(error) => {
                    last_failure = Some(format!(
                        "Ollama request failed while curating story context: {error}"
                    ));
                }
            }
            if attempt == max_retries {
                break;
            }
            let reason = last_failure
                .as_deref()
                .unwrap_or("Story context curation failed.");
            report_translation_retry(window, job, attempt + 1, reason).await?;
        }
        Err(last_failure.unwrap_or_else(|| "Story context curation failed.".into()))
    }
    save_job(app, job).map_err(|error| error.to_string())?;
    if let Some(book_id) = &job.library_id {
        window
            .emit(
                "story-context-updated",
                StoryContextUpdate {
                    book_id: book_id.clone(),
                    context: job.context.clone(),
                    added,
                },
            )
            .map_err(|error| format!("reporting updated story context: {error}"))?;
    }
    Ok(added)
}

fn record_context_update_failure(
    app: &AppHandle,
    job: &mut Job,
    section_index: usize,
    error: String,
    window: &tauri::Window,
) -> Result<(), String> {
    let title = job.sections[section_index].title.clone();
    job.sections[section_index].context_updated = false;
    job.context_update_failures
        .retain(|failure| !failure.starts_with(&format!("{title}: ")));
    job.context_update_failures
        .push(format!("{title}: {error}"));
    save_job(app, job).map_err(|error| error.to_string())?;
    window
        .emit(
            "translation-progress",
            Progress {
                job_id: job.id.clone(),
                completed: job.completed_chunks,
                total: job.total_chunks,
                estimated_remaining_seconds: estimate_remaining_seconds(
                    job.completed_chunks,
                    job.total_chunks,
                    job.elapsed_translation_millis,
                ),
                message: format!(
                    "Translation is saved, but story context for “{title}” could not be updated: {error}. Run the translation again to retry context extraction."
                ),
            },
        )
        .map_err(|error| format!("reporting story context extraction failure: {error}"))?;
    Ok(())
}

async fn update_or_record_section_context(
    app: &AppHandle,
    client: &Client,
    endpoint: &str,
    job: &mut Job,
    section_index: usize,
    window: &tauri::Window,
    max_retries: usize,
) -> Result<(), String> {
    let section_started = Instant::now();
    let update_result = update_section_story_context(
        app,
        client,
        endpoint,
        job,
        section_index,
        window,
        max_retries,
    )
    .await;
    job.elapsed_translation_millis = job
        .elapsed_translation_millis
        .saturating_add(section_started.elapsed().as_millis().min(u64::MAX as u128) as u64);
    if let Err(error) = update_result {
        record_context_update_failure(app, job, section_index, error, window)?;
    }
    save_job(app, job).map_err(|error| error.to_string())?;
    Ok(())
}

async fn send_chat_completion(
    client: &Client,
    model: &str,
    messages: Vec<ChatMessage<'_>>,
    ollama_endpoint: Option<&str>,
    max_retries: usize,
    window: &tauri::Window,
    job: &Job,
    section_index: usize,
    chunk_index: usize,
) -> Result<String, String> {
    let request = ChatRequest {
        model,
        messages,
        stream: false,
        think: false,
        options: ChatOptions { temperature: 0.2 },
    };

    let endpoint = ollama_endpoint.ok_or("Ollama endpoint not configured")?;
    let request_url = endpoint_url(endpoint, "api/chat").map_err(|error| error.to_string())?;

    let mut last_failure = None;
    for attempt in 0..=max_retries {
        let response = client.post(request_url.clone()).json(&request).send().await;
        match response {
            Ok(response) if response.status().is_success() => {
                match response.json::<ChatResponse>().await {
                    Ok(response) if !response.message.content.trim().is_empty() => {
                        return Ok(response.message.content.trim().to_string());
                    }
                    Ok(response) => {
                        last_failure = Some(empty_translation_reason(&response, section_index));
                    }
                    Err(error) => {
                        last_failure = Some(format!("reading translation for section {}: {}", section_index + 1, error));
                    }
                }
            }
            Ok(response) => {
                let status = response.status();
                let _text = response.text().await.unwrap_or_default();
                let reason = format!("Ollama returned HTTP {} while translating section {}.", status, section_index + 1);
                if !should_retry_http_status(status) {
                    return Err(reason);
                }
                last_failure = Some(reason.clone());
            }
            Err(error) => {
                let reason = format!("Ollama request failed for section {}: {}", section_index + 1, error);
                last_failure = Some(reason.clone());
            }
        }
        if attempt == max_retries {
            break;
        }
        if let Some(failure) = &last_failure {
            report_translation_retry(window, job, attempt + 1, failure).await?;
        }
    }
    Err(last_failure.unwrap_or_else(|| {
        format!(
            "Translation request for section {} failed after automatic retries.",
            section_index + 1
        )
    }))
}

#[tauri::command]
async fn translate_job(
    app: AppHandle,
    job_id: String,
    endpoint: String,
    window: tauri::Window,
    interactive: bool,
) -> Result<Job, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|error| error.to_string())?;
    let mut job = load_job(&app, &job_id).map_err(|error| error.to_string())?;
    if job.model.trim().is_empty() {
        return Err("Select a model before translating.".into());
    }

    let settings = load_settings(&app)
        .map_err(|error| format!("Loading settings: {error}"))?;
    let auto_retry = settings.auto_retry;
    let max_retries = if auto_retry {
        MAX_TRANSLATION_RETRIES
    } else {
        0
    };
    let glossary = job
        .glossary
        .clone()
        .into_iter()
        .map(|entry| {
            let gender = if entry.character_gender.trim().is_empty() {
                String::new()
            } else {
                format!(" [Character gender: {}]", entry.character_gender.trim())
            };
            if entry.note.is_empty() {
                format!("- {} → {}{}", entry.source_term, entry.target_term, gender)
            } else {
                format!(
                    "- {} → {} ({}){}",
                    entry.source_term, entry.target_term, entry.note, gender
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    for index in 0..job.sections.len() {
        let mut overrides = load_job_overrides(&app, &job.id).map_err(|error| error.to_string())?;
        apply_job_titles(&mut job, &overrides);
        if job.sections[index].translated.is_some() {
            if !job.sections[index].context_updated {
                update_or_record_section_context(
                    &app,
                    &client,
                    &endpoint,
                    &mut job,
                    index,
                    &window,
                    max_retries,
                )
                .await?;
            }
            continue;
        }
        job.current_section_index = Some(index);
        overrides.current_section_index = Some(index);
        save_job_overrides(&app, &job.id, &overrides).map_err(|error| error.to_string())?;
        save_job(&app, &job).map_err(|error| error.to_string())?;
        if let Some(next_model) = overrides.pending_model.take() {
            job.model = next_model;
            save_job(&app, &job).map_err(|error| error.to_string())?;
            save_job_overrides(&app, &job.id, &overrides).map_err(|error| error.to_string())?;
        }
        let system = build_translation_prompt(
            &job.translation_prompt,
            &job.source_language,
            &job.genre,
            &job.tones,
            &job.instructions,
            &job.context.join("\n"),
            &glossary,
        );
        let source_chunks = split_translation_chunks(&job.sections[index].text, 3000);
        if job.sections[index].translated_chunks.len() > source_chunks.len() {
            return Err(format!(
                "Saved translation state is inconsistent for section {}.",
                index + 1
            ));
        }
        for chunk_index in job.sections[index].translated_chunks.len()..source_chunks.len() {
            let chunk_started = Instant::now();
            let messages = vec![
                ChatMessage {
                    role: "system",
                    content: system.clone(),
                },
                ChatMessage {
                    role: "user",
                    content: source_chunks[chunk_index].clone(),
                },
            ];
            let mut translated_content = send_chat_completion(
                &client,
                &job.model,
                messages,
                Some(&endpoint),
                max_retries,
                &window,
                &job,
                index,
                chunk_index,
            )
            .await?;
            if interactive {
                let question_id = Uuid::new_v4().to_string();
                window
                    .emit(
                        "interactive-translation-question",
                        serde_json::json!({
                            "id": question_id,
                            "job_id": job.id,
                            "section_index": index,
                            "section_title": job.sections[index].title,
                            "chunk_index": chunk_index,
                            "source": source_chunks[chunk_index],
                            "suggestion": translated_content,
                            "timeout_seconds": 30
                        }),
                    )
                    .map_err(|error| format!("showing translation question: {error}"))?;
                let answers = INTERACTIVE_ANSWERS.get_or_init(|| Mutex::new(HashMap::new()));
                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    if let Some(answer) = answers
                        .lock()
                        .map_err(|_| "Interactive translation state was poisoned.".to_string())?
                        .remove(&question_id)
                    {
                        if !answer.trim().is_empty() {
                            translated_content = answer;
                        }
                        break;
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
            job.elapsed_translation_millis = job
                .elapsed_translation_millis
                .saturating_add(chunk_started.elapsed().as_millis().min(u64::MAX as u128) as u64);
            job.sections[index]
                .translated_chunks
                .push(translated_content);
            let (completed_chunks, total_chunks) = job_chunk_counts(&job);
            job.completed_chunks = completed_chunks;
            job.total_chunks = total_chunks;
            save_job(&app, &job).map_err(|error| error.to_string())?;
            window
                .emit(
                    "translation-progress",
                    Progress {
                        job_id: job.id.clone(),
                        completed: job.completed_chunks,
                        total: job.total_chunks,
                        estimated_remaining_seconds: estimate_remaining_seconds(
                            job.completed_chunks,
                            job.total_chunks,
                            job.elapsed_translation_millis,
                        ),
                        message: format!(
                            "Translated chunk {} of {} · {} of {} sections",
                            job.completed_chunks,
                            job.total_chunks,
                            job.completed_sections
                                + usize::from(chunk_index + 1 == source_chunks.len()),
                            job.section_count
                        ),
                    },
                )
                .map_err(|error| format!("reporting translation progress: {error}"))?;
        }
        job.sections[index].translated = Some(translated_with_source_breaks(
            &source_chunks,
            &job.sections[index].translated_chunks,
        ));
        job.completed_sections = job
            .sections
            .iter()
            .filter(|section| section.translated.is_some())
            .count();
        job.completed_chunks = job_chunk_counts(&job).0;
        save_job(&app, &job).map_err(|error| error.to_string())?;
        update_or_record_section_context(
            &app,
            &client,
            &endpoint,
            &mut job,
            index,
            &window,
            max_retries,
        )
        .await?;
    }
    job.current_section_index = None;
    let mut overrides = load_job_overrides(&app, &job.id).map_err(|error| error.to_string())?;
    overrides.current_section_index = None;
    save_job_overrides(&app, &job.id, &overrides).map_err(|error| error.to_string())?;
    save_job(&app, &job).map_err(|error| error.to_string())?;
    Ok(job)
}

#[tauri::command]
fn export_job(app: AppHandle, job_id: String, output_path: String) -> Result<(), String> {
    let job = load_job(&app, &job_id).map_err(|error| error.to_string())?;
    if job
        .sections
        .iter()
        .any(|section| section.translated.is_none())
    {
        return Err("Translate all sections before exporting.".into());
    }
    let output = Path::new(&output_path);
    match job.output_format.as_str() {
        "txt" => {
            let mut text = String::new();
            for section in &job.sections {
                text.push_str(section.translated.as_deref().unwrap_or(""));
                text.push_str("\n\n");
            }
            fs::write(output, text.trim_end())
                .map_err(|error| format!("writing TXT output: {error}"))?;
        }
        "epub" => {
            let assets_dir = jobs_dir(&app)
                .map_err(|error| error.to_string())?
                .join(&job.id)
                .join("assets");
            let assets_dir = if assets_dir.exists() {
                assets_dir
            } else if let Some(book_id) = &job.library_id {
                library_dir(&app)
                    .map_err(|error| error.to_string())?
                    .join(book_id)
                    .join("illustrations")
            } else {
                assets_dir
            };
            let cover_path = if let Some(book_id) = &job.library_id {
                let book = load_library_book(&app, book_id).map_err(|error| error.to_string())?;
                book.cover_image
                    .map(|file_name| {
                        library_dir(&app)
                            .map(|directory| directory.join(book_id).join(file_name))
                            .map_err(|error| error.to_string())
                    })
                    .transpose()?
            } else {
                None
            };
            let fallback_assets = job
                .library_id
                .as_ref()
                .map(|book_id| {
                    library_dir(&app)
                        .map(|directory| directory.join(book_id).join("illustrations"))
                        .map_err(|error| error.to_string())
                })
                .transpose()?;
            write_epub(
                &job,
                &assets_dir,
                fallback_assets.as_deref(),
                cover_path.as_deref(),
                output,
            )
            .map_err(|error| error.to_string())?
        }
        _ => return Err("Unsupported output format.".into()),
    }
    Ok(())
}

/// Snapshot a completed library translation into the reader's ignored staging folder.
/// Never write a plaintext book into reader/public/ or a Pages artifact.
#[tauri::command]
fn export_library_to_reader(app: AppHandle, book_id: String, output_dir: String) -> Result<String, String> {
    let book = load_library_book(&app, &book_id).map_err(|error| error.to_string())?;
    if book.status != "Completed" {
        return Err("Finish translation before exporting to the reader.".into());
    }
    let job_id = book.job_id.as_deref().ok_or("No translation is attached to this novel.")?;
    let job = load_job(&app, job_id).map_err(|error| error.to_string())?;
    if matches!(job.queue_status.as_deref(), Some("Translating" | "Queued")) {
        return Err("Wait until translation finishes before exporting this novel.".into());
    }
    if job.sections.is_empty() || job.sections.iter().any(|section| section.translated.is_none()) {
        return Err("All chapters must be translated before exporting.".into());
    }
    let directory = Path::new(&output_dir);
    // Prevent accidental plaintext publication, even if a caller bypasses the UI.
    if directory.file_name().and_then(|name| name.to_str()) != Some("books-private")
        || directory.parent().and_then(|parent| parent.file_name()).and_then(|name| name.to_str()) != Some("reader") {
        return Err("Select the reader/books-private directory, not a public directory.".into());
    }
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let stem = book.id.clone();
    let target = directory.join(format!("{stem}.epub"));
    let temporary = directory.join(format!("{stem}.{}.tmp", Uuid::new_v4()));
    let assets = jobs_dir(&app).map_err(|error| error.to_string())?.join(job_id).join("assets");
    let fallback = library_dir(&app).map_err(|error| error.to_string())?.join(&book.id).join("illustrations");
    let cover = book.cover_image.as_ref().map(|name| library_dir(&app).map(|dir| dir.join(&book.id).join(name)))
        .transpose().map_err(|error| error.to_string())?;
    if let Err(error) = write_epub(&job, if assets.exists() { &assets } else { &fallback }, Some(&fallback), cover.as_deref(), &temporary) {
        let _ = fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    if target.exists() { fs::remove_file(&target).map_err(|error| error.to_string())?; }
    fs::rename(&temporary, &target).map_err(|error| error.to_string())?;
    let cover_file = if let Some(source) = cover.as_ref().filter(|path| path.is_file()) {
        let extension = source.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
        if matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
            let name = format!("{stem}.cover.{extension}");
            fs::copy(source, directory.join(&name)).map_err(|error| error.to_string())?;
            Some(name)
        } else { None }
    } else { None };
    let metadata = serde_json::json!({
        "title": book.title, "original_title": book.original_title, "author": book.author,
        "synopsis": book.synopsis, "tags": book.tags, "genres": book.genres,
        "raw_tags_en": book.raw_tags_en, "raw_tags_ko": book.raw_tags_ko,
        "source_language": book.source_language, "publication_status": book.publication_status,
        "cover_file": cover_file,
    });
    fs::write(directory.join(format!("{stem}.metadata.json")), serde_json::to_vec_pretty(&metadata).map_err(|error| error.to_string())?)
        .map_err(|error| error.to_string())?;
    Ok(target.to_string_lossy().into_owned())
}

#[tauri::command]
fn answer_interactive_translation(question_id: String, answer: String) -> Result<(), String> {
    INTERACTIVE_ANSWERS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "Interactive translation state was poisoned.".to_string())?
        .insert(question_id, answer);
    Ok(())
}

#[tauri::command]
async fn retranslate_job_sections(
    app: AppHandle,
    window: tauri::Window,
    job_id: String,
    section_indices: Vec<usize>,
    instructions: String,
    genre: String,
    tones: Vec<String>,
    model: String,
    endpoint: String,
    interactive: bool,
) -> Result<Job, String> {
    let mut job = load_job(&app, &job_id).map_err(|error| error.to_string())?;
    if matches!(job.queue_status.as_deref(), Some("Queued" | "Translating")) {
        return Err("Wait until the novel is no longer in the translation queue.".into());
    }
    let mut section_indices = section_indices;
    section_indices.sort_unstable();
    section_indices.dedup();
    if section_indices.is_empty() {
        return Err("Select at least one chapter to retranslate.".into());
    }
    if section_indices
        .iter()
        .any(|index| *index >= job.sections.len())
    {
        return Err("One or more selected chapters no longer exist.".into());
    }
    let selected_model = if model.trim().is_empty() {
        job.model.clone()
    } else {
        model.trim().to_string()
    };
    if selected_model.is_empty() {
        return Err("Select a model before retranslating.".into());
    }
    
    let _app_settings = load_settings(&app).map_err(|error| error.to_string())?;

    for section_index in section_indices {
        let source_text = job.sections[section_index].text.clone();
        let settings = load_settings(&app).map_err(|error| error.to_string())?;
        let max_retries = if settings.auto_retry {
            MAX_TRANSLATION_RETRIES
        } else {
            0
        };
        let glossary = job
            .glossary
            .iter()
            .map(|entry| format!("- {} → {}", entry.source_term, entry.target_term))
            .collect::<Vec<_>>()
            .join("\n");
        let specific = instructions.trim();
        let instructions = if specific.is_empty() {
            job.instructions.clone()
        } else {
            format!(
                "{}\n\nSpecific instructions for this chapter:\n{specific}",
                job.instructions
            )
        };
        let genre = if genre.trim().is_empty() {
            job.genre.clone()
        } else {
            genre.trim().to_string()
        };
        let tones = if tones.is_empty() {
            job.tones.clone()
        } else {
            tones.clone()
        };
        let system = build_translation_prompt(
            &job.translation_prompt,
            &job.source_language,
            &genre,
            &tones,
            &instructions,
            &job.context.join("\n"),
            &glossary,
        );
        let client = Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|error| error.to_string())?;
        let source_chunks = split_translation_chunks(&source_text, 3000);
        let mut translated_chunks = Vec::with_capacity(source_chunks.len());
        for (chunk_index, source_chunk) in source_chunks.iter().enumerate() {
            let messages = vec![
                ChatMessage {
                    role: "system",
                    content: system.clone(),
                },
                ChatMessage {
                    role: "user",
                    content: source_chunk.clone(),
                },
            ];
            let mut translated_content = send_chat_completion(
                &client,
                &selected_model,
                messages,
                Some(&endpoint),
                max_retries,
                &window,
                &job,
                section_index,
                chunk_index,
            )
            .await?;
            if interactive {
                let question_id = Uuid::new_v4().to_string();
                window
                    .emit(
                        "interactive-translation-question",
                        serde_json::json!({
                            "id": question_id,
                            "section_title": job.sections[section_index].title,
                            "chunk_index": chunk_index,
                            "source": source_chunk,
                            "suggestion": translated_content,
                            "timeout_seconds": 30
                        }),
                    )
                    .map_err(|error| format!("showing translation question: {error}"))?;
                let answers = INTERACTIVE_ANSWERS.get_or_init(|| Mutex::new(HashMap::new()));
                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    if let Some(answer) = answers
                        .lock()
                        .map_err(|_| "Interactive translation state was poisoned.".to_string())?
                        .remove(&question_id)
                    {
                        if !answer.trim().is_empty() {
                            translated_content = answer;
                        }
                        break;
                    }
                    if Instant::now() >= deadline {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
            translated_chunks.push(translated_content);
        }
        let translated = translated_with_source_breaks(&source_chunks, &translated_chunks);
        let section = job
            .sections
            .get_mut(section_index)
            .ok_or_else(|| "Section no longer exists.".to_string())?;
        section.translated_chunks = translated_chunks;
        section.translated = Some(translated);
        section.context_updated = false;
        save_job(&app, &job).map_err(|error| error.to_string())?;
    }
    Ok(job)
}

fn write_epub(
    job: &Job,
    assets_dir: &Path,
    fallback_assets_dir: Option<&Path>,
    cover_path: Option<&Path>,
    path: &Path,
) -> Result<()> {
    let file = fs::File::create(path).context("creating EPUB output")?;
    let mut archive = ZipWriter::new(file);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    archive
        .start_file("mimetype", stored)
        .context("writing EPUB mimetype")?;
    archive
        .write_all(b"application/epub+zip")
        .context("writing EPUB mimetype")?;
    let compressed =
        SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    archive
        .add_directory("META-INF/", compressed)
        .context("creating EPUB metadata directory")?;
    archive
        .start_file("META-INF/container.xml", compressed)
        .context("writing EPUB container")?;
    archive.write_all(br#"<?xml version="1.0" encoding="UTF-8"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).context("writing EPUB container")?;
    archive
        .add_directory("OEBPS/", compressed)
        .context("creating EPUB content directory")?;
    archive
        .add_directory("OEBPS/text/", compressed)
        .context("creating EPUB chapter directory")?;
    let cover_file_name = if let Some(cover_path) = cover_path {
        let file_name = cover_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| anyhow!("invalid cover image filename"))?;
        let mime =
            image_mime_type(file_name).ok_or_else(|| anyhow!("unsupported cover image type"))?;
        archive
            .add_directory("OEBPS/images/", compressed)
            .context("creating EPUB image directory")?;
        let mut image = fs::File::open(cover_path).context("opening cover image")?;
        let mut bytes = Vec::new();
        image
            .read_to_end(&mut bytes)
            .context("reading cover image")?;
        archive
            .start_file(format!("OEBPS/images/{file_name}"), compressed)
            .context("writing EPUB cover image")?;
        archive
            .write_all(&bytes)
            .context("writing EPUB cover image")?;
        Some((file_name.to_string(), mime))
    } else {
        None
    };
    let title = xml_escape(&job.title);
    let mut manifest = String::new();
    let mut spine = String::new();
    let title_page = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml" xml:lang="en"><head><title>{}</title><meta charset="utf-8"/></head><body><h1>{}</h1><p>Translated with Novel Translator.</p></body></html>"#,
        title, title
    );
    archive
        .start_file("OEBPS/text/titlepage.xhtml", compressed)
        .context("writing EPUB title page")?;
    archive
        .write_all(title_page.as_bytes())
        .context("writing EPUB title page")?;
    let colophon = r#"<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml" xml:lang="en"><head><title>Colophon</title><meta charset="utf-8"/></head><body><h1>Colophon</h1><p>Translated locally with Novel Translator.</p></body></html>"#;
    archive
        .start_file("OEBPS/text/colophon.xhtml", compressed)
        .context("writing EPUB colophon")?;
    archive
        .write_all(colophon.as_bytes())
        .context("writing EPUB colophon")?;
    manifest.push_str(r#"<item id="title-page" href="text/titlepage.xhtml" media-type="application/xhtml+xml"/><item id="colophon" href="text/colophon.xhtml" media-type="application/xhtml+xml"/>"#);
    if let Some((file_name, mime)) = &cover_file_name {
        let cover_page = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml" xml:lang="en"><head><title>{}</title><meta charset="utf-8"/><style>html,body{{margin:0;width:100%;height:100%}}img{{display:block;width:100%;height:100%;object-fit:contain}}</style></head><body><img src="../images/{}" alt="Cover"/></body></html>"#,
            title,
            xml_escape(file_name)
        );
        archive
            .start_file("OEBPS/text/cover.xhtml", compressed)
            .context("writing EPUB cover page")?;
        archive
            .write_all(cover_page.as_bytes())
            .context("writing EPUB cover page")?;
        manifest.push_str(&format!(
            r#"<item id="cover-page" href="text/cover.xhtml" media-type="application/xhtml+xml"/><item id="cover-image" href="images/{}" media-type="{}" properties="cover-image"/>"#,
            xml_escape(file_name),
            mime
        ));
        spine.push_str(r#"<itemref idref="cover-page"/>"#);
    }
    spine.push_str(r#"<itemref idref="title-page"/><itemref idref="colophon"/>"#);
    let nav_items = job
        .sections
        .iter()
        .enumerate()
        .map(|(index, section)| {
            let translated = section.translated.as_deref().unwrap_or("");
            let (heading, _) = export_section_heading(&section.title, translated);
            format!(
                r#"<li><a href="text/chapter{}.xhtml">{}</a></li>"#,
                index + 1,
                xml_escape(&heading)
            )
        })
        .collect::<String>();
    let nav = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>Navigation</title></head><body><nav epub:type="toc" id="toc"><h1>Table of Contents</h1><ol><li><a href="text/titlepage.xhtml">Title Page</a></li>{}<li><a href="text/colophon.xhtml">Colophon</a></li></ol></nav></body></html>"#,
        nav_items
    );
    archive
        .start_file("OEBPS/nav.xhtml", compressed)
        .context("writing EPUB navigation")?;
    archive
        .write_all(nav.as_bytes())
        .context("writing EPUB navigation")?;
    manifest.push_str(
        r#"<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>"#,
    );
    for (index, section) in job.sections.iter().enumerate() {
        let id = format!("chapter{}", index + 1);
        let href = format!("text/{id}.xhtml");
        let image_markup = section
            .images
            .iter()
            .filter(|image| {
                assets_dir.join(&image.file_name).is_file()
                    || fallback_assets_dir
                        .map(|directory| directory.join(&image.file_name).is_file())
                        .unwrap_or(false)
            })
            .map(|image| {
                format!(
                    r#"<figure><img src="../images/{}" alt="{}" style="max-width:100%"/></figure>"#,
                    xml_escape(&image.file_name),
                    xml_escape(&image.alt)
                )
            })
            .collect::<String>();
        let translated = section.translated.as_deref().unwrap_or("");
        let (heading, translated) = export_section_heading(&section.title, translated);
        let content = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml" xml:lang="en"><head><title>{}</title><meta charset="utf-8"/><style>body{{white-space:pre-wrap;font-family:serif;line-height:1.6;margin:8%}}h1{{font-size:1.4em}}img{{height:auto}}</style></head><body><h1>{}</h1><p>{}</p>{}</body></html>"#,
            xml_escape(&heading),
            xml_escape(&heading),
            xml_escape(translated),
            image_markup,
        );
        archive
            .start_file(format!("OEBPS/{href}"), compressed)
            .with_context(|| format!("writing EPUB chapter {}", index + 1))?;
        archive
            .write_all(content.as_bytes())
            .context("writing translated chapter")?;
        manifest.push_str(&format!(
            r#"<item id="{id}" href="{href}" media-type="application/xhtml+xml"/>"#
        ));
        spine.push_str(&format!(r#"<itemref idref="{id}"/>"#));
    }
    let mut unique_images = std::collections::BTreeMap::new();
    for section in &job.sections {
        for image in &section.images {
            unique_images.insert(image.file_name.clone(), image.file_name.clone());
        }
    }
    if !unique_images.is_empty() && cover_file_name.is_none() {
        archive
            .add_directory("OEBPS/images/", compressed)
            .context("creating EPUB image directory")?;
    }
    for file_name in unique_images.keys() {
        let Some(mime) = image_mime_type(file_name) else {
            continue;
        };
        let image_path = if assets_dir.join(file_name).is_file() {
            assets_dir.join(file_name)
        } else if let Some(directory) = fallback_assets_dir {
            directory.join(file_name)
        } else {
            assets_dir.join(file_name)
        };
        let mut image = fs::File::open(image_path)
            .with_context(|| format!("opening EPUB asset {file_name}"))?;
        let mut bytes = Vec::new();
        image
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading EPUB asset {file_name}"))?;
        archive
            .start_file(format!("OEBPS/images/{file_name}"), compressed)
            .with_context(|| format!("writing EPUB asset {file_name}"))?;
        archive.write_all(&bytes).context("writing EPUB image")?;
        manifest.push_str(&format!(
            r#"<item id="asset-{}" href="images/{}" media-type="{}"/>"#,
            xml_escape(file_name),
            xml_escape(file_name),
            mime
        ));
    }
    archive
        .start_file("OEBPS/content.opf", compressed)
        .context("writing EPUB package metadata")?;
    let package = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="book-id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="book-id">{}</dc:identifier><dc:title>{}</dc:title><dc:language>en</dc:language></metadata><manifest>{}</manifest><spine>{}</spine></package>"#,
        xml_escape(&job.id),
        title,
        manifest,
        spine
    );
    archive
        .write_all(package.as_bytes())
        .context("writing EPUB package metadata")?;
    archive.finish().context("finalizing EPUB archive")?;
    Ok(())
}

fn image_mime_type(file_name: &str) -> Option<&'static str> {
    match Path::new(file_name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    #[test]
    fn epub_export_uses_first_translated_line_as_title_suffix() {
        let (heading, body) =
            export_section_heading("Chapter 1", "Vita Won't Lose!\nThe story begins.");
        assert_eq!(heading, "Chapter 1 — Vita Won't Lose!");
        assert_eq!(body, "The story begins.");
    }

    #[test]
    fn chapter_filenames_become_readable_titles() {
        assert_eq!(display_section_title("chapter001"), "Chapter 1");
        assert_eq!(display_section_title("chapter12"), "Chapter 12");
        assert_eq!(display_section_title("Prologue"), "Prologue");
    }
    use super::*;

    #[test]
    fn text_cleanup_preserves_line_breaks_and_normalizes_whitespace() {
        assert_eq!(clean_text("  one\r\ntwo\u{00a0} "), "one\ntwo");
    }

    #[test]
    fn translation_chunks_preserve_unicode_and_chunk_boundaries() {
        let source = "First line.\nSecond paragraph with a long phrase. 日本語 🙂";
        let chunks = split_translation_chunks(source, 12);
        assert!(chunks.len() > 1);
        assert_eq!(chunks.concat(), source);
        let translated =
            translated_with_source_breaks(&chunks, &vec!["A".to_string(); chunks.len()]);
        assert!(translated.contains("\n"));
        assert!(translated.contains(" "));
    }

    #[test]
    fn eta_is_unavailable_until_a_chunk_finishes_and_updates_from_average_speed() {
        assert_eq!(estimate_remaining_seconds(0, 4, 0), None);
        assert_eq!(estimate_remaining_seconds(2, 4, 30_000), Some(30));
        assert_eq!(estimate_remaining_seconds(4, 4, 30_000), Some(0));
    }

    #[test]
    fn translation_prompt_replaces_context_and_glossary_placeholders() {
        let prompt = build_translation_prompt(
            "Translate {source_language}.{genre}{tones}{instructions}{context}{glossary}",
            "Japanese",
            "fantasy",
            &["Literary".to_string()],
            "Keep honorifics.",
            "- Sakura and Mina are friends.",
            "- Sakura → Cherry Blossom",
        );
        assert!(prompt.starts_with("Translate Japanese."));
        assert!(prompt.contains("fantasy"));
        assert!(prompt.contains("Literary"));
        assert!(prompt.contains("Keep honorifics."));
        assert!(prompt.contains("Sakura and Mina are friends."));
        assert!(prompt.contains("Sakura → Cherry Blossom"));
    }

    #[test]
    fn custom_translation_prompt_without_context_placeholder_still_receives_context() {
        let prompt = build_translation_prompt(
            "Translate this {source_language} text. {glossary}",
            "Korean",
            "",
            &[],
            "",
            "The Han family is noble.",
            "",
        );
        assert!(prompt.contains("The Han family is noble."));
    }

    #[test]
    fn translation_context_is_bounded_for_large_novels() {
        let context = (0..500)
            .map(|index| format!("Fact {index}: {}", "durable ".repeat(20)))
            .collect::<Vec<_>>()
            .join("\n");
        let prompt = build_translation_prompt("{context}", "Japanese", "", &[], "", &context, "");
        assert!(prompt.chars().count() < MAX_CONTEXT_PROMPT_CHARS + 200);
        assert!(prompt.contains("Additional lower-priority context omitted"));
    }

    #[test]
    fn glossary_suggestions_parse_json_surrounded_by_model_text() {
        let suggestions = parse_glossary_suggestions(
            "```json\n[{\"source_term\":\"勇者\",\"target_term\":\"Hero\",\"note\":\"title\"}]\n```",
        )
        .unwrap();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].source_term, "勇者");
        assert_eq!(suggestions[0].target_term, "Hero");
    }

    #[test]
    fn context_suggestions_parse_and_merge_without_duplicates() {
        let suggestions = parse_context_suggestions(
            "```json\n[{\"fact\":\"Mina and Jae are friends.\"},{\"fact\":\"The Han family is noble.\"}]\n```",
        )
        .unwrap();
        let mut context = vec!["Mina and Jae are friends.".to_string()];
        let added = merge_story_context(&mut context, suggestions);
        assert_eq!(added, 1);
        assert_eq!(context.len(), 2);
        assert!(context[1].contains("Han family"));
    }

    #[test]
    fn story_context_has_a_hard_size_limit() {
        let facts = (0..=MAX_CONTEXT_FACTS)
            .map(|index| format!("Durable fact {index}"))
            .collect::<Vec<_>>();
        assert!(normalize_story_context(facts).is_err());
    }

    #[test]
    fn legacy_sections_and_library_books_default_context_safely() {
        let section: Section = serde_json::from_value(serde_json::json!({
            "title": "Chapter",
            "text": "Text",
            "translated": "Translation",
            "translated_chunks": [],
            "images": []
        }))
        .unwrap();
        assert!(section.context_updated);
    }

    #[test]
    fn source_language_detection_uses_kana_and_hangul_with_fallback() {
        assert_eq!(
            detect_source_language("これは日本語の文章です。", "Korean"),
            "Japanese"
        );
        assert_eq!(
            detect_source_language("이것은 한국어 문장입니다.", "Japanese"),
            "Korean"
        );
        assert_eq!(
            detect_source_language("No script markers", "Japanese"),
            "Japanese"
        );
    }

    #[test]
    fn plain_text_glossary_parsing_supports_notes_and_rejects_duplicates() {
        let existing = vec![GlossaryEntry {
            id: "known-id".into(),
            source_language: "Japanese".into(),
            source_term: "勇者".into(),
            target_term: "Hero".into(),
            note: String::new(),
            character_gender: String::new(),
        }];
        let parsed = parse_library_glossary_text(
            "# Terms\n勇者\tBrave One\ttitle\tfemale\n魔王\tDemon Lord",
            "Japanese",
            &existing,
        )
        .unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, "known-id");
        assert_eq!(parsed[0].note, "title");
        assert_eq!(parsed[0].character_gender, "female");
        assert!(
            parse_library_glossary_text("勇者\tHero\n勇者\tBrave One", "Japanese", &existing)
                .is_err()
        );
    }

    #[test]
    fn per_book_glossary_overrides_global_term_without_losing_other_terms() {
        let global = GlossaryEntry {
            id: "global".into(),
            source_language: "Japanese".into(),
            source_term: "勇者".into(),
            target_term: "Hero".into(),
            note: String::new(),
            character_gender: String::new(),
        };
        let another = GlossaryEntry {
            id: "another".into(),
            source_language: "Japanese".into(),
            source_term: "魔王".into(),
            target_term: "Demon Lord".into(),
            note: String::new(),
            character_gender: String::new(),
        };
        let local = GlossaryEntry {
            id: "local".into(),
            source_language: "Japanese".into(),
            source_term: "勇者".into(),
            target_term: "Brave One".into(),
            note: "novel-specific".into(),
            character_gender: "female".into(),
        };
        let merged = merge_glossary(vec![global, another], &[local.clone()]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "another");
        assert_eq!(merged[1], local);
    }

    #[test]
    fn library_books_without_context_or_glossary_load_with_empty_values() {
        let book: LibraryBook = serde_json::from_value(serde_json::json!({
            "id": "book-1",
            "title": "Novel",
            "files": ["novel.txt"],
            "source_language": "Japanese",
            "status": "Not translated",
            "job_id": null
        }))
        .unwrap();
        assert!(book.glossary.is_empty());
        assert!(book.cover_image.is_none());
        assert!(book.context.is_empty());
        assert_eq!(book.output_format, "epub");
        assert_eq!(book.publication_status, "Unknown");
        assert!(book.tags.is_empty());
        assert!(book.raw_source_url.is_none());
    }

    #[test]
    fn raw_html_extracts_card_and_detail_metadata() {
        let url = Url::parse("https://www.raw-fucknovelpia.com/index.php?q=Vita").unwrap();
        let page = parse_raw_results(
            r#"<div class="grid"><div class="card"><a class="card-title-link" href="/novel/raw-abc"><strong class="book-title">Vita doesn&#039;t lose!</strong></a><span class="book-subtitle">비타는 지지 않아!</span><div class="carousel-meta">Ongoing · KO</div><div class="info"><div class="tags"><a>romance</a></div><div class="source-tag-set" data-lang="ko"><a>로맨스</a></div></div><img src="https://assets.raw-fucknovelpia.com/covers/abc/cover.jpg"></div></div><div class="pagination"><a href="?q=Vita&amp;page=2">Next ›</a></div>"#,
            &url,
            1,
        );
        assert_eq!(page.results[0].title, "Vita doesn't lose!");
        assert_eq!(page.results[0].publication_status, "Ongoing");
        assert_eq!(page.results[0].raw_tags_ko, ["로맨스"]);
        assert_eq!(page.next_page, Some(2));
        let detail_url =
            raw_url("https://www.raw-fucknovelpia.com/novel/raw-abc", "novel").unwrap();
        let detail = parse_raw_details(r#"<meta property="og:image" content="https://assets.raw-fucknovelpia.com/covers/abc/cover.jpg"><h1 class="hero-title">Vita</h1><p class="hero-subtitle">비타</p><div class="description">A story</div><div class="tag-group"><div class="tags"><a>ts</a></div></div><div data-source-tag-group><div class="source-tag-set" data-lang="en"><a>romance</a></div></div><div class="genres"><a class="genre-pill">adult</a></div><ul class="info-list"><li><strong>Author:</strong> Writer</li><li><strong>Status:</strong> Ongoing</li></ul><a class="novel-download-link" href="/download.php?slug=raw-abc&amp;token=xyz">Download</a>"#, &detail_url).unwrap();
        assert_eq!(detail.synopsis, "A story");
        assert_eq!(detail.author, "Writer");
        assert_eq!(detail.publication_status, "Ongoing");
        assert_eq!(detail.tags, ["ts"]);
        assert_eq!(detail.genres, ["adult"]);
        assert!(detail.download_url.contains("download.php"));
    }

    #[test]
    fn raw_title_suggestions_use_embedded_titles_and_limit_results() {
        let html = r#"<script>const titles = ["Don't ignore it just because it came from a sexual game.",42,"Just Because I Have Narrow Eyes","just because we can","unrelated"];
    if (!Array.isArray(titles) || titles.length === 0) { return; }</script>"#;
        let titles = parse_raw_titles(html).unwrap();
        assert_eq!(
            match_raw_titles(&titles, "JUST because"),
            vec![
                "Don't ignore it just because it came from a sexual game.",
                "Just Because I Have Narrow Eyes",
                "just because we can",
            ]
        );
        assert!(match_raw_titles(&titles, "jus").is_empty());
        assert!(parse_raw_titles("<html>No titles</html>").is_err());
        let many = (0..20)
            .map(|i| format!("Just because {i}"))
            .collect::<Vec<_>>();
        assert_eq!(match_raw_titles(&many, "just because").len(), 12);
    }

    #[test]
    fn job_overrides_preserve_title_edits_and_pending_model_without_changing_progress() {
        let mut job: Job = serde_json::from_value(serde_json::json!({
            "id":"00000000-0000-4000-8000-000000000001", "title":"Old novel", "source_language":"Korean",
            "output_format":"epub", "genre":"", "tones":[], "instructions":"", "model":"first",
            "sections":[{"title":"Old chapter","text":"원문","translated":"English","images":[]}],
            "completed_sections":1,"section_count":1
        })).unwrap();
        let mut overrides = JobOverrides::default();
        overrides.titles.insert(0, "New chapter".into());
        overrides.novel_title = Some("New novel".into());
        overrides.pending_model = Some("second".into());
        apply_job_titles(&mut job, &overrides);
        assert_eq!(job.title, "New novel");
        assert_eq!(job.sections[0].title, "New chapter");
        assert_eq!(job.sections[0].translated.as_deref(), Some("English"));
        assert_eq!(job.model, "first");
        assert_eq!(overrides.pending_model.as_deref(), Some("second"));
    }

    #[test]
    fn novel_updates_url_and_metadata_are_validated() {
        assert!(
            validate_novel_updates_url("https://www.novelupdates.com/series/example-novel/")
                .is_ok()
        );
        assert!(validate_novel_updates_url(
            "https://www.novelupdates.com.evil.com/series/example/"
        )
        .is_err());
        assert!(
            validate_novel_updates_url("https://www.novelupdates.com/series/../admin/").is_err()
        );
        let url = validate_novel_updates_url("https://www.novelupdates.com/series/example-novel/")
            .unwrap();
        let html = r#"<div class="seriestitlenu">Example Novel</div><div id="editdescription">Story synopsis.</div><div id="showtags"><a>fantasy</a></div><div id="seriesgenre"><a>Adventure</a></div><div id="showauthors"><a>Writer</a></div><div id="editstatus">Completed</div><div id="seriesimg"><img src="/images/cover.jpg"></div>"#;
        let result = parse_novel_updates_metadata(html, &url).unwrap();
        assert_eq!(result.title, "Example Novel");
        assert_eq!(result.author, "Writer");
        assert_eq!(result.tags, ["fantasy"]);
        assert_eq!(result.genres, ["Adventure"]);
        assert_eq!(result.publication_status, "Completed");
        assert!(parse_novel_updates_metadata("<html>Access denied</html>", &url).is_err());
    }

    #[test]
    fn novel_updates_tag_lists_can_exceed_old_limit_without_losing_tags() {
        let tags = (0..120)
            .map(|index| format!("Tag {index}"))
            .chain([" tag 1 ".to_string(), "TAG 2".to_string()])
            .collect();
        let normalized = normalize_labels(tags).unwrap();
        assert_eq!(normalized.len(), 120);
        assert_eq!(normalized[0], "Tag 0");
        assert_eq!(normalized[119], "Tag 119");
        assert!(
            normalize_labels((0..501).map(|index| format!("Tag {index}")).collect())
                .unwrap_err()
                .contains("500 unique")
        );
    }

    fn advanced_test_filters() -> RawAdvancedSearch {
        RawAdvancedSearch {
            query: "Vita".into(),
            author: String::new(),
            uploader: String::new(),
            translator_group: String::new(),
            country: String::new(),
            year_from: String::new(),
            year_to: String::new(),
            status: String::new(),
            language: String::new(),
            view_lang: "en".into(),
            read_only: "any".into(),
            sort: "newest".into(),
            has_images: false,
            has_audiobook: false,
            tag_mode: "and".into(),
            tags_include: Vec::new(),
            tags_exclude: Vec::new(),
            source_tag_mode: "and".into(),
            source_tags_include: Vec::new(),
            source_tags_exclude: Vec::new(),
            genre_mode: "and".into(),
            genres_include: Vec::new(),
            genres_exclude: Vec::new(),
        }
    }

    #[test]
    fn advanced_search_builds_site_filters_and_rejects_invalid_values() {
        let mut filters = advanced_test_filters();
        filters.tags_include = vec!["romance".into(), "ts".into()];
        filters.tags_exclude = vec!["horror".into()];
        filters.source_tags_include = vec!["순애".into()];
        filters.genres_include = vec!["7".into()];
        filters.genres_exclude = vec!["11".into()];
        filters.year_from = "2020".into();
        filters.has_images = true;
        let url = raw_advanced_url(&filters, 2).unwrap();
        let pairs = url.query_pairs().collect::<Vec<_>>();
        assert_eq!(url.path(), "/search.php");
        assert_eq!(
            pairs
                .iter()
                .filter(|(key, _)| key == "tags_include[]")
                .count(),
            2
        );
        for (name, value) in [
            ("page", "2"),
            ("source_tags_include[]", "순애"),
            ("genres_include[]", "7"),
            ("genres_exclude[]", "11"),
            ("tags_exclude[]", "horror"),
            ("has_images", "1"),
        ] {
            assert!(pairs
                .iter()
                .any(|(key, found)| key == name && found == value));
        }
        filters.genres_include = vec!["999".into()];
        assert!(raw_advanced_url(&filters, 1).is_err());
        filters.genres_include = vec!["7".into()];
        filters.year_to = "2019".into();
        assert!(raw_advanced_url(&filters, 1).is_err());
    }

    #[test]
    fn advanced_search_parses_raw_and_non_raw_cards_and_pagination() {
        let url =
            Url::parse("https://www.raw-fucknovelpia.com/search.php?q=Vita&view_lang=en").unwrap();
        let html = r#"<div class="result-count">Results: 42</div><div class="grid">
        <article class="card-book"><a href="/novel/raw-79137561a591"><img src="https://assets.raw-fucknovelpia.com/covers/x/cover_thumb.jpg"><div class="title">Vita doesn&#039;t lose!</div><div class="title-sub">비타는 지지 않아!</div><div class="meta">Ongoing · ko</div><div class="tag-group"><div class="tags"><span class="tag">romance</span></div></div><div data-source-tag-group><div class="source-tag-set" data-lang="ko"><span class="tag">순애</span></div></div></a></article>
        <article class="card-book"><a href="/novel/some-translation"><div class="title">Translation</div></a></article>
        </div><div class="pagination"><a href="search.php?q=Vita&amp;page=2">Next ›</a></div>"#;
        let page = parse_raw_advanced_results(html, &url, 1);
        assert_eq!(page.total, Some(42));
        assert_eq!(page.next_page, Some(2));
        assert_eq!(page.results.len(), 2);
        assert!(page.results[0].is_raw);
        assert!(!page.results[1].is_raw);
        assert_eq!(page.results[0].title, "Vita doesn't lose!");
        assert_eq!(page.results[0].tags, ["romance"]);
        assert_eq!(page.results[0].raw_tags_ko, ["순애"]);
        let options = parse_raw_advanced_options("<datalist id='all-tags'><option value='romance'></datalist><datalist id='all-source-tags'><option value='순애'></datalist>");
        assert_eq!(options.tags, ["romance"]);
        assert_eq!(options.source_tags, ["순애"]);
    }

    #[test]
    fn advanced_search_deserializes_frontend_payload_and_respects_title_language() {
        let mut json = serde_json::json!({
            "query": "", "author": "", "uploader": "", "translator_group": "", "country": "",
            "year_from": "", "year_to": "", "status": "", "language": "", "view_lang": "ko",
            "read_only": "any", "sort": "newest", "has_images": false, "has_audiobook": false,
            "tag_mode": "and", "tags_include": [], "tags_exclude": [],
            "source_tag_mode": "and", "source_tags_include": [], "source_tags_exclude": [],
            "genre_mode": "and", "genres_include": [], "genres_exclude": [],
        });
        let filters: RawAdvancedSearch = serde_json::from_value(json.clone()).unwrap();
        let url = raw_advanced_url(&filters, 1).unwrap();
        assert!(url
            .query_pairs()
            .any(|(key, value)| key == "view_lang" && value == "ko"));
        assert!(!url.query_pairs().any(|(key, _)| key == "page"));
        json["view_lang"] = "invalid".into();
        assert!(raw_advanced_url(&serde_json::from_value(json).unwrap(), 1).is_err());
    }

    #[test]
    fn raw_links_are_restricted_and_filenames_are_safe() {
        assert!(raw_url("https://evil.example/novel/raw-abc", "novel").is_err());
        assert!(raw_url("http://www.raw-fucknovelpia.com/novel/raw-abc", "novel").is_err());
        assert!(raw_url("https://www.raw-fucknovelpia.com/novel/raw-../x", "novel").is_err());
        assert!(raw_url(
            "https://assets.raw-fucknovelpia.com/covers/x/a.jpg",
            "cover"
        )
        .is_ok());
        assert_eq!(
            safe_epub_name("Vita: The / Story?"),
            "Vita_ The _ Story_.epub"
        );
        assert_eq!(
            normalize_labels(vec![" ts ".into(), "TS".into(), "romance".into()]).unwrap(),
            ["ts", "romance"]
        );
    }

    #[test]
    fn raw_zip_rejects_bad_archives_and_non_epub_payloads() {
        assert!(decrypted_epub(b"not a zip".to_vec()).is_err());
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        writer
            .start_file("jumbled.epub", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"not an epub").unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        assert!(decrypted_epub(bytes).unwrap_err().contains("not an EPUB"));
    }

    #[test]
    fn raw_download_validation_explains_html_limits_and_incomplete_zip() {
        assert!(validate_raw_zip_response(
            b"Too many downloads from this IP. Please wait a few minutes and try again.",
            Some("text/html")
        )
        .unwrap_err()
        .contains("limiting downloads"));
        assert!(validate_raw_zip_response(
            b"<!doctype html><html>Unavailable</html>",
            Some("text/html")
        )
        .unwrap_err()
        .contains("web page instead of a ZIP"));
        assert!(validate_raw_zip_response(b"Other response", None)
            .unwrap_err()
            .contains("did not return a ZIP"));
        assert!(decrypted_epub(b"PK\x03\x04incomplete".to_vec())
            .unwrap_err()
            .contains("not a complete ZIP"));
    }

    #[test]
    fn raw_zip_decrypts_a_password_protected_epub() {
        let mut inner = ZipWriter::new(Cursor::new(Vec::new()));
        inner
            .start_file("mimetype", SimpleFileOptions::default())
            .unwrap();
        inner.write_all(b"application/epub+zip").unwrap();
        let epub = inner.finish().unwrap().into_inner();
        let mut outer = ZipWriter::new(Cursor::new(Vec::new()));
        outer
            .start_file(
                "random.epub",
                SimpleFileOptions::default()
                    .with_aes_encryption(zip::AesMode::Aes256, "taiwanandnorthkorea"),
            )
            .unwrap();
        outer.write_all(&epub).unwrap();
        let zip = outer.finish().unwrap().into_inner();
        assert_eq!(decrypted_epub(zip).unwrap(), epub);
    }

    #[test]
    fn source_titles_strip_library_copy_prefix_only() {
        assert_eq!(
            source_section_title(Path::new("0001_Chapter 17.txt"), "Chapter"),
            "Chapter 17"
        );
        assert_eq!(
            source_section_title(Path::new("chapter_001.txt"), "Chapter"),
            "chapter_001"
        );
    }

    #[test]
    fn epub_image_keys_distinguish_appended_sources() {
        assert_ne!(
            epub_source_key(Path::new("000_book.epub")),
            epub_source_key(Path::new("0001_book.epub"))
        );
        assert_eq!(
            epub_source_key(Path::new("0001_chapter.epub")),
            "0001chapter"
        );
    }

    #[test]
    fn appended_chapters_keep_existing_translations() {
        let section = |translated: Option<&str>| Section {
            title: "Chapter".into(),
            text: "Source".into(),
            translated: translated.map(str::to_string),
            translated_chunks: Vec::new(),
            images: Vec::new(),
            context_updated: true,
        };
        let mut job = Job {
            id: "test".into(),
            title: "Novel".into(),
            source_language: "Korean".into(),
            output_format: "epub".into(),
            genre: String::new(),
            tones: Vec::new(),
            instructions: String::new(),
            model: String::new(),
            sections: vec![section(Some("English"))],
            completed_sections: 1,
            section_count: 1,
            completed_chunks: 1,
            total_chunks: 1,
            elapsed_translation_millis: 0,
            current_section_index: None,
            library_id: None,
            queue_status: Some("Completed".into()),
            queue_order: 0,
            translation_prompt: String::new(),
            glossary: Vec::new(),
            context: vec!["Fact".into()],
            context_update_failures: Vec::new(),
            context_limit: 100,
        };
        reconcile_appended_sections(&mut job, vec![section(None)]).unwrap();
        assert_eq!(job.sections[0].translated.as_deref(), Some("English"));
        assert_eq!(job.completed_sections, 1);
        assert_eq!(job.section_count, 2);
        assert_eq!(job.context, ["Fact"]);
        assert!(job.queue_status.is_none());
        job.queue_status = Some("Queued".into());
        assert!(reconcile_appended_sections(&mut job, vec![section(None)]).is_err());
    }

    #[test]
    fn old_settings_without_prompt_receive_the_default_prompt() {
        let settings: AppSettings = serde_json::from_str(
            r#"{"ollama_endpoint":"http://localhost:11434","default_source_language":"Japanese","default_output_format":"epub","default_model":""}"#,
        )
        .unwrap();
        assert_eq!(settings.translation_prompt, default_translation_prompt());
        assert!(!settings.dark_mode);
        assert!(settings.auto_retry);

        let dark_settings: AppSettings = serde_json::from_str(
            r#"{"ollama_endpoint":"http://localhost:11434","default_source_language":"Japanese","default_output_format":"epub","default_model":"","dark_mode":true}"#,
        )
        .unwrap();
        assert!(dark_settings.dark_mode);
        assert!(dark_settings.auto_retry);
    }

    #[test]
    fn automatic_retry_only_retries_rate_limits_and_server_errors() {
        assert!(should_retry_http_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(should_retry_http_status(
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(!should_retry_http_status(reqwest::StatusCode::NOT_FOUND));
        assert!(!should_retry_http_status(reqwest::StatusCode::BAD_REQUEST));
    }

    #[test]
    fn translation_request_disables_separate_model_thinking() {
        let request = ChatRequest {
            model: "local-model",
            messages: vec![ChatMessage {
                role: "user",
                content: "Translate this.".into(),
            }],
            stream: false,
            think: false,
            options: ChatOptions { temperature: 0.2 },
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["think"], false);
    }

    #[test]
    fn empty_model_responses_include_ollama_finish_diagnostics() {
        let response: ChatResponse = serde_json::from_str(
            r#"{"message":{"role":"assistant","content":"","thinking":"internal reasoning"},"done_reason":"length","eval_count":64}"#,
        )
        .unwrap();
        let reason = empty_translation_reason(&response, 0);
        assert!(reason.contains("finish reason: length"));
        assert!(reason.contains("64 generated tokens"));
        assert!(reason.contains("separate thinking text"));
        assert!(!reason.contains("internal reasoning"));
    }

    #[test]
    fn multiple_input_files_must_all_be_txt() {
        assert!(
            validate_input_paths(&["chapter1.txt".to_string(), "chapter2.txt".to_string()]).is_ok()
        );
        assert!(validate_input_paths(&["book.epub".to_string(), "notes.txt".to_string()]).is_err());
        assert!(validate_input_paths(&["book.docx".to_string()]).is_err());
    }

    #[test]
    fn epub_text_extraction_keeps_paragraph_breaks() {
        let html = br#"<html><body><h1>Title</h1><p>First paragraph.</p><p>Second<br/>line.</p></body></html>"#;
        let text = html_text(html).unwrap();
        assert_eq!(text, "Title\n\nFirst paragraph.\n\nSecond\nline.");
    }

    #[test]
    fn endpoint_path_keeps_custom_base_prefix() {
        let url = endpoint_url("http://localhost:11434/", "api/tags").unwrap();
        assert_eq!(url.as_str(), "http://localhost:11434/api/tags");
    }

    #[test]
    fn epub_asset_resolution_is_relative_to_chapter_and_rejects_remote_urls() {
        assert_eq!(
            resolve_epub_asset("OPS/Text/chapter.xhtml", "../Images/cover.png"),
            Some("OPS/Images/cover.png".to_string())
        );
        assert_eq!(
            resolve_epub_asset("OPS/Text/chapter.xhtml", "https://example.com/image.png"),
            None
        );
    }

    #[test]
    fn epub_chapter_paths_sort_naturally() {
        let mut paths = vec![
            "OPS/chapter10.xhtml",
            "OPS/chapter2.xhtml",
            "OPS/chapter1.xhtml",
        ];
        paths.sort_by(|left, right| natural_cmp(left, right));
        assert_eq!(
            paths,
            vec![
                "OPS/chapter1.xhtml",
                "OPS/chapter2.xhtml",
                "OPS/chapter10.xhtml"
            ]
        );
    }

    #[test]
    fn epub_metadata_escapes_xml() {
        assert_eq!(xml_escape("A & B <C>"), "A &amp; B &lt;C&gt;");
    }

    #[test]
    fn epub_export_embeds_images_and_valid_mimetype() {
        let test_dir = std::env::temp_dir().join(format!("novel-ai-{}", Uuid::new_v4()));
        let assets_dir = test_dir.join("assets");
        fs::create_dir_all(&assets_dir).unwrap();
        fs::write(assets_dir.join("image-0001.png"), b"image data").unwrap();
        let cover_path = test_dir.join("cover.png");
        fs::write(&cover_path, b"cover image").unwrap();
        let output_path = test_dir.join("book.epub");
        let job = Job {
            id: "test-book".to_string(),
            title: "A & B".to_string(),
            source_language: "Japanese".to_string(),
            output_format: "epub".to_string(),
            genre: String::new(),
            tones: Vec::new(),
            instructions: String::new(),
            model: "local-model".to_string(),
            sections: vec![Section {
                title: "Opening".to_string(),
                text: "Original".to_string(),
                translated: Some("Book Title\r\nEnglish text".to_string()),
                translated_chunks: vec!["Book Title\r\nEnglish text".to_string()],
                context_updated: true,
                images: vec![ImageAsset {
                    file_name: "image-0001.png".to_string(),
                    alt: "A scene".to_string(),
                }],
            }],
            completed_sections: 1,
            section_count: 1,
            completed_chunks: 1,
            total_chunks: 1,
            elapsed_translation_millis: 1000,
            current_section_index: None,
            library_id: None,
            queue_status: None,
            queue_order: 0,
            translation_prompt: default_translation_prompt(),
            glossary: Vec::new(),
            context: Vec::new(),
            context_update_failures: Vec::new(),
            context_limit: default_context_limit(),
        };
        write_epub(&job, &assets_dir, None, Some(&cover_path), &output_path).unwrap();
        let file = fs::File::open(&output_path).unwrap();
        let mut archive = ZipArchive::new(file).unwrap();
        assert_eq!(archive.by_index(0).unwrap().name(), "mimetype");
        let mut mimetype = String::new();
        archive
            .by_name("mimetype")
            .unwrap()
            .read_to_string(&mut mimetype)
            .unwrap();
        assert_eq!(mimetype, "application/epub+zip");
        assert!(archive.by_name("OEBPS/images/image-0001.png").is_ok());
        assert!(archive.by_name("OEBPS/images/cover.png").is_ok());
        let mut package = String::new();
        archive
            .by_name("OEBPS/content.opf")
            .unwrap()
            .read_to_string(&mut package)
            .unwrap();
        assert!(package.contains(r#"properties="cover-image""#));
        let mut cover_page = String::new();
        archive
            .by_name("OEBPS/text/cover.xhtml")
            .unwrap()
            .read_to_string(&mut cover_page)
            .unwrap();
        assert!(cover_page.contains("../images/cover.png"));
        let mut chapter = String::new();
        archive
            .by_name("OEBPS/text/chapter1.xhtml")
            .unwrap()
            .read_to_string(&mut chapter)
            .unwrap();
        assert!(chapter.contains("English text"));
        assert!(chapter.contains("Opening — Book Title"));
        assert!(chapter.contains("../images/image-0001.png"));
        assert!(!chapter.contains("Original"));
        let mut navigation = String::new();
        archive
            .by_name("OEBPS/nav.xhtml")
            .unwrap()
            .read_to_string(&mut navigation)
            .unwrap();
        assert!(navigation.contains("Opening — Book Title"));
        drop(archive);
        fs::remove_dir_all(test_dir).unwrap();
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            list_ollama_models,
            get_app_settings,
            update_app_settings,
            create_job,
            list_library,
            update_library_metadata,
            update_library_title,
            fetch_novel_updates_metadata,
            fetch_novel_updates_in_browser,
            apply_novel_updates_metadata,
            append_library_chapter,
            append_library_chapter_files,
            search_raw_novels,
            search_raw_advanced,
            get_raw_advanced_options,
            suggest_raw_titles,
            fetch_raw_novel_details,
            import_raw_novel,
            save_library_context,
            import_library_book,
            replace_library_book,
            list_library_illustrations,
            set_library_cover_image,
            remove_library_cover_image,
            delete_library_book,
            update_library_source_language,
            generate_library_glossary,
            generate_job_glossary_suggestions,
            save_library_glossary_text,
            save_library_glossary_entry,
            delete_library_glossary_entry,
            enqueue_library_book,
            list_queue,
            update_queue_status,
            restart_library_translation,
            list_glossary,
            write_text_file,
            save_glossary_entry,
            delete_glossary_entry,
            list_jobs,
            delete_saved_job,
            get_job,
            set_queue_model,
            get_queue_model_override,
            update_job_section_title,
            update_job_section_titles,
            update_job_section_text,
            update_job_section_translation,
            update_job_settings,
            translate_job,
            answer_interactive_translation,
            retranslate_job_sections,
            export_job,
            export_library_to_reader
        ])
        .run(tauri::generate_context!())
        .expect("error while running Novel Translator");
}
#[test]
fn generated_character_glossary_preserves_gender() {
    let suggestions = parse_glossary_suggestions(
            r#"[{"source_term":"그녀","target_term":"she","note":"pronoun","character_gender":"female"}]"#,
        )
        .unwrap();
    assert_eq!(suggestions[0].character_gender, "female");
}
#[test]
fn epub_export_does_not_duplicate_existing_chapter_heading() {
    assert_eq!(
        without_duplicate_export_heading("chapter000", "Chapter 000\nOnce upon a time."),
        "Once upon a time."
    );
    assert_eq!(
        without_duplicate_export_heading("chapter000", "The first scene."),
        "The first scene."
    );
}
