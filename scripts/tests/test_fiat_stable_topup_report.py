import importlib.util
import json
import tempfile
import unittest
from types import SimpleNamespace
from unittest.mock import patch
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "fiat-stable-topup-report.py"
SPEC = importlib.util.spec_from_file_location("fiat_stable_topup_report", SCRIPT)
REPORT = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(REPORT)


P1 = "aaaaa-aa"
P2 = "2vxsx-fae"


def entry(principal, epoch, source, delta, recorded=1):
    return {
        "principal": principal,
        "epoch": epoch,
        "source": source,
        "delta": delta,
        "recorded_at_ns": recorded,
    }


def candid_page(rows, next_offset, reached_end):
    records = []
    for row in rows:
        records.append(
            "record { "
            f'"principal" = principal "{row["principal"]}"; '
            f'epoch_index = {row["epoch"]} : nat64; '
            f'source = variant {{ {row["source"]} }}; '
            f'recorded_at_ns = {row["recorded_at_ns"]} : nat64; '
            f'points_delta = {row["delta"]} : nat; '
            "}"
        )
    text = "record { " + f"reached_end = {'true' if reached_end else 'false'}; "
    text += "entries = vec { " + " ".join(records) + " }; "
    text += f"next_offset = {next_offset} : nat64; }}"
    return text


