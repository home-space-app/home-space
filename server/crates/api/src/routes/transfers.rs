//! Files and folders sent to a device.

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
};
use uuid::Uuid;

use db::IncomingScope;
use files::ArchiveEntry;
use protocol::{
    CreateTransfer, CreatedTransfer, CreatedTransferItem, IncomingQuery, Node, NodeKind,
    SaveTransfer, TicketRequest, Transfer, TransferState, TransferSummary,
};

use crate::extract::{ApiJson, ApiPath, ApiQuery};
use crate::{
    ApiError, AppState,
    extract::{AuthUser, Tenant, TicketQuery},
    support::{
        archive::zip_response,
        convert::{summary, transfer_item},
        download::{Download, ranged},
        publish::{publish_changed, publish_ready, publish_saved},
    },
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/transfers", post(create))
        .route("/transfers/incoming", get(incoming))
        .route("/transfers/outgoing", get(outgoing))
        .route("/transfers/{id}", get(get_transfer).delete(cancel))
        .route("/transfers/{id}/items/{item}/content", get(content))
        .route("/transfers/{id}/archive", get(archive))
        .route("/transfers/{id}/decline", post(decline))
        .route("/transfers/{id}/received", post(received))
        .route("/transfers/{id}/save", post(save))
}

fn publish_finished(state: &AppState, transfer: &db::Transfer) {
    publish_changed(
        &state.hub,
        transfer.id,
        transfer.sender_id,
        transfer.recipient_id,
        transfer.state,
    );
}

async fn create(
    State(state): State<AppState>,
    user: AuthUser,
    ApiJson(request): ApiJson<CreateTransfer>,
) -> Result<(StatusCode, Json<CreatedTransfer>), ApiError> {
    let (transfer, items) = state
        .files
        .create_transfer(user.id, user.device_id, request)
        .await?;
    let created = CreatedTransfer {
        id: transfer.id,
        state: transfer.state,
        items: items
            .into_iter()
            .map(|item| CreatedTransferItem {
                upload_needed: item.kind == NodeKind::File && item.sha256.is_none(),
                id: item.id,
                path: item.path,
            })
            .collect(),
    };
    if transfer.state == TransferState::Ready {
        publish_ready(&state, transfer);
    }
    Ok((StatusCode::CREATED, Json(created)))
}

/// The transfers waiting for the caller: ready, or declined until they expire.
async fn incoming(
    State(state): State<AppState>,
    user: AuthUser,
    ApiQuery(query): ApiQuery<IncomingQuery>,
) -> Result<Json<Vec<TransferSummary>>, ApiError> {
    let scope = match (query.this_device, user.device_id) {
        (false, device) => Some(IncomingScope::AllExceptFrom(device)),
        (true, Some(device)) => Some(IncomingScope::ToDevice(device)),
        (true, None) => None,
    };
    let transfers = match scope {
        Some(scope) => state.files.incoming_transfers(user.id, scope).await?,
        None => Vec::new(),
    };
    Ok(Json(transfers.into_iter().map(summary).collect()))
}

async fn outgoing(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<Json<Vec<TransferSummary>>, ApiError> {
    let transfers = state
        .files
        .outgoing_transfers(user.id, user.device_id)
        .await?;
    Ok(Json(transfers.into_iter().map(summary).collect()))
}

async fn get_transfer(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<Transfer>, ApiError> {
    let (transfer, items) = state.files.transfer(user.id, id).await?;
    Ok(Json(Transfer {
        summary: summary(transfer),
        items: items.into_iter().map(transfer_item).collect(),
    }))
}

async fn cancel(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, ApiError> {
    let transfer = state.files.cancel_transfer(user.id, id).await?;
    publish_finished(&state, &transfer);
    Ok(StatusCode::NO_CONTENT)
}

async fn decline(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, ApiError> {
    let transfer = state.files.decline_transfer(user.id, id).await?;
    publish_finished(&state, &transfer);
    Ok(StatusCode::NO_CONTENT)
}

async fn received(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, ApiError> {
    let transfer = state.files.confirm_transfer(user.id, id).await?;
    publish_finished(&state, &transfer);
    Ok(StatusCode::NO_CONTENT)
}

/// Saves the transfer into the caller's tree; answers the nodes created at the top.
async fn save(
    State(state): State<AppState>,
    user: AuthUser,
    ApiPath(id): ApiPath<Uuid>,
    body: Option<Json<SaveTransfer>>,
) -> Result<Json<Vec<Node>>, ApiError> {
    let parent = body.and_then(|Json(body)| body.parent_id);
    let saved = state
        .files
        .on_device(user.device_id)
        .save_transfer(user.id, id, parent)
        .await?;
    publish_finished(&state, &saved.transfer);
    let top = publish_saved(&state, user.id, parent, saved.folders, saved.files).await;
    Ok(Json(top))
}

/// A file of a ready transfer, for its recipient. Answers one `Range` of bytes when asked,
/// so an interrupted download can resume.
async fn content(
    State(state): State<AppState>,
    tenant: Tenant,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<TicketQuery>,
    ApiPath((id, item_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Response, ApiError> {
    let caller = tenant
        .caller(
            &headers,
            query.ticket.as_deref(),
            &TicketRequest::TransferItem {
                transfer_id: id,
                item_id,
            },
        )
        .await?;
    let (item, file) = state.files.open_transfer_item(caller, id, item_id).await?;
    let name = item.path.rsplit('/').next().unwrap_or(&item.path);
    let download = Download {
        name,
        mime: item.mime.as_deref(),
        size: u64::try_from(item.size).unwrap_or_default(),
    };
    ranged(&headers, download, file).await
}

/// Every file and folder of a ready transfer as one zip, for its recipient.
async fn archive(
    State(state): State<AppState>,
    tenant: Tenant,
    headers: HeaderMap,
    ApiQuery(query): ApiQuery<TicketQuery>,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, ApiError> {
    let caller = tenant
        .caller(
            &headers,
            query.ticket.as_deref(),
            &TicketRequest::TransferArchive { transfer_id: id },
        )
        .await?;
    let (transfer, items) = state.files.transfer_archive(caller, id).await?;
    let entries = items
        .into_iter()
        .map(|(item, path)| ArchiveEntry {
            source: (item.kind == NodeKind::File).then_some(path),
            path: item.path,
            kind: item.kind,
            size: item.size,
            modified_at: item.modified_at,
        })
        .collect();
    Ok(zip_response(&archive_name(&transfer), entries))
}

/// The name of a transfer's zip: its only top-level item, or who sent it.
fn archive_name(transfer: &db::Transfer) -> String {
    match &transfer.name {
        Some(name) => name.clone(),
        None => format!("From {}", transfer.sender_name),
    }
}
