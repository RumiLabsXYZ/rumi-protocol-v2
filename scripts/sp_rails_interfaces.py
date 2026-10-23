#!/usr/bin/env python3
"""Prepare and capture current Rust Candid exports for the SP rails.

The checked-in ``*.did`` files are compatibility baselines.  This tool builds
an isolated native host exporter from the *current* Rust source on every run;
it never imports the previous /private/tmp exporter or copies an old source
snapshot.  The generated source uses absolute module paths and records hashes
for the root source, every included module, the package manifest, and the
workspace lockfile.

Preparation is safe to run while another native build owns the Cargo cache.  It
writes exporter sources, manifests, and a capture command under ``--evidence``.
Pass ``--capture`` after the native build lock is available to run Cargo and
then emit full Candid exports, additive compatibility projections, and didc
checks.  No production DID or declaration is modified by this script.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path


SERVICE_START = re.compile(r"(?m)^\s*service\s*(?::|\()")
TYPE_START = re.compile(r"(?m)^type\s+([A-Za-z_]\w*)\s*=")
MODULE_DECL = re.compile(r"(?m)^(?P<indent>\s*)(?P<public>pub\s+)?mod\s+(?P<name>[A-Za-z_]\w*)\s*;")

RAIL_SPECS = {
    "sp": {
        "source": "src/stability_pool/src/lib.rs",
        "manifest": "src/stability_pool/Cargo.toml",
        "baseline": "src/stability_pool/stability_pool.did",
        "package": "stability_pool",
        "methods": [
            "start_3usd_conversion", "resume_3usd_conversion",
            "reconcile_3usd_conversion_pull_v1", "get_3usd_conversion_status",
            "get_active_3usd_conversion", "get_3usd_conversion_next_seq",
            "get_active_3usd_conversion_v1", "cancel_3usd_conversion",
            "quote_3usd_conversion", "get_ordinary_liquidation_status",
            "retry_ordinary_liquidation", "cancel_ordinary_liquidation",
            "reconcile_3usd_conversion_pool_input_v1",
            "reconcile_3usd_conversion_approval_v1",
            "reconcile_3usd_conversion_revoke_v1",
            "reconcile_3usd_conversion_refund_v1",
            "reconcile_ordinary_liquidation_approval",
            "admin_acknowledge_three_usd_reserve_v2_client",
        ],
        # The source intentionally adds `admitted : opt bool` to the existing
        # liquidation result.  Exclude those handlers from strict unchanged
        # wire checks and report that exclusion explicitly.
        "intentional_drift": ["notify_liquidatable_vaults", "execute_liquidation"],
    },
    "threepool": {
        "source": "src/rumi_3pool/src/lib.rs",
        "manifest": "src/rumi_3pool/Cargo.toml",
        "baseline": "src/rumi_3pool/rumi_3pool.did",
        "package": "rumi_3pool",
        "methods": [
            "add_liquidity_with_receipt_v1", "get_liquidity_receipt_v1",
            "add_liquidity_with_receipt_client_v1",
            "set_liquidity_receipt_client", "get_liquidity_receipt_client",
            "recover_liquidity_receipt_v1",
            "reconcile_liquidity_receipt_input_v1",
            "get_active_liquidity_receipt_v1",
            "recover_liquidity_receipt_for_v1",
            "reconcile_liquidity_receipt_input_for_v1",
            "reconcile_liquidity_receipt_refund_for_v1",
        ],
        "intentional_drift": [],
    },
    "backend": {
        "source": "src/rumi_protocol_backend/src/main.rs",
        "manifest": "src/rumi_protocol_backend/Cargo.toml",
        "baseline": "src/rumi_protocol_backend/rumi_protocol_backend.did",
        "package": "rumi_protocol_backend",
        "methods": [
            "acknowledge_three_usd_reserve_v2_client",
            "set_three_pool_canister", "get_three_pool_canister",
            "set_three_usd_reserve_ingress_enabled",
            "get_three_usd_reserve_ingress_enabled",
            "stability_pool_prepare_liquidation",
            "stability_pool_execute_liquidation",
            "stability_pool_liquidation_status",
            "stability_pool_cancel_liquidation",
            "stability_pool_cancel_liquidation_bound",
            "retire_stability_pool_liquidate_with_reserves_v2",
            "get_stability_pool_liquidation_candidate",
            "stability_pool_reconcile_liquidation_payment",
            "stability_pool_reconcile_liquidation_protocol_fee",
            "stability_pool_reconcile_liquidation_payout",
        ],
        "intentional_drift": [],
    },
}


class InterfaceError(RuntimeError):
    pass


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def brace_end(text: str, opening: int) -> int:
    depth = 0
    quote = False
    escaped = False
    for index in range(opening, len(text)):
        char = text[index]
        if quote:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quote = False
            continue
        if char == '"':
            quote = True
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return index + 1
    raise InterfaceError("unclosed Candid service/type block")


def service_block(text: str) -> str:
    match = SERVICE_START.search(text)
    if not match:
        raise InterfaceError("Candid export has no service block")
    opening = text.find("{", match.end())
    if opening < 0:
        raise InterfaceError("Candid service has no body")
    end = brace_end(text, opening)
    tail = text[end:]
    semi = tail.find(";")
    # Candid accepts both `service { ... }` and `service { ... };`; the
    # checked-in 3pool/backend baselines use the former.
    return text[match.start():end + (semi + 1 if semi >= 0 else 0)].strip()


def service_methods(text: str) -> dict[str, str]:
    return {name: text[start:stop].strip() for name, start, stop in service_method_spans(text)}


def service_method_spans(text: str) -> list[tuple[str, int, int]]:
    match = SERVICE_START.search(text)
    if not match:
        raise InterfaceError("Candid export has no service block")
    opening = text.find("{", match.end())
    if opening < 0:
        raise InterfaceError("Candid service has no body")
    service_end = brace_end(text, opening)
    closing = service_end - 1
    spans: list[tuple[str, int, int]] = []
    cursor = opening + 1
    while cursor < closing:
        method_match = re.search(r"(?m)^\s*([A-Za-z_]\w*)\s*:", text[cursor:closing])
        if not method_match:
            break
        start = cursor + method_match.start()
        name = method_match.group(1)
        body_start = cursor + method_match.end()
        depth = 1
        quote = False
        escaped = False
        stop = None
        for index in range(body_start, closing):
            char = text[index]
            if quote:
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == '"':
                    quote = False
                continue
            if char == '"':
                quote = True
            elif char in "({[":
                depth += 1
            elif char in ")}]":
                depth -= 1
            elif char == ";" and depth == 1:
                stop = index + 1
                break
        if stop is None:
            raise InterfaceError(f"unterminated service method {name}")
        spans.append((name, start, stop))
        cursor = stop
    return spans


def type_blocks(text: str) -> dict[str, str]:
    service = SERVICE_START.search(text)
    service_at = service.start() if service else len(text)
    starts = list(TYPE_START.finditer(text[:service_at]))
    blocks: dict[str, str] = {}
    for index, match in enumerate(starts):
        stop = starts[index + 1].start() if index + 1 < len(starts) else service_at
        blocks[match.group(1)] = text[match.start():stop].strip()
    return blocks


def method_dependencies(methods: list[str], blocks: dict[str, str]) -> set[str]:
    names = set(blocks)
    needed: set[str] = set()
    todo = set(re.findall(r"\b[A-Za-z_]\w*\b", "\n".join(methods))) & names
    while todo:
        name = todo.pop()
        if name in needed:
            continue
        needed.add(name)
        todo |= set(re.findall(r"\b[A-Za-z_]\w*\b", blocks[name])) & names - needed
    return needed


def _candid_tokens(text: str) -> list[tuple[str, str]]:
    """Lex the small Candid surface used by generated type graphs.

    Whitespace and punctuation remain tokens so the original source can be
    reconstructed byte-for-byte apart from intentional type-name aliases.
    Quoted strings are opaque and therefore cannot accidentally rename a text
    value that happens to equal a Rust/Candid type name.
    """
    tokens: list[tuple[str, str]] = []
    cursor = 0
    while cursor < len(text):
        char = text[cursor]
        if char.isspace():
            end = cursor + 1
            while end < len(text) and text[end].isspace():
                end += 1
            tokens.append(("ws", text[cursor:end]))
            cursor = end
            continue
        if char == '"':
            end = cursor + 1
            escaped = False
            while end < len(text):
                current = text[end]
                if escaped:
                    escaped = False
                elif current == "\\":
                    escaped = True
                elif current == '"':
                    end += 1
                    break
                end += 1
            if end > len(text) or not text[end - 1:end] == '"':
                raise InterfaceError("unterminated quoted Candid string")
            tokens.append(("string", text[cursor:end]))
            cursor = end
            continue
        match = re.match(r"[A-Za-z_]\w*", text[cursor:])
        if match:
            value = match.group(0)
            tokens.append(("ident", value))
            cursor += len(value)
            continue
        tokens.append(("punct", char))
        cursor += 1
    return tokens


def rename_tokens(text: str, aliases: dict[str, str]) -> str:
    """Rename type identifiers while preserving Candid labels and strings.

    A regex cannot distinguish a type reference named ``TransferFromError``
    from a variant unit label or a record field with the same spelling.  The
    brace stack tracks variant bodies; an identifier followed by ``:`` is a
    field/method label, and an identifier followed by ``;`` or ``}`` inside a
    variant is a unit label.  All other aliases are type declarations or type
    references.
    """
    tokens = _candid_tokens(text)
    significant = [index for index, (kind, _) in enumerate(tokens)
                   if kind in {"ident", "punct"}]
    previous: dict[int, str | None] = {}
    following: dict[int, str | None] = {}
    position = {token_index: position for position, token_index in enumerate(significant)}
    for token_index in significant:
        offset = position[token_index]
        previous[token_index] = tokens[significant[offset - 1]][1] if offset else None
        following[token_index] = (
            tokens[significant[offset + 1]][1]
            if offset + 1 < len(significant) else None
        )

    brace_stack: list[str | None] = []
    brace_kind_at: dict[int, str | None] = {}
    for token_index, (kind, value) in enumerate(tokens):
        if kind != "punct":
            continue
        if value == "{":
            prior = previous.get(token_index)
            brace_kind_at[token_index] = prior if prior in {"variant", "record", "service"} else None
            brace_stack.append(brace_kind_at[token_index])
        elif value == "}":
            if brace_stack:
                brace_stack.pop()
        if brace_stack:
            brace_kind_at[token_index] = brace_stack[-1]

    active_braces: list[str | None] = []
    for token_index, (kind, value) in enumerate(tokens):
        if kind == "punct" and value == "{":
            active_braces.append(brace_kind_at.get(token_index))
            continue
        if kind == "punct" and value == "}":
            if active_braces:
                active_braces.pop()
            continue
        if kind != "ident" or value not in aliases:
            continue
        # `type Name = ...` declares a type name; a following colon always
        # marks a record, variant, or service label and must remain literal.
        if previous.get(token_index) == "type":
            tokens[token_index] = (kind, aliases[value])
            continue
        if following.get(token_index) == ":":
            continue
        if (active_braces and active_braces[-1] == "variant"
                and previous.get(token_index) in {"{", ";"}
                and following.get(token_index) in {";", "}"}):
            continue
        tokens[token_index] = (kind, aliases[value])
    return "".join(value for _, value in tokens)


def rename_tokens_regressions() -> list[str]:
    """Exercise collisions that previously corrupted published wire labels."""
    aliases = {
        "Container": "AliasContainer",
        "TransferFromError": "AliasTransferFromError",
        "RecordField": "AliasRecordField",
        "TupleType": "AliasTupleType",
    }
    cases = [
        (
            "variant-unit-and-record-field-collision",
            "type Container = record { TransferFromError: TransferFromError; "
            "tuple: record { TupleType }; choice: variant { TransferFromError; "
            "Err: TransferFromError }; note = \"TransferFromError\"; }",
            "type AliasContainer = record { TransferFromError: AliasTransferFromError; "
            "tuple: record { AliasTupleType }; choice: variant { TransferFromError; "
            "Err: AliasTransferFromError }; note = \"TransferFromError\"; }",
        ),
        (
            "service-label-and-return-type-collision",
            "service : { TransferFromError: (TransferFromError) -> "
            "(record { RecordField: TransferFromError }); }",
            "service : { TransferFromError: (AliasTransferFromError) -> "
            "(record { RecordField: AliasTransferFromError }); }",
        ),
        (
            "quoted-label-preservation",
            "type RecordField = record { label: text; description: \"TransferFromError\"; }",
            "type AliasRecordField = record { label: text; description: \"TransferFromError\"; }",
        ),
    ]
    failures = []
    for name, source, expected in cases:
        actual = rename_tokens(source, aliases)
        if actual != expected:
            failures.append(f"{name}: expected {expected!r}, got {actual!r}")
    if failures:
        raise InterfaceError("Candid rename regressions failed: " + " | ".join(failures))
    return [name for name, _, _ in cases]


def graph_components(export: str, method_names: list[str], prefix: str) -> tuple[str, str]:
    blocks = type_blocks(export)
    available = service_methods(export)
    missing = [name for name in method_names if name not in available]
    if missing:
        raise InterfaceError(f"current export is missing required methods: {missing}")
    selected = [available[name] for name in method_names]
    needed = method_dependencies(selected, blocks)
    aliases = {name: prefix + name for name in needed}
    types = "\n".join(rename_tokens(blocks[name], aliases) for name in blocks if name in needed)
    methods = "\n".join(rename_tokens(method, aliases) for method in selected)
    return types, methods


def graph_projection(export: str, method_names: list[str], prefix: str,
                     service_name: str = "service : {") -> str:
    types, methods = graph_components(export, method_names, prefix)
    return f"{types}\n{service_name}\n{methods}\n}};\n"


def subset_export(export: str, method_names: list[str], prefix: str) -> str:
    return graph_projection(export, method_names, prefix)


def sp_intentional_drift_graph(baseline: str, current: str, prefix: str,
                               method_names: list[str]) -> tuple[str, bool]:
    """Project SP's admitted result change without changing old inputs.

    The Rust source currently adds required fields to ``LiquidatableVaultInfo``
    and adds ``admitted`` to ``LiquidationResult``.  The former cannot be
    promoted into an old public method input.  Build those two handlers from
    the published method signatures, then replace only their result type with
    the exact current ``LiquidationResult`` record (which contains the
    optional admitted field and only builtin references).
    """
    baseline_methods = service_methods(baseline)
    current_blocks = type_blocks(current)
    result_block = current_blocks.get("LiquidationResult")
    if not result_block:
        raise InterfaceError("current SP export has no LiquidationResult type")
    aliases = {"LiquidationResult": prefix + "LiquidationResult"}
    result_block = rename_tokens(result_block, aliases)
    methods = []
    for name in method_names:
        if name not in baseline_methods:
            raise InterfaceError(f"SP baseline has no intentional method {name}")
        methods.append(rename_tokens(baseline_methods[name], aliases))
    input_drift = (
        type_blocks(baseline).get("LiquidatableVaultInfo")
        != current_blocks.get("LiquidatableVaultInfo")
    )
    methods_text = "\n".join(methods)
    return f"{result_block}\nservice : {{\n{methods_text}\n}};\n", input_drift


def additive_projection(baseline: str, current: str, method_names: list[str],
                        prefix: str, intentional_drift: list[str],
                        preserve_intentional_inputs: bool = False) -> tuple[str, list[str], list[str], bool]:
    """Build one compatibility service with additive and intentional drift methods."""
    baseline_methods = service_methods(baseline)
    # A published baseline may already contain an earlier additive projection.
    # Keep its aliases stable and choose a fresh namespace for this addition.
    while any(name.startswith(prefix) for name in type_blocks(baseline)):
        prefix = prefix.rstrip("_") + "Next_"
    new_methods = [name for name in method_names if name not in baseline_methods]
    types, methods = graph_components(current, new_methods, prefix)
    intentional_input_drift = False
    if preserve_intentional_inputs:
        drift_types, drift_methods, intentional_input_drift = "", "", False
        if intentional_drift:
            drift_graph, intentional_input_drift = sp_intentional_drift_graph(
                baseline, current, prefix + "IntentionalDrift_", intentional_drift
            )
            drift_types, drift_methods = drift_graph.split("\nservice : {\n", 1)
            drift_methods = drift_methods.rsplit("\n};", 1)[0]
    else:
        drift_types, drift_methods = graph_components(
            current, intentional_drift, prefix + "IntentionalDrift_"
        )
    match = SERVICE_START.search(baseline)
    if not match:
        raise InterfaceError("compatibility baseline has no service block")
    opening = baseline.find("{", match.end())
    if opening < 0:
        raise InterfaceError("compatibility baseline service has no body")
    service_end = brace_end(baseline, opening)
    closing = service_end - 1
    before_service = baseline[:match.start()].rstrip()
    remove_spans = [
        (start, stop)
        for name, start, stop in service_method_spans(baseline)
        if name in set(intentional_drift)
    ]
    body_end = closing
    body = baseline[match.start():body_end]
    for start, stop in reversed(remove_spans):
        relative_start = start - match.start()
        relative_stop = stop - match.start()
        body = body[:relative_start] + body[relative_stop:]
    service_body = body.rstrip()
    suffix = baseline[closing:]
    appended_methods = "\n".join(piece for piece in (methods, drift_methods) if piece)
    if appended_methods:
        service_body += "\n" + appended_methods
    pieces = [before_service]
    all_types = "\n".join(piece for piece in (types, drift_types) if piece)
    if all_types:
        pieces.append(all_types)
    pieces.append(service_body + suffix)
    return ("\n".join(piece for piece in pieces if piece) + "\n",
            new_methods, intentional_drift, intentional_input_drift)


def repeated_projection_regression() -> None:
    baseline = 'type Item = nat;\nservice : { old : () -> (Item) query; };\n'
    current = ('type Item = nat;\ntype Problem = variant { Missing };\n'
               'service : { old : () -> (Item) query; '
               'first : (Item) -> (variant { Ok : Item; Err : Problem }); '
               'second : (Item) -> (variant { Ok : Item; Err : Problem }); };\n')
    first, *_ = additive_projection(baseline, current, ["first"], "Rail_", [])
    second, *_ = additive_projection(first, current, ["first", "second"], "Rail_", [])
    names = TYPE_START.findall(second)
    if len(names) != len(set(names)) or set(service_methods(second)) != {"old", "first", "second"}:
        raise InterfaceError("repeated additive projection reused a published alias")


def replace_module_paths(source: Path, text: str) -> tuple[str, list[Path]]:
    modules: list[Path] = []

    def replacement(match: re.Match[str]) -> str:
        name = match.group("name")
        root = source.parent
        candidate = root / f"{name}.rs"
        if not candidate.is_file():
            candidate = root / name / "mod.rs"
        if not candidate.is_file():
            raise InterfaceError(f"module {name} from {source} has no source file")
        modules.append(candidate.resolve())
        public = match.group("public") or ""
        return f'{match.group("indent")}#[path="{candidate.resolve()}"]\n{match.group("indent")}{public}mod {name};'

    return MODULE_DECL.sub(replacement, text), modules


def prepare_source(root: Path, kind: str, evidence: Path) -> tuple[Path, list[Path]]:
    spec = RAIL_SPECS[kind]
    source = (root / spec["source"]).resolve()
    if not source.is_file():
        raise InterfaceError(f"missing source: {source}")
    text = source.read_text()
    modules: list[Path] = []
    if kind != "backend":
        text, modules = replace_module_paths(source, text)
    # Do not allow a checked-in export macro or a cfg(test) export helper to
    # define the host output.  The exporter below is always appended after all
    # handlers, which avoids the empty-service-before-handlers trap.
    text = re.sub(r"(?m)^\s*(?:ic_cdk::)?export_candid!\(\);\s*\n?", "", text)
    text = re.sub(r"(?m)^\s*candid::export_service!\(\);\s*\n?", "", text)
    if kind == "backend":
        replaced, count = re.subn(r"fn\s+main\s*\(\)\s*\{\s*\}",
                                  "fn main() { print!(\"{}\", __export_service()); }", text, count=1)
        if count != 1:
            raise InterfaceError("backend source must have exactly one empty fn main()")
        text = replaced
    else:
        text += "\nfn main() { print!(\"{}\", __export_service()); }\n"
    text += "\ncandid::export_service!();\n"
    destination = evidence / kind / "export.rs"
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(text)
    # Hash the complete current crate source tree, including backend `src/lib.rs`
    # and its modules.  The backend binary imports that library crate, so a
    # root-file-only hash would not bind the export to the exact current code.
    source_files = sorted(source.parent.rglob("*.rs"))
    return destination, source_files


def isolated_manifest(root: Path, kind: str, evidence: Path) -> tuple[Path, Path]:
    spec = RAIL_SPECS[kind]
    original = (root / spec["manifest"]).resolve()
    text = original.read_text()
    # Give each isolated exporter a package identity of its own.  The library
    # crate name stays stable below so current Rust source paths continue to
    # compile, while Cargo fingerprints from adjacent exporters cannot share
    # a package artifact accidentally in the common target directory.
    evidence_slug = re.sub(r"[^A-Za-z0-9]+", "-", evidence.name).strip("-").lower() or "capture"
    exporter_package = f"{spec['package']}-interface-export-{kind}-{evidence_slug}"
    text, package_count = re.subn(
        r'(?m)^name\s*=\s*"[^"]+"\s*$',
        f'name = "{exporter_package}"',
        text,
        count=1,
    )
    if package_count != 1:
        raise InterfaceError(f"manifest has no package name: {original}")
    lib_match = re.search(r"(?ms)^\[lib\]\s*\n(.*?)(?=^\[[^\n]+\]\s*$|\Z)", text)
    if not lib_match:
        raise InterfaceError(f"manifest has no lib section: {original}")
    lib_body = lib_match.group(1)
    if not re.search(r"(?m)^name\s*=", lib_body):
        lib_body = f'name = "{spec["package"]}"\n' + lib_body
        text = text[:lib_match.start(1)] + lib_body + text[lib_match.end(1):]
    # The isolated manifest is intentionally derived from the current package
    # manifest.  Make every local dependency absolute so no stale checkout or
    # /private/tmp source can satisfy the export build.
    def path_replacement(match: re.Match[str]) -> str:
        relative = match.group(1)
        return f'path = "{(original.parent / relative).resolve()}"'
    text = re.sub(r'path\s*=\s*"([^"]+)"', path_replacement, text)
    # The exporter is a native non-test binary.  Keeping the package's large
    # PocketIC/state-machine dev graph here makes Cargo resolve and compile
    # unrelated test-only crates, so remove that section from the isolated
    # manifest while leaving the checked-in manifest untouched.
    text = re.sub(
        r"(?ms)^\[dev-dependencies\]\s*\n.*?(?=^\[[^\]]+\]\s*$|\Z)",
        "",
        text,
        count=1,
    )
    # This manifest lives under the evidence directory rather than the
    # checkout.  Make it an independent workspace so Cargo never discovers
    # or mutates the production workspace metadata.
    text = text.rstrip() + "\n\n[workspace]\n"
    text += '\n\n[[bin]]\nname = "interface_export"\npath = "export.rs"\n'
    destination = evidence / kind / "Cargo.toml"
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(text)
    lock_source = (root / "Cargo.lock").resolve()
    if not lock_source.is_file():
        raise InterfaceError(f"missing workspace lockfile: {lock_source}")
    lock_copy = destination.parent / "Cargo.lock"
    shutil.copyfile(lock_source, lock_copy)
    return destination, lock_copy


def dependency_names(manifest_text: str, section: str) -> set[str]:
    match = re.search(
        rf"(?ms)^\[{re.escape(section)}\]\s*\n(.*?)(?=^\[[^\n]+\]\s*$|\Z)",
        manifest_text,
    )
    if not match:
        return set()
    return {
        item.group(1)
        for item in re.finditer(r"(?m)^\s*([A-Za-z0-9_-]+)\s*=", match.group(1))
    }


def adapt_isolated_lock(root: Path, kind: str, manifest: Path, lock_copy: Path,
                        evidence: Path, cargo: Path) -> dict:
    """Record and apply the lock-only change caused by removing dev deps.

    The production workspace lock is copied verbatim first.  Cargo's offline
    resolver cannot start from the stripped manifest because the pinned
    `ic-cdk` release is yanked, so the isolated lock first removes only the
    stripped package's dev dependency edges, then ``cargo metadata --offline``
    normalizes that isolated lock.  No production lock or manifest is written.
    """
    spec = RAIL_SPECS[kind]
    manifest_text_isolated = manifest.read_text()
    package_name_match = re.search(r'(?m)^name\s*=\s*"([^"]+)"\s*$', manifest_text_isolated)
    if not package_name_match:
        raise InterfaceError(f"isolated manifest has no package name: {manifest}")
    exporter_package = package_name_match.group(1)
    original_manifest = (root / spec["manifest"]).resolve()
    manifest_text = original_manifest.read_text()
    normal = dependency_names(manifest_text, "dependencies")
    dev = dependency_names(manifest_text, "dev-dependencies") - normal
    before = digest(lock_copy)
    text = lock_copy.read_text()
    package_match = re.search(
        rf'(?ms)^\[\[package\]\]\nname = "{re.escape(spec["package"])}"\n.*?(?=^\[\[package\]\]|\Z)',
        text,
    )
    if not package_match:
        raise InterfaceError(f"lock has no package entry for {spec['package']}")
    block = package_match.group(0)
    removed: list[str] = []
    adapted: list[str] = []
    in_dependencies = False
    for line in block.splitlines(True):
        stripped = line.strip()
        if stripped == "dependencies = [":
            in_dependencies = True
            adapted.append(line)
            continue
        if in_dependencies and stripped == "]":
            in_dependencies = False
            adapted.append(line)
            continue
        if in_dependencies and stripped.startswith('"') and stripped.endswith('",'):
            token = stripped[1:-2].split(" ", 1)[0]
            if token in dev:
                removed.append(token)
                continue
        adapted.append(line)
    adapted_block = "".join(adapted).replace(
        f'name = "{spec["package"]}"', f'name = "{exporter_package}"', 1
    )
    lock_copy.write_text(text[:package_match.start()] + adapted_block + text[package_match.end():])
    pruned = digest(lock_copy)
    record = {
        "method": "copy-root-lock-and-prune-stripped-dev-edges",
        "root_lock": str((root / "Cargo.lock").resolve()),
        "isolated_lock": str(lock_copy),
        "root_lock_sha256": digest((root / "Cargo.lock").resolve()),
        "copied_lock_sha256": before,
        "pruned_lock_sha256": pruned,
        "changed": before != pruned,
        "stripped_dev_dependencies": sorted(dev),
        "exporter_package_name": exporter_package,
        "removed_lock_edges": sorted(removed),
        "production_lock_touched": False,
        "offline": True,
        "locked_capture": True,
        "resolver_note": "the copied lock is pruned only for stripped dev edges; offline Cargo metadata then refreshes the isolated lock while retaining checked-in versions",
    }
    metadata_command = [
        str(cargo), "metadata", "--offline", "--format-version", "1",
        "--manifest-path", str(manifest),
    ]
    metadata_env = dict(**__import__("os").environ)
    metadata_env["CARGO_NET_OFFLINE"] = "true"
    metadata = subprocess.run(
        metadata_command,
        cwd=manifest.parent,
        env=metadata_env,
        capture_output=True,
        text=True,
    )
    metadata_stdout = evidence / kind / "lock-metadata.stdout"
    metadata_stderr = evidence / kind / "lock-metadata.stderr"
    metadata_stdout.write_text(metadata.stdout)
    metadata_stderr.write_text(metadata.stderr)
    record.update({
        "metadata_command": metadata_command,
        "metadata_exit_code": metadata.returncode,
        "metadata_stdout": str(metadata_stdout),
        "metadata_stderr": str(metadata_stderr),
        "metadata_stdout_sha256": digest(metadata_stdout),
    })
    if metadata.returncode:
        (evidence / kind / "lock-adaptation.json").write_text(json.dumps(record, indent=2) + "\n")
        raise InterfaceError(
            f"offline isolated lock metadata failed for {kind}: {metadata.stderr.strip()}"
        )
    record["adapted_lock_sha256"] = digest(lock_copy)
    (evidence / kind / "lock-adaptation.json").write_text(json.dumps(record, indent=2) + "\n")
    return record


def run_capture(cargo: Path, manifest: Path, evidence: Path, kind: str,
                target_dir: Path) -> tuple[int, str, str]:
    target = target_dir.resolve()
    command = [
        str(cargo), "run", "--offline", "--locked", "--jobs", "2", "--target-dir", str(target),
        "--manifest-path", str(manifest), "--bin", "interface_export",
    ]
    (evidence / kind / "capture-command.json").write_text(json.dumps({
        "command": command, "manifest": str(manifest), "target": str(target),
        "network": "not requested", "source": "current checkout",
    }, indent=2) + "\n")
    env = dict(**__import__("os").environ)
    env["CARGO_TARGET_DIR"] = str(target)
    env["CARGO_NET_OFFLINE"] = "true"
    env["CARGO_BUILD_JOBS"] = "2"
    process = subprocess.run(command, cwd=manifest.parent, env=env,
                             capture_output=True, text=True)
    (evidence / kind / "capture.stdout").write_text(process.stdout)
    (evidence / kind / "capture.stderr").write_text(process.stderr)
    if process.returncode == 0:
        (evidence / kind / "full.did").write_text(process.stdout)
    return process.returncode, process.stdout, process.stderr


def didc_check(didc: Path, name: str, left: Path, right: Path, evidence: Path) -> dict:
    command = [str(didc), "check", str(left), str(right)]
    process = subprocess.run(command, capture_output=True, text=True)
    result = {"name": name, "command": command, "exit_code": process.returncode,
              "stdout": process.stdout, "stderr": process.stderr}
    (evidence / f"check-{name}.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def bind(didc: Path, source: Path, evidence: Path, kind: str) -> None:
    for language, suffix in (("js", ".did.js"), ("ts", ".did.d.ts")):
        process = subprocess.run([str(didc), "bind", str(source), "-t", language],
                                 capture_output=True, text=True)
        if process.returncode:
            raise InterfaceError(f"didc bind {kind}/{language} failed: {process.stderr}")
        (evidence / kind / f"interface{suffix}").write_text(process.stdout)


def process_kind(root: Path, evidence: Path, didc: Path, cargo: Path,
                 kind: str, capture: bool, target_dir: Path,
                 baseline_override: Path | None = None) -> dict:
    spec = RAIL_SPECS[kind]
    source_file, source_files = prepare_source(root, kind, evidence)
    manifest, lock_copy = isolated_manifest(root, kind, evidence)
    input_record = {str(path): {"sha256": digest(path), "size": path.stat().st_size}
                    for path in source_files + [(root / spec["manifest"]).resolve()]}
    lock = root / "Cargo.lock"
    if lock.is_file():
        input_record[str(lock.resolve())] = {"sha256": digest(lock), "size": lock.stat().st_size}
    legacy_baseline = (root / spec["baseline"]).resolve()
    baseline = baseline_override.resolve() if baseline_override else legacy_baseline
    if baseline_override and kind != "backend":
        raise InterfaceError("--baseline-backend is only valid for backend")
    if not baseline.is_file():
        raise InterfaceError(f"missing compatibility baseline: {baseline}")
    input_record[str(baseline)] = {"sha256": digest(baseline), "size": baseline.stat().st_size}
    result = {"kind": kind, "source": str(source_file), "manifest": str(manifest),
              "lock_copy": str(lock_copy), "inputs": input_record, "captured": False,
              "baseline": str(baseline), "baseline_sha256": digest(baseline)}
    if baseline != legacy_baseline:
        result.update({"legacy_baseline": str(legacy_baseline),
                       "legacy_baseline_sha256": digest(legacy_baseline)})
    if not capture:
        result["status"] = "prepared; native capture intentionally not run"
        return result
    result["lock_adaptation"] = adapt_isolated_lock(
        root, kind, manifest, lock_copy, evidence, cargo
    )
    code, stdout, stderr = run_capture(cargo, manifest, evidence, kind, target_dir)
    if code:
        result.update({"status": "capture failed", "exit_code": code, "stderr": stderr})
        return result
    export = evidence / kind / "full.did"
    methods = service_methods(stdout)
    missing = [name for name in spec["methods"] if name not in methods]
    if missing:
        raise InterfaceError(f"{kind} current export missing required methods: {missing}")
    projected = evidence / kind / "additive-projection.did"
    baseline_text = baseline.read_text()
    projection, new_methods, drift_methods, intentional_input_drift = additive_projection(
        baseline_text,
        stdout,
        spec["methods"],
        f"{kind.title()}RailWire_",
        spec["intentional_drift"],
        preserve_intentional_inputs=(kind == "sp"),
    )
    projected.write_text(projection.rstrip() + "\n")
    baseline_methods = set(service_methods(baseline_text))
    unchanged = sorted(
        baseline_methods
        & set(methods)
        - set(new_methods)
        - set(spec["intentional_drift"])
    )
    checks = []
    baseline_subset = evidence / kind / "unchanged-baseline.did"
    source_subset = evidence / kind / "unchanged-source.did"
    baseline_subset.write_text(subset_export(baseline_text, unchanged, f"{kind.title()}Baseline_") )
    source_subset.write_text(subset_export(stdout, unchanged, f"{kind.title()}Source_") )
    checks.append(didc_check(didc, f"{kind}-upgrade-compatible", projected, baseline, evidence / kind))
    checks.append(didc_check(didc, f"{kind}-unchanged-forward", source_subset, baseline_subset, evidence / kind))
    checks.append(didc_check(didc, f"{kind}-unchanged-reverse", baseline_subset, source_subset, evidence / kind))
    # Compare the new method graph with and without generated aliases in both
    # directions.  This catches a label/type-name collision before that graph
    # is embedded in the additive projection.
    new_source_wire = evidence / kind / "new-source-wire.did"
    new_generated_wire = evidence / kind / "new-generated-wire.did"
    new_source_wire.write_text(graph_projection(stdout, new_methods, ""))
    new_generated_wire.write_text(
        graph_projection(stdout, new_methods, f"{kind.title()}RailWire_")
    )
    new_wire_checks = [
        didc_check(didc, f"{kind}-new-wire-forward",
                   new_source_wire, new_generated_wire, evidence / kind),
        didc_check(didc, f"{kind}-new-wire-reverse",
                   new_generated_wire, new_source_wire, evidence / kind),
    ]
    legacy_checks = []
    if baseline != legacy_baseline:
        # Keep the checked-in DID as an explicit historical comparison.  A
        # pre-existing metadata drift is reported separately and cannot hide
        # strict checks against the caller-selected deployed-source baseline.
        legacy_text = legacy_baseline.read_text()
        legacy_projection, legacy_new, _, _ = additive_projection(
            legacy_text,
            stdout,
            spec["methods"],
            f"{kind.title()}LegacyRailWire_",
            spec["intentional_drift"],
            preserve_intentional_inputs=(kind == "sp"),
        )
        legacy_projected = evidence / kind / "legacy-additive-projection.did"
        legacy_projected.write_text(legacy_projection.rstrip() + "\n")
        legacy_methods = set(service_methods(legacy_text))
        legacy_unchanged = sorted(
            legacy_methods
            & set(methods)
            - set(legacy_new)
            - set(spec["intentional_drift"])
        )
        legacy_baseline_subset = evidence / kind / "legacy-unchanged-baseline.did"
        legacy_source_subset = evidence / kind / "legacy-unchanged-source.did"
        legacy_baseline_subset.write_text(
            subset_export(legacy_text, legacy_unchanged, f"{kind.title()}LegacyBaseline_")
        )
        legacy_source_subset.write_text(
            subset_export(stdout, legacy_unchanged, f"{kind.title()}LegacySource_")
        )
        legacy_checks.extend([
            didc_check(didc, f"{kind}-legacy-upgrade-compatible",
                       legacy_projected, legacy_baseline, evidence / kind),
            didc_check(didc, f"{kind}-legacy-unchanged-forward",
                       legacy_source_subset, legacy_baseline_subset, evidence / kind),
            didc_check(didc, f"{kind}-legacy-unchanged-reverse",
                       legacy_baseline_subset, legacy_source_subset, evidence / kind),
        ])
    bind(didc, export, evidence, kind)
    result.update({"captured": True, "status": "captured", "export": str(export),
                   "projection": str(projected),
                   "new_methods": new_methods, "already_published_methods": sorted(
                       set(spec["methods"]) & baseline_methods
                   ),
                   "unexpected_current_additions": sorted(set(methods) - baseline_methods - set(new_methods)),
                   "unchanged_methods": unchanged, "intentional_drift": drift_methods,
                   "checks": checks + new_wire_checks, "legacy_checks": legacy_checks,
                   "new_wire_checks": new_wire_checks,
                   "intentional_input_drift_preserved": intentional_input_drift,
                   "legacy_compatibility_passed": (not legacy_checks) or all(
                       check["exit_code"] == 0 for check in legacy_checks
                   ), "export_sha256": digest(export),
                   "projection_sha256": digest(projected)})
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--didc", type=Path, default=Path.home() / ".cargo/bin/didc")
    parser.add_argument("--cargo", type=Path, default=Path.home() / ".cargo/bin/cargo")
    parser.add_argument(
        "--target-dir",
        type=Path,
        default=Path("/private/tmp/rumi-sp-reopening-20261023/native-target"),
        help="Shared Cargo target directory for all three native exporters.",
    )
    parser.add_argument("--capture", action="store_true",
                        help="Run native Cargo exporters and didc checks.")
    parser.add_argument(
        "--baseline-backend", type=Path,
        help="Use an explicit deployed-source backend DID for strict checks; the checked-in DID is still checked separately.",
    )
    args = parser.parse_args()
    root = args.root.resolve()
    evidence = args.evidence.resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    target_dir = args.target_dir.resolve()
    didc = args.didc.resolve()
    baseline_backend = args.baseline_backend.resolve() if args.baseline_backend else None
    if baseline_backend and not baseline_backend.is_file():
        raise SystemExit(f"missing --baseline-backend: {baseline_backend}")
    # Preserve the cargo shim's basename.  Resolving ~/.cargo/bin/cargo to
    # rustup changes argv[0] and makes rustup treat `run` as a toolchain name.
    cargo = args.cargo.absolute()
    if args.capture and (not didc.is_file() or not cargo.is_file()):
        raise SystemExit(f"capture requires didc and cargo: {didc}, {cargo}")
    results = []
    rename_regressions = []
    failure = None
    try:
        rename_regressions = rename_tokens_regressions()
        repeated_projection_regression()
        for kind in RAIL_SPECS:
            results.append(process_kind(root, evidence, didc, cargo, kind, args.capture,
                                       target_dir, baseline_backend if kind == "backend" else None))
    except Exception as error:
        failure = f"{type(error).__name__}: {error}"
        (evidence / "failure.txt").write_text(failure + "\n")
    capture_results = args.capture and failure is None and all(
        result.get("captured") for result in results
    )
    compatibility_passed = (not args.capture) or (
        failure is None and capture_results and all(
            check.get("exit_code") == 0
            for result in results
            for check in result.get("checks", [])
        )
    )
    report = {"prepared": failure is None, "captured": capture_results,
              "compatibility_passed": compatibility_passed,
              "local_only": True, "production_calls": 0, "results": results,
              "root": str(root), "didc": str(didc), "cargo": str(cargo),
              "target_dir": str(target_dir),
              "baseline_backend": str(baseline_backend) if baseline_backend else None,
              "rename_regressions": {"passed": failure is None,
                                     "cases": rename_regressions},
              "harness_sha256": digest(Path(__file__).resolve())}
    if failure:
        report["failure"] = failure
    (evidence / "verification.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    return 1 if failure or (args.capture and (not capture_results or not compatibility_passed)) else 0


if __name__ == "__main__":
    sys.exit(main())
