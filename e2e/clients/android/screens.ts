import type { Browser, ChainablePromiseElement } from 'webdriverio';
import type { Account } from '../../fixtures/api.ts';
import { ANDROID_PACKAGE, HOST_FROM_EMULATOR, type Adb } from '../../fixtures/android.ts';

// Screens of the Android app (clients/mobile/androidApp). Elements are found by their Compose
// test tags, which the app exposes as resource ids (testTagsAsResourceId), or by text.

const DEFAULT_WAIT = 20_000;

// XPath rather than UiSelector: right after the app restarts, a UiSelector lookup sometimes
// misses Compose nodes that the hierarchy dump (which XPath searches) already has.

/** The element with Compose test tag `tag`. */
export function tag(tag: string): string {
  return `//*[@resource-id=${xpathString(tag)}]`;
}

/** The element whose text is `text`. */
export function text(text: string): string {
  return `//*[@text=${xpathString(text)}]`;
}

export class AndroidApp {
  constructor(
    readonly driver: Browser,
    readonly adb: Adb,
    readonly serverPort: number,
  ) {}

  /** Starts the app from nothing: no session, no settings, permissions granted again. */
  async reset() {
    await this.driver.terminateApp(ANDROID_PACKAGE);
    await this.adb.shell('pm', 'clear', ANDROID_PACKAGE);
    await this.adb.grantPermissions();
    await this.driver.activateApp(ANDROID_PACKAGE);
  }

  /**
   * Sets a text field's whole value in one step. Clearing and typing races a Compose field,
   * which can put its old value back before the new one is typed.
   */
  async replaceText(field: ChainablePromiseElement | WebdriverIO.Element, value: string, check = true) {
    const element = await field;
    await this.driver.execute('mobile: replaceElementValue', { elementId: element.elementId, text: value });
    if (check) {
      await this.driver.waitUntil(async () => (await element.getText()) === value, {
        timeout: 5_000,
        timeoutMsg: `the field did not take the value "${value}"`,
      });
    }
  }

  get serverUrl() {
    return `http://${HOST_FROM_EMULATOR}:${this.serverPort}`;
  }

  async signIn(account: Pick<Account, 'username' | 'password'>): Promise<SignedIn> {
    const server = await this.driver.$(tag('login.server'));
    await server.waitForDisplayed({ timeout: DEFAULT_WAIT });
    await this.replaceText(server, this.serverUrl);
    // The form for a password appears once the server answered.
    const username = await this.driver.$(tag('login.username'));
    await username.waitForDisplayed({ timeout: DEFAULT_WAIT });
    await this.replaceText(username, account.username);
    await this.replaceText(await this.driver.$(tag('login.password')), account.password, false);
    await this.driver.hideKeyboard().catch(() => {});
    await this.driver.$(tag('login.signIn')).click();
    await this.driver.$(tag('tab.files')).waitForDisplayed({ timeout: DEFAULT_WAIT });
    return new SignedIn(this);
  }
}

export class SignedIn {
  constructor(readonly app: AndroidApp) {}

  async openTransfers(): Promise<TransfersScreen> {
    await this.app.driver.$(tag('tab.transfers')).click();
    return new TransfersScreen(this.app);
  }
}

export class TransfersScreen {
  constructor(readonly app: AndroidApp) {}

  /** The received transfer whose title contains `name`. */
  received(name: string) {
    return this.app.driver.$(
      `//*[@resource-id="transfer.received"][.//*[contains(@text, ${xpathString(name)})]]`,
    );
  }

  async expectReceived(name: string, timeout = 30_000) {
    await this.received(name).waitForDisplayed({
      timeout,
      timeoutMsg: `no received transfer "${name}" on the phone after ${timeout} ms`,
    });
  }

  /** Taps the transfer and picks an action from its sheet: "Download", "Save" or "Decline". */
  async act(name: string, action: 'Download' | 'Save' | 'Decline') {
    await this.received(name).click();
    const item = await this.app.driver.$(text(action));
    await item.waitForDisplayed({ timeout: DEFAULT_WAIT });
    await item.click();
  }
}

function xpathString(value: string): string {
  return value.includes('"') ? `'${value}'` : `"${value}"`;
}
