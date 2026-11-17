import { expect, type Locator, type Page } from '@playwright/test';
import { savedFile } from '../support/save-picker.ts';
import { AppPage } from './app-page.ts';

/** Transfers: files sent to this device, `/transfers`, or by it, `/transfers/sent`. */
export class TransfersPage extends AppPage {
  protected readonly path: string;

  constructor(
    page: Page,
    readonly tab: 'incoming' | 'sent' = 'incoming',
  ) {
    super(page);
    this.path = tab === 'sent' ? '/transfers/sent' : '/transfers';
  }

  async expectLoaded() {
    await expect(this.page.getByRole('heading', { name: 'Transfers' })).toBeVisible();
  }

  row(name: string): Locator {
    return this.page.getByTestId('transfer-row').filter({ hasText: name });
  }

  /**
   * Downloads the incoming transfer `name` and returns the bytes the page saved. The page's
   * context needs `stubSavePicker`, which keeps what is written in memory.
   */
  async download(name: string): Promise<Buffer> {
    await this.row(name).getByRole('button', { name: 'Download' }).click();
    return savedFile(this.page, name);
  }
}
