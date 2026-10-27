#!/usr/bin/env python3
"""Derive the sanitized Lookout projection from the canonical Kanban board.

Usage: python3 docs/coordination/base-icusd-bridge-lookout-derive.py

Reads docs/coordination/base-icusd-bridge-kanban.md and writes
docs/coordination/base-icusd-bridge-lookout.json. The output carries board metadata
only: card id, state, owner role, scope and a short evidence text with its proof
level. Paths, hashes longer than a short commit id, addresses and URLs are removed,
and text over 240 characters is cut at a clause or word boundary and marked.

It also validates docs/coordination/base-icusd-bridge-presentation.json, the
hand-maintained presentation sidecar ({summary, glossary:[{term, definition, cardId?}]})
that gives the board a short plain-English status and term definitions. The sidecar is
never rewritten here; the check mirrors the Lookout publisher's limits and fails if the
publisher's redaction would change any text.
This script never uploads anything. Publishing is done separately by the scoped Lookout
publisher under the event rule and limits in base-icusd-bridge-operating-rules.md
(sanitized board metadata only; only changed snapshots are published).
"""

import json
import pathlib
import re

ROOT = pathlib.Path(__file__).resolve().parent
BOARD = ROOT / "base-icusd-bridge-kanban.md"
OUT = ROOT / "base-icusd-bridge-lookout.json"
PRESENTATION = ROOT / "base-icusd-bridge-presentation.json"
PRIORITIES = ROOT / "base-icusd-bridge-priorities.json"

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
    return clip(text)


TRUNCATED = " [truncated]"


def clip(text: str, limit: int = 240) -> str:
    """Keep `text` within `limit` characters without cutting a word.

    Text that fits is returned unchanged. Otherwise it is cut at the last clause boundary
    ("; " or ". ") in the second half of the budget, or else at the last space, and marked
    with TRUNCATED so a reader knows the canonical board holds the rest.
    """
    if len(text) <= limit:
        return text
    budget = limit - len(TRUNCATED)
    head = text[:budget]
    cut = max(head.rfind("; "), head.rfind(". "))
    if cut < budget // 2:
        cut = head.rfind(" ")
    if cut <= 0:
        cut = budget
    return head[:cut].rstrip(" ;,.") + TRUNCATED


def owner_role(cell: str) -> str:
    cell = cell.lower()
    if cell.startswith("coordinator"):
        return "coordinator"
    if cell.startswith("supervisor"):
        return "supervisor"
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
    check_presentation(cards)
    check_priorities(cards)


# Mirrors safeField in the Lookout publisher: text it would redact must not be here.
REDACTIONS = [
    r"\b(?:gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,})\b",
    r"\bBearer\s+[A-Za-z0-9._~+/-]+=*",
    r"\b(?:0x)?[a-fA-F0-9]{64}\b",
    r"(?:/Users/[^\s|]+|/home/[^\s|]+|/private/[^\s|]+|[A-Za-z]:\\[^\s|]+)",
    r"(?i)\b(?:seed phrase|mnemonic|private key|api key|token|secret)\s*[:=]\s*[^|\n]+",
]


def check_presentation(cards: list) -> None:
    """Validate the presentation sidecar against the publisher's rules and the cards."""
    raw = json.loads(PRESENTATION.read_text(encoding="utf-8"))
    problems = []
    if not isinstance(raw, dict) or set(raw) != {"summary", "glossary"}:
        problems.append("top level must be exactly {summary, glossary}")
        raw = raw if isinstance(raw, dict) else {}
    summary, glossary = raw.get("summary"), raw.get("glossary")
    if not isinstance(summary, str) or not summary.strip():
        problems.append("summary must be non-empty text")
        summary = ""
    if not isinstance(glossary, list):
        problems.append("glossary must be a list")
        glossary = []
    if len(summary) > 600:
        problems.append(f"summary is {len(summary)} characters (max 600)")
    if len(glossary) > 200:
        problems.append(f"glossary has {len(glossary)} definitions (max 200)")
    ids = {c["id"] for c in cards}
    text_by_id = {c["id"]: f"{c['id']} {c['scope']} {c['evidenceText']}".lower() for c in cards}
    all_text = " ".join(text_by_id.values())
    seen, total, unused = set(), len(summary), []
    texts = [summary]
    for i, item in enumerate(glossary):
        if not isinstance(item, dict) or not set(item) <= {"term", "definition", "cardId"} or not {"term", "definition"} <= set(item):
            problems.append(f"glossary[{i}] must have term, definition and optional cardId only")
            continue
        term, definition, card = item["term"], item["definition"], item.get("cardId")
        if not term.strip() or not definition.strip():
            problems.append(f"glossary[{i}] has an empty term or definition")
        if len(term) > 100 or len(definition) > 1200:
            problems.append(f"glossary[{i}] {term!r} is too long")
        if card is not None and card not in ids:
            problems.append(f"glossary[{i}] {term!r} refers to unknown card {card!r}")
        key = (term, card)
        if key in seen:
            problems.append(f"duplicate term/card pair {key}")
        seen.add(key)
        total += len(term) + len(definition) + len(card or "")
        texts += [term, definition]
        where = text_by_id.get(card, "") if card else all_text
        if term.lower() not in where:
            unused.append(f"{term} ({card})" if card else term)
    if total > 70000:
        problems.append(f"presentation is {total} characters (max 70000)")
    for t in texts:
        for pattern in REDACTIONS:
            if re.search(pattern, t):
                problems.append(f"publisher would redact part of: {t[:60]!r}")
    if "\u2014" in "".join(texts):
        problems.append("presentation contains an em dash")
    if problems:
        raise SystemExit("presentation sidecar invalid:\n  " + "\n  ".join(problems))
    print(f"presentation ok: summary {len(summary)} chars, {len(glossary)} definitions")
    if unused:
        print(f"note: {len(unused)} terms do not appear in the card text they apply to: {', '.join(unused)}")



