import { randomUUID } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import path from 'node:path';

/** A user the tests created, with the password to sign in with. */
export interface Account {
  id: string;
  username: string;
  password: string;
}

/** The device a client registers as when it signs in (`DeviceInfo` in shared/protocol). */
export interface DeviceInfo {
  id: string;
  name: string;
  platform: string;
}

export interface Device {
  id: string;
  name: string;
  platform: string;
  signed_in: boolean;
  online: boolean;
  current: boolean;
}

export type TransferState =
  | 'uploading'
  | 'ready'
  | 'declined'
  | 'downloaded'
  | 'saved'
  | 'cancelled'
  | 'expired';

export interface TransferSummary {
  id: string;
  sender: { id: string; username: string };
  recipient: { id: string; username: string };
  recipient_device: string;
  recipient_device_name: string;
  state: TransferState;
  total_bytes: number;
  item_count: number;
  name?: string;
}

export interface Node {
  id: string;
  name: string;
  kind: string;
  size?: number;
  mime?: string;
  version?: number;
  parent_id?: string | null;
}

export interface FileVersion {
  version: number;
  size: number;
}

export interface Share {
  id: string;
  /** The secret of a link share, `/s/<token>`; absent for a share with users. */
  token?: string;
  items: Node[];
}

export interface TrashItem {
  node: Node;
  original_path: string;
}

/** Who can see a device and send files to it (`DeviceVisibility` in shared/protocol). */
export type DeviceVisibility = 'owner' | 'hidden' | 'everyone' | 'users';

export interface ReceivedShare {
  id: string;
  owner: { id: string; username: string };
  items: Node[];
}

export interface AdminAccount {
  id: string;
  username: string;
  is_admin: boolean;
  disabled: boolean;
  delete_at?: string | null;
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly body: string,
    what: string,
  ) {
    super(`${what}: HTTP ${status} ${body}`);
  }
}

/**
 * The server's REST API, for arranging a test and checking its outcome without a UI. Each
 * `Session` is one signed-in client; tokens are cached, since password hashing is slow and
 * the server only hashes a few at a time.
 */
export class Api {
  private readonly sessions = new Map<string, Session>();

  constructor(readonly baseURL: string) {}

  async login(account: Pick<Account, 'username' | 'password'>, device?: DeviceInfo): Promise<Session> {
    const response = await fetch(`${this.baseURL}/api/v1/auth/login`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ username: account.username, password: account.password, device }),
    });
    if (!response.ok) {
      throw new ApiError(response.status, await response.text(), `login as ${account.username}`);
    }
    const tokens = (await response.json()) as { access_token: string };
    return new Session(this.baseURL, tokens.access_token, account.username);
  }

  /** A session for `account` on a device of its own ("e2e-api"), reused across calls. */
  async as(account: Pick<Account, 'username' | 'password'>): Promise<Session> {
    let session = this.sessions.get(account.username);
    if (!session) {
      session = await this.login(account, { id: randomUUID(), name: 'e2e-api', platform: 'cli' });
      this.sessions.set(account.username, session);
    }
    return session;
  }
}

export class Session {
  constructor(
    readonly baseURL: string,
    readonly token: string,
    readonly username: string,
  ) {}

