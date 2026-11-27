//! Stable identities for executable titles.
//!
//! A title ID is read from a structurally validated NCCH header (NCSD) or
//! from the CIA TMD after it has been matched against the boot NCCH. It is
//! deliberately distinct from a file path or display name.

const KEY_PREFIX: &str = "title-v1:";

/// Stable title-level identity when the container exposes a non-zero title ID.
///
/// The identity intentionally excludes the container path and release version:
/// moving/renaming a file or installing a title update must not create a new
/// per-title preference namespace. Homebrew formats without a validated title
/// ID remain unidentified until a separate content-fingerprint policy exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GameIdentity {
    title_id: u64,
}

impl GameIdentity {
    /// Creates an identity only for a non-zero title ID.
    pub const fn from_title_id(title_id: u64) -> Option<Self> {
        if title_id == 0 {
            None
        } else {
            Some(Self { title_id })
        }
    }

    /// Returns the title ID exposed by the validated container metadata.
    pub const fn title_id(self) -> u64 {
        self.title_id
    }

    /// Returns a versioned, line-safe key suitable for settings files.
    pub fn stable_key(self) -> String {
        format!("{KEY_PREFIX}{:016x}", self.title_id)
    }

    /// Parses only the exact versioned title-ID key format.
    pub fn parse_stable_key(value: &str) -> Option<Self> {
        let encoded = value.strip_prefix(KEY_PREFIX)?;
        if encoded.len() != 16 || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let title_id = u64::from_str_radix(encoded, 16).ok()?;
        Self::from_title_id(title_id)
    }
}

#[cfg(test)]
mod tests {
    use super::GameIdentity;

    #[test]
    fn stable_key_round_trips_the_title_id() {
        let identity = GameIdentity::from_title_id(0x0004_0000_1234_5600).unwrap();
        assert_eq!(identity.stable_key(), "title-v1:0004000012345600");
        assert_eq!(
            GameIdentity::parse_stable_key(&identity.stable_key()),
            Some(identity)
        );
    }

    #[test]
    fn parser_accepts_uppercase_hex_without_changing_identity() {
        assert_eq!(
            GameIdentity::parse_stable_key("title-v1:00040000ABCDEF01")
                .unwrap()
                .title_id(),
            0x0004_0000_ABCD_EF01
        );
    }

    #[test]
    fn zero_and_malformed_keys_are_not_identities() {
        assert_eq!(GameIdentity::from_title_id(0), None);
        for key in [
            "",
            "0004000012345600",
            "title-v2:0004000012345600",
            "title-v1:00040000123456",
            "title-v1:00040000123456000",
            "title-v1:00040000123456xz",
            "title-v1:0000000000000000",
        ] {
            assert_eq!(GameIdentity::parse_stable_key(key), None, "{key}");
        }
    }

    #[test]
    fn different_title_ids_never_share_a_key() {
        let first = GameIdentity::from_title_id(0x0004_0000_1234_5600).unwrap();
        let second = GameIdentity::from_title_id(0x0004_0000_1234_5601).unwrap();
        assert_ne!(first, second);
        assert_ne!(first.stable_key(), second.stable_key());
    }
}
