import { readFileSync } from 'node:fs';
import { test, expect } from '../../fixtures/index.ts';
import { testFile } from '../../fixtures/paths.ts';
import { LoginPage, TrashPage } from '../../clients/web/index.ts';

// Each test arranges what it needs through the API and drives only the action under test
// through the web client; the API then checks that the server agrees with what is shown.

test('an uploaded file is listed and kept', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  const files = await new LoginPage(page).signInAndWait(alice);

  await files.upload(testFile('hello.txt'));
  await expect(files.row('hello.txt')).toBeVisible();

  await page.reload();
  await expect(files.row('hello.txt')).toBeVisible();
  const nodes = await (await api.as(alice)).rootNodes();
  expect(nodes).toContainEqual(
    expect.objectContaining({ name: 'hello.txt', size: readFileSync(testFile('hello.txt')).length }),
  );
});

test('a new folder is created and kept', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  const files = await new LoginPage(page).signInAndWait(alice);

  await files.newFolder('Projects');

  await page.reload();
  await expect(files.row('Projects')).toBeVisible();
  expect(await (await api.as(alice)).rootNodes()).toContainEqual(
    expect.objectContaining({ name: 'Projects', kind: 'folder' }),
  );
});

test('a file is uploaded into a folder', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const folder = await session.createFolder('Documents');
  const files = await new LoginPage(page).signInAndWait(alice);

  await files.openFolder('Documents');
  await files.upload(testFile('report.txt'));
  await expect(files.row('report.txt')).toBeVisible();

  expect(await session.nodes(folder.id)).toContainEqual(expect.objectContaining({ name: 'report.txt' }));
  expect(await session.rootNodes()).not.toContainEqual(expect.objectContaining({ name: 'report.txt' }));
});

test('a file is moved into another folder', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const archive = await session.createFolder('Archive');
  const year = await session.createFolder('2026', archive.id);
  await session.uploadFile(testFile('report.txt'));
  const files = await new LoginPage(page).signInAndWait(alice);

  await files.move('report.txt', ['Archive', '2026']);

  await files.openFolder('Archive');
  await files.openFolder('2026');
  await expect(files.row('report.txt')).toBeVisible();
  expect(await session.nodes(year.id)).toContainEqual(expect.objectContaining({ name: 'report.txt' }));
  expect(await session.rootNodes()).not.toContainEqual(expect.objectContaining({ name: 'report.txt' }));
});

test('a file and a folder are renamed', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const folder = await session.createFolder('Drafts');
  const file = await session.uploadFile(testFile('report.txt'));
  const files = await new LoginPage(page).signInAndWait(alice);

  await files.rename('report.txt', 'report-final.txt');
  await files.rename('Drafts', 'Published');

  await page.reload();
  await expect(files.row('report-final.txt')).toBeVisible();
  await expect(files.row('Published')).toBeVisible();
  const nodes = await session.rootNodes();
  // Renamed in place: the same nodes, under their new names.
  expect(nodes).toContainEqual(expect.objectContaining({ id: file.id, name: 'report-final.txt' }));
  expect(nodes).toContainEqual(expect.objectContaining({ id: folder.id, name: 'Published' }));
});

test('a deleted file goes to the recycle bin', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const folder = await session.createFolder('Old');
  await session.uploadFile(testFile('report.txt'), folder.id);
  const files = await new LoginPage(page).signInAndWait(alice);

  await files.openFolder('Old');
  await files.delete('report.txt');

  const trash = new TrashPage(page);
  await files.sidebar.open('Recycle bin');
  await expect(trash.row('report.txt')).toContainText('/Old/report.txt');
  expect(await session.nodes(folder.id)).toEqual([]);
  expect(await session.trash()).toContainEqual(
    expect.objectContaining({ node: expect.objectContaining({ name: 'report.txt' }) }),
  );
});

test('a restored file lands in Lost & Found and moves on from there', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const keep = await session.createFolder('Keep');
  const file = await session.uploadFile(testFile('report.txt'));
  const files = await new LoginPage(page).signInAndWait(alice);
  await files.delete('report.txt');

  const trash = new TrashPage(page);
  await files.sidebar.open('Recycle bin');
  await trash.restore('report.txt');
  await expect(trash.empty).toBeVisible();
  expect(await session.trash()).toEqual([]);

  // The success note links to Lost & Found, where restored files go.
  await trash.openLostAndFound();
  await expect(files.row('report.txt')).toBeVisible();
  const lostAndFound = await session.node('Lost & Found');
  expect(await session.nodes(lostAndFound.id)).toContainEqual(
    expect.objectContaining({ id: file.id, name: 'report.txt' }),
  );

  await files.move('report.txt', ['Keep']);

  await files.goHome();
  await files.openFolder('Keep');
  await expect(files.row('report.txt')).toBeVisible();
  expect(await session.nodes(keep.id)).toContainEqual(expect.objectContaining({ id: file.id }));
  expect(await session.nodes(lostAndFound.id)).toEqual([]);
});

test('a file is downloaded from its menu', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  await (await api.as(alice)).uploadFile(testFile('report.txt'));
  const files = await new LoginPage(page).signInAndWait(alice);

  const { name, bytes } = await files.download('report.txt');

  expect(name).toBe('report.txt');
  expect(bytes).toEqual(readFileSync(testFile('report.txt')));
});

test('uploading over a file keeps the old content as a version', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  const original = await session.uploadFile(testFile('report.txt'));
  const files = await new LoginPage(page).signInAndWait(alice);
  const updated = Buffer.from('Quarterly report\nRevenue went up, again.\n');

  await files.upload({ name: 'report.txt', mimeType: 'text/plain', buffer: updated });

  await expect(files.row('report.txt')).toHaveCount(1);
  await expect.poll(async () => (await session.node('report.txt')).version).toBe(2);
  const node = await session.node('report.txt');
  expect(node).toMatchObject({ id: original.id, size: updated.length });
  expect(await session.versions(original.id)).toEqual([
    expect.objectContaining({ version: 1, size: readFileSync(testFile('report.txt')).length }),
  ]);
  const details = await files.openDetails('report.txt');
  await expect(details.versions).toHaveCount(1);
  await expect(details.version(1)).toBeVisible();
  // The listing downloads the newest content.
  expect((await files.download('report.txt')).bytes).toEqual(updated);
});

test('a folder name that is taken is refused, and says why', async ({ page, users, api }) => {
  const alice = await users.create('alice');
  const session = await api.as(alice);
  await session.createFolder('Projects');
  const files = await new LoginPage(page).signInAndWait(alice);

  await files.submitNewFolder('Projects');

  // The name stays in the box to be changed, and no second folder is made.
  const name = page.getByRole('textbox', { name: 'Folder name' });
  await expect(name).toHaveValue('Projects');
  await expect(files.error).toBeVisible();
  // The listing reloads after a failed change; the reason must stay on screen.
  await page.waitForTimeout(2_000);
  await expect(files.error).toBeVisible();
  await expect(files.row('Projects')).toHaveCount(1);
  expect((await session.rootNodes()).filter((node) => node.name === 'Projects')).toHaveLength(1);

  // A free name goes through and clears the problem.
  await name.fill('Projects 2');
  await name.press('Enter');
  await expect(files.row('Projects 2')).toBeVisible();
  await expect(files.error).toBeHidden();
});
