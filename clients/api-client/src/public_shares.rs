//! Share links as a visitor opens them (`/public/shares/{token}`): no sign-in, and an access
//! token from [`Client::unlock_share`] for a link with a password.

use reqwest::RequestBuilder;

use protocol::{
    NodeId, PublicShare, ShareAccess, SharedFolderQuery, SharedNode, UnlockShare, paths,
};

use crate::{ApiError, Client, client::decode};

impl Client {
    /// What the link with `token` shares; the items stay hidden until a password-protected
    /// link is opened with its `access` token.
    pub async fn public_share(
        &self,
        token: &str,
        access: Option<&str>,
    ) -> Result<PublicShare, ApiError> {
        let request = self.http().get(self.url(&paths::public_share(token)));
        decode(with_access(request, access).send().await?).await
    }

    /// Trades the link's password for an access token; a wrong password is
    /// [`ApiError::Unauthorized`].
    pub async fn unlock_share(&self, token: &str, password: &str) -> Result<ShareAccess, ApiError> {
        let request = UnlockShare {
            password: password.to_owned(),
        };
        let response = self
            .http()
            .post(self.url(&paths::public_share_unlock(token)))
            .json(&request)
            .send()
            .await?;
        decode(response).await
    }

    /// What is in `parent`, a shared folder or a folder inside one.
    pub async fn shared_folder(
        &self,
        token: &str,
        parent: NodeId,
        access: Option<&str>,
    ) -> Result<Vec<SharedNode>, ApiError> {
        let query = SharedFolderQuery { parent_id: parent };
        let request = self
            .http()
            .get(self.url(&paths::public_share_nodes(token)))
            .query(&query);
        decode(with_access(request, access).send().await?).await
    }

    /// URL that downloads shared file `id` when opened, carrying the `access` token in the
    /// query since a link cannot send headers.
    pub fn shared_file_url(&self, token: &str, id: NodeId, access: Option<&str>) -> String {
        with_query(
            self.url(&paths::public_share_file_content(token, id)),
            &[(paths::SHARE_ACCESS_PARAM, access.map(str::to_owned))],
        )
    }

    /// URL that downloads everything the link shares as one zip when opened, or only
    /// `folder` and what is in it.
    pub fn shared_archive_url(
        &self,
        token: &str,
        folder: Option<NodeId>,
        access: Option<&str>,
    ) -> String {
        with_query(
            self.url(&paths::public_share_archive(token)),
            &[
                (
                    paths::SHARE_FOLDER_PARAM,
                    folder.map(|folder| folder.to_string()),
                ),
                (paths::SHARE_ACCESS_PARAM, access.map(str::to_owned)),
            ],
        )
    }
}

/// `url` with the `params` that have a value as its query. The values are ids and tokens,
/// which need no escaping.
fn with_query(url: String, params: &[(&str, Option<String>)]) -> String {
    let query = params
        .iter()
        .filter_map(|(name, value)| value.as_ref().map(|value| format!("{name}={value}")))
        .collect::<Vec<_>>()
        .join("&");
    if query.is_empty() {
        url
    } else {
        format!("{url}?{query}")
    }
}

fn with_access(request: RequestBuilder, access: Option<&str>) -> RequestBuilder {
    match access {
        Some(access) => request.header(paths::SHARE_ACCESS, access),
        None => request,
    }
}
