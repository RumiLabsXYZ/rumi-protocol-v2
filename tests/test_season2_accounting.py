import copy
import unittest

from scripts.season2_accounting import (
    CAMPAIGN_START_NS,
    CATEGORY_BITS,
    NANOS_PER_DAY,
    AccountingError,
    calculate,
)


def inputs(**changes):
    base = {
        "vault_debt": 0,
        "recorded_3pool_icusd": 0,
        "recorded_3pool_usdc": 0,
        "recorded_3pool_usdt": 0,
        "verified_3usd": 0,
        "sp_icusd": 0,
        "sp_3usd": 0,
        "amm_lp_value": 0,
    }
    base.update(changes)
    return base


def participant(end_ns, interval, *, registered_at_ns=CAMPAIGN_START_NS, repayments=None):
    return {
        "principal_id": "fake-principal-01",
        "registered_at_ns": registered_at_ns,
        "intervals": [interval],
        "repayments": repayments or [],
    }


def event_interval(start, end, values, known):
    return {
        "start_ns": start,
        "end_ns": end,
        "known_categories_mask": sum(CATEGORY_BITS[name] for name in known),
        "inputs": values,
    }


def snapshot_interval(start, end, a, b, known):
    return {
        "start_ns": start,
        "end_ns": end,
        "known_categories_mask": sum(CATEGORY_BITS[name] for name in known),
        "a": a,
        "b": b,
    }


def manifest(method, cutoff, people):
    return {
        "method": method,
        "campaign_start_ns": CAMPAIGN_START_NS,
        "cutoff_ns": cutoff,
        "eligible_repayment_ledgers": {"ckusdc": "xevnm-gaaaa-aaaar-qafnq-cai", "ckusdt": "cngnf-vqaaa-aaaar-qag4q-cai"},
        "participants": people,
    }


