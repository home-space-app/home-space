//! Picking people on the server.

use protocol::UserRef;

/// The users whose name contains `query`, ignoring case.
pub fn matching<'a>(users: &'a [UserRef], query: &str) -> Vec<&'a UserRef> {
    let query = query.trim().to_lowercase();
    users
        .iter()
        .filter(|user| query.is_empty() || user.username.to_lowercase().contains(&query))
        .collect()
}

#[cfg(test)]
mod tests {
    use protocol::UserId;
    use uuid::Uuid;

    use super::*;

    fn user(raw: u128, name: &str) -> UserRef {
        UserRef {
            id: UserId(Uuid::from_u128(raw)),
            username: name.to_owned(),
        }
    }

    #[test]
    fn people_are_filtered_by_name_ignoring_case() {
        let users = [user(1, "dana"), user(2, "Alex"), user(3, "maria")];
        let names = |query| {
            matching(&users, query)
                .into_iter()
                .map(|user| user.username.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(""), ["dana", "Alex", "maria"]);
        assert_eq!(names("a"), ["dana", "Alex", "maria"]);
        assert_eq!(names(" AL "), ["Alex"]);
        assert!(names("zed").is_empty());
    }
}
