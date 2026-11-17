import { expect, type Locator, type Page } from '@playwright/test';

/** The "Share" dialog of files and folders in the user's own files. */
export class ShareDialog {
  readonly root: Locator;

  constructor(readonly page: Page) {
    this.root = page.getByRole('dialog');
  }

  /** The people the share is drafted for, the owner first. */
  get people(): Locator {
    return this.root.locator('.share__people');
  }

  async addPerson(username: string) {
    await this.root.getByRole('tab', { name: /People/ }).click();
    await this.root.getByRole('button', { name: 'Add people' }).click();
    // The picker is a popover, outside the dialog in the accessibility tree.
    const picker = this.page.locator('#share-people');
    await picker.getByRole('searchbox', { name: 'Search people' }).fill(username);
    await picker.getByRole('checkbox', { name: username }).check();
    await this.page.keyboard.press('Escape');
    await expect(this.people).toContainText(username);
  }

  /** The link of a link share, once it is saved. */
  get link(): Locator {
    return this.root.getByRole('textbox', { name: 'Link' });
  }

  /** Makes a link anyone can open, without a password, and returns it. */
  async createLink(): Promise<string> {
    await this.root.getByRole('tab', { name: /Anyone with the link|Link/ }).click();
    await this.root.getByRole('button', { name: 'Save' }).click();
    // Saving a new link keeps the dialog open, to show the link.
    await expect(this.link).toHaveValue(/\/s\/[^/?]+$/);
    return this.link.inputValue();
  }

  /** Stops the share shown, after confirming. */
  async stopSharing() {
    await this.root.getByRole('button', { name: 'Stop sharing' }).click();
    await this.root.locator('.share__confirm-stop').click();
    await expect(this.root).toBeHidden();
  }

  async close() {
    await this.root.getByRole('button', { name: /^(Close|Cancel)$/ }).click();
    await expect(this.root).toBeHidden();
  }

  async save() {
    await this.root.getByRole('button', { name: 'Save' }).click();
    await expect(this.root).toBeHidden();
  }

  /** Shares, read-only, with the people named `usernames` on the server. */
  async shareWith(...usernames: string[]) {
    for (const username of usernames) {
      await this.addPerson(username);
    }
    await this.save();
  }
}
