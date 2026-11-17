import type { BrowserContext, Page } from '@playwright/test';

interface SavedFiles {
  __e2eSaved?: Record<string, number[]>;
}

/**
 * Replaces `showSaveFilePicker` in every page of `context` with one that saves into
 * `window.__e2eSaved[suggestedName]` once the file is closed. Chromium saves through the File
 * System Access picker, which a test cannot click. Call it before the page loads.
 */
export async function stubSavePicker(context: BrowserContext) {
  await context.addInitScript(() => {
    const saved: Record<string, number[]> = {};
    (window as unknown as SavedFiles).__e2eSaved = saved;
    (window as unknown as { showSaveFilePicker: unknown }).showSaveFilePicker = async (options?: {
      suggestedName?: string;
    }) => {
      const name = options?.suggestedName ?? 'download';
      const chunks: number[] = [];
      return {
        async createWritable() {
          return {
            async write(data: BufferSource | Blob | string) {
              const buffer =
                data instanceof Blob
                  ? await data.arrayBuffer()
                  : typeof data === 'string'
                    ? new TextEncoder().encode(data)
                    : data;
              const view =
                buffer instanceof ArrayBuffer
                  ? new Uint8Array(buffer)
                  : new Uint8Array(buffer.buffer, buffer.byteOffset, buffer.byteLength);
              for (const byte of view) chunks.push(byte);
            },
            async close() {
              saved[name] = chunks;
            },
            async abort() {},
          };
        },
        async remove() {},
      };
    };
  });
}

/** Waits for the stand-in picker to have saved `name`, and returns its bytes. */
export async function savedFile(page: Page, name: string): Promise<Buffer> {
  const handle = await page.waitForFunction(
    (file) => (window as unknown as SavedFiles).__e2eSaved?.[file],
    name,
    { timeout: 30_000 },
  );
  return Buffer.from((await handle.jsonValue()) as number[]);
}
