import ePub from 'epubjs';
import { decrypt, deriveKey, forgetKey, loadKey, readCatalog, saveKey } from './crypto.js';
import { adjacentSection } from './chapter-navigation.js';
import { readerFontCss } from './reader-fonts.js';
import { normalizedProgress, scrollFraction, sortedChapters } from './reading-progress.js';
import { layoutFor } from './layout.js';
import './style.css';
import './scrollbar.css';
import './interface.css';
import './mobile-reader.css';

const $ = id => document.getElementById(id);
const root = import.meta.env.BASE_URL;
const presets = {
  light: { background: '#ffffff', foreground: '#242936' },
  sepia: { background: '#f6efdf', foreground: '#443829' },
  dark: { background: '#101010', foreground: '#cccccc' },
};
const defaults = { theme: 'dark', ...presets.dark, interfaceMode: 'match', interfaceBackground: '#101010', interfaceForeground: '#cccccc', font: 'Georgia, serif', fontSize: 18, lineSpacing: 1.7, width: 760, flow: 'scrolled-chapter' };
let settings;
try { settings = { ...defaults, ...JSON.parse(localStorage.getItem('reader-settings')) }; }
catch { settings = { ...defaults }; }
// Older versions labeled single-section scrolling as "Continuous scroll".
if (settings.flow === 'scrolled-doc') settings.flow = 'scrolled-chapter';
if (!['match', 'custom'].includes(settings.interfaceMode)) settings.interfaceMode = 'match';
let index, key, catalog = [], book, rendition, activeId, toc = [], progressTimer, resizeTimer, toastTimer, detailBook;
let currentSectionIndex = null;
let chapters = [], pendingLocation, scrollContainer, scrollListener, loadingBook = false, advancing = false;
let scrolledToEnd = false, scrollingForward = false, lastScrollTop = 0, seekingChapter = false;
let mobileChromeTimer, previousTapAt = 0, mobileHintTimer;

function setMobileChrome(hidden, autoHide = false) {
  const isMobileReader = document.body.classList.contains('reader-active') && matchMedia('(max-width: 600px)').matches;
  document.body.classList.toggle('reader-chrome-hidden', isMobileReader && hidden);
  clearTimeout(mobileChromeTimer);
  if (isMobileReader && !hidden && autoHide) {
    mobileChromeTimer = setTimeout(() => setMobileChrome(true), 4000);
  }
}

function updateNavigationButtons() {
  const chapterMode = settings.flow === 'scrolled-chapter';
  $('prev-button').textContent = chapterMode ? '← Previous chapter' : '← Previous';
  $('next-button').textContent = chapterMode ? 'Next chapter →' : 'Next →';
  $('prev-button').disabled = !rendition || (chapterMode && !adjacentSection(book?.spine, currentSectionIndex, 'prev'));
  $('next-button').disabled = !rendition || (chapterMode && !adjacentSection(book?.spine, currentSectionIndex, 'next'));
}

async function navigate(direction) {
  if (!rendition) return;
  try {
    const previous = currentSectionIndex;
    flushProgress();
    scrolledToEnd = false;
    scrollingForward = false;
    if (settings.flow !== 'scrolled-chapter') {
      advancing = direction === 'next';
      await (direction === 'next' ? rendition.next() : rendition.prev());
      const nextIndex = rendition.currentLocation()?.start?.index;
      if (direction === 'next' && Number.isInteger(nextIndex) && previous !== nextIndex) markRead(previous);
      advancing = false;
      return;
    }
    const section = adjacentSection(book?.spine, currentSectionIndex, direction);
    if (!section) return;
    pendingLocation = null;
    await rendition.display(section.href); // unlike rendition.prev(), always starts at the top
    if (direction === 'next') markRead(previous);
    const location = rendition.currentLocation();
    if (location?.start) updateLocation(location);
  } catch (error) { toast(`Cannot change chapter: ${error.message}`); }
  finally { advancing = false; }
}

function toast(message) {
  $('toast').textContent = message;
  $('toast').hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { $('toast').hidden = true; }, 5000);
}

