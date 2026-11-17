import { test } from '../../fixtures/android-test.ts';
import { testFile } from '../../fixtures/paths.ts';

test('a folder another user shares shows in the Shared tab and opens', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const bob = await users.create('bob');
  const bobApi = await api.as(bob);
  const folder = await bobApi.createFolder('Holiday');
  await bobApi.uploadFile(testFile('report.txt'), folder.id, 'itinerary.txt');
  const files = await android.signIn(alice);
  const shared = await files.openShared();
  await shared.waitFor(shared.nothingShared, 'the note that nothing is shared');

  // Bob shares while Alice has the tab open.
  await bobApi.shareWith([folder.id], [alice.id]);
  await shared.refresh();

  await shared.expectRow('Holiday');
  await shared.waitFor(shared.detailContaining('Holiday', bob.username), `${bob.username} as the owner`);
  await shared.openFolder('Holiday');
  await shared.expectRow('itinerary.txt');
  const viewer = await shared.openFile('itinerary.txt');
  await viewer.expectLine('Revenue went up.');
});

test('a share with another user is not shown to anyone else', async ({ android, users, api }) => {
  const alice = await users.create('alice');
  const bob = await users.create('bob');
  const carol = await users.create('carol');
  const aliceApi = await api.as(alice);
  const file = await aliceApi.uploadFile(testFile('report.txt'));
  await aliceApi.shareWith([file.id], [bob.id]);

  const files = await android.signIn(carol);
  const shared = await files.openShared();

  await shared.waitFor(shared.nothingShared, 'the note that nothing is shared');
});
