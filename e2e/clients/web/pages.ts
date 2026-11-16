import { expect, type BrowserContext, type Locator, type Page } from '@playwright/test';
import type { Account } from '../../fixtures/api.ts';

// Page objects for the web client (clients/web). They find elements the way a user does, by
// role and label; a `data-testid` is used only where an element has no accessible name.

export class LoginPage {
  constructor(readonly page: Page) {}

  async goto() {
    await this.page.goto('/login');
  }

  async signIn(account: Pick<Account, 'username' | 'password'>) {
    await this.goto();
    await this.page.getByLabel('Username').fill(account.username);
    await this.page.getByLabel('Password').fill(account.password);
    await this.page.getByRole('button', { name: 'Sign in', exact: true }).click();
  }

  /** Signs in and waits for the file browser. */
  async signInAndWait(account: Pick<Account, 'username' | 'password'>): Promise<BrowserPage> {
    await this.signIn(account);
    const browser = new BrowserPage(this.page);
    await browser.expectLoaded();
    return browser;
  }

  get error(): Locator {
    return this.page.getByRole('alert');
  }
}

export class BrowserPage {
  constructor(readonly page: Page) {}

  async expectLoaded() {
    await expect(this.page.getByRole('button', { name: 'New folder' })).toBeVisible();
  }

  async upload(...files: string[]) {
    await this.page.locator('label[aria-label="Upload files"] input[type=file]').setInputFiles(files);
  }

  row(name: string): Locator {
    return this.page.locator('.node-table__row').filter({ hasText: name });
  }

  async openSendDialog(): Promise<SendDialog> {
    await this.page.getByRole('button', { name: 'Send files from this computer to a device' }).click();
    const dialog = new SendDialog(this.page);
    await expect(dialog.root).toBeVisible();
    return dialog;
  }

  async logOut() {
    await this.page.locator('button.account').click();
    await this.page.getByRole('menuitem', { name: 'Log out' }).click();
  }
}

/** The "Send to a device" dialog, for files from this computer. */
export class SendDialog {
  readonly root: Locator;

  constructor(readonly page: Page) {
    this.root = page.getByRole('dialog');
  }

  async addFiles(...files: string[]) {
    await this.root.locator('label.send-link', { hasText: 'Add files' }).locator('input[type=file]').setInputFiles(files);
  }

  /** Picks the device named `name` as the recipient. */
  async chooseDevice(name: string) {
    const option = this.root.getByRole('radio', { name: new RegExp(escape(name)) });
    if (!(await option.isVisible())) {
      // The picker folds away once a device is chosen; the "To" row opens it again.
      await this.root.getByTestId('send-target').click();
    }
    await option.check();
  }

  async send(files: string[], deviceName: string) {
    await this.addFiles(...files);
    await this.chooseDevice(deviceName);
    await this.root.getByRole('button', { name: new RegExp(`^Send to ${escape(deviceName)}`) }).click();
    await expect(this.root).toBeHidden();
  }
}

export class TransfersPage {
  constructor(readonly page: Page) {}

  async goto() {
    await this.page.goto('/transfers');
  }

  row(name: string): Locator {
    return this.page.getByTestId('transfer-row').filter({ hasText: name });
  }

  /**
   * Downloads the incoming transfer `name` and returns the bytes the page saved. Chromium
   * saves through the File System Access picker, which a test cannot click, so the page gets
   * a stand-in picker (see `stubSavePicker`) that keeps what is written in memory.
   */
  async download(name: string): Promise<Buffer> {
    await this.row(name).getByRole('button', { name: 'Download' }).click();
    const handle = await this.page.waitForFunction(
      (file) => (window as unknown as SavedFiles).__e2eSaved?.[file],
      name,
      { timeout: 30_000 },
    );
    const bytes = (await handle.jsonValue()) as number[];
    return Buffer.from(bytes);
  }
}

interface SavedFiles {
  __e2eSaved?: Record<string, number[]>;
}

/**
 * Replaces `showSaveFilePicker` in every page of `context` with one that saves into
 * `window.__e2eSaved[suggestedName]` once the file is closed. Call it before the page loads.
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

function escape(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