class FiatStableTopupReportTests(unittest.TestCase):
    def test_json_wrapped_page_parses_leaf_records_once(self):
        row = entry(P1, 4, "CkStable3PoolUnmatched", 7)
        raw = json.dumps({"response_candid": candid_page([row], 1, True)})
        page = REPORT.parse_page(raw)
        self.assertEqual(page["next_offset"], 1)
        self.assertEqual(page["entries"], [row])

    def test_global_pagination_requires_contiguous_forward_cursor(self):
        rows = [
            entry(P1, 0, "Registration", 0),
            entry(P1, 1, "CkStable3PoolUnmatched", 7),
            entry(P2, 1, "IcUsdDebt", 11),
        ]

        def query(offset, limit):
            self.assertEqual(limit, 2)
            return candid_page(rows[offset : offset + limit], min(offset + limit, 3), offset + limit >= 3)

        got = REPORT.collect_prefix(query, ledger_length=3, page_size=2)
        self.assertEqual(got, rows)

    def test_population_is_markers_union_positive_entries(self):
        rows = [
            entry(P1, 0, "Registration", 0),
            entry(P2, 0, "Registration", 0),
            entry(P1, 1, "IcUsdDebt", 10),
            entry("w7x7r-cokma-aaaaa-aaa", 1, "AmmLp", 3),
        ]
        report = REPORT.build_report(rows, ledger_length=4, cutoff=4)
        self.assertEqual(
            [u["principal"] for u in report["users"]],
            sorted({P1, P2, "w7x7r-cokma-aaaaa-aaa"}),
        )
        self.assertEqual(report["before_total_points_e8s"], 13)

    def test_floor_is_applied_per_original_row_not_aggregate(self):
        rows = [
            entry(P1, 1, "Registration", 0),
            entry(P1, 1, "CkStable3PoolUnmatched", 1),
            entry(P1, 1, "CkStable3PoolUnmatched", 2),
        ]
        report = REPORT.build_report(rows, ledger_length=3, cutoff=3)
        self.assertEqual(report["before_total_points_e8s"], 3)
        self.assertEqual(report["uplift_total_points_e8s"], 0)
        # floor((1 + 2) / 3) would incorrectly produce one point.
        self.assertEqual(report["after_total_points_e8s"], 3)

    def test_mixed_sources_and_explicit_cutover_preserve_flat_rows(self):
        rows = [
            entry(P1, 2, "Registration", 0),
            entry(P1, 2, "CkStable3PoolUnmatched", 9),
            entry(P1, 3, "CkStable3PoolFlat4x", 40),
            entry(P2, 3, "VaultRepayment", 5),
        ]
        report = REPORT.build_report(
            rows, ledger_length=4, cutoff=4, cutover_epoch=3
        )
        self.assertEqual(report["classification"], "mixed_legacy_and_future_flat4x_with_explicit_cutover")
        self.assertEqual(report["before_total_points_e8s"], 54)
        self.assertEqual(report["uplift_total_points_e8s"], 3)
        self.assertEqual(report["after_total_points_e8s"], 57)

    def test_existing_adjustment_or_unclassified_flat_prefix_is_rejected(self):
        topup = [
            entry(P1, 1, "CkStable3PoolUnmatched", 9),
            entry(P1, 2, "CkStable3PoolUnmatchedTopUp", 3),
        ]
        with self.assertRaisesRegex(REPORT.ReportError, "already contains"):
            REPORT.build_report(topup, ledger_length=2, cutoff=2)

        flat = [entry(P1, 3, "CkStable3PoolFlat4x", 40)]
        with self.assertRaisesRegex(REPORT.ReportError, "flat-4x"):
            REPORT.build_report(flat, ledger_length=1, cutoff=1)
        with self.assertRaisesRegex(REPORT.ReportError, "precedes supplied"):
            REPORT.build_report(flat, ledger_length=1, cutoff=1, cutover_epoch=4)

    def test_unknown_source_is_an_error(self):
        with self.assertRaisesRegex(REPORT.ReportError, "unknown PointSource"):
            REPORT.build_report(
                [entry(P1, 1, "FutureUnlistedSource", 1)],
                ledger_length=1,
                cutoff=1,
            )

    def test_raw_u128_max_unmatched_row_is_a_row_specific_error(self):
        rows = [
            entry(P1, 1, "Registration", 0),
            entry(P1, 2, "CkStable3PoolUnmatched", REPORT.U128_MAX),
        ]
        with self.assertRaisesRegex(
            REPORT.ReportError,
            r"ledger offset 1.*principal aaaaa-aa.*points_delta=340282366920938463463374607431768211455",
        ):
            REPORT.build_report(rows, ledger_length=2, cutoff=2)

    def test_scale_by_period_sentinel_unmatched_row_is_a_row_specific_error(self):
        rows = [
            entry(P1, 1, "Registration", 0),
            entry(
                P1,
                2,
                "CkStable3PoolUnmatched",
                REPORT.SCALED_SATURATION_SENTINEL,
            ),
        ]
        with self.assertRaisesRegex(
            REPORT.ReportError,
            rf"ledger offset 1.*points_delta={REPORT.SCALED_SATURATION_SENTINEL}",
        ):
            REPORT.build_report(rows, ledger_length=2, cutoff=2)

    def test_post_migration_reconciliation_matches_appended_corrections(self):
        original = [
            entry(P1, 1, "Registration", 0),
            entry(P1, 1, "CkStable3PoolUnmatched", 9),
            entry(P2, 1, "IcUsdDebt", 5),
        ]
        current = original + [
            entry(P1, 1, "CkStable3PoolUnmatchedTopUp", 3),
            entry(P2, 2, "CkStable3PoolFlat4x", 40),
        ]
        report = REPORT.build_report(original, ledger_length=3, cutoff=3)
        REPORT.reconcile_current_rows(
            report,
            current,
            cutoff=3,
            principal_totals={P1: 12, P2: 45},
        )
        self.assertEqual(report["mode"], "post_migration_reconciliation")
        post = report["post_migration_reconciliation"]
        self.assertEqual(post["correction_rows"], 1)
        self.assertEqual(post["correction_points_e8s"], 3)
        self.assertEqual(post["preview_uplift_points_e8s"], 3)
        self.assertTrue(post["corrections_match_preview"])
        self.assertTrue(post["principal_totals_match_ledger"])
        users = {user["principal"]: user for user in report["users"]}
        self.assertEqual(users[P1]["correction_points_e8s"], 3)
        self.assertEqual(users[P1]["principal_total_points_e8s"], 12)

    def test_reconciliation_rejects_correction_before_fixed_cutoff(self):
        rows = [
            entry(P1, 1, "CkStable3PoolUnmatchedTopUp", 3),
            entry(P1, 1, "CkStable3PoolUnmatched", 9),
        ]
        report = {
            "users": [{"principal": P1, "uplift_points_e8s": 3}],
        }
        with self.assertRaisesRegex(REPORT.ReportError, "original cutoff"):
            REPORT.reconcile_current_rows(
                report,
                rows,
                cutoff=1,
                principal_totals={P1: 12},
            )

    def test_reconciliation_rejects_per_principal_mismatch(self):
        original = [entry(P1, 1, "CkStable3PoolUnmatched", 9)]
        report = REPORT.build_report(original, ledger_length=1, cutoff=1)
        with self.assertRaisesRegex(REPORT.ReportError, "correction reconciliation"):
            REPORT.reconcile_current_rows(
                report,
                original + [entry(P1, 1, "CkStable3PoolUnmatchedTopUp", 2)],
                cutoff=1,
                principal_totals={P1: 11},
            )

    def test_reconciliation_rejects_principal_total_mismatch(self):
        original = [entry(P1, 1, "CkStable3PoolUnmatched", 9)]
        report = REPORT.build_report(original, ledger_length=1, cutoff=1)
        with self.assertRaisesRegex(REPORT.ReportError, "principal total"):
            REPORT.reconcile_current_rows(
                report,
                original + [entry(P1, 1, "CkStable3PoolUnmatchedTopUp", 3)],
                cutoff=1,
                principal_totals={P1: 999},
            )

    @patch.object(REPORT.subprocess, "run")
    def test_icp_call_is_explicitly_anonymous_query_only(self, run):
        run.return_value = SimpleNamespace(returncode=0, stdout="(0 : nat64,)", stderr="")
        REPORT.icp_call(
            "aaaaa-aa",
            "get_point_ledger_len",
            "()",
            network="ic",
            candid="points.did",
        )
        command = run.call_args.args[0]
        self.assertIn("--identity", command)
        self.assertEqual(command[command.index("--identity") + 1], "anonymous")
        self.assertIn("--query", command)
        self.assertEqual(command[command.index("--network") + 1], "ic")
        self.assertEqual(command[command.index("--output") + 1], "candid")
        self.assertIn("--json", command)

    def test_json_and_csv_keep_points_as_integers_and_decimals_as_strings(self):
        rows = [entry(P1, 1, "CkStable3PoolUnmatched", 4)]
        report = REPORT.build_report(rows, ledger_length=1, cutoff=1)
        with tempfile.TemporaryDirectory() as temp:
            json_path = Path(temp) / "report.json"
            csv_path = Path(temp) / "report.csv"
            REPORT.write_outputs(report, str(json_path), str(csv_path))
            saved = json.loads(json_path.read_text())
            self.assertIsInstance(saved["before_total_points_e8s"], int)
            self.assertIsInstance(saved["users"][0]["before_points_decimal"], str)
            self.assertIn("before_points_e8s", csv_path.read_text().splitlines()[0])


if __name__ == "__main__":
    unittest.main()
