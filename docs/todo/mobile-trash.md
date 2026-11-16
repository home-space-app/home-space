# Recycle bin on mobile

**Status (2026-11-16):** all four steps are done. In Files, Recover stays greyed out (see the File
Provider section).

## Problem

Deleting on Android and iOS moves files to the recycle bin, and the delete dialog says "You can
restore them from the recycle bin". Neither app can open the bin, so the only way back is desktop
or web. This is the last "No" row for both phones in `mobile-feature-parity.md`.

## How the bin works (server, unchanged)

- `GET /trash`: the binned **files**, most recently deleted first. A deleted folder is not listed
  as a folder: each of its files is listed on its own, with the path it had (`original_path`).
- `POST /trash/restore` `{ids, on_conflict}`: restored files go **directly into Lost & Found**,
  not back to where they were. `abort` reports clashes with files already in Lost & Found;
  `override` makes the restored file the current version; `skip` leaves the clashing ones in the
  bin. The answer names the Lost & Found folder (`target`).
- `DELETE /trash/{id}` deletes one file for good, and `DELETE /trash` empties the bin.
- Each change sends `TrashChanged` on the websocket; mobile-core already maps it to
  `LiveEvent.TrashChanged`.
- Nothing empties the bin by itself, so the screens never say files are removed after N days.

Home already shows Lost & Found once it holds something, so restored files can be found.

## Design

The designs are in Penpot, in "Home Space Client", on the Android and iOS pages under the
"Recycle bin" section:

- **Android:** Settings entry, list, row actions, selection, delete forever, overflow menu,
  restored, and empty.
- **iOS:** Settings entry, list, context menu, swipe actions, select (with nothing selected and
  with 2 selected), delete forever, restored, and empty.

Name clashes reuse the existing move conflict screens, so they have no board of their own. Both apps use the same words as web and desktop: "Recycle bin",
"Restore", "Delete forever", "Empty recycle bin", "Restored files go to Lost & Found".

**Where it opens:** a plain "Recycle bin" row in Settings, in a new "Files" section above Sync,
with no file count. The tab bar is full, and the bin is not a place to
browse every day. Lost & Found stays in Home as it is now.

**The list:** one row per binned file, with the file's icon and its name. The second line is
the folder it came from ("Home" for the top level), the date it was deleted, and its size, for
example `/Docs/Taxes · 14 Nov 2026 · 412.3 KB`. Rows are not opened: a binned file can't be previewed.
A line at the top says that restored files go to Lost & Found.

**Android (Material 3):**
- A top app bar with Back, the title "Recycle bin", and an overflow menu holding
  "Empty recycle bin". The menu item is disabled while the bin is empty.
- Tapping a row opens a bottom sheet with "Restore" and "Delete forever", the latter in the
  error colour.
- A long press starts selection mode, the same as in Files. The contextual bar shows
  "N selected" with Select all, and the bottom bar offers Restore and Delete forever.
- After a restore, a snackbar says "Restored 2 files to Lost & Found", with the action "Show".
- Delete forever and Empty ask first, in a dialog: "Permanently delete 12 files?" /
  "This can't be undone." / Cancel, Delete.
- The empty state shows an icon, "The recycle bin is empty", and "Deleted files stay here until
  you delete them forever."

**iOS (HIG):**
- A large title "Recycle Bin", pushed from Settings, with Select in the navigation bar.
- Swipe actions: leading Restore (tint) and trailing Delete (red, destructive). A long press opens
  a context menu with Restore and Delete Forever (destructive).
- Select mode, as in the Files app's Recently Deleted: the bottom toolbar offers "Restore All" and
  "Delete All" while nothing is selected, and "Restore" and "Delete" once something is.
- Deleting forever asks first, in an alert like the existing delete one: "Delete 2 Files
  Forever?" / "This can't be undone." / Cancel, Delete (destructive). This includes a swipe.
