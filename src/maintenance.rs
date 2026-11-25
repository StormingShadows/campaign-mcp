use crate::compiler::{list_orphaned_docs, prune_orphaned_docs};
use crate::engine::{is_carryable_text_medium, CampaignEngine};
use crate::models::{
    slugify, CampaignMeta, CatalogItem, CharacterBaseline, EntityHistoryEntry, EventPayload,
    LoreEntry, PlaceBaseline, ProgressClock, StoryAttachment, StoryText, WorldEvent,
};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

// =============================================================================
// Diagnostics / Campaign Linter
// =============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticFinding {
    /// Machine-readable diagnostic code (e.g. MISLABELED_LORE_AS_TEXT, BROKEN_ITEM_HOLDER)
    pub code: String,
    /// "error" | "warning" | "info"
    pub severity: String,
    /// Register where the issue was found ("character", "place", "lore", "text", "item", "history", "attachment", "relation", "ledger", "docs")
    pub register: String,
    /// Affected entity ID, history ID, or relative path
    pub entity_id: String,
    /// Clear explanation of what is wrong or mislabeled
    pub message: String,
    /// Suggested Chronicle tool to fix or reorganize this issue (if actionable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_tool: Option<String>,
    /// Ready-to-use suggested arguments for `suggested_tool`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_args: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub healthy: bool,
    pub error_count: usize,
    pub warning_count: usize,
    pub info_count: usize,
    pub ledger_hash_chain_valid: bool,
    pub counts: BTreeMap<String, usize>,
    pub findings: Vec<DiagnosticFinding>,
}

