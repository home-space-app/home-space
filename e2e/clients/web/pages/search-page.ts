import { expect, type Locator } from '@playwright/test';
import { escapeRegExp } from '../support/regex.ts';
import { AppPage } from './app-page.ts';

/** Search results, `/search?q=…`, across all of the user's files. */
export class SearchPage extends AppPage {
  protected readonly path = '/search';

  get box(): Locator {
    return this.page.getByRole('searchbox', { name: 'Search files and folders' });
  }

  get results(): Locator {
    return this.page.locator('.node-table__row');
  }

  get noMatches(): Locator {
    return this.page.getByText('Nothing matches your search.');
  }

  /** The result for `name`; it also shows where the file is. */
  row(name: string): Locator {
    return this.results.filter({ has: this.page.getByRole('link', { name: new RegExp(`^${escapeRegExp(name)}`) }) });
  }

  async expectLoaded() {
    await expect(this.box).toBeVisible();
    await expect(this.page).toHaveURL(/\/search/);
  }

  /** Types `term` into the search box, from any page that has one, and waits for the results. */
  async search(term: string) {
    await this.box.fill(term);
    await this.box.press('Enter');
    await expect(this.page).toHaveURL(new RegExp(`/search\\?q=${escapeRegExp(encodeURIComponent(term))}`));
  }

  /** Opens the result for `name`. */
  async open(name: string) {
    await this.row(name).getByRole('link').click();
  }
}