- After a restore, a banner at the bottom of the list says "Restored 2 files to Lost & Found",
  with a "Show" button that opens Lost & Found in the Files tab. It goes by itself after 5 seconds.
  (The app's notice is an alert, too heavy for a confirmation.)
- The empty state is a `ContentUnavailableView` with a trash icon, "No Deleted Files", and
  "Deleted files stay here until you delete them forever."

**Name clashes when restoring:** the existing move conflict sheet or dialog is reused, titled
"2 files already exist in Lost & Found". The choices are the move sheet's own, "Override"
(the restored file becomes the current version), "Skip these files" (they stay in the bin) and
Cancel.

**Live:** the list reloads on `TrashChanged` and `Resync`, and when the screen appears.

## Implementation

### mobile-core (`clients/mobile-core`)

New file `src/trash.rs`, exported on `CoreClient`:

- `trash() -> Vec<TrashEntry>`. A `TrashEntry` has `id`, `name`, `mime`, `size`, `folder` (the
  original path without the name, `/` for Home), `deleted_at` (an RFC 3339 string, as `Entry`
  has), and `detail` (the second line, built with `client_core::format`).
- `restore_trashed(ids, mode: MoveMode) -> RestoreResult`. A `RestoreResult` has `restored` (a
  count), `folder_id` (Lost & Found), `conflicts_title` and `conflicts: Vec<MoveConflict>`. It
  reuses the move types, so both apps reuse their conflict UI. Its `message` is "Restored 2
  files to Lost & Found".
- `delete_forever(ids)`: `DELETE /trash/{id}` for each id, stopping at the first failure. The
  server has no batch purge, and binned lists are short.
- `empty_trash()`.
- The api-client calls exist already: `trash`, `restore_trashed`, `purge_trashed` and
  `empty_trash`.
- Restoring makes Lost & Found non-empty, so the Home listing's hidden-folder rule
  (`lost_and_found`) shows it on the next list with no change.
- Tests, with `wiremock` as in `lib.rs`:
  - mapping and `detail`
  - restore with conflicts
  - restore that skips
  - `delete_forever` stopping at a failure
  - `empty_trash`
  - `Unauthorized` passed through

Then rebuild the bindings: `scripts/build-ios.sh debug` and the Android Gradle task.

### Android (`clients/mobile/androidApp`)

- `SubPage.TRASH`, opened from a new Settings "Files" section, like Offline files.
- New `TrashScreen.kt`:
  - the list, the row sheet and the selection mode, reusing `EntryIcon` and the `FileDialogs`
    selection bar styles
  - the conflict dialog from `FileDialogs.kt`, factored out of the move dialog so both use it
  - the snackbar's Show action opens Lost & Found in Files
- `TrashModel`: holds the list, the selection and the busy state. It reloads on `TrashChanged`
  and `Resync` from the existing events flow. `Unauthorized` ends the session as elsewhere.

### iOS (`clients/mobile/iosApp`)

- A "Files" section in `SettingsView` with a `NavigationLink` to the new `TrashView`.
- New `TrashView.swift`:
  - a `List` with `.swipeActions` and `.contextMenu`, `EditMode` for Select, and a bottom toolbar
  - `.confirmationDialog` for deleting forever
  - `ContentUnavailableView` for the empty state
- The conflict list in `FileDialogs.swift` (`MoveSheet.conflictList`) moves into a reusable
  `ConflictList`, which the move sheet and the bin both show.
- `SessionModel`: reloads on `.trashChanged`, as for `devicesChanged` (a `trashGeneration`
  counter). "Show" switches to the Files tab and opens Lost & Found through `AppRouter.openFolder`.

### File Provider (iOS, follow-up)

Declare a trash container (`.trashContainer`) backed by the same calls:
- enumerate it with `trash()`
- an item moved to it is a delete, and one moved out of it is a restore
- an item deleted from it is `delete_forever`

Files then offers an undoable delete instead of "Delete Now". Restoring from Files lands in Lost &
Found, so the item reports Lost & Found as its new parent. This needs the same mobile-core calls,
so it comes after the apps.

**Done:** deleting in Files moves to the bin; Recently Deleted lists the bin, labelled Home Space;
Delete Now there deletes for good (`FilesTrashUITests`). Binned items allow reading, as Files dims
unreadable ones; opening one fails with "feature unsupported", since the server sends no content
for binned files. **Not working:** Files keeps Recover greyed out for these items, whatever
capabilities they carry and whether or not the trash was listed again after the delete, so the
restore path in `modifyItem` is unreached. Restore from the app's Recycle Bin.

## Tests

- **mobile-core:** the unit tests above.
- **Android**: the app has no instrumented tests yet, so this was checked by hand on an emulator
  against the dev server: restore with Show, a clash with Override, selection, Delete forever
  and Empty.
- **iOS** `TrashUITests`, the same flow, plus Empty with the confirmation.
- **Specs:** a new rule in `docs/specs/mobile-files.feature`, "The recycle bin lists deleted files
  and restores them to Lost & Found".
- **Docs:** `mobile-feature-parity.md` changes the Trash row to Yes, and the README gets a short
  section on the bin.

## Order

1. mobile-core calls and tests.
2. Android screen, then iOS screen. They are independent once step 1 is in.
3. Specs, the parity table and the README.
4. The File Provider trash container.

## Decisions

- Settings shows a plain row, with no file count, so opening Settings costs no `GET /trash`.
- Restored files go to Lost & Found, as the server does today.

## Later

- Restore to the original folder when it still exists, falling back to Lost & Found. This is a
  server change that web, desktop and both phones would follow, so it gets its own plan.
