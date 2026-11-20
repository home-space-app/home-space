//! The address the browser comes back to after single sign-on.

use url::Url;

/// What the identity provider answered, from the query of the address it sent the browser to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SsoReply {
    /// The login code to exchange for a session.
    Code(String),
    /// Why the provider did not sign the user in, e.g. `access_denied`.
    Error(String),
}

/// The `code` or `error` in `url`'s query; a non-empty code wins. `None` when it has neither.
pub fn sso_reply(url: &Url) -> Option<SsoReply> {
    let (mut code, mut error) = (None, None);
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }
    match (code, error) {
        (Some(code), _) if !code.is_empty() => Some(SsoReply::Code(code)),
        (_, Some(error)) => Some(SsoReply::Error(error)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(url: &str) -> Option<SsoReply> {
        sso_reply(&Url::parse(url).expect("url"))
    }

    #[test]
    fn a_code_is_read_decoded() {
        assert_eq!(
            reply("http://x/cb?code=a%20b&error=late"),
            Some(SsoReply::Code("a b".to_owned()))
        );
    }

    #[test]
    fn an_error_is_read_when_there_is_no_code() {
        assert_eq!(
            reply("http://x/cb?code=&error=access_denied"),
            Some(SsoReply::Error("access_denied".to_owned()))
        );
    }

    #[test]
    fn neither_answers_nothing() {
        assert_eq!(reply("http://x/cb?state=1"), None);
        assert_eq!(reply("http://x/cb?code="), None);
    }
}
