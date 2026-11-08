// Renders every html/*.html page to screens/<name>-light.png and -dark.png at 2x, 1440px wide.
// Fonts come from node_modules, so it works offline. The web client uses the system UI font;
// the screenshots pin it to Noto Sans so they look the same on every machine.
// Usage: node render.js ../html ../screens
const { chromium } = require('playwright');
const fs = require('fs'), path = require('path');
const dir = process.argv[2], out = process.argv[3];
const F = p => fs.readFileSync(path.join(__dirname, 'node_modules', p));
const css = `
@font-face{font-family:'Material Symbols Outlined';font-style:normal;font-weight:100 700;font-display:block;src:url(https://local.fonts/ms.woff2) format('woff2')}
`;
const fonts = `
@font-face{font-family:'Noto Sans';font-weight:400;src:url(https://local.fonts/n400.woff2) format('woff2')}
@font-face{font-family:'Noto Sans';font-weight:500;src:url(https://local.fonts/n500.woff2) format('woff2')}
@font-face{font-family:'Noto Sans';font-weight:600;src:url(https://local.fonts/n600.woff2) format('woff2')}
@font-face{font-family:'Noto Sans';font-weight:700;src:url(https://local.fonts/n700.woff2) format('woff2')}
.hs{font-family:'Noto Sans',sans-serif!important}
`;
const files = {'ms.woff2':'material-symbols/material-symbols-outlined.woff2','n400.woff2':'@fontsource/noto-sans/files/noto-sans-latin-400-normal.woff2','n500.woff2':'@fontsource/noto-sans/files/noto-sans-latin-500-normal.woff2','n600.woff2':'@fontsource/noto-sans/files/noto-sans-latin-600-normal.woff2','n700.woff2':'@fontsource/noto-sans/files/noto-sans-latin-700-normal.woff2'};
(async () => {
  fs.mkdirSync(out, { recursive: true });
  const browser = await chromium.launch();
  const page = await browser.newPage({ deviceScaleFactor: 2, viewport: { width: 1440, height: 900 } });
  await page.route('https://fonts.googleapis.com/**', r => r.fulfill({ contentType: 'text/css', body: css }));
  await page.route('https://local.fonts/*', r => r.fulfill({ contentType: 'font/woff2', body: F(files[path.basename(r.request().url())]) }));
  for (const f of fs.readdirSync(dir).filter(f => f.endsWith('.html'))) {
    for (const theme of ['light', 'dark']) {
      await page.goto('file://' + path.join(dir, f));
      await page.addStyleTag({ content: fonts });
      if (theme === 'dark') await page.evaluate(() => document.querySelector('.hs').classList.add('dark'));
      await page.evaluate(() => document.fonts.ready);
      await page.waitForTimeout(150);
      const el = await page.$('body > div');
      await el.screenshot({ path: path.join(out, f.replace('.html', `-${theme}.png`)) });
    }
  }
  await browser.close();
})();
