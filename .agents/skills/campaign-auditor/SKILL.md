---
name: campaign-auditor
description: >-
  Dedicated Campaign Auditor & Reorganizer runbook for Chronicle RPG campaigns.
  Use whenever auditing a campaign database against source reference documents,
  identifying mislabeled registers, fixing broken links or lore contradictions,
  normalizing slugs/duplicates, populating missing entities/histories/items, or
  running periodic post-session hygiene.
---

# Chronicle Campaign Auditor & Reorganizer Skill

Activate this skill when acting as a **campaign maintainer / archivist / reorganizer** (separate from live play narration in `gm-campaign`). Your mission is to keep `<campaign>/campaign.db` and its auto-compiled `<campaign>/docs/` **100% consistent, accurately classified, free of drift, and faithful to source material**.

---

## 1. Golden Safety Rules

1. **Always Back Up First:** Before performing bulk edits, reclassifications, merges, or deletions, call `backup_campaign({ "label": "before-audit" })` (or copy `campaign.db` after WAL checkpoint).
2. **Never Manually Edit `<campaign>/docs/`:** All changes go through Chronicle tools so the SQLite database and SHA-256 event ledger remain canonical.
3. **Never Overwrite Player Progress:** When reconciling a campaign against source documents, preserve live play mutations (HP/resource changes, conditions, reading logs, moved NPCs, ticked clocks) unless explicitly fixing an error.
4. **Preserve Exact Voice & Canon:** When importing external source documents, attach them verbatim via `import_attachment` so future sessions can always inspect the original phrasing without bloating `get_scene_context`.

---

## 2. The 7-Step Audit & Reorganization Protocol

Execute these steps **in order** whenever auditing or cleaning up a campaign:

### Step 1 — Snapshot & Automated Linter
1. Call `campaign_status` to check counts (`characters`, `places`, `catalog_items`, `story_texts`, `entity_histories`, `lore_entries`, `attachments`, `clocks`, `events`), campaign metadata, and `ledger_hash_chain_valid`.
2. Call `diagnose_campaign` to surface all machine-detected errors, warnings, and info items:
   - `LEDGER_CHAIN_INVALID` / `ATTACHMENT_FILE_MISSING` (Errors)
   - `MISLABELED_LORE_AS_TEXT` / `MISLABELED_LORE_AS_ITEM` / `MISLABELED_LORE_AS_PLACE` / `MISLABELED_LORE_AS_CHARACTER` / `MISLABELED_LORE_AS_HISTORY`
   - `HISTORY_KIND_MISMATCH` / `BROKEN_*` dangling references
   - `UNSYNCED_CARRYABLE_TEXT` / `INVENTORY_CATALOG_DESYNC`
   - `DUPLICATE_NAME_SAME_REGISTER` / `DUPLICATE_SLUG_CROSS_REGISTER`
   - `ORPHANED_DOC_FILE` / `PLACEHOLDER_CAMPAIGN_META` / `GENERIC_ESTABLISHMENT_TYPE` / `ONE_WAY_ROUTE` / `MISSING_ENTITY_HISTORY`

### Step 2 — Campaign Metadata Check
- If campaign `name`, `description`, `ruleset_notes`, or `world_clock` still contain generic placeholders (e.g. `"My Campaign"`, `"Campaign premise"`), fix them cleanly with `update_campaign_meta`.

### Step 3 — Source-to-DB Fidelity Audit (When Source Files Are Provided)
Compare source files against SQLite records across **all 6 registers**:
1. **Attachments (`import_attachment` / `create_reference_doc`):**
   - Import canonical source files (skipping superseded draft duplicates!) into `docs/attachments/files/` with clear `summary`, `extracted_notes`, `keywords`, and `related_ids`.
2. **Register Purity (`reclassify_entity`):**
   - Physical readable documents (`book`, `scroll`, `grimoire`, `ritual-manual`, `journal`, `letter`, `tablet`) → `texts` (NOT `lore`).
   - Physical carryable objects (`weapon`, `tool`, `artifact`, `keepsake`, `record`, `key`) → `items` (NOT `lore`), with `origin_and_maker`, `previous_owners`, `current_holder_id`, and `significance`.
   - Named establishments (`tavern`, `pavilion`, `garden`, `guild`, `academy`, `temple`) → `places` with `place_type: "establishment"` (NOT generic `"building"` or `lore`).
   - Named NPCs trapped inside faction lore paragraphs → first-class `characters` linked via `upsert_relation` (`REPORTS_TO`, `MEMBER_OF`, `OPERATES`).
   - Dated historical milestones tied to a place, establishment, sect, item, or character → `entity_histories` (`add_entity_history`, NOT standalone `lore`).
3. **Lore Accuracy Check:**
   - Verify landmarks on establishments match the exact source layout.
   - Verify item/text physical nature and holders match source rules (e.g., consumable/blanking slips vs. retained case archives vs. client-held volumes).
   - Verify characters actually carry their signature tools, seals, ledgers, or keepsakes (`upsert_item` auto-syncs into character inventories!).

### Step 4 — Slug Standardization & Deduplication
- Use `rename_or_merge_entity` (`allow_merge: true`) whenever two entries represent the same entity under different slugs or when a slug does not match its canonical name. All cross-references across `relations`, `entity_histories`, `lore`, `texts`, `items`, `attachments`, `clocks`, `routes`, and character locations update automatically.

### Step 5 — Knowledge Graph & Route Symmetry
- Ensure every establishment links to its parent settlement and governing faction/owner (`GOVERNS`, `OWNS_AND_OPERATES`, `BRANCH_OF`, `HEADQUARTERED_AT`, `REPORTS_TO`).
- Ensure travel routes between settlements are reciprocal (`update_place` `action: "route"` on both ends) and do not point a city directly to an interior building of another city.

### Step 6 — Story-Engine Progress Clocks
- Check if the world's core narrative tensions (bottlenecks, succession crises, ticking secrets, rival factions) are represented as trackable `ProgressClock`s (`manage_clock` `create` / `update`).

### Step 7 — Safe Hygiene Pass & Final Verification
1. Run `cleanup_campaign({ "dry_run": true })` and inspect `actions`.
2. Apply with `cleanup_campaign({ "dry_run": false })` (prunes stale `.md` files, re-syncs carryable texts & inventories, deduplicates tags/keywords/links, normalizes enum casing, recompiles all docs).
3. Re-run `diagnose_campaign` and confirm:
   - `error_count == 0`
   - `warning_count == 0`
   - `ledger_hash_chain_valid == true`
4. Present a crisp before/after audit report listing every fix made and any open GM judgment calls.
