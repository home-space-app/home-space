# Mobile feature parity

## Problem

Desktop and web have most features. Android and iOS now also search, create folders, upload
picked files, share, send to devices, restore from the recycle bin, and update live while open. iOS has a share
extension and a File Provider extension. Each extension signs in with a session of its own, which the
server starts for the app (`/auth/device-sessions`) and the app keeps in a Keychain group and App
Group it shares with its extensions. Status
per client, as of 2026-11-16. Desktop, web and Android were checked in source; iOS was also run
against a dev server (`iosAppUITests/ParityUITests`).

| Feature | Desktop | Web | Android | iOS |
| --- | --- | --- | --- | --- |
| Sign in with password | Yes | Yes | Yes | Yes |
| Sign in with SSO | Yes | Yes | Yes | Yes |
| Stay signed in between launches | Yes | Yes | Yes | Yes |
| Sign out | Yes | Yes | Yes | Yes |
| Register this device | Yes | Yes | Yes | Yes |
| Device list (rename, sign out devices) | Yes | Yes | Partial: list only | Partial: list only |
| Browse folders | Yes | Yes | Yes | Yes |
| Breadcrumb or parent navigation | Yes | Yes | Yes | Partial: back navigation only |
| Sort by name, size and date | Yes | Yes | Partial: folders first, then name | Partial: folders first, then name |
| Select several items | Yes | Yes | Yes | Yes |
| Details panel (created, edited on, device) | Yes | Yes | Yes | Yes |
| Live updates while browsing | Yes | Yes | Yes | Yes |
| Create folder | Yes | Yes | Yes | Yes |
| Rename | Yes | Yes | Yes | Yes |
| Move, with conflict choice | Yes | Yes | Yes | Yes |
| Delete to bin | Yes | Yes | Yes | Yes |
| Trash: list, restore, delete for good, empty | Yes | Yes | Yes | Yes |
| Search | Yes | Yes | Yes | Yes |
| Share with links or users | Yes | Yes | Yes | Yes |
| Shared with me | Yes | Yes | Yes | Yes |
| Send to device | Yes | Yes | Yes | Yes |
| Receive transfers (open link, save, download, decline) | Yes | Yes | Yes | Yes |
| Notifications for received transfers | Partial: in the app only | Partial: in the app only | Yes | Yes: local, while open or in background refresh |
| Built-in text and image viewers, text editing | Yes | Yes | Yes | Yes |
| Share files from other apps into Home Space | n/a | n/a | Yes | Yes (`ShareExtension`) |
| Home Space in the system file picker | n/a | n/a | Yes | Yes (`FileProviderExtension`): also rename, move and delete |
| Upload files | Yes, also whole folders | Yes, not whole folders (Send takes folders) | Yes, not whole folders | Yes, not whole folders |
| Drag and drop to upload | Yes | Yes | n/a | n/a |
| Resumable uploads | Yes | Yes | Yes | Yes |
| Download a file | Yes | Yes | Yes | Yes |
| Save to a chosen location | Yes | Partial: the browser picks the folder | Yes | Yes |
| Keep offline copies on the device | Yes | Yes | Yes | Yes |
| Progress for transfers | Yes | Yes | Yes | Yes |
| Folder sync from this device | Yes | No | Auto-upload one way, offline folders both ways | Auto-upload one way, offline folders both ways |
| Quick upload of camera photos | No | No | Yes | Yes |
| Transfers continue with the app closed | n/a | n/a | Auto-upload: yes | Auto-upload: when iOS allows |
| Own storage usage | No | Yes (settings, sidebar) | No | No |
| Change password | No | Yes | No | No |
| Administration (server status, users, quotas) | No | Yes, for admins | No | No |

Sources: `clients/desktop/src/browser`, `clients/desktop/src/transfers.rs`, `clients/sync-core`,
`clients/web/src/upload.rs`, `clients/web/src/pages/browser`, `clients/mobile/androidApp`,
`clients/mobile/iosApp`, `clients/mobile-core/src/lib.rs`, `clients/mobile-core/src/events.rs`,
`clients/web/src/pages/settings.rs`, `clients/web/src/pages/admin.rs`.

## Scope

Close the gaps for Android and iOS that the desktop and web clients already cover. The
`mobile-core` crate already exposes the browse, rename, move, delete and download calls. New
features need the calls added there, so both apps get them. Sharing and folder sync have their own
write-ups and are only listed here.

## Proposal

Work in this order:

1. ~~**Trash**~~: done; see `mobile-trash.md`.
2. ~~**Live updates**~~: done; `mobile-core/src/events.rs`, opened while the app is in front.
3. **Medium items**: a device list screen, a sort choice, and an iOS breadcrumb trail.

Each step adds `mobile-core` calls first, then Android and iOS screens. Keep parity by checking this
table after each step.

## Open questions

- Should picking a whole folder to upload be added? Desktop has it; auto-upload covers the ongoing case.