function show(view) {
  for (const name of ['unlock', 'library', 'reader']) $(`${name}-view`).hidden = name !== view;
  document.body.classList.toggle('reader-active', view === 'reader');
  document.body.classList.remove('reader-chrome-hidden');
  clearTimeout(mobileChromeTimer);
  if (view === 'reader') {
    // Show chrome briefly on entry, then auto-hide after 4 s
    setMobileChrome(false, true);
    // Show a one-time hint on mobile so the user knows how to recall chrome
    if (matchMedia('(max-width: 600px)').matches && !localStorage.getItem('reader-hint-seen')) {
      const hint = document.getElementById('mobile-chrome-hint');
      if (hint) {
        hint.hidden = false;
        clearTimeout(mobileHintTimer);
        mobileHintTimer = setTimeout(() => {
          hint.classList.add('fade-out');
          setTimeout(() => { hint.hidden = true; hint.classList.remove('fade-out'); }, 650);
          localStorage.setItem('reader-hint-seen', '1');
        }, 3500);
      }
    }
  }
  $('library-button').hidden = view !== 'reader';
  $('settings-button').hidden = view === 'unlock';
  $('header-title').textContent = view === 'reader' ? catalog.find(item => item.id === activeId)?.title || 'Reader' : 'My Library';
  closePanels();
}

function closePanels() {
  $('backdrop').hidden = true;
  for (const id of ['chapters-panel', 'settings-panel']) {
    $(id).classList.remove('open');
    $(id).setAttribute('aria-hidden', 'true');
  }
}
function openPanel(id) {
  closePanels();
  $('backdrop').hidden = false;
  $(id).classList.add('open');
  $(id).setAttribute('aria-hidden', 'false');
}

