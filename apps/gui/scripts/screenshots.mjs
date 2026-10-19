import { existsSync } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { createServer } from 'vite';
import { webkit } from 'playwright';

const root = fileURLToPath(new URL('..', import.meta.url));
process.chdir(root);
if (!existsSync(webkit.executablePath())) {
  execFileSync(
    process.execPath,
    [
      fileURLToPath(
        new URL('cli.js', import.meta.resolve('playwright/package.json')),
      ),
      'install',
      'webkit',
    ],
    { stdio: 'inherit' },
  );
}
const started = performance.now();
const output = new URL('../screenshots/', import.meta.url);
await mkdir(output, { recursive: true });
const server = await createServer({
  root,
  mode: 'screenshots',
  // The injected entry is invisible to Vite's HTML scan; optimize its extra import up front.
  optimizeDeps: { include: ['@tauri-apps/api/mocks'] },
  server: { host: '127.0.0.1', port: 0, open: false, hmr: false },
});
let browser;
const failures = [];
const files = [];
async function cleanup() {
  try {
    await browser?.close();
  } finally {
    await server.close();
  }
}
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.once(signal, () => {
    void cleanup().finally(() => process.exit(1));
  });
}
try {
  await server.listen();
  const address = server.httpServer.address();
  if (!address || typeof address === 'string')
    throw new Error('Vite did not bind a local TCP port');
  const url = `http://127.0.0.1:${address.port}`;
  browser = await webkit.launch({ headless: true });
  for (const viewport of [
    { width: 1440, height: 900 },
    { width: 1024, height: 700 },
  ]) {
    for (const colorScheme of ['light', 'dark']) {
      const context = await browser.newContext({
        viewport,
        colorScheme,
        locale: 'en-GB',
        timezoneId: 'UTC',
        reducedMotion: 'reduce',
      });
      const page = await context.newPage();
      page.setDefaultTimeout(5000);
      page.on('pageerror', (error) => failures.push(error.message));
      page.on('console', (message) => {
        if (message.type() === 'error') failures.push(message.text());
      });
      await page.route('**/*', (route) => {
        const target = new URL(route.request().url());
        return target.origin === url || target.protocol === 'data:'
          ? route.continue()
          : route.abort();
      });
      async function capture(view) {
        await page.evaluate(async () => {
          await document.fonts.ready;
          for (const image of document.images) image.loading = 'eager';
        });
        await page.waitForFunction(() =>
          Array.from(document.images).every(
            (image) => image.complete && image.naturalWidth > 0,
          ),
        );
        if (await page.getByRole('alert').count())
          throw new Error(await page.getByRole('alert').allTextContents());
        const file = `${view}-${colorScheme}-${viewport.width}x${viewport.height}.png`;
        await page.screenshot({
          path: fileURLToPath(new URL(file, output)),
          animations: 'disabled',
        });
        files.push(file);
      }
      await page.goto(url);
      await page
        .getByRole('button', {
          name: 'Encrypted relay ready for review',
          exact: false,
        })
        .click();
      await page
        .getByRole('region', { name: 'Thread summary', exact: true })
        .waitFor();
      await capture('main');
      // Reload restores the unread fixture, including the thread just read above.
      await page.reload();
      await page.getByRole('combobox').selectOption('unread');
      await page
        .getByRole('button', {
          name: 'Encrypted relay ready for review',
          exact: false,
        })
        .waitFor();
      await capture('unread');
      await page
        .getByRole('button', { name: 'Space settings', exact: true })
        .click();
      await page
        .getByRole('region', { name: 'Space settings', exact: true })
        .waitFor();
      await capture('settings');
      await page
        .getByRole('region', { name: 'Space settings', exact: true })
        .getByRole('button', { name: 'Close', exact: true })
        .click();
      await page.getByRole('button', { name: 'Connect', exact: true }).click();
      await page
        .getByRole('region', { name: 'Connect a space', exact: true })
        .waitFor();
      await capture('connect');
      await page
        .getByRole('region', { name: 'Connect a space', exact: true })
        .getByRole('button', { name: 'Close', exact: true })
        .click();
      await page.getByRole('combobox').selectOption('all');
      await page
        .getByRole('button', {
          name: 'apps https://phi.example.com',
          exact: true,
        })
        .click();
      await page
        .getByText('No threads in this view.', { exact: true })
        .waitFor();
      await capture('empty');
      await context.close();
    }
  }
  if (failures.length) throw new Error(failures.join('\n'));
} finally {
  await cleanup();
}
console.log(files.map((file) => `screenshots/${file}`).join('\n'));
console.log(
  `Captured ${files.length} WebKit screenshots in ${((performance.now() - started) / 1000).toFixed(1)}s`,
);