pub fn diagnose_campaign(engine: &CampaignEngine) -> Result<DiagnosticReport> {
    let db = engine.db();
    let mut findings: Vec<DiagnosticFinding> = Vec::new();

    let chain_rep = db.verify_event_chain_detailed()?;
    let (chain_valid, event_count) = (chain_rep.valid, chain_rep.total_events);
    if !chain_valid {
        findings.push(DiagnosticFinding {
            code: "LEDGER_CHAIN_INVALID".to_string(),
            severity: "error".to_string(),
            register: "ledger".to_string(),
            entity_id: format!(
                "event#{}",
                chain_rep.first_failed_seq.unwrap_or(0)
            ),
            message: format!(
                "SHA-256 event hash chain failed at event #{} ({}): expected '{}', stored '{}'.",
                chain_rep.first_failed_seq.unwrap_or(0),
                chain_rep.failure_kind.as_deref().unwrap_or("unknown"),
                chain_rep.expected_hash.as_deref().unwrap_or("?"),
                chain_rep.stored_hash.as_deref().unwrap_or("?")
            ),
            suggested_tool: None,
            suggested_args: None,
        });
    }

    // 0. Check for placeholder campaign metadata
    if let Ok(meta) = db.get_campaign_meta() {
        let name_l = meta.name.trim().to_lowercase();
        let desc_l = meta.description.trim().to_lowercase();
        if matches!(name_l.as_str(), "" | "my campaign" | "untitled" | "new campaign")
            || matches!(desc_l.as_str(), "" | "campaign premise" | "your world premise")
        {
            findings.push(DiagnosticFinding {
                code: "PLACEHOLDER_CAMPAIGN_META".to_string(),
                severity: "info".to_string(),
                register: "campaign".to_string(),
                entity_id: meta.id.clone(),
                message: format!(
                    "Campaign metadata still uses placeholder values (name='{}', description='{}'). Update via update_campaign_meta.",
                    meta.name, meta.description
                ),
                suggested_tool: Some("update_campaign_meta".to_string()),
                suggested_args: Some(json!({
                    "name": "<Campaign Title>",
                    "description": "<Campaign Premise>"
                })),
            });
        }
    }

    let chars = engine.list_characters()?;
    let places = engine.list_places()?;
    let lore_entries = db.list_lore(None)?;
    let texts = db.list_texts()?;
    let items = db.list_catalog_items()?;
    let histories = db.list_all_histories()?;
    let attachments = db.list_attachments()?;
    let relations = db.list_all_relations()?;
    let clocks = db.list_clocks()?;

    let mut counts = BTreeMap::new();
    counts.insert("characters".to_string(), chars.len());
    counts.insert("places".to_string(), places.len());
    counts.insert("lore_entries".to_string(), lore_entries.len());
    counts.insert("story_texts".to_string(), texts.len());
    counts.insert("catalog_items".to_string(), items.len());
    counts.insert("entity_histories".to_string(), histories.len());
    counts.insert("attachments".to_string(), attachments.len());
    counts.insert("relations".to_string(), relations.len());
    counts.insert("clocks".to_string(), clocks.len());
    let factions = db.list_factions()?;
    counts.insert("factions".to_string(), factions.len());
    counts.insert("events".to_string(), event_count);

    // Build known entity ID sets
    let char_ids: BTreeSet<String> = chars.iter().map(|c| c.baseline.id.clone()).collect();
    let place_ids: BTreeSet<String> = places.iter().map(|p| p.baseline.id.clone()).collect();
    let lore_ids: BTreeSet<String> = lore_entries.iter().map(|l| l.id.clone()).collect();
    let text_ids: BTreeSet<String> = texts.iter().map(|t| t.id.clone()).collect();
    let item_ids: BTreeSet<String> = items.iter().map(|i| i.id.clone()).collect();
    let att_ids: BTreeSet<String> = attachments.iter().map(|a| a.id.clone()).collect();
    let clock_ids: BTreeSet<String> = clocks.iter().map(|c| c.id.clone()).collect();
    let faction_ids: BTreeSet<String> = factions.iter().map(|f| f.id.clone()).collect();

    let mut all_known_ids: BTreeSet<String> = BTreeSet::new();
    for s in [
        &char_ids,
        &place_ids,
        &lore_ids,
        &text_ids,
        &item_ids,
        &att_ids,
        &clock_ids,
        &faction_ids,
    ] {
        all_known_ids.extend(s.iter().cloned());
    }

    let detect_actual_kind = |eid: &str| -> Option<&'static str> {
        if char_ids.contains(eid) {
            Some("character")
        } else if place_ids.contains(eid) {
            Some("place")
        } else if faction_ids.contains(eid) {
            Some("faction")
        } else if text_ids.contains(eid) {
            Some("text")
        } else if item_ids.contains(eid) {
            Some("item")
        } else if lore_ids.contains(eid) {
            Some("lore")
        } else if att_ids.contains(eid) {
            Some("attachment")
        } else {
            None
        }
    };

    // 1. Orphaned compiled Markdown files in docs/
    for orphan in list_orphaned_docs(db, &engine.docs_dir())? {
        findings.push(DiagnosticFinding {
            code: "ORPHANED_DOC_FILE".to_string(),
            severity: "warning".to_string(),
            register: "docs".to_string(),
            entity_id: orphan.clone(),
            message: format!(
                "Stale compiled Markdown file 'docs/{orphan}' has no matching record in SQLite."
            ),
            suggested_tool: Some("cleanup_campaign".to_string()),
            suggested_args: Some(json!({ "dry_run": false })),
        });
    }

    // 2. Character checks
    let mut char_by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for cp in &chars {
        let id = &cp.baseline.id;
        char_by_name
            .entry(cp.baseline.name.trim().to_lowercase())
            .or_default()
            .push(id.clone());

        if let Some(ref loc) = cp.current.current_location {
            if !loc.is_empty() && !place_ids.contains(loc) {
                findings.push(DiagnosticFinding {
                    code: "BROKEN_CHARACTER_LOCATION".to_string(),
                    severity: "warning".to_string(),
                    register: "character".to_string(),
                    entity_id: id.clone(),
                    message: format!(
                        "Character '{}' is currently located at unknown place slug '{}'.",
                        cp.baseline.name, loc
                    ),
                    suggested_tool: Some("move_character".to_string()),
                    suggested_args: Some(json!({
                        "character_id": id,
                        "to_place_id": "<valid-place-slug>"
                    })),
                });
            }
        } else {
            findings.push(DiagnosticFinding {
                code: "INCOMPLETE_CHARACTER".to_string(),
                severity: "info".to_string(),
                register: "character".to_string(),
                entity_id: id.clone(),
                message: format!("Character '{}' has no current location set.", cp.baseline.name),
                suggested_tool: Some("move_character".to_string()),
                suggested_args: Some(json!({
                    "character_id": id,
                    "to_place_id": "<place-slug>"
                })),
            });
        }
    }
    push_duplicate_name_findings(&mut findings, "character", &char_by_name);

    // 3. Place checks
    let mut place_by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let place_routes_map: BTreeMap<String, BTreeSet<String>> = places
        .iter()
        .map(|p| {
            (
                p.baseline.id.clone(),
                p.current
                    .routes
                    .iter()
                    .map(|r| r.destination_id.clone())
                    .collect(),
            )
        })
        .collect();
    let history_entity_ids: BTreeSet<String> =
        histories.iter().map(|h| h.entity_id.clone()).collect();
    for pp in &places {
        let id = &pp.baseline.id;
        place_by_name
            .entry(pp.baseline.name.trim().to_lowercase())
            .or_default()
            .push(id.clone());

        // Detect named establishments typed as generic "building"
        if pp.baseline.place_type.eq_ignore_ascii_case("building") {
            let nl = pp.baseline.name.to_lowercase();
            if ["pavilion", "garden", "tavern", "inn", "guild", "hall", "academy", "temple", "shrine", "sect"]
                .iter()
                .any(|k| nl.contains(k))
            {
                findings.push(DiagnosticFinding {
                    code: "GENERIC_ESTABLISHMENT_TYPE".to_string(),
                    severity: "info".to_string(),
                    register: "place".to_string(),
                    entity_id: id.clone(),
                    message: format!(
                        "Place '{}' is typed as generic 'building' but appears to be a named establishment. Consider setting place_type='establishment'.",
                        pp.baseline.name
                    ),
                    suggested_tool: Some("update_place_baseline".to_string()),
                    suggested_args: Some(json!({
                        "place_id": id,
                        "place_type": "establishment",
                        "reason": "Clarified named establishment place_type"
                    })),
                });
            }
        }

        if pp.baseline.place_type.eq_ignore_ascii_case("establishment")
            && !history_entity_ids.contains(id)
        {
            findings.push(DiagnosticFinding {
                code: "MISSING_ENTITY_HISTORY".to_string(),
                severity: "info".to_string(),
                register: "place".to_string(),
                entity_id: id.clone(),
                message: format!(
                    "Establishment '{}' has no Historical Chronicle entries yet. Consider recording its founding/milestones via add_entity_history.",
                    pp.baseline.name
                ),
                suggested_tool: Some("add_entity_history".to_string()),
                suggested_args: Some(json!({
                    "entity_id": id,
                    "entity_kind": "establishment",
                    "era_or_date": "<era>",
                    "title": "<Founding or milestone>",
                    "details": "<historical details>"
                })),
            });
        }

        if let Some(ref parent) = pp.baseline.parent_place {
            if !parent.is_empty() && !place_ids.contains(parent) {
                findings.push(DiagnosticFinding {
                    code: "BROKEN_PLACE_PARENT".to_string(),
                    severity: "warning".to_string(),
                    register: "place".to_string(),
                    entity_id: id.clone(),
                    message: format!(
                        "Place '{}' references unknown parent_place '{}'.",
                        pp.baseline.name, parent
                    ),
                    suggested_tool: Some("update_place_baseline".to_string()),
                    suggested_args: Some(json!({
                        "place_id": id,
                        "parent_place": "<valid-parent-place-slug>"
                    })),
                });
            }
        }
        for route in &pp.current.routes {
            if !place_ids.contains(&route.destination_id) {
                findings.push(DiagnosticFinding {
                    code: "BROKEN_ROUTE_DESTINATION".to_string(),
                    severity: "warning".to_string(),
                    register: "place".to_string(),
                    entity_id: id.clone(),
                    message: format!(
                        "Place '{}' has a route pointing to unknown destination '{}'.",
                        pp.baseline.name, route.destination_id
                    ),
                    suggested_tool: Some("create_place".to_string()),
                    suggested_args: Some(json!({ "id": route.destination_id, "name": route.destination_id })),
                });
            } else if pp.baseline.parent_place.as_deref() != Some(route.destination_id.as_str()) {
                let has_return = place_routes_map
                    .get(&route.destination_id)
                    .map(|set| set.contains(id))
                    .unwrap_or(false);
                if !has_return {
                    findings.push(DiagnosticFinding {
                        code: "ONE_WAY_ROUTE".to_string(),
                        severity: "info".to_string(),
                        register: "place".to_string(),
                        entity_id: id.clone(),
                        message: format!(
                            "Place '{}' has a route to '{}', but '{}' has no reciprocal route back to '{}'.",
                            pp.baseline.name, route.destination_id, route.destination_id, id
                        ),
                        suggested_tool: Some("update_place".to_string()),
                        suggested_args: Some(json!({
                            "place_id": route.destination_id,
                            "action": "route",
                            "destination_id": id,
                            "travel_time": route.travel_time,
                            "terrain_difficulty": route.terrain_difficulty,
                            "route_status": route.status
                        })),
                    });
                }
            }
        }
        if pp.baseline.terrain.trim().is_empty() {
            findings.push(DiagnosticFinding {
                code: "INCOMPLETE_PLACE".to_string(),
                severity: "info".to_string(),
                register: "place".to_string(),
                entity_id: id.clone(),
                message: format!("Place '{}' has an empty terrain description.", pp.baseline.name),
                suggested_tool: Some("update_place_baseline".to_string()),
                suggested_args: Some(json!({ "place_id": id, "terrain": "<terrain description>" })),
            });
        }
    }
    push_duplicate_name_findings(&mut findings, "place", &place_by_name);

    // 4. Lore checks — INCLUDING Mislabeled Register Detection!
    let mut lore_by_title: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for l in &lore_entries {
        lore_by_title
            .entry(l.title.trim().to_lowercase())
            .or_default()
            .push(l.id.clone());

        if !is_canonical_visibility(&l.visibility) {
            findings.push(DiagnosticFinding {
                code: "NON_CANONICAL_ENUM".to_string(),
                severity: "warning".to_string(),
                register: "lore".to_string(),
                entity_id: l.id.clone(),
                message: format!(
                    "Lore '{}' has non-canonical visibility '{}' (expected public, rumor, or gm_secret).",
                    l.title, l.visibility
                ),
                suggested_tool: Some("cleanup_campaign".to_string()),
                suggested_args: Some(json!({ "dry_run": false })),
            });
        }

        let cat_l = l.category.trim().to_lowercase();
        let title_l = l.title.trim().to_lowercase();
        let summary_l = l.summary.trim().to_lowercase();

        // Heuristic A: Lore entry that looks like a physical readable Book / Scroll / Manual / Record
        let text_cat = matches!(
            cat_l.as_str(),
            "book" | "books" | "scroll" | "scrolls" | "grimoire" | "grimoires"
                | "manual" | "manuals" | "text" | "texts" | "tome" | "tomes"
                | "record" | "records" | "journal" | "journals" | "letter" | "letters"
                | "manuscript" | "manuscripts" | "codex"
        );
        let text_title = [
            "scroll of", "book of", "tome of", "codex of", "grimoire", "ritual manual",
            "manual of", "journal of", "diary of", "ledger of", "chronicle of", "letter from",
            "letter to", "tablet of",
        ]
        .iter()
        .any(|k| title_l.contains(k));

        if text_cat || text_title {
            findings.push(DiagnosticFinding {
                code: "MISLABELED_LORE_AS_TEXT".to_string(),
                severity: "warning".to_string(),
                register: "lore".to_string(),
                entity_id: l.id.clone(),
                message: format!(
                    "Lore entry '{}' (category '{}') appears to be an in-world readable book/scroll/manual. Move it to the Books & Scrolls register so reading status, language, and carryable inventory sync work properly.",
                    l.title, l.category
                ),
                suggested_tool: Some("reclassify_entity".to_string()),
                suggested_args: Some(json!({
                    "from_register": "lore",
                    "entity_id": l.id,
                    "to_register": "text",
                    "reason": "Reclassified readable in-world document from lore to texts register"
                })),
            });
        } else if matches!(
            cat_l.as_str(),
            "item" | "items" | "artifact" | "artifacts" | "weapon" | "weapons"
                | "armor" | "tool" | "tools" | "relic" | "relics" | "keepsake" | "keepsakes"
                | "gear" | "equipment" | "heirloom" | "heirlooms"
        ) {
            findings.push(DiagnosticFinding {
                code: "MISLABELED_LORE_AS_ITEM".to_string(),
                severity: "warning".to_string(),
                register: "lore".to_string(),
                entity_id: l.id.clone(),
                message: format!(
                    "Lore entry '{}' (category '{}') looks like a physical carryable item/artifact/weapon/keepsake. Move it to the Carryable Items register to track origin, previous owners, holder, and inventory sync.",
                    l.title, l.category
                ),
                suggested_tool: Some("reclassify_entity".to_string()),
                suggested_args: Some(json!({
                    "from_register": "lore",
                    "entity_id": l.id,
                    "to_register": "item",
                    "reason": "Reclassified physical item/artifact from lore to carryable items register"
                })),
            });
        } else if matches!(
            cat_l.as_str(),
            "place" | "places" | "location" | "locations" | "city" | "cities"
                | "town" | "towns" | "settlement" | "settlements" | "region" | "regions"
                | "tavern" | "taverns" | "establishment" | "establishments"
                | "dungeon" | "dungeons" | "temple" | "guildhall" | "academy"
        ) {
            findings.push(DiagnosticFinding {
                code: "MISLABELED_LORE_AS_PLACE".to_string(),
                severity: "warning".to_string(),
                register: "lore".to_string(),
                entity_id: l.id.clone(),
                message: format!(
                    "Lore entry '{}' (category '{}') looks like a Place or Establishment. Move it to the Places register to track terrain, seasons, weather, occupants, and routes.",
                    l.title, l.category
                ),
                suggested_tool: Some("reclassify_entity".to_string()),
                suggested_args: Some(json!({
                    "from_register": "lore",
                    "entity_id": l.id,
                    "to_register": "place",
                    "reason": "Reclassified place/establishment from lore to places register"
                })),
            });
        } else if matches!(
            cat_l.as_str(),
            "character" | "characters" | "npc" | "npcs" | "pc" | "pcs"
                | "person" | "people" | "companion" | "antagonist"
        ) {
            findings.push(DiagnosticFinding {
                code: "MISLABELED_LORE_AS_CHARACTER".to_string(),
                severity: "warning".to_string(),
                register: "lore".to_string(),
                entity_id: l.id.clone(),
                message: format!(
                    "Lore entry '{}' (category '{}') looks like a Character/NPC. Move it to the Characters register so location, stats, equipment, and event logs are tracked.",
                    l.title, l.category
                ),
                suggested_tool: Some("reclassify_entity".to_string()),
                suggested_args: Some(json!({
                    "from_register": "lore",
                    "entity_id": l.id,
                    "to_register": "character",
                    "reason": "Reclassified character/NPC from lore to characters register"
                })),
            });
        } else if cat_l == "history" || summary_l.contains("historical event") {
            if let Some(target_rel) = l.related_ids.iter().find(|r| all_known_ids.contains(*r)) {
                let target_kind = detect_actual_kind(target_rel).unwrap_or("place");
                findings.push(DiagnosticFinding {
                    code: "MISLABELED_LORE_AS_HISTORY".to_string(),
                    severity: "info".to_string(),
                    register: "lore".to_string(),
                    entity_id: l.id.clone(),
                    message: format!(
                        "History lore entry '{}' links directly to '{}' ({}). Consider moving it into Entity Histories so it compiles directly under '{}''s Historical Chronicle.",
                        l.title, target_rel, target_kind, target_rel
                    ),
                    suggested_tool: Some("reclassify_entity".to_string()),
                    suggested_args: Some(json!({
                        "from_register": "lore",
                        "entity_id": l.id,
                        "to_register": "history",
                        "overrides": {
                            "target_entity_id": target_rel,
                            "target_entity_kind": target_kind
                        },
                        "reason": "Moved entity-specific history from lore into entity_histories"
                    })),
                });
            }
        }

        for r in &l.related_ids {
            if !all_known_ids.contains(r) {
                findings.push(DiagnosticFinding {
                    code: "BROKEN_RELATED_ID".to_string(),
                    severity: "info".to_string(),
                    register: "lore".to_string(),
                    entity_id: l.id.clone(),
                    message: format!("Lore '{}' references unknown related_id '{}'.", l.title, r),
                    suggested_tool: None,
                    suggested_args: None,
                });
            }
        }
    }
    push_duplicate_name_findings(&mut findings, "lore", &lore_by_title);

    // 5. StoryText (Books & Scrolls) checks
    let item_by_id: BTreeMap<String, &CatalogItem> =
        items.iter().map(|i| (i.id.clone(), i)).collect();
    let mut text_by_title: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in &texts {
        text_by_title
            .entry(t.title.trim().to_lowercase())
            .or_default()
            .push(t.id.clone());

        if !is_canonical_read_status(&t.read_status) {
            findings.push(DiagnosticFinding {
                code: "NON_CANONICAL_ENUM".to_string(),
                severity: "warning".to_string(),
                register: "text".to_string(),
                entity_id: t.id.clone(),
                message: format!(
                    "StoryText '{}' has non-canonical read_status '{}'.",
                    t.title, t.read_status
                ),
                suggested_tool: Some("cleanup_campaign".to_string()),
                suggested_args: Some(json!({ "dry_run": false })),
            });
        }
        if let Some(ref h) = t.holder_or_location_id {
            if !h.is_empty() && !char_ids.contains(h) && !place_ids.contains(h) {
                findings.push(DiagnosticFinding {
                    code: "BROKEN_TEXT_HOLDER".to_string(),
                    severity: "warning".to_string(),
                    register: "text".to_string(),
                    entity_id: t.id.clone(),
                    message: format!(
                        "Book/Scroll '{}' has holder_or_location_id '{}' which is neither a known character nor place.",
                        t.title, h
                    ),
                    suggested_tool: Some("record_text_reading".to_string()),
                    suggested_args: Some(json!({
                        "text_id": t.id,
                        "holder_or_location_id": "<valid-char-or-place-slug>"
                    })),
                });
            }
        }
        for reader in &t.read_by {
            if !char_ids.contains(reader) {
                findings.push(DiagnosticFinding {
                    code: "BROKEN_TEXT_READER".to_string(),
                    severity: "warning".to_string(),
                    register: "text".to_string(),
                    entity_id: t.id.clone(),
                    message: format!(
                        "Book/Scroll '{}' lists unknown character '{}' in read_by.",
                        t.title, reader
                    ),
                    suggested_tool: None,
                    suggested_args: None,
                });
            }
        }
        if is_carryable_text_medium(&t.medium) {
            match item_by_id.get(&t.id) {
                None => {
                    findings.push(DiagnosticFinding {
                        code: "UNSYNCED_CARRYABLE_TEXT".to_string(),
                        severity: "warning".to_string(),
                        register: "text".to_string(),
                        entity_id: t.id.clone(),
                        message: format!(
                            "Carryable {} '{}' is missing its synced CatalogItem entry.",
                            t.medium, t.title
                        ),
                        suggested_tool: Some("cleanup_campaign".to_string()),
                        suggested_args: Some(json!({ "dry_run": false })),
                    });
                }
                Some(ci) if ci.current_holder_id != t.holder_or_location_id => {
                    findings.push(DiagnosticFinding {
                        code: "UNSYNCED_CARRYABLE_TEXT".to_string(),
                        severity: "warning".to_string(),
                        register: "text".to_string(),
                        entity_id: t.id.clone(),
                        message: format!(
                            "Holder mismatch for '{}': StoryText says {:?} while CatalogItem says {:?}.",
                            t.title, t.holder_or_location_id, ci.current_holder_id
                        ),
                        suggested_tool: Some("cleanup_campaign".to_string()),
                        suggested_args: Some(json!({ "dry_run": false })),
                    });
                }
                _ => {}
            }
        }
    }
    push_duplicate_name_findings(&mut findings, "text", &text_by_title);

    // 6. Carryable CatalogItems checks
    let char_by_id: BTreeMap<String, &_> =
        chars.iter().map(|c| (c.baseline.id.clone(), c)).collect();
    let mut item_by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for it in &items {
        item_by_name
            .entry(it.name.trim().to_lowercase())
            .or_default()
            .push(it.id.clone());

        if let Some(ref h) = it.current_holder_id {
            if !h.is_empty() && !char_ids.contains(h) && !place_ids.contains(h) {
                findings.push(DiagnosticFinding {
                    code: "BROKEN_ITEM_HOLDER".to_string(),
                    severity: "warning".to_string(),
                    register: "item".to_string(),
                    entity_id: it.id.clone(),
                    message: format!(
                        "Carryable item '{}' has current_holder_id '{}' which is neither a known character nor place.",
                        it.name, h
                    ),
                    suggested_tool: Some("transfer_catalog_item".to_string()),
                    suggested_args: Some(json!({
                        "item_id": it.id,
                        "to_holder_id": "<valid-char-or-place-slug>"
                    })),
                });
            } else if let Some(cp) = char_by_id.get(h) {
                let in_inv = cp
                    .current
                    .equipment
                    .iter()
                    .any(|e| e.name.eq_ignore_ascii_case(&it.name));
                if !in_inv {
                    findings.push(DiagnosticFinding {
                        code: "INVENTORY_CATALOG_DESYNC".to_string(),
                        severity: "warning".to_string(),
                        register: "item".to_string(),
                        entity_id: it.id.clone(),
                        message: format!(
                            "Catalog item '{}' is held by character '{}' but missing from their live carried equipment list.",
                            it.name, h
                        ),
                        suggested_tool: Some("cleanup_campaign".to_string()),
                        suggested_args: Some(json!({ "dry_run": false })),
                    });
                }
            }
        }
        if let Some(ref tid) = it.linked_text_id {
            if !tid.is_empty() && !text_ids.contains(tid) {
                findings.push(DiagnosticFinding {
                    code: "BROKEN_ITEM_LINKED_TEXT".to_string(),
                    severity: "warning".to_string(),
                    register: "item".to_string(),
                    entity_id: it.id.clone(),
                    message: format!(
                        "Carryable item '{}' links to missing StoryText '{}'.",
                        it.name, tid
                    ),
                    suggested_tool: Some("upsert_item".to_string()),
                    suggested_args: Some(json!({ "id": it.id, "name": it.name, "linked_text_id": "" })),
                });
            }
        }
        if it.origin_and_maker.trim().is_empty() && it.significance.trim().is_empty() {
            findings.push(DiagnosticFinding {
                code: "INCOMPLETE_ITEM".to_string(),
                severity: "info".to_string(),
                register: "item".to_string(),
                entity_id: it.id.clone(),
                message: format!(
                    "Carryable item '{}' has neither origin_and_maker nor significance recorded.",
                    it.name
                ),
                suggested_tool: Some("upsert_item".to_string()),
                suggested_args: Some(json!({
                    "id": it.id,
                    "name": it.name,
                    "origin_and_maker": "<maker/origin>",
                    "significance": "<story significance>"
                })),
            });
        }
    }
    push_duplicate_name_findings(&mut findings, "item", &item_by_name);

    // 7. Entity Histories checks (kind mismatch, dangling entity_id, invalid visibility)
    for h in &histories {
        if !is_canonical_entity_kind(&h.entity_kind) {
            findings.push(DiagnosticFinding {
                code: "NON_CANONICAL_ENUM".to_string(),
                severity: "warning".to_string(),
                register: "history".to_string(),
                entity_id: h.id.to_string(),
                message: format!(
                    "History #{} ('{}') has non-canonical entity_kind '{}'.",
                    h.id, h.title, h.entity_kind
                ),
                suggested_tool: Some("update_entity_history".to_string()),
                suggested_args: Some(json!({ "history_id": h.id, "entity_kind": "place" })),
            });
        }
        if !is_canonical_visibility(&h.visibility) {
            findings.push(DiagnosticFinding {
                code: "NON_CANONICAL_ENUM".to_string(),
                severity: "warning".to_string(),
                register: "history".to_string(),
                entity_id: h.id.to_string(),
                message: format!(
                    "History #{} ('{}') has non-canonical visibility '{}'.",
                    h.id, h.title, h.visibility
                ),
                suggested_tool: Some("cleanup_campaign".to_string()),
                suggested_args: Some(json!({ "dry_run": false })),
            });
        }
        if let Some(actual) = detect_actual_kind(&h.entity_id) {
            let ek = h.entity_kind.to_lowercase();
            let compatible = match actual {
                "place" => matches!(ek.as_str(), "place" | "establishment" | "sect" | "faction"),
                "lore" => matches!(ek.as_str(), "lore" | "sect" | "faction" | "establishment"),
                "text" => matches!(ek.as_str(), "text" | "item"),
                "item" => matches!(ek.as_str(), "item" | "text"),
                other => ek == other,
            };
            if !compatible {
                findings.push(DiagnosticFinding {
                    code: "HISTORY_KIND_MISMATCH".to_string(),
                    severity: "warning".to_string(),
                    register: "history".to_string(),
                    entity_id: h.id.to_string(),
                    message: format!(
                        "History #{} ('{}') is labeled entity_kind='{}', but '{}' exists in the '{}' register.",
                        h.id, h.title, h.entity_kind, h.entity_id, actual
                    ),
                    suggested_tool: Some("update_entity_history".to_string()),
                    suggested_args: Some(json!({
                        "history_id": h.id,
                        "entity_kind": actual
                    })),
                });
            }
        } else if !matches!(h.entity_kind.to_lowercase().as_str(), "sect" | "faction" | "establishment") {
            // Sects/establishments may exist purely as historical concepts before a full lore/place page is written,
            // so only warn for character/place/item/text/lore pointing to unknown IDs.
            findings.push(DiagnosticFinding {
                code: "BROKEN_HISTORY_ENTITY".to_string(),
                severity: "warning".to_string(),
                register: "history".to_string(),
                entity_id: h.id.to_string(),
                message: format!(
                    "History #{} ('{}') attaches to unknown {} '{}'.",
                    h.id, h.title, h.entity_kind, h.entity_id
                ),
                suggested_tool: Some("update_entity_history".to_string()),
                suggested_args: Some(json!({
                    "history_id": h.id,
                    "entity_id": "<valid-entity-slug>"
                })),
            });
        }
    }

    // 8. Attached Reference Docs checks
    for a in &attachments {
        let abs = engine.attachment_file_path(a);
        if !abs.exists() {
            findings.push(DiagnosticFinding {
                code: "ATTACHMENT_FILE_MISSING".to_string(),
                severity: "error".to_string(),
                register: "attachment".to_string(),
                entity_id: a.id.clone(),
                message: format!(
                    "Attached reference doc '{}' points to missing file '{}'.",
                    a.title, a.relative_path
                ),
                suggested_tool: Some("delete_entity".to_string()),
                suggested_args: Some(json!({
                    "register": "attachment",
                    "entity_id": a.id,
                    "reason": "Missing backing file on disk"
                })),
            });
        }
        if a.summary.trim().is_empty() && a.extracted_notes.trim().is_empty() {
            findings.push(DiagnosticFinding {
                code: "INCOMPLETE_ATTACHMENT".to_string(),
                severity: "info".to_string(),
                register: "attachment".to_string(),
                entity_id: a.id.clone(),
                message: format!(
                    "Attachment '{}' has empty summary and extracted_notes (limits FTS5 recall).",
                    a.title
                ),
                suggested_tool: None,
                suggested_args: None,
            });
        }
    }

    // 9. Knowledge Graph Relation checks
    for r in &relations {
        if !all_known_ids.contains(&r.source_id) {
            findings.push(DiagnosticFinding {
                code: "BROKEN_RELATION_ENDPOINT".to_string(),
                severity: "warning".to_string(),
                register: "relation".to_string(),
                entity_id: format!("{}--[{}]-->{}", r.source_id, r.relation_type, r.target_id),
                message: format!(
                    "Relation {} --[{}]--> {} references unknown source entity '{}'.",
                    r.source_id, r.relation_type, r.target_id, r.source_id
                ),
                suggested_tool: Some("remove_relation".to_string()),
                suggested_args: Some(json!({
                    "source_id": r.source_id,
                    "relation_type": r.relation_type,
                    "target_id": r.target_id,
                    "reason": "Removed dangling relation"
                })),
            });
        }
        if !all_known_ids.contains(&r.target_id) {
            findings.push(DiagnosticFinding {
                code: "BROKEN_RELATION_ENDPOINT".to_string(),
                severity: "warning".to_string(),
                register: "relation".to_string(),
                entity_id: format!("{}--[{}]-->{}", r.source_id, r.relation_type, r.target_id),
                message: format!(
                    "Relation {} --[{}]--> {} references unknown target entity '{}'.",
                    r.source_id, r.relation_type, r.target_id, r.target_id
                ),
                suggested_tool: Some("remove_relation".to_string()),
                suggested_args: Some(json!({
                    "source_id": r.source_id,
                    "relation_type": r.relation_type,
                    "target_id": r.target_id,
                    "reason": "Removed dangling relation"
                })),
            });
        }
    }

    // 10. Cross-register duplicate slug detection (excluding intentional text<->item linked pairs)
    let mut slug_registers: BTreeMap<String, Vec<&'static str>> = BTreeMap::new();
    for id in &char_ids {
        slug_registers.entry(id.clone()).or_default().push("character");
    }
    for id in &place_ids {
        slug_registers.entry(id.clone()).or_default().push("place");
    }
    for id in &lore_ids {
        slug_registers.entry(id.clone()).or_default().push("lore");
    }
    for id in &text_ids {
        slug_registers.entry(id.clone()).or_default().push("text");
    }
    for id in &item_ids {
        slug_registers.entry(id.clone()).or_default().push("item");
    }
    for (slug, regs) in slug_registers {
        if regs.len() > 1 && !(regs.len() == 2 && regs.contains(&"text") && regs.contains(&"item")) {
            findings.push(DiagnosticFinding {
                code: "DUPLICATE_SLUG_CROSS_REGISTER".to_string(),
                severity: "info".to_string(),
                register: regs.join("+"),
                entity_id: slug.clone(),
                message: format!(
                    "Slug '{}' exists in multiple registers ({}). Verify whether one is mislabeled or redundant.",
                    slug,
                    regs.join(", ")
                ),
                suggested_tool: None,
                suggested_args: None,
            });
        }
    }

    let error_count = findings.iter().filter(|f| f.severity == "error").count();
    let warning_count = findings.iter().filter(|f| f.severity == "warning").count();
    let info_count = findings.iter().filter(|f| f.severity == "info").count();

    Ok(DiagnosticReport {
        healthy: error_count == 0 && warning_count == 0,
        error_count,
        warning_count,
        info_count,
        ledger_hash_chain_valid: chain_valid,
        counts,
        findings,
    })
}

