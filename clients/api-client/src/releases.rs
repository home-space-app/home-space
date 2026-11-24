//! The latest Home Space release, for the "New version available" toast.

use serde::Deserialize;

/// A published release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// The version, without the tag's leading `v`, e.g. `0.14.0`.
    pub version: String,
    /// The release page, where the apps are attached.
    pub url: String,
}

/// The fields of GitHub's release JSON this reads.
#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

/// How long the check may take before it is given up.
#[cfg(not(target_arch = "wasm32"))]
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Reads the latest release from `url` (`client_core::update::RELEASES_API_URL`, or a stub in
/// tests). Any failure, such as no network or GitHub's rate limit, is `None`: the toast is
/// simply not shown, and the next start tries again.
pub async fn latest_release(url: &str) -> Option<Release> {
    fetch(url).await.ok()
}

async fn fetch(url: &str) -> Result<Release, reqwest::Error> {
    let request = reqwest::Client::new()
        .get(url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json");
    #[cfg(not(target_arch = "wasm32"))]
    let request = request
        .header(reqwest::header::USER_AGENT, crate::user_agent())
        .timeout(TIMEOUT);
    let release: GithubRelease = request.send().await?.error_for_status()?.json().await?;
    Ok(Release {
        version: release
            .tag_name
            .strip_prefix('v')
            .unwrap_or(&release.tag_name)
            .to_owned(),
        url: release.html_url,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;

    const LATEST: &str = "/repos/home-space-app/home-space/releases/latest";

    async fn github(response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(LATEST))
            .respond_with(response)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn reads_the_version_without_the_v_and_the_page() {
        let server = github(ResponseTemplate::new(200).set_body_json(json!({
            "tag_name": "v0.14.0",
            "html_url": "https://github.com/home-space-app/home-space/releases/tag/v0.14.0",
            "draft": false
        })))
        .await;

        let release = latest_release(&format!("{}{LATEST}", server.uri())).await;

        assert_eq!(
            release,
            Some(Release {
                version: "0.14.0".to_owned(),
                url: "https://github.com/home-space-app/home-space/releases/tag/v0.14.0".to_owned(),
            })
        );
    }

    #[tokio::test]
    async fn a_rate_limited_request_is_none() {
        let server = github(ResponseTemplate::new(403).set_body_json(json!({
            "message": "API rate limit exceeded"
        })))
        .await;

        assert_eq!(
            latest_release(&format!("{}{LATEST}", server.uri())).await,
            None
        );
    }

    #[tokio::test]
    async fn a_body_that_is_not_a_release_is_none() {
        let server = github(ResponseTemplate::new(200).set_body_string("<html>")).await;

        assert_eq!(
            latest_release(&format!("{}{LATEST}", server.uri())).await,
            None
        );
    }
}
