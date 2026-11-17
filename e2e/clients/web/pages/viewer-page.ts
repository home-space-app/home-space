import { expect, type Locator } from '@playwright/test';
import { AppPage } from './app-page.ts';

/** A file opened in the viewer, `/files/<id>/view`: text or an image. */
export class ViewerPage extends AppPage {
  /** The viewer's path holds the file's short id; reach it from a listing or a search. */
  protected get path(): string {
    throw new Error('Open the viewer from a listing or a search result');
  }

  get title(): Locator {
    return this.page.getByRole('heading', { level: 1 });
  }

  /** The text of a text file. */
  get text(): Locator {
    return this.page.locator('pre.text-viewer');
  }

  /** The image of an image file; its alternative text is the file's name. */
  image(name: string): Locator {
    return this.page.getByRole('img', { name, exact: true });
  }

  async expectLoaded() {
    await expect(this.page).toHaveURL(/\/files\/[^/]+\/view$/);
  }

  async expectFor(name: string) {
    await this.expectLoaded();
    await expect(this.title).toHaveText(name);
  }

  async back() {
    await this.page.getByRole('link', { name: 'Back' }).click();
  }
}