  async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const response = await fetch(`${this.baseURL}/api/v1${path}`, {
      method,
      headers: {
        authorization: `Bearer ${this.token}`,
        ...(body === undefined ? {} : { 'content-type': 'application/json' }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await response.text();
    if (!response.ok) {
      throw new ApiError(response.status, text, `${method} ${path} as ${this.username}`);
    }
    return (text ? JSON.parse(text) : undefined) as T;
  }

  get<T>(path: string) {
    return this.request<T>('GET', path);
  }

  /** `POST /admin/users`; needs an administrator's session. */
  createUser(username: string, password: string, isAdmin = false) {
    return this.request<{ id: string; username: string }>('POST', '/admin/users', {
      username,
      password,
      is_admin: isAdmin,
    });
  }

  devices() {
    return this.get<Device[]>('/devices');
  }

  /**
   * The one device on `platform` that isn't this session's, renamed to `name` so a UI can pick
   * it out (clients name themselves after the browser or phone model, which repeat).
   */
  async claimDevice(platform: string | RegExp, name: string): Promise<Device> {
    const matches = (await this.devices()).filter(
      (device) =>
        !device.current &&
        (typeof platform === 'string' ? device.platform === platform : platform.test(device.platform)),
    );
    if (matches.length !== 1) {
      throw new Error(
        `expected one ${platform} device for ${this.username}, found ${JSON.stringify(matches)}`,
      );
    }
    await this.request('PATCH', `/devices/${matches[0].id}`, { name });
    return { ...matches[0], name };
  }

  incomingTransfers() {
    return this.get<TransferSummary[]>('/transfers/incoming');
  }

  transfer(id: string) {
    return this.get<TransferSummary>(`/transfers/${id}`);
  }

  outgoingTransfers() {
    return this.get<TransferSummary[]>('/transfers/outgoing');
  }

  /** The files and folders in the user's root folder. */
  rootNodes() {
    return this.get<Node[]>('/nodes');
  }

  /** The files and folders in folder `parentId`, or in the root folder. */
  nodes(parentId?: string) {
    return this.get<Node[]>(parentId ? `/nodes?parent_id=${parentId}` : '/nodes');
  }

  /** The node called `name` in folder `parentId` (the root when unset); throws when missing. */
  async node(name: string, parentId?: string): Promise<Node> {
    const found = (await this.nodes(parentId)).find((node) => node.name === name);
    if (!found) {
      throw new Error(`${name} is not in ${parentId ?? 'the root folder'} of ${this.username}`);
    }
    return found;
  }

  createFolder(name: string, parentId?: string) {
    return this.request<Node>('POST', '/folders', { name, parent_id: parentId ?? null });
  }

  /**
   * Uploads the file at `file` into folder `parentId`, or the root folder. The type comes from
   * the extension, as a browser would send it, so the web client offers its viewers.
   */
  async uploadFile(file: string, parentId?: string, name = path.basename(file)) {
    return this.uploadContent(name, await readFile(file), parentId);
  }

  /**
   * Uploads `bytes` as file `name` into folder `parentId`, or the root folder. Over a file of
   * that name, the upload is its new content and the old content becomes a version.
   */
  async uploadContent(name: string, bytes: Buffer, parentId?: string) {
    await this.upload(
      { name, size: bytes.length, mime: mimeOf(name), ...(parentId ? { parent_id: parentId } : {}) },
      bytes,
    );
    return this.node(name, parentId);
  }

  /** `PATCH /nodes/{id}`: renames a file or folder. */
  rename(id: string, name: string) {
    return this.request<Node>('PATCH', `/nodes/${id}`, { name });
  }

  /** `PATCH /nodes/{id}`: moves a file or folder into folder `parentId`, or the root folder. */
  move(id: string, parentId: string | null) {
    return this.request<Node>('PATCH', `/nodes/${id}`, { parent_id: parentId });
  }

  /** `DELETE /nodes/{id}`: moves a file or folder to the recycle bin. */
  delete(id: string) {
    return this.request<void>('DELETE', `/nodes/${id}`);
  }

  /** Shares files and folders with other users, read-only. */
  shareWith(nodeIds: string[], recipientIds: string[]) {
    return this.request<Share>('POST', '/shares', { node_ids: nodeIds, recipients: recipientIds });
  }

  /** The previous versions of file `id`, newest first. */
  versions(id: string) {
    return this.get<FileVersion[]>(`/nodes/${id}/versions`);
  }

  /** The shares this user made, with people or by link. */
  shares() {
    return this.get<Share[]>('/shares');
  }

  trash() {
    return this.get<TrashItem[]>('/trash');
  }

  /** What other users shared with this user. */
  sharedWithMe() {
    return this.get<ReceivedShare[]>('/shared');
  }

  /** `PUT /devices/{id}/visibility`. */
  setDeviceVisibility(deviceId: string, visibility: DeviceVisibility, users: string[] = []) {
    return this.request('PUT', `/devices/${deviceId}/visibility`, { visibility, users });
  }

  /** `GET /admin/users`; needs an administrator's session. */
  accounts() {
    return this.get<AdminAccount[]>('/admin/users');
  }

  me() {
    return this.get<{ id: string; username: string }>('/me');
  }

  /**
   * Sends the file at `file` to `device` of this user, the way a client does: create the
   * transfer, then upload the bytes of its one item. Lets a test drive only the receiving
   * client.
   */
  async sendFile(device: Pick<Device, 'id'>, file: string, name = path.basename(file)) {
    const bytes = await readFile(file);
    const me = await this.me();
    const created = await this.request<{ id: string; items: { id: string; upload_needed: boolean }[] }>(
      'POST',
      '/transfers',
      {
        recipient_id: me.id,
        recipient_device: device.id,
        items: [{ path: name, kind: 'file', size: bytes.length }],
      },
    );
    for (const item of created.items.filter((item) => item.upload_needed)) {
      await this.upload({ transfer_item_id: item.id, name, size: bytes.length }, bytes);
    }
    return created.id;
  }

  /** `POST /uploads`, then the bytes in chunks of the size the server asks for. */
  async upload(request: Record<string, unknown> & { size: number }, bytes: Buffer) {
    const session = await this.request<{ id: string; offset: number; chunk_size: number }>(
      'POST',
      '/uploads',
      request,
    );
    let offset = session.offset;
    do {
      const chunk = bytes.subarray(offset, offset + session.chunk_size);
      const response = await fetch(`${this.baseURL}/api/v1/uploads/${session.id}`, {
        method: 'PATCH',
        headers: {
          authorization: `Bearer ${this.token}`,
          'content-type': 'application/octet-stream',
          'upload-offset': String(offset),
        },
        body: new Uint8Array(chunk),
      });
      if (!response.ok) {
        throw new ApiError(response.status, await response.text(), `upload chunk at ${offset}`);
      }
      offset = ((await response.json()) as { offset: number }).offset;
    } while (offset < bytes.length);
  }
}

const MIME_TYPES: Record<string, string> = {
  '.txt': 'text/plain',
  '.png': 'image/png',
};

function mimeOf(name: string) {
  return MIME_TYPES[path.extname(name).toLowerCase()] ?? 'application/octet-stream';
}
