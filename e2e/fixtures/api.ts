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
