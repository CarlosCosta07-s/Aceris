//! NCCH Access Control Info service-access lists.
//!
//! The ARM11 local system capabilities area contains a fixed service-access
//! list of 32 eight-byte names and, on newer system versions, an extended
//! list of two additional eight-byte names. Entries are retained byte-for-byte
//! so parsing never normalizes a guest-provided service name.
//!
//! This module only parses the ACI bytes. Signature/access-descriptor
//! validation and enforcement by the SRV service manager belong to the NCCH
//! loader and process/service lifecycle respectively.

/// Size of one service name in the NCCH service-access list.
pub const SERVICE_NAME_SIZE: usize = 8;

/// Size of the complete ARM11 Access Control Info structure.
pub const ACCESS_CONTROL_INFO_SIZE: usize = 0x200;

/// Offset of the regular service-access list in ARM11 local system capabilities.
pub const SERVICE_ACCESS_LIST_OFFSET: usize = 0x50;

/// Number of regular service-access entries.
pub const SERVICE_ACCESS_LIST_COUNT: usize = 32;

/// Offset of the extended service-access list in ARM11 local system capabilities.
pub const EXTENDED_SERVICE_ACCESS_LIST_OFFSET: usize = 0x150;

/// Number of extended service-access entries.
pub const EXTENDED_SERVICE_ACCESS_LIST_COUNT: usize = 2;

/// One fixed-width service name from the exheader.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ServiceName {
    bytes: [u8; SERVICE_NAME_SIZE],
}

impl ServiceName {
    /// Builds a service name from a guest/service-manager string.
    ///
    /// Names are case-sensitive and occupy at most eight ASCII bytes. Shorter
    /// names are padded with NUL bytes to the fixed-width exheader encoding.
    pub fn from_ascii(name: &str) -> Option<Self> {
        let source = name.as_bytes();
        if source.is_empty()
            || source.len() > SERVICE_NAME_SIZE
            || source.iter().any(|byte| !(0x21..=0x7E).contains(byte))
        {
            return None;
        }

        let mut bytes = [0; SERVICE_NAME_SIZE];
        bytes[..source.len()].copy_from_slice(source);
        Some(Self { bytes })
    }

    /// Returns the exact eight bytes stored in the exheader.
    pub const fn as_bytes(&self) -> &[u8; SERVICE_NAME_SIZE] {
        &self.bytes
    }

    /// Returns the name as ASCII when it is a valid NUL-padded service name.
    ///
    /// Eight-character names need no terminator. An embedded NUL followed by
    /// nonzero data or non-ASCII data is preserved by the parser but is not
    /// exposed as a normalized string.
    pub fn as_ascii(&self) -> Option<&str> {
        let end = self
            .bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(SERVICE_NAME_SIZE);
        if end == 0
            || self.bytes[..end]
                .iter()
                .any(|byte| !(0x21..=0x7E).contains(byte))
            || self.bytes[end..].iter().any(|byte| *byte != 0)
        {
            return None;
        }
        std::str::from_utf8(&self.bytes[..end]).ok()
    }

    /// Whether this slot is the all-zero unused entry.
    pub fn is_empty(&self) -> bool {
        self.bytes.iter().all(|byte| *byte == 0)
    }
}

/// Parsed regular and extended service-access lists from one ARM11 ACI.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceAccessControl {
    regular: Vec<ServiceName>,
    extended: Vec<ServiceName>,
}

impl ServiceAccessControl {
    /// Parses the two service-access lists from the 0x200-byte ACI structure.
    ///
    /// Unused all-zero slots are omitted. Nonzero slots remain byte-exact,
    /// including any unusual encoding; this parser does not silently repair
    /// malformed names or apply service permissions.
    pub fn parse(access_control_info: &[u8; ACCESS_CONTROL_INFO_SIZE]) -> Self {
        let regular = parse_name_slots(
            access_control_info,
            SERVICE_ACCESS_LIST_OFFSET,
            SERVICE_ACCESS_LIST_COUNT,
        );
        let extended = parse_name_slots(
            access_control_info,
            EXTENDED_SERVICE_ACCESS_LIST_OFFSET,
            EXTENDED_SERVICE_ACCESS_LIST_COUNT,
        );
        Self { regular, extended }
    }

