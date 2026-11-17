import { test, expect } from '../../fixtures/android-test.ts';
import { tag } from '../../clients/android/index.ts';

test('a wrong password is refused, and says why', async ({ android, users }) => {
  const alice = await users.create('alice');
  const login = android.login;

  await login.submit({ username: alice.username, password: 'not-the-password' });

  await login.waitFor(login.problem('Invalid username or password'), 'the reason the sign-in failed');
  expect(await login.isShown(tag('tab.files'))).toBe(false);
  // The right password still works after a wrong one.
  await login.signIn(alice);
});

test('the session survives a restart, and signing out ends it', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const files = await android.signIn(alice);

  await android.restart();
  const profile = await files.openProfile();
  await profile.waitFor(profile.username(alice.username), 'the username on the profile');
  await profile.waitFor(profile.thisPhone, 'this phone among the devices');
  const phones = (await (await api.as(alice)).devices()).filter((device) => device.platform === 'android');
  expect(phones).toHaveLength(1);

  const login = await profile.signOut();

  await android.restart();
  await login.expectShown();
});
