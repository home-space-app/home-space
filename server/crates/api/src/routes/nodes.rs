use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post, put},
};

use files::{Names, NodeUpdate};
use protocol::{
    ContentQuery, CreateFolderRequest, FileVersion, ListQuery, MoveRequest, MoveResponse, Node,
    NodeId, NodeProperties, Patch, TicketRequest, UpdateRequest, UploadQuery,
};

use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::{
    ApiError, AppState,
    extract::{AuthUser, Tenant, TicketQuery},
    support::{
        body::{body_stream, declared_type},
        convert::move_response,
        download::{Download, ranged_in_folder},
        outcome::stored,
    },
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/nodes", get(list))
        .route("/nodes/move", post(move_nodes))
        .route("/nodes/{id}", get(get_node).patch(update).delete(delete))
        .route("/nodes/{id}/ancestors", get(ancestors))
        .route("/nodes/{id}/properties", get(properties))
        .route("/folders", post(create_folder))
        .route("/files", post(upload).layer(DefaultBodyLimit::disable()))
        .route("/nodes/{id}/versions", get(list_versions))
        .route(
            "/nodes/{id}/versions/{version}",
            axum::routing::delete(delete_version),
        )
        .route(
            "/files/{id}/versions/{version}/content",
            get(download_version),
        )
        .route(
            "/files/{id}/content",
            put(replace_content)
                .layer(DefaultBodyLimit::disable())
                .get(download),
        )
}

/// The names of the nodes in `parent`, for a read or write by `user`'s session; `423 locked`
/// inside a password folder the session has not unlocked.
async fn names(
    tenant: &Tenant,
    user: &AuthUser,
    parent: Option<NodeId>,
) -> Result<Names, ApiError> {
    Ok(tenant.vault.names(user.id, user.session_id, parent).await?)
}

/// Node `id` and the names of its folder, which the session must be able to open.
async fn open_node(
    state: &AppState,
    tenant: &Tenant,
    user: &AuthUser,
    id: NodeId,
) -> Result<(db::Node, Names), ApiError> {
    let node = state.files.nodes.get(user.id, id).await?;
    let names = names(tenant, user, node.parent_id).await?;
    Ok((node, names))
}

