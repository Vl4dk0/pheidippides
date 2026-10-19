import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
export default defineConfig(({ command, mode }) => ({
  plugins: [
    svelte(),
    // A separate development entry keeps fixtures and IPC mocks out of every build.
    ...(command === 'serve' && mode === 'screenshots'
      ? [
          {
            name: 'screenshot-entry',
            transformIndexHtml: (html: string) =>
              html.replace('/src/main.ts', '/scripts/screenshot-mock.ts'),
          },
        ]
      : []),
  ],
  clearScreen: false,
  test: { include: ['src/**/*.test.ts'] },
}));
