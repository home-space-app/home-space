# Built-in file viewers

The web, desktop and Android clients show some files themselves instead of downloading them or
handing them to another app. Which viewer shows a file depends on its MIME type, which the server
stores (it guesses the type from the file name when the uploader sends none).

There are two viewers:

- The **text viewer** handles `text/*` and the text-like `application/*` types in
  `client_core::viewers::TEXT_LIKE_TYPES` (JSON, XML, YAML, JavaScript, shell scripts, TOML, SQL and
  others), up to 2 MiB.
- The **image viewer** handles the image types in `client_core::viewers::IMAGE_TYPES` (PNG, JPEG,
  GIF, WebP and BMP), plus the types each client can decode itself, up to 50 MiB.

## How a file opens

| Client | Opening a file with a viewer | The previous behaviour |
|---|---|---|
| Web | The file name links to `/files/{id}/view`. Search results link there too. | The row menu still has **Download**. |
| Desktop | Double-click, Enter, Space, the toolbar's Open and the details panel's Open show the viewer in the main window. Back, Escape or Backspace return to the folder. | The context menu has **Open with system app**. |
| Android | A tap shows the viewer full screen. The back arrow returns to the list. | The row menu has **Open in another app** and **Edit in another app**. |

Files with no viewer, or larger than the viewer's limit, open the way they did before.

## The text viewer

- A file always opens read-only. On desktop and Android the text can still be selected and copied.
- **Edit** turns the view into a plain editor. There is no highlighting or other editing help.
- **Cancel** drops the edits and shows the file again.
- **Save** stores the text as the file's new content. The server keeps the previous content as a
  version, so it can still be downloaded or restored from the file's details.
- The text keeps the file's encoding details. A UTF-8 byte order mark stays, and a file whose lines
  all end in CRLF is saved with CRLF (`client_core::text::TextDoc`).
- Content that is not valid UTF-8 is shown with replacement characters and cannot be edited, since
  saving would change the bytes that could not be decoded.
- When another device changes the file while it is shown, web and desktop reload it. During
  editing, they show a notice instead and keep the edits. Saving then makes the edits current, and
  the other change stays as a version.
- Android asks before discarding edits on Back. Desktop disables Back while editing.

## The image viewer

- It only shows the picture, scaled down to fit. There is no editing.
- When the image's folder holds other images, the viewer steps through them in the order the folder
  lists them. Files of other types are skipped.
  - **Space** or **→** shows the next image, and **←** the previous one.
  - The header also has previous and next buttons, and a counter such as `3 / 12`.
  - On Android the images can be swiped. The keys work there with a hardware keyboard.
- Stepping stops at the first and last image, and the button for that direction is disabled there.
- An image opened from search results steps through its own folder.
- On the web, stepping replaces the page in the history, so the browser's Back returns to the
  folder instead of walking back through every image.
- Uploads, renames and deletes in the folder update the images to step through while the viewer is
  open (web and desktop).

Saving uses `PUT /api/v1/files/{id}/content` when the content fits in one upload chunk, and a
resumable upload with `node_id` set when it does not (see [chunked-upload.md](chunked-upload.md)).
The shared helper is `api_client::Client::replace_with_bytes`; the web client has its own in
`clients/web/src/upload.rs`.

## Adding a viewer

The registry is one table in `clients/client-core/src/viewers.rs`. To add a viewer:

1. Add a variant to `ViewerKind` and a `Registration` to `Viewers::shared` with its MIME patterns
   (`image/*` or an exact type), whether it is `Editable` or `ReadOnly`, and its size limit. The first
   registration that matches wins. If the viewer steps between the files of a folder, return `true`
   for it from `ViewerKind::navigates`. `client_core::gallery::Gallery` then does the stepping.
2. Web: add a component in `clients/web/src/pages/viewer/` and its arm in the `match viewer.kind` of
   `ViewerPage` (`mod.rs`).
3. Desktop: add a module in `clients/desktop/src/browser/viewer/`, a `Body` variant, and its arm
   in `State::update` (`mod.rs`).
4. Android: add the variant to `ViewerKind` in `clients/mobile-core/src/viewers.rs`, then a state class
   and screen in `ViewerScreen.kt` with their arms in `OpenViewer.of` and `ViewerScreen`.

Every `match` on `ViewerKind` is exhaustive, so the compiler points at each place a new kind needs.

## Client-specific types

`Viewers::shared()` holds only the types every client can show. A client that can show more adds
them to a viewer with `with_types`. The added types use that viewer's editing mode and size limit, and
the shared types are matched first. Each client keeps one instance and looks every file up in it:

| Client | Where | Types added |
|---|---|---|
| Web | `clients/web/src/viewers.rs` | `image/svg+xml`, `image/avif`, `image/x-icon`, `image/vnd.microsoft.icon` |
| Desktop | `clients/desktop/src/viewers.rs` | `image/tiff`, `image/x-icon`, `image/vnd.microsoft.icon` |
| Android | `registerPlatformImageTypes` in `ImageViewer.kt`, through `add_viewer_types` | `image/heic` and `image/heif` on Android 9+, `image/avif` on Android 12+ |

A type can be added from what the client finds at runtime. For example, desktop could check for a
RAW codec on the system when it starts and then add `image/x-canon-cr2` and similar types to the
image viewer.