    /// Returns the regular 32-entry service-access list with unused slots removed.
    pub fn regular_services(&self) -> &[ServiceName] {
        &self.regular
    }

    /// Returns the two-entry extended service-access list with unused slots removed.
    pub fn extended_services(&self) -> &[ServiceName] {
        &self.extended
    }

    /// Iterates over regular and extended service names in exheader order.
    pub fn services(&self) -> impl Iterator<Item = &ServiceName> {
        self.regular.iter().chain(self.extended.iter())
    }

    /// Checks an exact, case-sensitive service name encoded as guest bytes.
    ///
    /// The input must contain one to eight printable ASCII bytes and must not
    /// include an explicit terminator; the fixed-width exheader slot supplies
    /// the remaining NUL padding.
    pub fn allows_bytes(&self, service_name: &[u8]) -> bool {
        if service_name.is_empty()
            || service_name.len() > SERVICE_NAME_SIZE
            || service_name
                .iter()
                .any(|byte| !(0x21..=0x7E).contains(byte))
        {
            return false;
        }

        let mut requested = [0; SERVICE_NAME_SIZE];
        requested[..service_name.len()].copy_from_slice(service_name);
        self.services().any(|entry| entry.as_bytes() == &requested)
    }

    /// Checks an exact, case-sensitive service-name match against either list.
    pub fn allows(&self, service_name: &str) -> bool {
        self.allows_bytes(service_name.as_bytes())
    }

    /// Number of nonempty service entries across both lists.
    pub fn len(&self) -> usize {
        self.regular.len() + self.extended.len()
    }

    /// Whether the ACI contains no service entries.
    pub fn is_empty(&self) -> bool {
        self.regular.is_empty() && self.extended.is_empty()
    }
}