PRIORITY_FIELDS = ["id", "title", "status", "blockedReason", "nextAction", "cardIds"]
PRIORITY_STATUSES = {"ready", "in_progress", "blocked", "paused", "done"}


def check_priorities(cards: list) -> None:
    """Validate the priority list (the publisher's rules) and its mirror in the board."""
    raw = json.loads(PRIORITIES.read_text(encoding="utf-8"))
    problems = []
    if not isinstance(raw, dict) or set(raw) != {"items"} or not isinstance(raw.get("items"), list):
        raise SystemExit("priority list must be exactly {items: [...]}")
    items = raw["items"]
    if len(items) > 30:
        problems.append(f"{len(items)} items (max 30)")
    ids = {c["id"] for c in cards}
    seen = set()
    for n, it in enumerate(items):
        if not isinstance(it, dict) or set(it) != set(PRIORITY_FIELDS):
            problems.append(f"item {n} must have exactly {PRIORITY_FIELDS}")
            continue
        if not all(isinstance(it[k], str) for k in PRIORITY_FIELDS[:5]) or not isinstance(it["cardIds"], list):
            problems.append(f"item {n} has a field of the wrong type")
            continue
        pid = it["id"]
        if not pid or len(pid) > 80 or pid in seen:
            problems.append(f"item {n} id {pid!r} is empty, too long or repeated")
        seen.add(pid)
        if not it["title"] or len(it["title"]) > 160:
            problems.append(f"{pid}: title must be 1..160 characters")
        if it["status"] not in PRIORITY_STATUSES:
            problems.append(f"{pid}: invalid status {it['status']!r}")
        if len(it["blockedReason"]) > 600 or len(it["nextAction"]) > 600:
            problems.append(f"{pid}: text over 600 characters")
        if it["status"] in ("blocked", "paused") and not it["blockedReason"].strip():
            problems.append(f"{pid}: {it['status']} needs a blockedReason")
        if it["status"] in ("ready", "in_progress") and not it["nextAction"].strip():
            problems.append(f"{pid}: {it['status']} needs a nextAction")
        if len(it["cardIds"]) > 100 or len(set(it["cardIds"])) != len(it["cardIds"]):
            problems.append(f"{pid}: too many or repeated cardIds")
        for cid in it["cardIds"]:
            if cid not in ids:
                problems.append(f"{pid}: unknown card {cid!r}")
        for t in [it["title"], it["blockedReason"], it["nextAction"], pid]:
            for pattern in REDACTIONS:
                if re.search(pattern, t):
                    problems.append(f"{pid}: publisher would redact part of: {t[:60]!r}")
            if "\u2014" in t:
                problems.append(f"{pid}: em dash")
    # The board's "Priorities" table must list the same items, in the same order and status.
    section = BOARD.read_text().split("## Priorities (authoritative order)", 1)
    rows = []
    if len(section) == 2:
        for line in section[1].split("\n## ", 1)[0].splitlines():
            m = re.match(r"\| (\d+) \| `([^`]+)`: .*? \| ([a-z_]+) \|", line)
            if m:
                rows.append((int(m.group(1)), m.group(2), m.group(3)))
    expected = [(n + 1, it["id"], it["status"]) for n, it in enumerate(items) if isinstance(it, dict)]
    if rows != expected:
        problems.append(f"board priority table {rows} does not match the JSON {expected}")
    if problems:
        raise SystemExit("priority list invalid:\n  " + "\n  ".join(problems))
    order = ", ".join(f"{n + 1}:{it['id']}={it['status']}" for n, it in enumerate(items))
    print(f"priorities ok: {len(items)} items ({order})")


if __name__ == "__main__":
    main()
