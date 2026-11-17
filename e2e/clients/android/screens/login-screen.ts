import type { Account } from '../../../fixtures/api.ts';
import { tag, text } from '../support/locators.ts';
import { DEFAULT_WAIT, Screen } from './screen.ts';
import { FilesScreen } from './files-screen.ts';

/** The sign-in screen: the server's address, then a username and password. */
export class LoginScreen extends Screen {
  readonly server = tag('login.server');
  readonly username = tag('login.username');
  readonly password = tag('login.password');
  readonly signInButton = tag('login.signIn');

  /** The reason a sign-in failed. */
  problem(message: string) {
    return text(message);
  }

  async expectShown(timeout = DEFAULT_WAIT) {
    await this.waitFor(this.server, 'the sign-in screen', timeout);
  }

  /** Fills the form and taps Sign in, without waiting for the outcome. */
  async submit(account: Pick<Account, 'username' | 'password'>) {
    await this.expectShown();
    await this.app.replaceText(this.find(this.server), this.app.serverUrl);
    // The form for a password appears once the server answered.
    await this.waitFor(this.username, 'the username field');
    await this.app.replaceText(this.find(this.username), account.username);
    await this.app.replaceText(this.find(this.password), account.password, false);
    await this.driver.hideKeyboard().catch(() => {});
    await this.tap(this.signInButton, 'the Sign in button');
  }

  async signIn(account: Pick<Account, 'username' | 'password'>): Promise<FilesScreen> {
    await this.submit(account);
    await this.waitFor(tag('tab.files'), 'the Files tab');
    return new FilesScreen(this.app);
  }
}