class Season2AccountingTests(unittest.TestCase):
    def test_event_model_integer_rounding_and_flat_four_x(self):
        end = CAMPAIGN_START_NS + NANOS_PER_DAY // 2
        raw = inputs(
            vault_debt=101,
            recorded_3pool_usdc=1000,
            recorded_3pool_usdt=1000,
            verified_3usd=1000,
        )
        known = [
            "icusd_debt",
            "icusd_3pool",
            "fiat_stable_3pool",
            "icusd_stability_pool",
            "threeusd_stability_pool",
            "amm_lp",
            "vault_repayment",
        ]
        person = participant(end, event_interval(CAMPAIGN_START_NS, end, raw, known))
        result = calculate(manifest("event_time_integral_model", end, [person]))
        row = result["participants"][0]
        # 101 * half-day floors to 50. Verification cap is 1005/2000;
        # each fiat leg floors to 502, then flat 4x, then half-day floors.
        self.assertEqual(row["category_points"]["icusd_debt"], 50)
        self.assertEqual(row["category_points"]["fiat_stable_3pool"], 2008)
        self.assertEqual(row["total_points"], 2058)
        self.assertTrue(result["provisional"])
        self.assertFalse(result["credits_approved"])
        self.assertIn("constant-balance model", result["note"])

    def test_inherited_snapshot_min_chooses_whole_snapshot(self):
        end = CAMPAIGN_START_NS + NANOS_PER_DAY
        known = [
            "icusd_debt",
            "icusd_3pool",
            "fiat_stable_3pool",
            "icusd_stability_pool",
            "threeusd_stability_pool",
            "amm_lp",
            "vault_repayment",
        ]
        a = inputs(vault_debt=100)
        b = inputs(amm_lp_value=80)  # weighted AMM value is 160
        person = participant(end, snapshot_interval(CAMPAIGN_START_NS, end, a, b, known))
        result = calculate(manifest("inherited_two_snapshot", end, [person]))
        row = result["participants"][0]
        self.assertEqual(row["category_points"]["icusd_debt"], 100)
        self.assertEqual(row["category_points"]["amm_lp"], 0)
        self.assertEqual(row["total_points"], 100)
        self.assertIn("whole-snapshot minimum", result["note"])

    def test_inherited_snapshot_rejects_partial_unknown(self):
        end = CAMPAIGN_START_NS + NANOS_PER_DAY
        known = ["icusd_debt", "icusd_3pool", "fiat_stable_3pool", "amm_lp"]
        person = participant(end, snapshot_interval(CAMPAIGN_START_NS, end, inputs(), inputs(), known))
        with self.assertRaisesRegex(AccountingError, "rejects partial unknown"):
            calculate(manifest("inherited_two_snapshot", end, [person]))

    def test_verification_tolerance_is_applied_with_per_leg_floor(self):
        end = CAMPAIGN_START_NS + NANOS_PER_DAY
        known = ["icusd_3pool", "fiat_stable_3pool"]
        raw = inputs(recorded_3pool_usdc=1000, recorded_3pool_usdt=1000, verified_3usd=1000)
        person = participant(end, event_interval(CAMPAIGN_START_NS, end, raw, known))
        row = calculate(manifest("event_time_integral_model", end, [person]))["participants"][0]
        self.assertEqual(row["category_points"]["fiat_stable_3pool"], 4016)
        self.assertIsNone(row["category_points"]["icusd_debt"])
        self.assertIsNone(row["total_points"])

    def test_unknown_category_is_null_and_totals_are_unresolved(self):
        end = CAMPAIGN_START_NS + NANOS_PER_DAY
        raw = inputs(
            vault_debt=10,
            recorded_3pool_icusd=None,
            recorded_3pool_usdc=None,
            recorded_3pool_usdt=None,
            verified_3usd=None,
            sp_icusd=None,
            sp_3usd=None,
            amm_lp_value=None,
        )
        person = participant(end, event_interval(CAMPAIGN_START_NS, end, raw, ["icusd_debt"]))
        result = calculate(manifest("event_time_integral_model", end, [person]))
        row = result["participants"][0]
        self.assertEqual(row["category_points"]["icusd_debt"], 10)
        self.assertIsNone(row["category_points"]["amm_lp"])
        self.assertIsNone(row["total_points"])
        self.assertIsNone(result["aggregate_total_points"])

    def test_gap_is_rejected(self):
        end = CAMPAIGN_START_NS + 2 * NANOS_PER_DAY
        known = ["icusd_debt"]
        first = event_interval(CAMPAIGN_START_NS, CAMPAIGN_START_NS + NANOS_PER_DAY, inputs(vault_debt=1), known)
        person = participant(end, first)
        with self.assertRaisesRegex(AccountingError, "missing interval history|gap|ends at"):
            calculate(manifest("event_time_integral_model", end, [person]))

    def test_overlap_is_rejected(self):
        end = CAMPAIGN_START_NS + 2 * NANOS_PER_DAY
        known = ["icusd_debt"]
        first = event_interval(CAMPAIGN_START_NS, CAMPAIGN_START_NS + NANOS_PER_DAY, inputs(vault_debt=1), known)
        second = event_interval(CAMPAIGN_START_NS + NANOS_PER_DAY - 1, end, inputs(vault_debt=1), known)
        person = participant(end, first)
        person["intervals"].append(second)
        with self.assertRaisesRegex(AccountingError, "temporal overlap"):
            calculate(manifest("event_time_integral_model", end, [person]))

    def test_negative_input_and_pre_campaign_interval_are_rejected(self):
        end = CAMPAIGN_START_NS + NANOS_PER_DAY
        bad_values = inputs(vault_debt=-1)
        person = participant(end, event_interval(CAMPAIGN_START_NS, end, bad_values, ["icusd_debt"]))
        with self.assertRaisesRegex(AccountingError, "integer in"):
            calculate(manifest("event_time_integral_model", end, [person]))

        person = participant(end, event_interval(CAMPAIGN_START_NS - 1, end, inputs(), []))
        with self.assertRaisesRegex(AccountingError, "starts before the Season 2 campaign"):
            calculate(manifest("event_time_integral_model", end, [person]))

    def test_repayment_window_is_capped_at_cutoff(self):
        end = CAMPAIGN_START_NS + 2 * NANOS_PER_DAY
        repayment_start = CAMPAIGN_START_NS + NANOS_PER_DAY
        known = ["vault_repayment"]
        interval = event_interval(CAMPAIGN_START_NS, end, inputs(), known)
        event = {
            "source_tag": 0,
            "event_id": 5,
            "asset_ledger": "xevnm-gaaaa-aaaar-qafnq-cai",
            "asset_evidence_ref": "test:verified-ledger",
            "amount_usd_e8s": 100,
            "repaid_at_ns": repayment_start,
            "window_end_ns": repayment_start + 10 * NANOS_PER_DAY,
        }
        person = participant(end, interval, repayments=[event])
        row = calculate(manifest("event_time_integral_model", end, [person]))["participants"][0]
        self.assertEqual(row["category_points"]["vault_repayment"], 500)

    def test_duplicate_repayment_and_pre_campaign_event_are_rejected(self):
        end = CAMPAIGN_START_NS + NANOS_PER_DAY
        event = {
            "source_tag": 0,
            "event_id": 8,
            "asset_ledger": "xevnm-gaaaa-aaaar-qafnq-cai",
            "asset_evidence_ref": "test:verified-ledger",
            "amount_usd_e8s": 10,
            "repaid_at_ns": CAMPAIGN_START_NS,
            "window_end_ns": CAMPAIGN_START_NS + NANOS_PER_DAY,
        }
        interval = event_interval(CAMPAIGN_START_NS, end, inputs(), ["vault_repayment"])
        person = participant(end, interval, repayments=[event, copy.deepcopy(event)])
        with self.assertRaisesRegex(AccountingError, "repeats source_tag/event_id"):
            calculate(manifest("event_time_integral_model", end, [person]))

        old_event = dict(event, event_id=9, repaid_at_ns=CAMPAIGN_START_NS - 1)
        person["repayments"] = [old_event]
        with self.assertRaisesRegex(AccountingError, "predates the Season 2 campaign"):
            calculate(manifest("event_time_integral_model", end, [person]))

    def test_repayment_requires_source_eligible_ledger_and_evidence(self):
        end = CAMPAIGN_START_NS + NANOS_PER_DAY
        event = {"source_tag": 0, "event_id": 12, "amount_usd_e8s": 10,
                 "repaid_at_ns": CAMPAIGN_START_NS, "window_end_ns": end,
                 "asset_ledger": "xevnm-gaaaa-aaaar-qafnq-cai", "asset_evidence_ref": "test:verified"}
        person = participant(end, event_interval(CAMPAIGN_START_NS, end, inputs(), ["vault_repayment"]), repayments=[event])
        for changed, expected in (({"source_tag": 1}, "backend event"),
                                  ({"asset_ledger": "unknown"}, "asset_ledger"),
                                  ({"asset_evidence_ref": ""}, "asset_evidence_ref")):
            person["repayments"] = [dict(event, **changed)]
            with self.assertRaisesRegex(AccountingError, expected):
                calculate(manifest("event_time_integral_model", end, [person]))
        naked = dict(event); del naked["asset_ledger"]
        person["repayments"] = [naked]
        with self.assertRaisesRegex(AccountingError, "asset_ledger"):
            calculate(manifest("event_time_integral_model", end, [person]))

    def test_repayment_event_cannot_be_credited_to_two_participants(self):
        end = CAMPAIGN_START_NS + NANOS_PER_DAY
        event = {"source_tag": 0, "event_id": 17, "amount_usd_e8s": 10,
                 "repaid_at_ns": CAMPAIGN_START_NS, "window_end_ns": end,
                 "asset_ledger": "xevnm-gaaaa-aaaar-qafnq-cai", "asset_evidence_ref": "test:verified"}
        interval = event_interval(CAMPAIGN_START_NS, end, inputs(), ["vault_repayment"])
        first = participant(end, interval, repayments=[event])
        second = copy.deepcopy(first); second["principal_id"] = "fake-principal-02"
        with self.assertRaisesRegex(AccountingError, "repeats source_tag/event_id"):
            calculate(manifest("event_time_integral_model", end, [first, second]))

    def test_repayment_floors_each_supplied_interval(self):
        unit = NANOS_PER_DAY // 10
        end = CAMPAIGN_START_NS + 2 * unit
        known = list(CATEGORY_BITS)
        first = snapshot_interval(CAMPAIGN_START_NS, CAMPAIGN_START_NS + unit, inputs(), inputs(), known)
        second = snapshot_interval(CAMPAIGN_START_NS + unit, end, inputs(), inputs(), known)
        event = {"source_tag": 0, "event_id": 14, "amount_usd_e8s": 1,
                 "repaid_at_ns": CAMPAIGN_START_NS, "window_end_ns": end,
                 "asset_ledger": "xevnm-gaaaa-aaaar-qafnq-cai", "asset_evidence_ref": "test:verified"}
        person = participant(end, first, repayments=[event]); person["intervals"].append(second)
        result = calculate(manifest("inherited_two_snapshot", end, [person]))
        self.assertEqual(result["participants"][0]["category_points"]["vault_repayment"], 0)
        self.assertIn("not proof of captures", result["note"])

    def test_inherited_rejects_unresolved_mid_epoch_registration(self):
        end = CAMPAIGN_START_NS + NANOS_PER_DAY
        registered = CAMPAIGN_START_NS + 1
        interval = snapshot_interval(registered, end, inputs(), inputs(), list(CATEGORY_BITS))
        person = participant(end, interval, registered_at_ns=registered)
        with self.assertRaisesRegex(AccountingError, "capture eligibility"):
            calculate(manifest("inherited_two_snapshot", end, [person]))

    def test_method_must_be_explicit(self):
        with self.assertRaisesRegex(AccountingError, "method is required"):
            calculate({"campaign_start_ns": CAMPAIGN_START_NS, "cutoff_ns": CAMPAIGN_START_NS + 1, "participants": []})

    def test_missing_repayment_manifest_and_empty_participant_file_are_rejected(self):
        end = CAMPAIGN_START_NS + NANOS_PER_DAY
        person = participant(end, event_interval(CAMPAIGN_START_NS, end, inputs(), ["icusd_debt"]))
        del person["repayments"]
        with self.assertRaisesRegex(AccountingError, "repayments is required"):
            calculate(manifest("event_time_integral_model", end, [person]))
        with self.assertRaisesRegex(AccountingError, "non-empty array"):
            calculate(manifest("event_time_integral_model", end, []))


if __name__ == "__main__":
    unittest.main()
