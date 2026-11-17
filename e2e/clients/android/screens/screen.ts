import type { AndroidApp } from '../app.ts';
import { label, tag } from '../support/locators.ts';

/** How long to wait for something the app shows, by default. */
export const DEFAULT_WAIT = 20_000;

/**
 * What every screen object has: the app, and waits that fail with a message saying what was
 * missing. Selectors are XPath (see support/locators.ts) and must match one element.
 */
export abstract class Screen {
  constructor(readonly app: AndroidApp) {}

  get driver() {
    return this.app.driver;
  }

  protected find(selector: string) {
    return this.driver.$(selector);
  }

  /** Whether `selector` is on screen now; doesn't wait. */
  async isShown(selector: string): Promise<boolean> {
    const element = this.find(selector);
    return (await element.isExisting()) && (await element.isDisplayed());
  }

  /** Waits until `selector` is on screen. */
  async waitFor(selector: string, what = selector, timeout = DEFAULT_WAIT) {
    await this.find(selector).waitForDisplayed({
      timeout,
      timeoutMsg: `${what} is not on screen after ${timeout} ms`,
    });
  }

  /** Waits until `selector` is off screen. */
  async waitGone(selector: string, what = selector, timeout = DEFAULT_WAIT) {
    await this.find(selector).waitForDisplayed({
      timeout,
      reverse: true,
      timeoutMsg: `${what} is still on screen after ${timeout} ms`,
    });
  }

  /** Waits for `selector`, then taps it. */
  async tap(selector: string, what = selector) {
    await this.waitFor(selector, what);
    await this.find(selector).click();
  }

  /** The text of `selector`, once it is on screen. */
  async textOf(selector: string, what = selector): Promise<string> {
    await this.waitFor(selector, what);
    return this.find(selector).getText();
  }

  /** Taps the top bar's back arrow. */
  async back() {
    await this.tap(label('Back'), 'the Back button');
  }
}

/** A screen with the navigation bar: Files, Transfers, Shared and Settings. */
export abstract class TabScreen extends Screen {
  async openFiles() {
    const { FilesScreen } = await import('./files-screen.ts');
    await this.tap(tag('tab.files'), 'the Files tab');
    return new FilesScreen(this.app);
  }

  async openTransfers() {
    const { TransfersScreen } = await import('./transfers-screen.ts');
    await this.tap(tag('tab.transfers'), 'the Transfers tab');
    return new TransfersScreen(this.app);
  }

  async openShared() {
    const { SharedScreen } = await import('./shared-screen.ts');
    await this.tap(tag('tab.shared'), 'the Shared tab');
    return new SharedScreen(this.app);
  }

  async openSettings() {
    const { SettingsScreen } = await import('./settings-screen.ts');
    await this.tap(tag('tab.settings'), 'the Settings tab');
    return new SettingsScreen(this.app);
  }
}
