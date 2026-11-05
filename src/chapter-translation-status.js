// A display-only heuristic: saved text is not proof that the model translated it.
// Ignore an isolated source-language heading or character name in otherwise English text.
export function translationStatus(section, sourceLanguage) {
  const translated = section.translated;
  if (translated == null || !translated.trim()) return "No saved translation";

  const script = sourceLanguage === "Korean" ? /[\uac00-\ud7a3]/gu
    : sourceLanguage === "Japanese" ? /[\u3040-\u30ff\u3400-\u9fff]/gu : null;
  if (!script) return "Saved translation";

  const sourceCount = (section.text.match(script) || []).length;
  const translatedCount = (translated.match(script) || []).length;
  if (sourceCount >= 3 && section.text.trim() === translated.trim()) {
    return "Likely untranslated (saved text)";
  }
  const letters = (translated.match(/\p{L}/gu) || []).length;
  if (sourceCount >= 8 && translatedCount >= 8 && translatedCount / Math.max(letters, 1) >= 0.35) {
    return "Likely untranslated (saved text)";
  }
  return "Saved translation";
}

export function sourceLanguageTitle(title, sourceLanguage) {
  if (sourceLanguage === "Korean") return /[\uac00-\ud7a3]/u.test(title);
  if (sourceLanguage === "Japanese") return /[\u3040-\u30ff\u3400-\u9fff]/u.test(title);
  return false;
}