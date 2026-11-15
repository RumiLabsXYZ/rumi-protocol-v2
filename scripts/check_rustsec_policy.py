#!/usr/bin/env python3
"""Enforce an exact, expiring policy over cargo-audit JSON output."""

from __future__ import annotations

import argparse
import datetime as dt
import json
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--audit", required=True, type=Path)
    parser.add_argument("--policy", required=True, type=Path)
    parser.add_argument("--summary", required=True, type=Path)
    parser.add_argument("--report", required=True, type=Path)
    return parser.parse_args()


def observed_findings(audit: dict) -> set[tuple[str, str, str, str]]:
    findings: set[tuple[str, str, str, str]] = set()
    for item in audit.get("vulnerabilities", {}).get("list", []):
        findings.add(
            (
                "vulnerability",
                item["advisory"]["id"],
                item["package"]["name"],
                item["package"]["version"],
            )
        )
    for kind, items in audit.get("warnings", {}).items():
        for item in items:
            advisory = item.get("advisory") or {}
            findings.add(
                (
                    kind,
                    advisory.get("id", ""),
                    item["package"]["name"],
                    item["package"]["version"],
                )
            )
    return findings


def accepted_findings(policy: dict) -> tuple[set[tuple[str, str, str, str]], list[dict]]:
    findings: set[tuple[str, str, str, str]] = set()
    expired: list[dict] = []
    today = dt.date.today()
    for entry in policy["accepted"]:
        required = {"kind", "package", "version", "advisory_ids", "scope", "rationale", "evidence", "expires"}
        missing = sorted(required - entry.keys())
        if missing:
            raise ValueError(f"policy entry missing fields {missing}: {entry}")
        expiry = dt.date.fromisoformat(entry["expires"])
        if expiry < today:
            expired.append(entry)
        for advisory_id in entry["advisory_ids"]:
            findings.add((entry["kind"], advisory_id, entry["package"], entry["version"]))
    return findings, expired


def label(item: tuple[str, str, str, str]) -> str:
    kind, advisory_id, package, version = item
    advisory = advisory_id or "yanked"
    return f"{kind}: {advisory} {package}@{version}"


def main() -> int:
    args = parse_args()
    audit = json.loads(args.audit.read_text())
    policy = json.loads(args.policy.read_text())
    observed = observed_findings(audit)
    accepted, expired = accepted_findings(policy)
    unapproved = sorted(observed - accepted)
    stale = sorted(accepted - observed)

    report = {
        "policy_version": policy["version"],
        "observed_count": len(observed),
        "accepted_count": len(observed & accepted),
        "unapproved": [label(item) for item in unapproved],
        "stale": [label(item) for item in stale],
        "expired": expired,
        "warnings": {kind: len(items) for kind, items in audit.get("warnings", {}).items()},
    }
    args.report.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")

    lines = [
        "## RustSec dependency policy",
        "",
        f"- Observed exact findings: **{len(observed)}**",
        f"- Reviewed baseline matches: **{len(observed & accepted)}**",
        f"- Unapproved findings: **{len(unapproved)}**",
        f"- Stale policy tuples: **{len(stale)}**",
        f"- Expired policy entries: **{len(expired)}**",
        f"- Informational warnings: **{sum(len(items) for items in audit.get('warnings', {}).values())}**",
        "",
        "Vulnerabilities, unmaintained notices, and yanked packages are enforced as exact, expiring tuples.",
        "The full cargo-audit JSON and policy report are attached as workflow artifacts.",
    ]
    for heading, items in (("Unapproved", unapproved), ("Stale", stale)):
        if items:
            lines.extend(["", f"### {heading}", ""])
            lines.extend(f"- `{label(item)}`" for item in items)
    if expired:
        lines.extend(["", "### Expired", ""])
        lines.extend(
            f"- `{entry['kind']} {entry['package']}@{entry['version']}` expired {entry['expires']}"
            for entry in expired
        )
    args.summary.write_text("\n".join(lines) + "\n")
    return 1 if unapproved or stale or expired else 0


if __name__ == "__main__":
    raise SystemExit(main())
