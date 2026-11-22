use crate::autonomy::{
    ActorAutonomyBrief, ActorBelief, ActorKnowledgeBrief, AutonomyCommitRequest, AutonomyDossier,
    CharacterProfileExt, FactionMember, FactionRecord, KnowledgeClaim, KnowledgeRecordView,
    KnowledgeSearchFilter, StructuredGoal,
};
use crate::compiler::{compile_all_docs, prune_orphaned_docs, CompileReport};
use crate::context::{assemble_scene_context, SceneContextPacket, SceneContextRequest};
use crate::db::CampaignDb;
use crate::dice::{roll_dice, DiceRollResult};
use crate::mechanics::{
    collect_hydrated_modifiers, evaluate_check, execute_table_draw, normalize_random_table,
    CheckRequest, CheckResolution, ContestParticipantResult, ContestRequest, ContestResolution,
    DrawTableOutcome, DrawTableRequest, EncounterLogEntry, EncounterParticipant, EncounterState,
    EventCausalMeta, HydrationOutcome, RandomTable, RulesetProfile, StagedDiscovery,
    StagedDiscoveryItem,
};
use crate::models::{
    slugify, CampaignMeta, CatalogItem, CharacterBaseline, CharacterProjection, EntityHistoryEntry,
    EventPayload, GraphRelation, Item, Landmark, LoreEntry, PlaceBaseline, PlaceProjection,
    ProgressClock, Route, Skill, StatusCondition, StoryAttachment, StoryText, Technique,
    WorldClock, WorldEvent,
};
use crate::projection::{
    load_all_character_projections, load_all_place_projections, load_character_projection,
    load_place_projection,
};
use crate::search::{
    resolve_character_id, resolve_place_id, search_all, search_attachments_filtered,
    search_characters, search_events_filtered, search_histories_filtered, search_items_filtered,
    search_lore_filtered, search_places, search_relations_filtered, search_texts_filtered,
    AttachmentSearchFilter, CharacterSearchFilter, EventSearchFilter, HistorySearchFilter,
    ItemSearchFilter, LoreSearchFilter, OmniSearchResult, PlaceSearchFilter, RelationSearchFilter,
    TextSearchFilter,
};
use crate::world_sim::{
    compute_encumbrance, compute_shortest_route, evaluate_market_prices, generate_seasonal_weather,
    reputation_tier_label, synthesize_routes_from_places, travel_injury_factor, CraftingProject,
    CrimeRecord, EncumbranceReport, InjuryRecord, ItemEconomics, JurisdictionRecord,
    MarketPriceReport, PlaceSpatialExt, ReputationStanding, SettlementMarket, TravelPlan,
    TravelRoute,
};
use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct ImportAttachmentRequest {
    pub id: Option<String>,
    pub title: String,
    pub source_path: Option<String>,
    pub text_content: Option<String>,
    pub base64_content: Option<String>,
    pub filename: Option<String>,
    pub category: Option<String>,
    pub mime_type: Option<String>,
    pub summary: String,
    pub extracted_notes: String,
    pub visibility: Option<String>,
    pub keywords: Vec<String>,
    pub tags: Vec<String>,
    pub related_ids: Vec<String>,
    pub notes: String,
}

/// High-level campaign coordinator combining SQLite Event Sourcing,
/// Knowledge Graph, FTS5 Lorebook, Deterministic Dice, and Automatic Markdown Compilation.
pub struct CampaignEngine {
    root_dir: PathBuf,
    db: CampaignDb,
    auto_compile: bool,
}

impl CampaignEngine {
    pub fn open(campaign_dir: &Path) -> Result<Self> {
        let db_path = campaign_dir.join("campaign.db");
        let db = CampaignDb::open(&db_path)?;
        Ok(Self {
            root_dir: campaign_dir.to_path_buf(),
            db,
            auto_compile: true,
        })
    }

