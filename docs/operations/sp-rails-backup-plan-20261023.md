# SP rails backup preservation plan - 2026-10-23

The user explicitly authorized repairing, deploying and restoring conversion/liquidation rails without another approval. This is this task's concrete backup operation plan, not an extension of an earlier inspection operator's authority.

Backend `tfesu-vyaaa-aaaap-qrd7a-cai` has ten snapshots, its capacity limit. Before any backend replacement, its oldest snapshot (timestamp `1787733991429007038`, total size `88413621`) has been fully downloaded in native upload-restorable format. Payload and metadata sizes/SHA256 are verified and retained in the operator's private evidence store. No snapshot has yet been replaced/deleted for this task.

After independent review of that preservation proof, root may stop the backend, create ONE current snapshot with `--replace` naming ONLY that oldest ID, download and verify the current snapshot, then restart the unchanged backend. Re-list and compare exact settings, original module, all other snapshot IDs and Running status. Keep both full local payload sets; never prune them in this task. Old snapshot identity is replaced on-network; its entire prior state remains in the saved native restore payload. No application-state correction or fund movement is part of backup creation.

Record a durable submission marker before the replacement. Submit once. If the CLI result is ambiguous, inspect authoritative snapshot inventory and status before any further replacement; do not repeat the mutation or switch executor. The original code must be restarted after an unambiguous backup failure when safe. Do not upgrade without a verified current recovery point and reviewed migration evidence. SP and 3pool already have fresh verified downloaded snapshots, with no older snapshots replaced.

This plan authorizes no unrelated deletion, controller change, reinstall, balance rewrite, recovery packet replay, or legacy excess disposition. Source implementation and all deterministic/adversarial release gates remain separate from this backup operation.