function applySettings() {
  document.documentElement.style.setProperty('--page', settings.background);
  document.documentElement.style.setProperty('--ink', settings.foreground);
  const uiBackground = settings.interfaceMode === 'custom' ? settings.interfaceBackground : settings.background;
  const uiForeground = settings.interfaceMode === 'custom' ? settings.interfaceForeground : settings.foreground;
  document.documentElement.style.setProperty('--ui-bg', uiBackground);
  document.documentElement.style.setProperty('--ui-fg', uiForeground);
  document.querySelector('meta[name="theme-color"]').content = uiBackground;
  document.documentElement.style.setProperty('--read-width', `${settings.width}px`);
  // Match native inputs and scrollbars to the selected interface background.
  const rgb = uiBackground.match(/^#([\da-f]{6})$/i)?.[1];
  if (rgb) {
    const channels = [0, 2, 4].map(offset => parseInt(rgb.slice(offset, offset + 2), 16) / 255);
    document.documentElement.style.colorScheme = channels.reduce((sum, value, index) => sum + value * [0.2126, 0.7152, 0.0722][index], 0) < 0.5 ? 'dark' : 'light';
  }
  $('theme').value = settings.theme;
  $('background').value = settings.background;
  $('foreground').value = settings.foreground;
  $('interface-mode').value = settings.interfaceMode;
  $('interface-colors').hidden = settings.interfaceMode !== 'custom';
  $('interface-background').value = settings.interfaceBackground;
  $('interface-foreground').value = settings.interfaceForeground;
  $('font').value = settings.font;
  $('font-size').value = settings.fontSize;
  $('font-size-value').value = `${settings.fontSize}px`;
  $('line-spacing').value = settings.lineSpacing;
  $('line-spacing-value').value = settings.lineSpacing;
  $('width').value = settings.width;
  $('width-value').value = `${settings.width}px`;
  $('flow').value = settings.flow;
  updateNavigationButtons();
  localStorage.setItem('reader-settings', JSON.stringify(settings));
  if (rendition) {
    rendition.themes.default({ html: { background: `${settings.background} !important` }, body: { color: `${settings.foreground} !important`, background: `${settings.background} !important`, 'font-family': `${settings.font} !important`, 'font-size': `${settings.fontSize}px !important`, 'line-height': `${settings.lineSpacing} !important` },
      p: { 'line-height': `${settings.lineSpacing} !important` }, 'a:link': { color: `${settings.foreground} !important` } });
    rendition.themes.font(settings.font);
    rendition.themes.fontSize(`${settings.fontSize}px`);
  }
}

function progress(id) {
  try { return normalizedProgress(JSON.parse(localStorage.getItem(`reader-progress-${id}`))); }
  catch { return normalizedProgress(null); }
}

function writeProgress(id, patch) {
  if (!id) return;
  try { localStorage.setItem(`reader-progress-${id}`, JSON.stringify({ ...progress(id), ...patch })); }
  catch (error) { console.warn('Could not save reading progress:', error); }
}

function markRead(sectionIndex) {
  if (!activeId || !Number.isInteger(sectionIndex) || sectionIndex < 0) return;
  const state = progress(activeId);
  if (state.read.includes(sectionIndex)) return;
  writeProgress(activeId, { read: [...state.read, sectionIndex] });
  if (detailBook?.id === activeId) renderDetailChapters();
}

function getScrollContainer() {
  return rendition?.manager?.container || null;
}

function flushProgress() {
  clearTimeout(progressTimer);
  if (!activeId || loadingBook || !rendition) return;
  const latest = rendition.currentLocation();
  const location = latest?.start ? latest : pendingLocation || rendition.location;
  const position = location?.start;
  if (!position?.cfi || !Number.isInteger(position.index)) return;
  // Continuous mode mounts and removes sections on demand, so its total scroll
  // height is unstable. Its CFI is the reliable saved position.
  const scroll = settings.flow === 'scrolled-chapter' ? scrollFraction(getScrollContainer()) : null;
  writeProgress(activeId, { cfi: position.cfi, index: position.index, scroll });
  if (scroll !== null) {
    const total = book?.spine?.spineItems?.length || 1;
    $('progress-label').textContent = `${Math.min(100, Math.round((position.index + scroll) / total * 100))}%`;
  }
  if (scrolledToEnd && scroll !== null && scroll >= 0.985 && position.index === currentSectionIndex) markRead(position.index);
  pendingLocation = null;
}

function scheduleProgress(location) {
  if (loadingBook) return;
  if (location?.start) pendingLocation = location;
  clearTimeout(progressTimer);
  progressTimer = setTimeout(flushProgress, 350);
}

function detachScroll() {
  if (scrollContainer && scrollListener) scrollContainer.removeEventListener('scroll', scrollListener);
  scrollContainer = null;
  scrollListener = null;
}

function attachScroll() {
  detachScroll();
  scrollContainer = getScrollContainer();
  if (!scrollContainer) return;
  scrollContainer.style.touchAction = 'pan-y';
  scrollContainer.style.overscrollBehaviorY = 'contain';
  scrollContainer.style.webkitOverflowScrolling = 'touch';
  lastScrollTop = scrollContainer.scrollTop;
  scrollListener = () => {
    if (loadingBook || settings.flow === 'paginated') return;
    if (matchMedia('(max-width: 600px)').matches) setMobileChrome(true);
    const container = getScrollContainer();
    if (settings.flow === 'scrolled-continuous' && !seekingChapter) {
      scrollingForward = container.scrollTop > lastScrollTop + 1;
      lastScrollTop = container.scrollTop;
    }
    if (settings.flow === 'scrolled-chapter' && container && container.scrollHeight > container.clientHeight + 1 && scrollFraction(container) >= 0.985) scrolledToEnd = true;
    rendition.reportLocation();
    scheduleProgress(rendition.location);
  };
  scrollContainer.addEventListener('scroll', scrollListener, { passive: true });
}

function unloadBook(save = true) {
  if (save) flushProgress();
  detachScroll();
  clearTimeout(progressTimer);
  if (rendition) rendition.destroy();
  if (book) book.destroy();
  rendition = null; book = null; currentSectionIndex = null; pendingLocation = null;
  scrolledToEnd = false; scrollingForward = false; seekingChapter = false;
}

function tagsFor(item) { return [...new Set([...(item.tags || []), ...(item.genres || []), ...(item.raw_tags_en || []), ...(item.raw_tags_ko || [])])]; }
function setText(id, value) { $(id).textContent = value || ''; }
function renderDetailChapters() {
  const list = $('detail-chapter-list'); list.replaceChildren();
  if (!detailBook || !chapters.length) return;
  const state = progress(detailBook.id);
  setText('detail-chapter-status', `${state.read.filter(index => chapters.some(item => item.index === index)).length} of ${chapters.length} chapters read on this browser`);
  for (const item of sortedChapters(chapters, state.read, $('chapter-sort').value)) {
    const button = document.createElement('button'); button.type = 'button'; button.className = 'detail-chapter';
    const name = document.createElement('span'); name.textContent = item.label;
    const status = document.createElement('span');
    status.className = 'chapter-state';
    status.textContent = state.read.includes(item.index) ? '✓ Read' : state.index === item.index ? 'In progress' : 'Unread';
    button.append(name, status);
    button.addEventListener('click', () => openBook(detailBook.id, item.href));
    list.append(button);
  }
}

async function loadDetailChapters(item) {
  detailBook = item;
  chapters = [];
  setText('detail-chapter-status', 'Loading chapters…');
  $('detail-chapter-list').replaceChildren();
  try {
    const encrypted = await fetchJson(`${item.id}.json`);
    const bytes = await decrypt(key, encrypted);
    const preview = ePub(bytes);
    try {
      await preview.ready;
      const navigation = await preview.loaded.navigation;
      const labels = flattenToc(navigation.toc || []);
      const entries = preview.spine.spineItems.filter(section => section.linear).map(section => ({
        index: section.index, href: section.href, order: section.index,
        label: labels.find(entry => entry.href?.split('#')[0] === section.href)?.label || `Section ${section.index + 1}`,
      }));
      if (detailBook === item) chapters = entries;
    } finally { preview.destroy(); }
    if (detailBook === item) {
      if (!chapters.length) setText('detail-chapter-status', 'No chapters found in this EPUB.');
      else renderDetailChapters();
    }
  } catch (error) {
    if (detailBook === item) setText('detail-chapter-status', `Cannot load chapters: ${error.message}`);
  }
}
function showDetails(item) {
  $('library-controls').hidden = true;
  $('book-list').hidden = true;
  $('book-details').hidden = false;
  setText('detail-title', item.title);
  setText('detail-original', item.original_title);
  setText('detail-author', item.author ? `By ${item.author}` : 'Unknown author');
  setText('detail-meta', [item.source_language, item.publication_status && `${item.publication_status} publication`].filter(Boolean).join(' · '));
  setText('detail-synopsis', item.synopsis || 'No synopsis available.');
  const cover = $('detail-cover'); cover.replaceChildren();
  if (item.cover) { const image = document.createElement('img'); image.src = item.cover; image.alt = `Cover of ${item.title}`; cover.append(image); }
  else cover.textContent = '✦';
  const tags = $('detail-tags'); tags.replaceChildren();
  for (const tag of tagsFor(item)) { const chip = document.createElement('span'); chip.className = 'tag'; chip.textContent = tag; tags.append(chip); }
  $('read-button').textContent = progress(item.id).cfi ? 'Continue reading →' : 'Start reading →';
  $('read-button').onclick = () => openBook(item.id);
  show('library');
  window.scrollTo(0, 0);
  loadDetailChapters(item);
}

function updateFilters() {
  const tagSelect = $('tag-filter'); const selected = tagSelect.value;
  tagSelect.replaceChildren(new Option('All tags', ''));
  for (const tag of [...new Set(catalog.flatMap(tagsFor))].sort((a, b) => a.localeCompare(b))) tagSelect.add(new Option(tag, tag));
  tagSelect.value = selected;
}

function renderLibrary() {
  $('book-details').hidden = true;
  $('library-controls').hidden = false;
  const list = $('book-list');
  list.hidden = false;
  list.replaceChildren();
  const query = $('book-search').value.trim().toLocaleLowerCase();
  const visible = catalog.filter(item => {
    if ($('publication-filter').value && (item.publication_status || 'Unknown') !== $('publication-filter').value) return false;
    if ($('language-filter').value && item.source_language !== $('language-filter').value) return false;
    if ($('tag-filter').value && !tagsFor(item).includes($('tag-filter').value)) return false;
    return !query || [item.title, item.original_title, item.author, item.synopsis, item.source_language, item.publication_status, ...tagsFor(item)].some(value => String(value || '').toLocaleLowerCase().includes(query));
  });
  setText('result-count', `${visible.length} of ${catalog.length} novels`);
  if (!visible.length) {
    const empty = document.createElement('p');
    empty.textContent = catalog.length ? 'No novels match your search.' : 'Your library is empty. Add EPUBs and run the packaging script.';
    list.append(empty);
  }
  for (const item of visible) {
    const card = document.createElement('button');
    card.type = 'button';
    card.className = 'book-card';
    const cover = document.createElement('span');
    cover.className = 'book-cover';
    if (item.cover) { const image = document.createElement('img'); image.src = item.cover; image.alt = ''; cover.append(image); }
    else cover.textContent = '✦';
    const details = document.createElement('span'); details.className = 'book-details';
    const title = document.createElement('strong'); title.textContent = item.title;
    const position = document.createElement('span');
    position.textContent = progress(item.id).cfi ? 'Continue reading →' : 'Start reading →';
    const author = document.createElement('span'); author.textContent = item.author || item.original_title || '';
    const chips = document.createElement('span'); chips.className = 'card-tags'; chips.textContent = tagsFor(item).slice(0, 3).join(' · ');
    details.append(title, author, chips, position);
    card.append(cover, details);
    card.addEventListener('click', () => showDetails(item));
    list.append(card);
  }
  show('library');
}

async function fetchJson(path) {
  const response = await fetch(`${root}books/${path}`, { cache: 'no-store' });
  if (!response.ok) throw new Error(`Unable to load ${path} (${response.status}).`);
  return response.json();
}

async function initialize() {
  try { applySettings(); }
  catch (error) { console.error('Could not initialize reader settings:', error); }
  try { index = await fetchJson('index.json'); }
  catch (error) { show('unlock'); $('unlock-error').textContent = `${error.message} Publish encrypted books first.`; $('unlock-form').querySelector('button').disabled = true; return; }
  if (index.version !== 1) { show('unlock'); $('unlock-error').textContent = 'Unsupported library format.'; $('unlock-form').querySelector('button').disabled = true; return; }
  try {
    key = await loadKey();
  } catch (error) { console.warn('Could not restore the remembered reader key:', error); key = null; }
  if (key) {
    try { catalog = await readCatalog(key, index); }
    catch (error) { console.warn('Remembered key cannot decrypt the current catalog:', error); key = null; }
    if (key) { updateFilters(); renderLibrary(); return; }
  }
  show('unlock');
}

function flattenToc(items, depth = 0) {
  return items.flatMap(item => [{ label: item.label, href: item.href, depth }, ...flattenToc(item.subitems || [], depth + 1)]);
}

function renderToc() {
  const list = $('chapter-list'); list.replaceChildren();
  for (const item of toc) {
    const link = document.createElement('button');
    link.type = 'button'; link.textContent = item.label;
    link.style.paddingLeft = `${20 + Math.min(item.depth, 5) * 16}px`;
    link.addEventListener('click', async () => {
      closePanels();
      try {
        flushProgress(); scrolledToEnd = false; scrollingForward = false; seekingChapter = true;
        await rendition.display(item.href);
      }
      catch (error) { toast(`Cannot open chapter: ${error.message}`); }
      finally { seekingChapter = false; lastScrollTop = getScrollContainer()?.scrollTop || 0; }
    });
    list.append(link);
  }
}

function updateLocation(location) {
  if (!location?.start || !activeId) return;
  const { href, index: spineIndex } = location.start;
  if (Number.isInteger(currentSectionIndex) && spineIndex !== currentSectionIndex) {
    if (advancing || (settings.flow === 'scrolled-continuous' && scrollingForward && !loadingBook && !seekingChapter && spineIndex > currentSectionIndex)) markRead(currentSectionIndex);
    scrolledToEnd = false;
    scrollingForward = false;
  }
  currentSectionIndex = spineIndex;
  updateNavigationButtons();
  scheduleProgress(location);
  const current = toc.filter(item => {
    const chapter = item.href?.split('#')[0];
    return href && chapter && (href.split('#')[0].endsWith(chapter) || chapter.endsWith(href.split('#')[0]));
  }).at(-1);
  $('chapter-name').textContent = current?.label || `Section ${spineIndex + 1}`;
  const total = book.spine?.spineItems?.length || 1;
  const percentage = Math.min(100, Math.round((spineIndex / total) * 100));
  $('progress-label').textContent = `${percentage}%`;
  $('location-label').textContent = `Section ${spineIndex + 1} of ${total}`;
}

async function openBook(id, locationOverride, savedBeforeSwitch = false) {
  if (!key) return;
  const item = catalog.find(entry => entry.id === id);
  if (!item) return;
  try {
    unloadBook(!savedBeforeSwitch);
    activeId = id;
    detailBook = null;
    loadingBook = true;
    show('reader');
    $('chapter-name').textContent = 'Loading book…';
    $('progress-label').textContent = '';
    $('location-label').textContent = '';
    $('book-area').replaceChildren();
    const encrypted = await fetchJson(`${id}.json`);
    const bytes = await decrypt(key, encrypted);
    book = ePub(bytes);
    await book.ready;
    toc = flattenToc((await book.loaded.navigation).toc || []);
    renderToc();
    rendition = book.renderTo('book-area', {
      width: '100%', height: '100%', ...layoutFor(settings.flow),
      spread: 'none', allowScriptedContent: false,
    });
    const fontCss = readerFontCss();
    rendition.hooks.content.register(contents => {
      contents.addStylesheetCss(fontCss, 'reader-font-faces');
      const locationHref = contents.section?.href?.split('#')[0];
      const title = toc.filter(item => item.href?.split('#')[0] === locationHref).at(-1)?.label;
      if (title) {
        const normalize = value => value.replace(/[\s\p{P}\p{S}]/gu, '').toLocaleLowerCase();
        const heading = [...contents.document.querySelectorAll('h1,h2,h3,h4,h5,h6')]
          .find(element => normalize(element.textContent) === normalize(title));
        if (heading) heading.style.display = 'none';
      }
      contents.document.addEventListener('touchend', event => {
        const now = Date.now();
        if (now - previousTapAt < 350) {
          event.preventDefault();
          setMobileChrome(!document.body.classList.contains('reader-chrome-hidden'), true);
          previousTapAt = 0;
        } else {
          previousTapAt = now;
        }
      }, { passive: false });
    });
    rendition.on('relocated', location => {
      updateLocation(location);
    });
    applySettings();
    const saved = progress(id);
    const target = locationOverride || saved.cfi;
    try { await rendition.display(target || undefined); }
    catch { await rendition.display(); }
    // epub.js may initially report the beginning of a section before the
    // requested CFI is positioned. Restore scroll after display, then persist.
    attachScroll();
    const savedSection = saved.cfi ? book.spine.get(saved.cfi) : null;
    if (!locationOverride && settings.flow === 'scrolled-chapter' && saved.scroll !== null &&
        saved.index === savedSection?.index) {
      const container = getScrollContainer();
      if (container) container.scrollTop = saved.scroll * Math.max(0, container.scrollHeight - container.clientHeight);
    }
    loadingBook = false;
    if (rendition.location?.start) updateLocation(rendition.location);
  } catch (error) {
    loadingBook = false;
    unloadBook();
    toast(`Cannot open book: ${error.message}`);
    renderLibrary();
  }
}

$('unlock-form').addEventListener('submit', async event => {
  event.preventDefault();
  if (!index) return;
  const button = event.currentTarget.querySelector('button');
  button.disabled = true;
  $('unlock-error').textContent = '';
  try {
    const candidate = await deriveKey($('passphrase').value, index.salt);
    try { catalog = await readCatalog(candidate, index); }
    catch (error) {
      if (error.name === 'OperationError') $('unlock-error').textContent = 'This passphrase cannot decrypt the current catalog. Use the exact passphrase from the last successful npm run pack (case, spaces and punctuation matter).';
      else { console.error('Could not decode the decrypted library catalog:', error); $('unlock-error').textContent = 'Catalog data is invalid. Check the browser console for details.'; }
      return;
    }
    updateFilters();
    key = candidate;
    try { await saveKey(candidate); }
    catch { toast('Unlocked, but this browser cannot remember the key. Check private browsing or storage settings.'); }
    $('passphrase').value = '';
    renderLibrary();
  } catch (error) {
    console.error('Reader unlock failed outside decryption:', error);
    $('unlock-error').textContent = 'Unable to initialize the reader. Check the browser console for details.';
  }
  finally { button.disabled = false; }
});
$('forget-button').addEventListener('click', async () => {
  if (!confirm('Forget this browser? You will need your passphrase to unlock again.')) return;
  try { await forgetKey(); } catch { toast('Could not remove the saved key. Clear this site’s browser data.'); return; }
  unloadBook();
  key = null; catalog = []; activeId = null; detailBook = null;
  show('unlock');
});
$('library-button').addEventListener('click', () => {
  unloadBook();
  renderLibrary();
});
$('close-details').addEventListener('click', () => { detailBook = null; renderLibrary(); });
$('chapter-sort').addEventListener('change', renderDetailChapters);
window.addEventListener('pagehide', flushProgress);
for (const id of ['book-search', 'publication-filter', 'language-filter', 'tag-filter']) {
  $(id).addEventListener(id === 'book-search' ? 'input' : 'change', renderLibrary);
}
$('settings-button').addEventListener('click', () => openPanel('settings-panel'));
$('chapters-button').addEventListener('click', () => openPanel('chapters-panel'));
$('backdrop').addEventListener('click', closePanels);
document.querySelectorAll('.close-button').forEach(button => button.addEventListener('click', closePanels));
document.addEventListener('keydown', event => {
  if (event.key === 'Escape') closePanels();
  if (event.target.closest('input,select,button') || $('backdrop').hidden === false || !rendition) return;
  if (event.key === 'ArrowRight') navigate('next');
  if (event.key === 'ArrowLeft') navigate('prev');
});
$('prev-button').addEventListener('click', () => navigate('prev'));
$('next-button').addEventListener('click', () => navigate('next'));
for (const [element, property] of [['font', 'font'], ['font-size', 'fontSize'], ['line-spacing', 'lineSpacing'], ['width', 'width']]) {
  $(element).addEventListener('input', event => { settings[property] = event.target.value; applySettings(); });
}
async function changeLayout(flow, alreadySaved = false) {
  if (!alreadySaved) flushProgress();
  const id = activeId;
  const currentLayout = rendition?.manager?.name;
  const currentFlow = rendition?.settings.flow;
  settings.flow = flow;
  applySettings();
  // Rebuild when the flow or manager changes, then reopen the saved CFI.
  if (rendition && id && (currentLayout !== layoutFor(flow).manager || currentFlow !== layoutFor(flow).flow)) {
    await openBook(id, undefined, true);
  }
}
$('flow').addEventListener('change', event => changeLayout(event.target.value));
$('theme').addEventListener('change', event => {
  settings.theme = event.target.value;
  if (presets[settings.theme]) Object.assign(settings, presets[settings.theme]);
  applySettings();
});
for (const [element, property] of [['background', 'background'], ['foreground', 'foreground']]) {
  $(element).addEventListener('input', event => { settings[property] = event.target.value; settings.theme = 'custom'; applySettings(); });
}
$('interface-mode').addEventListener('change', event => {
  if (event.target.value === 'custom' && settings.interfaceMode !== 'custom') {
    settings.interfaceBackground = settings.background;
    settings.interfaceForeground = settings.foreground;
  }
  settings.interfaceMode = event.target.value;
  applySettings();
});
for (const [element, property] of [['interface-background', 'interfaceBackground'], ['interface-foreground', 'interfaceForeground']]) {
  $(element).addEventListener('input', event => { settings[property] = event.target.value; applySettings(); });
}
$('reset-settings').addEventListener('click', () => {
  flushProgress();
  settings = { ...defaults };
  changeLayout(settings.flow, true);
});
window.addEventListener('resize', () => {
  if (document.body.classList.contains('reader-active')) {
    if (matchMedia('(max-width: 600px)').matches) {
      setMobileChrome(document.body.classList.contains('reader-chrome-hidden'));
    } else {
      document.body.classList.remove('reader-chrome-hidden');
    }
  }
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => { if (rendition) rendition.resize(); }, 200);
});
initialize();