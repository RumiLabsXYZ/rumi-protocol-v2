#!/usr/bin/env python3
"""Derive the sanitized Lookout projection from the canonical Kanban board.

Usage: python3 docs/coordination/base-icusd-bridge-lookout-derive.py

Reads docs/coordination/base-icusd-bridge-kanban.md and writes
docs/coordination/base-icusd-bridge-lookout.json. The output carries board metadata
only: card id, state, owner role, scope and a short evidence text with its proof
level. Paths, hashes longer than a short commit id, addresses and URLs are removed.
Publishing is HELD (see base-icusd-bridge-operating-rules.md); this script never
uploads anything.
"""

import json
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parent
BOARD = ROOT / "base-icusd-bridge-kanban.md"
OUT = ROOT / "base-icusd-bridge-lookout.json"

# Public board links only (no local paths, no payloads).
LINKS = {
    "BB-14": [{"title": "Draft PR #591", "url": "https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/591"}],
}

STATE = {"done": "done", "in progress": "in_progress", "todo": "todo", "blocked": "blocked", "held": "held"}


def sanitize(text: str) -> str:
    # Drop backticked fragments that look like paths or code.
    text = re.sub(r"`[^`]*[/\\.][^`]*`", "", text)
    text = text.replace("`", "")
    # Drop anything that looks like an address, long hash or URL.
    text = re.sub(r"0x[0-9a-fA-F]{9,}", "", text)
    text = re.sub(r"https?://\S+", "", text)
    text = re.sub(r"\b[0-9a-f]{12,}\b", "", text)
    text = re.sub(r"\s+", " ", text).strip(" ;,")
    return text[:240]


def owner_role(cell: str) -> str:
    cell = cell.lower()
    if cell.startswith("coordinator"):
        return "coordinator"
    if "haiku" in cell:
        return "worker-haiku"
    if "sonnet" in cell:
        return "worker-sonnet"
    return "worker"


def main() -> None:
    cards = []
    for line in BOARD.read_text().splitlines():
        if not line.startswith("| BB-"):
            continue
        cols = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cols) < 7:
            continue
        title, state, owner, _files, _deps, evidence, proof = cols[:7]
        card_id, _, scope = title.partition(" ")
        state = state.split(" (")[0]
        cards.append(
            {
                "id": card_id,
                "state": STATE.get(state, state),
                "owner": owner_role(owner),
                "scope": sanitize(scope),
                "evidenceText": f"proof={proof}; {sanitize(evidence)}".strip("; "),
                "evidenceLinks": LINKS.get(card_id, []),
            }
        )
    OUT.write_text(json.dumps({"cards": cards}, indent=2) + "\n")
    print(f"wrote {len(cards)} cards to {OUT.name}")


if __name__ == "__main__":
    main()
