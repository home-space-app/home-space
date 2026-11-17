import { expect, type Locator, type Page } from '@playwright/test';

/** Asks before files and folders go to the recycle bin. */
export class DeleteDialog {
  readonly root: Locator;

  constructor(readonly page: Page) {
    this.root = page.getByRole('alertdialog');
  }

  async expectFor(name: string) {
    await expect(this.root).toContainText(`Move “${name}” to recycle bin?`);
  }

  async confirm() {
    await this.root.getByRole('button', { name: 'Move to bin' }).click();
    await expect(this.root).toBeHidden();
  }

  async cancel() {
    await this.root.getByRole('button', { name: 'Cancel' }).click();
    await expect(this.root).toBeHidden();
  }
}
