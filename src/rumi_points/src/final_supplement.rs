//! One atomic, separately receipted Season 1 final-period compensation.
use crate::{
    outage_reconciliation::{OutageCredit, OutageCreditKind, OutageCreditPage},
    state,
};
use candid::{CandidType, Principal};
use ic_stable_structures::{storable::Bound, StableCell, Storable};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, cell::RefCell};
const START: u64 = 1792972800000000000;
const END: u64 = 1793491199000000000;
const TOTAL: u128 = 55971020305749;
const EVIDENCE: [u8; 32] = [
    219, 231, 33, 248, 33, 253, 106, 48, 159, 84, 252, 249, 166, 215, 84, 205, 69, 227, 139, 138,
    251, 52, 54, 181, 251, 22, 172, 69, 215, 76, 242, 125,
];
const ALLOCATION: [u8; 32] = [
    166, 6, 85, 53, 42, 103, 5, 195, 125, 117, 160, 255, 2, 115, 4, 18, 230, 92, 89, 100, 195, 32,
    218, 60, 147, 199, 99, 205, 22, 175, 218, 119,
];
// Domain-specific immutable grant: (principal, exact pre-credit total, supplement).
const ROWS: &[(&str, u128, u128)] = &[
    (
        "2bcfz-dnfki-nq7q2-v4gmo-4kvkg-3dabg-yn5it-ivcs5-ikpss-kk77p-lae",
        2055780112255,
        48434245255,
    ),
    (
        "4alqm-afk6k-bybok-qvdyo-cnv7y-klel6-xm2pz-7h7jk-utmys-kttf3-vqe",
        5592932842,
        0,
    ),
    (
        "6ixwn-zfdck-dvelg-ng2um-zaoqj-frt7r-ojyox-4hhb2-cexnt-3wsf5-pqe",
        62648695432628,
        4871761544468,
    ),
    (
        "aja7b-6aown-ucyjz-a6ddu-w67th-fuok4-vxqmi-24d76-74igd-rkgrp-5ae",
        147473714593,
        876213499,
    ),
    (
        "akzqy-74twk-ctls2-jufen-kygii-cev4l-w5w62-foeml-rmgye-xtwme-uae",
        24102899476821,
        2434969599794,
    ),
    (
        "cgsme-y5yno-hrlam-ws6he-aryy7-s5m43-6sm4a-4uqds-avudn-tyubn-7qe",
        60373558647598,
        15188947409540,
    ),
    (
        "hwnsp-qozsc-pmxto-c3m27-noe32-v45gq-lbo3z-ztkd3-l4e4x-st4bp-wqe",
        96499078571,
        4249998633,
    ),
    (
        "ivhvf-a2nxr-t436u-6t3g3-eckj2-gd6ky-q575b-37ciq-7r43a-62nav-wqe",
        488991782682,
        5599601074,
    ),
    (
        "khkrd-hwwv7-2m5k4-n6tyi-qqasb-yay5e-dpbtf-2abbf-bzji2-zrvey-dae",
        121279349988563,
        24642385496128,
    ),
    (
        "nqh27-eplay-bkspf-s4aho-uhvlu-mvwgx-gepgb-6jb3h-tm5qg-egucg-7qe",
        1430783986591,
        818099369,
    ),
    (
        "sorbk-tm3y3-uzhwj-d45hv-2qquh-tdlln-s3qna-etnov-sgx32-qsvci-kae",
        5452786255,
        0,
    ),
    (
        "stzp3-bnvwm-zqzjh-o6mv6-ci53m-wj5k6-xyhe7-fnyp2-c64o3-7vokj-bqe",
        5452786362,
        0,
    ),
    (
        "tjsut-lqpan-5xkiw-dq3j4-xpp53-uhhy2-dyam5-vtrcx-aadcg-bvxsx-6ae",
        32707880912399,
        5263158278713,
    ),
    (
        "ulapy-ysfkn-fobdy-nif4d-s5y22-s4wlq-bqkxm-4iznv-4r5qo-begk2-hae",
        6481630490,
        52107439,
    ),
    (
        "zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae",
        124054833934753,
        3509767711837,
    ),
];
#[derive(Serialize, Deserialize, Clone, Default)]
enum Stored {
    #[default]
    Empty,
    V1(Vec<OutageCredit>),
}
impl Storable for Stored {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        let mut b = Vec::new();
        ciborium::ser::into_writer(self, &mut b).expect("encode final supplement");
        Cow::Owned(b)
    }
    fn from_bytes(b: Cow<'_, [u8]>) -> Self {
        ciborium::de::from_reader(b.as_ref()).expect("decode final supplement")
    }
    const BOUND: Bound = Bound::Unbounded;
}
thread_local! {static RECEIPTS:RefCell<StableCell<Stored,state::VMem>>=RefCell::new(StableCell::init(state::final_supplement_memory(),Stored::Empty).expect("initialize final supplement"));}
fn receipts() -> Vec<OutageCredit> {
    RECEIPTS.with(|c| match c.borrow().get() {
        Stored::Empty => vec![],
        Stored::V1(v) => v.clone(),
    })
}
#[derive(CandidType, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub complete: bool,
    pub rows: u32,
    pub credited_points: u128,
    pub evidence_digest: Vec<u8>,
}
pub fn status() -> Status {
    let r = receipts();
    Status {
        complete: r.len() == ROWS.len(),
        rows: r.len() as u32,
        credited_points: r.iter().map(|r| r.credited_points).sum(),
        evidence_digest: EVIDENCE.to_vec(),
    }
}
pub fn apply(caller: Principal, now: u64) -> Result<Status, String> {
    let old = crate::outage_reconciliation::status();
    let prior = old.audit_count_matches_receipts
        && old.progress.is_some_and(|p| {
            p.complete && p.rows_applied == 45 && p.credited_points == 93265666151589
        });
    apply_with_prior(caller, now, prior)
}
fn apply_with_prior(caller: Principal, now: u64, prior: bool) -> Result<Status, String> {
    if caller == Principal::anonymous() || !state::is_admin(caller) {
        return Err("unauthorized".into());
    }
    if status().complete {
        return Ok(status());
    }
    let _epoch = state::EpochGuard::new().ok_or("epoch transition active")?;
    let _poll = state::PollGuard::new().ok_or("poll active")?;
    if state::season_bounds() != (1780272000000000000, END)
        || now <= END
        || state::get_open_epoch().is_some()
        || state::current_epoch_index() != 21
        || state::point_ledger_len() != 493
        || state::registered_count() != 15
    {
        return Err("final season prestate changed".into());
    }
    if !prior {
        return Err("earlier compensation not reconciled".into());
    }
    if !receipts().is_empty() {
        return Err("unexpected partial final supplement".into());
    }
    let parsed: Vec<_> = ROWS
        .iter()
        .map(|(p, b, a)| (Principal::from_text(p).expect("pinned principal"), *b, *a))
        .collect();
    for (p, b, a) in &parsed {
        let ps = state::get_principal_state(p).ok_or("missing participant")?;
        if state::is_excluded(p)
            || ps.total_points != *b
            || ps.last_epoch_processed != 20
            || ps.registered_at_ns >= START
        {
            return Err("participant prestate changed".into());
        }
        state::preflight_outage_credit(p, *a)?;
    }
    if parsed.iter().map(|(_, _, a)| *a).sum::<u128>() != TOTAL {
        return Err("grant total mismatch".into());
    }
    // No await or recoverable failure after the first write: totals and receipts
    // commit in one message or roll back together. Retrying returns the receipt.
    let mut audit = Vec::new();
    for (p, _, a) in parsed {
        state::commit_outage_credit(&p, a);
        audit.push(OutageCredit {
            kind: OutageCreditKind::OutageMakeWhole,
            allocation_digest: ALLOCATION,
            evidence_digest: EVIDENCE,
            principal: p,
            epoch_index: 21,
            scheduled_period_start_ns: START,
            scheduled_period_end_ns: END,
            coverage_start_ns: Some(START),
            coverage_end_ns: Some(END),
            normal_ledger_cutoff: 493,
            normal_points_earned: 0,
            target_epoch_points: a,
            credited_points: a,
            applied_at_ns: now,
        });
    }
    RECEIPTS.with(|c| {
        c.borrow_mut()
            .set(Stored::V1(audit))
            .expect("persist final supplement")
    });
    state::set_epoch_driver_enabled(caller, false).expect("validated admin");
    state::set_poll_enabled(caller, false).expect("validated admin");
    Ok(status())
}
pub fn public_count() -> u64 {
    crate::outage_reconciliation::credit_count() + receipts().len() as u64
}
pub fn public_page(offset: u64, limit: u32) -> OutageCreditPage {
    let end = public_count();
    let stop = offset
        .min(end)
        .saturating_add(limit.min(100) as u64)
        .min(end);
    let legacy = crate::outage_reconciliation::credit_count();
    let new = receipts();
    let mut entries = vec![];
    for i in offset.min(end)..stop {
        entries.push(if i < legacy {
            crate::outage_reconciliation::credit_entries(i, 1)
                .entries
                .remove(0)
        } else {
            new[(i - legacy) as usize].clone()
        });
    }
    OutageCreditPage {
        entries,
        next_offset: stop,
        reached_end: stop == end,
    }
}
pub fn public_principal_page(p: Principal, offset: u64, limit: u32) -> OutageCreditPage {
    let total = public_count();
    let mut cursor = offset.min(total);
    let mut entries = vec![];
    let wanted = limit.min(100) as usize;
    for r in public_page(cursor, 100).entries {
        if entries.len() == wanted {
            break;
        }
        cursor += 1;
        if r.principal == p {
            entries.push(r)
        }
    }
    OutageCreditPage {
        entries,
        next_offset: cursor,
        reached_end: cursor == total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn immutable_grant_totals_and_receipt_storage_roundtrip() {
        let principals: std::collections::BTreeSet<_> = ROWS
            .iter()
            .map(|(p, _, _)| Principal::from_text(p).unwrap())
            .collect();
        assert_eq!(principals.len(), 15);
        assert_eq!(
            ROWS.iter().map(|(_, before, _)| before).sum::<u128>(),
            429409727203403
        );
        assert_eq!(
            ROWS.iter().map(|(_, _, amount)| amount).sum::<u128>(),
            TOTAL
        );
        let encoded = Stored::Empty.to_bytes().into_owned();
        assert!(matches!(
            Stored::from_bytes(Cow::Owned(encoded)),
            Stored::Empty
        ));
    }
    #[test]
    fn anonymous_and_changed_prestate_cannot_credit() {
        let admin = Principal::self_authenticating(b"final-supplement-admin");
        state::init_state(None, admin);
        assert_eq!(
            apply(Principal::anonymous(), END + 1),
            Err("unauthorized".into())
        );
        assert_eq!(
            apply(admin, END + 1),
            Err("final season prestate changed".into())
        );
        assert_eq!(status().credited_points, 0);
        assert_eq!(status().rows, 0);
    }
}

#[cfg(test)]
mod application_tests {
    use super::*;
    #[test]
    fn successful_credit_persists_and_retry_adds_zero() {
        let admin = Principal::self_authenticating(b"final-supplement-application");
        state::init_state(
            Some(crate::types::InitArgs {
                season_start_ns: Some(1780272000000000000),
                season_end_ns: Some(END),
                ..Default::default()
            }),
            admin,
        );
        state::with_state_mut(|s| s.current_epoch_index = 21);
        for (p, b, _) in ROWS {
            let principal = Principal::from_text(p).unwrap();
            let mut ps = crate::types::PrincipalState::new(
                principal,
                1781000000000000000,
                crate::types::QualifyingAction::MintIcUsd,
            );
            ps.total_points = *b;
            ps.last_epoch_processed = 20;
            state::test_put_final_participant(ps);
        }
        for _ in 0..493 {
            state::append_point_entry(crate::types::PointEntry {
                principal: admin,
                epoch_index: 0,
                points_delta: 0,
                source: crate::types::PointSource::Registration,
                recorded_at_ns: 0,
            });
        }
        assert!(apply_with_prior(admin, END + 1, false).is_err());
        let first = apply_with_prior(admin, END + 1, true).unwrap();
        assert!(first.complete);
        assert_eq!(first.credited_points, TOTAL);
        let stored = Stored::V1(receipts());
        let restored = Stored::from_bytes(Cow::Owned(stored.to_bytes().into_owned()));
        let Stored::V1(r) = restored else {
            panic!("lost populated receipt")
        };
        assert_eq!(r.len(), 15);
        assert_eq!(r.iter().map(|r| r.credited_points).sum::<u128>(), TOTAL);
        let again = apply_with_prior(admin, END + 2, true).unwrap();
        assert_eq!(again, first);
        for (p, b, a) in ROWS {
            assert_eq!(
                state::get_principal_state(&Principal::from_text(p).unwrap())
                    .unwrap()
                    .total_points,
                b + a
            );
        }
        assert_eq!(state::point_ledger_len(), 493);
    }
}
