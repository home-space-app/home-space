import type { Page } from '@playwright/test';
import { Sidebar } from '../components/sidebar.ts';

/** A page of the signed-in app: it has the sidebar, and a path of its own to go to. */
export abstract class AppPage {
  readonly sidebar: Sidebar;

  constructor(readonly page: Page) {
    this.sidebar = new Sidebar(page);
  }

  /** The client-side path of the page, e.g. `/trash`. */
  protected abstract readonly path: string;

  async goto() {
    await this.page.goto(this.path);
    await this.expectLoaded();
  }

  /** Waits until the page shows what makes it this page. */
  abstract expectLoaded(): Promise<void>;
}
