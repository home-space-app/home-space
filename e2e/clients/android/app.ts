import type { Browser, ChainablePromiseElement } from 'webdriverio';
import type { Account } from '../../fixtures/api.ts';
import { ANDROID_PACKAGE, HOST_FROM_EMULATOR, type Adb } from '../../fixtures/android.ts';
import { LoginScreen } from './screens/login-screen.ts';
import type { FilesScreen } from './screens/files-screen.ts';

/** The Android app (clients/mobile/androidApp) on the device under test. */
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

  /** Stops the app and starts it again, keeping its data: the session must survive. */
  async restart() {
    await this.driver.terminateApp(ANDROID_PACKAGE);
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

  /** The test server, as the emulator reaches it. */
  get serverUrl() {
    return `http://${HOST_FROM_EMULATOR}:${this.serverPort}`;
  }

  get login(): LoginScreen {
    return new LoginScreen(this);
  }

  /** Signs in to the test server and waits for the Files tab. */
  signIn(account: Pick<Account, 'username' | 'password'>): Promise<FilesScreen> {
    return this.login.signIn(account);
  }
}
