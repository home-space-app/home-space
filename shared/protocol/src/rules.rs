//! Rules for the values clients send, shared by request validation and the server's services
//! so both refuse exactly the same things. Clients may use them to check input before sending.

/// The longest username, in bytes.
pub const MAX_USERNAME_LEN: usize = 64;
/// The shortest password, in bytes.
pub const MIN_PASSWORD_LEN: usize = 8;
/// The longest password, in bytes. Hashing takes time in proportion to the length, and
/// nothing anyone types or a password manager makes comes near it.
pub const MAX_PASSWORD_LEN: usize = 1024;
/// The longest device name, in characters, after surrounding blanks are removed.
pub const MAX_DEVICE_NAME_CHARS: usize = 100;
/// The longest file or folder name, in characters.
pub const MAX_NODE_NAME_CHARS: usize = 255;
/// The longest name stored inside a protected folder, in characters: the sealed text of a
/// name of [`MAX_NODE_NAME_CHARS`] characters (base64url of its padded AES-SIV sealing, see
/// `vault-core`). The server allows names this long inside protected folders only.
pub const MAX_SEALED_NODE_NAME_CHARS: usize = 1387;
/// The most items one share holds, not counting what is inside shared folders.
pub const MAX_SHARE_ITEMS: usize = 1000;
/// The most users one share is for.
pub const MAX_SHARE_RECIPIENTS: usize = 100;
/// The most shared items copied into one's own files at once.
pub const MAX_COPY_ITEMS: usize = 1000;

/// Why `username` cannot be used, if it cannot.
pub fn username_problem(username: &str) -> Option<&'static str> {
    (username.is_empty() || username.len() > MAX_USERNAME_LEN)
        .then_some("username must be 1-64 characters")
}

/// Why `password` cannot be set, if it cannot: it must be 8 to 1024 characters.
pub fn password_problem(password: &str) -> Option<&'static str> {
    if password.len() < MIN_PASSWORD_LEN {
        Some("password must be at least 8 characters")
    } else if password.len() > MAX_PASSWORD_LEN {
        Some("password must be at most 1024 characters")
    } else {
        None
    }
}

/// `name` as a device name, without surrounding blanks, or `None` when it is blank or too long.
pub fn device_name(name: &str) -> Option<&str> {
    let name = name.trim();
    (1..=MAX_DEVICE_NAME_CHARS)
        .contains(&name.chars().count())
        .then_some(name)
}

/// Why `name` cannot name a file or folder, if it cannot.
pub fn node_name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() || name.chars().count() > MAX_NODE_NAME_CHARS {
        Some("a name must be 1-255 characters")
    } else {
        name_characters_problem(name)
    }
}

/// Why `name` cannot name a file or folder inside a protected folder, if it cannot: the
/// rules of [`node_name_problem`] with room for a sealed name, up to
/// [`MAX_SEALED_NODE_NAME_CHARS`] characters. A request cannot say whether its folder is
/// protected, so request validation allows this much and the server refuses a name longer than
/// [`MAX_NODE_NAME_CHARS`] outside protected folders.
pub fn sealed_node_name_problem(name: &str) -> Option<&'static str> {
    if name.is_empty() || name.chars().count() > MAX_SEALED_NODE_NAME_CHARS {
        Some("a name must be 1-255 characters, or 1-1387 inside a protected folder")
    } else {
        name_characters_problem(name)
    }
}

/// What both kinds of name refuse besides their length.
fn name_characters_problem(name: &str) -> Option<&'static str> {
    if name == "." || name == ".." {
        Some("a name cannot be . or ..")
    } else if name.contains(['/', '\\', '\0']) {
        Some("a name cannot contain /, \\ or a NUL character")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_names_follow_the_database_rule() {
        for name in [
            "a",
            "report.pdf",
            "ünïcode",
            &"é".repeat(MAX_NODE_NAME_CHARS),
            ".a",
        ] {
            assert_eq!(node_name_problem(name), None, "{name}");
        }
        let too_long = "a".repeat(MAX_NODE_NAME_CHARS + 1);
        for name in ["", ".", "..", "a/b", "a\\b", "a\0b", &too_long] {
            assert!(node_name_problem(name).is_some(), "{name:?}");
        }
    }

    #[test]
    fn names_in_protected_folders_may_be_as_long_as_a_sealed_name() {
        let longest = "a".repeat(MAX_SEALED_NODE_NAME_CHARS);
        let plain = "é".repeat(MAX_NODE_NAME_CHARS + 1);
        for name in ["a", "tj7VHBiK2oF", plain.as_str(), &longest] {
            assert_eq!(sealed_node_name_problem(name), None, "{name}");
        }
        assert!(node_name_problem(&plain).is_some());
        assert!(node_name_problem(&longest).is_some());
        let too_long = "a".repeat(MAX_SEALED_NODE_NAME_CHARS + 1);
        for name in ["", ".", "..", "a/b", "a\\b", "a\0b", &too_long] {
            assert!(sealed_node_name_problem(name).is_some(), "{name:?}");
        }
    }

    #[test]
    fn device_names_are_trimmed() {
        assert_eq!(device_name("  Laptop "), Some("Laptop"));
        assert_eq!(device_name("   "), None);
        assert_eq!(device_name(&"x".repeat(MAX_DEVICE_NAME_CHARS + 1)), None);
    }

    #[test]
    fn passwords_are_8_to_1024_bytes() {
        assert!(password_problem("short1").is_some());
        assert!(password_problem("correct horse").is_none());
        assert!(password_problem(&"a".repeat(MAX_PASSWORD_LEN + 1)).is_some());
    }
}
