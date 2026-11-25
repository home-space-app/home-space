import type { CDPSession, Page } from '@playwright/test';
import { test, expect } from '../../fixtures/index.ts';
import { FilesPage, LoginPage, SettingsPage } from '../../clients/web/index.ts';

// WebAuthn takes a host name, not an IP address: the web client is loaded from `localhost`.
test.use({ baseURL: async ({ server }, use) => use(server.url.replace('127.0.0.1', 'localhost')) });

/**
 * Chrome's virtual authenticator, standing in for a phone's or computer's passkeys. While
 * `present`, it answers every request at once, also the one the login page leaves waiting in
 * the username field's autofill; the test turns it off to sign in some other way first.
 */
class VirtualAuthenticator {
  private constructor(
    private readonly cdp: CDPSession,
    private readonly id: string,
  ) {}

  static async on(page: Page): Promise<VirtualAuthenticator> {
    const cdp = await page.context().newCDPSession(page);
    await cdp.send('WebAuthn.enable');
    const { authenticatorId } = await cdp.send('WebAuthn.addVirtualAuthenticator', {
      options: {
        protocol: 'ctap2',
        transport: 'internal',
        hasResidentKey: true,
        hasUserVerification: true,
        isUserVerified: true,
        automaticPresenceSimulation: true,
      },
    });
    return new VirtualAuthenticator(cdp, authenticatorId);
  }

  async present(enabled: boolean) {
    await this.cdp.send('WebAuthn.setAutomaticPresenceSimulation', {
      authenticatorId: this.id,
      enabled,
    });
  }
}

function securityCard(page: Page) {
  return page.locator('section.card', {
    has: page.getByRole('heading', { name: 'Two-factor sign-in and passkeys' }),
  });
}

test('a passkey added in the settings signs in on its own and after the password', async ({
  page,
  users,
}) => {
  const authenticator = await VirtualAuthenticator.on(page);
  const alice = await users.create('passkey');
  const files = await new LoginPage(page).signInAndWait(alice);

  const settings = new SettingsPage(page);
  await settings.goto();
  const card = securityCard(page);
  await card.getByRole('button', { name: 'Add a passkey' }).click();
  await expect(card.getByText('Save these recovery codes')).toBeVisible();
  await expect(card.locator('.recovery-codes li')).toHaveCount(10);
  await card.getByRole('button', { name: 'I saved them' }).click();
  await expect(card.locator('.key-list__item')).toHaveCount(1);
  await expect(card.locator('.key-list__name')).toHaveText('Passkey');

  // On its own: no username, no password. The autofill's request is withdrawn for the
  // button's, which the authenticator answers once it is back.
  await authenticator.present(false);
  await files.sidebar.logOut();
  await expect(page).toHaveURL(/\/login$/);
  await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
  await authenticator.present(true);
  await new FilesPage(page).expectLoaded();
  await expect(files.sidebar.accountName).toHaveText(alice.username);

  // After the password, as the second step.
  await authenticator.present(false);
  await files.sidebar.logOut();
  const login = new LoginPage(page);
  await login.signIn(alice);
  await expect(
    page.getByText('Use your passkey or security key to finish signing in.'),
  ).toBeVisible();
  await authenticator.present(true);
  await page.getByRole('button', { name: 'Use passkey or security key' }).click();
  await new FilesPage(page).expectLoaded();
});