    pub fn set_auto_compile(&mut self, enabled: bool) {
        self.auto_compile = enabled;
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    pub fn docs_dir(&self) -> PathBuf {
        self.root_dir.join("docs")
    }

    pub fn db(&self) -> &CampaignDb {
        &self.db
    }

    pub fn init_campaign(
        &self,
        name: &str,
        description: &str,
        ruleset_notes: &str,
        clock: Option<WorldClock>,
    ) -> Result<CampaignMeta> {
        let meta = CampaignMeta {
            id: slugify(name),
            name: name.to_string(),
            description: description.to_string(),
            ruleset_notes: ruleset_notes.to_string(),
            clock: clock.unwrap_or_default(),
            current_session: 1,
            created_at: Utc::now().to_rfc3339(),
        };
        self.db.init_campaign(&meta)?;
        self.maybe_compile()?;
        Ok(meta)
    }

    pub fn compile_docs(&self) -> Result<CompileReport> {
        compile_all_docs(&self.db, &self.docs_dir())
    }

    pub(crate) fn maybe_compile(&self) -> Result<()> {
        if self.auto_compile {
            let _ = self.compile_docs()?;
        }
        Ok(())
    }

    // -------------------------------------------------------------------------
    // World Clock & Sessions
    // -------------------------------------------------------------------------

    pub fn advance_time(
        &self,
        minutes: u64,
        global_weather: Option<String>,
        reason: Option<String>,
    ) -> Result<(WorldClock, bool, WorldEvent)> {
        let mut meta = self.db.get_campaign_meta()?;
        let season_changed = meta.clock.advance(minutes);
        if let Some(w) = global_weather {
            if !w.trim().is_empty() {
                meta.clock.global_weather = w;
            }
        }
        self.db
            .update_clock_and_session(&meta.clock, meta.current_session)?;

        let mut summary = format!(
            "Advanced time by {} min -> {} ({})",
            minutes,
            meta.clock.format_short(),
            meta.clock.time_of_day()
        );
        if season_changed {
            summary.push_str(&format!(" [Season changed to {}!]", meta.clock.season));
        }
        if let Some(r) = reason {
            if !r.trim().is_empty() {
                summary.push_str(&format!(" — {r}"));
            }
        }

        let ev = self.db.append_event(
            None,
            vec![],
            summary,
            EventPayload::TimeAdvanced {
                minutes,
                new_clock: meta.clock.clone(),
                season_changed,
            },
        )?;
        self.maybe_compile()?;
        Ok((meta.clock, season_changed, ev))
    }

    pub fn start_new_session(&self, title: &str, summary: &str) -> Result<(u32, WorldEvent)> {
        let mut meta = self.db.get_campaign_meta()?;
        meta.current_session += 1;
        self.db
            .update_clock_and_session(&meta.clock, meta.current_session)?;
        let ev = self.db.append_event(
            None,
            vec![],
            format!("Session {}: {} — {}", meta.current_session, title, summary),
            EventPayload::SessionMarker {
                session_number: meta.current_session,
                title: title.to_string(),
                summary: summary.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((meta.current_session, ev))
    }

    // -------------------------------------------------------------------------
    // Characters
    // -------------------------------------------------------------------------

    pub fn create_character(&self, mut baseline: CharacterBaseline) -> Result<CharacterProjection> {
        if baseline.id.trim().is_empty() {
            baseline.id = slugify(&baseline.name);
        } else {
            baseline.id = slugify(&baseline.id);
        }
        if self.db.get_character_baseline(&baseline.id)?.is_some() {
            bail!("Character '{}' already exists.", baseline.id);
        }
        let meta = self.db.get_campaign_meta()?;
        if baseline.created_at_world_time.is_empty() {
            baseline.created_at_world_time = meta.clock.format_short();
        }
        if let Some(ref loc) = baseline.initial_location {
            baseline.initial_location = Some(slugify(loc));
        }

        let mut affected = vec![baseline.id.clone()];
        if let Some(ref loc) = baseline.initial_location {
            affected.push(loc.clone());
        }

        let ev = self.db.append_event(
            baseline.initial_location.clone(),
            affected,
            format!(
                "Character created: {} ({}, {})",
                baseline.name,
                baseline.kind,
                if baseline.archetype.is_empty() {
                    "unclassified"
                } else {
                    &baseline.archetype
                }
            ),
            EventPayload::CharacterCreated {
                character_id: baseline.id.clone(),
            },
        )?;
        self.db.insert_character_baseline(&baseline, ev.seq)?;
        self.maybe_compile()?;
        load_character_projection(&self.db, &baseline.id, None)
    }

    pub fn get_character(&self, id: &str, as_of_seq: Option<i64>) -> Result<CharacterProjection> {
        let cid = resolve_character_id(&self.db, id)?;
        load_character_projection(&self.db, &cid, as_of_seq)
    }

    pub fn list_characters(&self) -> Result<Vec<CharacterProjection>> {
        Ok(load_all_character_projections(&self.db, None)?
            .into_values()
            .collect())
    }

    pub fn move_character(
        &self,
        character_id: &str,
        to_place_id: &str,
        travel_notes: &str,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let pid = resolve_place_id(&self.db, to_place_id).unwrap_or_else(|_| slugify(to_place_id));
        let proj = load_character_projection(&self.db, &cid, None)?;
        let from_place = proj.current.current_location.clone();

        let mut affected = vec![cid.clone(), pid.clone()];
        if let Some(ref fp) = from_place {
            affected.push(fp.clone());
        }
        let summary = format!(
            "{} traveled from {} to {}{}",
            proj.baseline.name,
            from_place.as_deref().unwrap_or("unknown"),
            pid,
            if travel_notes.is_empty() {
                String::new()
            } else {
                format!(" ({travel_notes})")
            }
        );

        let ev = self.db.append_event(
            Some(pid.clone()),
            affected,
            summary,
            EventPayload::CharacterMoved {
                character_id: cid.clone(),
                from_place,
                to_place: pid,
                travel_notes: travel_notes.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((load_character_projection(&self.db, &cid, None)?, ev))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn modify_resource(
        &self,
        character_id: &str,
        resource: &str,
        delta: Option<i64>,
        set_current: Option<i64>,
        set_max: Option<i64>,
        unit: Option<String>,
        reason: &str,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let old = proj.current.resources.get(resource).cloned();
        let change_desc = if let Some(v) = set_current {
            format!("set to {v}")
        } else if let Some(d) = delta {
            format!("{d:+}")
        } else if let Some(m) = set_max {
            format!("max set to {m}")
        } else {
            "updated".to_string()
        };
        let old_str = old
            .map(|r| format!(" (was {}/{})", r.current, r.max))
            .unwrap_or_default();
        let summary = format!(
            "{}: {} {}{}{}",
            proj.baseline.name,
            resource,
            change_desc,
            old_str,
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );

        let ev = self.db.append_event(
            proj.current.current_location.clone(),
            vec![cid.clone()],
            summary,
            EventPayload::ResourceChanged {
                character_id: cid.clone(),
                resource: resource.to_string(),
                delta,
                set_current,
                set_max,
                unit,
                reason: reason.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((load_character_projection(&self.db, &cid, None)?, ev))
    }

    pub fn modify_stat(
        &self,
        character_id: &str,
        stat: &str,
        delta: Option<i64>,
        set_value: Option<i64>,
        reason: &str,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let desc = if let Some(v) = set_value {
            format!("set to {v}")
        } else if let Some(d) = delta {
            format!("{d:+}")
        } else {
            "updated".to_string()
        };
        let summary = format!(
            "{}: stat {} {}{}",
            proj.baseline.name,
            stat,
            desc,
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );

        let ev = self.db.append_event(
            proj.current.current_location.clone(),
            vec![cid.clone()],
            summary,
            EventPayload::StatChanged {
                character_id: cid.clone(),
                stat: stat.to_string(),
                delta,
                set_value,
                reason: reason.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((load_character_projection(&self.db, &cid, None)?, ev))
    }

    pub fn set_character_status(
        &self,
        character_id: &str,
        alive_status: &str,
        reason: &str,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let summary = format!(
            "{} status changed to '{}'{}",
            proj.baseline.name,
            alive_status,
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );
        let ev = self.db.append_event(
            proj.current.current_location.clone(),
            vec![cid.clone()],
            summary,
            EventPayload::CharacterStatusChanged {
                character_id: cid.clone(),
                alive_status: alive_status.to_string(),
                reason: reason.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((load_character_projection(&self.db, &cid, None)?, ev))
    }

    pub fn add_condition(
        &self,
        character_id: &str,
        condition: StatusCondition,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let summary = format!(
            "{} gained {} '{}'{}",
            proj.baseline.name,
            condition.kind,
            condition.name,
            if condition.notes.is_empty() {
                String::new()
            } else {
                format!(" — {}", condition.notes)
            }
        );
        let ev = self.db.append_event(
            proj.current.current_location.clone(),
            vec![cid.clone()],
            summary,
            EventPayload::ConditionAdded {
                character_id: cid.clone(),
                condition,
            },
        )?;
        self.maybe_compile()?;
        Ok((load_character_projection(&self.db, &cid, None)?, ev))
    }

    pub fn remove_condition(
        &self,
        character_id: &str,
        condition_name: &str,
        reason: &str,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let summary = format!(
            "{} cleared condition '{}'{}",
            proj.baseline.name,
            condition_name,
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );
        let ev = self.db.append_event(
            proj.current.current_location.clone(),
            vec![cid.clone()],
            summary,
            EventPayload::ConditionRemoved {
                character_id: cid.clone(),
                condition_name: condition_name.to_string(),
                reason: reason.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((load_character_projection(&self.db, &cid, None)?, ev))
    }

    pub fn add_item(
        &self,
        character_id: &str,
        item: Item,
        reason: &str,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let summary = format!(
            "{} acquired {} x{}{}",
            proj.baseline.name,
            item.name,
            item.quantity,
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );
        let ev = self.db.append_event(
            proj.current.current_location.clone(),
            vec![cid.clone()],
            summary,
            EventPayload::ItemAcquired {
                character_id: cid.clone(),
                item,
            },
        )?;
        self.maybe_compile()?;
        Ok((load_character_projection(&self.db, &cid, None)?, ev))
    }

    pub fn remove_item(
        &self,
        character_id: &str,
        item_name: &str,
        quantity: i64,
        reason: &str,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let summary = format!(
            "{} lost/used {} x{}{}",
            proj.baseline.name,
            item_name,
            quantity,
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );
        let ev = self.db.append_event(
            proj.current.current_location.clone(),
            vec![cid.clone()],
            summary,
            EventPayload::ItemRemoved {
                character_id: cid.clone(),
                item_name: item_name.to_string(),
                quantity,
                reason: reason.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((load_character_projection(&self.db, &cid, None)?, ev))
    }

    pub fn equip_item(
        &self,
        character_id: &str,
        item_name: &str,
        slot: Option<String>,
        equipped: bool,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let verb = if equipped { "equipped" } else { "unequipped" };
        let summary = format!("{} {} '{}'", proj.baseline.name, verb, item_name);
        let ev = self.db.append_event(
            proj.current.current_location.clone(),
            vec![cid.clone()],
            summary,
            EventPayload::ItemEquipped {
                character_id: cid.clone(),
                item_name: item_name.to_string(),
                slot,
                equipped,
            },
        )?;
        self.maybe_compile()?;
        Ok((load_character_projection(&self.db, &cid, None)?, ev))
    }

    pub fn upsert_skill(
        &self,
        character_id: &str,
        skill: Skill,
        reason: &str,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let summary = format!(
            "{} skill '{}' updated to rank '{}'{}",
            proj.baseline.name,
            skill.name,
            skill.rank,
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );
        let ev = self.db.append_event(
            proj.current.current_location.clone(),
            vec![cid.clone()],
            summary,
            EventPayload::SkillUpdated {
                character_id: cid.clone(),
                skill,
            },
        )?;
        self.maybe_compile()?;
        Ok((load_character_projection(&self.db, &cid, None)?, ev))
    }

    pub fn upsert_technique(
        &self,
        character_id: &str,
        technique: Technique,
        reason: &str,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let summary = format!(
            "{} learned/updated technique '{}' ({}){}",
            proj.baseline.name,
            technique.name,
            if technique.mastery.is_empty() {
                &technique.tier
            } else {
                &technique.mastery
            },
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );
        let ev = self.db.append_event(
            proj.current.current_location.clone(),
            vec![cid.clone()],
            summary,
            EventPayload::TechniqueUpdated {
                character_id: cid.clone(),
                technique,
            },
        )?;
        self.maybe_compile()?;
        Ok((load_character_projection(&self.db, &cid, None)?, ev))
    }

    pub fn update_goal(
        &self,
        character_id: &str,
        goal: &str,
        completed: bool,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let status = if completed { "completed" } else { "added" };
        let summary = format!("{} {} goal: {}", proj.baseline.name, status, goal);
        let ev = self.db.append_event(
            proj.current.current_location.clone(),
            vec![cid.clone()],
            summary,
            EventPayload::GoalUpdated {
                character_id: cid.clone(),
                goal: goal.to_string(),
                completed,
            },
        )?;
        self.maybe_compile()?;
        Ok((load_character_projection(&self.db, &cid, None)?, ev))
    }

    // -------------------------------------------------------------------------
    // Places (Terrain, Seasons, Landmarks, Routes, Conditions)
    // -------------------------------------------------------------------------

    pub fn create_place(&self, mut baseline: PlaceBaseline) -> Result<PlaceProjection> {
        if baseline.id.trim().is_empty() {
            baseline.id = slugify(&baseline.name);
        } else {
            baseline.id = slugify(&baseline.id);
        }
        if self.db.get_place_baseline(&baseline.id)?.is_some() {
            bail!("Place '{}' already exists.", baseline.id);
        }
        let meta = self.db.get_campaign_meta()?;
        if baseline.created_at_world_time.is_empty() {
            baseline.created_at_world_time = meta.clock.format_short();
        }
        if let Some(ref p) = baseline.parent_place {
            baseline.parent_place = Some(slugify(p));
        }
        for r in &mut baseline.routes {
            r.destination_id = slugify(&r.destination_id);
        }

        let ev = self.db.append_event(
            Some(baseline.id.clone()),
            vec![baseline.id.clone()],
            format!(
                "Place charted: {} ({}, terrain: {})",
                baseline.name, baseline.place_type, baseline.terrain
            ),
            EventPayload::PlaceCreated {
                place_id: baseline.id.clone(),
            },
        )?;
        self.db.insert_place_baseline(&baseline, ev.seq)?;
        self.maybe_compile()?;
        load_place_projection(&self.db, &baseline.id, None)
    }

    pub fn get_place(&self, id: &str, as_of_seq: Option<i64>) -> Result<PlaceProjection> {
        let pid = resolve_place_id(&self.db, id)?;
        load_place_projection(&self.db, &pid, as_of_seq)
    }

    pub fn list_places(&self) -> Result<Vec<PlaceProjection>> {
        Ok(load_all_place_projections(&self.db, None)?
            .into_values()
            .collect())
    }

    pub fn set_place_weather(
        &self,
        place_id: &str,
        weather: &str,
        notes: &str,
    ) -> Result<(PlaceProjection, WorldEvent)> {
        let pid = resolve_place_id(&self.db, place_id)?;
        let proj = load_place_projection(&self.db, &pid, None)?;
        let summary = format!(
            "{} weather shifted to '{}'{}",
            proj.baseline.name,
            weather,
            if notes.is_empty() {
                String::new()
            } else {
                format!(" — {notes}")
            }
        );
        let ev = self.db.append_event(
            Some(pid.clone()),
            vec![pid.clone()],
            summary,
            EventPayload::PlaceWeatherChanged {
                place_id: pid.clone(),
                weather: weather.to_string(),
                notes: notes.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((load_place_projection(&self.db, &pid, None)?, ev))
    }

    pub fn set_place_condition(
        &self,
        place_id: &str,
        condition: &str,
        active: bool,
        notes: &str,
    ) -> Result<(PlaceProjection, WorldEvent)> {
        let pid = resolve_place_id(&self.db, place_id)?;
        let proj = load_place_projection(&self.db, &pid, None)?;
        let verb = if active { "gained" } else { "cleared" };
        let summary = format!(
            "{} {} condition '{}'{}",
            proj.baseline.name,
            verb,
            condition,
            if notes.is_empty() {
                String::new()
            } else {
                format!(" — {notes}")
            }
        );
        let ev = self.db.append_event(
            Some(pid.clone()),
            vec![pid.clone()],
            summary,
            EventPayload::PlaceConditionChanged {
                place_id: pid.clone(),
                condition: condition.to_string(),
                active,
                notes: notes.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((load_place_projection(&self.db, &pid, None)?, ev))
    }

    pub fn set_place_control(
        &self,
        place_id: &str,
        faction: Option<String>,
        danger_level: Option<String>,
        notes: &str,
    ) -> Result<(PlaceProjection, WorldEvent)> {
        let pid = resolve_place_id(&self.db, place_id)?;
        let proj = load_place_projection(&self.db, &pid, None)?;
        let summary = format!(
            "{} control/danger updated (faction: {}, danger: {}){}",
            proj.baseline.name,
            faction.as_deref().unwrap_or("unchanged"),
            danger_level.as_deref().unwrap_or("unchanged"),
            if notes.is_empty() {
                String::new()
            } else {
                format!(" — {notes}")
            }
        );
        let ev = self.db.append_event(
            Some(pid.clone()),
            vec![pid.clone()],
            summary,
            EventPayload::PlaceControlChanged {
                place_id: pid.clone(),
                faction,
                danger_level,
                notes: notes.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((load_place_projection(&self.db, &pid, None)?, ev))
    }

    pub fn upsert_landmark(
        &self,
        place_id: &str,
        landmark: Landmark,
        notes: &str,
    ) -> Result<(PlaceProjection, WorldEvent)> {
        let pid = resolve_place_id(&self.db, place_id)?;
        let proj = load_place_projection(&self.db, &pid, None)?;
        let summary = format!(
            "{}: landmark '{}' updated to status '{}'{}",
            proj.baseline.name,
            landmark.name,
            landmark.status,
            if notes.is_empty() {
                String::new()
            } else {
                format!(" — {notes}")
            }
        );
        let ev = self.db.append_event(
            Some(pid.clone()),
            vec![pid.clone()],
            summary,
            EventPayload::LandmarkUpdated {
                place_id: pid.clone(),
                landmark,
            },
        )?;
        self.maybe_compile()?;
        Ok((load_place_projection(&self.db, &pid, None)?, ev))
    }

    pub fn upsert_route(
        &self,
        place_id: &str,
        mut route: Route,
        notes: &str,
    ) -> Result<(PlaceProjection, WorldEvent)> {
        let pid = resolve_place_id(&self.db, place_id)?;
        route.destination_id = slugify(&route.destination_id);
        let proj = load_place_projection(&self.db, &pid, None)?;
        let summary = format!(
            "{}: route to '{}' is now '{}'{}",
            proj.baseline.name,
            route.destination_id,
            route.status,
            if notes.is_empty() {
                String::new()
            } else {
                format!(" — {notes}")
            }
        );
        let ev = self.db.append_event(
            Some(pid.clone()),
            vec![pid.clone(), route.destination_id.clone()],
            summary,
            EventPayload::RouteUpdated {
                place_id: pid.clone(),
                route,
            },
        )?;
        self.maybe_compile()?;
        Ok((load_place_projection(&self.db, &pid, None)?, ev))
    }

    // -------------------------------------------------------------------------
    // World-Building & Lore (Separate Reference Codex)
    // -------------------------------------------------------------------------

    pub fn upsert_lore(&self, mut entry: LoreEntry, notes: &str) -> Result<(LoreEntry, WorldEvent)> {
        if entry.id.trim().is_empty() {
            entry.id = slugify(&entry.title);
        } else {
            entry.id = slugify(&entry.id);
        }
        if entry.category.trim().is_empty() {
            entry.category = "world".to_string();
        }
        entry.related_ids = entry.related_ids.into_iter().map(|s| slugify(&s)).collect();

        let meta = self.db.get_campaign_meta()?;
        let now_world = meta.clock.format_short();
        let existing = self.db.get_lore(&entry.id)?;
        let action = if let Some(ex) = existing {
            entry.created_at_world_time = ex.created_at_world_time;
            entry.updated_at_world_time = now_world;
            "updated"
        } else {
            entry.created_at_world_time = now_world.clone();
            entry.updated_at_world_time = now_world;
            "created"
        };

        self.db.upsert_lore(&entry)?;
        let mut affected = vec![entry.id.clone()];
        affected.extend(entry.related_ids.clone());

        let summary = format!(
            "Lore '{}' [{}] {}{}",
            entry.title,
            entry.category,
            action,
            if notes.is_empty() {
                String::new()
            } else {
                format!(" — {notes}")
            }
        );
        let ev = self.db.append_event_causal(
            None,
            affected,
            summary,
            EventPayload::LoreRecorded {
                lore_id: entry.id.clone(),
                action: action.to_string(),
                notes: notes.to_string(),
            },
            EventCausalMeta {
                actor_id: None,
                causal_event_ids: vec![],
                visibility: entry.visibility.clone(),
            },
        )?;
        self.maybe_compile()?;
        Ok((entry, ev))
    }

    pub fn get_lore(&self, id: &str) -> Result<LoreEntry> {
        let slug = slugify(id);
        self.db
            .get_lore(&slug)?
            .ok_or_else(|| anyhow!("Lore entry '{slug}' not found"))
    }

    pub fn search_lore(&self, query: &str, limit: usize) -> Result<Vec<LoreEntry>> {
        self.db.search_lore_fts(query, limit)
    }

    // -------------------------------------------------------------------------
    // Books, Scrolls, Grimoires & Story Texts
    // -------------------------------------------------------------------------

    pub fn upsert_text(&self, mut text: StoryText, notes: &str) -> Result<(StoryText, WorldEvent)> {
        if text.id.trim().is_empty() {
            text.id = slugify(&text.title);
        } else {
            text.id = slugify(&text.id);
        }
        if text.medium.trim().is_empty() {
            text.medium = "book".to_string();
        }
        if text.subject.trim().is_empty() {
            text.subject = "history".to_string();
        }
        if text.language.trim().is_empty() {
            text.language = "Common".to_string();
        }
        if text.condition.trim().is_empty() {
            text.condition = "Intact".to_string();
        }
        if text.read_status.trim().is_empty() {
            text.read_status = "undiscovered".to_string();
        }
        text.read_by = text
            .read_by
            .into_iter()
            .map(|r| resolve_character_id(&self.db, &r).unwrap_or_else(|_| slugify(&r)))
            .collect();
        if let Some(ref loc) = text.holder_or_location_id {
            if !loc.trim().is_empty() {
                let resolved = resolve_character_id(&self.db, loc)
                    .or_else(|_| resolve_place_id(&self.db, loc))
                    .unwrap_or_else(|_| slugify(loc));
                text.holder_or_location_id = Some(resolved);
            } else {
                text.holder_or_location_id = None;
            }
        }
        text.related_ids = text.related_ids.into_iter().map(|s| slugify(&s)).collect();

        let meta = self.db.get_campaign_meta()?;
        let now_world = meta.clock.format_short();
        let existing = self.db.get_text(&text.id)?;
        let action = if let Some(ex) = existing {
            text.created_at_world_time = ex.created_at_world_time;
            text.updated_at_world_time = now_world;
            "updated"
        } else {
            text.created_at_world_time = now_world.clone();
            text.updated_at_world_time = now_world;
            "cataloged"
        };

        self.db.upsert_text(&text)?;
        let mut affected = vec![text.id.clone()];
        if let Some(ref h) = text.holder_or_location_id {
            affected.push(h.clone());
        }
        affected.extend(text.related_ids.clone());

        let summary = format!(
            "Text '{}' ({}, {}) {}{}",
            text.title,
            text.medium,
            text.subject,
            action,
            if notes.is_empty() {
                String::new()
            } else {
                format!(" — {notes}")
            }
        );
        let ev = self.db.append_event(
            text.holder_or_location_id.clone(),
            affected,
            summary,
            EventPayload::TextRecorded {
                text_id: text.id.clone(),
                action: action.to_string(),
                notes: notes.to_string(),
            },
        )?;

        // Automatically keep physical books, manuals, scrolls, records, etc. synced as carryable CatalogItems!
        if is_carryable_text_medium(&text.medium) {
            self.sync_text_as_carryable_item(&text)?;
        }

        self.maybe_compile()?;
        Ok((text, ev))
    }

    pub fn get_text(&self, id: &str) -> Result<StoryText> {
        let slug = slugify(id);
        self.db
            .get_text(&slug)?
            .ok_or_else(|| anyhow!("Book/Scroll text '{slug}' not found"))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_text_reading(
        &self,
        text_id: &str,
        reader_id: Option<String>,
        new_read_status: Option<String>,
        new_holder_or_location_id: Option<String>,
        new_condition: Option<String>,
        notes: &str,
    ) -> Result<(StoryText, WorldEvent)> {
        let mut text = self.get_text(text_id)?;
        let meta = self.db.get_campaign_meta()?;
        text.updated_at_world_time = meta.clock.format_short();

        let resolved_reader = match reader_id {
            Some(r) if !r.trim().is_empty() => {
                let cid = resolve_character_id(&self.db, &r).unwrap_or_else(|_| slugify(&r));
                if !text.read_by.contains(&cid) {
                    text.read_by.push(cid.clone());
                }
                Some(cid)
            }
            _ => None,
        };

        if let Some(ref st) = new_read_status {
            if !st.trim().is_empty() {
                text.read_status = st.clone();
            }
        } else if resolved_reader.is_some()
            && (text.read_status == "undiscovered" || text.read_status == "discovered_unread")
        {
            text.read_status = "read".to_string();
        }

        let prev_holder = text.holder_or_location_id.clone();
        let resolved_holder = match new_holder_or_location_id {
            Some(h) if !h.trim().is_empty() => {
                let hid = resolve_character_id(&self.db, &h)
                    .or_else(|_| resolve_place_id(&self.db, &h))
                    .unwrap_or_else(|_| slugify(&h));
                text.holder_or_location_id = Some(hid.clone());
                Some(hid)
            }
            _ => None,
        };

        if let Some(ref cond) = new_condition {
            if !cond.trim().is_empty() {
                text.condition = cond.clone();
            }
        }

        self.db.upsert_text(&text)?;

        let mut affected = vec![text.id.clone()];
        if let Some(ref r) = resolved_reader {
            affected.push(r.clone());
        }
        if let Some(ref h) = resolved_holder {
            affected.push(h.clone());
        }

        let mut parts = Vec::new();
        if let Some(ref r) = resolved_reader {
            parts.push(format!("studied by {r}"));
        }
        parts.push(format!("status: {}", text.read_status));
        if let Some(ref h) = resolved_holder {
            parts.push(format!("now at/held by {h}"));
        }
        if let Some(ref c) = new_condition {
            parts.push(format!("condition: {c}"));
        }

        let summary = format!(
            "Text '{}' ({}){}",
            text.title,
            parts.join(", "),
            if notes.is_empty() {
                String::new()
            } else {
                format!(" — {notes}")
            }
        );

        let ev = self.db.append_event(
            text.holder_or_location_id.clone(),
            affected,
            summary,
            EventPayload::TextStudied {
                text_id: text.id.clone(),
                reader_id: resolved_reader,
                read_status: Some(text.read_status.clone()),
                holder_or_location_id: resolved_holder.clone(),
                condition: new_condition,
                notes: notes.to_string(),
            },
        )?;

        // If the physical book/scroll/manual moved to a new holder, sync its carryable CatalogItem & character inventory!
        if is_carryable_text_medium(&text.medium) {
            if let Some(ref new_h) = resolved_holder {
                if prev_holder.as_deref() != Some(new_h.as_str()) {
                    let _ = self.sync_text_holder_transfer(&text, prev_holder.as_deref(), new_h);
                }
            } else {
                let _ = self.sync_text_as_carryable_item(&text);
            }
        }

        self.maybe_compile()?;
        Ok((text, ev))
    }

    pub fn search_texts_by(&self, filter: &TextSearchFilter) -> Result<Vec<StoryText>> {
        search_texts_filtered(&self.db, filter)
    }

    // -------------------------------------------------------------------------
    // Attached Reference Docs & Media (`docs/attachments/files/`)
    // -------------------------------------------------------------------------

    /// Copies a local file (via `source_path`) OR writes inline `text_content` / `base64_content`
    /// into `<campaign>/docs/attachments/files/`, indexes metadata + extracted notes in SQLite/FTS5,
    /// logs an event, compiles `docs/attachments/<id>.md`, and returns the addressable record.
    pub fn import_attachment(
        &self,
        req: ImportAttachmentRequest,
    ) -> Result<(StoryAttachment, PathBuf, WorldEvent)> {
        if req.title.trim().is_empty() {
            bail!("Attachment title cannot be empty");
        }

        let (bytes, inferred_filename, source_origin) = if let Some(ref src) = req.source_path {
            let src_path = Path::new(src);
            if !src_path.exists() {
                bail!("Source file not found: {}", src_path.display());
            }
            let data = std::fs::read(src_path)
                .with_context(|| format!("Failed to read source file {}", src_path.display()))?;
            let fname = req.filename.clone().unwrap_or_else(|| {
                src_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("attachment.bin")
                    .to_string()
            });
            (data, fname, format!("copied from {}", src_path.display()))
        } else if let Some(ref b64) = req.base64_content {
            let data = decode_base64(b64)?;
            let fname = req
                .filename
                .clone()
                .unwrap_or_else(|| format!("{}.bin", slugify(&req.title)));
            (data, fname, "uploaded via base64".to_string())
        } else if let Some(ref txt) = req.text_content {
            let fname = req
                .filename
                .clone()
                .unwrap_or_else(|| format!("{}.md", slugify(&req.title)));
            (txt.as_bytes().to_vec(), fname, "saved inline text".to_string())
        } else {
            bail!("Must provide one of 'source_path', 'text_content', or 'base64_content'");
        };

        let id = match req.id {
            Some(ref raw) if !raw.trim().is_empty() => slugify(raw),
            _ => slugify(&req.title),
        };

        let ext = Path::new(&inferred_filename)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                e.chars()
                    .filter(|c| c.is_ascii_alphanumeric())
                    .map(|c| c.to_ascii_lowercase())
                    .collect::<String>()
            })
            .filter(|e| !e.is_empty());

        let stored_filename = match &ext {
            Some(e) => format!("{id}.{e}"),
            None => id.clone(),
        };

        let files_dir = self.docs_dir().join("attachments").join("files");
        std::fs::create_dir_all(&files_dir)?;
        let dest_abs = files_dir.join(&stored_filename);
        std::fs::write(&dest_abs, &bytes)
            .with_context(|| format!("Failed writing attachment to {}", dest_abs.display()))?;

        let relative_path = format!("docs/attachments/files/{stored_filename}");
        let sha256 = sha256_hex(&bytes);
        let mime_type = req
            .mime_type
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| guess_mime_from_ext(ext.as_deref()).to_string());
        let category = req
            .category
            .filter(|c| !c.trim().is_empty())
            .unwrap_or_else(|| guess_category_from_mime(&mime_type).to_string());
        let visibility = req
            .visibility
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "public".to_string());

        let related_ids: Vec<String> = req.related_ids.into_iter().map(|s| slugify(&s)).collect();

        let meta = self.db.get_campaign_meta()?;
        let now_world = meta.clock.format_short();
        let existing = self.db.get_attachment(&id)?;
        let (created_at, action) = if let Some(ex) = existing {
            (ex.created_at_world_time, "updated")
        } else {
            (now_world.clone(), "attached")
        };

        let att = StoryAttachment {
            id: id.clone(),
            title: req.title.clone(),
            original_filename: inferred_filename,
            stored_filename,
            relative_path: relative_path.clone(),
            category,
            mime_type,
            size_bytes: bytes.len() as u64,
            sha256,
            summary: req.summary,
            extracted_notes: req.extracted_notes,
            visibility,
            keywords: req.keywords,
            tags: req.tags,
            related_ids: related_ids.clone(),
            source_origin,
            created_at_world_time: created_at,
            updated_at_world_time: now_world,
        };

        self.db.upsert_attachment(&att)?;

        let mut affected = vec![att.id.clone()];
        affected.extend(related_ids);
        let summary = format!(
            "Attached doc '{}' [{}] -> {}{}",
            att.title,
            att.category,
            att.relative_path,
            if req.notes.is_empty() {
                String::new()
            } else {
                format!(" — {}", req.notes)
            }
        );

        let ev = self.db.append_event(
            None,
            affected,
            summary,
            EventPayload::AttachmentRecorded {
                attachment_id: att.id.clone(),
                relative_path,
                action: action.to_string(),
                notes: req.notes,
            },
        )?;
        self.maybe_compile()?;
        Ok((att, dest_abs, ev))
    }

    pub fn get_attachment(&self, id: &str) -> Result<StoryAttachment> {
        let slug = slugify(id);
        self.db
            .get_attachment(&slug)?
            .ok_or_else(|| anyhow!("Attached doc '{slug}' not found"))
    }

    pub fn attachment_file_path(&self, att: &StoryAttachment) -> PathBuf {
        self.docs_dir()
            .join("attachments")
            .join("files")
            .join(&att.stored_filename)
    }

    pub fn search_attachments_by(
        &self,
        filter: &AttachmentSearchFilter,
    ) -> Result<Vec<StoryAttachment>> {
        search_attachments_filtered(&self.db, filter)
    }

    /// Creates a new reference file (or updates/appends to an existing one) in
    /// `<campaign>/docs/attachments/files/` to store pasted-in references, lore dumps, or rules.
    #[allow(clippy::too_many_arguments)]
    pub fn create_reference_doc(
        &self,
        id: Option<String>,
        title: String,
        filename: Option<String>,
        content: String,
        append: bool,
        category: Option<String>,
        summary: String,
        extracted_notes: String,
        visibility: Option<String>,
        keywords: Vec<String>,
        tags: Vec<String>,
        related_ids: Vec<String>,
        notes: String,
    ) -> Result<(StoryAttachment, PathBuf, WorldEvent)> {
        let slug = match id {
            Some(ref r) if !r.trim().is_empty() => slugify(r),
            _ => slugify(&title),
        };
        let final_text = if append {
            if let Some(existing) = self.db.get_attachment(&slug)? {
                let path = self.attachment_file_path(&existing);
                let prev = std::fs::read_to_string(&path).unwrap_or_default();
                if prev.is_empty() {
                    content
                } else {
                    format!("{prev}\n\n{content}")
                }
            } else {
                content
            }
        } else {
            content
        };
        let fname = filename
            .filter(|f| !f.trim().is_empty())
            .unwrap_or_else(|| format!("{slug}.md"));

        self.import_attachment(ImportAttachmentRequest {
            id: Some(slug),
            title,
            source_path: None,
            text_content: Some(final_text),
            base64_content: None,
            filename: Some(fname),
            category: category.or_else(|| Some("world-doc".to_string())),
            mime_type: None,
            summary,
            extracted_notes,
            visibility,
            keywords,
            tags,
            related_ids,
            notes,
        })
    }

    // -------------------------------------------------------------------------
    // Cataloged Carryable Items (Weapons, Tools, Artifacts, Keepsakes, Books/Manuals/Records)
    // -------------------------------------------------------------------------

    pub fn upsert_catalog_item(
        &self,
        mut item: CatalogItem,
        notes: &str,
    ) -> Result<(CatalogItem, WorldEvent)> {
        if item.name.trim().is_empty() {
            bail!("Item name cannot be empty");
        }
        if item.id.trim().is_empty() {
            item.id = slugify(&item.name);
        } else {
            item.id = slugify(&item.id);
        }
        if item.item_type.trim().is_empty() {
            item.item_type = "gear".to_string();
        }
        if item.condition_or_durability.trim().is_empty() {
            item.condition_or_durability = "Intact".to_string();
        }
        if item.quantity <= 0 {
            item.quantity = 1;
        }
        if let Some(ref h) = item.current_holder_id {
            if !h.trim().is_empty() {
                let resolved = resolve_character_id(&self.db, h)
                    .or_else(|_| resolve_place_id(&self.db, h))
                    .unwrap_or_else(|_| slugify(h));
                item.current_holder_id = Some(resolved);
            } else {
                item.current_holder_id = None;
            }
        }
        if let Some(ref tid) = item.linked_text_id {
            if !tid.trim().is_empty() {
                item.linked_text_id = Some(slugify(tid));
            } else {
                item.linked_text_id = None;
            }
        }
        item.related_ids = item.related_ids.into_iter().map(|s| slugify(&s)).collect();

        let meta = self.db.get_campaign_meta()?;
        let now_world = meta.clock.format_short();
        let existing = self.db.get_catalog_item(&item.id)?;
        let prev_holder = existing.as_ref().and_then(|e| e.current_holder_id.clone());
        let action = if let Some(ex) = existing {
            item.created_at_world_time = ex.created_at_world_time;
            item.updated_at_world_time = now_world;
            if item.previous_owners.is_empty() {
                item.previous_owners = ex.previous_owners;
            }
            "updated"
        } else {
            item.created_at_world_time = now_world.clone();
            item.updated_at_world_time = now_world;
            "cataloged"
        };

        self.db.upsert_catalog_item(&item)?;

        // Sync carryable item into character's live equipment/inventory if held by a character
        self.sync_catalog_item_to_character_inventories(&item, prev_holder.as_deref())?;

        let mut affected = vec![item.id.clone()];
        if let Some(ref h) = item.current_holder_id {
            affected.push(h.clone());
        }
        if let Some(ref tid) = item.linked_text_id {
            affected.push(tid.clone());
        }
        affected.extend(item.related_ids.clone());

        let summary = format!(
            "Item '{}' [{}] {}{}",
            item.name,
            item.item_type,
            action,
            if notes.is_empty() {
                String::new()
            } else {
                format!(" — {notes}")
            }
        );
        let ev = self.db.append_event(
            item.current_holder_id.clone(),
            affected,
            summary,
            EventPayload::ItemCataloged {
                item_id: item.id.clone(),
                action: action.to_string(),
                holder_id: item.current_holder_id.clone(),
                notes: notes.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((item, ev))
    }

    pub fn get_catalog_item(&self, id: &str) -> Result<CatalogItem> {
        let slug = slugify(id);
        self.db
            .get_catalog_item(&slug)?
            .ok_or_else(|| anyhow!("Catalog item '{slug}' not found"))
    }

    pub fn transfer_catalog_item(
        &self,
        item_id: &str,
        to_holder_id: Option<String>,
        equipped: Option<bool>,
        condition_or_durability: Option<String>,
        notes: &str,
    ) -> Result<(CatalogItem, WorldEvent)> {
        let mut item = self.get_catalog_item(item_id)?;
        let meta = self.db.get_campaign_meta()?;
        item.updated_at_world_time = meta.clock.format_short();

        let from_holder = item.current_holder_id.clone();
        let resolved_to = match to_holder_id {
            Some(ref h) if !h.trim().is_empty() => {
                let hid = resolve_character_id(&self.db, h)
                    .or_else(|_| resolve_place_id(&self.db, h))
                    .unwrap_or_else(|_| slugify(h));
                Some(hid)
            }
            _ => None,
        };

        // Record previous owner cleanly when holder changes
        if let Some(ref old_h) = from_holder {
            if resolved_to.as_deref() != Some(old_h.as_str())
                && !item.previous_owners.iter().any(|p| p == old_h)
            {
                item.previous_owners.push(old_h.clone());
            }
        }
        item.current_holder_id = resolved_to.clone();
        if let Some(eq) = equipped {
            item.equipped = eq;
        }
        if let Some(ref cond) = condition_or_durability {
            if !cond.trim().is_empty() {
                item.condition_or_durability = cond.clone();
            }
        }

        self.db.upsert_catalog_item(&item)?;
        self.sync_catalog_item_to_character_inventories(&item, from_holder.as_deref())?;

        // Keep linked StoryText holder in sync if this carryable item is a book/manual/scroll/record
        if let Some(ref tid) = item.linked_text_id {
            if let Some(mut txt) = self.db.get_text(tid)? {
                txt.holder_or_location_id = resolved_to.clone();
                if let Some(ref cond) = condition_or_durability {
                    if !cond.trim().is_empty() {
                        txt.condition = cond.clone();
                    }
                }
                txt.updated_at_world_time = item.updated_at_world_time.clone();
                self.db.upsert_text(&txt)?;
            }
        }

        let mut affected = vec![item.id.clone()];
        if let Some(ref fh) = from_holder {
            affected.push(fh.clone());
        }
        if let Some(ref th) = resolved_to {
            affected.push(th.clone());
        }
        if let Some(ref tid) = item.linked_text_id {
            affected.push(tid.clone());
        }

        let summary = format!(
            "Item '{}' transferred from {} to {}{}",
            item.name,
            from_holder.as_deref().unwrap_or("unclaimed"),
            resolved_to.as_deref().unwrap_or("unclaimed"),
            if notes.is_empty() {
                String::new()
            } else {
                format!(" — {notes}")
            }
        );

        let ev = self.db.append_event(
            resolved_to.clone(),
            affected,
            summary,
            EventPayload::CatalogItemTransferred {
                item_id: item.id.clone(),
                from_holder_id: from_holder,
                to_holder_id: resolved_to,
                condition_or_durability,
                notes: notes.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((item, ev))
    }

    pub fn search_items_by(&self, filter: &ItemSearchFilter) -> Result<Vec<CatalogItem>> {
        search_items_filtered(&self.db, filter)
    }

    // -------------------------------------------------------------------------
    // Entity Histories (Places, Establishments, Sects/Factions, Items, Texts, Chars)
    // -------------------------------------------------------------------------

    pub fn add_entity_history(
        &self,
        mut entry: EntityHistoryEntry,
    ) -> Result<(EntityHistoryEntry, WorldEvent)> {
        if entry.entity_id.trim().is_empty() {
            bail!("entity_id cannot be empty");
        }
        if entry.title.trim().is_empty() {
            bail!("History entry title cannot be empty");
        }
        // Resolve fuzzy character/place IDs if matching; otherwise slugify cleanly (e.g. sects/establishments/items/lore)
        entry.entity_id = resolve_character_id(&self.db, &entry.entity_id)
            .or_else(|_| resolve_place_id(&self.db, &entry.entity_id))
            .unwrap_or_else(|_| slugify(&entry.entity_id));
        if entry.entity_kind.trim().is_empty() {
            entry.entity_kind = "place".to_string();
        }
        if entry.visibility.trim().is_empty() {
            entry.visibility = "public".to_string();
        }
        entry.related_ids = entry.related_ids.into_iter().map(|s| slugify(&s)).collect();

        let meta = self.db.get_campaign_meta()?;
        entry.recorded_at_world_time = meta.clock.format_short();

        let hist_id = self.db.insert_entity_history(&entry)?;
        entry.id = hist_id;

        let mut affected = vec![entry.entity_id.clone()];
        affected.extend(entry.related_ids.clone());
        let summary = format!(
            "History recorded for {} [{}] ({}): {}",
            entry.entity_id,
            entry.entity_kind,
            if entry.era_or_date.is_empty() {
                "Undated"
            } else {
                &entry.era_or_date
            },
            entry.title
        );
        let ev = self.db.append_event_causal(
            None,
            affected,
            summary,
            EventPayload::EntityHistoryAdded {
                history_id: hist_id,
                entity_id: entry.entity_id.clone(),
                entity_kind: entry.entity_kind.clone(),
                era_or_date: entry.era_or_date.clone(),
                title: entry.title.clone(),
            },
            EventCausalMeta {
                actor_id: None,
                causal_event_ids: vec![],
                visibility: entry.visibility.clone(),
            },
        )?;
        self.maybe_compile()?;
        Ok((entry, ev))
    }

    pub fn search_histories_by(
        &self,
        filter: &HistorySearchFilter,
    ) -> Result<Vec<EntityHistoryEntry>> {
        search_histories_filtered(&self.db, filter)
    }

    // Internal helpers to keep carryable Books/Manuals/Records & CatalogItems synced with character inventories
    pub(crate) fn sync_catalog_item_to_character_inventories(
        &self,
        item: &CatalogItem,
        prev_holder: Option<&str>,
    ) -> Result<()> {
        // Remove from previous character holder if changed
        if let Some(old_h) = prev_holder {
            if item.current_holder_id.as_deref() != Some(old_h)
                && self.db.get_character_baseline(old_h)?.is_some()
            {
                let proj = load_character_projection(&self.db, old_h, None)?;
                if proj
                    .current
                    .equipment
                    .iter()
                    .any(|i| i.name.eq_ignore_ascii_case(&item.name))
                {
                    let _ = self.db.append_event(
                        proj.current.current_location.clone(),
                        vec![old_h.to_string(), item.id.clone()],
                        format!("{} no longer carries '{}'", proj.baseline.name, item.name),
                        EventPayload::ItemRemoved {
                            character_id: old_h.to_string(),
                            item_name: item.name.clone(),
                            quantity: item.quantity,
                            reason: "Catalog item transferred".to_string(),
                        },
                    )?;
                }
            }
        }

        // Ensure current character holder has it in their carried/equipped inventory
        if let Some(ref cur_h) = item.current_holder_id {
            if self.db.get_character_baseline(cur_h)?.is_some() {
                let proj = load_character_projection(&self.db, cur_h, None)?;
                let already_has = proj
                    .current
                    .equipment
                    .iter()
                    .any(|i| i.name.eq_ignore_ascii_case(&item.name));
                if !already_has {
                    let mut tags = item.tags.clone();
                    if !tags.iter().any(|t| t.eq_ignore_ascii_case(&item.item_type)) {
                        tags.push(item.item_type.clone());
                    }
                    let inv_item = Item {
                        name: item.name.clone(),
                        slot: item.slot.clone(),
                        quantity: item.quantity,
                        equipped: item.equipped,
                        durability: Some(item.condition_or_durability.clone()),
                        tags,
                        description: if item.description.is_empty() {
                            item.significance.clone()
                        } else {
                            item.description.clone()
                        },
                    };
                    let _ = self.db.append_event(
                        proj.current.current_location.clone(),
                        vec![cur_h.clone(), item.id.clone()],
                        format!("{} carries '{}' [{}]", proj.baseline.name, item.name, item.item_type),
                        EventPayload::ItemAcquired {
                            character_id: cur_h.clone(),
                            item: inv_item,
                        },
                    )?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn sync_text_as_carryable_item(&self, text: &StoryText) -> Result<()> {
        let existing = self.db.get_catalog_item(&text.id)?;
        let prev_holder = existing.as_ref().and_then(|e| e.current_holder_id.clone());
        let mut item = existing.unwrap_or_else(|| CatalogItem {
            id: text.id.clone(),
            name: text.title.clone(),
            item_type: text.medium.clone(),
            rarity_or_tier: String::new(),
            slot: None,
            quantity: 1,
            equipped: false,
            condition_or_durability: text.condition.clone(),
            origin_and_maker: if text.author.is_empty() {
                String::new()
            } else {
                format!("Authored by {}", text.author)
            },
            previous_owners: Vec::new(),
            current_holder_id: text.holder_or_location_id.clone(),
            significance: text.summary.clone(),
            properties_and_effects: text.mechanics_or_unlocks.join("; "),
            description: format!("{} ({}, {})", text.summary, text.medium, text.language),
            linked_text_id: Some(text.id.clone()),
            gm_notes: text.gm_notes.clone(),
            keywords: text.keywords.clone(),
            tags: text.tags.clone(),
            related_ids: text.related_ids.clone(),
            created_at_world_time: text.created_at_world_time.clone(),
            updated_at_world_time: text.updated_at_world_time.clone(),
        });

        item.name = text.title.clone();
        item.item_type = text.medium.clone();
        item.condition_or_durability = text.condition.clone();
        item.current_holder_id = text.holder_or_location_id.clone();
        item.linked_text_id = Some(text.id.clone());
        item.updated_at_world_time = text.updated_at_world_time.clone();
        if item.significance.is_empty() {
            item.significance = text.summary.clone();
        }
        self.db.upsert_catalog_item(&item)?;
        self.sync_catalog_item_to_character_inventories(&item, prev_holder.as_deref())?;
        Ok(())
    }

    fn sync_text_holder_transfer(
        &self,
        text: &StoryText,
        prev_holder: Option<&str>,
        new_holder: &str,
    ) -> Result<()> {
        if self.db.get_catalog_item(&text.id)?.is_none() {
            self.sync_text_as_carryable_item(text)?;
        }
        if let Some(mut item) = self.db.get_catalog_item(&text.id)? {
            if let Some(old_h) = prev_holder {
                if old_h != new_holder && !item.previous_owners.iter().any(|p| p == old_h) {
                    item.previous_owners.push(old_h.to_string());
                }
            }
            item.current_holder_id = Some(new_holder.to_string());
            item.condition_or_durability = text.condition.clone();
            item.updated_at_world_time = text.updated_at_world_time.clone();
            self.db.upsert_catalog_item(&item)?;
            self.sync_catalog_item_to_character_inventories(&item, prev_holder)?;
        }
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Knowledge Graph Relations
    // -------------------------------------------------------------------------

    pub fn upsert_relation(&self, mut rel: GraphRelation) -> Result<WorldEvent> {
        rel.source_id = slugify(&rel.source_id);
        rel.target_id = slugify(&rel.target_id);
        let meta = self.db.get_campaign_meta()?;
        rel.updated_at_world_time = meta.clock.format_short();
        self.db.upsert_relation(&rel)?;

        let summary = format!(
            "Relation: {} --[{}]--> {}{}",
            rel.source_id,
            rel.relation_type,
            rel.target_id,
            if rel.notes.is_empty() {
                String::new()
            } else {
                format!(" ({})", rel.notes)
            }
        );
        let rel_vis = rel.visibility.clone();
        let ev = self.db.append_event_causal(
            None,
            vec![rel.source_id.clone(), rel.target_id.clone()],
            summary,
            EventPayload::RelationUpserted { relation: rel },
            EventCausalMeta {
                actor_id: None,
                causal_event_ids: vec![],
                visibility: rel_vis,
            },
        )?;
        self.maybe_compile()?;
        Ok(ev)
    }

    pub fn remove_relation(
        &self,
        source_id: &str,
        relation_type: &str,
        target_id: &str,
        reason: &str,
    ) -> Result<WorldEvent> {
        let s = slugify(source_id);
        let t = slugify(target_id);
        self.db.delete_relation(&s, relation_type, &t)?;
        let ev = self.db.append_event(
            None,
            vec![s.clone(), t.clone()],
            format!("Relation ended: {s} --[{relation_type}]--> {t} ({reason})"),
            EventPayload::RelationRemoved {
                source_id: s,
                relation_type: relation_type.to_string(),
                target_id: t,
                reason: reason.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok(ev)
    }

    // -------------------------------------------------------------------------
    // Progress Clocks / Quests / Threats
    // -------------------------------------------------------------------------

    pub fn create_clock(&self, mut clock: ProgressClock) -> Result<(ProgressClock, WorldEvent)> {
        if clock.id.trim().is_empty() {
            clock.id = slugify(&clock.name);
        } else {
            clock.id = slugify(&clock.id);
        }
        clock.segments = clock.segments.max(1);
        clock.filled = clock.filled.clamp(0, clock.segments);
        clock.related_ids = clock.related_ids.into_iter().map(|s| slugify(&s)).collect();

        self.db.upsert_clock(&clock)?;
        let summary = format!(
            "Clock created: '{}' ({}/{}, {})",
            clock.name, clock.filled, clock.segments, clock.kind
        );
        let ev = self.db.append_event(
            None,
            clock.related_ids.clone(),
            summary,
            EventPayload::ClockCreated {
                clock: clock.clone(),
            },
        )?;
        self.maybe_compile()?;
        Ok((clock, ev))
    }

    pub fn tick_clock(
        &self,
        clock_id: &str,
        delta: i64,
        set_status: Option<String>,
        reason: &str,
    ) -> Result<(ProgressClock, WorldEvent)> {
        let id = slugify(clock_id);
        let mut clock = self
            .db
            .get_clock(&id)?
            .ok_or_else(|| anyhow!("Clock '{id}' not found"))?;
        clock.filled = (clock.filled + delta).clamp(0, clock.segments);
        if let Some(ref st) = set_status {
            clock.status = st.clone();
        } else if clock.filled >= clock.segments {
            clock.status = "completed".to_string();
        }
        self.db.upsert_clock(&clock)?;

        let summary = format!(
            "Clock '{}' ticked {:+} -> {}/{} [{}]{}",
            clock.name,
            delta,
            clock.filled,
            clock.segments,
            clock.status,
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );
        let ev = self.db.append_event(
            None,
            clock.related_ids.clone(),
            summary,
            EventPayload::ClockTicked {
                clock_id: id,
                delta,
                set_status,
                reason: reason.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((clock, ev))
    }

    // -------------------------------------------------------------------------
    // Narrative Events, Dice Rolling, Scene Context & Time-Travel Rollback
    // -------------------------------------------------------------------------

    pub fn log_narrative_event(
        &self,
        title: &str,
        details: &str,
        location_id: Option<String>,
        affected_entities: Vec<String>,
        tags: Vec<String>,
    ) -> Result<WorldEvent> {
        let loc = location_id.map(|s| slugify(&s));
        let affected: Vec<String> = affected_entities.into_iter().map(|s| slugify(&s)).collect();
        let summary = if details.is_empty() {
            title.to_string()
        } else {
            format!("{title}: {details}")
        };
        let ev = self.db.append_event(
            loc,
            affected,
            summary,
            EventPayload::NarrativeEvent {
                title: title.to_string(),
                details: details.to_string(),
                tags,
            },
        )?;
        self.maybe_compile()?;
        Ok(ev)
    }

    pub fn roll_and_optionally_log(
        &self,
        expression: &str,
        actor_id: Option<String>,
        location_id: Option<String>,
        purpose: &str,
        log_to_ledger: bool,
        seed: Option<u64>,
    ) -> Result<(DiceRollResult, Option<WorldEvent>)> {
        let res = roll_dice(expression, seed)?;
        let ev = if log_to_ledger {
            let actor = actor_id.map(|s| slugify(&s));
            let loc = location_id.map(|s| slugify(&s));
            let mut affected = Vec::new();
            if let Some(ref a) = actor {
                affected.push(a.clone());
            }
            let summary = format!(
                "Roll {} = {} (rolls: {:?}, kept: {:?}, mod: {:+}){}",
                res.expression,
                res.total,
                res.rolls,
                res.kept,
                res.modifier,
                if purpose.is_empty() {
                    String::new()
                } else {
                    format!(" — {purpose}")
                }
            );
            let e = self.db.append_event(
                loc,
                affected,
                summary,
                EventPayload::DiceRolled {
                    expression: res.expression.clone(),
                    rolls: res.rolls.clone(),
                    kept: res.kept.clone(),
                    modifier: res.modifier,
                    total: res.total,
                    actor_id: actor,
                    purpose: purpose.to_string(),
                },
            )?;
            self.maybe_compile()?;
            Some(e)
        } else {
            None
        };
        Ok((res, ev))
    }

    pub fn get_scene_context(&self, req: &SceneContextRequest) -> Result<SceneContextPacket> {
        assemble_scene_context(&self.db, req)
    }

    pub fn rollback_to_seq(&self, to_seq: i64) -> Result<(PathBuf, CompileReport)> {
        let backup = self.db.rollback_to_seq(to_seq)?;
        let report = self.compile_docs()?;
        let _ = prune_orphaned_docs(&self.db, &self.docs_dir())?;
        Ok((backup, report))
    }

    // -------------------------------------------------------------------------
    // Rich RPG "Search By" Queries
    // -------------------------------------------------------------------------

    pub fn search_characters_by(
        &self,
        filter: &CharacterSearchFilter,
    ) -> Result<Vec<CharacterProjection>> {
        search_characters(&self.db, filter)
    }

    pub fn search_places_by(&self, filter: &PlaceSearchFilter) -> Result<Vec<PlaceProjection>> {
        search_places(&self.db, filter)
    }

    pub fn search_lore_by(&self, filter: &LoreSearchFilter) -> Result<Vec<LoreEntry>> {
        search_lore_filtered(&self.db, filter)
    }

    pub fn search_events_by(&self, filter: &EventSearchFilter) -> Result<Vec<WorldEvent>> {
        search_events_filtered(&self.db, filter)
    }

    pub fn search_relations_by(
        &self,
        filter: &RelationSearchFilter,
    ) -> Result<Vec<GraphRelation>> {
        search_relations_filtered(&self.db, filter)
    }

    pub fn search_omni(&self, query: &str, limit_per_kind: usize) -> Result<OmniSearchResult> {
        search_all(&self.db, query, limit_per_kind)
    }

    // -------------------------------------------------------------------------
    // Tailor-Made RPG Helpers: Atomic Item Transfer & Party Rest
    // -------------------------------------------------------------------------

    /// Atomically transfers an item from one character to another, verifying ownership first.
    pub fn transfer_item(
        &self,
        from_character_id: &str,
        to_character_id: &str,
        item_name: &str,
        quantity: i64,
        reason: &str,
    ) -> Result<(WorldEvent, WorldEvent)> {
        if quantity <= 0 {
            bail!("Transfer quantity must be positive (got {quantity})");
        }
        let from_id = resolve_character_id(&self.db, from_character_id)?;
        let to_id = resolve_character_id(&self.db, to_character_id)?;
        if from_id == to_id {
            bail!("Source and destination character cannot be the same ('{from_id}')");
        }

        let from_proj = load_character_projection(&self.db, &from_id, None)?;
        let to_proj = load_character_projection(&self.db, &to_id, None)?;

        let existing = from_proj
            .current
            .equipment
            .iter()
            .find(|i| i.name.eq_ignore_ascii_case(item_name))
            .cloned()
            .ok_or_else(|| {
                anyhow!(
                    "{} does not possess item '{}'",
                    from_proj.baseline.name,
                    item_name
                )
            })?;

        if existing.quantity < quantity {
            bail!(
                "{} only has {}x '{}', cannot transfer {}",
                from_proj.baseline.name,
                existing.quantity,
                existing.name,
                quantity
            );
        }

        let reason_from = if reason.is_empty() {
            format!("Transferred to {}", to_proj.baseline.name)
        } else {
            format!("Transferred to {} ({reason})", to_proj.baseline.name)
        };
        let reason_to = if reason.is_empty() {
            format!("Received from {}", from_proj.baseline.name)
        } else {
            format!("Received from {} ({reason})", from_proj.baseline.name)
        };

        let (_, ev_rem) = self.remove_item(&from_id, &existing.name, quantity, &reason_from)?;
        let transferred_item = Item {
            name: existing.name,
            slot: existing.slot,
            quantity,
            equipped: false,
            durability: existing.durability,
            tags: existing.tags,
            description: existing.description,
        };
        let (_, ev_add) = self.add_item(&to_id, transferred_item, &reason_to)?;
        Ok((ev_rem, ev_add))
    }

    /// Performs a Short Rest or Long Rest for one or more characters:
    /// optionally advances world time, restores resources (full or percentage), and clears named conditions.
    pub fn rest_characters(
        &self,
        character_ids: &[String],
        rest_type: &str,
        advance_minutes: Option<u64>,
        restore_percent: u8,
        resources_filter: &[String],
        clear_conditions: &[String],
        notes: &str,
    ) -> Result<Vec<String>> {
        let mut summaries = Vec::new();
        let minutes = advance_minutes.unwrap_or_else(|| {
            if rest_type.eq_ignore_ascii_case("short") {
                60
            } else {
                480
            }
        });

        if minutes > 0 {
            let reason = if notes.is_empty() {
                format!("{rest_type} rest")
            } else {
                format!("{rest_type} rest — {notes}")
            };
            let (_, _, ev) = self.advance_time(minutes, None, Some(reason))?;
            summaries.push(ev.summary);
        }

        let pct = restore_percent.min(100) as f64 / 100.0;
        for raw_cid in character_ids {
            let cid = resolve_character_id(&self.db, raw_cid)?;
            let proj = load_character_projection(&self.db, &cid, None)?;

            for (res_name, pool) in &proj.current.resources {
                if !resources_filter.is_empty()
                    && !resources_filter
                        .iter()
                        .any(|rf| rf.eq_ignore_ascii_case(res_name))
                {
                    continue;
                }
                if pool.current < pool.max {
                    let target = if restore_percent >= 100 {
                        pool.max
                    } else {
                        let gain = ((pool.max as f64) * pct).ceil() as i64;
                        (pool.current + gain).min(pool.max)
                    };
                    if target > pool.current {
                        let (_, ev) = self.modify_resource(
                            &cid,
                            res_name,
                            None,
                            Some(target),
                            None,
                            None,
                            &format!("{rest_type} rest recovery"),
                        )?;
                        summaries.push(ev.summary);
                    }
                }
            }

            for cond_name in clear_conditions {
                if proj
                    .current
                    .conditions
                    .iter()
                    .any(|c| c.name.eq_ignore_ascii_case(cond_name))
                {
                    let (_, ev) = self.remove_condition(
                        &cid,
                        cond_name,
                        &format!("Cleared during {rest_type} rest"),
                    )?;
                    summaries.push(ev.summary);
                }
            }
        }

        Ok(summaries)
    }

    // -------------------------------------------------------------------------
    // Phase 1: Causal Logging, Ruleset Profiles, Checks, Contests, Random Tables & Encounters
    // -------------------------------------------------------------------------

    pub fn log_narrative_event_causal(
        &self,
        title: &str,
        details: &str,
        location_id: Option<String>,
        affected_entities: Vec<String>,
        tags: Vec<String>,
        causal: EventCausalMeta,
    ) -> Result<WorldEvent> {
        let loc = match location_id {
            Some(l) if !l.trim().is_empty() => Some(resolve_place_id(&self.db, &l).unwrap_or_else(|_| slugify(&l))),
            _ => None,
        };
        let affected: Vec<String> = affected_entities
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .map(|s| slugify(&s))
            .collect();
        let summary = if details.is_empty() {
            title.to_string()
        } else {
            format!("{title}: {details}")
        };
        let ev = self.db.append_event_causal(
            loc,
            affected,
            summary,
            EventPayload::NarrativeEvent {
                title: title.to_string(),
                details: details.to_string(),
                tags,
            },
            causal,
        )?;
        self.maybe_compile()?;
        Ok(ev)
    }

    pub fn upsert_ruleset_profile(&self, mut profile: RulesetProfile) -> Result<(RulesetProfile, WorldEvent)> {
        if profile.name.trim().is_empty() {
            bail!("Ruleset profile name cannot be empty");
        }
        profile.id = if profile.id.trim().is_empty() {
            slugify(&profile.name)
        } else {
            slugify(&profile.id)
        };
        let valid_modes = ["roll_over", "roll_under", "dice_pool", "step_dice"];
        let mode_l = profile.resolution_mode.trim().to_lowercase();
        if !valid_modes.contains(&mode_l.as_str()) {
            bail!(
                "Invalid resolution_mode '{}'. Expected one of: roll_over, roll_under, dice_pool, step_dice.",
                profile.resolution_mode
            );
        }
        profile.resolution_mode = mode_l;
        // Validate base dice expression early
        let _ = roll_dice(&profile.base_dice, Some(1))
            .with_context(|| format!("Invalid base_dice '{}' in profile '{}'", profile.base_dice, profile.id))?;
        if let Some(ref adv) = profile.advantage_dice {
            let _ = roll_dice(adv, Some(1))
                .with_context(|| format!("Invalid advantage_dice '{adv}' in profile '{}'", profile.id))?;
        }
        if let Some(ref dis) = profile.disadvantage_dice {
            let _ = roll_dice(dis, Some(1))
                .with_context(|| format!("Invalid disadvantage_dice '{dis}' in profile '{}'", profile.id))?;
        }

        let existing = self.db.get_ruleset_profile(&profile.id)?;
        profile.version = existing.map(|e| e.version + 1).unwrap_or(1);
        let now = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_ruleset_profile(&profile, &now)?;

        let ev = self.db.append_event(
            None,
            vec![profile.id.clone()],
            format!(
                "Ruleset profile '{}' ({}, {}) v{} saved.",
                profile.name, profile.resolution_mode, profile.base_dice, profile.version
            ),
            EventPayload::RulesetProfileUpserted {
                profile_id: profile.id.clone(),
                resolution_mode: profile.resolution_mode.clone(),
                base_dice: profile.base_dice.clone(),
                version: profile.version,
            },
        )?;
        self.maybe_compile()?;
        Ok((profile, ev))
    }

    pub fn get_ruleset_profile_or_default(&self, profile_id: Option<&str>) -> Result<RulesetProfile> {
        let target = profile_id
            .filter(|s| !s.trim().is_empty())
            .map(slugify)
            .unwrap_or_else(|| "default".to_string());
        if let Some(p) = self.db.get_ruleset_profile(&target)? {
            return Ok(p);
        }
        if target != "default" {
            bail!(
                "Ruleset profile '{target}' not found. Define it via `upsert_ruleset_profile` or omit `profile` to use 'default'."
            );
        }
        Ok(RulesetProfile::default())
    }

    pub fn list_ruleset_profiles(&self) -> Result<Vec<RulesetProfile>> {
        self.db.list_ruleset_profiles()
    }

    /// Resolves a structured, system-agnostic action check with automatic live state hydration
    /// (stats, skills, techniques, equipment, character injuries/conditions, & live location weather/conditions).
    pub fn resolve_check(&self, mut req: CheckRequest) -> Result<CheckResolution> {
        if req.action.trim().is_empty() {
            bail!("Check `action` cannot be empty");
        }
        let profile = self.get_ruleset_profile_or_default(req.profile.as_deref())?;
        let hydrate = req.auto_hydrate.unwrap_or(profile.auto_hydrate);

        let (actor_proj, resolved_actor_id) = match req.actor_id.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(raw) => {
                let cid = resolve_character_id(&self.db, raw)?;
                let proj = load_character_projection(&self.db, &cid, None)?;
                (Some(proj), Some(cid))
            }
            None => (None, None),
        };
        req.actor_id = resolved_actor_id.clone();

        let place_proj = if let Some(ref ch) = actor_proj {
            if let Some(ref loc_id) = ch.current.current_location {
                load_place_projection(&self.db, loc_id, None).ok()
            } else {
                None
            }
        } else {
            None
        };
        let location_id = place_proj.as_ref().map(|p| p.baseline.id.clone());

        let hydrated = if hydrate {
            collect_hydrated_modifiers(
                &profile,
                actor_proj.as_ref(),
                place_proj.as_ref(),
                req.stat.as_deref(),
                req.skill.as_deref(),
                req.technique.as_deref(),
                &req.equipment,
                &req.action,
            )
        } else {
            HydrationOutcome::default()
        };

        if req.strict_validation && !hydrated.missing_refs.is_empty() {
            bail!(
                "Strict validation failed for check '{}': {}",
                req.action,
                hydrated.missing_refs.join(" | ")
            );
        }

        let mut res = evaluate_check(
            &profile,
            &req,
            hydrated.modifiers,
            hydrated.warnings,
            location_id.clone(),
        )?;

        if !req.dry_run && req.log_to_ledger {
            let mut affected = Vec::new();
            if let Some(ref cid) = resolved_actor_id {
                affected.push(cid.clone());
            }
            let actor_label = actor_proj
                .as_ref()
                .map(|c| c.baseline.name.as_str())
                .unwrap_or("Check");
            let summary = format!(
                "{actor_label} — '{}' [{}]: {} (roll {}, mod {:+}, total {} vs DC {}, margin {:+})",
                res.action,
                res.profile_id,
                res.outcome.to_uppercase(),
                res.roll,
                res.modifier,
                res.total,
                res.difficulty,
                res.margin
            );
            let ev = self.db.append_event_causal(
                location_id,
                affected,
                summary,
                EventPayload::CheckResolved {
                    actor_id: resolved_actor_id.clone(),
                    action: res.action.clone(),
                    profile_id: res.profile_id.clone(),
                    resolution_mode: res.resolution_mode.clone(),
                    expression: res.expression.clone(),
                    seed: res.seed,
                    roll: res.roll,
                    modifier: res.modifier,
                    total: res.total,
                    difficulty: res.difficulty,
                    margin: res.margin,
                    outcome: res.outcome.clone(),
                    stakes: res.stakes.clone(),
                    modifiers_json: serde_json::to_value(&res.modifiers).unwrap_or_default(),
                },
                EventCausalMeta {
                    actor_id: resolved_actor_id,
                    causal_event_ids: req.causal_event_ids,
                    visibility: req.visibility,
                },
            )?;
            res.event_id = Some(ev.seq);
            self.maybe_compile()?;
        }

        Ok(res)
    }

    /// Resolves a contested / opposed check between 2+ participants with deterministic derived seeds
    /// and auto-hydrated character/environment modifiers.
    pub fn resolve_contest(&self, req: ContestRequest) -> Result<ContestResolution> {
        if req.action.trim().is_empty() {
            bail!("Contest `action` cannot be empty");
        }
        if req.participants.len() < 2 {
            bail!("resolve_contest requires at least 2 participants");
        }
        let profile = self.get_ruleset_profile_or_default(req.profile.as_deref())?;
        let base_seed = req.seed.unwrap_or_else(|| {
            use rand::Rng;
            rand::thread_rng().gen::<u64>()
        });

        let mut results = Vec::with_capacity(req.participants.len());
        let mut affected = Vec::new();
        let mut shared_loc: Option<String> = None;
        let mut all_warnings: Vec<String> = Vec::new();

        for (idx, p) in req.participants.iter().enumerate() {
            let cid = resolve_character_id(&self.db, &p.actor_id)?;
            let proj = load_character_projection(&self.db, &cid, None)?;
            let place = proj
                .current
                .current_location
                .as_deref()
                .and_then(|lid| load_place_projection(&self.db, lid, None).ok());
            let loc_id = place.as_ref().map(|pl| pl.baseline.id.clone());
            if shared_loc.is_none() {
                shared_loc = loc_id.clone();
            }
            affected.push(cid.clone());

            let hydrate = p.auto_hydrate.unwrap_or(profile.auto_hydrate);
            let hydrated = if hydrate {
                collect_hydrated_modifiers(
                    &profile,
                    Some(&proj),
                    place.as_ref(),
                    p.stat.as_deref(),
                    p.skill.as_deref(),
                    p.technique.as_deref(),
                    &p.equipment,
                    &req.action,
                )
            } else {
                HydrationOutcome::default()
            };

            if req.strict_validation && !hydrated.missing_refs.is_empty() {
                bail!(
                    "Strict validation failed for contest '{}' participant '{}': {}",
                    req.action,
                    cid,
                    hydrated.missing_refs.join(" | ")
                );
            }
            all_warnings.extend(hydrated.warnings.clone());

            let sub_seed = base_seed.wrapping_add((idx as u64).wrapping_mul(0x9E37_79B9));
            let single_req = CheckRequest {
                actor_id: Some(cid.clone()),
                action: req.action.clone(),
                profile: Some(profile.id.clone()),
                stat: p.stat.clone(),
                skill: p.skill.clone(),
                technique: p.technique.clone(),
                equipment: p.equipment.clone(),
                difficulty: 0,
                modifier: p.modifier,
                situational_modifiers: p.situational_modifiers.clone(),
                advantage: p.advantage.clone(),
                dice_override: p.dice_override.clone(),
                auto_hydrate: Some(hydrate),
                stakes: req.stakes.clone(),
                seed: Some(sub_seed),
                dry_run: false,
                strict_validation: req.strict_validation,
                log_to_ledger: false,
                causal_event_ids: vec![],
                visibility: req.visibility.clone(),
            };
            let checked = evaluate_check(
                &profile,
                &single_req,
                hydrated.modifiers,
                hydrated.warnings,
                loc_id.clone(),
            )?;
            let role = if p.role.trim().is_empty() {
                if idx == 0 {
                    "initiator".to_string()
                } else if idx == 1 {
                    "defender".to_string()
                } else {
                    format!("participant-{}", idx + 1)
                }
            } else {
                p.role.clone()
            };
            results.push(ContestParticipantResult {
                actor_id: cid,
                role,
                expression: checked.expression,
                advantage: checked.advantage,
                seed: checked.seed,
                rolls: checked.rolls,
                kept: checked.kept,
                roll: checked.roll,
                modifiers: checked.modifiers,
                modifier: checked.modifier,
                total: checked.total,
                location_id: loc_id,
                warnings: checked.warnings,
            });
        }

        // Determine winner (higher total wins for roll_over/dice_pool/step_dice; lower roll_under wins if roll_under)
        let lower_wins = profile.resolution_mode == "roll_under";
        let mut ranked: Vec<usize> = (0..results.len()).collect();
        ranked.sort_by(|&a, &b| {
            let cmp_total = if lower_wins {
                results[a].total.cmp(&results[b].total)
            } else {
                results[b].total.cmp(&results[a].total)
            };
            cmp_total
        });

        let top_idx = ranked[0];
        let runner_idx = ranked[1];
        let raw_margin = if lower_wins {
            results[runner_idx].total - results[top_idx].total
        } else {
            results[top_idx].total - results[runner_idx].total
        };

        let tb = req.tie_breaker.trim().to_lowercase();
        let (winner_idx, is_tie) = if raw_margin > 0 {
            (Some(top_idx), false)
        } else {
            match tb.as_str() {
                "defender" => {
                    let def = results
                        .iter()
                        .position(|r| r.role.eq_ignore_ascii_case("defender"))
                        .unwrap_or(1);
                    (Some(def), true)
                }
                "higher_modifier" => {
                    if results[top_idx].modifier > results[runner_idx].modifier {
                        (Some(top_idx), true)
                    } else if results[runner_idx].modifier > results[top_idx].modifier {
                        (Some(runner_idx), true)
                    } else {
                        (None, true)
                    }
                }
                _ => (None, true),
            }
        };

        let winner_id = winner_idx.map(|i| results[i].actor_id.clone());
        let winner_role = winner_idx.map(|i| results[i].role.clone());

        let mut resolution = ContestResolution {
            action: req.action.clone(),
            profile_id: profile.id.clone(),
            resolution_mode: profile.resolution_mode.clone(),
            seed: base_seed,
            participants: results,
            winner_id: winner_id.clone(),
            winner_role,
            margin: raw_margin,
            is_tie,
            tie_breaker_used: tb,
            stakes: req.stakes.clone(),
            warnings: all_warnings,
            event_id: None,
        };

        if req.log_to_ledger {
            let summary = format!(
                "Contest '{}' [{}]: winner={} (margin {}){}",
                resolution.action,
                resolution.profile_id,
                winner_id.as_deref().unwrap_or("tie/draw"),
                resolution.margin,
                if resolution.stakes.is_empty() {
                    String::new()
                } else {
                    format!(" — stakes: {}", resolution.stakes)
                }
            );
            let ev = self.db.append_event_causal(
                shared_loc,
                affected,
                summary,
                EventPayload::ContestResolved {
                    action: resolution.action.clone(),
                    profile_id: resolution.profile_id.clone(),
                    seed: resolution.seed,
                    winner_id: resolution.winner_id.clone(),
                    margin: resolution.margin,
                    is_tie: resolution.is_tie,
                    stakes: resolution.stakes.clone(),
                    participants_json: serde_json::to_value(&resolution.participants)
                        .unwrap_or_default(),
                },
                EventCausalMeta {
                    actor_id: resolution.participants.first().map(|p| p.actor_id.clone()),
                    causal_event_ids: req.causal_event_ids,
                    visibility: req.visibility,
                },
            )?;
            resolution.event_id = Some(ev.seq);
            self.maybe_compile()?;
        }

        Ok(resolution)
    }

    /// Creates or updates a versioned random table (weighted, dice-range, nested, conditional, or loot).
    pub fn upsert_random_table(&self, table: RandomTable) -> Result<(RandomTable, WorldEvent)> {
        let mut normalized = normalize_random_table(table)?;
        let now = self.db.get_campaign_meta()?.clock.format_short();
        if let Some(existing) = self.db.get_random_table(&normalized.id)? {
            normalized.version = existing.version + 1;
            normalized.created_at_world_time = existing.created_at_world_time;
        } else {
            normalized.version = 1;
            normalized.created_at_world_time = now.clone();
        }
        normalized.updated_at_world_time = now;
        self.db.upsert_random_table(&normalized)?;

        let ev = self.db.append_event(
            None,
            vec![normalized.id.clone()],
            format!(
                "Random table '{}' [{}] v{} saved ({} entries).",
                normalized.name,
                normalized.category,
                normalized.version,
                normalized.entries.len()
            ),
            EventPayload::RandomTableUpserted {
                table_id: normalized.id.clone(),
                name: normalized.name.clone(),
                category: normalized.category.clone(),
                version: normalized.version,
                entry_count: normalized.entries.len(),
            },
        )?;
        self.maybe_compile()?;
        Ok((normalized, ev))
    }

    pub fn get_random_table(&self, table_id: &str) -> Result<RandomTable> {
        let id = slugify(table_id);
        self.db
            .get_random_table(&id)?
            .ok_or_else(|| anyhow!("Random table '{id}' not found"))
    }

    pub fn list_random_tables(&self) -> Result<Vec<RandomTable>> {
        self.db.list_random_tables()
    }

    /// Draws from a persistent random table with context filtering, stateful without-replacement decks,
    /// seeded repeatability, nested sub-tables, and automatic Staged Discovery Receipts for loot tables.
    pub fn draw_random_table(&self, mut req: DrawTableRequest) -> Result<DrawTableOutcome> {
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};

        let table = self.get_random_table(&req.table_id)?;
        let meta = self.db.get_campaign_meta()?;
        let now = meta.clock.format_short();

        // Auto-hydrate missing DrawContext fields from live campaign clock & place state!
        if req.context.season.is_none() {
            req.context.season = Some(meta.clock.season.clone());
        }
        let resolved_loc = match req.context.location.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(raw) => {
                let pid = resolve_place_id(&self.db, raw).unwrap_or_else(|_| slugify(raw));
                if let Ok(pl) = load_place_projection(&self.db, &pid, None) {
                    if req.context.weather.is_none() && !pl.current.current_weather.is_empty() {
                        req.context.weather = Some(pl.current.current_weather.clone());
                    }
                    if req.context.place_type.is_none() && !pl.baseline.place_type.is_empty() {
                        req.context.place_type = Some(pl.baseline.place_type.clone());
                    }
                    for tag in &pl.baseline.tags {
                        if !req.context.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
                            req.context.tags.push(tag.clone());
                        }
                    }
                }
                req.context.location = Some(pid.clone());
                Some(pid)
            }
            None => None,
        };
        if req.context.weather.is_none() && !meta.clock.global_weather.is_empty() {
            req.context.weather = Some(meta.clock.global_weather.clone());
        }

        let deck_scope = req
            .deck_scope
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| resolved_loc.clone())
            .unwrap_or_else(|| "global".to_string());

        let mut drawn_ids = if req.reset_deck {
            std::collections::BTreeSet::new()
        } else {
            self.db.get_table_drawn_ids(&table.id, &deck_scope)?
        };

        let seed = req.seed.unwrap_or_else(|| rand::thread_rng().gen::<u64>());
        let mut rng = StdRng::seed_from_u64(seed);
        let mut visited = std::collections::BTreeSet::new();

        let (eligible_count, draws) = execute_table_draw(
            &table,
            &req.context,
            req.count.max(1),
            req.replacement,
            &mut drawn_ids,
            &mut rng,
            0,
            &mut visited,
            &|sub_id| self.db.get_random_table(&slugify(sub_id)),
        )?;

        if !req.replacement || req.reset_deck {
            self.db
                .set_table_drawn_ids(&table.id, &deck_scope, &drawn_ids, &now)?;
        }

        // Stage loot/discovery receipt without auto-granting any item to any character!
        let should_stage = req
            .stage_discovery
            .unwrap_or(table.is_loot_table || table.category.eq_ignore_ascii_case("loot"));
        let mut staged_items: Vec<StagedDiscoveryItem> = Vec::new();
        if should_stage {
            for d in &draws {
                if let Some(ref li) = d.loot_item {
                    let mut item = li.clone();
                    item.index = staged_items.len();
                    staged_items.push(item);
                }
                for sub in &d.subtable_results {
                    if let Some(ref li) = sub.loot_item {
                        let mut item = li.clone();
                        item.index = staged_items.len();
                        staged_items.push(item);
                    }
                }
            }
        }

        let mut staged_discovery = if should_stage && !staged_items.is_empty() {
            let disc_id = format!("disc-{}-{:06x}", table.id, (seed & 0xFF_FFFF));
            Some(StagedDiscovery {
                id: disc_id,
                source_table_id: table.id.clone(),
                table_version: table.version,
                location_id: resolved_loc.clone(),
                source_description: req
                    .context
                    .source_description
                    .clone()
                    .unwrap_or_else(|| format!("Drawn from table '{}'", table.name)),
                seed,
                status: "pending".to_string(),
                items: staged_items,
                created_at_world_time: now,
                draw_event_id: None,
            })
        } else {
            None
        };

        let event_id = if req.log_to_ledger {
            let mut affected = vec![table.id.clone()];
            for p in &req.context.party {
                affected.push(slugify(p));
            }
            if let Some(ref disc) = staged_discovery {
                affected.push(disc.id.clone());
            }
            let result_labels: Vec<String> = draws.iter().map(|d| d.result.clone()).collect();
            let summary = format!(
                "Drew {}x from '{}' (v{}, seed {}): {}{}",
                draws.len(),
                table.name,
                table.version,
                seed,
                result_labels.join(" | "),
                staged_discovery
                    .as_ref()
                    .map(|d| format!(" [staged discovery: {} — NOT yet claimed]", d.id))
                    .unwrap_or_default()
            );
            let ev = self.db.append_event_causal(
                resolved_loc.clone(),
                affected,
                summary,
                EventPayload::RandomTableDrawn {
                    table_id: table.id.clone(),
                    table_version: table.version,
                    seed,
                    replacement: req.replacement,
                    count: draws.len(),
                    staged_discovery_id: staged_discovery.as_ref().map(|d| d.id.clone()),
                    results: result_labels,
                },
                EventCausalMeta {
                    actor_id: req.context.party.first().map(|s| slugify(s)),
                    causal_event_ids: req.causal_event_ids,
                    visibility: "public".to_string(),
                },
            )?;
            Some(ev.seq)
        } else {
            None
        };

        if let Some(ref mut disc) = staged_discovery {
            disc.draw_event_id = event_id;
            self.db.upsert_staged_discovery(disc)?;
        }

        self.maybe_compile()?;
        Ok(DrawTableOutcome {
            table_id: table.id,
            table_name: table.name,
            table_version: table.version,
            seed,
            replacement: req.replacement,
            deck_scope,
            eligible_entry_count: eligible_count,
            draws,
            staged_discovery,
            event_id,
        })
    }

    /// Atomically claims selected items from a Staged Discovery Receipt into a character's inventory
    /// (and optionally creates full carryable `CatalogItem`s with provenance). Prevents double-claiming!
    pub fn claim_discovery(
        &self,
        discovery_id: &str,
        claimant_id: &str,
        item_indices: Option<Vec<usize>>,
        create_catalog_items: bool,
        notes: &str,
    ) -> Result<(StagedDiscovery, Vec<String>, WorldEvent)> {
        let mut disc = self
            .db
            .get_staged_discovery(discovery_id)?
            .ok_or_else(|| anyhow!("Staged discovery '{discovery_id}' not found"))?;
        let cid = resolve_character_id(&self.db, claimant_id)?;
        let char_proj = load_character_projection(&self.db, &cid, None)?;

        let target_indices: Vec<usize> = match item_indices {
            Some(idxs) if !idxs.is_empty() => idxs,
            _ => disc
                .items
                .iter()
                .filter(|it| it.claimed_by.is_none())
                .map(|it| it.index)
                .collect(),
        };

        if target_indices.is_empty() {
            bail!("No unclaimed items remain in discovery '{discovery_id}'");
        }

        // Validate ALL indices before mutating anything (atomic guard — no partial duplication!)
        for &idx in &target_indices {
            let Some(item) = disc.items.iter().find(|it| it.index == idx) else {
                bail!("Invalid item index {idx} in discovery '{discovery_id}'");
            };
            if let Some(ref prev) = item.claimed_by {
                bail!(
                    "Item #{idx} ('{}') in discovery '{discovery_id}' was already claimed by '{prev}'",
                    item.name
                );
            }
        }

        let mut claimed_names = Vec::new();
        for &idx in &target_indices {
            if let Some(item) = disc.items.iter_mut().find(|it| it.index == idx) {
                item.claimed_by = Some(cid.clone());
                claimed_names.push(format!("{} (x{})", item.name, item.quantity));

                if create_catalog_items {
                    let item_slug = slugify(&format!("{}-{}", item.name, disc.id));
                    let cat = CatalogItem {
                        id: item_slug,
                        name: item.name.clone(),
                        item_type: item.item_type.clone(),
                        rarity_or_tier: item.rarity_or_tier.clone(),
                        slot: None,
                        quantity: item.quantity,
                        equipped: false,
                        condition_or_durability: item.condition_or_durability.clone(),
                        origin_and_maker: disc.source_description.clone(),
                        previous_owners: disc
                            .location_id
                            .clone()
                            .into_iter()
                            .collect(),
                        current_holder_id: Some(cid.clone()),
                        significance: format!(
                            "Claimed from staged discovery '{}' (table '{}', seed {}).",
                            disc.id, disc.source_table_id, disc.seed
                        ),
                        properties_and_effects: item.properties_and_effects.clone(),
                        description: item.description.clone(),
                        linked_text_id: None,
                        gm_notes: String::new(),
                        keywords: item.tags.clone(),
                        tags: item.tags.clone(),
                        related_ids: disc.location_id.clone().into_iter().collect(),
                        created_at_world_time: String::new(),
                        updated_at_world_time: String::new(),
                    };
                    let _ = self.upsert_catalog_item(cat, &format!("Claimed from {}", disc.id))?;
                } else {
                    let inv_item = Item {
                        name: item.name.clone(),
                        slot: None,
                        quantity: item.quantity,
                        equipped: false,
                        durability: Some(item.condition_or_durability.clone()),
                        tags: item.tags.clone(),
                        description: item.description.clone(),
                    };
                    let _ = self.add_item(
                        &cid,
                        inv_item,
                        &format!("Claimed from {} ({})", disc.source_description, disc.id),
                    )?;
                }
            }
        }

        let all_claimed = disc.items.iter().all(|it| it.claimed_by.is_some());
        disc.status = if all_claimed {
            "claimed".to_string()
        } else {
            "partially_claimed".to_string()
        };
        self.db.upsert_staged_discovery(&disc)?;

        let causal_parents: Vec<i64> = disc.draw_event_id.into_iter().collect();
        let ev = self.db.append_event_causal(
            disc.location_id.clone().or(char_proj.current.current_location),
            vec![cid.clone(), disc.id.clone()],
            format!(
                "{} claimed {} from '{}' ({}){}",
                char_proj.baseline.name,
                claimed_names.join(", "),
                disc.source_description,
                disc.id,
                if notes.is_empty() {
                    String::new()
                } else {
                    format!(" — {notes}")
                }
            ),
            EventPayload::DiscoveryClaimed {
                discovery_id: disc.id.clone(),
                claimant_id: cid.clone(),
                claimed_items: claimed_names.clone(),
                notes: notes.to_string(),
            },
            EventCausalMeta {
                actor_id: Some(cid),
                causal_event_ids: causal_parents,
                visibility: "public".to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((disc, claimed_names, ev))
    }

    pub fn list_staged_discoveries(&self, status_filter: Option<&str>) -> Result<Vec<StagedDiscovery>> {
        self.db.list_staged_discoveries(status_filter)
    }

    // -------------------------------------------------------------------------
    // Encounters & Initiative Management (Combat + Non-Combat Scenes)
    // -------------------------------------------------------------------------

    pub fn start_encounter(
        &self,
        id: Option<String>,
        title: &str,
        encounter_type: &str,
        location_id: Option<String>,
        mut participants: Vec<EncounterParticipant>,
        auto_roll_initiative_dice: Option<&str>,
        zones: Vec<String>,
        environmental_notes: &str,
        seed: Option<u64>,
    ) -> Result<(EncounterState, WorldEvent)> {
        use rand::Rng;
        if title.trim().is_empty() {
            bail!("Encounter title cannot be empty");
        }
        if participants.is_empty() {
            bail!("Encounter must have at least one participant");
        }
        let enc_id = match id.filter(|s| !s.trim().is_empty()) {
            Some(s) => slugify(&s),
            None => slugify(title),
        };
        let resolved_loc = match location_id.filter(|s| !s.trim().is_empty()) {
            Some(l) => Some(resolve_place_id(&self.db, &l).unwrap_or_else(|_| slugify(&l))),
            None => None,
        };
        let base_seed = seed.unwrap_or_else(|| rand::thread_rng().gen::<u64>());

        for (idx, p) in participants.iter_mut().enumerate() {
            if let Ok(cid) = resolve_character_id(&self.db, &p.entity_id) {
                p.entity_id = cid.clone();
                if p.name.trim().is_empty() {
                    if let Ok(ch) = load_character_projection(&self.db, &cid, None) {
                        p.name = ch.baseline.name;
                    }
                }
            } else {
                p.entity_id = slugify(&p.entity_id);
                if p.name.trim().is_empty() {
                    p.name = p.entity_id.clone();
                }
            }
            if let Some(init_dice) = auto_roll_initiative_dice {
                if p.initiative_roll.is_none() {
                    let r = roll_dice(
                        init_dice,
                        Some(base_seed.wrapping_add((idx as u64).wrapping_mul(0x517C_C1B7))),
                    )?;
                    p.initiative_roll = Some(r.total);
                    p.initiative += r.total;
                }
            }
            p.reaction_available = true;
            if p.status.trim().is_empty() {
                p.status = "active".to_string();
            }
        }

        let now = self.db.get_campaign_meta()?.clock.format_short();
        let mut enc = EncounterState {
            id: enc_id.clone(),
            title: title.to_string(),
            encounter_type: if encounter_type.trim().is_empty() {
                "combat".to_string()
            } else {
                encounter_type.trim().to_lowercase()
            },
            location_id: resolved_loc.clone(),
            status: "active".to_string(),
            round: 1,
            active_turn_index: 0,
            zones,
            environmental_notes: environmental_notes.to_string(),
            participants,
            action_log: vec![],
            outcome: String::new(),
            aftermath: String::new(),
            started_at_world_time: now,
            ended_at_world_time: None,
        };
        enc.sort_initiative();
        self.db.upsert_encounter(&enc)?;

        let p_ids: Vec<String> = enc.participants.iter().map(|p| p.entity_id.clone()).collect();
        let mut affected = vec![enc.id.clone()];
        affected.extend(p_ids.clone());
        let ev = self.db.append_event(
            resolved_loc,
            affected,
            format!(
                "Encounter started: '{}' [{}] ({} participants; first up: {}).",
                enc.title,
                enc.encounter_type,
                enc.participants.len(),
                enc.active_participant()
                    .map(|p| p.name.as_str())
                    .unwrap_or("none")
            ),
            EventPayload::EncounterStarted {
                encounter_id: enc.id.clone(),
                title: enc.title.clone(),
                encounter_type: enc.encounter_type.clone(),
                participants: p_ids,
            },
        )?;
        self.maybe_compile()?;
        Ok((enc, ev))
    }

    pub fn resolve_encounter_action(
        &self,
        encounter_id: &str,
        actor_id: &str,
        action_type: &str,
        description: &str,
        target_ids: Vec<String>,
        new_position: Option<String>,
        new_range_band: Option<String>,
        resource_deltas: Vec<(String, String, i64)>, // (character_id, resource_name, delta)
        add_conditions: Vec<(String, StatusCondition)>,
        remove_conditions: Vec<(String, String)>,
        advance_turn: bool,
        causal_event_ids: Vec<i64>,
    ) -> Result<(EncounterState, WorldEvent)> {
        let eid = slugify(encounter_id);
        let mut enc = self
            .db
            .get_encounter(&eid)?
            .ok_or_else(|| anyhow!("Encounter '{eid}' not found"))?;
        if !enc.status.eq_ignore_ascii_case("active") {
            bail!("Encounter '{}' is already '{}'", enc.id, enc.status);
        }

        let resolved_actor = resolve_character_id(&self.db, actor_id).unwrap_or_else(|_| slugify(actor_id));
        let act_type = if action_type.trim().is_empty() {
            "action".to_string()
        } else {
            action_type.trim().to_lowercase()
        };

        // Update participant position/range/reaction if present
        if let Some(p) = enc
            .participants
            .iter_mut()
            .find(|p| p.entity_id == resolved_actor)
        {
            if act_type == "reaction" {
                if !p.reaction_available {
                    bail!("Participant '{}' has already spent their reaction this round", p.name);
                }
                p.reaction_available = false;
            }
            if let Some(pos) = new_position {
                p.position = pos;
            }
            if let Some(rb) = new_range_band {
                p.range_band = rb;
            }
        }

        // Apply live resource deltas and conditions to actual character projections!
        let mut sub_events = causal_event_ids;
        for (cid, res_name, delta) in resource_deltas {
            if let Ok((_, ev)) = self.modify_resource(
                &cid,
                &res_name,
                Some(delta),
                None,
                None,
                None,
                &format!("Encounter '{}' (Round {}): {}", enc.title, enc.round, description),
            ) {
                sub_events.push(ev.seq);
            }
        }
        for (cid, cond) in add_conditions {
            if let Ok((_, ev)) = self.add_condition(&cid, cond) {
                sub_events.push(ev.seq);
            }
        }
        for (cid, cond_name) in remove_conditions {
            if let Ok((_, ev)) = self.remove_condition(
                &cid,
                &cond_name,
                &format!("Encounter '{}' (Round {})", enc.title, enc.round),
            ) {
                sub_events.push(ev.seq);
            }
        }

        let resolved_targets: Vec<String> = target_ids
            .into_iter()
            .map(|t| resolve_character_id(&self.db, &t).unwrap_or_else(|_| slugify(&t)))
            .collect();

        let mut affected = vec![enc.id.clone(), resolved_actor.clone()];
        affected.extend(resolved_targets.clone());

        let ev = self.db.append_event_causal(
            enc.location_id.clone(),
            affected,
            format!(
                "[{} R{}] {} ({}): {}",
                enc.title, enc.round, resolved_actor, act_type, description
            ),
            EventPayload::EncounterActionResolved {
                encounter_id: enc.id.clone(),
                round: enc.round,
                actor_id: resolved_actor.clone(),
                action_type: act_type.clone(),
                description: description.to_string(),
                target_ids: resolved_targets.clone(),
            },
            EventCausalMeta {
                actor_id: Some(resolved_actor.clone()),
                causal_event_ids: sub_events,
                visibility: "public".to_string(),
            },
        )?;

        enc.action_log.push(EncounterLogEntry {
            round: enc.round,
            actor_id: resolved_actor,
            action_type: act_type,
            description: description.to_string(),
            target_ids: resolved_targets,
            event_id: Some(ev.seq),
        });

        if advance_turn {
            let _ = enc.advance_turn();
        }
        self.db.upsert_encounter(&enc)?;
        self.maybe_compile()?;
        Ok((enc, ev))
    }

    pub fn advance_encounter_turn(
        &self,
        encounter_id: &str,
        advance_world_minutes: Option<u64>,
    ) -> Result<(EncounterState, Option<String>)> {
        let eid = slugify(encounter_id);
        let mut enc = self
            .db
            .get_encounter(&eid)?
            .ok_or_else(|| anyhow!("Encounter '{eid}' not found"))?;
        if !enc.status.eq_ignore_ascii_case("active") {
            bail!("Encounter '{}' is already '{}'", enc.id, enc.status);
        }
        let prev_round = enc.round;
        let next_actor = enc.advance_turn();
        if enc.round > prev_round {
            if let Some(mins) = advance_world_minutes.filter(|&m| m > 0) {
                let _ = self.advance_time(
                    mins,
                    None,
                    Some(format!("Encounter '{}' advanced to Round {}", enc.title, enc.round)),
                )?;
            }
        }
        self.db.upsert_encounter(&enc)?;
        self.maybe_compile()?;
        Ok((enc, next_actor))
    }

    pub fn update_encounter_participant(
        &self,
        encounter_id: &str,
        participant: EncounterParticipant,
    ) -> Result<EncounterState> {
        let eid = slugify(encounter_id);
        let mut enc = self
            .db
            .get_encounter(&eid)?
            .ok_or_else(|| anyhow!("Encounter '{eid}' not found"))?;
        let pid = resolve_character_id(&self.db, &participant.entity_id)
            .unwrap_or_else(|_| slugify(&participant.entity_id));
        if let Some(existing) = enc.participants.iter_mut().find(|p| p.entity_id == pid) {
            if !participant.name.trim().is_empty() {
                existing.name = participant.name;
            }
            if !participant.side.trim().is_empty() {
                existing.side = participant.side;
            }
            if !participant.position.is_empty() {
                existing.position = participant.position;
            }
            if !participant.range_band.is_empty() {
                existing.range_band = participant.range_band;
            }
            if !participant.status.trim().is_empty() {
                existing.status = participant.status;
            }
            if !participant.notes.is_empty() {
                existing.notes = participant.notes;
            }
            if participant.initiative != 0 {
                existing.initiative = participant.initiative;
            }
            existing.reaction_available = participant.reaction_available;
        } else {
            let mut new_p = participant;
            new_p.entity_id = pid;
            enc.participants.push(new_p);
            enc.sort_initiative();
        }
        self.db.upsert_encounter(&enc)?;
        self.maybe_compile()?;
        Ok(enc)
    }

    pub fn end_encounter(
        &self,
        encounter_id: &str,
        outcome: &str,
        aftermath: &str,
    ) -> Result<(EncounterState, WorldEvent)> {
        let eid = slugify(encounter_id);
        let mut enc = self
            .db
            .get_encounter(&eid)?
            .ok_or_else(|| anyhow!("Encounter '{eid}' not found"))?;
        let now = self.db.get_campaign_meta()?.clock.format_short();
        enc.status = "ended".to_string();
        enc.outcome = outcome.to_string();
        enc.aftermath = aftermath.to_string();
        enc.ended_at_world_time = Some(now);
        self.db.upsert_encounter(&enc)?;

        let mut affected = vec![enc.id.clone()];
        for p in &enc.participants {
            affected.push(p.entity_id.clone());
        }
        let ev = self.db.append_event(
            enc.location_id.clone(),
            affected,
            format!(
                "Encounter ended: '{}' after {} round(s) — {}{}",
                enc.title,
                enc.round,
                outcome,
                if aftermath.is_empty() {
                    String::new()
                } else {
                    format!(" ({aftermath})")
                }
            ),
            EventPayload::EncounterEnded {
                encounter_id: enc.id.clone(),
                title: enc.title.clone(),
                rounds: enc.round,
                outcome: outcome.to_string(),
                aftermath: aftermath.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((enc, ev))
    }

    pub fn get_encounter(&self, encounter_id: &str) -> Result<EncounterState> {
        let eid = slugify(encounter_id);
        self.db
            .get_encounter(&eid)?
            .ok_or_else(|| anyhow!("Encounter '{eid}' not found"))
    }

    pub fn list_encounters(&self, status_filter: Option<&str>) -> Result<Vec<EncounterState>> {
        self.db.list_encounters(status_filter)
    }

    // -------------------------------------------------------------------------
    // Phase 2: Structured Profiles, Epistemic Knowledge, Factions & Autonomy Sandbox
    // -------------------------------------------------------------------------

    pub fn get_character_profile(&self, character_id: &str) -> Result<CharacterProfileExt> {
        let cid = resolve_character_id(&self.db, character_id)?;
        Ok(self.db.get_character_profile(&cid)?.unwrap_or_else(|| {
            CharacterProfileExt {
                character_id: cid,
                ..Default::default()
            }
        }))
    }

    pub fn update_character_profile(
        &self,
        character_id: &str,
        mut patch: serde_json::Value,
    ) -> Result<(CharacterProfileExt, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let char_proj = load_character_projection(&self.db, &cid, None)?;
        let existing = self.get_character_profile(&cid)?;
        let mut merged_val = serde_json::to_value(&existing)?;
        let mut fields_updated = Vec::new();

        // Special handling for upserting individual structured_goals without wiping existing ones
        let upsert_goal_val = patch
            .as_object_mut()
            .and_then(|m| m.remove("upsert_goal"));

        if let (Some(dst), Some(src)) = (merged_val.as_object_mut(), patch.as_object()) {
            for (k, v) in src {
                if k == "character_id" || v.is_null() {
                    continue;
                }
                dst.insert(k.clone(), v.clone());
                fields_updated.push(k.clone());
            }
        }

        let mut profile: CharacterProfileExt = serde_json::from_value(merged_val)?;
        profile.character_id = cid.clone();

        if let Some(gv) = upsert_goal_val {
            let mut g: StructuredGoal = serde_json::from_value(gv)?;
            if g.goal_id.trim().is_empty() {
                g.goal_id = slugify(&g.description);
            } else {
                g.goal_id = slugify(&g.goal_id);
            }
            if let Some(ex) = profile
                .structured_goals
                .iter_mut()
                .find(|x| x.goal_id == g.goal_id)
            {
                *ex = g;
            } else {
                profile.structured_goals.push(g);
            }
            fields_updated.push("structured_goals".to_string());
        }

        profile.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_character_profile(&profile)?;

        let ev = self.db.append_event(
            char_proj.current.current_location,
            vec![cid.clone()],
            format!(
                "Updated structured profile for {} ({})",
                char_proj.baseline.name,
                if fields_updated.is_empty() {
                    "profile".to_string()
                } else {
                    fields_updated.join(", ")
                }
            ),
            EventPayload::CharacterProfileUpdated {
                character_id: cid,
                fields_updated,
            },
        )?;
        self.maybe_compile()?;
        Ok((profile, ev))
    }

    /// Records a world/rumor knowledge claim and per-actor epistemic beliefs (supports partial observation,
    /// confidence scores, concealment, disbelief, and superseding outdated claims).
    pub fn record_knowledge(
        &self,
        mut claim: KnowledgeClaim,
        mut beliefs: Vec<ActorBelief>,
        supersedes_claim_id: Option<String>,
        causal_event_ids: Vec<i64>,
    ) -> Result<(KnowledgeRecordView, WorldEvent)> {
        if claim.claim.trim().is_empty() {
            bail!("Knowledge `claim` text cannot be empty");
        }
        let now = self.db.get_campaign_meta()?.clock.format_short();
        claim.id = if claim.id.trim().is_empty() {
            slugify(&format!("{}-{}", claim.subject, &claim.claim[..claim.claim.len().min(48)]))
        } else {
            slugify(&claim.id)
        };
        if let Some(existing) = self.db.get_knowledge_claim(&claim.id)? {
            claim.created_at_world_time = existing.created_at_world_time;
        } else {
            claim.created_at_world_time = now.clone();
        }
        claim.updated_at_world_time = now.clone();
        self.db.upsert_knowledge_claim(&claim)?;

        // Mark superseded claim so actors holding only the older claim automatically get flagged outdated!
        if let Some(old_id) = supersedes_claim_id.filter(|s| !s.trim().is_empty()) {
            let old_slug = slugify(&old_id);
            if let Some(mut old_claim) = self.db.get_knowledge_claim(&old_slug)? {
                old_claim.superseded_by_id = Some(claim.id.clone());
                old_claim.updated_at_world_time = now.clone();
                self.db.upsert_knowledge_claim(&old_claim)?;
            }
        }

        let mut actors_updated = Vec::new();
        for b in &mut beliefs {
            b.claim_id = claim.id.clone();
            b.actor_id = resolve_character_id(&self.db, &b.actor_id)
                .unwrap_or_else(|_| slugify(&b.actor_id));
            b.confidence = b.confidence.min(100);
            if b.learned_at_world_time.trim().is_empty() {
                b.learned_at_world_time = now.clone();
            }
            if b.stance.eq_ignore_ascii_case("concealing") {
                b.concealing = true;
            }
            self.db.upsert_actor_belief(b)?;
            actors_updated.push(b.actor_id.clone());
        }

        let mut affected = vec![claim.id.clone(), slugify(&claim.subject)];
        affected.extend(actors_updated.clone());
        let ev = self.db.append_event_causal(
            None,
            affected,
            format!(
                "Knowledge recorded [{} | {}]: '{}' (known/updated for: {})",
                claim.subject,
                claim.truth_status,
                claim.claim,
                if actors_updated.is_empty() {
                    "none yet".to_string()
                } else {
                    actors_updated.join(", ")
                }
            ),
            EventPayload::KnowledgeRecorded {
                claim_id: claim.id.clone(),
                subject: claim.subject.clone(),
                truth_status: claim.truth_status.clone(),
                actors_updated,
            },
            EventCausalMeta {
                actor_id: None,
                causal_event_ids,
                visibility: claim.visibility.clone(),
            },
        )?;

        let all_beliefs = self.db.list_beliefs_for_claim(&claim.id)?;
        self.maybe_compile()?;
        Ok((
            KnowledgeRecordView {
                claim,
                beliefs: all_beliefs,
            },
            ev,
        ))
    }

    /// Propagates a knowledge claim from a source actor (or rumor mill) to target actors with optional
    /// confidence decay, perceived distortion, and deception tracking. Enforces epistemic honesty!
    pub fn propagate_knowledge(
        &self,
        claim_id: &str,
        from_actor_id: Option<String>,
        to_actor_ids: Vec<String>,
        stance: Option<String>,
        confidence_decay: u8,
        distorted_claim: Option<String>,
        deceptive: bool,
        notes: &str,
    ) -> Result<(Vec<ActorBelief>, WorldEvent)> {
        let cid = slugify(claim_id);
        let claim = self
            .db
            .get_knowledge_claim(&cid)?
            .ok_or_else(|| anyhow!("Knowledge claim '{cid}' not found"))?;
        if to_actor_ids.is_empty() {
            bail!("'to_actor_ids' must contain at least one recipient");
        }
        let now = self.db.get_campaign_meta()?.clock.format_short();

        let (resolved_from, base_conf, base_text) = match from_actor_id.filter(|s| !s.trim().is_empty()) {
            Some(raw_from) => {
                let fid = resolve_character_id(&self.db, &raw_from).unwrap_or_else(|_| slugify(&raw_from));
                if !deceptive {
                    let Some(src_belief) = self.db.get_actor_belief(&cid, &fid)? else {
                        bail!(
                            "Epistemic guard: actor '{fid}' does not hold claim '{cid}' and cannot truthfully propagate it (set deceptive=true if fabricating)."
                        );
                    };
                    let txt = if src_belief.perceived_claim.is_empty() {
                        claim.claim.clone()
                    } else {
                        src_belief.perceived_claim
                    };
                    (Some(fid), src_belief.confidence, txt)
                } else {
                    (Some(fid), 75u8, claim.claim.clone())
                }
            }
            None => (None, 65u8, claim.claim.clone()),
        };

        let new_conf = base_conf.saturating_sub(confidence_decay).max(5);
        let target_stance = stance.unwrap_or_else(|| {
            if deceptive {
                "misinformed".to_string()
            } else if resolved_from.is_some() {
                "testimony".to_string()
            } else {
                "rumor".to_string()
            }
        });
        let perceived = distorted_claim
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(base_text);

        let mut created = Vec::new();
        let mut resolved_targets = Vec::new();
        for raw_to in to_actor_ids {
            let tid = resolve_character_id(&self.db, &raw_to).unwrap_or_else(|_| slugify(&raw_to));
            let belief = ActorBelief {
                claim_id: cid.clone(),
                actor_id: tid.clone(),
                stance: target_stance.clone(),
                confidence: new_conf,
                perceived_claim: perceived.clone(),
                source: resolved_from
                    .clone()
                    .unwrap_or_else(|| "local-rumor".to_string()),
                source_trustworthy: Some(!deceptive),
                concealing: false,
                outdated: claim.superseded_by_id.is_some(),
                learned_at_world_time: now.clone(),
                notes: notes.to_string(),
            };
            self.db.upsert_actor_belief(&belief)?;
            resolved_targets.push(tid);
            created.push(belief);
        }

        let mut affected = vec![cid.clone()];
        if let Some(ref f) = resolved_from {
            affected.push(f.clone());
        }
        affected.extend(resolved_targets.clone());
        let ev = self.db.append_event_causal(
            None,
            affected,
            format!(
                "Knowledge '{}' propagated from {} -> {} [stance: {}, conf: {}%{}]",
                cid,
                resolved_from.as_deref().unwrap_or("rumor-mill"),
                resolved_targets.join(", "),
                target_stance,
                new_conf,
                if deceptive { ", DECEPTIVE" } else { "" }
            ),
            EventPayload::KnowledgePropagated {
                claim_id: cid,
                from_actor_id: resolved_from.clone(),
                to_actor_ids: resolved_targets,
                deceptive,
                notes: notes.to_string(),
            },
            EventCausalMeta {
                actor_id: resolved_from,
                causal_event_ids: claim.source_event_id.into_iter().collect(),
                visibility: claim.visibility,
            },
        )?;
        self.maybe_compile()?;
        Ok((created, ev))
    }

    pub fn search_knowledge(&self, filter: &KnowledgeSearchFilter) -> Result<Vec<KnowledgeRecordView>> {
        let base_claims = if let Some(ref q) = filter.query.as_deref().filter(|s| !s.trim().is_empty()) {
            self.db.search_knowledge_claims_fts(q, filter.limit.unwrap_or(50))?
        } else {
            self.db.list_knowledge_claims()?
        };

        let actor_filter = filter
            .actor_id
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| resolve_character_id(&self.db, s).unwrap_or_else(|_| slugify(s)));
        let not_known_filter = filter
            .not_known_by
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| resolve_character_id(&self.db, s).unwrap_or_else(|_| slugify(s)));
        let include_outdated = filter.include_outdated.unwrap_or(true);

        let mut results = Vec::new();
        for claim in base_claims {
            if !include_outdated && claim.superseded_by_id.is_some() {
                continue;
            }
            if let Some(ref subj) = filter.subject.as_deref().filter(|s| !s.trim().is_empty()) {
                let sl = subj.to_lowercase();
                if !claim.subject.to_lowercase().contains(&sl) && slugify(&claim.subject) != slugify(subj) {
                    continue;
                }
            }
            if let Some(ref ts) = filter.truth_status.as_deref().filter(|s| !s.trim().is_empty()) {
                if !claim.truth_status.eq_ignore_ascii_case(ts) {
                    continue;
                }
            }
            if let Some(ref vis) = filter.visibility.as_deref().filter(|s| !s.trim().is_empty()) {
                if !claim.visibility.eq_ignore_ascii_case(vis) {
                    continue;
                }
            }

            let mut beliefs = self.db.list_beliefs_for_claim(&claim.id)?;
            for b in &mut beliefs {
                if claim.superseded_by_id.is_some() {
                    b.outdated = true;
                }
            }

            if let Some(ref nkb) = not_known_filter {
                if beliefs.iter().any(|b| &b.actor_id == nkb) {
                    continue;
                }
            }
            if let Some(ref aid) = actor_filter {
                beliefs.retain(|b| &b.actor_id == aid);
                if beliefs.is_empty() {
                    continue;
                }
            }
            if let Some(ref st) = filter.stance.as_deref().filter(|s| !s.trim().is_empty()) {
                beliefs.retain(|b| b.stance.eq_ignore_ascii_case(st));
                if beliefs.is_empty() {
                    continue;
                }
            }
            if let Some(min_c) = filter.min_confidence {
                beliefs.retain(|b| b.confidence >= min_c);
                if beliefs.is_empty() {
                    continue;
                }
            }

            results.push(KnowledgeRecordView { claim, beliefs });
            if let Some(lim) = filter.limit {
                if results.len() >= lim {
                    break;
                }
            }
        }
        Ok(results)
    }

    // -------------------------------------------------------------------------
    // Dedicated Factions Register (`factions`)
    // -------------------------------------------------------------------------

    pub fn upsert_faction(&self, faction: FactionRecord, notes: &str) -> Result<(FactionRecord, WorldEvent)> {
        if faction.name.trim().is_empty() {
            bail!("Faction name cannot be empty");
        }
        let mut f = faction.normalize();
        let now = self.db.get_campaign_meta()?.clock.format_short();
        let action = if let Some(existing) = self.db.get_faction(&f.id)? {
            f.created_at_world_time = existing.created_at_world_time;
            "updated"
        } else {
            f.created_at_world_time = now.clone();
            "created"
        };
        f.updated_at_world_time = now;
        self.db.upsert_faction(&f)?;

        let mut affected = vec![f.id.clone()];
        if let Some(ref l) = f.leader_id {
            affected.push(l.clone());
        }
        for m in &f.members {
            affected.push(m.character_id.clone());
        }
        let ev = self.db.append_event(
            f.territory.first().cloned(),
            affected,
            format!(
                "Faction '{}' [{} | {} members | tier: {}] {action}{}",
                f.name,
                f.faction_type,
                f.members.len(),
                f.capacity_tier,
                if notes.is_empty() {
                    String::new()
                } else {
                    format!(" — {notes}")
                }
            ),
            EventPayload::FactionRecorded {
                faction_id: f.id.clone(),
                name: f.name.clone(),
                action: action.to_string(),
                notes: notes.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((f, ev))
    }

    pub fn get_faction(&self, faction_id: &str) -> Result<FactionRecord> {
        let id = slugify(faction_id);
        self.db
            .get_faction(&id)?
            .ok_or_else(|| anyhow!("Faction '{id}' not found"))
    }

    pub fn list_factions(&self) -> Result<Vec<FactionRecord>> {
        self.db.list_factions()
    }

    pub fn add_faction_member(
        &self,
        faction_id: &str,
        mut member: FactionMember,
    ) -> Result<(FactionRecord, WorldEvent)> {
        let mut f = self.get_faction(faction_id)?;
        member.character_id = resolve_character_id(&self.db, &member.character_id)
            .unwrap_or_else(|_| slugify(&member.character_id));
        let cid = member.character_id.clone();
        if let Some(ex) = f.members.iter_mut().find(|m| m.character_id == cid) {
            *ex = member;
        } else {
            f.members.push(member);
        }
        self.upsert_faction(f, &format!("Added/updated member '{cid}'"))
    }

    pub fn remove_faction_member(
        &self,
        faction_id: &str,
        character_id: &str,
        reason: &str,
    ) -> Result<(FactionRecord, WorldEvent)> {
        let mut f = self.get_faction(faction_id)?;
        let cid = resolve_character_id(&self.db, character_id)
            .unwrap_or_else(|_| slugify(character_id));
        let before = f.members.len();
        f.members.retain(|m| m.character_id != cid);
        if f.members.len() == before {
            bail!("Character '{cid}' is not a member of faction '{}'", f.id);
        }
        if f.leader_id.as_deref() == Some(cid.as_str()) {
            f.leader_id = None;
        }
        self.upsert_faction(
            f,
            &format!(
                "Removed member '{cid}'{}",
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(" ({reason})")
                }
            ),
        )
    }

    pub fn modify_faction_resource(
        &self,
        faction_id: &str,
        resource: &str,
        delta: i64,
        allow_debt: bool,
        reason: &str,
    ) -> Result<(FactionRecord, WorldEvent)> {
        let mut f = self.get_faction(faction_id)?;
        let cur = f.resources.get(resource).copied().unwrap_or(0);
        let next = cur + delta;
        if next < 0 && !allow_debt {
            bail!(
                "Insufficient faction resource '{}' for '{}': has {cur}, attempted delta {delta} (set allow_debt=true to incur debt)",
                resource,
                f.name
            );
        }
        f.resources.insert(resource.to_string(), next);
        self.upsert_faction(
            f,
            &format!(
                "Resource '{resource}': {cur} -> {next} ({delta:+}){}",
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(" — {reason}")
                }
            ),
        )
    }

    /// Promotes a faction currently stored as a `LoreEntry` into the dedicated `factions` register.
    pub fn promote_lore_to_faction(
        &self,
        lore_id: &str,
        faction_type: Option<String>,
        capacity_tier: Option<String>,
        remove_old_lore: bool,
    ) -> Result<(FactionRecord, WorldEvent)> {
        let lid = slugify(lore_id);
        let lore = self
            .db
            .get_lore(&lid)?
            .ok_or_else(|| anyhow!("Lore entry '{lid}' not found"))?;
        let f = FactionRecord {
            id: lore.id.clone(),
            name: lore.title.clone(),
            faction_type: faction_type.unwrap_or_else(|| "faction".to_string()),
            origins: lore.summary.clone(),
            leadership_model: String::new(),
            ideology: String::new(),
            established_territory: vec![],
            institutional_structure: String::new(),
            capacity_tier: capacity_tier.unwrap_or_else(|| "local".to_string()),
            leader_id: None,
            members: vec![],
            territory: vec![],
            resources: std::collections::BTreeMap::new(),
            reputation: std::collections::BTreeMap::new(),
            objectives: vec![],
            internal_conflicts: vec![],
            morale: 60,
            active_schemes: vec![],
            tags: lore.tags.clone(),
            notes: lore.body.clone(),
            created_at_world_time: lore.created_at_world_time,
            updated_at_world_time: String::new(),
        };
        let res = self.upsert_faction(f, &format!("Promoted from lore entry '{lid}'"))?;
        if remove_old_lore {
            let _ = self.db.delete_lore(&lid)?;
            self.maybe_compile()?;
            let _ = prune_orphaned_docs(&self.db, &self.docs_dir())?;
        }
        Ok(res)
    }

    // -------------------------------------------------------------------------
    // Two-Phase Constrained Agency Sandbox (`simulate_autonomy`)
    // -------------------------------------------------------------------------

    /// Phase A (`mode: "prepare"`): Builds a strictly epistemic-isolated Autonomy Dossier for
    /// a solo NPC, group of NPCs, or Faction over `duration_hours`. ZERO GM secrets or unknown claims leak!
    pub fn prepare_autonomy_dossier(
        &self,
        scope_type: &str,
        faction_id: Option<&str>,
        actor_ids: &[String],
        duration_hours: u64,
        location_id: Option<&str>,
    ) -> Result<AutonomyDossier> {
        let meta = self.db.get_campaign_meta()?;
        let all_claims = self.db.list_knowledge_claims()?;

        let faction = match faction_id.filter(|s| !s.trim().is_empty()) {
            Some(fid) => Some(self.get_faction(fid)?),
            None => None,
        };

        let mut target_actor_ids: Vec<String> = Vec::new();
        for raw in actor_ids {
            if let Ok(cid) = resolve_character_id(&self.db, raw) {
                if !target_actor_ids.contains(&cid) {
                    target_actor_ids.push(cid);
                }
            }
        }
        if let Some(ref fac) = faction {
            if target_actor_ids.is_empty() {
                for m in &fac.members {
                    if !target_actor_ids.contains(&m.character_id) {
                        target_actor_ids.push(m.character_id.clone());
                    }
                }
            }
        }

        let mut allowed_claims_set = std::collections::BTreeSet::new();
        let mut actor_briefs = Vec::new();
        let mut reachable_places = std::collections::BTreeSet::new();
        let mut resolved_loc = location_id
            .filter(|s| !s.trim().is_empty())
            .map(|l| resolve_place_id(&self.db, l).unwrap_or_else(|_| slugify(l)));

        for cid in &target_actor_ids {
            let proj = match load_character_projection(&self.db, cid, None) {
                Ok(p) => p,
                Err(_) => continue,
            };
            if resolved_loc.is_none() {
                resolved_loc = proj.current.current_location.clone();
            }
            if let Some(ref loc) = proj.current.current_location {
                reachable_places.insert(loc.clone());
                if let Ok(pl) = load_place_projection(&self.db, loc, None) {
                    for r in &pl.current.routes {
                        if !r.status.eq_ignore_ascii_case("blocked")
                            && !r.status.eq_ignore_ascii_case("destroyed")
                        {
                            reachable_places.insert(r.destination_id.clone());
                        }
                    }
                }
            }

            let ext = self.get_character_profile(cid)?;
            let beliefs = self.db.list_beliefs_for_actor(cid)?;
            let mut known_claims = Vec::new();
            let mut known_claim_ids = std::collections::BTreeSet::new();
            for b in beliefs {
                if let Some(cl) = all_claims.iter().find(|c| c.id == b.claim_id) {
                    known_claim_ids.insert(cl.id.clone());
                    allowed_claims_set.insert(cl.id.clone());
                    known_claims.push(ActorKnowledgeBrief {
                        claim_id: cl.id.clone(),
                        actor_id: cid.clone(),
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
            // Count active claims this actor does NOT know (number only — ZERO text leaked!)
            let blind_spot_count = all_claims
                .iter()
                .filter(|c| c.superseded_by_id.is_none() && !known_claim_ids.contains(&c.id))
                .count();

            // Gather interpersonal tensions from multi-dimensional relations
            let mut personal_tensions = Vec::new();
            for rel in self.db.list_relations_for_entity(cid)? {
                let other = if rel.source_id == *cid {
                    &rel.target_id
                } else {
                    &rel.source_id
                };
                if rel.dimensions.resentment.unwrap_or(0) >= 25
                    || rel.dimensions.fear.unwrap_or(0) >= 35
                    || rel.dimensions.trust.is_some_and(|t| t <= 35)
                    || rel.relation_type.to_lowercase().contains("rival")
                    || rel.relation_type.to_lowercase().contains("hostil")
                {
                    let dim_str = rel.dimensions.format_compact();
                    personal_tensions.push(format!(
                        "{} -> {} [{}]{}",
                        rel.source_id,
                        other,
                        rel.relation_type,
                        if dim_str.is_empty() {
                            String::new()
                        } else {
                            format!(" ({dim_str})")
                        }
                    ));
                }
            }

            let mut active_goals = ext
                .structured_goals
                .into_iter()
                .filter(|g| g.status.eq_ignore_ascii_case("active"))
                .collect::<Vec<_>>();
            active_goals.sort_by(|a, b| b.priority.cmp(&a.priority));

            actor_briefs.push(ActorAutonomyBrief {
                character_id: cid.clone(),
                name: proj.baseline.name,
                alive_status: proj.current.alive_status,
                location_id: proj.current.current_location,
                active_conditions: proj
                    .current
                    .conditions
                    .iter()
                    .map(|c| format!("{} ({})", c.name, c.severity))
                    .collect(),
                carried_items: proj
                    .current
                    .equipment
                    .iter()
                    .map(|i| format!("{} x{}", i.name, i.quantity))
                    .collect(),
                resources: proj
                    .current
                    .resources
                    .iter()
                    .map(|(k, v)| (k.clone(), format!("{}/{} {}", v.current, v.max, v.unit)))
                    .collect(),
                risk_tolerance: ext.risk_tolerance,
                morale: ext.morale,
                stress: ext.stress,
                motivations: ext.long_term_motivations,
                fears: ext.fears,
                current_plans: ext.current_plans,
                active_goals,
                known_claims,
                blind_spot_count,
                personal_tensions,
            });
        }

        // Compute deterministic baseline upkeep estimate (days * active headcount)
        let days = ((duration_hours as f64) / 24.0).ceil().max(1.0) as i64;
        let active_headcount = actor_briefs
            .iter()
            .filter(|a| a.alive_status.eq_ignore_ascii_case("alive") || a.alive_status.eq_ignore_ascii_case("active"))
            .count()
            .max(faction.as_ref().map(|f| f.members.len()).unwrap_or(0)) as i64;
        let mut projected_upkeep = std::collections::BTreeMap::new();
        if active_headcount > 0 {
            projected_upkeep.insert("daily_rations".to_string(), active_headcount * days);
        }

        // Institutional relations (strictly faction-kind edges)
        let mut institutional_relations = Vec::new();
        let mut capacity_constraints = Vec::new();
        if let Some(ref fac) = faction {
            for rel in self.db.list_relations_for_entity(&fac.id)? {
                if rel.source_kind == "faction" || rel.target_kind == "faction" {
                    institutional_relations.push(format!(
                        "{} -[{}]-> {} ({})",
                        rel.source_id, rel.relation_type, rel.target_id, rel.notes
                    ));
                }
            }
            capacity_constraints.push(format!(
                "Faction '{}' operational tier is '{}' with {} roster member(s) and morale {}/100. Actions must stay within this physical capacity.",
                fac.name,
                fac.capacity_tier,
                fac.members.len(),
                fac.morale
            ));
            for (rk, rv) in &fac.resources {
                capacity_constraints.push(format!("Available faction resource '{rk}': {rv}"));
            }
        }
        if duration_hours > 0 {
            capacity_constraints.push(format!("Bounded simulation window: {duration_hours} hour(s)."));
        }

        // Only PUBLIC surrounding events (never leak gm_secret events into autonomy brief!)
        let surrounding_public_events: Vec<String> = self
            .db
            .list_recent_events(12)?
            .into_iter()
            .filter(|e| e.visibility == "public")
            .filter(|e| {
                resolved_loc.is_none()
                    || e.location_id == resolved_loc
                    || e.affected_entities
                        .iter()
                        .any(|ae| target_actor_ids.contains(ae))
            })
            .take(8)
            .map(|e| format!("[#{} | {}] {}", e.seq, e.world_time, e.summary))
            .collect();

        Ok(AutonomyDossier {
            scope_type: scope_type.to_string(),
            world_time: meta.clock.format_short(),
            duration_hours,
            location_id: resolved_loc,
            faction,
            actors: actor_briefs,
            reachable_places: reachable_places.into_iter().collect(),
            projected_upkeep,
            institutional_relations,
            surrounding_public_events,
            capacity_constraints,
            allowed_claim_ids: allowed_claims_set.into_iter().collect(),
        })
    }

    /// Phase B (`mode: "commit"`): Validates every proposed action against actual resources,
    /// alive status, and epistemic knowledge boundaries BEFORE atomically committing to SQLite!
    pub fn commit_autonomy_turn(
        &self,
        req: AutonomyCommitRequest,
    ) -> Result<(serde_json::Value, WorldEvent)> {
        // 1. Validate faction resources BEFORE mutating anything (atomic guard — zero resource hallucination!)
        let mut faction_opt = match req.faction_id.as_deref().filter(|s| !s.trim().is_empty()) {
            Some(fid) => Some(self.get_faction(fid)?),
            None => None,
        };
        if let Some(ref fac) = faction_opt {
            for rd in &req.faction_resource_deltas {
                let cur = fac.resources.get(&rd.resource).copied().unwrap_or(0);
                if cur + rd.delta < 0 && !req.allow_debt {
                    bail!(
                        "Autonomy validation failed: faction '{}' only has {} of '{}', cannot apply delta {} (no silent resource creation; pass allow_debt=true to incur debt).",
                        fac.name,
                        cur,
                        rd.resource,
                        rd.delta
                    );
                }
            }
        }

        // 2. Validate every action's actor status AND epistemic knowledge bounds!
        let mut all_actors = Vec::new();
        for raw_aid in &req.actor_ids {
            if let Ok(cid) = resolve_character_id(&self.db, raw_aid) {
                if !all_actors.contains(&cid) {
                    all_actors.push(cid);
                }
            }
        }
        for act in &req.actions {
            let cid = resolve_character_id(&self.db, &act.actor_id)?;
            if !all_actors.contains(&cid) {
                all_actors.push(cid.clone());
            }
            let proj = load_character_projection(&self.db, &cid, None)?;
            if proj.current.alive_status.eq_ignore_ascii_case("deceased")
                || proj.current.alive_status.eq_ignore_ascii_case("dead")
            {
                bail!("Autonomy validation failed: actor '{cid}' is deceased and cannot act.");
            }
            for claim_id in &act.relied_on_claim_ids {
                let cslug = slugify(claim_id);
                if self.db.get_actor_belief(&cslug, &cid)?.is_none() {
                    bail!(
                        "Epistemic guard blocked action by '{cid}': actor does NOT know claim '{cslug}'. NPCs cannot act on GM secrets or unlearned facts!"
                    );
                }
            }
        }

        // 3. Apply optional world clock advance
        let mut sub_events = req.causal_event_ids.clone();
        if req.advance_world_clock && req.duration_hours > 0 {
            let (_, _, ev) = self.advance_time(
                req.duration_hours * 60,
                None,
                Some(format!("Autonomy window ({}h, {})", req.duration_hours, req.scope_type)),
            )?;
            sub_events.push(ev.seq);
        }

        // 4. Commit validated faction updates
        if let Some(ref mut fac) = faction_opt {
            for rd in &req.faction_resource_deltas {
                let cur = fac.resources.get(&rd.resource).copied().unwrap_or(0);
                fac.resources.insert(rd.resource.clone(), cur + rd.delta);
            }
            if let Some(md) = req.morale_delta {
                fac.morale = (fac.morale + md).clamp(0, 100);
            }
            for c in &req.add_internal_conflicts {
                if !c.trim().is_empty() && !fac.internal_conflicts.contains(c) {
                    fac.internal_conflicts.push(c.clone());
                }
            }
            for c in &req.resolve_internal_conflicts {
                fac.internal_conflicts
                    .retain(|x| !x.eq_ignore_ascii_case(c));
            }
            for gu in &req.goal_updates {
                if slugify(&gu.owner_id) == fac.id {
                    let gslug = slugify(&gu.goal_id);
                    if let Some(g) = fac.objectives.iter_mut().find(|o| o.goal_id == gslug) {
                        if let Some(d) = gu.progress_delta {
                            g.progress += d;
                        }
                        if let Some(ref st) = gu.set_status {
                            g.status = st.clone();
                        }
                        if !gu.notes.is_empty() {
                            g.notes = gu.notes.clone();
                        }
                    }
                }
            }
            let (_, fev) = self.upsert_faction(fac.clone(), "Autonomy turn committed")?;
            sub_events.push(fev.seq);
        }

        // 5. Commit character resource & structured goal updates
        for crd in &req.character_resource_deltas {
            let (_, cev) = self.modify_resource(
                &crd.character_id,
                &crd.resource,
                Some(crd.delta),
                None,
                None,
                None,
                "Off-screen autonomy activity",
            )?;
            sub_events.push(cev.seq);
        }
        for gu in &req.goal_updates {
            if faction_opt
                .as_ref()
                .is_some_and(|f| slugify(&gu.owner_id) == f.id)
            {
                continue;
            }
            if let Ok(cid) = resolve_character_id(&self.db, &gu.owner_id) {
                let mut ext = self.get_character_profile(&cid)?;
                let gslug = slugify(&gu.goal_id);
                if let Some(g) = ext.structured_goals.iter_mut().find(|o| o.goal_id == gslug) {
                    if let Some(d) = gu.progress_delta {
                        g.progress += d;
                    }
                    if let Some(ref st) = gu.set_status {
                        g.status = st.clone();
                    }
                    if !gu.notes.is_empty() {
                        g.notes = gu.notes.clone();
                    }
                    ext.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
                    self.db.upsert_character_profile(&ext)?;
                }
            }
        }

        // 6. Log causal summary event
        let summary = if !req.summary.trim().is_empty() {
            req.summary.clone()
        } else {
            let act_lines: Vec<String> = req
                .actions
                .iter()
                .map(|a| format!("{}: {}", a.actor_id, a.action))
                .collect();
            format!(
                "Autonomy [{} | {}h]: {}",
                req.scope_type,
                req.duration_hours,
                if act_lines.is_empty() {
                    "Upkeep & goals updated".to_string()
                } else {
                    act_lines.join("; ")
                }
            )
        };
        let mut affected = all_actors.clone();
        if let Some(ref fac) = faction_opt {
            affected.push(fac.id.clone());
        }
        let loc = req
            .location_id
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|l| resolve_place_id(&self.db, l).unwrap_or_else(|_| slugify(l)));

        let ev = self.db.append_event_causal(
            loc,
            affected,
            summary.clone(),
            EventPayload::AutonomySimulated {
                scope_type: req.scope_type,
                faction_id: faction_opt.as_ref().map(|f| f.id.clone()),
                actor_ids: all_actors,
                duration_hours: req.duration_hours,
                actions_count: req.actions.len(),
                summary,
            },
            EventCausalMeta {
                actor_id: None,
                causal_event_ids: sub_events,
                visibility: "public".to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((
            serde_json::json!({
                "committed": true,
                "event_seq": ev.seq,
                "actions_committed": req.actions.len(),
                "faction": faction_opt
            }),
            ev,
        ))
    }

    // -------------------------------------------------------------------------
    // Phase 3: Connected Geography, Travel, Exploration, Economy & Law/Reputation
    // -------------------------------------------------------------------------

    pub fn all_effective_routes(&self) -> Result<Vec<TravelRoute>> {
        let mut explicit = self.db.list_travel_routes()?;
        let explicit_pairs: std::collections::BTreeSet<(String, String)> = explicit
            .iter()
            .flat_map(|r| {
                let mut v = vec![(r.from_place_id.clone(), r.to_place_id.clone())];
                if r.bidirectional {
                    v.push((r.to_place_id.clone(), r.from_place_id.clone()));
                }
                v
            })
            .collect();
        let places = load_all_place_projections(&self.db, None)?;
        for syn in synthesize_routes_from_places(&places) {
            if !explicit_pairs.contains(&(syn.from_place_id.clone(), syn.to_place_id.clone())) {
                explicit.push(syn);
            }
        }
        Ok(explicit)
    }

    pub fn upsert_travel_route(&self, route: TravelRoute) -> Result<(TravelRoute, WorldEvent)> {
        let mut norm = route.normalize()?;
        norm.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_travel_route(&norm)?;

        // Also mirror route status onto PlaceCurrent via RouteUpdated when places exist
        let summary = format!(
            "Travel route '{}' ({} {} {}, {:.1} km, status: {}) updated.",
            norm.id,
            norm.from_place_id,
            if norm.bidirectional { "<->" } else { "->" },
            norm.to_place_id,
            norm.distance_km,
            norm.status
        );
        let ev = self.db.append_event(
            Some(norm.from_place_id.clone()),
            vec![norm.id.clone(), norm.from_place_id.clone(), norm.to_place_id.clone()],
            summary,
            EventPayload::RouteUpdated {
                place_id: norm.from_place_id.clone(),
                route: Route {
                    destination_id: norm.to_place_id.clone(),
                    travel_time: format!("{:.1} km", norm.distance_km),
                    terrain_difficulty: norm.terrain.clone(),
                    status: norm.status.clone(),
                    notes: norm.notes.clone(),
                },
            },
        )?;
        self.maybe_compile()?;
        Ok((norm, ev))
    }

    pub fn update_place_spatial(&self, mut sp: PlaceSpatialExt) -> Result<PlaceSpatialExt> {
        sp.place_id = resolve_place_id(&self.db, &sp.place_id)?;
        sp.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_place_spatial(&sp)?;
        self.maybe_compile()?;
        Ok(sp)
    }

    pub fn calculate_travel(
        &self,
        from_place_id: &str,
        to_place_id: &str,
        character_ids: &[String],
        transport_mode: &str,
        forced_march: bool,
    ) -> Result<TravelPlan> {
        let from_id = resolve_place_id(&self.db, from_place_id)
            .unwrap_or_else(|_| slugify(from_place_id));
        let to_id = resolve_place_id(&self.db, to_place_id)
            .unwrap_or_else(|_| slugify(to_place_id));
        let meta = self.db.get_campaign_meta()?;
        let season = meta.clock.season.clone();
        let weather = load_place_projection(&self.db, &from_id, None)
            .ok()
            .map(|p| p.current.current_weather)
            .filter(|w| !w.is_empty())
            .unwrap_or_else(|| meta.clock.global_weather.clone());

        let econ_map = self.db.list_item_economics_map()?;
        let mut slowest_speed_mult = if forced_march { 1.25f64 } else { 1.0f64 };
        let mut enc_notes = Vec::new();
        let mut inj_notes = Vec::new();
        let mut traveler_count = 0i64;

        for raw_cid in character_ids {
            if let Ok(cid) = resolve_character_id(&self.db, raw_cid) {
                if let Ok(proj) = load_character_projection(&self.db, &cid, None) {
                    traveler_count += 1;
                    let enc = compute_encumbrance(&proj, &econ_map);
                    if enc.travel_speed_multiplier < 1.0 {
                        slowest_speed_mult = slowest_speed_mult.min(enc.travel_speed_multiplier);
                        enc_notes.push(format!(
                            "{}: {} load ({:.1}/{:.1} kg, speed x{:.2})",
                            proj.baseline.name,
                            enc.load_tier,
                            enc.total_weight_kg,
                            enc.carry_capacity_kg,
                            enc.travel_speed_multiplier
                        ));
                    }
                    let (inj_f, notes) = travel_injury_factor(&proj);
                    if inj_f < 1.0 {
                        slowest_speed_mult *= inj_f;
                        inj_notes.extend(notes);
                    }
                }
            }
        }
        traveler_count = traveler_count.max(1);

        let routes = self.all_effective_routes()?;
        let baseline = compute_shortest_route(
            &routes,
            &from_id,
            &to_id,
            transport_mode,
            &season,
            &weather,
            slowest_speed_mult,
            true,
        );
        let live = compute_shortest_route(
            &routes,
            &from_id,
            &to_id,
            transport_mode,
            &season,
            &weather,
            slowest_speed_mult,
            false,
        );

        let mut rerouted_around = Vec::new();
        if let Some((ref base_legs, _, _)) = baseline {
            for bl in base_legs {
                if !matches!(bl.status.as_str(), "open" | "hazardous") {
                    rerouted_around.push(format!("{} ({})", bl.route_id, bl.status));
                }
            }
        }

        match live {
            Some((legs, total_km, travel_hours)) => {
                let daily_travel_cap = if forced_march { 12.0 } else { 8.0 };
                let rest_stops = ((travel_hours / daily_travel_cap).floor() as u32).saturating_sub(0);
                let total_elapsed = ((travel_hours + (rest_stops as f64 * 8.0)) * 10.0).round() / 10.0;
                let days = ((total_elapsed / 24.0).ceil() as i64).max(1);
                let rations = days * traveler_count;
                let mut tolls: std::collections::BTreeMap<String, i64> =
                    std::collections::BTreeMap::new();
                let mut hazards = Vec::new();
                for l in &legs {
                    if l.toll_cost > 0 {
                        *tolls.entry(l.toll_currency.clone()).or_insert(0) += l.toll_cost;
                    }
                    for h in &l.hazards {
                        if !hazards.contains(h) {
                            hazards.push(h.clone());
                        }
                    }
                }
                if forced_march {
                    hazards.push("Forced march fatigue risk".to_string());
                }
                let mut warnings = Vec::new();
                for l in &legs {
                    if let Some(orig_r) = routes.iter().find(|r| r.id == l.route_id) {
                        if orig_r.distance_estimated {
                            if orig_r.distance_km <= 0.0 {
                                warnings.push(format!(
                                    "Route '{}' has no configured distance or parseable travel_time ('{}'); configure exact distance_km via manage_travel_and_exploration(action='upsert_route').",
                                    orig_r.id, orig_r.canonical_travel_time
                                ));
                            } else {
                                warnings.push(format!(
                                    "Route '{}' distance ({:.1} km) was estimated from prose travel_time '{}'.",
                                    orig_r.id, orig_r.distance_km, orig_r.canonical_travel_time
                                ));
                            }
                        }
                    }
                }
                Ok(TravelPlan {
                    from_place_id: from_id,
                    to_place_id: to_id,
                    viable: true,
                    transport_mode: transport_mode.to_string(),
                    forced_march,
                    season,
                    weather,
                    legs,
                    total_distance_km: total_km,
                    total_travel_hours: travel_hours,
                    rest_stops_required: rest_stops,
                    total_elapsed_hours: total_elapsed,
                    rations_required: rations,
                    tolls_by_currency: tolls,
                    hazards,
                    rerouted_around,
                    encumbrance_notes: enc_notes,
                    injury_fatigue_notes: inj_notes,
                    warnings,
                })
            }
            None => Ok(TravelPlan {
                from_place_id: from_id,
                to_place_id: to_id,
                viable: false,
                transport_mode: transport_mode.to_string(),
                forced_march,
                season,
                weather,
                legs: vec![],
                total_distance_km: 0.0,
                total_travel_hours: 0.0,
                rest_stops_required: 0,
                total_elapsed_hours: 0.0,
                rations_required: 0,
                tolls_by_currency: std::collections::BTreeMap::new(),
                hazards: vec!["No passable route exists between origin and destination".to_string()],
                rerouted_around,
                encumbrance_notes: enc_notes,
                injury_fatigue_notes: inj_notes,
                warnings: vec![],
            }),
        }
    }

    pub fn advance_travel(
        &self,
        from_place_id: &str,
        to_place_id: &str,
        character_ids: &[String],
        transport_mode: &str,
        forced_march: bool,
        pay_tolls_from: Option<&str>,
        hazard_table_id: Option<&str>,
        seed: Option<u64>,
    ) -> Result<(TravelPlan, Option<DrawTableOutcome>, WorldEvent)> {
        let plan = self.calculate_travel(
            from_place_id,
            to_place_id,
            character_ids,
            transport_mode,
            forced_march,
        )?;
        if !plan.viable {
            bail!(
                "Cannot advance travel from '{}' to '{}': no passable route exists (blocked: {:?})",
                plan.from_place_id,
                plan.to_place_id,
                plan.rerouted_around
            );
        }

        // Validate & deduct tolls atomically before moving!
        if let Some(payer) = pay_tolls_from.filter(|s| !s.trim().is_empty()) {
            let payer_id = slugify(payer);
            for (curr, amt) in &plan.tolls_by_currency {
                if *amt > 0 {
                    let _ = self.transfer_currency(
                        &payer_id,
                        "road-tolls-treasury",
                        *amt,
                        curr,
                        &format!("Travel tolls {} -> {}", plan.from_place_id, plan.to_place_id),
                    )?;
                }
            }
        }

        let mins = (plan.total_elapsed_hours * 60.0).round().max(15.0) as u64;
        let (_, _, time_ev) = self.advance_time(
            mins,
            None,
            Some(format!(
                "Overland travel {} -> {} ({:.1} km via {})",
                plan.from_place_id, plan.to_place_id, plan.total_distance_km, plan.transport_mode
            )),
        )?;

        let mut moved_ids = Vec::new();
        for raw_cid in character_ids {
            if let Ok(cid) = resolve_character_id(&self.db, raw_cid) {
                let _ = self.move_character(
                    &cid,
                    &plan.to_place_id,
                    &format!(
                        "Traveled {:.1} km from {} ({:.1}h, {})",
                        plan.total_distance_km,
                        plan.from_place_id,
                        plan.total_elapsed_hours,
                        plan.transport_mode
                    ),
                )?;
                moved_ids.push(cid);
            }
        }

        let hazard_draw = if let Some(tid) = hazard_table_id.filter(|s| !s.trim().is_empty()) {
            Some(self.draw_random_table(DrawTableRequest {
                table_id: tid.to_string(),
                context: crate::mechanics::DrawContext {
                    location: Some(plan.to_place_id.clone()),
                    season: Some(plan.season.clone()),
                    weather: Some(plan.weather.clone()),
                    party: moved_ids.clone(),
                    source_description: Some(format!(
                        "Travel along {} -> {}",
                        plan.from_place_id, plan.to_place_id
                    )),
                    ..Default::default()
                },
                count: 1,
                replacement: true,
                deck_scope: None,
                reset_deck: false,
                seed,
                stage_discovery: None,
                log_to_ledger: true,
                causal_event_ids: vec![time_ev.seq],
            })?)
        } else {
            None
        };

        let mut affected = vec![plan.from_place_id.clone(), plan.to_place_id.clone()];
        affected.extend(moved_ids.clone());
        let ev = self.db.append_event_causal(
            Some(plan.to_place_id.clone()),
            affected,
            format!(
                "Traveled {} -> {} ({:.1} km, {:.1}h, {}){}",
                plan.from_place_id,
                plan.to_place_id,
                plan.total_distance_km,
                plan.total_elapsed_hours,
                plan.transport_mode,
                if plan.rerouted_around.is_empty() {
                    String::new()
                } else {
                    format!(" [detoured around: {}]", plan.rerouted_around.join(", "))
                }
            ),
            EventPayload::TravelResolved {
                from_place_id: plan.from_place_id.clone(),
                to_place_id: plan.to_place_id.clone(),
                transport_mode: plan.transport_mode.clone(),
                distance_km: plan.total_distance_km,
                elapsed_hours: plan.total_elapsed_hours,
                travelers: moved_ids,
                rerouted_around: plan.rerouted_around.clone(),
            },
            EventCausalMeta {
                actor_id: character_ids.first().map(|s| slugify(s)),
                causal_event_ids: vec![time_ev.seq],
                visibility: "public".to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((plan, hazard_draw, ev))
    }

    pub fn explore_area(
        &self,
        place_id: &str,
        actor_id: Option<&str>,
        mode: &str,
        skill: Option<&str>,
        difficulty: Option<i64>,
        table_id: Option<&str>,
        seed: Option<u64>,
    ) -> Result<serde_json::Value> {
        let pid = resolve_place_id(&self.db, place_id)?;
        let place = load_place_projection(&self.db, &pid, None)?;
        let spatial = self.db.get_place_spatial(&pid)?;
        let mut causal_ids = Vec::new();
        let mut findings = Vec::new();

        let check = if let Some(dc) = difficulty {
            let res = self.resolve_check(CheckRequest {
                actor_id: actor_id.map(|s| s.to_string()),
                action: format!("Explore {} ({mode})", place.baseline.name),
                profile: None,
                stat: None,
                skill: skill.map(|s| s.to_string()),
                technique: None,
                equipment: vec![],
                difficulty: dc,
                modifier: 0,
                situational_modifiers: vec![],
                advantage: "none".to_string(),
                dice_override: None,
                auto_hydrate: Some(true),
                stakes: format!("Discovery quality while {mode}ing {}", place.baseline.name),
                seed,
                dry_run: false,
                strict_validation: false,
                log_to_ledger: true,
                causal_event_ids: vec![],
                visibility: "public".to_string(),
            })?;
            if let Some(eid) = res.event_id {
                causal_ids.push(eid);
            }
            Some(res)
        } else {
            None
        };

        let succeeded = check
            .as_ref()
            .map(|c| matches!(c.outcome.as_str(), "critical_success" | "success" | "partial_success"))
            .unwrap_or(true);

        if succeeded {
            for lm in &place.current.landmarks {
                findings.push(format!("Landmark: {} ({}) — {}", lm.name, lm.status, lm.description));
            }
            if let Some(ref sp) = spatial {
                for dep in &sp.resource_deposits {
                    findings.push(format!("Resource deposit: {dep}"));
                }
            }
        }

        let draw = if succeeded {
            if let Some(tid) = table_id.filter(|s| !s.trim().is_empty()) {
                let out = self.draw_random_table(DrawTableRequest {
                    table_id: tid.to_string(),
                    context: crate::mechanics::DrawContext {
                        location: Some(pid.clone()),
                        party: actor_id.into_iter().map(|s| slugify(s)).collect(),
                        source_description: Some(format!("Exploration ({mode}) of {}", place.baseline.name)),
                        ..Default::default()
                    },
                    count: 1,
                    replacement: true,
                    deck_scope: None,
                    reset_deck: false,
                    seed,
                    stage_discovery: None,
                    log_to_ledger: true,
                    causal_event_ids: causal_ids.clone(),
                })?;
                for d in &out.draws {
                    findings.push(format!("Discovery: {}", d.result));
                }
                Some(out)
            } else {
                None
            }
        } else {
            None
        };

        let resolved_actor = actor_id
            .filter(|s| !s.trim().is_empty())
            .map(|a| resolve_character_id(&self.db, a).unwrap_or_else(|_| slugify(a)));
        let ev = self.db.append_event_causal(
            Some(pid.clone()),
            resolved_actor.clone().into_iter().chain(std::iter::once(pid.clone())).collect(),
            format!(
                "Explored {} ({mode}): {} finding(s)",
                place.baseline.name,
                findings.len()
            ),
            EventPayload::AreaExplored {
                place_id: pid.clone(),
                mode: mode.to_string(),
                actor_id: resolved_actor.clone(),
                findings: findings.clone(),
            },
            EventCausalMeta {
                actor_id: resolved_actor,
                causal_event_ids: causal_ids,
                visibility: "public".to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok(serde_json::json!({
            "event_seq": ev.seq,
            "place_id": pid,
            "mode": mode,
            "check": check,
            "findings": findings,
            "table_draw": draw,
            "spatial": spatial
        }))
    }

    // -------------------------------------------------------------------------
    // Economy: Encumbrance, Atomic Multi-Currency & Dynamic Markets
    // -------------------------------------------------------------------------

    pub fn set_item_economics(&self, mut econ: ItemEconomics) -> Result<ItemEconomics> {
        econ.item_key = slugify(&econ.item_key);
        self.db.upsert_item_economics(&econ)?;
        Ok(econ)
    }

    pub fn check_character_encumbrance(&self, character_id: &str) -> Result<EncumbranceReport> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let econ_map = self.db.list_item_economics_map()?;
        Ok(compute_encumbrance(&proj, &econ_map))
    }

    pub fn set_currency_balance(
        &self,
        owner_id: &str,
        currency: &str,
        amount: i64,
        reason: &str,
    ) -> Result<(i64, WorldEvent)> {
        if amount < 0 {
            bail!("Currency balance cannot be set to a negative number ({amount})");
        }
        let oid = slugify(owner_id);
        let curr = normalize_currency_key(currency);
        let now = self.db.get_campaign_meta()?.clock.format_short();
        self.db.set_currency_balance(&oid, &curr, amount, &now)?;
        let ev = self.db.append_event(
            None,
            vec![oid.clone()],
            format!("Balance set for '{oid}': {amount} {curr} ({reason})"),
            EventPayload::TransactionExecuted {
                kind: "set_balance".to_string(),
                from_id: None,
                to_id: Some(oid),
                amount,
                currency: curr,
                item_name: None,
                notes: reason.to_string(),
            },
        )?;
        Ok((amount, ev))
    }

    /// Strictly atomic currency transfer between characters, merchants, or faction treasuries.
    /// Zero silent creation or duplication!
    pub fn transfer_currency(
        &self,
        from_id: &str,
        to_id: &str,
        amount: i64,
        currency: &str,
        reason: &str,
    ) -> Result<(i64, i64, WorldEvent)> {
        if amount <= 0 {
            bail!("Transfer amount must be positive (got {amount})");
        }
        let from = slugify(from_id);
        let to = slugify(to_id);
        if from == to {
            bail!("Cannot transfer currency from '{from}' to itself");
        }
        let curr = normalize_currency_key(currency);
        let from_bal = self.db.get_currency_balance(&from, &curr)?;
        if from_bal < amount {
            bail!(
                "Insufficient {curr} funds for '{from}': has {from_bal}, attempted to transfer {amount}"
            );
        }
        let to_bal = self.db.get_currency_balance(&to, &curr)?;
        let now = self.db.get_campaign_meta()?.clock.format_short();
        let new_from = from_bal - amount;
        let new_to = to_bal + amount;
        self.db.set_currency_balance(&from, &curr, new_from, &now)?;
        self.db.set_currency_balance(&to, &curr, new_to, &now)?;

        let ev = self.db.append_event(
            None,
            vec![from.clone(), to.clone()],
            format!(
                "Transferred {amount} {curr}: {from} ({new_from} left) -> {to} ({new_to} total){}",
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(" — {reason}")
                }
            ),
            EventPayload::TransactionExecuted {
                kind: "transfer".to_string(),
                from_id: Some(from),
                to_id: Some(to),
                amount,
                currency: curr,
                item_name: None,
                notes: reason.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((new_from, new_to, ev))
    }

    /// Atomic trade / purchase / wage / tax transaction: validates buyer funds & tax BEFORE moving money or items!
    pub fn execute_transaction(
        &self,
        kind: &str,
        buyer_id: &str,
        seller_id: &str,
        amount: i64,
        currency: &str,
        tax_rate_pct: i64,
        tax_recipient_id: Option<&str>,
        item_name: Option<&str>,
        item_quantity: i64,
        notes: &str,
    ) -> Result<(serde_json::Value, WorldEvent)> {
        if amount <= 0 {
            bail!("Transaction amount must be positive");
        }
        let buyer = slugify(buyer_id);
        let seller = slugify(seller_id);
        let curr = normalize_currency_key(currency);
        let tax_amt = if tax_rate_pct > 0 {
            ((amount as f64) * (tax_rate_pct as f64 / 100.0)).round() as i64
        } else {
            0
        };
        let total_buyer_cost = amount + tax_amt;

        let buyer_bal = self.db.get_currency_balance(&buyer, &curr)?;
        if buyer_bal < total_buyer_cost {
            bail!(
                "Transaction rejected (atomic guard): '{buyer}' has {buyer_bal} {curr}, needs {total_buyer_cost} ({amount} + {tax_amt} tax). No money or items were modified."
            );
        }

        // Perform atomic balance updates
        let now = self.db.get_campaign_meta()?.clock.format_short();
        let seller_bal = self.db.get_currency_balance(&seller, &curr)?;
        self.db
            .set_currency_balance(&buyer, &curr, buyer_bal - total_buyer_cost, &now)?;
        self.db
            .set_currency_balance(&seller, &curr, seller_bal + amount, &now)?;
        if tax_amt > 0 {
            let tax_to = tax_recipient_id
                .filter(|s| !s.trim().is_empty())
                .map(slugify)
                .unwrap_or_else(|| "local-treasury".to_string());
            let tb = self.db.get_currency_balance(&tax_to, &curr)?;
            self.db
                .set_currency_balance(&tax_to, &curr, tb + tax_amt, &now)?;
        }

        // If an item was purchased and buyer is a character, deliver it cleanly
        if let Some(iname) = item_name.filter(|s| !s.trim().is_empty()) {
            let qty = item_quantity.max(1);
            // If seller is also a character carrying the item, transfer it; otherwise mint from merchant stock
            if self.db.get_character_baseline(&seller)?.is_some()
                && self.db.get_character_baseline(&buyer)?.is_some()
            {
                let _ = self.transfer_item(&seller, &buyer, iname, qty, notes);
            } else if self.db.get_character_baseline(&buyer)?.is_some() {
                let _ = self.add_item(
                    &buyer,
                    Item {
                        name: iname.to_string(),
                        slot: None,
                        quantity: qty,
                        equipped: false,
                        durability: Some("New".to_string()),
                        tags: vec!["purchased".to_string()],
                        description: format!("Purchased from {seller} for {amount} {curr}"),
                    },
                    notes,
                )?;
            }
        }

        let ev = self.db.append_event(
            None,
            vec![buyer.clone(), seller.clone()],
            format!(
                "Transaction [{kind}]: {buyer} paid {amount} {curr} (+{tax_amt} tax) to {seller}{}",
                item_name
                    .map(|n| format!(" for {n} x{}", item_quantity.max(1)))
                    .unwrap_or_default()
            ),
            EventPayload::TransactionExecuted {
                kind: kind.to_string(),
                from_id: Some(buyer.clone()),
                to_id: Some(seller.clone()),
                amount,
                currency: curr.clone(),
                item_name: item_name.map(|s| s.to_string()),
                notes: notes.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((
            serde_json::json!({
                "event_seq": ev.seq,
                "buyer_balance": buyer_bal - total_buyer_cost,
                "seller_balance": seller_bal + amount,
                "tax_paid": tax_amt,
                "currency": curr
            }),
            ev,
        ))
    }

    pub fn upsert_market(&self, mut market: SettlementMarket) -> Result<(SettlementMarket, WorldEvent)> {
        market.place_id = resolve_place_id(&self.db, &market.place_id)
            .unwrap_or_else(|_| slugify(&market.place_id));
        market.supply_origins = market
            .supply_origins
            .into_iter()
            .map(|s| resolve_place_id(&self.db, &s).unwrap_or_else(|_| slugify(&s)))
            .collect();
        for c in &mut market.commodities {
            c.imported_from = c
                .imported_from
                .iter()
                .map(|s| resolve_place_id(&self.db, s).unwrap_or_else(|_| slugify(s)))
                .collect();
        }
        market.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_market(&market)?;
        let ev = self.db.append_event(
            Some(market.place_id.clone()),
            vec![market.place_id.clone()],
            format!(
                "Market '{}' updated ({} commodities).",
                market.name,
                market.commodities.len()
            ),
            EventPayload::MarketUpdated {
                place_id: market.place_id.clone(),
                action: "upsert".to_string(),
                notes: market.notes.clone(),
            },
        )?;
        self.maybe_compile()?;
        Ok((market, ev))
    }

    pub fn get_market_prices(&self, place_id: &str) -> Result<MarketPriceReport> {
        let pid = resolve_place_id(&self.db, place_id).unwrap_or_else(|_| slugify(place_id));
        let market = self
            .db
            .get_market(&pid)?
            .ok_or_else(|| anyhow!("No market configured for place '{pid}'"))?;
        let meta = self.db.get_campaign_meta()?;
        let weather = load_place_projection(&self.db, &pid, None)
            .ok()
            .map(|p| p.current.current_weather)
            .filter(|w| !w.is_empty())
            .unwrap_or_else(|| meta.clock.global_weather.clone());
        let routes = self.all_effective_routes()?;
        Ok(evaluate_market_prices(&market, &routes, &meta.clock.season, &weather))
    }

    pub fn update_market_supply(
        &self,
        place_id: &str,
        commodity_name: &str,
        supply_delta: i64,
        demand_delta: i64,
        reason: &str,
    ) -> Result<(MarketPriceReport, WorldEvent)> {
        let pid = resolve_place_id(&self.db, place_id).unwrap_or_else(|_| slugify(place_id));
        let mut market = self
            .db
            .get_market(&pid)?
            .ok_or_else(|| anyhow!("No market configured for place '{pid}'"))?;
        let Some(c) = market
            .commodities
            .iter_mut()
            .find(|x| x.name.eq_ignore_ascii_case(commodity_name))
        else {
            bail!("Commodity '{commodity_name}' not found in market '{}'", market.name);
        };
        c.supply = (c.supply + supply_delta).max(0);
        c.demand = (c.demand + demand_delta).max(1);
        market.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_market(&market)?;
        let ev = self.db.append_event(
            Some(pid.clone()),
            vec![pid.clone()],
            format!(
                "Market '{}' ({commodity_name}): supply {supply_delta:+}, demand {demand_delta:+} — {reason}",
                market.name
            ),
            EventPayload::MarketUpdated {
                place_id: pid.clone(),
                action: "supply_updated".to_string(),
                notes: reason.to_string(),
            },
        )?;
        self.maybe_compile()?;
        let report = self.get_market_prices(&pid)?;
        Ok((report, ev))
    }

    // -------------------------------------------------------------------------
    // Law, Jurisdictions, Crime & Reputation
    // -------------------------------------------------------------------------

    pub fn upsert_jurisdiction(&self, mut j: JurisdictionRecord) -> Result<(JurisdictionRecord, WorldEvent)> {
        j.id = if j.id.trim().is_empty() {
            slugify(&j.name)
        } else {
            slugify(&j.id)
        };
        j.place_ids = j
            .place_ids
            .into_iter()
            .map(|p| resolve_place_id(&self.db, &p).unwrap_or_else(|_| slugify(&p)))
            .collect();
        j.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_jurisdiction(&j)?;
        let ev = self.db.append_event(
            j.place_ids.first().cloned(),
            vec![j.id.clone()],
            format!("Jurisdiction '{}' saved ({} places).", j.name, j.place_ids.len()),
            EventPayload::LawOrReputationUpdated {
                kind: "jurisdiction".to_string(),
                subject_id: j.id.clone(),
                target_id: j.controlling_faction_id.clone().unwrap_or_default(),
                summary: j.name.clone(),
            },
        )?;
        Ok((j, ev))
    }

    pub fn record_crime(&self, mut crime: CrimeRecord) -> Result<(CrimeRecord, WorldEvent)> {
        crime.jurisdiction_id = slugify(&crime.jurisdiction_id);
        crime.suspect_id = resolve_character_id(&self.db, &crime.suspect_id)
            .unwrap_or_else(|_| slugify(&crime.suspect_id));
        if crime.id.trim().is_empty() {
            crime.id = slugify(&format!("{}-{}", crime.suspect_id, &crime.offense[..crime.offense.len().min(32)]));
        } else {
            crime.id = slugify(&crime.id);
        }
        crime.recorded_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_crime(&crime)?;
        let ev = self.db.append_event(
            None,
            vec![crime.suspect_id.clone(), crime.jurisdiction_id.clone()],
            format!(
                "Crime recorded [{}]: {} — '{}' (status: {}, bounty: {} {})",
                crime.jurisdiction_id,
                crime.suspect_id,
                crime.offense,
                crime.warrant_status,
                crime.bounty,
                crime.bounty_currency
            ),
            EventPayload::LawOrReputationUpdated {
                kind: "crime".to_string(),
                subject_id: crime.suspect_id.clone(),
                target_id: crime.jurisdiction_id.clone(),
                summary: crime.offense.clone(),
            },
        )?;
        self.maybe_compile()?;
        Ok((crime, ev))
    }

    pub fn modify_reputation(
        &self,
        entity_id: &str,
        target_group_id: &str,
        delta: i64,
        notes: &str,
    ) -> Result<(ReputationStanding, WorldEvent)> {
        let eid = resolve_character_id(&self.db, entity_id).unwrap_or_else(|_| slugify(entity_id));
        let tid = slugify(target_group_id);
        let existing = self
            .db
            .list_reputations(Some(&eid), Some(&tid))?
            .into_iter()
            .next();
        let new_score = (existing.map(|r| r.score).unwrap_or(0) + delta).clamp(-100, 100);
        let tier = reputation_tier_label(new_score).to_string();
        let now = self.db.get_campaign_meta()?.clock.format_short();
        let rep = ReputationStanding {
            entity_id: eid.clone(),
            target_group_id: tid.clone(),
            score: new_score,
            tier: tier.clone(),
            notes: notes.to_string(),
            updated_at_world_time: now,
        };
        self.db.upsert_reputation(&rep)?;
        let ev = self.db.append_event(
            None,
            vec![eid.clone(), tid.clone()],
            format!(
                "Reputation {eid} with {tid}: {delta:+} -> {new_score} ({tier}){}",
                if notes.is_empty() {
                    String::new()
                } else {
                    format!(" — {notes}")
                }
            ),
            EventPayload::LawOrReputationUpdated {
                kind: "reputation".to_string(),
                subject_id: eid,
                target_id: tid,
                summary: format!("{new_score} ({tier})"),
            },
        )?;
        self.maybe_compile()?;
        Ok((rep, ev))
    }

    // -------------------------------------------------------------------------
    // Phase 4: Persistent Injuries/Recovery, Downtime/Crafting, Weather & Bounded World Tick
    // -------------------------------------------------------------------------

    pub fn apply_injury(&self, mut inj: InjuryRecord) -> Result<(InjuryRecord, WorldEvent)> {
        inj.character_id = resolve_character_id(&self.db, &inj.character_id)?;
        if inj.id.trim().is_empty() {
            inj.id = slugify(&format!("{}-{}", inj.character_id, inj.name));
        } else {
            inj.id = slugify(&inj.id);
        }
        if inj.recovery_days_total <= 0 {
            inj.recovery_days_total = match inj.severity.to_lowercase().as_str() {
                "minor" => 2,
                "moderate" => 5,
                "severe" => 12,
                "critical" => 25,
                _ => 5,
            };
        }
        if inj.recovery_days_remaining <= 0 {
            inj.recovery_days_remaining = inj.recovery_days_total;
        }
        let now = self.db.get_campaign_meta()?.clock.format_short();
        inj.inflicted_at_world_time = now.clone();
        inj.updated_at_world_time = now;
        inj.status = "active".to_string();
        self.db.upsert_injury(&inj)?;

        // Sync directly into character's live StatusCondition so resolve_check & travel feel it immediately!
        let _ = self.add_condition(
            &inj.character_id,
            StatusCondition {
                name: inj.name.clone(),
                kind: "injury".to_string(),
                severity: inj.severity.clone(),
                duration: format!("{} days remaining", inj.recovery_days_remaining),
                source: inj.body_location.clone(),
                notes: inj.notes.clone(),
            },
        )?;

        let ev = self.db.append_event_causal(
            None,
            vec![inj.character_id.clone(), inj.id.clone()],
            format!(
                "Injury inflicted on {}: {} [{} | {} days recovery]",
                inj.character_id, inj.name, inj.severity, inj.recovery_days_remaining
            ),
            EventPayload::InjuryUpdated {
                injury_id: inj.id.clone(),
                character_id: inj.character_id.clone(),
                action: "inflicted".to_string(),
                severity: inj.severity.clone(),
                days_remaining: inj.recovery_days_remaining,
            },
            EventCausalMeta {
                actor_id: Some(inj.character_id.clone()),
                causal_event_ids: inj.source_event_id.into_iter().collect(),
                visibility: "public".to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((inj, ev))
    }

    pub fn treat_injury(
        &self,
        injury_id: &str,
        healer_notes: &str,
        days_reduced: i64,
    ) -> Result<(InjuryRecord, WorldEvent)> {
        let id = slugify(injury_id);
        let mut inj = self
            .db
            .get_injury(&id)?
            .ok_or_else(|| anyhow!("Injury '{id}' not found"))?;
        inj.treated = true;
        inj.status = "healing".to_string();
        inj.recovery_days_remaining = (inj.recovery_days_remaining - days_reduced.max(0)).max(1);
        inj.complications.clear();
        if !healer_notes.is_empty() {
            inj.notes = format!("{} | Treated: {healer_notes}", inj.notes);
        }
        inj.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_injury(&inj)?;
        let ev = self.db.append_event(
            None,
            vec![inj.character_id.clone(), inj.id.clone()],
            format!(
                "Injury '{}' treated ({} days remaining).",
                inj.name, inj.recovery_days_remaining
            ),
            EventPayload::InjuryUpdated {
                injury_id: inj.id.clone(),
                character_id: inj.character_id.clone(),
                action: "treated".to_string(),
                severity: inj.severity.clone(),
                days_remaining: inj.recovery_days_remaining,
            },
        )?;
        self.maybe_compile()?;
        Ok((inj, ev))
    }

    pub fn advance_injury_recovery(
        &self,
        character_id: &str,
        days: i64,
    ) -> Result<Vec<InjuryRecord>> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let active = self.db.list_injuries(Some(&cid), true)?;
        let now = self.db.get_campaign_meta()?.clock.format_short();
        let mut updated = Vec::new();
        for mut inj in active {
            // Treated injuries heal at full pace; untreated severe/critical heal at half pace!
            let effective_days = if inj.treated
                || matches!(inj.severity.to_lowercase().as_str(), "minor" | "moderate")
            {
                days.max(0)
            } else {
                (days.max(0) + 1) / 2
            };
            inj.recovery_days_remaining = (inj.recovery_days_remaining - effective_days).max(0);
            inj.updated_at_world_time = now.clone();
            if inj.recovery_days_remaining == 0 {
                inj.status = "healed".to_string();
                let _ = self.remove_condition(&cid, &inj.name, "Fully recovered")?;
            } else {
                inj.status = "healing".to_string();
            }
            self.db.upsert_injury(&inj)?;
            let _ = self.db.append_event(
                None,
                vec![cid.clone(), inj.id.clone()],
                format!(
                    "Recovery for {} — '{}': {} ({} days remaining)",
                    cid, inj.name, inj.status, inj.recovery_days_remaining
                ),
                EventPayload::InjuryUpdated {
                    injury_id: inj.id.clone(),
                    character_id: cid.clone(),
                    action: inj.status.clone(),
                    severity: inj.severity.clone(),
                    days_remaining: inj.recovery_days_remaining,
                },
            )?;
            updated.push(inj);
        }
        self.maybe_compile()?;
        Ok(updated)
    }

    pub fn begin_crafting(&self, mut proj: CraftingProject) -> Result<(CraftingProject, WorldEvent)> {
        proj.crafter_id = resolve_character_id(&self.db, &proj.crafter_id)?;
        if proj.id.trim().is_empty() {
            proj.id = slugify(&format!("craft-{}-{}", proj.crafter_id, proj.item_name));
        } else {
            proj.id = slugify(&proj.id);
        }
        proj.status = "in_progress".to_string();
        proj.started_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_crafting_project(&proj)?;
        let ev = self.db.append_event(
            None,
            vec![proj.crafter_id.clone(), proj.id.clone()],
            format!(
                "{} began crafting '{}' ({}h required, DC {}).",
                proj.crafter_id, proj.item_name, proj.required_hours, proj.difficulty
            ),
            EventPayload::DowntimeOrCraftingResolved {
                kind: "begin_crafting".to_string(),
                actor_id: proj.crafter_id.clone(),
                project_id: Some(proj.id.clone()),
                outcome: "in_progress".to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((proj, ev))
    }

    pub fn resolve_crafting(
        &self,
        project_id: &str,
        hours_worked: u64,
        profile: Option<&str>,
        seed: Option<u64>,
    ) -> Result<(CraftingProject, Option<CheckResolution>, WorldEvent)> {
        let pid = slugify(project_id);
        let mut proj = self
            .db
            .get_crafting_project(&pid)?
            .ok_or_else(|| anyhow!("Crafting project '{pid}' not found"))?;
        proj.hours_completed += hours_worked;
        if hours_worked > 0 {
            let _ = self.advance_time(
                hours_worked * 60,
                None,
                Some(format!("Crafting '{}' ({}h)", proj.item_name, hours_worked)),
            )?;
        }

        let mut check_opt = None;
        if proj.hours_completed >= proj.required_hours {
            let check = self.resolve_check(CheckRequest {
                actor_id: Some(proj.crafter_id.clone()),
                action: format!("Complete crafting '{}'", proj.item_name),
                profile: profile.map(|s| s.to_string()),
                stat: None,
                skill: if proj.skill_name.is_empty() {
                    None
                } else {
                    Some(proj.skill_name.clone())
                },
                technique: None,
                equipment: vec![],
                difficulty: proj.difficulty,
                modifier: 0,
                situational_modifiers: vec![],
                advantage: "none".to_string(),
                dice_override: None,
                auto_hydrate: Some(true),
                stakes: "Crafted item quality or wasted materials".to_string(),
                seed,
                dry_run: false,
                strict_validation: false,
                log_to_ledger: true,
                causal_event_ids: vec![],
                visibility: "public".to_string(),
            })?;
            let (status, quality) = match check.outcome.as_str() {
                "critical_success" => ("completed", "masterwork"),
                "success" => ("completed", "fine"),
                "partial_success" => ("completed", "flawed"),
                _ => ("failed", "ruined"),
            };
            proj.status = status.to_string();
            proj.quality = quality.to_string();
            if status == "completed" {
                let item = CatalogItem {
                    id: slugify(&proj.item_name),
                    name: proj.item_name.clone(),
                    item_type: proj.item_type.clone(),
                    rarity_or_tier: quality.to_string(),
                    slot: None,
                    quantity: 1,
                    equipped: false,
                    condition_or_durability: format!("Newly crafted ({quality})"),
                    origin_and_maker: format!("Crafted by {}", proj.crafter_id),
                    previous_owners: vec![],
                    current_holder_id: Some(proj.crafter_id.clone()),
                    significance: format!("Crafted via project '{}'", proj.id),
                    properties_and_effects: proj.properties_and_effects.clone(),
                    description: proj.description.clone(),
                    linked_text_id: None,
                    gm_notes: String::new(),
                    keywords: vec![proj.item_type.clone(), quality.to_string()],
                    tags: vec!["crafted".to_string(), quality.to_string()],
                    related_ids: vec![proj.crafter_id.clone()],
                    created_at_world_time: String::new(),
                    updated_at_world_time: String::new(),
                };
                let _ = self.upsert_catalog_item(item, "Completed crafting project")?;
            }
            check_opt = Some(check);
        }
        self.db.upsert_crafting_project(&proj)?;
        let ev = self.db.append_event(
            None,
            vec![proj.crafter_id.clone(), proj.id.clone()],
            format!(
                "Crafting '{}' ({}/{}h): {} [{}]",
                proj.item_name, proj.hours_completed, proj.required_hours, proj.status, proj.quality
            ),
            EventPayload::DowntimeOrCraftingResolved {
                kind: "resolve_crafting".to_string(),
                actor_id: proj.crafter_id.clone(),
                project_id: Some(proj.id.clone()),
                outcome: format!("{} ({})", proj.status, proj.quality),
            },
        )?;
        self.maybe_compile()?;
        Ok((proj, check_opt, ev))
    }

    pub fn advance_place_weather(
        &self,
        place_id: &str,
        seed: Option<u64>,
    ) -> Result<(String, WorldEvent)> {
        use rand::Rng;
        let pid = resolve_place_id(&self.db, place_id)?;
        let pl = load_place_projection(&self.db, &pid, None)?;
        let meta = self.db.get_campaign_meta()?;
        let s = seed.unwrap_or_else(|| rand::thread_rng().gen::<u64>());
        let next_w = generate_seasonal_weather(
            &meta.clock.season,
            &pl.baseline.climate,
            &pl.current.current_weather,
            s,
        );
        let (_, ev) = self.set_place_weather(&pid, &next_w, &format!("Seasonal weather roll (seed {s})"))?;
        Ok((next_w, ev))
    }

    /// Bounded-scope world advancement orchestrator: advances clock, regional weather, injury recovery,
    /// market production/consumption drift, routine faction upkeep, and optional leak-proof autonomy dossiers!
    #[allow(clippy::too_many_arguments)]
    pub fn advance_world(
        &self,
        duration_hours: u64,
        place_ids: &[String],
        faction_ids: &[String],
        character_ids: &[String],
        tick_weather: bool,
        tick_recovery: bool,
        tick_markets: bool,
        tick_upkeep: bool,
        prepare_dossiers: bool,
        seed: Option<u64>,
    ) -> Result<serde_json::Value> {
        use rand::Rng;
        if duration_hours == 0 {
            bail!("advance_world requires duration_hours > 0");
        }
        let base_seed = seed.unwrap_or_else(|| rand::thread_rng().gen::<u64>());
        let days = ((duration_hours as f64) / 24.0).ceil().max(1.0) as i64;
        let mut sub_events = Vec::new();

        // 1. Advance World Clock
        let (new_clock, season_changed, clock_ev) = self.advance_time(
            duration_hours * 60,
            None,
            Some(format!("Bounded world tick ({duration_hours}h / ~{days}d)")),
        )?;
        sub_events.push(clock_ev.seq);

        // 2. Scoped Regional Weather
        let mut weather_updates = std::collections::BTreeMap::new();
        if tick_weather {
            for (idx, pid) in place_ids.iter().enumerate() {
                if let Ok((w, wev)) = self.advance_place_weather(
                    pid,
                    Some(base_seed.wrapping_add((idx as u64).wrapping_mul(0x9E37))),
                ) {
                    weather_updates.insert(slugify(pid), w);
                    sub_events.push(wev.seq);
                }
            }
        }

        // 3. Scoped Injury Recovery
        let mut recovery_updates = std::collections::BTreeMap::new();
        if tick_recovery {
            for cid in character_ids {
                if let Ok(injs) = self.advance_injury_recovery(cid, days) {
                    if !injs.is_empty() {
                        recovery_updates.insert(slugify(cid), injs);
                    }
                }
            }
        }

        // 4. Scoped Market Production/Consumption Drift
        let mut market_reports = Vec::new();
        if tick_markets {
            for pid in place_ids {
                let rpid = resolve_place_id(&self.db, pid).unwrap_or_else(|_| slugify(pid));
                if let Some(mut mkt) = self.db.get_market(&rpid)? {
                    for c in &mut mkt.commodities {
                        let net = (c.daily_production - c.daily_consumption) * days;
                        c.supply = (c.supply + net).max(0);
                    }
                    mkt.updated_at_world_time = new_clock.format_short();
                    self.db.upsert_market(&mkt)?;
                    if let Ok(rep) = self.get_market_prices(&rpid) {
                        market_reports.push(rep);
                    }
                }
            }
        }

        // 5. Scoped Faction Upkeep & Optional Autonomy Dossiers
        let mut dossiers = Vec::new();
        let mut upkeep_applied = Vec::new();
        for fid in faction_ids {
            if let Ok(fac) = self.get_faction(fid) {
                if tick_upkeep && !fac.members.is_empty() {
                    let drain = (fac.members.len() as i64) * days;
                    for key in ["rations", "grain", "food", "supplies"] {
                        if fac.resources.contains_key(key) {
                            let cur = fac.resources[key];
                            let actual = drain.min(cur);
                            if actual > 0 {
                                let _ = self.modify_faction_resource(
                                    &fac.id,
                                    key,
                                    -actual,
                                    false,
                                    &format!("Routine upkeep ({days}d)"),
                                );
                                upkeep_applied.push(format!("{}: -{actual} {key}", fac.id));
                            }
                            break;
                        }
                    }
                }
                if prepare_dossiers {
                    if let Ok(d) = self.prepare_autonomy_dossier(
                        "faction",
                        Some(&fac.id),
                        &[],
                        duration_hours,
                        fac.territory.first().map(|s| s.as_str()),
                    ) {
                        dossiers.push(d);
                    }
                }
            }
        }

        let summary = format!(
            "World advanced {duration_hours}h -> {} (season_changed={season_changed}, weather={}, recoveries={}, markets={}, upkeep={})",
            new_clock.format_short(),
            weather_updates.len(),
            recovery_updates.len(),
            market_reports.len(),
            upkeep_applied.len()
        );
        let master_ev = self.db.append_event_causal(
            place_ids.first().map(|s| slugify(s)),
            place_ids
                .iter()
                .chain(faction_ids.iter())
                .chain(character_ids.iter())
                .map(|s| slugify(s))
                .collect(),
            summary.clone(),
            EventPayload::WorldAdvanced {
                duration_hours,
                scoped_places: place_ids.iter().map(|s| slugify(s)).collect(),
                scoped_factions: faction_ids.iter().map(|s| slugify(s)).collect(),
                scoped_characters: character_ids.iter().map(|s| slugify(s)).collect(),
                summary,
            },
            EventCausalMeta {
                actor_id: None,
                causal_event_ids: sub_events,
                visibility: "public".to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok(serde_json::json!({
            "event_seq": master_ev.seq,
            "world_clock": new_clock,
            "season_changed": season_changed,
            "weather_updates": weather_updates,
            "recovery_updates": recovery_updates,
            "market_reports": market_reports,
            "upkeep_applied": upkeep_applied,
            "autonomy_dossiers": dossiers
        }))
    }
}

pub(crate) fn normalize_currency_key(raw: &str) -> String {
    let s: String = raw
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = s.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "copper_cash".to_string()
    } else {
        trimmed
    }
}

pub(crate) fn is_carryable_text_medium(medium: &str) -> bool {
    !matches!(
        medium.trim().to_lowercase().as_str(),
        "inscription" | "monument" | "mural" | "carving" | "obelisk"
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn guess_mime_from_ext(ext: Option<&str>) -> &'static str {
    match ext.unwrap_or("") {
        "md" | "markdown" => "text/markdown",
        "txt" | "log" => "text/plain",
        "json" => "application/json",
        "yaml" | "yml" => "text/yaml",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        _ => "application/octet-stream",
    }
}

fn guess_category_from_mime(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        "map"
    } else if mime == "application/pdf" {
        "handout"
    } else if mime.starts_with("audio/") {
        "audio"
    } else if mime.starts_with("text/") || mime == "application/json" {
        "world-doc"
    } else {
        "other"
    }
}

/// Zero-dependency RFC 4648 base64 decoder (ignores whitespace & optional data-URI prefix).
fn decode_base64(input: &str) -> Result<Vec<u8>> {
    let payload = match input.split_once(";base64,") {
        Some((_, rest)) => rest,
        None => input,
    };
    let clean: Vec<u8> = payload
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    if clean.len() % 4 != 0 {
        bail!("Invalid base64 length ({})", clean.len());
    }
    let val = |b: u8| -> Result<u8> {
        match b {
            b'A'..=b'Z' => Ok(b - b'A'),
            b'a'..=b'z' => Ok(b - b'a' + 26),
            b'0'..=b'9' => Ok(b - b'0' + 52),
            b'+' | b'-' => Ok(62),
            b'/' | b'_' => Ok(63),
            _ => bail!("Invalid base64 byte: {}", b as char),
        }
    };
    let mut out = Vec::with_capacity((clean.len() / 4) * 3);
    for chunk in clean.chunks_exact(4) {
        let a = val(chunk[0])?;
        let b = val(chunk[1])?;
        out.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            let c = val(chunk[2])?;
            out.push((b << 4) | (c >> 2));
            if chunk[3] != b'=' {
                let d = val(chunk[3])?;
                out.push((c << 6) | d);
            }
        }
    }
    Ok(out)
}
