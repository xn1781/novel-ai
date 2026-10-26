import openRegular from '@fontsource/open-sans/files/open-sans-latin-400-normal.woff2?url';
import openItalic from '@fontsource/open-sans/files/open-sans-latin-400-italic.woff2?url';
import openBold from '@fontsource/open-sans/files/open-sans-latin-700-normal.woff2?url';
import openBoldItalic from '@fontsource/open-sans/files/open-sans-latin-700-italic.woff2?url';
import merriweatherRegular from '@fontsource/merriweather/files/merriweather-latin-400-normal.woff2?url';
import merriweatherItalic from '@fontsource/merriweather/files/merriweather-latin-400-italic.woff2?url';
import merriweatherBold from '@fontsource/merriweather/files/merriweather-latin-700-normal.woff2?url';
import merriweatherBoldItalic from '@fontsource/merriweather/files/merriweather-latin-700-italic.woff2?url';

// EPUB chapters live in srcdoc/blob iframes, so URLs must be absolute and the
// font faces must be registered inside each chapter, not just on the site page.
export function readerFontCss(baseUrl = document.baseURI) {
  const face = (name, file, weight, style) =>
    `@font-face{font-family:'${name}';src:url('${new URL(file, baseUrl).href}') format('woff2');font-weight:${weight};font-style:${style};font-display:swap}`;
  return [
    face('Open Sans', openRegular, 400, 'normal'),
    face('Open Sans', openItalic, 400, 'italic'),
    face('Open Sans', openBold, 700, 'normal'),
    face('Open Sans', openBoldItalic, 700, 'italic'),
    face('Merriweather', merriweatherRegular, 400, 'normal'),
    face('Merriweather', merriweatherItalic, 400, 'italic'),
    face('Merriweather', merriweatherBold, 700, 'normal'),
    face('Merriweather', merriweatherBoldItalic, 700, 'italic'),
  ].join('\n');
}