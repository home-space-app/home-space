import { expect, type Locator } from '@playwright/test';
import { AppPage } from './app-page.ts';

/** A file open in the viewer, a popup over the page it was opened from (`?file=<id>`). */
export class ViewerPage extends AppPage {
  /** The viewer opens over another page; reach it from a listing or a search. */
  protected get path(): string {
    throw new Error('Open the viewer from a listing or a search result');
  }

  get root(): Locator {
    return this.page.getByRole('dialog');
  }

  get title(): Locator {
    return this.root.getByRole('heading', { level: 2 });
  }

  /** The text of a text file. */
  get text(): Locator {
    return this.root.locator('pre.text-viewer');
  }

  /** The image of an image file; its alternative text is the file's name. */
  image(name: string): Locator {
    return this.root.getByRole('img', { name, exact: true });
  }

  async expectLoaded() {
    await expect(this.page).toHaveURL(/[?&]file=[^&]+$/);
    await expect(this.root).toBeVisible();
  }

  async expectFor(name: string) {
    await this.expectLoaded();
    await expect(this.title).toHaveText(name);
  }

  /** Closes the popup with its close button, back to the page under it. */
  async close() {
    await this.root.getByRole('link', { name: 'Close' }).click();
    await expect(this.root).toBeHidden();
  }

  /** Closes the popup with Escape. */
  async dismiss() {
    await this.page.keyboard.press('Escape');
    await expect(this.root).toBeHidden();
  }
}
