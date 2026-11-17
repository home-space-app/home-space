import { expect, type Locator, type Page } from '@playwright/test';

/** The dialog that picks a folder to move files and folders to. It opens at Home. */
export class MoveDialog {
  readonly root: Locator;

  constructor(readonly page: Page) {
    this.root = page.getByRole('dialog');
  }

  async expectFor(name: string) {
    await expect(this.root.getByRole('heading', { name: `Move “${name}”` })).toBeVisible();
  }

  /** Opens the folders of `path` one after another, from Home. */
  async open(...path: string[]) {
    for (const folder of path) {
      await this.root.getByRole('button', { name: folder, exact: true }).click();
    }
    await expect(this.root).toContainText(path.length ? `To /${path.join('/')}` : 'To Home');
  }

  async confirm() {
    await this.root.getByRole('button', { name: 'Move here' }).click();
    await expect(this.root).toBeHidden();
  }

  /** Moves to the folder reached through `path` from Home. */
  async moveTo(path: string[]) {
    await this.open(...path);
    await this.confirm();
  }
}
