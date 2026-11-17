import { text } from '../support/locators.ts';
import { Screen } from './screen.ts';
import { LoginScreen } from './login-screen.ts';

/** The profile, opened from the avatar on the Files tab: the account, its devices, Sign out. */
export class ProfileScreen extends Screen {
  readonly signOutButton = text('Sign out');
  /** The note on this phone's row in Devices. */
  readonly thisPhone = text('This phone · Android');

  async expectShown() {
    await this.waitFor(text('Profile'), 'the profile');
  }

  /** The account's name, as the profile shows it. */
  username(name: string) {
    return text(name);
  }

  async signOut(): Promise<LoginScreen> {
    await this.tap(this.signOutButton, 'the Sign out button');
    const login = new LoginScreen(this.app);
    await login.expectShown();
    return login;
  }
}