async fn list(
    State(state): State<AppState>,
    tenant: Tenant,
    user: AuthUser,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<Vec<Node>>, ApiError> {
    let names = names(&tenant, &user, query.parent_id).await?;
    let nodes = state.files.nodes.list(user.id, query.parent_id).await?;
    Ok(Json(
        names
            .show_listing(nodes)
            .into_iter()
            .map(Node::from)
            .collect(),
    ))
}

async fn get_node(
    State(state): State<AppState>,
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<Json<Node>, ApiError> {
    let (node, names) = open_node(&state, &tenant, &user, id).await?;
    Ok(Json(names.show_node(node).into()))
}

async fn ancestors(
    State(state): State<AppState>,
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<Json<Vec<Node>>, ApiError> {
    open_node(&state, &tenant, &user, id).await?;
    let nodes = state.files.nodes.ancestors(user.id, id).await?;
    let parents: Vec<NodeId> = nodes.iter().filter_map(|node| node.parent_id).collect();
    let shown = tenant
        .vault
        .shown(user.id, user.session_id, &parents)
        .await?;
    nodes
        .into_iter()
        .map(|mut node| {
            node.name = shown
                .name(node.parent_id, node.kind, &node.name)
                .ok_or_else(|| ApiError::from(accounts::AccountError::Locked))?;
            Ok(Node::from(node))
        })
        .collect::<Result<_, ApiError>>()
        .map(Json)
}

async fn properties(
    State(state): State<AppState>,
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<Json<NodeProperties>, ApiError> {
    open_node(&state, &tenant, &user, id).await?;
    let properties = state.files.nodes.properties(user.id, id).await?;
    Ok(Json(NodeProperties {
        source_device: properties.source_device,
    }))
}

async fn create_folder(
    State(state): State<AppState>,
    tenant: Tenant,
    user: AuthUser,
    ApiJson(request): ApiJson<CreateFolderRequest>,
) -> Result<(StatusCode, Json<Node>), ApiError> {
    let names = names(&tenant, &user, request.parent_id).await?;
    let node = state
        .files
        .nodes
        .create_folder(user.actor(), request.parent_id, &request.name, &names)
        .await?;
    Ok((StatusCode::CREATED, Json(names.show_node(node).into())))
}

async fn upload(
    State(state): State<AppState>,
    tenant: Tenant,
    user: AuthUser,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<UploadQuery>,
    body: Body,
) -> Result<(StatusCode, Json<Node>), ApiError> {
    let names = names(&tenant, &user, query.parent_id).await?;
    let outcome = state
        .files
        .nodes
        .upload(
            user.actor(),
            files::NewFile {
                parent: query.parent_id,
                name: &query.name,
                names: &names,
                content_type: declared_type(&headers),
                modified_at: query.modified_at,
            },
            body_stream(body),
        )
        .await?;
    Ok(stored(outcome, &names))
}

async fn replace_content(
    State(state): State<AppState>,
    tenant: Tenant,
    user: AuthUser,
    headers: HeaderMap,
    ApiPath(id): ApiPath<NodeId>,
    ApiQuery(query): ApiQuery<ContentQuery>,
    body: Body,
) -> Result<Json<Node>, ApiError> {
    let (_, names) = open_node(&state, &tenant, &user, id).await?;
    let outcome = state
        .files
        .nodes
        .replace_content(
            user.actor(),
            id,
            &names,
            declared_type(&headers),
            query.modified_at,
            body_stream(body),
        )
        .await?;
    Ok(Json(names.show_node(outcome.into_node()).into()))
}

async fn download(
    State(state): State<AppState>,
    tenant: Tenant,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<TicketQuery>,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<Response, ApiError> {
    let (owner, session) = tenant
        .caller_session(
            &headers,
            query.ticket.as_deref(),
            &TicketRequest::Download { node_id: id },
        )
        .await?;
    let keys = tenant.vault.read_keys(owner, session, id).await?;
    let (node, file) = state.files.nodes.download(owner, id).await?;
    let names = tenant.vault.names(owner, session, node.parent_id).await?;
    let node = names.show_node(node);
    ranged_in_folder(&headers, Download::of(&node), file, keys).await
}

async fn download_version(
    State(state): State<AppState>,
    tenant: Tenant,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<TicketQuery>,
    ApiPath((id, version)): ApiPath<(NodeId, i32)>,
) -> Result<Response, ApiError> {
    let (owner, session) = tenant
        .caller_session(
            &headers,
            query.ticket.as_deref(),
            &TicketRequest::DownloadVersion {
                node_id: id,
                version,
            },
        )
        .await?;
    let keys = tenant.vault.read_keys(owner, session, id).await?;
    let (node, meta, file) = state
        .files
        .versions
        .download_version(owner, id, version)
        .await?;
    let names = tenant.vault.names(owner, session, node.parent_id).await?;
    let node = names.show_node(node);
    let download = Download {
        name: &node.name,
        mime: Some(&meta.mime),
        size: u64::try_from(meta.size).unwrap_or_default(),
    };
    ranged_in_folder(&headers, download, file, keys).await
}

async fn list_versions(
    State(state): State<AppState>,
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<Json<Vec<FileVersion>>, ApiError> {
    open_node(&state, &tenant, &user, id).await?;
    let versions = state.files.versions.versions(user.id, id).await?;
    Ok(Json(versions.into_iter().map(FileVersion::from).collect()))
}

async fn delete_version(
    State(state): State<AppState>,
    tenant: Tenant,
    user: AuthUser,
    ApiPath((id, version)): ApiPath<(NodeId, i32)>,
) -> Result<StatusCode, ApiError> {
    open_node(&state, &tenant, &user, id).await?;
    state
        .files
        .versions
        .delete_version(user.id, id, version)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn update(
    State(state): State<AppState>,
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
    ApiJson(request): ApiJson<UpdateRequest>,
) -> Result<Json<Node>, ApiError> {
    // Moves stay within one password folder or outside all, so the names of the node's folder
    // are those of its destination too.
    let (_, names) = open_node(&state, &tenant, &user, id).await?;
    match request.parent_id {
        Patch::Set(parent) => {
            tenant
                .vault
                .check_move(user.id, &[id], Some(parent))
                .await?
        }
        Patch::Clear => tenant.vault.check_move(user.id, &[id], None).await?,
        Patch::Keep => {}
    }
    let changes = NodeUpdate {
        name: request.name,
        parent_id: request.parent_id,
    };
    let outcome = state
        .files
        .nodes
        .update(user.actor(), id, changes, &names)
        .await?;
    Ok(Json(names.show_node(outcome.node).into()))
}

async fn move_nodes(
    State(state): State<AppState>,
    tenant: Tenant,
    user: AuthUser,
    ApiJson(request): ApiJson<MoveRequest>,
) -> Result<Json<MoveResponse>, ApiError> {
    tenant
        .vault
        .check_move(user.id, &request.ids, request.parent_id)
        .await?;
    let names = names(&tenant, &user, request.parent_id).await?;
    // A skip that would bin a protected file rolls the move back (`409 protected`).
    let batch = state
        .files
        .nodes
        .move_nodes(
            user.actor(),
            &request.ids,
            request.parent_id,
            request.on_conflict,
        )
        .await?;
    Ok(Json(move_response(batch, &names)))
}

async fn delete(
    State(state): State<AppState>,
    tenant: Tenant,
    user: AuthUser,
    ApiPath(id): ApiPath<NodeId>,
) -> Result<StatusCode, ApiError> {
    // What is inside a password folder, and a folder holding one, only once unlocked. What is
    // inside a protected folder of either lock is deleted for good, not kept in the bin.
    names(&tenant, &user, Some(id)).await?;
    open_node(&state, &tenant, &user, id).await?;
    tenant
        .vault
        .require_unlocked_below(user.id, user.session_id, id)
        .await?;
    state.files.trash.delete(user.actor(), id).await?;
    Ok(StatusCode::NO_CONTENT)
}