fn push_duplicate_name_findings(
    findings: &mut Vec<DiagnosticFinding>,
    register: &str,
    by_name: &BTreeMap<String, Vec<String>>,
) {
    for (name, ids) in by_name {
        if !name.is_empty() && ids.len() > 1 {
            findings.push(DiagnosticFinding {
                code: "DUPLICATE_NAME_SAME_REGISTER".to_string(),
                severity: "warning".to_string(),
                register: register.to_string(),
                entity_id: ids.join(", "),
                message: format!(
                    "Duplicate {} name '{}' found under multiple IDs: {}. Consider merging into one canonical slug.",
                    register,
                    name,
                    ids.join(", ")
                ),
                suggested_tool: Some("rename_or_merge_entity".to_string()),
                suggested_args: Some(json!({
                    "register": register,
                    "old_id": ids[1],
                    "new_id": ids[0],
                    "allow_merge": true,
                    "reason": "Merged duplicate entity"
                })),
            });
        }
    }
}

// =============================================================================
// Direct Profile / Baseline / History Editing
// =============================================================================

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateCharacterBaselineRequest {
    pub character_id: String,
    pub name: Option<String>,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub origin: Option<String>,
    pub archetype: Option<String>,
    pub faction: Option<String>,
    pub appearance: Option<String>,
    pub personality: Option<String>,
    pub backstory: Option<String>,
    pub initial_stats: Option<BTreeMap<String, i64>>,
    pub tags: Option<Vec<String>>,
    pub notes: Option<String>,
    #[serde(default)]
    pub reason: String,
}

