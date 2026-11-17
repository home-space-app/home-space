import { readFile } from 'node:fs/promises';
import type { Download, Page } from '@playwright/test';

/** Runs `start`, which makes the browser download a file, and returns that download's bytes. */
export async function downloaded(page: Page, start: () => Promise<void>): Promise<{ name: string; bytes: Buffer }> {
  const [download] = await Promise.all([page.waitForEvent('download'), start()]);
  return { name: download.suggestedFilename(), bytes: await bytesOf(download) };
}

async function bytesOf(download: Download) {
  const path = await download.path();
  return readFile(path);
}
