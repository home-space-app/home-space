// Page objects for the web client (clients/web), laid out like its source: a class per page in
// `pages/`, and in `components/` the parts several pages share or that open over a page
// (dialogs, the sidebar, the file table). Pages extend `AppPage`; actions that open a dialog
// return its object. Elements are found the way a user finds them, by role and label; a
// `data-testid` is used only where an element has no accessible name.

export { AppPage } from './pages/app-page.ts';
export { LoginPage } from './pages/login-page.ts';
export { FilesPage } from './pages/files-page.ts';
export { TrashPage } from './pages/trash-page.ts';
export { SharedWithMePage } from './pages/shared-with-me-page.ts';
export { TransfersPage } from './pages/transfers-page.ts';
export { DevicesPage } from './pages/devices-page.ts';
export { AdminUsersPage } from './pages/admin-users-page.ts';
export { SettingsPage } from './pages/settings-page.ts';
export { DuplicatesPage } from './pages/duplicates-page.ts';
export { SearchPage } from './pages/search-page.ts';
export { ViewerPage } from './pages/viewer-page.ts';
export { PublicSharePage } from './pages/public-share-page.ts';
export type { FileContent } from './pages/files-page.ts';

export { Sidebar, type Section } from './components/sidebar.ts';
export { NodeTable } from './components/node-table.ts';
export { MoveDialog } from './components/move-dialog.ts';
export { DeleteDialog } from './components/delete-dialog.ts';
export { SendDialog } from './components/send-dialog.ts';
export { ShareDialog } from './components/share-dialog.ts';
export { DetailsPanel } from './components/details-panel.ts';
export { AddUserDialog, EditUserSheet, DeleteUserDialog } from './components/user-dialogs.ts';

export { stubSavePicker } from './support/save-picker.ts';
export { downloaded } from './support/download.ts';
