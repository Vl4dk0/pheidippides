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
        const layoutIssue = await page.evaluate(() => {
          if (document.documentElement.scrollWidth > innerWidth)
            return 'Workspace overflows horizontally';
          const dialog = document.querySelector('dialog[open]');
          if (dialog) {
            const overlay = dialog.getBoundingClientRect();
            if (
              overlay.x !== 0 ||
              overlay.y !== 0 ||
              overlay.width !== innerWidth ||
              overlay.height !== innerHeight
            )
              return 'Modal backdrop does not cover the full window';
            const bounds = dialog
              .querySelector('.modal')
              .getBoundingClientRect();
            if (
              bounds.x < 0 ||
              bounds.y < 0 ||
              bounds.right > innerWidth ||
              bounds.bottom > innerHeight
            )
              return 'Dialog extends outside viewport';
            if (!dialog.contains(document.activeElement))
              return 'Focus escaped the modal';
          }
          return '';
        });
        if (layoutIssue) throw new Error(`${view}: ${layoutIssue}`);
        const file = `${view}-${colorScheme}-${viewport.width}x${viewport.height}.png`;
        await page.screenshot({
          path: fileURLToPath(new URL(file, output)),
          animations: 'disabled',
        });
        files.push(file);
      }
      await page.clock.setFixedTime(new Date('2026-10-20T09:30:00Z'));
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
      async function expectAtBottom() {
        await page.waitForFunction(() => {
          const viewport = document.querySelector(
            '.thread-history .thread-scroll',
          );
          return (
            viewport &&
            viewport.scrollHeight -
              viewport.clientHeight -
              viewport.scrollTop <=
              1
          );
        });
      }
      async function incomingMessage() {
        const count = await page.locator('.message').count();
        await page.evaluate(() =>
          window.dispatchEvent(new Event('screenshot:incoming')),
        );
        await page.waitForFunction(
          (count) => document.querySelectorAll('.message').length === count + 1,
          count,
        );
      }
      await expectAtBottom();
      const summaryPlacement = await page
        .locator('.summary-card')
        .evaluate((summary) => {
          const composer = document.querySelector('.composer');
          return (
            !summary.closest('.thread-scroll') &&
            summary.nextElementSibling === composer &&
            summary.getBoundingClientRect().bottom <=
              composer.getBoundingClientRect().top
          );
        });
      if (!summaryPlacement)
        throw new Error('Summary is not directly above the composer');
      const reactionGap = await page
        .locator('.reaction-bar')
        .first()
        .evaluate((row) => {
          const buttons = Array.from(row.querySelectorAll('button')).filter(
            (button) => button.getClientRects().length,
          );
          const first = buttons[0].getBoundingClientRect();
          const second = buttons[1].getBoundingClientRect();
          return (
            second.x - first.right - parseFloat(getComputedStyle(row).columnGap)
          );
        });
      if (Math.abs(reactionGap) > 1)
        throw new Error('Active reactions have a ghost gap');
      await capture('main');
      const quietReaction = page
        .locator('.message')
        .last()
        .getByRole('button', {
          name: 'Like',
          exact: true,
          includeHidden: true,
        });
      if (
        (await quietReaction.evaluate((el) => getComputedStyle(el).display)) !==
        'none'
      )
        throw new Error('Empty reaction is visible at rest');
      const quietMessage = page.locator('.message').last();
      const restingBounds = await quietMessage.boundingBox();
      const bottomSpace = await quietMessage.evaluate(
        (card) =>
          card.getBoundingClientRect().bottom -
          card.querySelector('.body').getBoundingClientRect().bottom,
      );
      if (bottomSpace > 30)
        throw new Error('Empty reactions reserve card space');
      await quietMessage.hover();
      if (
        JSON.stringify(restingBounds) !==
        JSON.stringify(await quietMessage.boundingBox())
      )
        throw new Error('Hover reactions moved or resized the message');
      if (
        !(await quietReaction.evaluate((el) => el.getClientRects().length > 0))
      )
        throw new Error('Hover did not reveal reactions');
      await capture('main-hover');
      await page.mouse.move(0, 0);
      await page
        .getByRole('toolbar', { name: 'Reactions to message 5', exact: true })
        .focus();
      await page.keyboard.press('Tab');
      if (
        !(await quietReaction.evaluate((el) => el === document.activeElement))
      )
        throw new Error('Keyboard could not reach an empty reaction');
      if (
        !(await quietReaction.evaluate((el) => el.getClientRects().length > 0))
      )
        throw new Error('Keyboard focus did not reveal reactions');
      await page
        .getByRole('button', { name: 'Hide thread list', exact: true })
        .click();
      await page
        .getByRole('button', { name: 'Show thread list', exact: true })
        .click();
      await page.locator('.summary-card summary').click();
      if (
        (await page.locator('.summary-card details').getAttribute('open')) !==
        null
      )
        throw new Error('Summary did not collapse');
      await page.locator('.summary-card summary').click();
      await expectAtBottom();
      await incomingMessage();
      await expectAtBottom();
      // Within one message of the bottom still follows incoming replies.
      await page
        .locator('.thread-history .thread-scroll')
        .evaluate((viewport) => {
          viewport.scrollTop =
            viewport.scrollHeight - viewport.clientHeight - 40;
        });
      await page.waitForFunction(() => {
        const viewport = document.querySelector(
          '.thread-history .thread-scroll',
        );
        const gap =
          viewport.scrollHeight - viewport.clientHeight - viewport.scrollTop;
        return gap >= 39 && gap <= 41;
      });
      await incomingMessage();
      await expectAtBottom();
      await page
        .locator('.thread-history .thread-scroll')
        .evaluate((viewport) => {
          viewport.scrollTop = 0;
        });
      const jump = page.getByRole('button', { name: /^Jump to newest/ });
      await jump.waitFor();
      const historyPosition = await page
        .locator('.thread-history .thread-scroll')
        .evaluate((viewport) => viewport.scrollTop);
      await incomingMessage();
      await page
        .getByRole('button', {
          name: 'Jump to newest · 1 new message',
          exact: true,
        })
        .waitFor();
      if (
        Math.abs(
          (await page
            .locator('.thread-history .thread-scroll')
            .evaluate((viewport) => viewport.scrollTop)) - historyPosition,
        ) > 1
      )
        throw new Error('Incoming reply pulled the reader out of history');
      await capture('history');
      await jump.click();
      await expectAtBottom();
      if (await jump.count())
        throw new Error('Jump button remained at the bottom');
      await page
        .getByRole('button', { name: 'CLI release checklist', exact: false })
        .click();
      await page
        .getByRole('heading', { name: 'CLI release checklist', exact: true })
        .waitFor();
      if (
        await page
          .getByRole('button', { name: 'Delete thread', exact: true })
          .count()
      )
        throw new Error('Non-author thread showed Delete thread button');
      await expectAtBottom();
      await page
        .getByRole('button', {
          name: 'Encrypted relay ready for review',
          exact: false,
        })
        .click();
      await page
        .getByRole('heading', {
          name: 'Encrypted relay ready for review',
          exact: true,
        })
        .waitFor();
      await expectAtBottom();
      await page
        .getByRole('button', { name: 'Close thread', exact: true })
        .click();
      await page
        .getByRole('button', { name: 'Reopen thread', exact: true })
        .waitFor();
      await page
        .getByRole('button', { name: 'Closed threads', exact: true })
        .click();
      await page
        .getByRole('button', {
          name: 'Encrypted relay ready for review',
          exact: false,
        })
        .waitFor();
      await capture('closed');
      await page
        .getByRole('button', { name: 'Reopen thread', exact: true })
        .click();
      await page
        .getByRole('button', { name: 'Close thread', exact: true })
        .waitFor();
      await page
        .getByRole('button', { name: 'All threads', exact: true })
        .click();
      await page
        .getByRole('button', { name: 'Delete thread', exact: true })
        .click();
      await page
        .getByRole('region', { name: 'Delete thread', exact: true })
        .waitFor();
      await page.keyboard.press('Escape');
      await page
        .getByRole('button', { name: 'Space settings', exact: true })
        .click();
      await page.getByRole('button', { name: 'View', exact: true }).click();
      for (const decoration of [
        'summary',
        'reactions',
        'attachments',
        'metadata',
      ])
        await page
          .getByRole('checkbox', { name: decoration, exact: true })
          .uncheck();
      if (
        await page
          .locator('.summary-card, .reaction-bar, .attachments, .message-meta')
          .count()
      )
        throw new Error('Hidden decorations remain visible');
      for (const decoration of [
        'summary',
        'reactions',
        'attachments',
        'metadata',
      ])
        await page
          .getByRole('checkbox', { name: decoration, exact: true })
          .check();
      for (const panel of ['spaces', 'threads', 'Thread view']) {
        await page
          .getByRole('checkbox', { name: panel, exact: true })
          .uncheck();
        await page.getByRole('checkbox', { name: panel, exact: true }).check();
      }
      await page.keyboard.press('Escape');
      const message = page.getByRole('textbox', {
        name: 'Message',
        exact: true,
      });
      await message.fill(
        Array(20).fill('A longer human perspective.').join('\n'),
      );
      if ((await message.boundingBox()).height > 161)
        throw new Error('Composer expanded beyond its height limit');
      await message.fill('');
      // Reload restores the unread fixture, including the thread just read above.
      await page.reload();
      await page
        .getByRole('button', { name: 'Unread threads', exact: true })
        .click();
      await page
        .getByRole('button', {
          name: 'Encrypted relay ready for review',
          exact: false,
        })
        .waitFor();
      await capture('unread');
      const workspaceBounds = await page.locator('.workspace').boundingBox();
      await page
        .getByRole('button', { name: 'Space settings', exact: true })
        .click();
      await page
        .getByRole('region', { name: 'Space settings', exact: true })
        .waitFor();
      if (
        JSON.stringify(workspaceBounds) !==
        JSON.stringify(await page.locator('.workspace').boundingBox())
      )
        throw new Error('Opening settings moved the workspace');
      if (
        (await page.evaluate(() =>
          document.activeElement?.getAttribute('aria-label'),
        )) !== 'General'
      )
        throw new Error('Settings did not focus the active tab');
      await capture('settings');
      await page.getByLabel('Display name', { exact: true }).fill('Vladko J.');
      await page.getByLabel('Retention · days', { exact: true }).fill('30');
      await page.getByLabel('Summary interval', { exact: true }).fill('4');
      await page
        .getByRole('button', { name: 'Save changes', exact: true })
        .click();
      await page.waitForFunction(
        () =>
          !document.querySelector('button[aria-label="Save changes"]').disabled,
      );
      await page.keyboard.press('Escape');
      await page
        .getByRole('button', { name: 'Space settings', exact: true })
        .click();
      if (
        (await page
          .getByLabel('Display name', { exact: true })
          .inputValue()) !== 'Vladko J.' ||
        (await page
          .getByLabel('Retention · days', { exact: true })
          .inputValue()) !== '30' ||
        (await page
          .getByLabel('Summary interval', { exact: true })
          .inputValue()) !== '4'
      )
        throw new Error(
          'Single Save did not persist both profile and space settings',
        );
      await page.getByLabel('Display name', { exact: true }).fill('Vladko');
      await page.getByLabel('Retention · days', { exact: true }).fill('14');
      await page.getByLabel('Summary interval', { exact: true }).fill('3');
      await page
        .getByRole('button', { name: 'Save changes', exact: true })
        .click();
      await page.waitForFunction(
        () =>
          !document.querySelector('button[aria-label="Save changes"]').disabled,
      );
      for (const section of ['Members', 'Devices', 'Recovery', 'View']) {
        await page.getByRole('button', { name: section, exact: true }).click();
        if (section === 'Devices') {
          const selfRow = page
            .locator('.plain-list > li')
            .filter({ hasText: 'this device' });
          if (
            (await selfRow.getByRole('button').count()) ||
            !/\s· this device/.test(await selfRow.textContent())
          )
            throw new Error(
              'Current device has a revoke action or a missing separator space',
            );
          if (
            (await page
              .getByRole('button', { name: 'Revoke device', exact: true })
              .count()) !== 1
          )
            throw new Error('Other own devices lost their revoke action');
        }
        if (section === 'View') {
          const columns = await page
            .locator('.view-options')
            .evaluate((options) => {
              const labels = options.querySelectorAll('label');
              const first = labels[0].getBoundingClientRect();
              const second = labels[1].getBoundingClientRect();
              return (
                labels.length === 8 &&
                !options.querySelector('svg') &&
                first.y === second.y &&
                first.x !== second.x
              );
            });
          const matchingCheckboxes = await page
            .locator('.view-options')
            .evaluate((options) => {
              const checked = options.querySelector('input:checked');
              const unchecked = options.querySelector('input:not(:checked)');
              const first = getComputedStyle(checked);
              const second = getComputedStyle(unchecked);
              return (
                ['width', 'height', 'borderRadius', 'appearance'].every(
                  (key) => first[key] === second[key],
                ) && first.appearance === 'none'
              );
            });
          if (!matchingCheckboxes)
            throw new Error(
              'Checked and unchecked controls have different shapes',
            );
          if (!columns)
            throw new Error(
              'View options are not a consistent two-column grid',
            );
        }
        await capture(`settings-${section.toLowerCase()}`);
      }
      await page.getByRole('button', { name: 'General', exact: true }).click();
      await page
        .getByRole('region', { name: 'Space settings', exact: true })
        .getByRole('button', { name: 'Close', exact: true })
        .click();
      await page.getByRole('button', { name: 'Connect', exact: true }).click();
      await page
        .getByRole('region', { name: 'Connect a space', exact: true })
        .waitFor();
      if (
        (await page.evaluate(() => document.activeElement?.tagName)) !== 'INPUT'
      )
        throw new Error('Connect did not focus its first input');
      await capture('connect');
      await page
        .getByRole('button', { name: 'Create space', exact: true })
        .click();
      if (
        (await page.evaluate(() => document.activeElement?.tagName)) !== 'INPUT'
      )
        throw new Error('Create did not focus its first input');
      await capture('create');
      await page.keyboard.press('Escape');
      if (await page.getByRole('dialog').count())
        throw new Error('Escape did not close the connection dialog');
      await page
        .getByRole('button', { name: 'All threads', exact: true })
        .click();
      await page
        .getByRole('button', {
          name: 'Open apps',
          exact: true,
        })
        .click();
      await page
        .getByText('No threads in this view.', { exact: true })
        .waitFor();
      await capture('empty');
      await page
        .getByRole('button', { name: 'New thread', exact: true })
        .click();
      const draft = await page.locator('.thread-draft').boundingBox();
      const heading = await page.locator('.thread-draft h2').boundingBox();
      const body = await page
        .getByRole('textbox', { name: 'Message', exact: true })
        .boundingBox();
      if (body.y - heading.y > 250 || draft.width > 640)
        throw new Error('New-thread fields are detached from their heading');
      const textAlignment = await page
        .locator('.thread-draft')
        .evaluate((form) => {
          function contentLeft(field) {
            const style = getComputedStyle(field);
            return (
              field.getBoundingClientRect().left +
              parseFloat(style.paddingLeft) +
              parseFloat(style.borderLeftWidth)
            );
          }
          return (
            contentLeft(form.querySelector('textarea')) -
            contentLeft(form.querySelector('input'))
          );
        });
      if (Math.abs(textAlignment) > 1)
        throw new Error('Draft body text is indented relative to its inputs');
      await capture('compose');
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
