//! Read access for the private Cycle Sentinel telemetry surface.
//!
//! This allowlist grants read-only access. Governance signers are also allowed
//! to read private telemetry by `require_telemetry_viewer`; visibility still
//! does not grant permission to operate the Sentinel.

use candid::Principal;

const TELEMETRY_VIEWERS: [&str; 3] = [
    "zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae",
    "stzp3-bnvwm-zqzjh-o6mv6-ci53m-wj5k6-xyhe7-fnyp2-c64o3-7vokj-bqe",
    "4alqm-afk6k-bybok-qvdyo-cnv7y-klel6-xm2pz-7h7jk-utmys-kttf3-vqe",
];

pub(crate) fn is_telemetry_viewer(caller: Principal) -> bool {
    if caller == Principal::anonymous() {
        return false;
    }
    let caller_text = caller.to_text();
    TELEMETRY_VIEWERS.contains(&caller_text.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_principals_are_valid_unique_and_not_anonymous() {
        let parsed: Vec<_> = TELEMETRY_VIEWERS
            .iter()
            .map(|text| Principal::from_text(text).expect("valid viewer principal"))
            .collect();
        assert_eq!(parsed.len(), 3);
        assert_eq!(
            parsed
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            3
        );
        assert!(!parsed.contains(&Principal::anonymous()));
    }

    #[test]
    fn only_listed_principals_can_read_private_telemetry() {
        for text in TELEMETRY_VIEWERS {
            assert!(is_telemetry_viewer(Principal::from_text(text).unwrap()));
        }
        assert!(!is_telemetry_viewer(Principal::anonymous()));
        assert!(!is_telemetry_viewer(Principal::from_slice(&[0x42])));
    }
}
