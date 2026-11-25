use crate::autonomy::{ActorKnowledgeBrief, FactionRecord, RelationshipDimensions};
use crate::db::CampaignDb;
use crate::mechanics::{EncounterState, StagedDiscovery};
use crate::models::{
    CatalogItem, CharacterCurrent, EntityHistoryEntry, GraphRelation, LoreEntry, PlaceCurrent,
    ProgressClock, StoryAttachment, StoryText, WorldClock, WorldEvent,
};
use crate::projection::{load_all_character_projections, load_all_place_projections};
use crate::search::{resolve_character_id, resolve_place_id};
use crate::signals::{
    evaluate_live_signals, filter_signals_for_perspective, render_signals_markdown, EngineSignal,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneContextRequest {
    pub place_id: Option<String>,
    #[serde(default)]
    pub character_ids: Vec<String>,
    #[serde(default)]
    pub scene_text: String,
    #[serde(default = "default_recent_limit")]
    pub recent_events_limit: usize,
    #[serde(default = "default_include_secrets")]
    pub include_gm_secrets: bool,
    /// Optional epistemic POV: "gm" (default), "player", or a specific "<character_id>".
    /// Non-GM perspectives enforce a hard epistemic wall (zero GM secrets, only that actor's actual beliefs!).
    #[serde(default)]
    pub perspective: Option<String>,
}

fn default_recent_limit() -> usize {
    8
}

fn default_include_secrets() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterBrief {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub archetype: String,
    pub current: CharacterCurrent,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub progression_tracks: Vec<crate::world_sim::CharacterProgressionTrack>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_titles: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub avatar_brief: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub apparel_fit_brief: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub perceived_change_note: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub avatar_delta_brief: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub transformation_trajectory: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_jobs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key_proficiencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_traits: Vec<String>,
    pub recent_events: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceBrief {
    pub id: String,
    pub name: String,
    pub place_type: String,
    pub terrain: String,
    pub climate: String,
    pub active_seasonal_note: Option<String>,
    pub current: PlaceCurrent,
    pub recent_events: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggeredLore {
    pub id: String,
    pub title: String,
    pub category: String,
    pub visibility: String,
    pub trigger_reason: String,
    pub summary: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggeredText {
    pub id: String,
    pub title: String,
    pub medium: String,
    pub subject: String,
    pub language: String,
    pub condition: String,
    pub read_status: String,
    pub read_by: Vec<String>,
    pub holder_or_location_id: Option<String>,
    pub trigger_reason: String,
    pub summary: String,
    pub content: String,
    pub mechanics_or_unlocks: Vec<String>,
    pub gm_notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggeredAttachment {
    pub id: String,
    pub title: String,
    pub category: String,
    pub mime_type: String,
    pub relative_path: String,
    pub mcp_uri: String,
    pub visibility: String,
    pub trigger_reason: String,
    pub summary: String,
    pub extracted_notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggeredItem {
    pub id: String,
    pub name: String,
    pub item_type: String,
    pub condition_or_durability: String,
    pub current_holder_id: Option<String>,
    pub previous_owners: Vec<String>,
    pub significance: String,
    pub properties_and_effects: String,
    pub linked_text_id: Option<String>,
    pub gm_notes: String,
    pub trigger_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlindSpotNotice {
    pub actor_id: String,
    pub unknown_claim_id: String,
    pub subject: String,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneContextPacket {
    pub campaign_name: String,
    pub session_number: u32,
    pub world_clock: WorldClock,
    pub time_of_day: String,
    #[serde(default = "default_perspective_label")]
    pub perspective: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_signals: Vec<EngineSignal>,
    pub active_place: Option<PlaceBrief>,
    pub characters: Vec<CharacterBrief>,
    pub knowledge_graph_edges: Vec<GraphRelation>,
    pub triggered_lore: Vec<TriggeredLore>,
    pub relevant_texts: Vec<TriggeredText>,
    pub relevant_items: Vec<TriggeredItem>,
    pub relevant_histories: Vec<EntityHistoryEntry>,
    pub relevant_attachments: Vec<TriggeredAttachment>,
    pub active_clocks: Vec<ProgressClock>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub active_encounters: Vec<EncounterState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_discoveries: Vec<StagedDiscovery>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub character_knowledge: Vec<ActorKnowledgeBrief>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub npc_blind_spots: Vec<BlindSpotNotice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prominent_figure_movements: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub local_map_grid_neighborhood: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub present_factions: Vec<FactionRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gm_only_secrets: Vec<String>,
    pub recent_campaign_events: Vec<String>,
}

fn default_perspective_label() -> String {
    "gm".to_string()
}

/// Assembles a compact, deterministic, hallucination-proof GM context packet for the current scene:
/// - World clock & season
/// - Active place (terrain, seasonal effects, weather, landmarks, routes, present chars)
/// - Present & requested characters (vitals, stats, conditions, gear, skills, techniques, recent events)
/// - 1-hop Knowledge Graph edges around the scene
/// - Keyword-triggered + always-on + FTS5-matched World-Building & Lore entries
/// - Active Progress Clocks / Quests
pub fn assemble_scene_context(
    db: &CampaignDb,
    req: &SceneContextRequest,
) -> Result<SceneContextPacket> {
    let meta = db.get_campaign_meta()?;
    let all_chars = load_all_character_projections(db, None)?;
    let all_places = load_all_place_projections(db, None)?;

    let raw_persp = req
        .perspective
        .as_deref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or("gm");
    let is_gm_perspective = raw_persp.eq_ignore_ascii_case("gm");
    let pov_actor_id: Option<String> = if is_gm_perspective || raw_persp.eq_ignore_ascii_case("player") {
        None
    } else {
        Some(resolve_character_id(db, raw_persp).unwrap_or_else(|_| crate::models::slugify(raw_persp)))
    };
    let effective_include_secrets = is_gm_perspective && req.include_gm_secrets;
    let perspective_label = if is_gm_perspective {
        "gm".to_string()
    } else if let Some(ref aid) = pov_actor_id {
        aid.clone()
    } else {
        "player".to_string()
    };

    // Preload secret entity IDs so even legacy events whose `events.visibility` column
    // defaulted to "public" cannot leak GM-secret lore, histories, attachments, claims, or relations
    // into non-GM recent_events or recent_campaign_events!
    let secret_lore_ids: BTreeSet<String> = if effective_include_secrets {
        BTreeSet::new()
    } else {
        db.list_lore(None)?
            .into_iter()
            .filter(|l| l.visibility == "gm_secret")
            .map(|l| l.id)
            .collect()
    };
    let secret_history_ids: BTreeSet<i64> = if effective_include_secrets {
        BTreeSet::new()
    } else {
        db.list_all_histories()?
            .into_iter()
            .filter(|h| h.visibility == "gm_secret")
            .map(|h| h.id)
            .collect()
    };
    let secret_claim_ids: BTreeSet<String> = if effective_include_secrets {
        BTreeSet::new()
    } else {
        db.list_knowledge_claims()?
            .into_iter()
            .filter(|c| c.visibility == "gm_secret")
            .map(|c| c.id)
            .collect()
    };
    let secret_attachment_ids: BTreeSet<String> = if effective_include_secrets {
        BTreeSet::new()
    } else {
        db.list_attachments()?
            .into_iter()
            .filter(|a| a.visibility == "gm_secret")
            .map(|a| a.id)
            .collect()
    };
    let event_visible = |ev: &WorldEvent| -> bool {
        if effective_include_secrets {
            return true;
        }
        if ev.visibility == "gm_secret" {
            return false;
        }
        match &ev.payload {
            crate::models::EventPayload::RelationUpserted { relation } => {
                relation.visibility != "gm_secret" && relation.dimensions.is_empty()
            }
            crate::models::EventPayload::LoreRecorded { lore_id, .. } => {
                !secret_lore_ids.contains(lore_id)
            }
            crate::models::EventPayload::EntityHistoryAdded { history_id, .. } => {
                !secret_history_ids.contains(history_id)
            }
            crate::models::EventPayload::KnowledgeRecorded { claim_id, .. } => {
                !secret_claim_ids.contains(claim_id)
            }
            crate::models::EventPayload::AttachmentRecorded { attachment_id, .. } => {
                !secret_attachment_ids.contains(attachment_id)
            }
            _ => true,
        }
    };

    // Resolve fuzzy character & place identifiers cleanly
    let resolved_char_inputs: Vec<String> = req
        .character_ids
        .iter()
        .map(|cid| resolve_character_id(db, cid).unwrap_or_else(|_| cid.clone()))
        .collect();

    // Determine active place
    let resolved_place_id = req
        .place_id
        .as_deref()
        .map(|pid| resolve_place_id(db, pid).unwrap_or_else(|_| pid.to_string()))
        .or_else(|| {
            resolved_char_inputs.first().and_then(|cid| {
                all_chars
                    .get(cid)
                    .and_then(|cp| cp.current.current_location.clone())
            })
        });

    let mut focus_entities: BTreeSet<String> = BTreeSet::new();
    let mut trigger_haystack = req.scene_text.to_lowercase();

    let active_place = if let Some(ref pid) = resolved_place_id {
        if let Some(pp) = all_places.get(pid) {
            focus_entities.insert(pid.clone());
            trigger_haystack.push(' ');
            trigger_haystack.push_str(&pp.baseline.name.to_lowercase());
            trigger_haystack.push(' ');
            trigger_haystack.push_str(&pp.baseline.terrain.to_lowercase());
            for tag in &pp.baseline.tags {
                trigger_haystack.push(' ');
                trigger_haystack.push_str(&tag.to_lowercase());
            }
            for lm in &pp.current.landmarks {
                trigger_haystack.push(' ');
                trigger_haystack.push_str(&lm.name.to_lowercase());
            }

            let active_seasonal_note = pp
                .baseline
                .seasonal_profiles
                .iter()
                .find(|(s, _)| s.eq_ignore_ascii_case(&meta.clock.season))
                .map(|(_, desc)| desc.clone());

            let mut recent: Vec<String> = pp
                .event_log
                .iter()
                .rev()
                .filter(|e| event_visible(e))
                .take(5)
                .map(format_event_oneliner)
                .collect();
            recent.reverse();

            Some(PlaceBrief {
                id: pp.baseline.id.clone(),
                name: pp.baseline.name.clone(),
                place_type: pp.baseline.place_type.clone(),
                terrain: pp.baseline.terrain.clone(),
                climate: pp.baseline.climate.clone(),
                active_seasonal_note,
                current: pp.current.clone(),
                recent_events: recent,
            })
        } else {
            None
        }
    } else {
        None
    };

    // Gather characters: explicitly requested + everyone currently at active_place
    let mut char_ids: BTreeSet<String> = resolved_char_inputs.iter().cloned().collect();
    if let Some(ref pb) = active_place {
        for cid in &pb.current.present_characters {
            char_ids.insert(cid.clone());
        }
    }
    // If neither place nor characters specified, include all PCs as default focus
    if char_ids.is_empty() && active_place.is_none() {
        for (cid, cp) in &all_chars {
            if cp.baseline.kind.eq_ignore_ascii_case("pc") {
                char_ids.insert(cid.clone());
            }
        }
    }

    let mut characters = Vec::new();
    for cid in &char_ids {
        if let Some(cp) = all_chars.get(cid) {
            focus_entities.insert(cid.clone());
            trigger_haystack.push(' ');
            trigger_haystack.push_str(&cp.baseline.name.to_lowercase());
            trigger_haystack.push(' ');
            trigger_haystack.push_str(&cp.baseline.faction.to_lowercase());
            for tag in &cp.baseline.tags {
                trigger_haystack.push(' ');
                trigger_haystack.push_str(&tag.to_lowercase());
            }

            let mut recent: Vec<String> = cp
                .event_log
                .iter()
                .rev()
                .filter(|e| event_visible(e))
                .take(5)
                .map(format_event_oneliner)
                .collect();
            recent.reverse();

            let mut progressions = db.list_character_progressions(cid).unwrap_or_default();
            if !effective_include_secrets {
                for tr in &mut progressions {
                    tr.gm_notes.clear();
                }
            }

            let prof_opt = db.get_character_profile(cid).ok().flatten();
            let show_self_perceived = effective_include_secrets
                || (raw_persp.eq_ignore_ascii_case("player") && cp.baseline.kind.eq_ignore_ascii_case("pc"))
                || pov_actor_id.as_deref() == Some(cid.as_str());
            let (
                active_titles,
                avatar_brief,
                apparel_fit_brief,
                perceived_change_note,
                avatar_delta_brief,
                transformation_trajectory,
                active_jobs,
                key_proficiencies,
                active_traits,
            ) = if let Some(ref prof) = prof_opt {
                let titles: Vec<String> = prof
                    .titles
                    .iter()
                    .filter(|t| t.equipped && (effective_include_secrets || t.visibility != "gm_secret"))
                    .map(|t| {
                        if t.mechanical_effect.is_empty() {
                            t.name.clone()
                        } else {
                            format!("{} ({})", t.name, t.mechanical_effect)
                        }
                    })
                    .collect();
                let av_brief = prof.avatar.format_observable_brief();
                let app_fit = crate::autonomy::evaluate_equipped_apparel_fit(
                    &cp.current.equipment,
                    &prof.avatar,
                )
                .brief;
                let self_change = if show_self_perceived {
                    prof.avatar.perceived_change_note.trim().to_string()
                } else {
                    String::new()
                };
                let av_delta = if effective_include_secrets {
                    prof.avatar.format_delta_brief()
                } else {
                    String::new()
                };
                let av_traj = if effective_include_secrets {
                    prof.avatar.transformation_trajectory.trim().to_string()
                } else {
                    String::new()
                };
                let jobs: Vec<String> = prof
                    .jobs
                    .iter()
                    .filter(|j| j.active)
                    .map(|j| {
                        format!(
                            "{} [{} Lv {} ({}/{} XP)]",
                            j.name, j.role, j.level, j.current_xp, j.xp_to_next
                        )
                    })
                    .collect();
                let profs: Vec<String> = prof
                    .proficiencies
                    .iter()
                    .map(|p| {
                        format!(
                            "{} ({}{})",
                            p.name,
                            p.rank,
                            if p.bonus.is_empty() {
                                String::new()
                            } else {
                                format!(" {}", p.bonus)
                            }
                        )
                    })
                    .collect();
                let traits: Vec<String> = prof
                    .traits_and_perks
                    .iter()
                    .filter(|t| t.active)
                    .map(|t| {
                        if t.rank_or_grade.is_empty() {
                            format!("{} [{}]", t.name, t.category)
                        } else {
                            format!("{} [{} • {}]", t.name, t.category, t.rank_or_grade)
                        }
                    })
                    .collect();
                (
                    titles,
                    av_brief,
                    app_fit,
                    self_change,
                    av_delta,
                    av_traj,
                    jobs,
                    profs,
                    traits,
                )
            } else {
                (
                    vec![],
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    vec![],
                    vec![],
                    vec![],
                )
            };

            characters.push(CharacterBrief {
                id: cp.baseline.id.clone(),
                name: cp.baseline.name.clone(),
                kind: cp.baseline.kind.clone(),
                archetype: cp.baseline.archetype.clone(),
                current: cp.current.clone(),
                progression_tracks: progressions,
                active_titles,
                avatar_brief,
                apparel_fit_brief,
                perceived_change_note,
                avatar_delta_brief,
                transformation_trajectory,
                active_jobs,
                key_proficiencies,
                active_traits,
                recent_events: recent,
            });
        }
    }

    // 1-hop Knowledge Graph edges around all focused entities
    let mut seen_edges: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut knowledge_graph_edges = Vec::new();
    let mut gm_only_secrets: Vec<String> = Vec::new();
    for eid in &focus_entities {
        for mut rel in db.list_relations_for_entity(eid)? {
            let key = (
                rel.source_id.clone(),
                rel.relation_type.clone(),
                rel.target_id.clone(),
            );
            if seen_edges.insert(key) {
                if !effective_include_secrets {
                    if rel.visibility == "gm_secret" {
                        // Hide completely secret edges from player/NPC POV
                        continue;
                    }
                    // Strip hidden multi-dimensional scores from non-GM POV unless explicitly public
                    if !rel.dimensions.is_empty() {
                        rel.dimensions = RelationshipDimensions::default();
                    }
                } else if !rel.dimensions.is_empty() || rel.visibility == "gm_secret" {
                    let dim_txt = rel.dimensions.format_compact();
                    gm_only_secrets.push(format!(
                        "Hidden relation: {} -[{}]-> {} [{}]{}",
                        rel.source_id,
                        rel.relation_type,
                        rel.target_id,
                        rel.visibility,
                        if dim_txt.is_empty() {
                            String::new()
                        } else {
                            format!(" ({dim_txt})")
                        }
                    ));
                }
                knowledge_graph_edges.push(rel);
            }
        }
    }

    // Dynamic Lorebook Retrieval:
    // 1. Always-include entries
    // 2. Direct cross-reference to focus entities
    // 3. Keyword trigger matches against scene_text + active place/character names
    // 4. FTS5 BM25 hits against scene_text
    let all_lore = db.list_lore(None)?;
    let mut matched_lore: BTreeMap<String, (LoreEntry, Vec<String>)> = BTreeMap::new();

    for entry in all_lore {
        if !effective_include_secrets && entry.visibility == "gm_secret" {
            continue;
        }
        let mut reasons = Vec::new();
        if entry.always_include {
            reasons.push("always_include".to_string());
        }
        for rid in &entry.related_ids {
            if focus_entities.contains(rid) {
                reasons.push(format!("linked_to:{rid}"));
            }
        }
        for kw in &entry.keywords {
            let k = kw.trim().to_lowercase();
            if !k.is_empty() && trigger_haystack.contains(&k) {
                reasons.push(format!("keyword:{kw}"));
            }
        }
        if !reasons.is_empty() {
            matched_lore.insert(entry.id.clone(), (entry, reasons));
        }
    }

    if !req.scene_text.trim().is_empty() {
        for hit in db.search_lore_fts(&req.scene_text, 6)? {
            if !effective_include_secrets && hit.visibility == "gm_secret" {
                continue;
            }
            matched_lore
                .entry(hit.id.clone())
                .and_modify(|(_, reasons)| reasons.push("fts5_match".to_string()))
                .or_insert_with(|| (hit, vec!["fts5_match".to_string()]));
        }
    }

    let mut triggered_lore: Vec<TriggeredLore> = matched_lore
        .into_values()
        .map(|(l, reasons)| TriggeredLore {
            id: l.id,
            title: l.title,
            category: l.category,
            visibility: l.visibility,
            trigger_reason: reasons.join(", "),
            summary: l.summary,
            body: l.body,
        })
        .collect();
    // High-priority downstream ordering: active GM secrets (always_include or entity-linked) surface FIRST in GM POV!
    triggered_lore.sort_by(|a, b| {
        let rank = |l: &TriggeredLore| -> u8 {
            let is_sec = l.visibility == "gm_secret";
            let is_always_or_linked = l.trigger_reason.contains("always_include")
                || l.trigger_reason.contains("linked_to:");
            match (is_sec, is_always_or_linked) {
                (true, true) => 0,
                (true, false) => 1,
                (false, true) => 2,
                (false, false) => 3,
            }
        };
        rank(a).cmp(&rank(b)).then_with(|| a.id.cmp(&b.id))
    });

    // Relevant Books, Scrolls & Story Texts:
    // Surface texts located at active place, held by present characters, linked to focus entities,
    // or triggered by keywords/FTS5 against scene_text.
    let all_texts = db.list_texts()?;
    let mut matched_texts: BTreeMap<String, (StoryText, Vec<String>)> = BTreeMap::new();
    for t in all_texts {
        let mut reasons = Vec::new();
        if let Some(ref h) = t.holder_or_location_id {
            if focus_entities.contains(h) {
                reasons.push(format!("present_at_or_held_by:{h}"));
            }
        }
        for rid in &t.related_ids {
            if focus_entities.contains(rid) {
                reasons.push(format!("linked_to:{rid}"));
            }
        }
        for kw in &t.keywords {
            let k = kw.trim().to_lowercase();
            if !k.is_empty() && trigger_haystack.contains(&k) {
                reasons.push(format!("keyword:{kw}"));
            }
        }
        if !reasons.is_empty() {
            matched_texts.insert(t.id.clone(), (t, reasons));
        }
    }
    if !req.scene_text.trim().is_empty() {
        for hit in db.search_texts_fts(&req.scene_text, 6)? {
            matched_texts
                .entry(hit.id.clone())
                .and_modify(|(_, reasons)| reasons.push("fts5_match".to_string()))
                .or_insert_with(|| (hit, vec!["fts5_match".to_string()]));
        }
    }

    let relevant_texts: Vec<TriggeredText> = matched_texts
        .into_values()
        .map(|(t, reasons)| TriggeredText {
            id: t.id,
            title: t.title,
            medium: t.medium,
            subject: t.subject,
            language: t.language,
            condition: t.condition,
            read_status: t.read_status,
            read_by: t.read_by,
            holder_or_location_id: t.holder_or_location_id,
            trigger_reason: reasons.join(", "),
            summary: t.summary,
            content: t.content,
            mechanics_or_unlocks: t.mechanics_or_unlocks,
            gm_notes: if effective_include_secrets {
                t.gm_notes
            } else {
                String::new()
            },
        })
        .collect();

    let active_clocks: Vec<ProgressClock> = db
        .list_clocks()?
        .into_iter()
        .filter(|c| c.status.eq_ignore_ascii_case("active"))
        .collect();

    // Relevant Attached Reference Docs & Media
    let all_atts = db.list_attachments()?;
    let mut matched_atts: BTreeMap<String, (StoryAttachment, Vec<String>)> = BTreeMap::new();
    for a in all_atts {
        if !effective_include_secrets && a.visibility == "gm_secret" {
            continue;
        }
        let mut reasons = Vec::new();
        for rid in &a.related_ids {
            if focus_entities.contains(rid) {
                reasons.push(format!("linked_to:{rid}"));
            }
        }
        for kw in &a.keywords {
            let k = kw.trim().to_lowercase();
            if !k.is_empty() && trigger_haystack.contains(&k) {
                reasons.push(format!("keyword:{kw}"));
            }
        }
        if !reasons.is_empty() {
            matched_atts.insert(a.id.clone(), (a, reasons));
        }
    }
    if !req.scene_text.trim().is_empty() {
        for hit in db.search_attachments_fts(&req.scene_text, 6)? {
            if !effective_include_secrets && hit.visibility == "gm_secret" {
                continue;
            }
            matched_atts
                .entry(hit.id.clone())
                .and_modify(|(_, reasons)| reasons.push("fts5_match".to_string()))
                .or_insert_with(|| (hit, vec!["fts5_match".to_string()]));
        }
    }

    let relevant_attachments: Vec<TriggeredAttachment> = matched_atts
        .into_values()
        .map(|(a, reasons)| TriggeredAttachment {
            mcp_uri: format!("chronicle://attachments/{}", a.id),
            id: a.id,
            title: a.title,
            category: a.category,
            mime_type: a.mime_type,
            relative_path: a.relative_path,
            visibility: a.visibility,
            trigger_reason: reasons.join(", "),
            summary: a.summary,
            extracted_notes: a.extracted_notes,
        })
        .collect();

    // Relevant Cataloged Carryable Items (held by present chars, at active place, linked, or matched)
    let all_items = db.list_catalog_items()?;
    let mut matched_items: BTreeMap<String, (CatalogItem, Vec<String>)> = BTreeMap::new();
    for it in all_items {
        let mut reasons = Vec::new();
        if let Some(ref h) = it.current_holder_id {
            if focus_entities.contains(h) {
                reasons.push(format!("held_by_or_at:{h}"));
            }
        }
        for rid in &it.related_ids {
            if focus_entities.contains(rid) {
                reasons.push(format!("linked_to:{rid}"));
            }
        }
        for kw in &it.keywords {
            let k = kw.trim().to_lowercase();
            if !k.is_empty() && trigger_haystack.contains(&k) {
                reasons.push(format!("keyword:{kw}"));
            }
        }
        if !reasons.is_empty() {
            matched_items.insert(it.id.clone(), (it, reasons));
        }
    }
    if !req.scene_text.trim().is_empty() {
        for hit in db.search_catalog_items_fts(&req.scene_text, 6)? {
            matched_items
                .entry(hit.id.clone())
                .and_modify(|(_, reasons)| reasons.push("fts5_match".to_string()))
                .or_insert_with(|| (hit, vec!["fts5_match".to_string()]));
        }
    }
    let relevant_items: Vec<TriggeredItem> = matched_items
        .into_values()
        .map(|(it, reasons)| TriggeredItem {
            id: it.id,
            name: it.name,
            item_type: it.item_type,
            condition_or_durability: it.condition_or_durability,
            current_holder_id: it.current_holder_id,
            previous_owners: it.previous_owners,
            significance: it.significance,
            properties_and_effects: it.properties_and_effects,
            linked_text_id: it.linked_text_id,
            gm_notes: if effective_include_secrets {
                it.gm_notes
            } else {
                String::new()
            },
            trigger_reason: reasons.join(", "),
        })
        .collect();

    // Relevant Historical Chronicles for active place, present chars, triggered lore/items/texts
    let mut history_entities = focus_entities.clone();
    for l in &triggered_lore {
        history_entities.insert(l.id.clone());
    }
    for it in &relevant_items {
        history_entities.insert(it.id.clone());
    }
    let mut seen_hist_ids: BTreeSet<i64> = BTreeSet::new();
    let mut relevant_histories: Vec<EntityHistoryEntry> = Vec::new();
    for eid in &history_entities {
        for h in db.list_histories_for_entity(eid)? {
            if !effective_include_secrets && h.visibility == "gm_secret" {
                continue;
            }
            if seen_hist_ids.insert(h.id) {
                relevant_histories.push(h);
            }
        }
    }

    let active_encounters = db.list_encounters(Some("active"))?;
    let pending_discoveries: Vec<StagedDiscovery> = db
        .list_staged_discoveries(None)?
        .into_iter()
        .filter(|d| {
            (d.status == "pending" || d.status == "partially_claimed")
                && (d.location_id.is_none()
                    || d.location_id
                        .as_deref()
                        .is_some_and(|l| focus_entities.contains(l)))
        })
        .collect();

    // Epistemic Knowledge & Blind Spots (POV-enforced!)
    let all_claims = db.list_knowledge_claims()?;
    let present_char_ids: Vec<String> = characters.iter().map(|c| c.id.clone()).collect();
    let knowledge_actor_scope: Vec<String> = if let Some(ref pov_cid) = pov_actor_id {
        vec![pov_cid.clone()]
    } else if !is_gm_perspective {
        // "player" perspective: only requested character_ids (PCs)
        resolved_char_inputs.clone()
    } else {
        present_char_ids.clone()
    };

    let mut character_knowledge: Vec<ActorKnowledgeBrief> = Vec::new();
    let mut actor_known_map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for aid in &knowledge_actor_scope {
        let mut known_set = BTreeSet::new();
        for b in db.list_beliefs_for_actor(aid)? {
            if let Some(cl) = all_claims.iter().find(|c| c.id == b.claim_id) {
                known_set.insert(cl.id.clone());
                character_knowledge.push(ActorKnowledgeBrief {
                    claim_id: cl.id.clone(),
                    actor_id: aid.clone(),
                    subject: cl.subject.clone(),
                    perceived_claim: if b.perceived_claim.is_empty() {
                        cl.claim.clone()
                    } else {
                        b.perceived_claim.clone()
                    },
                    stance: b.stance,
                    confidence: b.confidence,
                    source: b.source,
                    concealing: b.concealing,
                    outdated: b.outdated || cl.superseded_by_id.is_some(),
                    learned_at: b.learned_at_world_time,
                });
            }
        }
        actor_known_map.insert(aid.clone(), known_set);
    }

    // Blind spots & GM-only secrets (ONLY populated for GM perspective!)
    let mut npc_blind_spots: Vec<BlindSpotNotice> = Vec::new();
    if effective_include_secrets {
        for cl in &all_claims {
            if cl.superseded_by_id.is_some() {
                continue;
            }
            let relevant_to_scene = focus_entities.contains(&crate::models::slugify(&cl.subject))
                || cl.related_ids.iter().any(|r| focus_entities.contains(r))
                || (!trigger_haystack.is_empty()
                    && trigger_haystack.contains(&cl.subject.to_lowercase()));
            if cl.visibility == "gm_secret" && relevant_to_scene {
                gm_only_secrets.push(format!(
                    "Secret Claim (`{}` | subject: {} | {}): {}",
                    cl.id, cl.subject, cl.truth_status, cl.claim
                ));
            }
            if relevant_to_scene {
                for aid in &present_char_ids {
                    let knows = actor_known_map
                        .get(aid)
                        .is_some_and(|s| s.contains(&cl.id));
                    if !knows {
                        npc_blind_spots.push(BlindSpotNotice {
                            actor_id: aid.clone(),
                            unknown_claim_id: cl.id.clone(),
                            subject: cl.subject.clone(),
                            note: format!(
                                "`{aid}` does NOT know `{}` ({})",
                                cl.id, cl.subject
                            ),
                        });
                    }
                }
            }
        }
        // Surface character private secrets only in GM eyes-only block
        for cid in &present_char_ids {
            if let Ok(Some(ext)) = db.get_character_profile(cid) {
                for sec in ext.secrets {
                    gm_only_secrets.push(format!("Character secret (`{cid}`): {sec}"));
                }
            }
        }
    }

    // Present Factions (territory matches active place OR contains present characters)
    let present_factions: Vec<FactionRecord> = db
        .list_factions()?
        .into_iter()
        .filter(|f| {
            focus_entities.contains(&f.id)
                || f.territory.iter().any(|t| focus_entities.contains(t))
                || f.members
                    .iter()
                    .any(|m| present_char_ids.contains(&m.character_id))
        })
        .map(|mut f| {
            if !effective_include_secrets {
                f.active_schemes.clear();
                f.internal_conflicts.clear();
            }
            f
        })
        .collect();

    let recent_campaign_events: Vec<String> = db
        .list_recent_events(req.recent_events_limit)?
        .iter()
        .filter(|e| event_visible(e))
        .map(format_event_oneliner)
        .collect();

    // Gather prominent figures currently in transit, visiting away from home, or stranded
    let mut prominent_figure_movements: Vec<String> = Vec::new();
    for (cid, cp) in &all_chars {
        if cp.baseline.kind.eq_ignore_ascii_case("pc") {
            continue;
        }
        if let Ok(Some(prof)) = db.get_character_profile(cid) {
            let m = &prof.mobility;
            if m.current_status == "in_transit" {
                if let Some(ref tr) = m.active_trip {
                    let detour = if tr.detour_note.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", tr.detour_note)
                    };
                    prominent_figure_movements.push(format!(
                        "**{}** (`{cid}`) — IN TRANSIT `{}` -> `{}` ({}, ETA {}){detour}",
                        cp.baseline.name, tr.origin_place_id, tr.destination_place_id, tr.purpose, tr.eta_world_time
                    ));
                }
            } else if m.current_status == "visiting" {
                if let Some(ref tr) = m.active_trip {
                    prominent_figure_movements.push(format!(
                        "**{}** (`{cid}`) — VISITING `{}` for {} (home: `{}`; departing after {})",
                        cp.baseline.name,
                        tr.destination_place_id,
                        tr.purpose,
                        m.home_place_id.as_deref().unwrap_or("unknown"),
                        tr.return_after_world_time
                    ));
                }
            } else if m.current_status == "stranded" {
                let loc = cp.current.current_location.as_deref().unwrap_or("unknown");
                let note = m
                    .active_trip
                    .as_ref()
                    .map(|t| t.detour_note.as_str())
                    .unwrap_or("Route severed");
                prominent_figure_movements.push(format!(
                    "**{}** (`{cid}`) — STRANDED at `{loc}` ({note})",
                    cp.baseline.name
                ));
            }
        }
    }

    // Compact, token-safe 3x3 compass neighborhood if the active place is anchored on a 2D MapGrid
    let mut local_map_grid_neighborhood: Vec<String> = Vec::new();
    if let Some(ref ap) = active_place {
        if let Ok(anchor_cells) = db.find_map_cells_for_place(&ap.id) {
            for ac in anchor_cells.into_iter().take(2) {
                if let (Ok(Some(grid)), Ok(all_cells)) =
                    (db.get_map_grid(&ac.grid_id), db.list_map_cells(&ac.grid_id))
                {
                    let lines = crate::world_sim::format_grid_neighborhood_lines(
                        &grid,
                        &all_cells,
                        ac.x,
                        ac.y,
                        effective_include_secrets,
                    );
                    local_map_grid_neighborhood.extend(lines);
                }
            }
        }
    }

    // Evaluate & perspective-filter active Engine Signals / Alarms
    let pc_ids: BTreeSet<String> = all_chars
        .values()
        .filter(|c| c.baseline.kind.eq_ignore_ascii_case("pc"))
        .map(|c| c.baseline.id.clone())
        .collect();
    let raw_signals = evaluate_live_signals(
        db,
        &all_chars,
        &all_places,
        Some(&focus_entities),
        active_place.as_ref().map(|p| p.id.as_str()),
        &meta.clock.format_short(),
    )
    .unwrap_or_default();
    let active_signals = filter_signals_for_perspective(
        raw_signals,
        raw_persp,
        pov_actor_id.as_deref(),
        &pc_ids,
        effective_include_secrets,
    );

    Ok(SceneContextPacket {
        campaign_name: meta.name,
        session_number: meta.current_session,
        time_of_day: meta.clock.time_of_day().to_string(),
        world_clock: meta.clock,
        perspective: perspective_label,
        active_signals,
        active_place,
        characters,
        knowledge_graph_edges,
        triggered_lore,
        relevant_texts,
        relevant_items,
        relevant_histories,
        relevant_attachments,
        active_clocks,
        active_encounters,
        pending_discoveries,
        character_knowledge,
        npc_blind_spots,
        prominent_figure_movements,
        local_map_grid_neighborhood,
        present_factions,
        gm_only_secrets,
        recent_campaign_events,
    })
}

fn format_event_oneliner(ev: &WorldEvent) -> String {
    let loc = ev
        .location_id
        .as_deref()
        .map(|l| format!(" @ {l}"))
        .unwrap_or_default();
    format!("[#{} | {}{}] {}", ev.seq, ev.world_time, loc, ev.summary)
}

/// Renders a SceneContextPacket as compact, LLM-optimized Markdown (~40% fewer tokens than JSON)
/// with optional `max_chars` token-budget safety truncation.
pub fn render_scene_context_markdown(
    pkt: &SceneContextPacket,
    max_chars: Option<usize>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# GM Scene Brief: {} (Session {})",
        pkt.campaign_name, pkt.session_number
    );
    let _ = writeln!(
        out,
        "**World Clock:** {} ({}) | **Season:** {} | **Global Weather:** {}",
        pkt.world_clock.format_short(),
        pkt.time_of_day,
        pkt.world_clock.season,
        pkt.world_clock.global_weather
    );
    if let Some(last_ev) = pkt.recent_campaign_events.last() {
        let _ = writeln!(out, "**Last Committed Ledger Event:** {last_ev}");
    }
    let _ = writeln!(out);

    if !pkt.active_signals.is_empty() {
        out.push_str(&render_signals_markdown(&pkt.active_signals));
    }

    if let Some(ref p) = pkt.active_place {
        let _ = writeln!(out, "## Active Place: {} (`{}`)", p.name, p.id);
        let _ = writeln!(
            out,
            "- **Type:** {} | **Terrain:** {} | **Climate:** {}",
            p.place_type, p.terrain, p.climate
        );
        let _ = writeln!(
            out,
            "- **Weather:** {} | **Danger:** {} | **Control:** {}",
            p.current.current_weather,
            p.current.danger_level,
            p.current
                .controlling_faction
                .as_deref()
                .unwrap_or("Unclaimed")
        );
        if let Some(ref sn) = p.active_seasonal_note {
            let _ = writeln!(out, "- **Season Effect ({}):** {}", p.current.current_season, sn);
        }
        if !p.current.active_conditions.is_empty() {
            let _ = writeln!(
                out,
                "- **Active Local Conditions:** {}",
                p.current.active_conditions.join(", ")
            );
        }
        if !p.current.landmarks.is_empty() {
            let lms: Vec<String> = p
                .current
                .landmarks
                .iter()
                .map(|l| format!("{} [{}]", l.name, l.status))
                .collect();
            let _ = writeln!(out, "- **Landmarks:** {}", lms.join("; "));
        }
        if !pkt.local_map_grid_neighborhood.is_empty() {
            for line in &pkt.local_map_grid_neighborhood {
                let _ = writeln!(out, "{line}");
            }
        }
        if !p.current.routes.is_empty() {
            let routes: Vec<String> = p
                .current
                .routes
                .iter()
                .map(|r| format!("-> `{}` ({}, {})", r.destination_id, r.travel_time, r.status))
                .collect();
            let _ = writeln!(out, "- **Exits:** {}", routes.join("; "));
        }
        let _ = writeln!(out);
    }

    if !pkt.characters.is_empty() {
        let _ = writeln!(out, "## Characters in Scene ({})", pkt.characters.len());
        for ch in &pkt.characters {
            let res: Vec<String> = ch
                .current
                .resources
                .iter()
                .map(|(k, v)| format!("{k} {}/{}", v.current, v.max))
                .collect();
            let stats: Vec<String> = ch
                .current
                .stats
                .iter()
                .map(|(k, v)| format!("{k}:{v}"))
                .collect();
            let conds: Vec<String> = ch
                .current
                .conditions
                .iter()
                .map(|c| format!("{} ({})", c.name, c.kind))
                .collect();
            let eq: Vec<String> = ch
                .current
                .equipment
                .iter()
                .filter(|i| i.equipped)
                .map(|i| i.name.clone())
                .collect();
            let bag: Vec<String> = ch
                .current
                .equipment
                .iter()
                .filter(|i| !i.equipped)
                .map(|i| format!("{} x{}", i.name, i.quantity))
                .collect();
            let techs: Vec<String> = ch
                .current
                .techniques
                .iter()
                .map(|t| {
                    let mut extra = Vec::new();
                    if let Some(ref st) = t.source_text_id {
                        let lbl = if !t.manual_title.is_empty() {
                            format!("Manual: {} (text:{st})", t.manual_title)
                        } else {
                            format!("Manual: text:{st}")
                        };
                        extra.push(lbl);
                    } else if !t.manual_title.is_empty() {
                        extra.push(format!("Manual: {}", t.manual_title));
                    }
                    if !t.manual_notes.is_empty() {
                        extra.push(format!("Notes: {}", t.manual_notes));
                    }
                    if extra.is_empty() {
                        format!("{} ({}, {})", t.name, t.cost, t.mastery)
                    } else {
                        format!("{} ({}, {}; {})", t.name, t.cost, t.mastery, extra.join(" | "))
                    }
                })
                .collect();
            let skills: Vec<String> = ch
                .current
                .skills
                .iter()
                .map(|s| {
                    format!(
                        "{} {}{}",
                        s.name,
                        s.rank,
                        s.modifier
                            .as_deref()
                            .map(|m| format!(" {m}"))
                            .unwrap_or_default()
                    )
                })
                .collect();

            let _ = writeln!(
                out,
                "### {} (`{}` | {} | {} | {})",
                ch.name, ch.id, ch.kind, ch.archetype, ch.current.alive_status
            );
            if !ch.active_titles.is_empty() {
                let _ = writeln!(out, "- **Active Titles:** {}", ch.active_titles.join(" | "));
            }
            if !ch.avatar_brief.is_empty() {
                let _ = writeln!(out, "- **Observable Appearance (Present-Tense):** {}", ch.avatar_brief);
            }
            if !ch.apparel_fit_brief.is_empty() {
                let _ = writeln!(out, "- **Equipped Apparel Fit:** {}", ch.apparel_fit_brief);
            }
            if !ch.perceived_change_note.is_empty() {
                let _ = writeln!(out, "- **Self-Perceived Ongoing Change:** {}", ch.perceived_change_note);
            }
            if !ch.avatar_delta_brief.is_empty() {
                let _ = writeln!(
                    out,
                    "- **Physical Delta vs. Baseline (GM / Returning-NPC Ref):** {}",
                    ch.avatar_delta_brief
                );
            }
            if !ch.transformation_trajectory.is_empty() {
                let _ = writeln!(
                    out,
                    "- **Active Transformation Trajectory (GM Only — Ongoing Drift & Next Trigger):** {}",
                    ch.transformation_trajectory
                );
            }
            if !ch.active_traits.is_empty() {
                let _ = writeln!(out, "- **Traits & Physique:** {}", ch.active_traits.join("; "));
            }
            if !res.is_empty() {
                let _ = writeln!(out, "- **Vitals:** {}", res.join(" | "));
            }
            if !ch.active_jobs.is_empty() {
                let _ = writeln!(out, "- **Jobs / Classes:** {}", ch.active_jobs.join(" | "));
            }
            if !ch.progression_tracks.is_empty() {
                let tracks: Vec<String> = ch
                    .progression_tracks
                    .iter()
                    .map(|tr| {
                        let bn = if tr.at_bottleneck { " [BOTTLENECK]" } else { "" };
                        let manual = if let Some(ref st) = tr.source_text_id {
                            if !tr.manual_title.is_empty() {
                                format!(" [Manual: {} (text:{st})]", tr.manual_title)
                            } else {
                                format!(" [Manual: text:{st}]")
                            }
                        } else if !tr.manual_title.is_empty() {
                            format!(" [Manual: {}]", tr.manual_title)
                        } else {
                            String::new()
                        };
                        let mnotes = if !tr.manual_notes.is_empty() {
                            format!(" [Manual Notes: {}]", tr.manual_notes)
                        } else {
                            String::new()
                        };
                        let gm = if !tr.gm_notes.is_empty() {
                            format!(" *(GM: {})*", tr.gm_notes)
                        } else {
                            String::new()
                        };
                        format!(
                            "{}: {} [{}/{} XP, total {}, {:+}]{}{}{}{}",
                            tr.name,
                            tr.format_stage(),
                            tr.current_xp,
                            tr.xp_to_next,
                            tr.cumulative_xp,
                            tr.check_modifier,
                            bn,
                            manual,
                            mnotes,
                            gm
                        )
                    })
                    .collect();
                let _ = writeln!(out, "- **Progression:** {}", tracks.join(" | "));
            }
            if !stats.is_empty() {
                let _ = writeln!(out, "- **Stats:** {}", stats.join(", "));
            }
            if !conds.is_empty() {
                let _ = writeln!(out, "- **Conditions:** {}", conds.join(", "));
            }
            if !eq.is_empty() {
                let _ = writeln!(out, "- **Equipped:** {}", eq.join(", "));
            }
            if !bag.is_empty() {
                if bag.len() > 6 {
                    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
                    for item in ch.current.equipment.iter().filter(|i| !i.equipped) {
                        let grp = crate::models::primary_item_group(
                            &item.tags,
                            item.slot.as_deref(),
                            &item.name,
                        );
                        let label = if item.quantity > 1 {
                            format!("{} x{}", item.name, item.quantity)
                        } else {
                            item.name.clone()
                        };
                        grouped.entry(grp).or_default().push(label);
                    }
                    let chunks: Vec<String> = grouped
                        .into_iter()
                        .map(|(g, list)| format!("[{g}] {}", list.join(", ")))
                        .collect();
                    let _ = writeln!(out, "- **Carried (by tag):** {}", chunks.join(" | "));
                } else {
                    let _ = writeln!(out, "- **Carried:** {}", bag.join(", "));
                }
            }
            if !skills.is_empty() {
                let _ = writeln!(out, "- **Skills:** {}", skills.join("; "));
            }
            if !ch.key_proficiencies.is_empty() {
                let _ = writeln!(out, "- **Proficiencies:** {}", ch.key_proficiencies.join("; "));
            }
            if !techs.is_empty() {
                let _ = writeln!(out, "- **Techniques:** {}", techs.join("; "));
            }
            if !ch.current.active_goals.is_empty() {
                let _ = writeln!(out, "- **Goals:** {}", ch.current.active_goals.join(" | "));
            }
            let _ = writeln!(out);
        }
    }

    // High-priority GM-Only Secrets & Active Hidden Blueprints surfaced immediately after Characters in Scene (GM POV ONLY)
    if pkt.perspective == "gm" {
        let priority_secrets: Vec<&TriggeredLore> = pkt
            .triggered_lore
            .iter()
            .filter(|l| l.visibility == "gm_secret")
            .collect();
        if !priority_secrets.is_empty() {
            let _ = writeln!(
                out,
                "## High-Priority GM Secrets & Active Hidden Blueprints ({})",
                priority_secrets.len()
            );
            let _ = writeln!(
                out,
                "> **GM-ONLY DOWNSTREAM DIRECTIVE:** Adjudicate hidden mechanics, gradual physical trajectories, and NPC motivations using these authoritative secrets without leaking hidden explanations to player POV.\n"
            );
            for l in &priority_secrets {
                let _ = writeln!(
                    out,
                    "### [GM SECRET] {} (`{}` | {} | trigger: {})",
                    l.title, l.id, l.category, l.trigger_reason
                );
                let _ = writeln!(out, "{}\n", l.body);
            }
        }
    }

    if !pkt.knowledge_graph_edges.is_empty() {
        let _ = writeln!(out, "## Knowledge Graph Links");
        for e in &pkt.knowledge_graph_edges {
            let s = e
                .strength
                .map(|v| format!(" ({v:+})"))
                .unwrap_or_default();
            let _ = writeln!(
                out,
                "- `{}` --[{}]{}--> `{}`: {}",
                e.source_id, e.relation_type, s, e.target_id, e.notes
            );
        }
        let _ = writeln!(out);
    }

    let non_secret_lore: Vec<&TriggeredLore> = if pkt.perspective == "gm" {
        pkt.triggered_lore
            .iter()
            .filter(|l| l.visibility != "gm_secret")
            .collect()
    } else {
        pkt.triggered_lore.iter().collect()
    };
    if !non_secret_lore.is_empty() {
        let _ = writeln!(out, "## Triggered Lorebook ({})", non_secret_lore.len());
        for l in non_secret_lore {
            let secret_tag = if l.visibility == "gm_secret" {
                " (visibility: gm_secret)"
            } else if l.visibility == "rumor" {
                " (visibility: rumor)"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "### {} (`{}` | {}{} | trigger: {})",
                l.title, l.id, l.category, secret_tag, l.trigger_reason
            );
            let _ = writeln!(out, "{}\n", l.body);
        }
    }

    if !pkt.relevant_texts.is_empty() {
        let _ = writeln!(
            out,
            "## Relevant Books, Scrolls & Texts ({})",
            pkt.relevant_texts.len()
        );
        for t in &pkt.relevant_texts {
            let _ = writeln!(
                out,
                "### {} (`{}` | {} | {} | lang: {} | status: {} | trigger: {})",
                t.title, t.id, t.medium, t.subject, t.language, t.read_status, t.trigger_reason
            );
            if !t.summary.is_empty() {
                let _ = writeln!(out, "- **Overview:** {}", t.summary);
            }
            if !t.read_by.is_empty() {
                let _ = writeln!(out, "- **Read By:** {}", t.read_by.join(", "));
            }
            if !t.mechanics_or_unlocks.is_empty() {
                let _ = writeln!(
                    out,
                    "- **Unlocks/Rituals:** {}",
                    t.mechanics_or_unlocks.join("; ")
                );
            }
            if !t.gm_notes.is_empty() {
                let _ = writeln!(out, "- **GM Reference Notes:** {}", t.gm_notes);
            }
            let _ = writeln!(out, "{}\n", t.content);
        }
    }

    if !pkt.relevant_items.is_empty() {
        let _ = writeln!(
            out,
            "## Notable Carryable Items & Provenance ({})",
            pkt.relevant_items.len()
        );
        for it in &pkt.relevant_items {
            let holder = it.current_holder_id.as_deref().unwrap_or("unclaimed");
            let prev = if it.previous_owners.is_empty() {
                String::new()
            } else {
                format!(" | prev owners: {}", it.previous_owners.join(" -> "))
            };
            let _ = writeln!(
                out,
                "- **{}** (`{}` | {} | {} | holder: `{}`{} | trigger: {})",
                it.name, it.id, it.item_type, it.condition_or_durability, holder, prev, it.trigger_reason
            );
            if !it.significance.is_empty() {
                let _ = writeln!(out, "  - Significance: {}", it.significance);
            }
            if !it.properties_and_effects.is_empty() {
                let _ = writeln!(out, "  - Properties: {}", it.properties_and_effects);
            }
            if let Some(ref tid) = it.linked_text_id {
                let _ = writeln!(out, "  - Readable Text: `{tid}`");
            }
            if !it.gm_notes.is_empty() {
                let _ = writeln!(out, "  - [GM NOTES]: {}", it.gm_notes);
            }
        }
        let _ = writeln!(out);
    }

    if !pkt.relevant_histories.is_empty() {
        let _ = writeln!(
            out,
            "## Relevant Historical Chronicles ({})",
            pkt.relevant_histories.len()
        );
        for h in &pkt.relevant_histories {
            let sec = if h.visibility == "gm_secret" {
                " [GM SECRET]"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "- `[{} / {}]` **{} — {}**{}: {}",
                h.entity_kind, h.entity_id, h.era_or_date, h.title, sec, h.details
            );
            if !h.significance.is_empty() {
                let _ = writeln!(out, "  - Significance: {}", h.significance);
            }
        }
        let _ = writeln!(out);
    }

    if !pkt.relevant_attachments.is_empty() {
        let _ = writeln!(
            out,
            "## Attached Reference Docs & Media ({})",
            pkt.relevant_attachments.len()
        );
        for a in &pkt.relevant_attachments {
            let sec = if a.visibility == "gm_secret" {
                " [GM SECRET]"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "- **{}** (`{}` | {}{} | `{}` | `{}` | trigger: {})",
                a.title, a.id, a.category, sec, a.relative_path, a.mcp_uri, a.trigger_reason
            );
            if !a.summary.is_empty() {
                let _ = writeln!(out, "  - Summary: {}", a.summary);
            }
            if !a.extracted_notes.is_empty() {
                let _ = writeln!(out, "  - Notes: {}", a.extracted_notes);
            }
        }
        let _ = writeln!(out);
    }

    if !pkt.active_clocks.is_empty() {
        let _ = writeln!(out, "## Active Clocks & Quests");
        for cl in &pkt.active_clocks {
            let _ = writeln!(
                out,
                "- **{}** (`{}` | {}): {}/{} — {}",
                cl.name, cl.id, cl.kind, cl.filled, cl.segments, cl.description
            );
        }
        let _ = writeln!(out);
    }

    if !pkt.active_encounters.is_empty() {
        let _ = writeln!(out, "## Active Encounters");
        for enc in &pkt.active_encounters {
            let active_who = enc
                .active_participant()
                .map(|p| format!("{} ({})", p.name, p.entity_id))
                .unwrap_or_else(|| "none".to_string());
            let _ = writeln!(
                out,
                "- **{}** (`{}` | {} | Round {} | Active Turn: {})",
                enc.title, enc.id, enc.encounter_type, enc.round, active_who
            );
            for p in &enc.participants {
                let _ = writeln!(
                    out,
                    "  - `{}` [{} | Init {} | Pos: {} | React: {} | {}]",
                    p.entity_id,
                    p.side,
                    p.initiative,
                    if p.position.is_empty() { "—" } else { &p.position },
                    if p.reaction_available { "ready" } else { "spent" },
                    p.status
                );
            }
        }
        let _ = writeln!(out);
    }

    if !pkt.pending_discoveries.is_empty() {
        let _ = writeln!(out, "## Unclaimed Staged Discoveries (NOT Yet In Inventory)");
        for disc in &pkt.pending_discoveries {
            let unclaimed: Vec<String> = disc
                .items
                .iter()
                .filter(|it| it.claimed_by.is_none())
                .map(|it| format!("[#{}] {} x{}", it.index, it.name, it.quantity))
                .collect();
            let _ = writeln!(
                out,
                "- **{}** (`{}` | {}): {}",
                disc.source_description,
                disc.id,
                disc.status,
                unclaimed.join(", ")
            );
        }
        let _ = writeln!(out);
    }

    if !pkt.present_factions.is_empty() {
        let _ = writeln!(out, "## Present & Nearby Factions");
        for f in &pkt.present_factions {
            let _ = writeln!(
                out,
                "- **{}** (`{}` | {} | tier: {} | members: {} | morale: {}/100)",
                f.name,
                f.id,
                f.faction_type,
                f.capacity_tier,
                f.members.len(),
                f.morale
            );
            if !f.internal_conflicts.is_empty() {
                let _ = writeln!(
                    out,
                    "  - Internal Conflicts: {}",
                    f.internal_conflicts.join("; ")
                );
            }
        }
        let _ = writeln!(out);
    }

    if !pkt.character_knowledge.is_empty() {
        let _ = writeln!(
            out,
            "## Character Knowledge & Beliefs (POV: `{}`)",
            pkt.perspective
        );
        for k in &pkt.character_knowledge {
            let flags = format!(
                "{}{}",
                if k.concealing { " | CONCEALING" } else { "" },
                if k.outdated { " | OUTDATED" } else { "" }
            );
            let _ = writeln!(
                out,
                "- `{}` on **{}** (`{}` | {}, {}%{}): {}",
                k.actor_id, k.subject, k.claim_id, k.stance, k.confidence, flags, k.perceived_claim
            );
        }
        let _ = writeln!(out);
    }

    if !pkt.prominent_figure_movements.is_empty() {
        let _ = writeln!(out, "## Living World: Prominent Figures Away / In Transit");
        for mv in &pkt.prominent_figure_movements {
            let _ = writeln!(out, "- {mv}");
        }
        let _ = writeln!(out);
    }

    if !pkt.npc_blind_spots.is_empty() {
        let _ = writeln!(out, "## Epistemic Blind Spots (Actors Do NOT Know These)");
        for bs in &pkt.npc_blind_spots {
            let _ = writeln!(out, "- {}", bs.note);
        }
        let _ = writeln!(out);
    }

    if !pkt.gm_only_secrets.is_empty() {
        let _ = writeln!(out, "## GM EYES ONLY (Hidden World Notes)");
        for sec in &pkt.gm_only_secrets {
            let _ = writeln!(out, "- {sec}");
        }
        let _ = writeln!(out);
    }

    if !pkt.recent_campaign_events.is_empty() {
        let _ = writeln!(out, "## Recent Events");
        for ev in &pkt.recent_campaign_events {
            let _ = writeln!(out, "- {ev}");
        }
    }

    if let Some(limit) = max_chars {
        if out.len() > limit && limit > 40 {
            let mut end = limit - 36;
            while !out.is_char_boundary(end) && end > 0 {
                end -= 1;
            }
            out.truncate(end);
            out.push_str("\n\n...(truncated to max_chars budget)");
        }
    }

    out
}
