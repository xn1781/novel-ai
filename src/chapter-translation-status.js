// Inspect saved output, not the source. Any Korean or Japanese script is flagged
// for review, including names and isolated headings, as requested.
export function translationStatus(section) {
  const translated = section.translated;
  if (translated == null || !translated.trim()) return "No saved translation";
  return /[\u1100-\u11ff\u3130-\u318f\uac00-\ud7a3\ua960-\ua97f\ud7b0-\ud7ff\u3040-\u30ff\u3400-\u9fff]/u.test(translated)
    ? "Contains Korean/Japanese (saved text)" : "No Korean/Japanese detected";
}

export function chapterOptionLabel(section, index, sourceLanguage) {
  const title = sourceLanguageTitle(section.title, sourceLanguage) ? "English title not saved" : section.title;
  return `${index + 1}. ${title} — ${translationStatus(section)}`;
}

export function sourceLanguageTitle(title, sourceLanguage) {
  if (sourceLanguage === "Korean") return /[\uac00-\ud7a3]/u.test(title);
  if (sourceLanguage === "Japanese") return /[\u3040-\u30ff\u3400-\u9fff]/u.test(title);
  return false;
}