fn parse_name_slots(
    access_control_info: &[u8; ACCESS_CONTROL_INFO_SIZE],
    offset: usize,
    count: usize,
) -> Vec<ServiceName> {
    let end = offset + count * SERVICE_NAME_SIZE;
    access_control_info[offset..end]
        .chunks_exact(SERVICE_NAME_SIZE)
        .filter_map(|slot| {
            let bytes: [u8; SERVICE_NAME_SIZE] = slot.try_into().ok()?;
            let name = ServiceName { bytes };
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        ACCESS_CONTROL_INFO_SIZE, EXTENDED_SERVICE_ACCESS_LIST_OFFSET, SERVICE_ACCESS_LIST_OFFSET,
        SERVICE_NAME_SIZE, ServiceAccessControl, ServiceName,
    };

    #[test]
    fn parses_regular_and_extended_service_names_without_changing_bytes() {
        let mut aci = [0; ACCESS_CONTROL_INFO_SIZE];
        aci[SERVICE_ACCESS_LIST_OFFSET..SERVICE_ACCESS_LIST_OFFSET + SERVICE_NAME_SIZE]
            .copy_from_slice(b"gsp::Gpu");
        aci[SERVICE_ACCESS_LIST_OFFSET + SERVICE_NAME_SIZE
            ..SERVICE_ACCESS_LIST_OFFSET + 2 * SERVICE_NAME_SIZE]
            .copy_from_slice(b"cfg:u\0\0\0");
        aci[EXTENDED_SERVICE_ACCESS_LIST_OFFSET
            ..EXTENDED_SERVICE_ACCESS_LIST_OFFSET + SERVICE_NAME_SIZE]
            .copy_from_slice(b"csnd:SND");

        let parsed = ServiceAccessControl::parse(&aci);

        assert_eq!(parsed.regular_services().len(), 2);
        assert_eq!(parsed.extended_services().len(), 1);
        assert_eq!(parsed.regular_services()[0].as_bytes(), b"gsp::Gpu");
        assert_eq!(parsed.regular_services()[1].as_ascii(), Some("cfg:u"));
        assert_eq!(parsed.extended_services()[0].as_ascii(), Some("csnd:SND"));
        assert_eq!(parsed.len(), 3);
    }

    #[test]
    fn ignores_only_all_zero_slots_and_does_not_compact_or_rewrite_names() {
        let mut aci = [0; ACCESS_CONTROL_INFO_SIZE];
        let slot = SERVICE_ACCESS_LIST_OFFSET + 7 * SERVICE_NAME_SIZE;
        aci[slot..slot + SERVICE_NAME_SIZE].copy_from_slice(b"apt:U\0\0\0");

        let parsed = ServiceAccessControl::parse(&aci);

        assert_eq!(parsed.regular_services().len(), 1);
        assert_eq!(parsed.regular_services()[0].as_ascii(), Some("apt:U"));
        assert_eq!(parsed.regular_services()[0].as_bytes(), b"apt:U\0\0\0");
    }

    #[test]
    fn membership_is_exact_case_sensitive_and_checks_both_lists() {
        let mut aci = [0; ACCESS_CONTROL_INFO_SIZE];
        aci[SERVICE_ACCESS_LIST_OFFSET..SERVICE_ACCESS_LIST_OFFSET + SERVICE_NAME_SIZE]
            .copy_from_slice(b"hid:USER");
        aci[EXTENDED_SERVICE_ACCESS_LIST_OFFSET
            ..EXTENDED_SERVICE_ACCESS_LIST_OFFSET + SERVICE_NAME_SIZE]
            .copy_from_slice(b"am:net\0\0");

        let parsed = ServiceAccessControl::parse(&aci);

        assert!(parsed.allows("hid:USER"));
        assert!(parsed.allows("am:net"));
        assert!(parsed.allows_bytes(b"hid:USER"));
        assert!(!parsed.allows_bytes(b"hid:user"));
        assert!(!parsed.allows_bytes(b"hid:USER\0"));
        assert!(!parsed.allows("hid:user"));
        assert!(!parsed.allows("hid:"));
        assert!(!parsed.allows("service-name-too-long"));
    }

    #[test]
    fn rejects_invalid_guest_query_names_without_rejecting_the_aci() {
        let mut aci = [0; ACCESS_CONTROL_INFO_SIZE];
        aci[SERVICE_ACCESS_LIST_OFFSET..SERVICE_ACCESS_LIST_OFFSET + SERVICE_NAME_SIZE]
            .copy_from_slice(b"fs:USER\0");
        let parsed = ServiceAccessControl::parse(&aci);

        assert!(!parsed.allows(""));
        assert!(!parsed.allows("fs:USER\0"));
        assert!(!parsed.allows("é"));
        assert_eq!(parsed.regular_services()[0].as_ascii(), Some("fs:USER"));
    }

    #[test]
    fn all_zero_aci_has_no_allowed_services() {
        let parsed = ServiceAccessControl::parse(&[0; ACCESS_CONTROL_INFO_SIZE]);
        assert!(parsed.is_empty());
        assert_eq!(parsed.len(), 0);
        assert_eq!(parsed.services().count(), 0);
        assert_eq!(ServiceName::from_ascii(""), None);
    }

    #[test]
    fn preserves_non_ascii_or_malformed_nonzero_slots_for_diagnostics() {
        let mut aci = [0; ACCESS_CONTROL_INFO_SIZE];
        let slot = SERVICE_ACCESS_LIST_OFFSET;
        aci[slot..slot + SERVICE_NAME_SIZE].copy_from_slice(&[b'a', 0, b'b', 0, 0, 0, 0, 0]);
        let parsed = ServiceAccessControl::parse(&aci);

        assert_eq!(parsed.regular_services().len(), 1);
        assert_eq!(parsed.regular_services()[0].as_ascii(), None);
        assert_eq!(
            parsed.regular_services()[0].as_bytes(),
            &[b'a', 0, b'b', 0, 0, 0, 0, 0]
        );
    }
}