pub fn update_character_baseline(
    engine: &CampaignEngine,
    req: UpdateCharacterBaselineRequest,
) -> Result<(CharacterBaseline, WorldEvent)> {
    let proj = engine.get_character(&req.character_id, None)?;
    let mut b = proj.baseline;
    let mut changed = Vec::new();

    if let Some(v) = req.name {
        if !v.trim().is_empty() {
            b.name = v;
            changed.push("name".to_string());
        }
    }
    if let Some(v) = req.title {
        b.title = v;
        changed.push("title".to_string());
    }
    if let Some(v) = req.kind {
        b.kind = v;
        changed.push("kind".to_string());
    }
    if let Some(v) = req.origin {
        b.origin = v;
        changed.push("origin".to_string());
    }
    if let Some(v) = req.archetype {
        b.archetype = v;
        changed.push("archetype".to_string());
    }
    if let Some(v) = req.faction {
        b.faction = v;
        changed.push("faction".to_string());
    }
    if let Some(v) = req.appearance {
        b.appearance = v;
        changed.push("appearance".to_string());
    }
    if let Some(v) = req.personality {
        b.personality = v;
        changed.push("personality".to_string());
    }
    if let Some(v) = req.backstory {
        b.backstory = v;
        changed.push("backstory".to_string());
    }
    if let Some(v) = req.initial_stats {
        b.initial_stats = v;
        changed.push("initial_stats".to_string());
    }
    if let Some(v) = req.tags {
        b.tags = dedupe_trimmed(v);
        changed.push("tags".to_string());
    }
    if let Some(v) = req.notes {
        b.notes = v;
        changed.push("notes".to_string());
    }
    if changed.is_empty() {
        bail!("No fields provided to update on character '{}'", b.id);
    }

    engine.db().update_character_baseline(&b)?;
    let summary = format!(
        "Edited character '{}' baseline ({}){}",
        b.name,
        changed.join(", "),
        if req.reason.is_empty() {
            String::new()
        } else {
            format!(" — {}", req.reason)
        }
    );
    let ev = engine.db().append_event(
        proj.current.current_location,
        vec![b.id.clone()],
        summary,
        EventPayload::EntityEdited {
            register: "character".to_string(),
            entity_id: b.id.clone(),
            fields_changed: changed,
            reason: req.reason,
        },
    )?;
    engine.maybe_compile()?;
    Ok((b, ev))
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdatePlaceBaselineRequest {
    pub place_id: String,
    pub name: Option<String>,
    pub place_type: Option<String>,
    pub parent_place: Option<String>,
    pub terrain: Option<String>,
    pub climate: Option<String>,
    pub seasonal_profiles: Option<BTreeMap<String, String>>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub notes: Option<String>,
    #[serde(default)]
    pub reason: String,
}

pub fn update_place_baseline(
    engine: &CampaignEngine,
    req: UpdatePlaceBaselineRequest,
) -> Result<(PlaceBaseline, WorldEvent)> {
    let proj = engine.get_place(&req.place_id, None)?;
    let mut b = proj.baseline;
    let mut changed = Vec::new();

    if let Some(v) = req.name {
        if !v.trim().is_empty() {
            b.name = v;
            changed.push("name".to_string());
        }
    }
    if let Some(v) = req.place_type {
        b.place_type = v;
        changed.push("place_type".to_string());
    }
    if let Some(v) = req.parent_place {
        b.parent_place = if v.trim().is_empty() {
            None
        } else {
            Some(slugify(&v))
        };
        changed.push("parent_place".to_string());
    }
    if let Some(v) = req.terrain {
        b.terrain = v;
        changed.push("terrain".to_string());
    }
    if let Some(v) = req.climate {
        b.climate = v;
        changed.push("climate".to_string());
    }
    if let Some(v) = req.seasonal_profiles {
        for (k, val) in v {
            b.seasonal_profiles.insert(k, val);
        }
        changed.push("seasonal_profiles".to_string());
    }
    if let Some(v) = req.description {
        b.description = v;
        changed.push("description".to_string());
    }
    if let Some(v) = req.tags {
        b.tags = dedupe_trimmed(v);
        changed.push("tags".to_string());
    }
    if let Some(v) = req.notes {
        b.notes = v;
        changed.push("notes".to_string());
    }
    if changed.is_empty() {
        bail!("No fields provided to update on place '{}'", b.id);
    }

    engine.db().update_place_baseline(&b)?;
    let summary = format!(
        "Edited place '{}' baseline ({}){}",
        b.name,
        changed.join(", "),
        if req.reason.is_empty() {
            String::new()
        } else {
            format!(" — {}", req.reason)
        }
    );
    let ev = engine.db().append_event(
        Some(b.id.clone()),
        vec![b.id.clone()],
        summary,
        EventPayload::EntityEdited {
            register: "place".to_string(),
            entity_id: b.id.clone(),
            fields_changed: changed,
            reason: req.reason,
        },
    )?;
    engine.maybe_compile()?;
    Ok((b, ev))
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateEntityHistoryRequest {
    pub history_id: i64,
    pub entity_id: Option<String>,
    pub entity_kind: Option<String>,
    pub era_or_date: Option<String>,
    pub sort_order: Option<i64>,
    pub title: Option<String>,
    pub details: Option<String>,
    pub significance: Option<String>,
    pub visibility: Option<String>,
    pub related_ids: Option<Vec<String>>,
    #[serde(default)]
    pub reason: String,
}

pub fn update_entity_history(
    engine: &CampaignEngine,
    req: UpdateEntityHistoryRequest,
) -> Result<(EntityHistoryEntry, WorldEvent)> {
    let mut entry = engine
        .db()
        .get_entity_history_by_id(req.history_id)?
        .ok_or_else(|| anyhow!("Entity history #{} not found", req.history_id))?;
    let mut changed = Vec::new();

    if let Some(v) = req.entity_id {
        if !v.trim().is_empty() {
            entry.entity_id = slugify(&v);
            changed.push("entity_id".to_string());
        }
    }
    if let Some(v) = req.entity_kind {
        entry.entity_kind = normalize_entity_kind(&v);
        changed.push("entity_kind".to_string());
    }
    if let Some(v) = req.era_or_date {
        entry.era_or_date = v;
        changed.push("era_or_date".to_string());
    }
    if let Some(v) = req.sort_order {
        entry.sort_order = v;
        changed.push("sort_order".to_string());
    }
    if let Some(v) = req.title {
        if !v.trim().is_empty() {
            entry.title = v;
            changed.push("title".to_string());
        }
    }
    if let Some(v) = req.details {
        entry.details = v;
        changed.push("details".to_string());
    }
    if let Some(v) = req.significance {
        entry.significance = v;
        changed.push("significance".to_string());
    }
    if let Some(v) = req.visibility {
        entry.visibility = normalize_visibility(&v);
        changed.push("visibility".to_string());
    }
    if let Some(v) = req.related_ids {
        entry.related_ids = dedupe_slugs(v);
        changed.push("related_ids".to_string());
    }
    if changed.is_empty() {
        bail!("No fields provided to update on history #{}", entry.id);
    }

    engine.db().update_entity_history(&entry)?;
    let mut affected = vec![entry.entity_id.clone()];
    affected.extend(entry.related_ids.clone());
    let summary = format!(
        "Edited history #{} '{}' on {} '{}' ({})",
        entry.id,
        entry.title,
        entry.entity_kind,
        entry.entity_id,
        changed.join(", ")
    );
    let ev = engine.db().append_event(
        None,
        affected,
        summary,
        EventPayload::EntityEdited {
            register: "history".to_string(),
            entity_id: entry.id.to_string(),
            fields_changed: changed,
            reason: req.reason,
        },
    )?;
    engine.maybe_compile()?;
    Ok((entry, ev))
}

// =============================================================================
// Reclassify / Move Mislabeled Data Between Registers
// =============================================================================

#[derive(Debug, Clone, Deserialize)]
pub struct ReclassifyRequest {
    /// Source register: "lore", "text", "item", "attachment", "history"
    pub from_register: String,
    /// Source ID (slug, or integer string for "history")
    pub entity_id: String,
    /// Destination register: "lore", "text", "item", "place", "character", "history"
    pub to_register: String,
    /// Optional new slug in destination register (defaults to source slug)
    #[serde(default)]
    pub new_id: Option<String>,
    /// Keep the original record in the source register instead of moving it (default false = move)
    #[serde(default)]
    pub keep_source: bool,
    /// Optional destination-specific field overrides (e.g. medium, subject, item_type, place_type, target_entity_id, era_or_date)
    #[serde(default)]
    pub overrides: Value,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReclassifyResult {
    pub from_register: String,
    pub from_id: String,
    pub to_register: String,
    pub to_id: String,
    pub removed_source: bool,
    pub pruned_docs: Vec<String>,
    pub event_seq: i64,
    pub summary: String,
}

pub fn reclassify_entity(
    engine: &CampaignEngine,
    req: ReclassifyRequest,
) -> Result<ReclassifyResult> {
    let from_reg = normalize_register(&req.from_register)?;
    let to_reg = normalize_register(&req.to_register)?;
    if from_reg == to_reg {
        bail!("Source and target registers are identical ('{from_reg}').");
    }

    let ov = &req.overrides;
    let ov_str = |k: &str| -> Option<String> {
        ov.get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
    };
    let ov_i64 = |k: &str| -> Option<i64> { ov.get(k).and_then(|v| v.as_i64()) };

    // Extract a normalized intermediate payload from the source register
    struct Extracted {
        id: String,
        title: String,
        category_or_type: String,
        summary: String,
        body: String,
        holder_or_location: Option<String>,
        visibility: String,
        keywords: Vec<String>,
        tags: Vec<String>,
        related_ids: Vec<String>,
        gm_notes: String,
        source_history_id: Option<i64>,
    }

    let extracted = match from_reg {
        "lore" => {
            let slug = slugify(&req.entity_id);
            let l = engine
                .db()
                .get_lore(&slug)?
                .ok_or_else(|| anyhow!("Lore entry '{slug}' not found"))?;
            Extracted {
                id: l.id,
                title: l.title,
                category_or_type: l.category,
                summary: l.summary,
                body: l.body,
                holder_or_location: None,
                visibility: l.visibility,
                keywords: l.keywords,
                tags: l.tags,
                related_ids: l.related_ids,
                gm_notes: String::new(),
                source_history_id: None,
            }
        }
        "text" => {
            let t = engine.get_text(&req.entity_id)?;
            Extracted {
                id: t.id,
                title: t.title,
                category_or_type: t.medium,
                summary: t.summary,
                body: t.content,
                holder_or_location: t.holder_or_location_id,
                visibility: "public".to_string(),
                keywords: t.keywords,
                tags: t.tags,
                related_ids: t.related_ids,
                gm_notes: t.gm_notes,
                source_history_id: None,
            }
        }
        "item" => {
            let it = engine.get_catalog_item(&req.entity_id)?;
            let mut body_parts = Vec::new();
            if !it.description.is_empty() {
                body_parts.push(it.description.clone());
            }
            if !it.origin_and_maker.is_empty() {
                body_parts.push(format!("Origin & Maker: {}", it.origin_and_maker));
            }
            if !it.significance.is_empty() {
                body_parts.push(format!("Significance: {}", it.significance));
            }
            if !it.properties_and_effects.is_empty() {
                body_parts.push(format!("Properties: {}", it.properties_and_effects));
            }
            Extracted {
                id: it.id,
                title: it.name,
                category_or_type: it.item_type,
                summary: if it.significance.is_empty() {
                    it.description
                } else {
                    it.significance
                },
                body: body_parts.join("\n\n"),
                holder_or_location: it.current_holder_id,
                visibility: "public".to_string(),
                keywords: it.keywords,
                tags: it.tags,
                related_ids: it.related_ids,
                gm_notes: it.gm_notes,
                source_history_id: None,
            }
        }
        "attachment" => {
            let att = engine.get_attachment(&req.entity_id)?;
            let file_text = std::fs::read_to_string(engine.attachment_file_path(&att))
                .unwrap_or_else(|_| att.extracted_notes.clone());
            Extracted {
                id: att.id,
                title: att.title,
                category_or_type: att.category,
                summary: att.summary,
                body: if file_text.trim().is_empty() {
                    att.extracted_notes
                } else {
                    file_text
                },
                holder_or_location: None,
                visibility: att.visibility,
                keywords: att.keywords,
                tags: att.tags,
                related_ids: att.related_ids,
                gm_notes: String::new(),
                source_history_id: None,
            }
        }
        "history" => {
            let hid: i64 = req
                .entity_id
                .trim()
                .parse()
                .map_err(|_| anyhow!("History entity_id must be an integer ID"))?;
            let h = engine
                .db()
                .get_entity_history_by_id(hid)?
                .ok_or_else(|| anyhow!("Entity history #{hid} not found"))?;
            let mut rels = h.related_ids.clone();
            if !rels.contains(&h.entity_id) {
                rels.push(h.entity_id.clone());
            }
            Extracted {
                id: slugify(&h.title),
                title: h.title,
                category_or_type: "history".to_string(),
                summary: if h.significance.is_empty() {
                    format!("{} ({})", h.era_or_date, h.entity_id)
                } else {
                    h.significance
                },
                body: format!("Era / Date: {}\n\n{}", h.era_or_date, h.details),
                holder_or_location: None,
                visibility: h.visibility,
                keywords: vec![h.entity_id],
                tags: vec!["history".to_string(), h.entity_kind],
                related_ids: rels,
                gm_notes: String::new(),
                source_history_id: Some(hid),
            }
        }
        other => bail!(
            "Cannot reclassify from register '{other}'. Supported source registers: lore, text, item, attachment, history."
        ),
    };

    let target_slug = req
        .new_id
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(slugify)
        .unwrap_or_else(|| extracted.id.clone());

    // Create destination record in the target register
    let created_id: String = match to_reg {
        "lore" => {
            let entry = LoreEntry {
                id: target_slug.clone(),
                title: ov_str("title").unwrap_or(extracted.title),
                category: ov_str("category").unwrap_or_else(|| {
                    if extracted.category_or_type.is_empty() {
                        "world".to_string()
                    } else {
                        extracted.category_or_type
                    }
                }),
                summary: ov_str("summary").unwrap_or(extracted.summary),
                body: ov_str("body").unwrap_or(extracted.body),
                keywords: extracted.keywords,
                tags: extracted.tags,
                visibility: ov_str("visibility").unwrap_or(extracted.visibility),
                always_include: ov
                    .get("always_include")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                related_ids: extracted.related_ids,
                created_at_world_time: String::new(),
                updated_at_world_time: String::new(),
            };
            let (saved, _) = engine.upsert_lore(entry, "Reclassified into lore")?;
            saved.id
        }
        "text" => {
            let medium = ov_str("medium").unwrap_or_else(|| {
                let low = extracted.title.to_lowercase();
                if low.contains("scroll") {
                    "scroll".to_string()
                } else if low.contains("grimoire") {
                    "grimoire".to_string()
                } else if low.contains("manual") {
                    "ritual-manual".to_string()
                } else if low.contains("journal") || low.contains("diary") {
                    "journal".to_string()
                } else if low.contains("letter") {
                    "letter".to_string()
                } else if low.contains("tablet") {
                    "tablet".to_string()
                } else {
                    "book".to_string()
                }
            });
            let text = StoryText {
                id: target_slug.clone(),
                title: ov_str("title").unwrap_or(extracted.title),
                medium,
                subject: ov_str("subject").unwrap_or_else(|| "history".to_string()),
                author: ov_str("author").unwrap_or_default(),
                language: ov_str("language").unwrap_or_else(|| "Common".to_string()),
                condition: ov_str("condition").unwrap_or_else(|| "Intact".to_string()),
                read_status: ov_str("read_status")
                    .unwrap_or_else(|| "discovered_unread".to_string()),
                read_by: Vec::new(),
                holder_or_location_id: ov_str("holder_or_location_id")
                    .or(extracted.holder_or_location),
                summary: ov_str("summary").unwrap_or(extracted.summary),
                content: ov_str("content").unwrap_or(extracted.body),
                mechanics_or_unlocks: Vec::new(),
                gm_notes: ov_str("gm_notes").unwrap_or(extracted.gm_notes),
                keywords: extracted.keywords,
                tags: extracted.tags,
                related_ids: extracted.related_ids,
                created_at_world_time: String::new(),
                updated_at_world_time: String::new(),
            };
            let (saved, _) = engine.upsert_text(text, "Reclassified into books & scrolls")?;
            saved.id
        }
        "item" => {
            let item_type = ov_str("item_type").unwrap_or_else(|| {
                let c = extracted.category_or_type.to_lowercase();
                if c.contains("weapon") {
                    "weapon".to_string()
                } else if c.contains("tool") {
                    "tool".to_string()
                } else if c.contains("keepsake") || c.contains("heirloom") {
                    "keepsake".to_string()
                } else if c.contains("book") || c.contains("scroll") || c.contains("manual") {
                    c
                } else {
                    "artifact".to_string()
                }
            });
            let item = CatalogItem {
                id: target_slug.clone(),
                name: ov_str("name").unwrap_or(extracted.title),
                item_type,
                rarity_or_tier: ov_str("rarity_or_tier").unwrap_or_default(),
                slot: ov_str("slot"),
                quantity: 1,
                equipped: false,
                condition_or_durability: ov_str("condition_or_durability")
                    .unwrap_or_else(|| "Intact".to_string()),
                origin_and_maker: ov_str("origin_and_maker").unwrap_or_default(),
                previous_owners: Vec::new(),
                current_holder_id: ov_str("current_holder_id").or(extracted.holder_or_location),
                significance: ov_str("significance").unwrap_or(extracted.summary),
                properties_and_effects: ov_str("properties_and_effects").unwrap_or_default(),
                description: ov_str("description").unwrap_or(extracted.body),
                linked_text_id: ov_str("linked_text_id"),
                gm_notes: ov_str("gm_notes").unwrap_or(extracted.gm_notes),
                keywords: extracted.keywords,
                tags: extracted.tags,
                related_ids: extracted.related_ids,
                created_at_world_time: String::new(),
                updated_at_world_time: String::new(),
                ..Default::default()
            };
            let (saved, _) = engine.upsert_catalog_item(item, "Reclassified into carryable items")?;
            saved.id
        }
        "place" => {
            let place = PlaceBaseline {
                id: target_slug.clone(),
                name: ov_str("name").unwrap_or(extracted.title),
                place_type: ov_str("place_type").unwrap_or_else(|| {
                    let c = extracted.category_or_type.to_lowercase();
                    if c.contains("tavern") || c.contains("establishment") || c.contains("guild") {
                        "establishment".to_string()
                    } else {
                        "location".to_string()
                    }
                }),
                parent_place: ov_str("parent_place").map(|s| slugify(&s)),
                terrain: ov_str("terrain").unwrap_or_else(|| "Unspecified".to_string()),
                climate: ov_str("climate").unwrap_or_default(),
                seasonal_profiles: BTreeMap::new(),
                description: ov_str("description").unwrap_or(extracted.body),
                initial_weather: ov_str("initial_weather").unwrap_or_else(|| "Clear".to_string()),
                initial_faction: ov_str("initial_faction"),
                initial_danger_level: ov_str("initial_danger_level")
                    .unwrap_or_else(|| "Moderate".to_string()),
                places_of_significance: Vec::new(),
                routes: Vec::new(),
                tags: extracted.tags,
                notes: extracted.summary,
                created_at_world_time: String::new(),
            };
            let saved = engine.create_place(place)?;
            saved.baseline.id
        }
        "character" => {
            let ch = CharacterBaseline {
                id: target_slug.clone(),
                name: ov_str("name").unwrap_or(extracted.title),
                title: ov_str("title").unwrap_or_default(),
                kind: ov_str("kind").unwrap_or_else(|| "npc".to_string()),
                origin: ov_str("origin").unwrap_or_default(),
                archetype: ov_str("archetype").unwrap_or_default(),
                faction: ov_str("faction").unwrap_or_default(),
                appearance: ov_str("appearance").unwrap_or(extracted.summary),
                personality: ov_str("personality").unwrap_or_default(),
                backstory: ov_str("backstory").unwrap_or(extracted.body),
                initial_location: ov_str("initial_location").map(|s| slugify(&s)),
                initial_stats: BTreeMap::new(),
                initial_resources: BTreeMap::new(),
                initial_equipment: Vec::new(),
                initial_skills: Vec::new(),
                initial_techniques: Vec::new(),
                initial_goals: Vec::new(),
                tags: extracted.tags,
                notes: String::new(),
                created_at_world_time: String::new(),
            };
            let saved = engine.create_character(ch)?;
            saved.baseline.id
        }
        "history" => {
            let target_entity = ov_str("target_entity_id")
                .or_else(|| extracted.related_ids.first().cloned())
                .ok_or_else(|| {
                    anyhow!(
                        "Reclassifying into 'history' requires overrides.target_entity_id (or a related_id on the source record)."
                    )
                })?;
            let target_kind = ov_str("target_entity_kind").unwrap_or_else(|| "place".to_string());
            let entry = EntityHistoryEntry {
                id: 0,
                entity_id: slugify(&target_entity),
                entity_kind: normalize_entity_kind(&target_kind),
                era_or_date: ov_str("era_or_date").unwrap_or_else(|| "Historical".to_string()),
                sort_order: ov_i64("sort_order").unwrap_or(0),
                title: ov_str("title").unwrap_or(extracted.title),
                details: ov_str("details").unwrap_or(extracted.body),
                significance: ov_str("significance").unwrap_or(extracted.summary),
                related_ids: extracted.related_ids,
                visibility: ov_str("visibility").unwrap_or(extracted.visibility),
                recorded_at_world_time: String::new(),
            };
            let (saved, _) = engine.add_entity_history(entry)?;
            saved.id.to_string()
        }
        other => bail!(
            "Cannot reclassify into register '{other}'. Supported targets: lore, text, item, place, character, history."
        ),
    };

    // If slug changed and we moved between entity registers (not history), rewire references!
    if to_reg != "history" && extracted.id != created_id {
        let _ = rewrite_all_references(engine, &extracted.id, &created_id, None)?;
    }

    // Remove source record unless keep_source is true
    let mut removed_source = false;
    if !req.keep_source {
        removed_source = match from_reg {
            "lore" => engine.db().delete_lore(&extracted.id)?,
            "text" => {
                // If migrating text -> item, keep the item we just created; otherwise delete text only
                engine.db().delete_text(&extracted.id)?
            }
            "item" => {
                // If migrating item -> text, upsert_text already auto-synced a linked CatalogItem under the same ID!
                if to_reg == "text" && created_id == extracted.id {
                    false
                } else {
                    engine.db().delete_catalog_item(&extracted.id)?
                }
            }
            "attachment" => engine.db().delete_attachment(&extracted.id)?,
            "history" => {
                if let Some(hid) = extracted.source_history_id {
                    engine.db().delete_entity_history(hid)?
                } else {
                    false
                }
            }
            _ => false,
        };
    }

    let summary = format!(
        "Reclassified {}:{} -> {}:{}{}",
        from_reg,
        extracted.id,
        to_reg,
        created_id,
        if req.reason.is_empty() {
            String::new()
        } else {
            format!(" ({})", req.reason)
        }
    );
    let ev = engine.db().append_event(
        None,
        vec![extracted.id.clone(), created_id.clone()],
        summary.clone(),
        EventPayload::EntityReclassified {
            from_register: from_reg.to_string(),
            from_id: extracted.id.clone(),
            to_register: to_reg.to_string(),
            to_id: created_id.clone(),
            reason: req.reason,
        },
    )?;

    engine.maybe_compile()?;
    let pruned_docs = prune_orphaned_docs(engine.db(), &engine.docs_dir())?;

    Ok(ReclassifyResult {
        from_register: from_reg.to_string(),
        from_id: extracted.id,
        to_register: to_reg.to_string(),
        to_id: created_id,
        removed_source,
        pruned_docs,
        event_seq: ev.seq,
        summary,
    })
}

// =============================================================================
// Rename Slug OR Merge Duplicate Entities Across Registers
// =============================================================================

#[derive(Debug, Clone, Deserialize)]
pub struct RenameOrMergeRequest {
    /// Register: "character", "place", "lore", "text", "item", "attachment"
    pub register: String,
    pub old_id: String,
    pub new_id: String,
    /// If true and `new_id` already exists in the same register, merges `old_id` into `new_id` and removes `old_id`.
    #[serde(default)]
    pub allow_merge: bool,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenameOrMergeResult {
    pub register: String,
    pub old_id: String,
    pub new_id: String,
    pub merged: bool,
    pub references_updated: usize,
    pub pruned_docs: Vec<String>,
    pub event_seq: i64,
    pub summary: String,
}

pub fn rename_or_merge_entity(
    engine: &CampaignEngine,
    req: RenameOrMergeRequest,
) -> Result<RenameOrMergeResult> {
    let reg = normalize_register(&req.register)?;
    let old_id = slugify(&req.old_id);
    let new_id = slugify(&req.new_id);
    if old_id == new_id {
        bail!("old_id and new_id resolve to the same slug '{old_id}'.");
    }

    let db = engine.db();
    let merged: bool;

    match reg {
        "character" => {
            let old = db
                .get_character_baseline(&old_id)?
                .ok_or_else(|| anyhow!("Character '{old_id}' not found"))?;
            if let Some(mut target) = db.get_character_baseline(&new_id)? {
                if !req.allow_merge {
                    bail!("Character '{new_id}' already exists (pass allow_merge: true to merge).");
                }
                // Merge useful baseline fields & tags into target
                if target.backstory.is_empty() && !old.backstory.is_empty() {
                    target.backstory = old.backstory;
                }
                if target.appearance.is_empty() && !old.appearance.is_empty() {
                    target.appearance = old.appearance;
                }
                target.tags.extend(old.tags);
                target.tags = dedupe_trimmed(target.tags);
                db.update_character_baseline(&target)?;
                db.delete_character_baseline(&old_id)?;
                merged = true;
            } else {
                let mut renamed = old;
                renamed.id = new_id.clone();
                db.rename_character_baseline_id(&old_id, &new_id, &renamed)?;
                merged = false;
            }
        }
        "place" => {
            let old = db
                .get_place_baseline(&old_id)?
                .ok_or_else(|| anyhow!("Place '{old_id}' not found"))?;
            if let Some(mut target) = db.get_place_baseline(&new_id)? {
                if !req.allow_merge {
                    bail!("Place '{new_id}' already exists (pass allow_merge: true to merge).");
                }
                if target.description.is_empty() && !old.description.is_empty() {
                    target.description = old.description;
                }
                for (s, eff) in old.seasonal_profiles {
                    target.seasonal_profiles.entry(s).or_insert(eff);
                }
                target.tags.extend(old.tags);
                target.tags = dedupe_trimmed(target.tags);
                db.update_place_baseline(&target)?;
                db.delete_place_baseline(&old_id)?;
                merged = true;
            } else {
                let mut renamed = old;
                renamed.id = new_id.clone();
                db.rename_place_baseline_id(&old_id, &new_id, &renamed)?;
                merged = false;
            }
        }
        "lore" => {
            let old = db
                .get_lore(&old_id)?
                .ok_or_else(|| anyhow!("Lore '{old_id}' not found"))?;
            if let Some(mut target) = db.get_lore(&new_id)? {
                if !req.allow_merge {
                    bail!("Lore '{new_id}' already exists (pass allow_merge: true to merge).");
                }
                if !old.body.trim().is_empty() && !target.body.contains(old.body.trim()) {
                    target.body = format!("{}\n\n---\n\n{}", target.body, old.body);
                }
                target.keywords.extend(old.keywords);
                target.keywords = dedupe_trimmed(target.keywords);
                target.tags.extend(old.tags);
                target.tags = dedupe_trimmed(target.tags);
                target.related_ids.extend(old.related_ids);
                target.related_ids = dedupe_slugs(target.related_ids);
                db.upsert_lore(&target)?;
                db.delete_lore(&old_id)?;
                merged = true;
            } else {
                let mut renamed = old;
                renamed.id = new_id.clone();
                db.upsert_lore(&renamed)?;
                db.delete_lore(&old_id)?;
                merged = false;
            }
        }
        "text" => {
            let old = db
                .get_text(&old_id)?
                .ok_or_else(|| anyhow!("StoryText '{old_id}' not found"))?;
            if let Some(mut target) = db.get_text(&new_id)? {
                if !req.allow_merge {
                    bail!("StoryText '{new_id}' already exists (pass allow_merge: true to merge).");
                }
                target.read_by.extend(old.read_by);
                target.read_by = dedupe_slugs(target.read_by);
                target.keywords.extend(old.keywords);
                target.keywords = dedupe_trimmed(target.keywords);
                target.tags.extend(old.tags);
                target.tags = dedupe_trimmed(target.tags);
                target.related_ids.extend(old.related_ids);
                target.related_ids = dedupe_slugs(target.related_ids);
                db.upsert_text(&target)?;
                db.delete_text(&old_id)?;
                merged = true;
            } else {
                let mut renamed = old;
                renamed.id = new_id.clone();
                db.upsert_text(&renamed)?;
                db.delete_text(&old_id)?;
                merged = false;
            }
        }
        "item" => {
            let old = db
                .get_catalog_item(&old_id)?
                .ok_or_else(|| anyhow!("CatalogItem '{old_id}' not found"))?;
            if let Some(mut target) = db.get_catalog_item(&new_id)? {
                if !req.allow_merge {
                    bail!("CatalogItem '{new_id}' already exists (pass allow_merge: true to merge).");
                }
                target.previous_owners.extend(old.previous_owners);
                target.previous_owners = dedupe_trimmed(target.previous_owners);
                if target.origin_and_maker.is_empty() {
                    target.origin_and_maker = old.origin_and_maker;
                }
                if target.significance.is_empty() {
                    target.significance = old.significance;
                }
                target.keywords.extend(old.keywords);
                target.keywords = dedupe_trimmed(target.keywords);
                target.tags.extend(old.tags);
                target.tags = dedupe_trimmed(target.tags);
                target.related_ids.extend(old.related_ids);
                target.related_ids = dedupe_slugs(target.related_ids);
                db.upsert_catalog_item(&target)?;
                db.delete_catalog_item(&old_id)?;
                merged = true;
            } else {
                let mut renamed = old;
                renamed.id = new_id.clone();
                db.upsert_catalog_item(&renamed)?;
                db.delete_catalog_item(&old_id)?;
                merged = false;
            }
        }
        "attachment" => {
            let old = db
                .get_attachment(&old_id)?
                .ok_or_else(|| anyhow!("Attachment '{old_id}' not found"))?;
            if let Some(mut target) = db.get_attachment(&new_id)? {
                if !req.allow_merge {
                    bail!("Attachment '{new_id}' already exists (pass allow_merge: true to merge).");
                }
                target.keywords.extend(old.keywords);
                target.keywords = dedupe_trimmed(target.keywords);
                target.tags.extend(old.tags);
                target.tags = dedupe_trimmed(target.tags);
                target.related_ids.extend(old.related_ids);
                target.related_ids = dedupe_slugs(target.related_ids);
                db.upsert_attachment(&target)?;
                db.delete_attachment(&old_id)?;
                merged = true;
            } else {
                let mut renamed = old;
                renamed.id = new_id.clone();
                db.upsert_attachment(&renamed)?;
                db.delete_attachment(&old_id)?;
                merged = false;
            }
        }
        "faction" => {
            let old = db
                .get_faction(&old_id)?
                .ok_or_else(|| anyhow!("Faction '{old_id}' not found"))?;
            if let Some(mut target) = db.get_faction(&new_id)? {
                if !req.allow_merge {
                    bail!("Faction '{new_id}' already exists (pass allow_merge: true to merge).");
                }
                if target.origins.is_empty() && !old.origins.is_empty() {
                    target.origins = old.origins;
                }
                if target.leadership_model.is_empty() && !old.leadership_model.is_empty() {
                    target.leadership_model = old.leadership_model;
                }
                if target.ideology.is_empty() && !old.ideology.is_empty() {
                    target.ideology = old.ideology;
                }
                if target.leader_id.is_none() {
                    target.leader_id = old.leader_id;
                }
                for m in old.members {
                    if !target
                        .members
                        .iter()
                        .any(|tm| tm.character_id == m.character_id)
                    {
                        target.members.push(m);
                    }
                }
                target.territory.extend(old.territory);
                target.territory = dedupe_slugs(target.territory);
                for (rk, rv) in old.resources {
                    target.resources.entry(rk).or_insert(rv);
                }
                for (rk, rv) in old.reputation {
                    target.reputation.entry(rk).or_insert(rv);
                }
                for o in old.objectives {
                    if !target.objectives.iter().any(|to| to.goal_id == o.goal_id) {
                        target.objectives.push(o);
                    }
                }
                target.internal_conflicts.extend(old.internal_conflicts);
                target.internal_conflicts = dedupe_trimmed(target.internal_conflicts);
                target.tags.extend(old.tags);
                target.tags = dedupe_trimmed(target.tags);
                db.upsert_faction(&target)?;
                db.delete_faction(&old_id)?;
                merged = true;
            } else {
                let mut renamed = old;
                renamed.id = new_id.clone();
                db.upsert_faction(&renamed)?;
                db.delete_faction(&old_id)?;
                merged = false;
            }
        }
        other => bail!(
            "Unsupported register '{other}' for rename_or_merge_entity. Supported: character, place, lore, text, item, attachment, faction."
        ),
    }

    let references_updated = rewrite_all_references(engine, &old_id, &new_id, Some(reg))?;

    let verb = if merged { "Merged" } else { "Renamed" };
    let summary = format!(
        "{verb} {reg} '{old_id}' -> '{new_id}' ({references_updated} cross-reference(s) updated){}",
        if req.reason.is_empty() {
            String::new()
        } else {
            format!(" — {}", req.reason)
        }
    );
    let ev = db.append_event(
        None,
        vec![old_id.clone(), new_id.clone()],
        summary.clone(),
        EventPayload::EntityRenamedOrMerged {
            register: reg.to_string(),
            old_id: old_id.clone(),
            new_id: new_id.clone(),
            merged,
            references_updated,
            reason: req.reason,
        },
    )?;

    engine.maybe_compile()?;
    let pruned_docs = prune_orphaned_docs(db, &engine.docs_dir())?;

    Ok(RenameOrMergeResult {
        register: reg.to_string(),
        old_id,
        new_id,
        merged,
        references_updated,
        pruned_docs,
        event_seq: ev.seq,
        summary,
    })
}

fn rewrite_all_references(
    engine: &CampaignEngine,
    old_id: &str,
    new_id: &str,
    register_hint: Option<&str>,
) -> Result<usize> {
    let db = engine.db();
    let mut updated = 0usize;

    // 1. Relations
    for mut r in db.list_all_relations()? {
        let mut changed = false;
        let orig_s = r.source_id.clone();
        let orig_t = r.target_id.clone();
        let orig_ty = r.relation_type.clone();
        if r.source_id == old_id {
            r.source_id = new_id.to_string();
            changed = true;
        }
        if r.target_id == old_id {
            r.target_id = new_id.to_string();
            changed = true;
        }
        if changed {
            db.delete_relation(&orig_s, &orig_ty, &orig_t)?;
            if r.source_id != r.target_id {
                db.upsert_relation(&r)?;
            }
            updated += 1;
        }
    }

    // 2. Entity Histories
    for mut h in db.list_all_histories()? {
        let mut changed = false;
        if h.entity_id == old_id {
            h.entity_id = new_id.to_string();
            changed = true;
        }
        if replace_in_vec(&mut h.related_ids, old_id, new_id) {
            changed = true;
        }
        if changed {
            db.update_entity_history(&h)?;
            updated += 1;
        }
    }

    // 3. Lore related_ids
    for mut l in db.list_lore(None)? {
        if replace_in_vec(&mut l.related_ids, old_id, new_id) {
            db.upsert_lore(&l)?;
            updated += 1;
        }
    }

    // 4. StoryTexts
    for mut t in db.list_texts()? {
        let mut changed = false;
        if t.holder_or_location_id.as_deref() == Some(old_id) {
            t.holder_or_location_id = Some(new_id.to_string());
            changed = true;
        }
        if replace_in_vec(&mut t.read_by, old_id, new_id) {
            changed = true;
        }
        if replace_in_vec(&mut t.related_ids, old_id, new_id) {
            changed = true;
        }
        if changed {
            db.upsert_text(&t)?;
            updated += 1;
        }
    }

    // 5. CatalogItems
    for mut it in db.list_catalog_items()? {
        let mut changed = false;
        if it.current_holder_id.as_deref() == Some(old_id) {
            it.current_holder_id = Some(new_id.to_string());
            changed = true;
        }
        if it.linked_text_id.as_deref() == Some(old_id) {
            it.linked_text_id = Some(new_id.to_string());
            changed = true;
        }
        if replace_in_vec(&mut it.previous_owners, old_id, new_id) {
            changed = true;
        }
        if replace_in_vec(&mut it.related_ids, old_id, new_id) {
            changed = true;
        }
        if changed {
            db.upsert_catalog_item(&it)?;
            updated += 1;
        }
    }

    // 6. Attachments
    for mut a in db.list_attachments()? {
        if replace_in_vec(&mut a.related_ids, old_id, new_id) {
            db.upsert_attachment(&a)?;
            updated += 1;
        }
    }

    // 7. Clocks
    for mut cl in db.list_clocks()? {
        if replace_in_vec(&mut cl.related_ids, old_id, new_id) {
            db.upsert_clock(&cl)?;
            updated += 1;
        }
    }

    // 8. Places (parent_place & routes) + Characters currently at a renamed/merged place
    if register_hint.is_none() || register_hint == Some("place") {
        for mut pb in db.list_place_baselines()? {
            let mut changed = false;
            if pb.parent_place.as_deref() == Some(old_id) {
                pb.parent_place = Some(new_id.to_string());
                changed = true;
            }
            for rt in &mut pb.routes {
                if rt.destination_id == old_id {
                    rt.destination_id = new_id.to_string();
                    changed = true;
                }
            }
            if changed {
                db.update_place_baseline(&pb)?;
                updated += 1;
            }
        }
        for mut cb in db.list_character_baselines()? {
            if cb.initial_location.as_deref() == Some(old_id) {
                cb.initial_location = Some(new_id.to_string());
                db.update_character_baseline(&cb)?;
                updated += 1;
            }
        }
        for cp in engine.list_characters()? {
            if cp.current.current_location.as_deref() == Some(old_id) {
                let _ = db.append_event(
                    Some(new_id.to_string()),
                    vec![cp.baseline.id.clone(), new_id.to_string()],
                    format!(
                        "{} location remapped from '{}' to '{}' after place rename/merge",
                        cp.baseline.name, old_id, new_id
                    ),
                    EventPayload::CharacterMoved {
                        character_id: cp.baseline.id.clone(),
                        from_place: Some(old_id.to_string()),
                        to_place: new_id.to_string(),
                        travel_notes: "Place renamed/merged".to_string(),
                    },
                )?;
                updated += 1;
            }
        }
    }

    Ok(updated)
}

// =============================================================================
// Clean Entity Deletion (with Stale Doc Pruning & Audit Trail)
// =============================================================================

#[derive(Debug, Clone, Deserialize)]
pub struct DeleteEntityRequest {
    /// Register: "character", "place", "lore", "text", "item", "history", "attachment", "clock"
    pub register: String,
    pub entity_id: String,
    /// If true and deleting an attachment, also delete its backing file under `docs/attachments/files/`
    #[serde(default)]
    pub remove_backing_file: bool,
    /// If true, also remove Knowledge Graph relations referencing this entity
    #[serde(default = "default_true")]
    pub cleanup_relations: bool,
    #[serde(default)]
    pub reason: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
pub struct DeleteEntityResult {
    pub register: String,
    pub entity_id: String,
    pub deleted: bool,
    pub relations_removed: usize,
    pub pruned_docs: Vec<String>,
    pub event_seq: i64,
    pub summary: String,
}

pub fn delete_entity(
    engine: &CampaignEngine,
    req: DeleteEntityRequest,
) -> Result<DeleteEntityResult> {
    let reg = normalize_register(&req.register)?;
    let db = engine.db();

    let (canonical_id, deleted) = match reg {
        "history" => {
            let hid: i64 = req
                .entity_id
                .trim()
                .parse()
                .map_err(|_| anyhow!("History entity_id must be an integer ID"))?;
            (hid.to_string(), db.delete_entity_history(hid)?)
        }
        "character" => {
            let id = slugify(&req.entity_id);
            (id.clone(), db.delete_character_baseline(&id)?)
        }
        "place" => {
            let id = slugify(&req.entity_id);
            (id.clone(), db.delete_place_baseline(&id)?)
        }
        "lore" => {
            let id = slugify(&req.entity_id);
            (id.clone(), db.delete_lore(&id)?)
        }
        "text" => {
            let id = slugify(&req.entity_id);
            (id.clone(), db.delete_text(&id)?)
        }
        "item" => {
            let id = slugify(&req.entity_id);
            (id.clone(), db.delete_catalog_item(&id)?)
        }
        "attachment" => {
            let id = slugify(&req.entity_id);
            if req.remove_backing_file {
                if let Some(att) = db.get_attachment(&id)? {
                    let abs = engine.attachment_file_path(&att);
                    if abs.exists() {
                        let _ = std::fs::remove_file(&abs);
                    }
                }
            }
            (id.clone(), db.delete_attachment(&id)?)
        }
        "clock" => {
            let id = slugify(&req.entity_id);
            (id.clone(), db.delete_clock(&id)?)
        }
        "faction" => {
            let id = slugify(&req.entity_id);
            (id.clone(), db.delete_faction(&id)?)
        }
        other => bail!("Unsupported register '{other}' for delete_entity."),
    };

    if !deleted {
        bail!("{reg} '{canonical_id}' not found.");
    }

    let mut relations_removed = 0usize;
    if req.cleanup_relations && reg != "history" && reg != "clock" {
        for r in db.list_relations_for_entity(&canonical_id)? {
            if db.delete_relation(&r.source_id, &r.relation_type, &r.target_id)? {
                relations_removed += 1;
            }
        }
    }

    let summary = format!(
        "Deleted {reg} '{canonical_id}'{}",
        if req.reason.is_empty() {
            String::new()
        } else {
            format!(" ({})", req.reason)
        }
    );
    let ev = db.append_event(
        None,
        vec![canonical_id.clone()],
        summary.clone(),
        EventPayload::EntityDeleted {
            register: reg.to_string(),
            entity_id: canonical_id.clone(),
            reason: req.reason,
        },
    )?;

    engine.maybe_compile()?;
    let pruned_docs = prune_orphaned_docs(db, &engine.docs_dir())?;

    Ok(DeleteEntityResult {
        register: reg.to_string(),
        entity_id: canonical_id,
        deleted: true,
        relations_removed,
        pruned_docs,
        event_seq: ev.seq,
        summary,
    })
}

// =============================================================================
// Automated Safe Campaign Hygiene / Cleanup
// =============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct CleanupReport {
    pub dry_run: bool,
    pub orphaned_docs_removed: Vec<String>,
    pub texts_synced_to_items: Vec<String>,
    pub items_synced_to_inventories: Vec<String>,
    pub records_normalized: usize,
    pub actions: Vec<String>,
}

pub fn cleanup_campaign(engine: &CampaignEngine, dry_run: bool) -> Result<CleanupReport> {
    let db = engine.db();
    let mut actions = Vec::new();
    let mut texts_synced = Vec::new();
    let mut items_synced = Vec::new();
    let mut records_normalized = 0usize;

    // 1. Orphaned compiled .md files
    let orphans = list_orphaned_docs(db, &engine.docs_dir())?;
    for o in &orphans {
        actions.push(format!(
            "{} stale doc 'docs/{o}'",
            if dry_run { "Would prune" } else { "Pruned" }
        ));
    }

    // 2. Normalize & deduplicate Lore
    for mut l in db.list_lore(None)? {
        let norm_vis = normalize_visibility(&l.visibility);
        let dedup_kw = dedupe_trimmed(l.keywords.clone());
        let dedup_tags = dedupe_trimmed(l.tags.clone());
        let dedup_rel = dedupe_slugs(l.related_ids.clone());
        if l.visibility != norm_vis
            || l.keywords != dedup_kw
            || l.tags != dedup_tags
            || l.related_ids != dedup_rel
        {
            records_normalized += 1;
            actions.push(format!(
                "{} lore '{}' metadata (visibility/tags/keywords/links)",
                if dry_run { "Would normalize" } else { "Normalized" },
                l.id
            ));
            if !dry_run {
                l.visibility = norm_vis;
                l.keywords = dedup_kw;
                l.tags = dedup_tags;
                l.related_ids = dedup_rel;
                db.upsert_lore(&l)?;
            }
        }
    }

    // 3. Normalize StoryTexts & ensure carryable physical texts sync to CatalogItems
    for mut t in db.list_texts()? {
        let norm_rs = normalize_read_status(&t.read_status);
        let dedup_rb = dedupe_slugs(t.read_by.clone());
        let dedup_kw = dedupe_trimmed(t.keywords.clone());
        let dedup_tags = dedupe_trimmed(t.tags.clone());
        let dedup_rel = dedupe_slugs(t.related_ids.clone());
        if t.read_status != norm_rs
            || t.read_by != dedup_rb
            || t.keywords != dedup_kw
            || t.tags != dedup_tags
            || t.related_ids != dedup_rel
        {
            records_normalized += 1;
            actions.push(format!(
                "{} story text '{}' metadata",
                if dry_run { "Would normalize" } else { "Normalized" },
                t.id
            ));
            if !dry_run {
                t.read_status = norm_rs;
                t.read_by = dedup_rb;
                t.keywords = dedup_kw;
                t.tags = dedup_tags;
                t.related_ids = dedup_rel;
                db.upsert_text(&t)?;
            }
        }

        if is_carryable_text_medium(&t.medium) {
            let existing = db.get_catalog_item(&t.id)?;
            let needs_sync = match existing {
                None => true,
                Some(ref ci) => ci.current_holder_id != t.holder_or_location_id,
            };
            if needs_sync {
                texts_synced.push(t.id.clone());
                actions.push(format!(
                    "{} carryable text '{}' into catalog_items & holder inventory",
                    if dry_run { "Would sync" } else { "Synced" },
                    t.id
                ));
                if !dry_run {
                    engine.sync_text_as_carryable_item(&t)?;
                }
            }
        }
    }

    // 4. Normalize CatalogItems & sync held items into character inventories
    let chars = engine.list_characters()?;
    let char_by_id: BTreeMap<String, &_> =
        chars.iter().map(|c| (c.baseline.id.clone(), c)).collect();
    for mut it in db.list_catalog_items()? {
        let dedup_prev = dedupe_trimmed(it.previous_owners.clone());
        let dedup_kw = dedupe_trimmed(it.keywords.clone());
        let dedup_tags = dedupe_trimmed(it.tags.clone());
        let dedup_rel = dedupe_slugs(it.related_ids.clone());
        if it.previous_owners != dedup_prev
            || it.keywords != dedup_kw
            || it.tags != dedup_tags
            || it.related_ids != dedup_rel
        {
            records_normalized += 1;
            actions.push(format!(
                "{} catalog item '{}' metadata",
                if dry_run { "Would normalize" } else { "Normalized" },
                it.id
            ));
            if !dry_run {
                it.previous_owners = dedup_prev;
                it.keywords = dedup_kw;
                it.tags = dedup_tags;
                it.related_ids = dedup_rel;
                db.upsert_catalog_item(&it)?;
            }
        }

        if let Some(ref h) = it.current_holder_id {
            if let Some(cp) = char_by_id.get(h) {
                let in_inv = cp
                    .current
                    .equipment
                    .iter()
                    .any(|e| e.name.eq_ignore_ascii_case(&it.name));
                if !in_inv {
                    items_synced.push(it.id.clone());
                    actions.push(format!(
                        "{} catalog item '{}' into character '{}' inventory",
                        if dry_run { "Would sync" } else { "Synced" },
                        it.id,
                        h
                    ));
                    if !dry_run {
                        engine.sync_catalog_item_to_character_inventories(&it, None)?;
                    }
                }
            }
        }
    }

    // 5. Normalize Entity Histories
    for mut h in db.list_all_histories()? {
        let norm_kind = normalize_entity_kind(&h.entity_kind);
        let norm_vis = normalize_visibility(&h.visibility);
        let dedup_rel = dedupe_slugs(h.related_ids.clone());
        if h.entity_kind != norm_kind || h.visibility != norm_vis || h.related_ids != dedup_rel {
            records_normalized += 1;
            actions.push(format!(
                "{} history #{} ('{}') metadata",
                if dry_run { "Would normalize" } else { "Normalized" },
                h.id,
                h.title
            ));
            if !dry_run {
                h.entity_kind = norm_kind;
                h.visibility = norm_vis;
                h.related_ids = dedup_rel;
                db.update_entity_history(&h)?;
            }
        }
    }

    let orphaned_docs_removed = if dry_run {
        orphans
    } else {
        engine.compile_docs()?;
        prune_orphaned_docs(db, &engine.docs_dir())?
    };

    Ok(CleanupReport {
        dry_run,
        orphaned_docs_removed,
        texts_synced_to_items: texts_synced,
        items_synced_to_inventories: items_synced,
        records_normalized,
        actions,
    })
}

// =============================================================================
// Helper Normalization Utilities
// =============================================================================

fn normalize_register(raw: &str) -> Result<&'static str> {
    match raw.trim().to_lowercase().as_str() {
        "character" | "characters" | "char" | "pc" | "npc" => Ok("character"),
        "place" | "places" | "location" | "establishment" => Ok("place"),
        "lore" | "world" | "lorebook" => Ok("lore"),
        "text" | "texts" | "book" | "books" | "scroll" | "scrolls" => Ok("text"),
        "item" | "items" | "artifact" | "relic" | "gear" => Ok("item"),
        "history" | "histories" | "entity_history" => Ok("history"),
        "attachment" | "attachments" | "doc" | "docs" => Ok("attachment"),
        "clock" | "clocks" => Ok("clock"),
        "faction" | "factions" | "organization" | "sect" | "guild" => Ok("faction"),
        other => bail!(
            "Unknown register '{other}'. Expected one of: character, place, lore, text, item, history, attachment, clock, faction."
        ),
    }
}

fn is_canonical_visibility(v: &str) -> bool {
    matches!(v, "public" | "rumor" | "gm_secret")
}

fn normalize_visibility(v: &str) -> String {
    match v.trim().to_lowercase().replace([' ', '-'], "_").as_str() {
        "rumor" | "rumour" => "rumor".to_string(),
        "gm_secret" | "secret" | "gm" | "hidden" | "private" => "gm_secret".to_string(),
        _ => "public".to_string(),
    }
}

fn is_canonical_read_status(v: &str) -> bool {
    matches!(
        v,
        "undiscovered" | "discovered_unread" | "partially_read" | "read" | "deciphered"
    )
}

fn normalize_read_status(v: &str) -> String {
    match v.trim().to_lowercase().replace([' ', '-'], "_").as_str() {
        "undiscovered" | "hidden" => "undiscovered".to_string(),
        "discovered_unread" | "unread" | "found" => "discovered_unread".to_string(),
        "partially_read" | "partial" | "skimmed" => "partially_read".to_string(),
        "deciphered" | "translated" | "decoded" => "deciphered".to_string(),
        "read" | "studied" | "completed" => "read".to_string(),
        _ => "discovered_unread".to_string(),
    }
}

fn is_canonical_entity_kind(v: &str) -> bool {
    matches!(
        v,
        "place" | "establishment" | "sect" | "faction" | "item" | "text" | "character" | "lore"
    )
}

fn normalize_entity_kind(v: &str) -> String {
    match v.trim().to_lowercase().as_str() {
        "establishment" | "tavern" | "guild" | "academy" | "temple" | "inn" => {
            "establishment".to_string()
        }
        "sect" | "order" | "cult" | "clan" => "sect".to_string(),
        "faction" | "house" | "organization" => "faction".to_string(),
        "item" | "artifact" | "weapon" | "tool" | "keepsake" | "relic" => "item".to_string(),
        "text" | "book" | "scroll" | "manual" | "record" => "text".to_string(),
        "character" | "pc" | "npc" => "character".to_string(),
        "lore" | "world" => "lore".to_string(),
        _ => "place".to_string(),
    }
}

fn dedupe_trimmed(items: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for s in items {
        let t = s.trim().to_string();
        if !t.is_empty() && seen.insert(t.to_lowercase()) {
            out.push(t);
        }
    }
    out
}

fn dedupe_slugs(items: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for s in items {
        if !s.trim().is_empty() {
            let sl = slugify(&s);
            if seen.insert(sl.clone()) {
                out.push(sl);
            }
        }
    }
    out
}

fn replace_in_vec(vec: &mut Vec<String>, old_id: &str, new_id: &str) -> bool {
    let mut changed = false;
    for item in vec.iter_mut() {
        if item == old_id {
            *item = new_id.to_string();
            changed = true;
        }
    }
    if changed {
        *vec = dedupe_trimmed(std::mem::take(vec));
    }
    changed
}

// =============================================================================
// Campaign Metadata, Attachment Metadata, Clock Update & Safe Backup
// =============================================================================

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateCampaignMetaRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub ruleset_notes: Option<String>,
    pub era: Option<String>,
    pub year: Option<i32>,
    pub season: Option<String>,
    pub day: Option<u32>,
    pub hour: Option<u32>,
    pub minute: Option<u32>,
    pub days_per_season: Option<u32>,
    pub seasons: Option<Vec<String>>,
    pub global_weather: Option<String>,
    #[serde(default)]
    pub reason: String,
}

pub fn update_campaign_meta(
    engine: &CampaignEngine,
    req: UpdateCampaignMetaRequest,
) -> Result<(CampaignMeta, WorldEvent)> {
    let mut meta = engine.db().get_campaign_meta()?;
    let mut changed = Vec::new();

    if let Some(v) = req.name {
        if !v.trim().is_empty() {
            meta.name = v;
            changed.push("name".to_string());
        }
    }
    if let Some(v) = req.description {
        meta.description = v;
        changed.push("description".to_string());
    }
    if let Some(v) = req.ruleset_notes {
        meta.ruleset_notes = v;
        changed.push("ruleset_notes".to_string());
    }
    if let Some(v) = req.era {
        meta.clock.era = v;
        changed.push("clock.era".to_string());
    }
    if let Some(v) = req.year {
        meta.clock.year = v;
        changed.push("clock.year".to_string());
    }
    if let Some(v) = req.season {
        meta.clock.season = v;
        changed.push("clock.season".to_string());
    }
    if let Some(v) = req.day {
        meta.clock.day = v.max(1);
        changed.push("clock.day".to_string());
    }
    if let Some(v) = req.hour {
        meta.clock.hour = v % 24;
        changed.push("clock.hour".to_string());
    }
    if let Some(v) = req.minute {
        meta.clock.minute = v % 60;
        changed.push("clock.minute".to_string());
    }
    if let Some(v) = req.days_per_season {
        meta.clock.days_per_season = v.max(1);
        changed.push("clock.days_per_season".to_string());
    }
    if let Some(v) = req.seasons {
        if !v.is_empty() {
            meta.clock.seasons = v;
            changed.push("clock.seasons".to_string());
        }
    }
    if let Some(v) = req.global_weather {
        meta.clock.global_weather = v;
        changed.push("clock.global_weather".to_string());
    }
    if changed.is_empty() {
        bail!("No fields provided to update on campaign metadata.");
    }

    engine.db().update_campaign_meta_row(&meta)?;
    let summary = format!(
        "Updated campaign metadata ({}){}",
        changed.join(", "),
        if req.reason.is_empty() {
            String::new()
        } else {
            format!(" — {}", req.reason)
        }
    );
    let ev = engine.db().append_event(
        None,
        vec![meta.id.clone()],
        summary,
        EventPayload::EntityEdited {
            register: "campaign".to_string(),
            entity_id: meta.id.clone(),
            fields_changed: changed,
            reason: req.reason,
        },
    )?;
    engine.maybe_compile()?;
    Ok((meta, ev))
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateAttachmentRequest {
    pub attachment_id: String,
    pub title: Option<String>,
    pub category: Option<String>,
    pub summary: Option<String>,
    pub extracted_notes: Option<String>,
    pub visibility: Option<String>,
    pub keywords: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub related_ids: Option<Vec<String>>,
    #[serde(default)]
    pub reason: String,
}

pub fn update_attachment_metadata(
    engine: &CampaignEngine,
    req: UpdateAttachmentRequest,
) -> Result<(StoryAttachment, WorldEvent)> {
    let mut att = engine.get_attachment(&req.attachment_id)?;
    let mut changed = Vec::new();
    if let Some(v) = req.title {
        if !v.trim().is_empty() {
            att.title = v;
            changed.push("title".to_string());
        }
    }
    if let Some(v) = req.category {
        att.category = v;
        changed.push("category".to_string());
    }
    if let Some(v) = req.summary {
        att.summary = v;
        changed.push("summary".to_string());
    }
    if let Some(v) = req.extracted_notes {
        att.extracted_notes = v;
        changed.push("extracted_notes".to_string());
    }
    if let Some(v) = req.visibility {
        att.visibility = normalize_visibility(&v);
        changed.push("visibility".to_string());
    }
    if let Some(v) = req.keywords {
        att.keywords = dedupe_trimmed(v);
        changed.push("keywords".to_string());
    }
    if let Some(v) = req.tags {
        att.tags = dedupe_trimmed(v);
        changed.push("tags".to_string());
    }
    if let Some(v) = req.related_ids {
        att.related_ids = dedupe_slugs(v);
        changed.push("related_ids".to_string());
    }
    if changed.is_empty() {
        bail!("No fields provided to update on attachment '{}'", att.id);
    }
    att.updated_at_world_time = engine.db().get_campaign_meta()?.clock.format_short();
    engine.db().upsert_attachment(&att)?;
    let mut affected = vec![att.id.clone()];
    affected.extend(att.related_ids.clone());
    let summary = format!("Updated attachment '{}' ({})", att.title, changed.join(", "));
    let ev = engine.db().append_event(
        None,
        affected,
        summary,
        EventPayload::EntityEdited {
            register: "attachment".to_string(),
            entity_id: att.id.clone(),
            fields_changed: changed,
            reason: req.reason,
        },
    )?;
    engine.maybe_compile()?;
    Ok((att, ev))
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateClockRequest {
    pub clock_id: String,
    pub name: Option<String>,
    pub kind: Option<String>,
    pub segments: Option<i64>,
    pub filled: Option<i64>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub related_ids: Option<Vec<String>>,
    #[serde(default)]
    pub reason: String,
}

pub fn update_clock_fields(
    engine: &CampaignEngine,
    req: UpdateClockRequest,
) -> Result<(ProgressClock, WorldEvent)> {
    let id = slugify(&req.clock_id);
    let mut cl = engine
        .db()
        .get_clock(&id)?
        .ok_or_else(|| anyhow!("Clock '{id}' not found"))?;
    let mut changed = Vec::new();
    if let Some(v) = req.name {
        if !v.trim().is_empty() {
            cl.name = v;
            changed.push("name".to_string());
        }
    }
    if let Some(v) = req.kind {
        cl.kind = v;
        changed.push("kind".to_string());
    }
    if let Some(v) = req.segments {
        cl.segments = v.max(1);
        cl.filled = cl.filled.clamp(0, cl.segments);
        changed.push("segments".to_string());
    }
    if let Some(v) = req.filled {
        cl.filled = v.clamp(0, cl.segments);
        changed.push("filled".to_string());
    }
    if let Some(v) = req.status {
        cl.status = v;
        changed.push("status".to_string());
    }
    if let Some(v) = req.description {
        cl.description = v;
        changed.push("description".to_string());
    }
    if let Some(v) = req.related_ids {
        cl.related_ids = dedupe_slugs(v);
        changed.push("related_ids".to_string());
    }
    if changed.is_empty() {
        bail!("No fields provided to update on clock '{id}'");
    }
    engine.db().upsert_clock(&cl)?;
    let summary = format!("Updated clock '{}' ({})", cl.name, changed.join(", "));
    let ev = engine.db().append_event(
        None,
        cl.related_ids.clone(),
        summary,
        EventPayload::EntityEdited {
            register: "clock".to_string(),
            entity_id: cl.id.clone(),
            fields_changed: changed,
            reason: req.reason,
        },
    )?;
    engine.maybe_compile()?;
    Ok((cl, ev))
}

pub fn backup_campaign(engine: &CampaignEngine, label: &str) -> Result<PathBuf> {
    engine.db().create_backup(label)
}

pub fn list_backups(engine: &CampaignEngine) -> Result<Vec<Value>> {
    let dir = engine.root_dir();
    let mut items = Vec::new();
    if dir.exists() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("campaign.backup-") && name.ends_with(".db") {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                items.push(json!({
                    "filename": name,
                    "size_bytes": size,
                    "path": entry.path().display().to_string()
                }));
            }
        }
    }
    items.sort_by(|a, b| {
        b["filename"]
            .as_str()
            .unwrap_or("")
            .cmp(a["filename"].as_str().unwrap_or(""))
    });
    Ok(items)
}

pub fn restore_backup(engine: &CampaignEngine, backup_filename: &str) -> Result<Value> {
    let (restored_from, safety_backup) = engine.db().restore_from_backup(backup_filename)?;
    let report = engine.compile_docs()?;
    let pruned_docs = prune_orphaned_docs(engine.db(), &engine.docs_dir())?;
    Ok(json!({
        "restored_from": restored_from.display().to_string(),
        "safety_backup": safety_backup.display().to_string(),
        "total_events": report.total_events,
        "pruned_docs": pruned_docs
    }))
}
