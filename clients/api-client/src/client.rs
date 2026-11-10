use std::sync::{Arc, Mutex, PoisonError};

use reqwest::{RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;
use tokio::sync::Mutex as AsyncMutex;

use protocol::{
    Capabilities, ChangesQuery, ChangesResponse, ClientMessage, CreateFolderRequest, DeviceInfo,
    ErrorBody, FileVersion, Health, ListQuery, LoginRequest, Me, MoveRequest, MoveResponse, Node,
    NodeId, NodeProperties, OidcExchangeRequest, OnConflict, Providers, RefreshRequest,
    RestoreRequest, SearchHit, SearchQuery, TicketRequest, TicketResponse, TokenPair, TrashItem,
    UpdateRequest, paths,
};
#[cfg(not(target_arch = "wasm32"))]
use protocol::{ContentQuery, UploadQuery};
#[cfg(not(target_arch = "wasm32"))]
use time::OffsetDateTime;

use crate::{ApiError, Pkce};

#[cfg(not(target_arch = "wasm32"))]
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// The longest wait for the next bytes of a response, so a stalled connection fails.
#[cfg(not(target_arch = "wasm32"))]
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Called with the new refresh token after every rotation, or `None` once the session ends.
/// With a [cookie session](Client::with_cookie_session) the token is empty: the browser has it.
pub type TokenListener = Arc<dyn Fn(Option<&str>) + Send + Sync>;

/// Client for one server; cheap to clone, clones share the session.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: String,
    shared: Arc<Shared>,
    device: Option<DeviceInfo>,
    cookie_session: bool,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

struct Shared {
    session: Mutex<Option<TokenPair>>,
    capabilities: Mutex<Option<Capabilities>>,
    refresh_gate: AsyncMutex<()>,
    listener: Option<TokenListener>,
}

impl Client {
    /// Creates a signed-out client for the server at `base_url` (no trailing slash).
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::build(base_url.into(), None)
    }

    /// Like [`Client::new`], notifying `listener` whenever the refresh token changes.
    pub fn with_listener(base_url: impl Into<String>, listener: TokenListener) -> Self {
        Self::build(base_url.into(), Some(listener))
    }

    fn build(base_url: String, listener: Option<TokenListener>) -> Self {
        Self {
            http: http_client(),
            base_url,
            shared: Arc::new(Shared {
                session: Mutex::new(None),
                capabilities: Mutex::new(None),
                refresh_gate: AsyncMutex::new(()),
                listener,
            }),
            device: None,
            cookie_session: false,
        }
    }

    /// The client of a browser: the server keeps the refresh token in a cookie that scripts
    /// cannot read, so this client never holds it and the session is shared by all tabs.
    pub fn with_cookie_session(mut self) -> Self {
        self.cookie_session = true;
        self
    }

    /// The client signing in as `device`, which the server then lists among the user's
    /// devices and tags its sessions and uploads with.
    pub fn with_device(mut self, device: DeviceInfo) -> Self {
        self.device = Some(device);
        self
    }

    /// The id of the device this client speaks for, if it names one.
    pub fn device_id(&self) -> Option<uuid::Uuid> {
        self.device.as_ref().map(|device| device.id)
    }

    /// The hello to send first on the events websocket, naming this client's device so the
    /// server registers it and shows it online; `None` when the client names no device.
    pub fn hello(&self) -> Option<String> {
        let hello = ClientMessage::Hello {
            device: self.device.clone()?,
        };
        serde_json::to_string(&hello).ok()
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// The server this client talks to (no trailing slash).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Absolute URL for a server path.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// Websocket URL for the events stream, authenticated by `ticket`.
    pub fn events_url(&self, ticket: &str) -> String {
        let base = self
            .base_url
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1);
        format!("{base}{}?{}={ticket}", paths::WS, paths::TICKET_PARAM)
    }

    /// URL that downloads a file when opened, authenticated by `ticket`.
    pub fn download_url(&self, id: NodeId, ticket: &str) -> String {
        format!(
            "{}?{}={ticket}",
            self.url(&paths::file_content(id)),
            paths::TICKET_PARAM
        )
    }

    /// URL that downloads a previous version of a file when opened, authenticated by `ticket`.
    pub fn version_download_url(&self, id: NodeId, version: i32, ticket: &str) -> String {
        format!(
            "{}?{}={ticket}",
            self.url(&paths::version_content(id, version)),
            paths::TICKET_PARAM
        )
    }

    /// Current access token, if signed in.
    pub fn access_token(&self) -> Option<String> {
        self.session().map(|session| session.access_token)
    }

    /// Exchanges the refresh token for a new session unless another caller already did
    /// since `stale_access_token` was read.
    pub async fn refresh_session(&self, stale_access_token: &str) -> Result<(), ApiError> {
        let _gate = self.shared.refresh_gate.lock().await;
        let current = self.session().ok_or(ApiError::Unauthorized)?;
        if current.access_token == stale_access_token {
            self.rotate(self.held_token(&current)).await
        } else {
            Ok(())
        }
    }

    /// Signs in with a username and password.
    pub async fn login(&self, username: &str, password: &str) -> Result<Me, ApiError> {
        let request = LoginRequest {
            username: username.to_owned(),
            password: password.to_owned(),
            device: self.device.clone(),
            cookie: self.cookie_session,
        };
        let response = self
            .http
            .post(self.url(paths::LOGIN))
            .json(&request)
            .send()
            .await?;
        let pair: TokenPair = decode(response).await?;
        self.store(pair);
        self.signed_in().await
    }

    /// How this server lets users sign in; needs no session.
    pub async fn providers(&self) -> Result<Providers, ApiError> {
        let response = self.http.get(self.url(paths::PROVIDERS)).send().await?;
        decode(response).await
    }

    /// The URL that starts a single sign-on: open it in a browser. Afterwards the server sends
    /// the browser to `redirect_uri` with a one-time `code` (or an `error`) in the query, which
    /// [`oidc_exchange`](Self::oidc_exchange) turns into a session.
    pub fn oidc_start_url(&self, redirect_uri: &str, pkce: &Pkce) -> Result<String, ApiError> {
        reqwest::Url::parse_with_params(
            &self.url(paths::OIDC_START),
            [
                ("redirect_uri", redirect_uri),
                ("code_challenge", pkce.challenge()),
            ],
        )
        .map(String::from)
        .map_err(|err| ApiError::Transport(format!("invalid server address: {err}")))
    }

    /// Completes a single sign-on with the `code` the server put in the redirect.
    pub async fn oidc_exchange(&self, code: &str, pkce: &Pkce) -> Result<Me, ApiError> {
        let request = OidcExchangeRequest {
            code: code.to_owned(),
            code_verifier: pkce.verifier().to_owned(),
            device: self.device.clone(),
            cookie: self.cookie_session,
        };
        let response = self
            .http
            .post(self.url(paths::OIDC_EXCHANGE))
            .json(&request)
            .send()
            .await?;
        let pair: TokenPair = decode(response).await?;
        self.store(pair);
        self.signed_in().await
    }

    /// Resumes a session from a saved refresh token.
    pub async fn restore(&self, refresh_token: &str) -> Result<Me, ApiError> {
        self.resume(Some(refresh_token)).await
    }

    /// Resumes a [cookie session](Self::with_cookie_session) from the browser's cookie.
    /// `saved_token` is a refresh token the browser still holds from before the cookie
    /// existed; the server moves it into the cookie.
    pub async fn resume(&self, saved_token: Option<&str>) -> Result<Me, ApiError> {
        {
            let _gate = self.shared.refresh_gate.lock().await;
            self.rotate(saved_token).await?;
        }
        self.signed_in().await
    }

    /// Ends the session and revokes its refresh token.
    pub async fn logout(&self) -> Result<(), ApiError> {
        let session = self.session();
        self.clear();
        if let Some(session) = session {
            let request = RefreshRequest {
                refresh_token: self.held_token(&session).map(str::to_owned),
                cookie: self.cookie_session,
            };
            let response = self
                .http
                .post(self.url(paths::LOGOUT))
                .json(&request)
                .send()
                .await?;
            expect_success(response).await?;
        }
        Ok(())
    }

    /// Asks whether the server and its database are up; needs no session.
    pub async fn health(&self) -> Result<Health, ApiError> {
        let response = self.http.get(self.url(paths::HEALTH)).send().await?;
        decode(response).await
    }

    /// The signed-in user.
    pub async fn me(&self) -> Result<Me, ApiError> {
        self.send_json(|| self.http.get(self.url(paths::ME))).await
    }

    /// Lists a folder, or the root when `parent` is `None`.
    pub async fn list(&self, parent: Option<NodeId>) -> Result<Vec<Node>, ApiError> {
        let query = ListQuery { parent_id: parent };
        self.send_json(|| self.http.get(self.url(paths::NODES)).query(&query))
            .await
    }

    /// Lists Home like [`Client::list`], leaving `lost_and_found` out while it is empty.
    pub async fn list_home(&self, lost_and_found: NodeId) -> Result<Vec<Node>, ApiError> {
        let mut nodes = self.list(None).await?;
        if nodes.iter().any(|node| node.id == lost_and_found)
            && self.list(Some(lost_and_found)).await?.is_empty()
        {
            nodes.retain(|node| node.id != lost_and_found);
        }
        Ok(nodes)
    }

    /// Searches the signed-in user's files and folders by name.
    pub async fn search(&self, query: &str) -> Result<Vec<SearchHit>, ApiError> {
        let query = SearchQuery {
            q: query.to_owned(),
            limit: None,
        };
        self.send_json(|| self.http.get(self.url(paths::SEARCH)).query(&query))
            .await
    }

    /// Nodes changed after cursor `since`, or only the current cursor without it.
    ///
    /// Fails with [`ErrorCode::Gone`](protocol::ErrorCode::Gone) when the server no longer
    /// keeps the changes after `since`; the caller then reads the whole tree again.
    pub async fn changes(&self, since: Option<i64>) -> Result<ChangesResponse, ApiError> {
        let query = ChangesQuery { since, limit: None };
        self.send_json(|| self.http.get(self.url(paths::CHANGES)).query(&query))
            .await
    }

    /// Metadata of one node.
    pub async fn get(&self, id: NodeId) -> Result<Node, ApiError> {
        self.send_json(|| self.http.get(self.url(&paths::node(id))))
            .await
    }

    /// Ancestors of a node from the root down to its parent.
    pub async fn ancestors(&self, id: NodeId) -> Result<Vec<Node>, ApiError> {
        self.send_json(|| self.http.get(self.url(&paths::ancestors(id))))
            .await
    }

    /// The predefined properties of a node.
    pub async fn node_properties(&self, id: NodeId) -> Result<NodeProperties, ApiError> {
        self.send_json(|| self.http.get(self.url(&paths::properties(id))))
            .await
    }

    /// Creates a folder under `parent` (or the root).
    pub async fn create_folder(
        &self,
        parent: Option<NodeId>,
        name: &str,
    ) -> Result<Node, ApiError> {
        let request = CreateFolderRequest {
            parent_id: parent,
            name: name.to_owned(),
        };
        self.send_json(|| self.http.post(self.url(paths::FOLDERS)).json(&request))
            .await
    }

    /// Renames a node.
    pub async fn rename(&self, id: NodeId, name: &str) -> Result<Node, ApiError> {
        self.update(
            id,
            &UpdateRequest {
                name: Some(name.to_owned()),
                parent_id: None,
            },
        )
        .await
    }

    /// Moves a node into `parent`, or to the root when `None`.
    pub async fn move_to(&self, id: NodeId, parent: Option<NodeId>) -> Result<Node, ApiError> {
        self.update(
            id,
            &UpdateRequest {
                name: None,
                parent_id: Some(parent),
            },
        )
        .await
    }

    /// Moves a node into `parent` (the root when `None`) under a new name, in one step.
    pub async fn relocate(
        &self,
        id: NodeId,
        parent: Option<NodeId>,
        name: &str,
    ) -> Result<Node, ApiError> {
        self.update(
            id,
            &UpdateRequest {
                name: Some(name.to_owned()),
                parent_id: Some(parent),
            },
        )
        .await
    }

    /// Moves several nodes into `parent`, or to the root when `None`.
    ///
    /// With [`OnConflict::Abort`] a clash changes nothing and is listed in the response;
    /// `Skip` moves the clashing files to the recycle bin instead.
    pub async fn move_nodes(
        &self,
        ids: &[NodeId],
        parent: Option<NodeId>,
        on_conflict: OnConflict,
    ) -> Result<MoveResponse, ApiError> {
        let request = MoveRequest {
            ids: ids.to_vec(),
            parent_id: parent,
            on_conflict,
        };
        self.send_json(|| self.http.post(self.url(paths::NODES_MOVE)).json(&request))
            .await
    }

    /// Deletes a node by moving its files, or everything below it, to the recycle bin.
    pub async fn delete(&self, id: NodeId) -> Result<(), ApiError> {
        let response = self
            .send(|| self.http.delete(self.url(&paths::node(id))))
            .await?;
        expect_success(response).await
    }

    /// The files in the recycle bin, most recently deleted first.
    pub async fn trash(&self) -> Result<Vec<TrashItem>, ApiError> {
        self.send_json(|| self.http.get(self.url(paths::TRASH)))
            .await
    }

    /// Moves binned files to Lost & Found; `on_conflict` decides what happens to files whose
    /// name is taken there.
    pub async fn restore_trashed(
        &self,
        ids: &[NodeId],
        on_conflict: OnConflict,
    ) -> Result<MoveResponse, ApiError> {
        let request = RestoreRequest {
            ids: ids.to_vec(),
            on_conflict,
        };
        self.send_json(|| {
            self.http
                .post(self.url(paths::TRASH_RESTORE))
                .json(&request)
        })
        .await
    }

    /// Permanently deletes every file in the recycle bin.
    pub async fn empty_trash(&self) -> Result<(), ApiError> {
        let response = self
            .send(|| self.http.delete(self.url(paths::TRASH)))
            .await?;
        expect_success(response).await
    }

    /// Permanently deletes one file in the recycle bin.
    pub async fn purge_trashed(&self, id: NodeId) -> Result<(), ApiError> {
        let response = self
            .send(|| self.http.delete(self.url(&paths::trash_item(id))))
            .await?;
        expect_success(response).await
    }

    /// Uploads a file under `parent` (or the root).
    ///
    /// `body` is called once per attempt, so an expired token can be refreshed and the
    /// upload retried with a fresh stream.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn upload<B>(
        &self,
        parent: Option<NodeId>,
        name: &str,
        body: B,
    ) -> Result<Node, ApiError>
    where
        B: Fn() -> reqwest::Body,
    {
        self.upload_modified(parent, name, None, body).await
    }

    /// Uploads a file whose content was last changed at `modified_at`.
    ///
    /// When a file of that name already holds content changed later, the upload is kept as
    /// one of its versions and the returned file keeps its current content.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn upload_modified<B>(
        &self,
        parent: Option<NodeId>,
        name: &str,
        modified_at: Option<OffsetDateTime>,
        body: B,
    ) -> Result<Node, ApiError>
    where
        B: Fn() -> reqwest::Body,
    {
        let query = UploadQuery {
            parent_id: parent,
            name: name.to_owned(),
            modified_at,
        };
        self.send_json(|| {
            self.http
                .post(self.url(paths::FILES))
                .query(&query)
                .body(body())
        })
        .await
    }

    /// Replaces the content of file `id`, last changed at `modified_at`; content older than
    /// the current one is kept as a version instead.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn replace_content<B>(
        &self,
        id: NodeId,
        modified_at: Option<OffsetDateTime>,
        body: B,
    ) -> Result<Node, ApiError>
    where
        B: Fn() -> reqwest::Body,
    {
        let query = ContentQuery { modified_at };
        self.send_json(|| {
            self.http
                .put(self.url(&paths::file_content(id)))
                .query(&query)
                .body(body())
        })
        .await
    }

    /// Starts downloading a file; the caller streams the body of the returned response.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn download(&self, id: NodeId) -> Result<Response, ApiError> {
        let response = self
            .send(|| self.http.get(self.url(&paths::file_content(id))))
            .await?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(error_from(response).await)
        }
    }

    /// Starts downloading a previous version of a file.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn download_version(&self, id: NodeId, version: i32) -> Result<Response, ApiError> {
        let response = self
            .send(|| {
                self.http
                    .get(self.url(&paths::version_content(id, version)))
            })
            .await?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(error_from(response).await)
        }
    }

    /// Previous versions of a file, newest first.
    pub async fn versions(&self, id: NodeId) -> Result<Vec<FileVersion>, ApiError> {
        self.send_json(|| self.http.get(self.url(&paths::versions(id))))
            .await
    }

    /// Deletes a previous version of a file.
    pub async fn delete_version(&self, id: NodeId, version: i32) -> Result<(), ApiError> {
        let response = self
            .send(|| self.http.delete(self.url(&paths::version(id, version))))
            .await?;
        expect_success(response).await
    }

    /// Requests a short-lived ticket for a websocket or a browser download.
    pub async fn ticket(&self, request: &TicketRequest) -> Result<TicketResponse, ApiError> {
        self.send_json(|| self.http.post(self.url(paths::TICKETS)).json(request))
            .await
    }

    async fn update(&self, id: NodeId, request: &UpdateRequest) -> Result<Node, ApiError> {
        self.send_json(|| self.http.patch(self.url(&paths::node(id))).json(request))
            .await
    }

    pub(crate) async fn send_json<T, F>(&self, build: F) -> Result<T, ApiError>
    where
        T: DeserializeOwned,
        F: Fn() -> RequestBuilder,
    {
        decode(self.send(build).await?).await
    }

    pub(crate) async fn send<F>(&self, build: F) -> Result<Response, ApiError>
    where
        F: Fn() -> RequestBuilder,
    {
        let token = self.access_token().ok_or(ApiError::Unauthorized)?;
        let response = build().bearer_auth(&token).send().await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            self.refresh_session(&token).await?;
            let token = self.access_token().ok_or(ApiError::Unauthorized)?;
            Ok(build().bearer_auth(token).send().await?)
        } else {
            Ok(response)
        }
    }

    /// The refresh token to send for `session`: none when the browser's cookie carries it.
    fn held_token<'a>(&self, session: &'a TokenPair) -> Option<&'a str> {
        (!self.cookie_session).then_some(session.refresh_token.as_str())
    }

    async fn rotate(&self, refresh_token: Option<&str>) -> Result<(), ApiError> {
        let request = RefreshRequest {
            refresh_token: refresh_token.map(str::to_owned),
            cookie: self.cookie_session,
        };
        let response = self
            .http
            .post(self.url(paths::REFRESH))
            .json(&request)
            .send()
            .await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            self.clear();
            Err(ApiError::Unauthorized)
        } else {
            self.store(decode(response).await?);
            Ok(())
        }
    }

    pub(crate) fn store(&self, pair: TokenPair) {
        let refresh_token = pair.refresh_token.clone();
        self.set_session(Some(pair));
        self.notify(Some(&refresh_token));
    }

    fn clear(&self) {
        self.set_session(None);
        self.set_capabilities(None);
        self.notify(None);
    }

    fn notify(&self, refresh_token: Option<&str>) {
        if let Some(listener) = &self.shared.listener {
            listener(refresh_token);
        }
    }

    fn session(&self) -> Option<TokenPair> {
        self.shared
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_session(&self, session: Option<TokenPair>) {
        let mut current = self
            .shared
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *current = session;
    }

    pub(crate) fn capabilities(&self) -> Option<Capabilities> {
        *self
            .shared
            .capabilities
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn set_capabilities(&self, capabilities: Option<Capabilities>) {
        let mut current = self
            .shared
            .capabilities
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *current = capabilities;
    }
}

async fn decode<T: DeserializeOwned>(response: Response) -> Result<T, ApiError> {
    if response.status().is_success() {
        Ok(response.json().await?)
    } else {
        Err(error_from(response).await)
    }
}

/// The HTTP client, which gives up on a connection that cannot be made or stops sending.
/// Neither limit caps a long transfer that keeps moving.
#[cfg(not(target_arch = "wasm32"))]
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .build()
        // Building only fails when the TLS backend cannot start; the default client then
        // fails each request with the same cause.
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// The browser handles timeouts itself.
#[cfg(target_arch = "wasm32")]
fn http_client() -> reqwest::Client {
    reqwest::Client::new()
}

pub(crate) async fn expect_success(response: Response) -> Result<(), ApiError> {
    if response.status().is_success() {
        Ok(())
    } else {
        Err(error_from(response).await)
    }
}

pub(crate) async fn error_from(response: Response) -> ApiError {
    let status = response.status();
    match response.json::<ErrorBody>().await {
        _ if status == StatusCode::UNAUTHORIZED => ApiError::Unauthorized,
        Ok(body) => ApiError::Api {
            code: body.error,
            message: body.message,
        },
        Err(_) => ApiError::Status {
            status: status.as_u16(),
            reason: status.canonical_reason().unwrap_or_default().to_owned(),
        },
    }
}
