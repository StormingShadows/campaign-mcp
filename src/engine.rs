use crate::autonomy::{
    ActorAutonomyBrief, ActorBelief, ActorKnowledgeBrief, AutonomyCommitRequest, AutonomyDossier,
    CharacterJob, CharacterOutfit, CharacterProficiency, CharacterProfileExt, CharacterTitle,
    CharacterTrait, FactionMember, FactionRecord, KnowledgeClaim, KnowledgeRecordView,
    KnowledgeSearchFilter, NpcEngagement, NpcTripState, PhysicalMarker, StatTrainingEntry,
    StructuredGoal,
};
use crate::compiler::{compile_all_docs, prune_orphaned_docs, CompileReport};
use crate::context::{assemble_scene_context, SceneContextPacket, SceneContextRequest};
use crate::db::CampaignDb;
use crate::dice::{roll_dice, DiceRollResult};
use crate::mechanics::{
    calculate_skill_xp_gain, collect_hydrated_modifiers_ext, currency_family, evaluate_check,
    evaluate_xp_rank, execute_table_draw, lookup_skill_definition, normalize_random_table,
    resolve_dc, CheckRequest, CheckResolution, ContestParticipantResult, ContestRequest,
    ContestResolution, DrawTableOutcome, DrawTableRequest, EncounterLogEntry, EncounterParticipant,
    EncounterState, EventCausalMeta, HydrationExtras, HydrationOutcome, ModifierSource,
    RandomTable, RulesetProfile, SkillProgressionOutcome, StagedDiscovery, StagedDiscoveryItem,
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
    compute_encumbrance, compute_shortest_route, evaluate_market_prices,
    format_grid_neighborhood_lines, generate_seasonal_weather, reputation_tier_label,
    synthesize_routes_from_places, travel_injury_factor, CharacterProgressionTrack,
    CraftingProject, CrimeRecord, EncumbranceReport, InjuryRecord, ItemEconomics,
    JurisdictionRecord, MapGrid, MapGridCell, MarketPriceReport, PlaceSpatialExt,
    ProgressionLadder, ReputationStanding, SettlementMarket, TravelPlan, TravelRoute,
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
    compile_defer_depth: std::cell::Cell<u32>,
    compile_dirty: std::cell::Cell<bool>,
}

impl CampaignEngine {
    pub fn open(campaign_dir: &Path) -> Result<Self> {
        let db_path = campaign_dir.join("campaign.db");
        let db = CampaignDb::open(&db_path)?;
        Ok(Self {
            root_dir: campaign_dir.to_path_buf(),
            db,
            auto_compile: true,
            compile_defer_depth: std::cell::Cell::new(0),
            compile_dirty: std::cell::Cell::new(false),
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
        if !self.auto_compile {
            return Ok(());
        }
        if self.compile_defer_depth.get() > 0 {
            self.compile_dirty.set(true);
            return Ok(());
        }
        self.compile_dirty.set(false);
        let _ = self.compile_docs()?;
        Ok(())
    }

    /// Defers Markdown doc compilation until the outermost batch completes, avoiding redundant disk I/O
    /// during multi-step operations (`commit_turn`, `advance_world`, stage rules, and routine ticks).
    pub fn with_deferred_compile<T>(&self, f: impl FnOnce(&Self) -> Result<T>) -> Result<T> {
        let prev = self.compile_defer_depth.get();
        self.compile_defer_depth.set(prev + 1);
        let res = f(self);
        self.compile_defer_depth.set(prev);
        if prev == 0 && self.compile_dirty.get() {
            self.compile_dirty.set(false);
            if self.auto_compile {
                let _ = self.compile_docs()?;
            }
        }
        res
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
        self.advance_time_internal(minutes, global_weather, reason, true)
    }

    pub fn advance_time_no_npc_tick(
        &self,
        minutes: u64,
        global_weather: Option<String>,
        reason: Option<String>,
    ) -> Result<(WorldClock, bool, WorldEvent)> {
        self.advance_time_internal(minutes, global_weather, reason, false)
    }

    fn advance_time_internal(
        &self,
        minutes: u64,
        global_weather: Option<String>,
        reason: Option<String>,
        tick_npc: bool,
    ) -> Result<(WorldClock, bool, WorldEvent)> {
        self.with_deferred_compile(|_| {
            let mut meta = self.db.get_campaign_meta()?;
            let prev_total_hours = meta.clock.total_hours();
            let season_changed = meta.clock.advance(minutes);
            let new_total_hours = meta.clock.total_hours();
            if let Some(w) = global_weather {
                if !w.trim().is_empty() {
                    meta.clock.global_weather = w;
                }
            }
            self.db
                .update_clock_and_session(&meta.clock, meta.current_session)?;

            let now_str = meta.clock.format_short();
            let mut summary = format!(
                "Advanced time by {} min -> {} ({})",
                minutes,
                now_str,
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
            // Auto-tick any time-cadence ProgressClocks & standing CharacterRoutines
            if new_total_hours > prev_total_hours {
                let _ = self.tick_auto_clocks(prev_total_hours, new_total_hours, &now_str);
                let _ = self.tick_character_routines(prev_total_hours, new_total_hours, &now_str);
            }
            // Automatically advance any active/prominent NPC trips & visiting schedules when >= 4 hours pass
            if tick_npc && minutes >= 240 {
                let elapsed_h = (minutes / 60).max(1);
                let seed = (new_total_hours.unsigned_abs()) ^ (ev.seq.unsigned_abs().wrapping_mul(0x9E37_79B9));
                let _ = self.tick_npc_movements(elapsed_h, &[], false, seed);
            }
            // Prolonged wear of tight/outgrown garments across 4+ hours causes lingering chafing/soreness
            if minutes >= 240 {
                let elapsed_h = (minutes / 60).max(1);
                if let Ok(all_chars) = self.db.list_character_baselines() {
                    for ch_b in all_chars {
                        let _ = self.sync_apparel_discomfort(&ch_b.id, elapsed_h);
                    }
                }
            }
            self.maybe_compile()?;
            Ok((meta.clock, season_changed, ev))
        })
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

    /// Awards Skill XP to a character's skill (initializing it from the ruleset's skill_table if new),
    /// automatically promotes rank/proficiency when XP crosses a threshold in `profile.xp_curve`,
    /// and logs the update to the event ledger.
    pub fn grant_skill_xp(
        &self,
        character_id: &str,
        skill_name: &str,
        xp_delta: u64,
        profile_id: Option<&str>,
        reason: &str,
    ) -> Result<(SkillProgressionOutcome, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let profile = self.get_ruleset_profile_or_default(profile_id)?;
        let target_l = skill_name.trim().to_lowercase();
        let table_def = lookup_skill_definition(&profile, skill_name);

        let existing = proj
            .current
            .skills
            .iter()
            .find(|s| s.name.to_lowercase() == target_l)
            .or_else(|| {
                proj.current.skills.iter().find(|s| {
                    let sn = s.name.to_lowercase();
                    sn.contains(&target_l) || target_l.contains(&sn)
                })
            })
            .cloned();

        let mut skill = if let Some(sk) = existing {
            sk
        } else if let Some(def) = table_def {
            Skill {
                name: def.name.clone(),
                rank: "Untrained".to_string(),
                category: def.category.clone(),
                modifier: Some("+0".to_string()),
                description: def.description.clone(),
                governing_stat: Some(def.governing_stat.clone()),
                xp: 0,
                proficiency_tier: Some("untrained".to_string()),
            }
        } else {
            Skill {
                name: skill_name.trim().to_string(),
                rank: "Untrained".to_string(),
                category: String::new(),
                modifier: Some("+0".to_string()),
                description: String::new(),
                governing_stat: None,
                xp: 0,
                proficiency_tier: Some("untrained".to_string()),
            }
        };

        if skill.governing_stat.is_none() {
            if let Some(def) = table_def {
                skill.governing_stat = Some(def.governing_stat.clone());
            }
        }

        let old_rank = skill.rank.clone();
        let old_bonus = skill
            .modifier
            .as_deref()
            .and_then(crate::mechanics::extract_signed_modifier)
            .or_else(|| {
                profile
                    .rank_bonuses
                    .get(&skill.rank.trim().to_lowercase())
                    .copied()
            })
            .unwrap_or(0);

        skill.xp = skill.xp.saturating_add(xp_delta);
        let (xp_tier, next_info) = evaluate_xp_rank(&profile.xp_curve, skill.xp);

        let mut leveled_up = false;
        let mut new_bonus = old_bonus;
        if xp_tier.bonus > old_bonus
            || (old_rank.eq_ignore_ascii_case("untrained")
                && !xp_tier.rank.eq_ignore_ascii_case("untrained"))
        {
            skill.rank = xp_tier.rank.clone();
            skill.modifier = Some(format!("{:+}", xp_tier.bonus));
            skill.proficiency_tier = Some(xp_tier.rank.to_lowercase());
            new_bonus = xp_tier.bonus;
            leveled_up = true;
        }

        let full_reason = if leveled_up {
            format!(
                "+{} XP ({} -> {}, {:+}){}",
                xp_delta,
                old_rank,
                skill.rank,
                new_bonus,
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(" — {reason}")
                }
            )
        } else {
            format!(
                "+{} XP (total {} XP){}",
                xp_delta,
                skill.xp,
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(" — {reason}")
                }
            )
        };

        let skill_name_final = skill.name.clone();
        let total_xp = skill.xp;
        let new_rank = skill.rank.clone();
        let (_, ev) = self.upsert_skill(&cid, skill, &full_reason)?;
        let (next_rank, xp_to_next_rank) = match next_info {
            Some((nr, rem)) => (Some(nr), Some(rem)),
            None => (None, None),
        };
        Ok((
            SkillProgressionOutcome {
                character_id: cid,
                skill_name: skill_name_final,
                xp_gained: xp_delta,
                total_xp,
                xp_to_next_rank,
                next_rank,
                old_rank,
                new_rank,
                new_bonus,
                leveled_up,
            },
            ev,
        ))
    }

    /// Correct, revert (negative delta), set, or reset a character's skill XP.
    /// When `recalc_rank` is true, recalculates rank/modifier/proficiency_tier BOTH upwards and downwards
    /// along the active RulesetProfile's XP curve (so reverting or resetting XP cleanly undoes mistaken promotions).
    #[allow(clippy::too_many_arguments)]
    pub fn adjust_skill_xp(
        &self,
        character_id: &str,
        skill_name: &str,
        delta_xp: Option<i64>,
        set_xp: Option<u64>,
        recalc_rank: bool,
        profile_id: Option<&str>,
        reason: &str,
    ) -> Result<(serde_json::Value, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let profile = self.get_ruleset_profile_or_default(profile_id)?;
        let table_def = lookup_skill_definition(&profile, skill_name);

        let mut skill = if let Some(existing) = proj
            .current
            .skills
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(skill_name))
        {
            existing.clone()
        } else {
            Skill {
                name: table_def
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| skill_name.to_string()),
                rank: "Untrained".to_string(),
                category: String::new(),
                modifier: Some("+0".to_string()),
                description: String::new(),
                governing_stat: table_def.map(|d| d.governing_stat.clone()),
                xp: 0,
                proficiency_tier: Some("untrained".to_string()),
            }
        };

        let old_xp = skill.xp;
        let old_rank = skill.rank.clone();
        let old_mod = skill.modifier.clone().unwrap_or_else(|| "+0".to_string());

        let new_xp = if let Some(v) = set_xp {
            v
        } else if let Some(d) = delta_xp {
            if d >= 0 {
                old_xp.saturating_add(d as u64)
            } else {
                old_xp.saturating_sub(d.unsigned_abs())
            }
        } else {
            old_xp
        };

        skill.xp = new_xp;
        let (xp_tier, next_info) = evaluate_xp_rank(&profile.xp_curve, skill.xp);
        let mut rank_changed = false;
        if recalc_rank {
            let target_mod = format!("{:+}", xp_tier.bonus);
            if !skill.rank.eq_ignore_ascii_case(&xp_tier.rank)
                || skill.modifier.as_deref() != Some(target_mod.as_str())
            {
                rank_changed = true;
            }
            skill.rank = xp_tier.rank.clone();
            skill.modifier = Some(target_mod);
            skill.proficiency_tier = Some(xp_tier.rank.to_lowercase());
        }

        let effective_delta = new_xp as i64 - old_xp as i64;
        let full_reason = format!(
            "XP adjusted {} -> {} ({:+}){}{}",
            old_xp,
            new_xp,
            effective_delta,
            if rank_changed {
                format!(", rank {} ({}) -> {} ({})", old_rank, old_mod, skill.rank, skill.modifier.as_deref().unwrap_or("+0"))
            } else {
                String::new()
            },
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );

        let skill_name_final = skill.name.clone();
        let new_rank = skill.rank.clone();
        let new_mod = skill.modifier.clone();
        let (updated_proj, ev) = self.upsert_skill(&cid, skill, &full_reason)?;
        let (next_rank, xp_to_next_rank) = match next_info {
            Some((nr, rem)) => (Some(nr), Some(rem)),
            None => (None, None),
        };
        Ok((
            serde_json::json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "character_id": cid,
                "skill_name": skill_name_final,
                "old_xp": old_xp,
                "new_xp": new_xp,
                "delta_xp": effective_delta,
                "old_rank": old_rank,
                "new_rank": new_rank,
                "new_modifier": new_mod,
                "rank_changed": rank_changed,
                "next_rank": next_rank,
                "xp_to_next_rank": xp_to_next_rank,
                "skills": updated_proj.current.skills
            }),
            ev,
        ))
    }

    /// Reads any campaign attachment, story text, lore entry, or compiled doc verbatim in exact
    /// UTF-8 character chunks (zero compression, zero summarization, strictly confined inside `<campaign>/`).
    pub fn read_campaign_file_chunk(
        &self,
        target: &str,
        offset_chars: usize,
        chunk_chars: usize,
        gm_override: bool,
    ) -> Result<serde_json::Value> {
        let t = target.trim();
        let docs_root = self.docs_dir();

        if t.is_empty() || t.eq_ignore_ascii_case("list") {
            let atts: Vec<serde_json::Value> = self
                .db
                .list_attachments()?
                .into_iter()
                .map(|a| {
                    serde_json::json!({
                        "target": format!("attachment:{}", a.id),
                        "title": a.title,
                        "mime_type": a.mime_type,
                        "relative_path": a.relative_path,
                        "size_bytes": a.size_bytes
                    })
                })
                .collect();
            let texts: Vec<serde_json::Value> = self
                .db
                .list_texts()?
                .into_iter()
                .map(|tx| {
                    serde_json::json!({
                        "target": format!("text:{}", tx.id),
                        "title": tx.title,
                        "read_status": tx.read_status,
                        "chars": tx.content.chars().count()
                    })
                })
                .collect();
            let mut doc_files = Vec::new();
            fn collect_docs(base: &Path, dir: &Path, out: &mut Vec<String>) {
                if let Ok(rd) = std::fs::read_dir(dir) {
                    for entry in rd.flatten() {
                        let p = entry.path();
                        if p.is_dir() {
                            collect_docs(base, &p, out);
                        } else if let Ok(rel) = p.strip_prefix(base) {
                            out.push(format!("doc:{}", rel.to_string_lossy().replace('\\', "/")));
                        }
                    }
                }
            }
            collect_docs(&docs_root, &docs_root, &mut doc_files);
            doc_files.sort();
            return Ok(serde_json::json!({
                "mode": "list",
                "attachments": atts,
                "texts": texts,
                "docs": doc_files
            }));
        }

        let effective_chunk = chunk_chars.clamp(500, 16000);
        let (resolved_target, title, mime_type, full_text) = if let Some(att_id) =
            t.strip_prefix("attachment:").or_else(|| t.strip_prefix("chronicle://attachments/"))
        {
            let att = self.get_attachment(att_id)?;
            let abs_path = self.attachment_file_path(&att);
            let raw = std::fs::read(&abs_path)
                .with_context(|| format!("Attachment file missing: {}", abs_path.display()))?;
            let text = String::from_utf8(raw).map_err(|_| {
                anyhow!(
                    "Attachment '{}' ({}) is binary and not UTF-8 text; inspect metadata via get_attachment.",
                    att.id,
                    att.mime_type
                )
            })?;
            (
                format!("attachment:{}", att.id),
                att.title,
                att.mime_type,
                text,
            )
        } else if let Some(text_id) =
            t.strip_prefix("text:").or_else(|| t.strip_prefix("chronicle://texts/"))
        {
            let tx = self.get_text(text_id)?;
            if !gm_override
                && matches!(
                    tx.read_status.as_str(),
                    "undiscovered" | "discovered_unread"
                )
            {
                bail!(
                    "Text '{}' has read_status '{}'. Record reading via `record_text_reading` first (or pass `gm_override: true` for GM-only inspection).",
                    tx.id,
                    tx.read_status
                );
            }
            (
                format!("text:{}", tx.id),
                tx.title,
                "text/markdown".to_string(),
                tx.content,
            )
        } else if let Some(lore_id) =
            t.strip_prefix("lore:").or_else(|| t.strip_prefix("chronicle://lore/"))
        {
            let l = self.get_lore(lore_id)?;
            (
                format!("lore:{}", l.id),
                l.title,
                "text/markdown".to_string(),
                l.body,
            )
        } else {
            // Campaign-relative docs path (`doc:...` or `docs/...` or bare relative path under docs/)
            let rel_raw = t
                .strip_prefix("doc:")
                .or_else(|| t.strip_prefix("docs/"))
                .or_else(|| t.strip_prefix("docs\\"))
                .unwrap_or(t)
                .replace('\\', "/");
            if rel_raw.contains("..") || rel_raw.starts_with('/') || rel_raw.contains(':') {
                bail!("Invalid doc path '{t}': path traversal or absolute paths are not permitted.");
            }
            let cand = docs_root.join(&rel_raw);
            let canon_root = std::fs::canonicalize(&docs_root)
                .with_context(|| format!("Campaign docs directory missing: {}", docs_root.display()))?;
            let canon_file = std::fs::canonicalize(&cand)
                .with_context(|| format!("Campaign doc not found: docs/{rel_raw}"))?;
            if !canon_file.starts_with(&canon_root) {
                bail!("Access denied: '{t}' resolves outside campaign docs directory.");
            }
            let raw = std::fs::read(&canon_file)?;
            let text = String::from_utf8(raw).map_err(|_| {
                anyhow!("File 'docs/{rel_raw}' is binary and cannot be read as UTF-8 text.")
            })?;
            (
                format!("doc:{rel_raw}"),
                rel_raw,
                "text/markdown".to_string(),
                text,
            )
        };

        let total_chars = full_text.chars().count();
        let total_chunks = if total_chars == 0 {
            1
        } else {
            (total_chars + effective_chunk - 1) / effective_chunk
        };
        let safe_offset = offset_chars.min(total_chars);
        let content: String = full_text
            .chars()
            .skip(safe_offset)
            .take(effective_chunk)
            .collect();
        let returned_chars = content.chars().count();
        let next_offset = safe_offset + returned_chars;
        let has_more = next_offset < total_chars;
        let chunk_index = (safe_offset / effective_chunk) + 1;

        let mut out = serde_json::json!({
            "verbatim": true,
            "target": resolved_target,
            "title": title,
            "mime_type": mime_type,
            "offset_chars": safe_offset,
            "returned_chars": returned_chars,
            "total_chars": total_chars,
            "chunk_index": chunk_index,
            "total_chunks": total_chunks,
            "has_more": has_more,
            "next_offset_chars": if has_more { serde_json::json!(next_offset) } else { serde_json::Value::Null },
            "content": content
        });
        if has_more {
            out["next_call"] = serde_json::json!({
                "target": resolved_target,
                "offset_chars": next_offset,
                "chunk_chars": effective_chunk
            });
        }
        Ok(out)
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
                remove: false,
                reason: reason.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((load_character_projection(&self.db, &cid, None)?, ev))
    }

    pub fn remove_resource(
        &self,
        character_id: &str,
        resource: &str,
        reason: &str,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let summary = format!(
            "{}: removed resource '{}'{}",
            proj.baseline.name,
            resource,
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
                delta: None,
                set_current: None,
                set_max: None,
                unit: None,
                remove: true,
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
        mut item: Item,
        reason: &str,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        // Canonicalize any garment measurement keys
        if !item.garment_measurements.is_empty() {
            let mut norm = std::collections::BTreeMap::new();
            for (k, v) in item.garment_measurements {
                let nk = crate::autonomy::normalize_measurement_key(&k);
                if !nk.is_empty() && !v.trim().is_empty() {
                    norm.insert(nk, v.trim().to_string());
                }
            }
            item.garment_measurements = norm;
        }
        // Normalize explicit tags and auto-infer primary category tags when acquiring a new item
        item.tags = crate::models::normalize_item_tags(&item.tags);
        if item.quantity > 0 && !crate::models::has_primary_item_category_tag(&item.tags) {
            let inferred = crate::models::infer_item_tags(
                &item.name,
                item.slot.as_deref(),
                None,
                &item.description,
                !item.garment_measurements.is_empty(),
            );
            for t in inferred {
                if !item.tags.iter().any(|e| e == &t) {
                    item.tags.push(t);
                }
            }
        }
        // Hard guard: refuse equipping a garment whose dimensions are too small for the character's current body
        if item.equipped && !item.garment_measurements.is_empty() {
            if let Ok(Some(prof)) = self.db.get_character_profile(&cid) {
                if let Some(rep) = crate::autonomy::evaluate_garment_fit(&item, &prof.avatar) {
                    if !rep.can_equip {
                        bail!(
                            "Cannot equip '{}' on {} — garment is too small to fit: {}. Acquire it unequipped (`equipped: false`) or tailor its dimensions first.",
                            item.name,
                            proj.baseline.name,
                            rep.blocking_reasons.join("; ")
                        );
                    }
                }
            }
        }
        let summary = format!(
            "{} {} {} x{}{}",
            proj.baseline.name,
            if item.quantity == 0 { "updated/tailored" } else { "acquired" },
            item.name,
            if item.quantity == 0 { 1 } else { item.quantity },
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );
        let sync_item = item.clone();
        let ev = self.db.append_event(
            proj.current.current_location.clone(),
            vec![cid.clone()],
            summary,
            EventPayload::ItemAcquired {
                character_id: cid.clone(),
                item,
            },
        )?;
        // Mirror garment sizing/cut/taper/tags onto any matching CatalogItem held by this character
        if !sync_item.garment_measurements.is_empty()
            || !sync_item.garment_cut.is_empty()
            || !sync_item.garment_taper.is_empty()
            || !sync_item.tags.is_empty()
        {
            if let Ok(cat_items) = self.db.list_catalog_items() {
                for mut ci in cat_items {
                    if ci.current_holder_id.as_deref() == Some(cid.as_str())
                        && ci.name.eq_ignore_ascii_case(&sync_item.name)
                    {
                        if !sync_item.garment_measurements.is_empty() {
                            ci.garment_measurements = sync_item.garment_measurements.clone();
                        }
                        if !sync_item.garment_cut.is_empty() {
                            ci.garment_cut = sync_item.garment_cut.clone();
                        }
                        if !sync_item.garment_taper.is_empty() {
                            ci.garment_taper = sync_item.garment_taper.clone();
                        }
                        if sync_item.slot.is_some() {
                            ci.slot = sync_item.slot.clone();
                        }
                        if !sync_item.tags.is_empty() {
                            for t in &sync_item.tags {
                                if !ci.tags.iter().any(|e| e.eq_ignore_ascii_case(t)) {
                                    ci.tags.push(t.clone());
                                }
                            }
                            ci.tags = crate::models::normalize_item_tags(&ci.tags);
                        }
                        let _ = self.db.upsert_catalog_item(&ci);
                    }
                }
            }
        }
        let _ = self.sync_apparel_discomfort(&cid, 0);
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
        if equipped {
            if let Some(existing) = proj
                .current
                .equipment
                .iter()
                .find(|i| i.name.eq_ignore_ascii_case(item_name))
            {
                if !existing.garment_measurements.is_empty() {
                    if let Ok(Some(prof)) = self.db.get_character_profile(&cid) {
                        if let Some(rep) =
                            crate::autonomy::evaluate_garment_fit(existing, &prof.avatar)
                        {
                            if !rep.can_equip {
                                bail!(
                                    "Cannot equip '{}' on {} — garment is too small to fit: {}. It must be tailored/altered or replaced before it can be worn.",
                                    existing.name,
                                    proj.baseline.name,
                                    rep.blocking_reasons.join("; ")
                                );
                            }
                        }
                    }
                }
            }
        }
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
        let _ = self.sync_apparel_discomfort(&cid, 0);
        self.maybe_compile()?;
        Ok((load_character_projection(&self.db, &cid, None)?, ev))
    }

    /// Adds, removes, or replaces tags on an existing item in a character's inventory and mirrors to any matching held CatalogItem.
    pub fn update_item_tags(
        &self,
        character_id: &str,
        item_name: &str,
        add_tags: Vec<String>,
        remove_tags: Vec<String>,
        replace_tags: Option<Vec<String>>,
        reason: &str,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let matched = proj
            .current
            .equipment
            .iter()
            .find(|i| i.name.eq_ignore_ascii_case(item_name))
            .ok_or_else(|| {
                anyhow!(
                    "Item '{}' not found in {}'s inventory.",
                    item_name,
                    proj.baseline.name
                )
            })?;
        let canonical_name = matched.name.clone();
        let add_norm = crate::models::normalize_item_tags(&add_tags);
        let rem_norm = crate::models::normalize_item_tags(&remove_tags);
        let rep_norm = replace_tags.map(|v| crate::models::normalize_item_tags(&v));

        let mut detail_parts = Vec::new();
        if let Some(ref rep) = rep_norm {
            detail_parts.push(format!("set=[{}]", rep.join(", ")));
        }
        if !add_norm.is_empty() {
            detail_parts.push(format!("+{}", add_norm.join(", +")));
        }
        if !rem_norm.is_empty() {
            detail_parts.push(format!("-{}", rem_norm.join(", -")));
        }
        let summary = format!(
            "{} updated tags on '{}' ({}){}",
            proj.baseline.name,
            canonical_name,
            detail_parts.join(" "),
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
            EventPayload::ItemTagsUpdated {
                character_id: cid.clone(),
                item_name: canonical_name.clone(),
                add_tags: add_norm,
                remove_tags: rem_norm,
                replace_tags: rep_norm,
                reason: reason.to_string(),
            },
        )?;
        let updated_proj = load_character_projection(&self.db, &cid, None)?;
        if let Some(updated_item) = updated_proj
            .current
            .equipment
            .iter()
            .find(|i| i.name.eq_ignore_ascii_case(&canonical_name))
        {
            if let Ok(cat_items) = self.db.list_catalog_items() {
                for mut ci in cat_items {
                    if ci.current_holder_id.as_deref() == Some(cid.as_str())
                        && ci.name.eq_ignore_ascii_case(&canonical_name)
                    {
                        ci.tags = updated_item.tags.clone();
                        let _ = self.db.upsert_catalog_item(&ci);
                    }
                }
            }
        }
        self.maybe_compile()?;
        Ok((updated_proj, ev))
    }

    /// Deterministically normalizes existing tags and infers missing primary category tags across a character's inventory.
    pub fn auto_tag_character_inventory(
        &self,
        character_id: &str,
        reason: &str,
    ) -> Result<serde_json::Value> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let mut updates: Vec<(String, Vec<String>)> = Vec::new();
        for item in &proj.current.equipment {
            let mut desired = crate::models::normalize_item_tags(&item.tags);
            if !crate::models::has_primary_item_category_tag(&desired) {
                let inferred = crate::models::infer_item_tags(
                    &item.name,
                    item.slot.as_deref(),
                    None,
                    &item.description,
                    !item.garment_measurements.is_empty(),
                );
                for t in inferred {
                    if !desired.iter().any(|e| e == &t) {
                        desired.push(t);
                    }
                }
            }
            if desired != item.tags {
                updates.push((item.name.clone(), desired));
            }
        }
        let reason_txt = if reason.is_empty() {
            "Auto-classified inventory tags"
        } else {
            reason
        };
        let updated_count = updates.len();
        let mut tagged_items = Vec::new();
        if !updates.is_empty() {
            self.with_deferred_compile(|eng| -> Result<()> {
                for (iname, new_tags) in updates {
                    let _ = eng.update_item_tags(
                        &cid,
                        &iname,
                        vec![],
                        vec![],
                        Some(new_tags.clone()),
                        reason_txt,
                    )?;
                    tagged_items.push(serde_json::json!({ "name": iname, "tags": new_tags }));
                }
                Ok(())
            })?;
        }
        let inv = self.search_inventory(&crate::search::InventorySearchFilter {
            character_id: Some(cid.clone()),
            ..Default::default()
        })?;
        Ok(serde_json::json!({
            "character_id": cid,
            "updated_count": updated_count,
            "tagged_items": tagged_items,
            "tag_facets": inv.tag_facets,
            "total_items": inv.total_inventory_count
        }))
    }

    /// Updates tags directly on a CatalogItem and mirrors the change into the holder's character inventory if held by a character.
    pub fn update_catalog_item_tags(
        &self,
        item_id: &str,
        add_tags: Vec<String>,
        remove_tags: Vec<String>,
        replace_tags: Option<Vec<String>>,
        reason: &str,
    ) -> Result<(CatalogItem, WorldEvent)> {
        let mut item = self.get_catalog_item(item_id)?;
        let meta = self.db.get_campaign_meta()?;
        item.updated_at_world_time = meta.clock.format_short();
        if let Some(rep) = replace_tags {
            item.tags = crate::models::normalize_item_tags(&rep);
        } else {
            item.tags = crate::models::normalize_item_tags(&item.tags);
        }
        let rem_norm = crate::models::normalize_item_tags(&remove_tags);
        if !rem_norm.is_empty() {
            item.tags.retain(|t| !rem_norm.iter().any(|r| r == t));
        }
        for t in crate::models::normalize_item_tags(&add_tags) {
            if !item.tags.iter().any(|e| e == &t) {
                item.tags.push(t);
            }
        }
        self.db.upsert_catalog_item(&item)?;
        if let Some(ref holder) = item.current_holder_id {
            if self.db.get_character_baseline(holder)?.is_some() {
                let _ = self.update_item_tags(
                    holder,
                    &item.name,
                    vec![],
                    vec![],
                    Some(item.tags.clone()),
                    if reason.is_empty() {
                        "Synced tags from catalog item"
                    } else {
                        reason
                    },
                );
            }
        }
        let ev = self.db.append_event(
            item.current_holder_id.clone(),
            vec![item.id.clone()],
            format!(
                "Updated tags on catalog item '{}': [{}]{}",
                item.name,
                item.tags.join(", "),
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(" — {reason}")
                }
            ),
            EventPayload::ItemCataloged {
                item_id: item.id.clone(),
                action: "tagged".to_string(),
                holder_id: item.current_holder_id.clone(),
                notes: reason.to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((item, ev))
    }

    pub fn search_inventory(
        &self,
        filter: &crate::search::InventorySearchFilter,
    ) -> Result<crate::search::InventorySearchResult> {
        crate::search::search_character_inventory(&self.db, filter)
    }

    /// Idempotently synchronizes persistent apparel discomfort status conditions from a character's
    /// live equipped garment fit (`odd_taper`/`baggy` -> awkward drape condition; `tight_restrictive`/
    /// `cannot_fit_too_small` -> binding & chafing debuff; 4+ hours of tight/outgrown wear -> lingering
    /// `Chafed & Sore` recovery condition until rested). Automatically clears active garment discomfort
    /// once well-fitted clothes are worn.
    pub fn sync_apparel_discomfort(&self, character_id: &str, prolonged_hours: u64) -> Result<()> {
        const COND_AWKWARD: &str = "Apparel Discomfort: Awkward Drape & Hitching";
        const COND_BINDING: &str = "Apparel Discomfort: Binding & Chafing";
        const COND_CHAFED: &str = "Chafed & Sore (Prolonged Tight Garments)";

        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let has_sized_equipped = proj
            .current
            .equipment
            .iter()
            .any(|i| i.equipped && !i.garment_measurements.is_empty());
        let has_active_apparel_cond = proj.current.conditions.iter().any(|c| {
            c.name.eq_ignore_ascii_case(COND_AWKWARD) || c.name.eq_ignore_ascii_case(COND_BINDING)
        });
        if !has_sized_equipped && !has_active_apparel_cond {
            return Ok(());
        }

        let Some(prof) = self.db.get_character_profile(&cid)? else {
            return Ok(());
        };
        let app = crate::autonomy::evaluate_equipped_apparel_fit(&proj.current.equipment, &prof.avatar);

        let existing_awk = proj
            .current
            .conditions
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(COND_AWKWARD));
        let existing_bind = proj
            .current
            .conditions
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(COND_BINDING));
        let existing_chafed = proj
            .current
            .conditions
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(COND_CHAFED));

        match app.discomfort_tier.as_str() {
            "awkward_drape" => {
                if existing_bind.is_some() {
                    let _ = self.remove_condition(
                        &cid,
                        COND_BINDING,
                        "Switched out of tight/outgrown garments",
                    );
                }
                let ill_names: Vec<String> = app
                    .reports
                    .iter()
                    .filter(|r| {
                        matches!(
                            r.fit_status.as_str(),
                            "odd_taper" | "baggy" | "baggy_odd_taper"
                        )
                    })
                    .map(|r| format!("{} ({})", r.item_name, r.fit_status))
                    .collect();
                let desired_notes = format!(
                    "Nagging tugging, bunching, and constant adjustment from ill-cut or baggy garments ({}). Live appearance/movement penalties are auto-applied by apparel fit.",
                    ill_names.join(", ")
                );
                let needs_upsert = existing_awk
                    .map(|c| c.notes != desired_notes)
                    .unwrap_or(true);
                if needs_upsert {
                    let _ = self.add_condition(
                        &cid,
                        StatusCondition {
                            name: COND_AWKWARD.to_string(),
                            kind: "condition".to_string(),
                            severity: "ongoing".to_string(),
                            duration: "While wearing ill-fitting clothes".to_string(),
                            source: "Equipped Apparel Fit".to_string(),
                            notes: desired_notes,
                        },
                    );
                }
            }
            "binding_chafing" | "painful_outgrown" => {
                if existing_awk.is_some() {
                    let _ = self.remove_condition(
                        &cid,
                        COND_AWKWARD,
                        "Escalated to tight/binding garment discomfort",
                    );
                }
                let is_outgrown = app.discomfort_tier == "painful_outgrown";
                let pen = if is_outgrown { -2 } else { -1 };
                let sev = if is_outgrown { "moderate" } else { "minor" };
                let desired_notes = format!(
                    "{pen} [scope: concentration, meditation, wisdom, stealth, sleep, rest] Pinching seams and tight binding irritate skin, distract focus, and disturb breathing/rest: {}",
                    app.brief
                );
                let needs_upsert = existing_bind
                    .map(|c| c.notes != desired_notes || c.severity != sev)
                    .unwrap_or(true);
                if needs_upsert {
                    let _ = self.add_condition(
                        &cid,
                        StatusCondition {
                            name: COND_BINDING.to_string(),
                            kind: "debuff".to_string(),
                            severity: sev.to_string(),
                            duration: "While wearing tight/outgrown clothes".to_string(),
                            source: "Equipped Apparel Fit".to_string(),
                            notes: desired_notes,
                        },
                    );
                }
                if prolonged_hours >= 4 && !existing_chafed {
                    let _ = self.add_condition(
                        &cid,
                        StatusCondition {
                            name: COND_CHAFED.to_string(),
                            kind: "condition".to_string(),
                            severity: "minor".to_string(),
                            duration: "Until rested".to_string(),
                            source: "Prolonged Tight Garment Wear".to_string(),
                            notes: "-1 [scope: concentration, meditation, wisdom, stealth, sleep, rest] Lingering red welt lines and chafed soreness from hours spent in tight or outgrown clothes; persists after unequipping until rested.".to_string(),
                        },
                    );
                }
            }
            _ => {
                if existing_awk.is_some() {
                    let _ = self.remove_condition(
                        &cid,
                        COND_AWKWARD,
                        "Apparel now fits cleanly or was unequipped",
                    );
                }
                if existing_bind.is_some() {
                    let _ = self.remove_condition(
                        &cid,
                        COND_BINDING,
                        "Tight/outgrown garments removed or tailored",
                    );
                }
            }
        }
        Ok(())
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
        mut technique: Technique,
        reason: &str,
    ) -> Result<(CharacterProjection, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        self.resolve_manual_link(
            &mut technique.source_text_id,
            &mut technique.source_item_id,
            &mut technique.manual_title,
        );
        if let Some(ref tid) = technique.progression_track_id {
            let trimmed = tid.trim();
            technique.progression_track_id = if trimmed.is_empty() {
                None
            } else {
                Some(slugify(trimmed))
            };
        }
        let mut involved = vec![cid.clone()];
        if let Some(ref st) = technique.source_text_id {
            involved.push(st.clone());
        }
        if let Some(ref si) = technique.source_item_id {
            if !involved.contains(si) {
                involved.push(si.clone());
            }
        }
        if let Some(ref pt) = technique.progression_track_id {
            if !involved.contains(pt) {
                involved.push(pt.clone());
            }
        }
        let manual_tag = if !technique.manual_title.is_empty() {
            format!(" [Manual: {}]", technique.manual_title)
        } else if let Some(ref st) = technique.source_text_id {
            format!(" [Manual: {st}]")
        } else {
            String::new()
        };
        let summary = format!(
            "{} learned/updated technique '{}' ({}){}{}",
            proj.baseline.name,
            technique.name,
            if technique.mastery.is_empty() {
                &technique.tier
            } else {
                &technique.mastery
            },
            manual_tag,
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );
        let ev = self.db.append_event(
            proj.current.current_location.clone(),
            involved,
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
        item.tags = crate::models::normalize_item_tags(&item.tags);
        if !crate::models::has_primary_item_category_tag(&item.tags) {
            let inferred = crate::models::infer_item_tags(
                &item.name,
                item.slot.as_deref(),
                Some(&item.item_type),
                &item.description,
                !item.garment_measurements.is_empty(),
            );
            for t in inferred {
                if !item.tags.iter().any(|e| e == &t) {
                    item.tags.push(t);
                }
            }
        }

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
                if !already_has || !item.garment_measurements.is_empty() || !item.garment_cut.is_empty() || !item.garment_taper.is_empty() {
                    let mut tags = item.tags.clone();
                    if !tags.iter().any(|t| t.eq_ignore_ascii_case(&item.item_type)) {
                        tags.push(item.item_type.clone());
                    }
                    let inv_item = Item {
                        name: item.name.clone(),
                        slot: item.slot.clone(),
                        quantity: if already_has { 0 } else { item.quantity },
                        equipped: item.equipped,
                        durability: Some(item.condition_or_durability.clone()),
                        tags,
                        description: if item.description.is_empty() {
                            item.significance.clone()
                        } else {
                            item.description.clone()
                        },
                        garment_measurements: item.garment_measurements.clone(),
                        garment_cut: item.garment_cut.clone(),
                        garment_taper: item.garment_taper.clone(),
                    };
                    let _ = self.db.append_event(
                        proj.current.current_location.clone(),
                        vec![cur_h.clone(), item.id.clone()],
                        format!(
                            "{} {} '{}' [{}]",
                            proj.baseline.name,
                            if already_has { "updated sizing for" } else { "carries" },
                            item.name,
                            item.item_type
                        ),
                        EventPayload::ItemAcquired {
                            character_id: cur_h.clone(),
                            item: inv_item,
                        },
                    )?;
                    let _ = self.sync_apparel_discomfort(cur_h, 0);
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
            ..Default::default()
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
        if clock.cadence_hours.is_some() && clock.last_tick_total_hours.is_none() {
            if let Ok(meta) = self.db.get_campaign_meta() {
                clock.last_tick_total_hours = Some(meta.clock.total_hours());
            }
        }

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
            garment_measurements: existing.garment_measurements,
            garment_cut: existing.garment_cut,
            garment_taper: existing.garment_taper,
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
            // Auto-clear lingering garment chafing during rest unless still sleeping in tight/outgrown clothes
            if proj
                .current
                .conditions
                .iter()
                .any(|c| c.name.eq_ignore_ascii_case("Chafed & Sore (Prolonged Tight Garments)"))
            {
                let still_tight = self
                    .db
                    .get_character_profile(&cid)
                    .ok()
                    .flatten()
                    .map(|prof| {
                        let tier = crate::autonomy::evaluate_equipped_apparel_fit(
                            &proj.current.equipment,
                            &prof.avatar,
                        )
                        .discomfort_tier;
                        tier == "binding_chafing" || tier == "painful_outgrown"
                    })
                    .unwrap_or(false);
                if !still_tight {
                    let (_, ev) = self.remove_condition(
                        &cid,
                        "Chafed & Sore (Prolonged Tight Garments)",
                        &format!("Skin chafing recovered during {rest_type} rest"),
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

        let actor_prof = resolved_actor_id
            .as_deref()
            .and_then(|cid| self.db.get_character_profile(cid).ok().flatten());
        let active_injuries = resolved_actor_id
            .as_deref()
            .and_then(|cid| self.db.list_injuries(Some(cid), false).ok())
            .unwrap_or_default();
        let actor_encumbrance = actor_proj.as_ref().map(|ch| {
            let econ_map = self.db.list_item_economics_map().unwrap_or_default();
            compute_encumbrance(ch, &econ_map)
        });
        let mut hydrated = if hydrate {
            collect_hydrated_modifiers_ext(
                &profile,
                actor_proj.as_ref(),
                actor_prof.as_ref(),
                place_proj.as_ref(),
                req.stat.as_deref(),
                req.skill.as_deref(),
                req.technique.as_deref(),
                &req.equipment,
                &req.action,
                HydrationExtras {
                    injuries: &active_injuries,
                    encumbrance: actor_encumbrance.as_ref(),
                },
            )
        } else {
            HydrationOutcome::default()
        };

        // Hydrate active stateful progression track modifier (explicit `progression_track` or tag match)
        if hydrate {
            if let Some(ref cid) = resolved_actor_id {
                if let Ok(tracks) = self.db.list_character_progressions(cid) {
                    let skill_l = req.skill.as_deref().unwrap_or("").trim().to_lowercase();
                    let action_l = req.action.to_lowercase();
                    let explicit_tr = req
                        .progression_track
                        .as_deref()
                        .map(|s| s.trim().to_lowercase())
                        .filter(|s| !s.is_empty());
                    let mut matched_explicit = false;
                    for tr in &tracks {
                        if matches!(tr.status.as_str(), "halted" | "crippled") {
                            continue;
                        }
                        let is_explicit = explicit_tr.as_ref().is_some_and(|e| {
                            tr.track_id.to_lowercase() == *e
                                || tr.name.to_lowercase() == *e
                                || tr.name.to_lowercase().contains(e)
                        });
                        if is_explicit {
                            matched_explicit = true;
                        }
                        let tag_match = tr.check_tags.iter().any(|tag| {
                            let tl = tag.trim().to_lowercase();
                            !tl.is_empty()
                                && ((!skill_l.is_empty()
                                    && (skill_l.contains(&tl)
                                        || slugify(&skill_l) == slugify(&tl)))
                                    || (tl.len() >= 4 && action_l.contains(&tl)))
                        });
                        if (is_explicit || tag_match) && tr.check_modifier != 0 {
                            hydrated.modifiers.push(ModifierSource {
                                source: format!("progression:{}", tr.track_id),
                                value: tr.check_modifier,
                                category: "progression".to_string(),
                                notes: format!("{} — {}", tr.name, tr.format_stage()),
                            });
                        }
                    }
                    if let Some(ref exp_name) = explicit_tr {
                        if !matched_explicit {
                            let msg = format!(
                                "Unknown progression track '{}' on '{}' (not enrolled; treated as +0).",
                                exp_name, cid
                            );
                            hydrated.warnings.push(msg.clone());
                            hydrated.missing_refs.push(msg);
                        }
                    }
                }
            }
        }

        if req.strict_validation && !hydrated.missing_refs.is_empty() {
            bail!(
                "Strict validation failed for check '{}': {}",
                req.action,
                hydrated.missing_refs.join(" | ")
            );
        }

        let (final_dc, dc_breakdown) = resolve_dc(
            &profile,
            if req.dc_tier.is_some() && req.difficulty == 0 {
                None
            } else {
                Some(req.difficulty)
            },
            req.dc_tier.as_deref(),
            &req.dc_factors,
        )?;
        req.difficulty = final_dc;

        let mut res = evaluate_check(
            &profile,
            &req,
            hydrated.modifiers,
            hydrated.warnings,
            location_id.clone(),
        )?;
        res.dc_breakdown = dc_breakdown;

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
                    actor_id: resolved_actor_id.clone(),
                    causal_event_ids: req.causal_event_ids,
                    visibility: req.visibility,
                },
            )?;
            res.event_id = Some(ev.seq);

            // Automatic difficulty-scaled Skill XP progression on live skill checks!
            let should_award_xp = req.award_skill_xp.unwrap_or(profile.auto_skill_xp);
            if should_award_xp {
                if let (Some(ref cid), Some(ref sk_name)) =
                    (&resolved_actor_id, req.skill.as_deref().filter(|s| !s.trim().is_empty()))
                {
                    let has_skill_or_standard = actor_proj.as_ref().is_some_and(|ch| {
                        let tl = sk_name.trim().to_lowercase();
                        ch.current.skills.iter().any(|s| {
                            let sn = s.name.to_lowercase();
                            sn == tl || sn.contains(&tl) || tl.contains(&sn)
                        })
                    }) || lookup_skill_definition(&profile, sk_name).is_some();

                    if has_skill_or_standard {
                        let xp_gain =
                            calculate_skill_xp_gain(&res.outcome, res.difficulty, &profile.xp_curve);
                        if xp_gain > 0 {
                            if let Ok((prog, _)) = self.grant_skill_xp(
                                cid,
                                sk_name,
                                xp_gain,
                                Some(&profile.id),
                                &format!("Check '{}' ({})", res.action, res.outcome),
                            ) {
                                if prog.leveled_up {
                                    let _ = self.emit_signal(crate::signals::EngineSignal {
                                        id: String::new(),
                                        dedup_key: format!("skill_rank_up:{cid}:{}:{}", slugify(&prog.skill_name), prog.new_rank),
                                        code: "SKILL_RANK_UP".to_string(),
                                        severity: "info".to_string(),
                                        category: "progression".to_string(),
                                        title: format!("Skill Rank Up: '{}' -> {}", prog.skill_name, prog.new_rank),
                                        summary: format!(
                                            "{} advanced '{}' from {} to {} (proficiency {:+}).",
                                            actor_proj.as_ref().map(|c| c.baseline.name.as_str()).unwrap_or(cid),
                                            prog.skill_name,
                                            prog.old_rank,
                                            prog.new_rank,
                                            prog.new_bonus
                                        ),
                                        entity_ids: vec![cid.clone()],
                                        location_id: res.location_id.clone(),
                                        suggested_actions: vec![],
                                        visibility: "player".to_string(),
                                        status: "active".to_string(),
                                        source_event_seq: res.event_id,
                                        created_at_world_time: String::new(),
                                        updated_at_world_time: String::new(),
                                    });
                                }
                                res.skill_progression = Some(prog);
                            }

                            // Coupled Activity XP: trickle into governing Stat Training XP & matching active Jobs!
                            if profile.auto_stat_and_job_xp {
                                let (st_gained, jb_gained) = self
                                    .award_coupled_activity_xp(
                                        cid,
                                        sk_name,
                                        req.stat.as_deref(),
                                        xp_gain,
                                        &profile,
                                        res.event_id,
                                    )
                                    .unwrap_or((None, Vec::new()));
                                res.stat_xp_gained = st_gained;
                                res.job_xp_gained = jb_gained;
                            }
                        }
                    }
                }
            }

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

            let actor_prof = self.db.get_character_profile(&cid).ok().flatten();
            let p_injuries = self.db.list_injuries(Some(&cid), false).unwrap_or_default();
            let econ_map = self.db.list_item_economics_map().unwrap_or_default();
            let p_enc = compute_encumbrance(&proj, &econ_map);
            let hydrate = p.auto_hydrate.unwrap_or(profile.auto_hydrate);
            let hydrated = if hydrate {
                collect_hydrated_modifiers_ext(
                    &profile,
                    Some(&proj),
                    actor_prof.as_ref(),
                    place.as_ref(),
                    p.stat.as_deref(),
                    p.skill.as_deref(),
                    p.technique.as_deref(),
                    &p.equipment,
                    &req.action,
                    HydrationExtras {
                        injuries: &p_injuries,
                        encumbrance: Some(&p_enc),
                    },
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
                progression_track: p.progression_track.clone(),
                equipment: p.equipment.clone(),
                difficulty: 0,
                dc_tier: None,
                dc_factors: vec![],
                modifier: p.modifier,
                situational_modifiers: p.situational_modifiers.clone(),
                advantage: p.advantage.clone(),
                dice_override: p.dice_override.clone(),
                auto_hydrate: Some(hydrate),
                award_skill_xp: Some(false),
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
                        ..Default::default()
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
                        ..Default::default()
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
            new_p.entity_id = pid.clone();
            enc.participants.push(new_p);
            enc.sort_initiative();
        }
        self.db.upsert_encounter(&enc)?;
        let _ = self.db.append_event(
            enc.location_id.clone(),
            vec![eid.clone(), pid.clone()],
            format!("Encounter '{}' participant updated: {}", enc.title, pid),
            EventPayload::NarrativeEvent {
                title: format!("Encounter Participant Updated: {pid}"),
                details: format!("Updated participant '{pid}' in encounter '{}'", enc.title),
                tags: vec!["encounter".to_string(), "participant".to_string()],
            },
        )?;
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

        // Deep-merge nested `avatar` object AND support flat avatar/measurement convenience keys
        const AVATAR_FLAT_KEYS: &[&str] = &[
            "avatar_summary",
            "appearance_stage",
            "height",
            "weight",
            "baseline_height",
            "baseline_weight",
            "chronological_age",
            "apparent_age",
            "lifespan_years",
            "body_type_or_build",
            "silhouette",
            "posture_and_gait",
            "proportions_summary",
            "clothing_fit",
            "perceived_change_note",
            "transformation_trajectory",
            "face_shape_and_features",
            "eyes",
            "skin_tone_and_texture",
            "hair",
            "facial_hair",
            "voice_and_speech",
            "scent_or_aura",
            "gender",
            "species",
            "portrait_attachment_id",
            "visual_prompt_anchor",
            "active_outfit",
        ];
        let replace_measurements = patch
            .get("replace_measurements")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let replace_baseline_measurements = patch
            .get("replace_baseline_measurements")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let record_checkpoint_val = patch
            .as_object_mut()
            .and_then(|m| m.remove("record_checkpoint"));

        let merge_meas_map =
            |avatar_obj: &mut serde_json::Map<String, serde_json::Value>,
             field_name: &str,
             incoming: &serde_json::Map<String, serde_json::Value>,
             replace: bool| {
                let mut cur_m = if replace {
                    serde_json::Map::new()
                } else {
                    avatar_obj
                        .get(field_name)
                        .and_then(|x| x.as_object())
                        .cloned()
                        .unwrap_or_default()
                };
                for (mk, mv) in incoming {
                    let norm_k = crate::autonomy::normalize_measurement_key(mk);
                    if norm_k.is_empty() {
                        continue;
                    }
                    let s = mv.as_str().map(str::trim).unwrap_or("");
                    if s.is_empty() && mv.is_string() {
                        cur_m.remove(mk);
                        cur_m.remove(&norm_k);
                    } else {
                        if mk != &norm_k {
                            cur_m.remove(mk);
                        }
                        let val = if let Some(st) = mv.as_str() {
                            serde_json::Value::String(st.trim().to_string())
                        } else {
                            mv.clone()
                        };
                        cur_m.insert(norm_k, val);
                    }
                }
                avatar_obj.insert(field_name.to_string(), serde_json::Value::Object(cur_m));
            };

        if let (Some(dst), Some(src)) = (merged_val.as_object_mut(), patch.as_object()) {
            let mut avatar_obj = dst
                .get("avatar")
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            let mut avatar_touched = false;

            if let Some(nested_av) = src.get("avatar").and_then(|v| v.as_object()) {
                for (ak, av) in nested_av {
                    if av.is_null() {
                        continue;
                    }
                    if ak == "measurements" {
                        if let Some(new_m) = av.as_object() {
                            merge_meas_map(&mut avatar_obj, "measurements", new_m, replace_measurements);
                            avatar_touched = true;
                            fields_updated.push("avatar.measurements".to_string());
                        }
                    } else if ak == "baseline_measurements" {
                        if let Some(new_m) = av.as_object() {
                            merge_meas_map(
                                &mut avatar_obj,
                                "baseline_measurements",
                                new_m,
                                replace_baseline_measurements,
                            );
                            avatar_touched = true;
                            fields_updated.push("avatar.baseline_measurements".to_string());
                        }
                    } else {
                        avatar_obj.insert(ak.clone(), av.clone());
                        avatar_touched = true;
                        fields_updated.push(format!("avatar.{ak}"));
                    }
                }
            }

            if let Some(top_m) = src.get("measurements").and_then(|v| v.as_object()) {
                merge_meas_map(&mut avatar_obj, "measurements", top_m, replace_measurements);
                avatar_touched = true;
                fields_updated.push("avatar.measurements".to_string());
            }
            if let Some(top_bm) = src.get("baseline_measurements").and_then(|v| v.as_object()) {
                merge_meas_map(
                    &mut avatar_obj,
                    "baseline_measurements",
                    top_bm,
                    replace_baseline_measurements,
                );
                avatar_touched = true;
                fields_updated.push("avatar.baseline_measurements".to_string());
            }

            for (k, v) in src {
                if k == "character_id"
                    || k == "avatar"
                    || k == "measurements"
                    || k == "replace_measurements"
                    || k == "baseline_measurements"
                    || k == "replace_baseline_measurements"
                    || v.is_null()
                {
                    continue;
                }
                if AVATAR_FLAT_KEYS.contains(&k.as_str()) {
                    avatar_obj.insert(k.clone(), v.clone());
                    avatar_touched = true;
                    fields_updated.push(format!("avatar.{k}"));
                } else {
                    dst.insert(k.clone(), v.clone());
                    fields_updated.push(k.clone());
                }
            }

            if avatar_touched {
                dst.insert("avatar".to_string(), serde_json::Value::Object(avatar_obj));
            }
        }

        let mut profile: CharacterProfileExt = serde_json::from_value(merged_val)?;
        profile.character_id = cid.clone();
        if let Some(cp_val) = record_checkpoint_val {
            let now_str = self.db.get_campaign_meta()?.clock.format_short();
            let stage = cp_val
                .get("stage")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    if !profile.avatar.appearance_stage.is_empty() {
                        profile.avatar.appearance_stage.clone()
                    } else {
                        "Checkpoint".to_string()
                    }
                });
            let world_time = cp_val
                .get("world_time")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or(now_str);
            let summary = cp_val
                .get("summary")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            let height = cp_val
                .get("height")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| profile.avatar.height.clone());
            let weight = cp_val
                .get("weight")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| profile.avatar.weight.clone());
            let meas = if let Some(obj) = cp_val.get("measurements").and_then(|v| v.as_object()) {
                obj.iter()
                    .filter_map(|(k, v)| {
                        let nk = crate::autonomy::normalize_measurement_key(k);
                        let vs = v.as_str().map(str::trim).unwrap_or("").to_string();
                        if nk.is_empty() || vs.is_empty() {
                            None
                        } else {
                            Some((nk, vs))
                        }
                    })
                    .collect()
            } else {
                profile.avatar.measurements.clone()
            };
            let cp = crate::autonomy::AppearanceCheckpoint {
                stage: stage.clone(),
                world_time,
                summary,
                height,
                weight,
                measurements: meas,
            };
            if let Some(ex) = profile
                .avatar
                .transformation_log
                .iter_mut()
                .find(|x| x.stage.eq_ignore_ascii_case(&stage))
            {
                *ex = cp;
            } else {
                profile.avatar.transformation_log.push(cp);
            }
            fields_updated.push("avatar.transformation_log".to_string());
        }

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
                character_id: cid.clone(),
                fields_updated,
            },
        )?;
        let _ = self.sync_apparel_discomfort(&cid, 0);
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
        let _ = self.db.append_event(
            Some(sp.place_id.clone()),
            vec![sp.place_id.clone()],
            format!("Spatial data updated for place '{}'", sp.place_id),
            EventPayload::NarrativeEvent {
                title: format!("Place Spatial Updated: {}", sp.place_id),
                details: format!(
                    "Updated spatial/coordinate metadata for '{}'",
                    sp.place_id
                ),
                tags: vec!["spatial".to_string(), "place".to_string()],
            },
        )?;
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
                progression_track: None,
                equipment: vec![],
                difficulty: dc,
                dc_tier: None,
                dc_factors: vec![],
                modifier: 0,
                situational_modifiers: vec![],
                advantage: "none".to_string(),
                dice_override: None,
                auto_hydrate: Some(true),
                award_skill_xp: None,
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
        let _ = self.db.append_event(
            None,
            vec![econ.item_key.clone()],
            format!(
                "Item economics set for '{}' (weight {:.2} kg, unit value {:.2} {})",
                econ.item_key, econ.weight_kg, econ.unit_value, econ.currency
            ),
            EventPayload::NarrativeEvent {
                title: format!("Item Economics Set: {}", econ.item_key),
                details: format!(
                    "Weight {:.2} kg, volume {:.2} L, unit value {:.2} {}",
                    econ.weight_kg, econ.volume_liters, econ.unit_value, econ.currency
                ),
                tags: vec!["economy".to_string(), "item-economics".to_string()],
            },
        )?;
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
        let now = self.db.get_campaign_meta()?.clock.format_short();
        let mut from_bal = self.db.get_currency_balance(&from, &curr)?;
        let mut conversion_note = String::new();
        if from_bal < amount {
            if let Some(note) = self.try_make_currency_change(&from, &curr, amount, &now)? {
                from_bal = self.db.get_currency_balance(&from, &curr)?;
                conversion_note = format!(" [{note}]");
            }
        }
        if from_bal < amount {
            bail!(
                "Insufficient {curr} funds for '{from}': has {from_bal}, attempted to transfer {amount}"
            );
        }
        let to_bal = self.db.get_currency_balance(&to, &curr)?;
        let new_from = from_bal - amount;
        let new_to = to_bal + amount;
        self.db.set_currency_balance(&from, &curr, new_from, &now)?;
        self.db.set_currency_balance(&to, &curr, new_to, &now)?;

        let ev = self.db.append_event(
            None,
            vec![from.clone(), to.clone()],
            format!(
                "Transferred {amount} {curr}: {from} ({new_from} left) -> {to} ({new_to} total){conversion_note}{}",
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
        let now = self.db.get_campaign_meta()?.clock.format_short();

        let mut buyer_bal = self.db.get_currency_balance(&buyer, &curr)?;
        let mut conversion_note: Option<String> = None;
        if buyer_bal < total_buyer_cost {
            if let Some(note) =
                self.try_make_currency_change(&buyer, &curr, total_buyer_cost, &now)?
            {
                buyer_bal = self.db.get_currency_balance(&buyer, &curr)?;
                conversion_note = Some(note);
            }
        }
        if buyer_bal < total_buyer_cost {
            bail!(
                "Transaction rejected (atomic guard): '{buyer}' has {buyer_bal} {curr}, needs {total_buyer_cost} ({amount} + {tax_amt} tax). No money or items were modified."
            );
        }

        // Perform atomic balance updates
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
                        ..Default::default()
                    },
                    notes,
                )?;
            }
        }

        let conv_suffix = conversion_note
            .as_ref()
            .map(|n| format!(" [{n}]"))
            .unwrap_or_default();
        let ev = self.db.append_event(
            None,
            vec![buyer.clone(), seller.clone()],
            format!(
                "Transaction [{kind}]: {buyer} paid {amount} {curr} (+{tax_amt} tax) to {seller}{}{conv_suffix}",
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
        let all_buyer_balances = self.db.list_currency_balances(&buyer).unwrap_or_default();
        Ok((
            serde_json::json!({
                "event_seq": ev.seq,
                "buyer_balance": buyer_bal - total_buyer_cost,
                "buyer_all_balances": all_buyer_balances,
                "seller_balance": seller_bal + amount,
                "tax_paid": tax_amt,
                "currency": curr,
                "currency_conversion": conversion_note,
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
                progression_track: None,
                equipment: vec![],
                difficulty: proj.difficulty,
                dc_tier: None,
                dc_factors: vec![],
                modifier: 0,
                situational_modifiers: vec![],
                advantage: "none".to_string(),
                dice_override: None,
                auto_hydrate: Some(true),
                award_skill_xp: None,
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
                    ..Default::default()
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
    /// market production/consumption drift, routine faction upkeep, prominent NPC business/networking travel,
    /// and optional leak-proof autonomy dossiers!
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
        tick_npc_movements: bool,
        seed: Option<u64>,
    ) -> Result<serde_json::Value> {
        use rand::Rng;
        if duration_hours == 0 {
            bail!("advance_world requires duration_hours > 0");
        }
        self.with_deferred_compile(|_| {
            let base_seed = seed.unwrap_or_else(|| rand::thread_rng().gen::<u64>());
            let days = ((duration_hours as f64) / 24.0).ceil().max(1.0) as i64;
            let mut sub_events = Vec::new();

            // 1. Advance World Clock (without premature NPC tick; Step 6 orchestrates NPC movements once)
            let (new_clock, season_changed, clock_ev) = self.advance_time_no_npc_tick(
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

            // 6. Prominent Figure Business Travel, Circuits & Networking Simulation
            let npc_movements = if tick_npc_movements {
                self.tick_npc_movements(duration_hours, character_ids, true, base_seed)
                    .unwrap_or_else(|e| serde_json::json!({ "error": e.to_string() }))
            } else {
                serde_json::json!({ "skipped": true })
            };
            let moved_count = npc_movements
                .get("events_count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            let summary = format!(
                "World advanced {duration_hours}h -> {} (season_changed={season_changed}, weather={}, recoveries={}, markets={}, upkeep={}, npc_movements={moved_count})",
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
            let active_signals = self
                .list_active_signals(Some("gm"), place_ids.first().map(String::as_str), character_ids, true)
                .unwrap_or_default();
            Ok(serde_json::json!({
                "event_seq": master_ev.seq,
                "world_clock": new_clock,
                "season_changed": season_changed,
                "weather_updates": weather_updates,
                "recovery_updates": recovery_updates,
                "market_reports": market_reports,
                "upkeep_applied": upkeep_applied,
                "npc_movements": npc_movements,
                "autonomy_dossiers": dossiers,
                "signals": active_signals
            }))
        })
    }

    // -------------------------------------------------------------------------
    // Stateful Progression Engine (Xianxia Realms 1-9, Parallel Manuals,
    // Sci-Fi Tiers, D&D Levels, Bottlenecks & Breakthroughs)
    // -------------------------------------------------------------------------

    /// Resolves and cross-links cultivation manual references (`source_text_id`, `source_item_id`, `manual_title`)
    /// against existing StoryTexts (Books & Scrolls) and CatalogItems so links always resolve cleanly by slug or title.
    fn resolve_manual_link(
        &self,
        source_text_id: &mut Option<String>,
        source_item_id: &mut Option<String>,
        manual_title: &mut String,
    ) {
        if let Some(ref s) = source_text_id {
            if s.trim().is_empty() {
                *source_text_id = None;
            }
        }
        if let Some(ref s) = source_item_id {
            if s.trim().is_empty() {
                *source_item_id = None;
            }
        }

        if let Some(raw) = source_text_id.clone() {
            let trimmed = raw.trim();
            let slug = slugify(trimmed);
            let found_text = self
                .db
                .get_text(trimmed)
                .ok()
                .flatten()
                .or_else(|| self.db.get_text(&slug).ok().flatten())
                .or_else(|| {
                    self.db.list_texts().ok().and_then(|list| {
                        list.into_iter().find(|t| t.title.eq_ignore_ascii_case(trimmed))
                    })
                });
            if let Some(tx) = found_text {
                *source_text_id = Some(tx.id.clone());
                if manual_title.trim().is_empty() {
                    *manual_title = tx.title.clone();
                }
                if source_item_id.is_none() {
                    if let Ok(items) = self.db.list_catalog_items() {
                        if let Some(it) = items
                            .into_iter()
                            .find(|i| i.linked_text_id.as_deref() == Some(tx.id.as_str()))
                        {
                            *source_item_id = Some(it.id);
                        }
                    }
                }
            } else {
                *source_text_id = Some(slug);
            }
        } else if let Some(raw_item) = source_item_id.clone() {
            let trimmed = raw_item.trim();
            let slug = slugify(trimmed);
            let found_item = self
                .db
                .get_catalog_item(trimmed)
                .ok()
                .flatten()
                .or_else(|| self.db.get_catalog_item(&slug).ok().flatten())
                .or_else(|| {
                    self.db.list_catalog_items().ok().and_then(|list| {
                        list.into_iter().find(|i| i.name.eq_ignore_ascii_case(trimmed))
                    })
                });
            if let Some(it) = found_item {
                *source_item_id = Some(it.id.clone());
                if manual_title.trim().is_empty() {
                    *manual_title = it.name.clone();
                }
                if source_text_id.is_none() {
                    *source_text_id = it.linked_text_id.clone();
                }
            } else {
                *source_item_id = Some(slug);
            }
        }
    }

    pub fn define_progression_ladder(
        &self,
        mut ladder: ProgressionLadder,
    ) -> Result<(ProgressionLadder, WorldEvent)> {
        if ladder.name.trim().is_empty() {
            bail!("Progression ladder `name` cannot be empty");
        }
        if ladder.stages.is_empty() {
            bail!("Progression ladder must define at least one stage");
        }
        if ladder.id.trim().is_empty() {
            ladder.id = slugify(&ladder.name);
        } else {
            ladder.id = slugify(&ladder.id);
        }
        self.resolve_manual_link(
            &mut ladder.source_text_id,
            &mut ladder.source_item_id,
            &mut ladder.manual_title,
        );
        ladder.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_progression_ladder(&ladder)?;
        let summary = format!(
            "Progression ladder '{}' [{}] defined ({} major stage(s))",
            ladder.name,
            ladder.kind,
            ladder.stages.len()
        );
        let mut involved = vec![ladder.id.clone()];
        if let Some(ref st) = ladder.source_text_id {
            involved.push(st.clone());
        }
        let ev = self.db.append_event(
            None,
            involved,
            summary,
            EventPayload::NarrativeEvent {
                title: format!("Progression Ladder Defined: {}", ladder.name),
                details: format!(
                    "Ladder '{}' ({}) with {} major stages.",
                    ladder.id,
                    ladder.kind,
                    ladder.stages.len()
                ),
                tags: vec!["progression".into(), "ladder".into()],
            },
        )?;
        self.maybe_compile()?;
        Ok((ladder, ev))
    }

    pub fn list_progression_ladders(&self) -> Result<Vec<ProgressionLadder>> {
        self.db.list_progression_ladders()
    }

    pub fn list_character_progressions(
        &self,
        character_id: &str,
    ) -> Result<Vec<CharacterProgressionTrack>> {
        let cid = resolve_character_id(&self.db, character_id)?;
        self.db.list_character_progressions(&cid)
    }

    pub fn enroll_progression_track(
        &self,
        mut track: CharacterProgressionTrack,
        reason: &str,
    ) -> Result<(CharacterProgressionTrack, WorldEvent)> {
        let cid = resolve_character_id(&self.db, &track.character_id)?;
        track.character_id = cid.clone();
        if track.track_id.trim().is_empty() {
            track.track_id = slugify(&track.name);
        } else {
            track.track_id = slugify(&track.track_id);
        }
        if track.name.trim().is_empty() {
            track.name = track.track_id.clone();
        }
        if track.status.trim().is_empty() {
            track.status = "active".to_string();
        }

        // Hydrate stage defaults from ladder if `ladder_id` is linked
        if let Some(ref lid) = track.ladder_id {
            let lid_slug = slugify(lid);
            if let Some(ladder) = self.db.get_progression_ladder(&lid_slug)? {
                track.ladder_id = Some(ladder.id.clone());
                let st_idx = (track.major_stage_index as usize).min(ladder.stages.len().saturating_sub(1));
                if let Some(st) = ladder.stages.get(st_idx) {
                    if track.major_stage_name.trim().is_empty() {
                        track.major_stage_name = st.name.clone();
                    }
                    track.sub_stage_max = st.sub_stages.max(1);
                    track.sub_stage = track.sub_stage.clamp(1, track.sub_stage_max);
                    track.sub_stage_label = st.format_sub_stage(track.sub_stage);
                    if track.xp_to_next == 0 || track.xp_to_next == 100 {
                        track.xp_to_next = st.xp_for_sub_stage(track.sub_stage);
                    }
                    if track.check_modifier == 0 {
                        track.check_modifier = st.passive_modifier;
                    }
                    if track.active_properties.is_empty() {
                        track.active_properties = st.properties.clone();
                    }
                    if track.halt_conditions.is_empty() {
                        track.halt_conditions = st.halt_conditions.clone();
                    }
                    if track.check_tags.is_empty() {
                        track.check_tags = ladder.default_check_tags.clone();
                    }
                    if track.source_text_id.is_none() {
                        track.source_text_id = ladder.source_text_id.clone();
                    }
                    if track.source_item_id.is_none() {
                        track.source_item_id = ladder.source_item_id.clone();
                    }
                    if track.manual_title.trim().is_empty() && !ladder.manual_title.is_empty() {
                        track.manual_title = ladder.manual_title.clone();
                    }
                    if track.manual_notes.trim().is_empty() && !ladder.manual_notes.is_empty() {
                        track.manual_notes = ladder.manual_notes.clone();
                    }
                }
            }
        } else if track.sub_stage_label.is_empty() && track.sub_stage_max > 1 {
            track.sub_stage_label = format!("Layer {}/{}", track.sub_stage, track.sub_stage_max);
        }

        self.resolve_manual_link(
            &mut track.source_text_id,
            &mut track.source_item_id,
            &mut track.manual_title,
        );

        track.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_character_progression(&track)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let manual_tag = if !track.manual_title.is_empty() {
            format!(" (Manual: {})", track.manual_title)
        } else if let Some(ref st) = track.source_text_id {
            format!(" (Manual: {st})")
        } else {
            String::new()
        };
        let summary = format!(
            "{} enrolled progression track '{}' [{}] at {}{}{}",
            proj.baseline.name,
            track.name,
            track.track_kind,
            track.format_stage(),
            manual_tag,
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );
        let mut involved = vec![cid.clone(), track.track_id.clone()];
        if let Some(ref st) = track.source_text_id {
            involved.push(st.clone());
        }
        let ev = self.db.append_event_causal(
            proj.current.current_location.clone(),
            involved,
            summary,
            EventPayload::ProgressionResolved {
                actor_id: cid,
                progression_kind: format!("enroll:{}", track.track_id),
                outcome: track.format_stage(),
                notes: reason.to_string(),
            },
            EventCausalMeta {
                actor_id: Some(track.character_id.clone()),
                causal_event_ids: vec![],
                visibility: "public".to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((track, ev))
    }

    pub fn advance_progression_xp(
        &self,
        character_id: &str,
        track_id: &str,
        xp_delta: u64,
        reason: &str,
    ) -> Result<(serde_json::Value, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let tid = slugify(track_id);
        let mut track = self
            .db
            .get_character_progression(&cid, &tid)?
            .ok_or_else(|| {
                anyhow!(
                    "Progression track '{tid}' not enrolled on character '{cid}'. Enroll it first via manage_progression(action: 'enroll_track')."
                )
            })?;
        if track.status.eq_ignore_ascii_case("halted") || track.status.eq_ignore_ascii_case("crippled") {
            bail!(
                "Progression track '{}' on '{}' is currently '{}' (halt conditions: {:?}). Clear the halt condition via update_track before advancing XP.",
                track.name,
                cid,
                track.status,
                track.halt_conditions
            );
        }

        let ladder = match track.ladder_id.as_deref() {
            Some(lid) => self.db.get_progression_ladder(lid)?,
            None => None,
        };
        let old_stage_str = track.format_stage();
        track.cumulative_xp = track.cumulative_xp.saturating_add(xp_delta);
        track.current_xp = track.current_xp.saturating_add(xp_delta);

        let mut stages_advanced: u32 = 0;
        let mut major_promoted = false;
        let mut hit_bottleneck_now = false;
        let mut entered_stages: Vec<(u32, u32, String)> = Vec::new();

        loop {
            let need = track.xp_to_next.max(1);
            if track.current_xp < need {
                break;
            }
            if track.sub_stage < track.sub_stage_max {
                track.current_xp -= need;
                track.sub_stage += 1;
                stages_advanced += 1;
                if let Some(ref lad) = ladder {
                    if let Some(st) = lad.stages.get(track.major_stage_index as usize) {
                        track.sub_stage_label = st.format_sub_stage(track.sub_stage);
                        track.xp_to_next = st.xp_for_sub_stage(track.sub_stage);
                    }
                } else if track.sub_stage_max > 1 {
                    track.sub_stage_label = format!("Layer {}/{}", track.sub_stage, track.sub_stage_max);
                }
                entered_stages.push((track.major_stage_index, track.sub_stage, track.format_stage()));
            } else {
                // Reached peak sub-stage of current major stage!
                let requires_bt = ladder
                    .as_ref()
                    .and_then(|l| l.stages.get(track.major_stage_index as usize))
                    .map(|s| s.breakthrough_required_at_peak)
                    .unwrap_or(true);
                let has_next_major = ladder
                    .as_ref()
                    .map(|l| ((track.major_stage_index + 1) as usize) < l.stages.len())
                    .unwrap_or(false);

                if requires_bt || !has_next_major {
                    if !track.at_bottleneck && has_next_major {
                        hit_bottleneck_now = true;
                    }
                    if has_next_major {
                        track.at_bottleneck = true;
                        let next_name = ladder
                            .as_ref()
                            .and_then(|l| l.stages.get((track.major_stage_index + 1) as usize))
                            .map(|s| s.name.clone())
                            .unwrap_or_else(|| "Next Major Realm".to_string());
                        if track.bottleneck_reason.is_empty() {
                            track.bottleneck_reason =
                                format!("Peak reached — breakthrough check required to enter {next_name}");
                        }
                    } else {
                        track.status = "completed".to_string();
                    }
                    break;
                } else if let Some(ref lad) = ladder {
                    // Auto-promote across flat levels (e.g., D&D Level 1 -> Level 2 -> ...)
                    track.current_xp -= need;
                    track.major_stage_index += 1;
                    let next_st = &lad.stages[track.major_stage_index as usize];
                    track.major_stage_name = next_st.name.clone();
                    track.sub_stage_max = next_st.sub_stages.max(1);
                    track.sub_stage = 1;
                    track.sub_stage_label = next_st.format_sub_stage(1);
                    track.xp_to_next = next_st.xp_for_sub_stage(1);
                    track.check_modifier = next_st.passive_modifier;
                    for prop in &next_st.properties {
                        if !track.active_properties.contains(prop) {
                            track.active_properties.push(prop.clone());
                        }
                    }
                    stages_advanced += 1;
                    major_promoted = true;
                    entered_stages.push((track.major_stage_index, track.sub_stage, track.format_stage()));
                }
            }
        }

        self.with_deferred_compile(|_| {
            track.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
            self.db.upsert_character_progression(&track)?;
            let proj = load_character_projection(&self.db, &cid, None)?;
            let new_stage_str = track.format_stage();
            let summary = format!(
                "{} '{}' +{} XP (total {}): {} -> {}{}{}",
                proj.baseline.name,
                track.name,
                xp_delta,
                track.cumulative_xp,
                old_stage_str,
                new_stage_str,
                if track.at_bottleneck { " [AT BOTTLENECK]" } else { "" },
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(" — {reason}")
                }
            );
            let ev = self.db.append_event_causal(
                proj.current.current_location.clone(),
                vec![cid.clone(), track.track_id.clone()],
                summary.clone(),
                EventPayload::ProgressionResolved {
                    actor_id: cid.clone(),
                    progression_kind: format!("xp:{}", track.track_id),
                    outcome: new_stage_str.clone(),
                    notes: summary.clone(),
                },
                EventCausalMeta {
                    actor_id: Some(cid.clone()),
                    causal_event_ids: vec![],
                    visibility: "public".to_string(),
                },
            )?;

            // Execute declarative StageTransitionRules for every newly entered stage/sub-stage!
            let mut applied_rules = Vec::new();
            for (m_idx, s_stg, stg_label) in &entered_stages {
                if let Some(rule) = Self::lookup_stage_rule(&track, ladder.as_ref(), *m_idx, *s_stg) {
                    if let Ok(rep) = self.apply_stage_transition_rule(&cid, &track.name, stg_label, &rule, Some(ev.seq)) {
                        applied_rules.push(rep);
                    }
                }
            }

            let sync_notice: Option<String> = if stages_advanced > 0 && applied_rules.is_empty() {
                self.db.get_character_profile(&cid).ok().flatten().and_then(|prof| {
                    let traj = prof.avatar.transformation_trajectory.trim();
                    let stg = prof.avatar.appearance_stage.trim();
                    if !traj.is_empty() {
                        let msg = format!(
                            "STAGE ADVANCED ({old_stage_str} -> {new_stage_str}) on a transforming character! Immediately update ALL axes in the same turn via manage_player_character(action='update_avatar'), modify_character_stat, and update_character_condition (or attach declarative `stage_rules` on the track). Active Trajectory: {traj}"
                        );
                        let _ = self.emit_signal(crate::signals::EngineSignal {
                            id: String::new(),
                            dedup_key: format!("appearance_sync_due:{cid}:{}:{}", track.track_id, new_stage_str),
                            code: "APPEARANCE_SYNC_DUE".to_string(),
                            severity: "critical".to_string(),
                            category: "appearance".to_string(),
                            title: format!("{} — Stage Transition Requires Appearance/Stat Sync ({new_stage_str})", proj.baseline.name),
                            summary: msg.clone(),
                            entity_ids: vec![cid.clone(), track.track_id.clone()],
                            location_id: proj.current.current_location.clone(),
                            suggested_actions: vec![format!("character(action='pc_sheet', sub_action='update_avatar', character_id='{cid}')")],
                            visibility: "gm_secret".to_string(),
                            status: "active".to_string(),
                            source_event_seq: Some(ev.seq),
                            created_at_world_time: String::new(),
                            updated_at_world_time: String::new(),
                        });
                        Some(msg)
                    } else if !stg.is_empty() {
                        Some(format!(
                            "STAGE ADVANCED ({old_stage_str} -> {new_stage_str}) on a character with appearance_stage '{stg}'. Check whether avatar measurements, qualitative traits, clothing fit, stats, and conditions need to advance now."
                        ))
                    } else {
                        None
                    }
                })
            } else {
                None
            };

            self.maybe_compile()?;
            let mut resp = serde_json::json!({
                "event_seq": ev.seq,
                "summary": summary,
                "xp_gained": xp_delta,
                "stages_advanced": stages_advanced,
                "major_promoted": major_promoted,
                "hit_bottleneck": hit_bottleneck_now,
                "at_bottleneck": track.at_bottleneck,
                "track": track
            });
            if !applied_rules.is_empty() {
                resp["stage_rules_applied"] = serde_json::json!(applied_rules);
            }
            if let Some(notice) = sync_notice {
                resp["appearance_sync_required"] = serde_json::json!(true);
                resp["appearance_sync_notice"] = serde_json::json!(notice);
            }
            Ok((resp, ev))
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn attempt_progression_breakthrough(
        &self,
        character_id: &str,
        track_id: &str,
        profile_id: Option<&str>,
        stat: Option<String>,
        skill: Option<String>,
        explicit_dc: Option<i64>,
        dc_tier: Option<String>,
        dc_factors: Vec<crate::mechanics::DcFactorInput>,
        modifier: i64,
        advantage: String,
        stakes: String,
        seed: Option<u64>,
        next_stage_name_override: Option<String>,
    ) -> Result<(serde_json::Value, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let tid = slugify(track_id);
        let mut track = self
            .db
            .get_character_progression(&cid, &tid)?
            .ok_or_else(|| anyhow!("Progression track '{tid}' not enrolled on '{cid}'"))?;
        if track.status.eq_ignore_ascii_case("halted") || track.status.eq_ignore_ascii_case("crippled") {
            bail!(
                "Cannot attempt breakthrough while track '{}' is '{}' (halt conditions: {:?}).",
                track.name,
                track.status,
                track.halt_conditions
            );
        }

        let ladder = match track.ladder_id.as_deref() {
            Some(lid) => self.db.get_progression_ladder(lid)?,
            None => None,
        };
        let cur_stage_def = ladder
            .as_ref()
            .and_then(|l| l.stages.get(track.major_stage_index as usize))
            .cloned();
        let next_stage_def = ladder
            .as_ref()
            .and_then(|l| l.stages.get((track.major_stage_index + 1) as usize))
            .cloned();

        let resolved_dc_opt = explicit_dc.or_else(|| cur_stage_def.as_ref().and_then(|s| s.breakthrough_dc));
        let resolved_tier = dc_tier.or_else(|| {
            if resolved_dc_opt.is_none() {
                cur_stage_def
                    .as_ref()
                    .and_then(|s| s.breakthrough_dc_tier.clone())
                    .or_else(|| Some("hard".to_string()))
            } else {
                None
            }
        });

        let target_label = next_stage_name_override
            .clone()
            .or_else(|| next_stage_def.as_ref().map(|s| s.name.clone()))
            .unwrap_or_else(|| format!("Next Stage ({})", track.major_stage_index + 2));

        let check = self.resolve_check(CheckRequest {
            actor_id: Some(cid.clone()),
            action: format!("Breakthrough '{}' -> {}", track.name, target_label),
            profile: profile_id.map(|s| s.to_string()),
            stat,
            skill,
            technique: None,
            progression_track: Some(track.track_id.clone()),
            equipment: vec![],
            difficulty: resolved_dc_opt.unwrap_or(0),
            dc_tier: resolved_tier,
            dc_factors,
            modifier,
            situational_modifiers: vec![],
            advantage,
            dice_override: None,
            auto_hydrate: Some(true),
            award_skill_xp: None,
            stakes: if stakes.is_empty() {
                format!("Advance into {target_label} vs stalled bottleneck / backlash")
            } else {
                stakes
            },
            seed,
            dry_run: false,
            strict_validation: false,
            log_to_ledger: true,
            causal_event_ids: vec![],
            visibility: "public".to_string(),
        })?;

        let old_stage = track.format_stage();
        let succeeded = matches!(check.outcome.as_str(), "success" | "critical_success");
        if succeeded {
            track.major_stage_index += 1;
            track.at_bottleneck = false;
            track.bottleneck_reason.clear();
            let carried_overflow = track.current_xp.saturating_sub(track.xp_to_next);
            track.current_xp = carried_overflow;
            if let Some(ns) = next_stage_def {
                track.major_stage_name = next_stage_name_override.unwrap_or(ns.name.clone());
                track.sub_stage_max = ns.sub_stages.max(1);
                track.sub_stage = 1;
                track.sub_stage_label = ns.format_sub_stage(1);
                track.xp_to_next = ns.xp_for_sub_stage(1);
                track.check_modifier = ns.passive_modifier;
                for prop in &ns.properties {
                    if !track.active_properties.contains(prop) {
                        track.active_properties.push(prop.clone());
                    }
                }
                track.halt_conditions = ns.halt_conditions;
            } else {
                track.major_stage_name = target_label.clone();
                track.sub_stage = 1;
                track.sub_stage_label = if track.sub_stage_max > 1 {
                    format!("Layer 1/{}", track.sub_stage_max)
                } else {
                    String::new()
                };
                track.check_modifier += 1;
            }
        } else {
            track.at_bottleneck = true;
            if track.bottleneck_reason.is_empty() {
                track.bottleneck_reason =
                    format!("Breakthrough into {target_label} failed ({})", check.outcome);
            }
        }

        track.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_character_progression(&track)?;
        let summary = if succeeded {
            format!(
                "BREAKTHROUGH SUCCEEDED ({}) — '{}' advanced from {} to {}",
                check.outcome,
                track.name,
                old_stage,
                track.format_stage()
            )
        } else {
            format!(
                "Breakthrough failed ({}) — '{}' remains at {} (bottleneck active)",
                check.outcome,
                track.name,
                track.format_stage()
            )
        };
        let causal = check.event_id.into_iter().collect();
        let sync_notice: Option<String> = if succeeded {
            self.db.get_character_profile(&cid).ok().flatten().and_then(|prof| {
                let traj = prof.avatar.transformation_trajectory.trim();
                let stg = prof.avatar.appearance_stage.trim();
                if !traj.is_empty() {
                    Some(format!(
                        "BREAKTHROUGH SUCCEEDED ({old_stage} -> {}) on a transforming character! Immediately update ALL axes via manage_player_character(action='update_avatar'), modify_character_stat, and update_character_condition. Active Trajectory: {traj}",
                        track.format_stage()
                    ))
                } else if !stg.is_empty() {
                    Some(format!(
                        "BREAKTHROUGH SUCCEEDED ({old_stage} -> {}) on a character with appearance_stage '{stg}'. Check whether avatar measurements, qualitative traits, clothing fit, stats, and conditions need to advance now.",
                        track.format_stage()
                    ))
                } else {
                    None
                }
            })
        } else {
            None
        };
        let ev = self.db.append_event_causal(
            check.location_id.clone(),
            vec![cid.clone(), track.track_id.clone()],
            summary.clone(),
            EventPayload::ProgressionResolved {
                actor_id: cid.clone(),
                progression_kind: format!("breakthrough:{}", track.track_id),
                outcome: if succeeded { "breakthrough_success" } else { "breakthrough_failed" }.to_string(),
                notes: summary.clone(),
            },
            EventCausalMeta {
                actor_id: Some(cid.clone()),
                causal_event_ids: causal,
                visibility: "public".to_string(),
            },
        )?;
        let mut applied_rules = Vec::new();
        if succeeded {
            // Clear any stale bottleneck signal on this track
            let _ = self.update_signal(
                &format!("prog_bottleneck:{cid}:{}", track.track_id),
                "resolved",
            );
            if let Some(rule) = Self::lookup_stage_rule(&track, ladder.as_ref(), track.major_stage_index, track.sub_stage) {
                if let Ok(rep) = self.apply_stage_transition_rule(&cid, &track.name, &track.format_stage(), &rule, Some(ev.seq)) {
                    applied_rules.push(rep);
                }
            }
        }
        self.maybe_compile()?;
        let mut resp = serde_json::json!({
            "event_seq": ev.seq,
            "succeeded": succeeded,
            "summary": summary,
            "check": check,
            "track": track
        });
        if !applied_rules.is_empty() {
            resp["stage_rules_applied"] = serde_json::json!(applied_rules);
        } else if let Some(notice) = sync_notice {
            resp["appearance_sync_required"] = serde_json::json!(true);
            resp["appearance_sync_notice"] = serde_json::json!(notice);
        }
        Ok((resp, ev))
    }

    pub fn update_progression_track(
        &self,
        character_id: &str,
        track_id: &str,
        patch: &serde_json::Value,
        reason: &str,
    ) -> Result<(CharacterProgressionTrack, WorldEvent)> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let tid = slugify(track_id);
        let mut track = self
            .db
            .get_character_progression(&cid, &tid)?
            .ok_or_else(|| anyhow!("Progression track '{tid}' not enrolled on '{cid}'"))?;

        if let Some(s) = patch.get("status").and_then(|v| v.as_str()) {
            track.status = s.to_string();
        }
        if let Some(b) = patch.get("at_bottleneck").and_then(|v| v.as_bool()) {
            track.at_bottleneck = b;
        }
        if let Some(s) = patch.get("bottleneck_reason").and_then(|v| v.as_str()) {
            track.bottleneck_reason = s.to_string();
        }
        if let Some(m) = patch.get("check_modifier").and_then(|v| v.as_i64()) {
            track.check_modifier = m;
        }
        if let Some(s) = patch.get("notes").and_then(|v| v.as_str()) {
            track.notes = s.to_string();
        }
        if let Some(s) = patch.get("gm_notes").and_then(|v| v.as_str()) {
            track.gm_notes = s.to_string();
        }
        if let Some(s) = patch.get("manual_notes").and_then(|v| v.as_str()) {
            track.manual_notes = s.to_string();
        }
        if let Some(s) = patch.get("manual_title").and_then(|v| v.as_str()) {
            track.manual_title = s.to_string();
        }
        let mut manual_link_changed = false;
        if let Some(v) = patch.get("source_text_id") {
            manual_link_changed = true;
            if v.is_null() {
                track.source_text_id = None;
                if patch.get("manual_title").is_none() {
                    track.manual_title.clear();
                }
            } else if let Some(s) = v.as_str() {
                if s.trim().is_empty() {
                    track.source_text_id = None;
                    if patch.get("manual_title").is_none() {
                        track.manual_title.clear();
                    }
                } else {
                    track.source_text_id = Some(s.to_string());
                    if patch.get("manual_title").is_none() {
                        track.manual_title.clear();
                    }
                }
            }
        }
        if let Some(v) = patch.get("source_item_id") {
            manual_link_changed = true;
            if v.is_null() {
                track.source_item_id = None;
            } else if let Some(s) = v.as_str() {
                track.source_item_id = if s.trim().is_empty() {
                    None
                } else {
                    Some(s.to_string())
                };
            }
        }
        if manual_link_changed {
            self.resolve_manual_link(
                &mut track.source_text_id,
                &mut track.source_item_id,
                &mut track.manual_title,
            );
        }
        if let Some(arr) = patch.get("active_properties").and_then(|v| v.as_array()) {
            track.active_properties = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }
        if let Some(arr) = patch.get("add_properties").and_then(|v| v.as_array()) {
            for p in arr.iter().filter_map(|v| v.as_str()) {
                if !track.active_properties.iter().any(|x| x == p) {
                    track.active_properties.push(p.to_string());
                }
            }
        }
        if let Some(arr) = patch.get("halt_conditions").and_then(|v| v.as_array()) {
            track.halt_conditions = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }
        if let Some(sr) = patch.get("stage_rules") {
            if let Ok(parsed) = serde_json::from_value::<
                std::collections::BTreeMap<String, crate::routines::StageTransitionRule>,
            >(sr.clone())
            {
                if patch
                    .get("replace_stage_rules")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    track.stage_rules = parsed;
                } else {
                    for (k, v) in parsed {
                        track.stage_rules.insert(k, v);
                    }
                }
            }
        }
        if let Some(arr) = patch.get("check_tags").and_then(|v| v.as_array()) {
            track.check_tags = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }
        let mut stage_changed = false;
        if let Some(idx) = patch.get("major_stage_index").and_then(|v| v.as_u64()) {
            track.major_stage_index = idx as u32;
            stage_changed = true;
        }
        if let Some(sub) = patch.get("sub_stage").and_then(|v| v.as_u64()) {
            track.sub_stage = (sub as u32).max(1);
            stage_changed = true;
        }
        if stage_changed {
            if let Some(ref lid) = track.ladder_id {
                if let Some(ladder) = self.db.get_progression_ladder(lid)? {
                    if let Some(st) = ladder.stages.get(track.major_stage_index as usize) {
                        track.major_stage_name = st.name.clone();
                        track.sub_stage_max = st.sub_stages.max(1);
                        track.sub_stage = track.sub_stage.clamp(1, track.sub_stage_max);
                        track.sub_stage_label = st.format_sub_stage(track.sub_stage);
                        track.xp_to_next = st.xp_for_sub_stage(track.sub_stage);
                        if patch.get("check_modifier").is_none() {
                            track.check_modifier = st.passive_modifier;
                        }
                        if patch.get("halt_conditions").is_none() {
                            track.halt_conditions = st.halt_conditions.clone();
                        }
                    }
                }
            } else {
                track.sub_stage = track.sub_stage.clamp(1, track.sub_stage_max.max(1));
                track.sub_stage_label = if track.sub_stage_max > 1 {
                    format!("Layer {}/{}", track.sub_stage, track.sub_stage_max)
                } else {
                    String::new()
                };
            }
        }
        if let Some(s) = patch.get("major_stage_name").and_then(|v| v.as_str()) {
            track.major_stage_name = s.to_string();
        }
        if let Some(m) = patch.get("sub_stage_max").and_then(|v| v.as_u64()) {
            track.sub_stage_max = (m as u32).max(1);
            track.sub_stage = track.sub_stage.clamp(1, track.sub_stage_max);
        }
        if let Some(s) = patch.get("sub_stage_label").and_then(|v| v.as_str()) {
            track.sub_stage_label = s.to_string();
        }
        if let Some(v) = patch.get("current_xp").and_then(|v| v.as_u64()) {
            track.current_xp = v;
            if patch.get("at_bottleneck").is_none() && track.current_xp < track.xp_to_next {
                track.at_bottleneck = false;
                track.bottleneck_reason.clear();
            }
        }
        if let Some(v) = patch.get("cumulative_xp").and_then(|v| v.as_u64()) {
            track.cumulative_xp = v;
        }
        if let Some(v) = patch.get("xp_to_next").and_then(|v| v.as_u64()) {
            track.xp_to_next = v.max(1);
        }

        track.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_character_progression(&track)?;
        let summary = format!(
            "Updated progression track '{}' on '{}' [status={}, stage={}]{}",
            track.name,
            cid,
            track.status,
            track.format_stage(),
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );
        let ev = self.db.append_event_causal(
            None,
            vec![cid.clone(), track.track_id.clone()],
            summary.clone(),
            EventPayload::ProgressionResolved {
                actor_id: cid.clone(),
                progression_kind: format!("update:{}", track.track_id),
                outcome: track.status.clone(),
                notes: summary,
            },
            EventCausalMeta {
                actor_id: Some(cid),
                causal_event_ids: vec![],
                visibility: if patch.get("gm_notes").is_some() {
                    "gm_secret".to_string()
                } else {
                    "public".to_string()
                },
            },
        )?;
        self.maybe_compile()?;
        Ok((track, ev))
    }

    /// Correct, revert (negative delta with optional automatic stage demotion), set, or reset XP
    /// on a CharacterProgressionTrack, logging an audited event to the hash-chained ledger.
    #[allow(clippy::too_many_arguments)]
    pub fn adjust_progression_xp(
        &self,
        character_id: &str,
        track_id: &str,
        delta_xp: Option<i64>,
        set_current_xp: Option<u64>,
        set_cumulative_xp: Option<u64>,
        reset_stage_xp: bool,
        reset_track: bool,
        allow_demote: bool,
        reason: &str,
    ) -> Result<(serde_json::Value, WorldEvent)> {
        if let Some(d) = delta_xp {
            if d > 0 && set_current_xp.is_none() && set_cumulative_xp.is_none() && !reset_stage_xp && !reset_track {
                return self.advance_progression_xp(character_id, track_id, d as u64, reason);
            }
        }

        let cid = resolve_character_id(&self.db, character_id)?;
        let tid = slugify(track_id);
        let mut track = self
            .db
            .get_character_progression(&cid, &tid)?
            .ok_or_else(|| anyhow!("Progression track '{tid}' not enrolled on '{cid}'"))?;

        let ladder = match track.ladder_id.as_deref() {
            Some(lid) => self.db.get_progression_ladder(lid)?,
            None => None,
        };

        let old_stage_str = track.format_stage();
        let old_current_xp = track.current_xp;
        let old_cumulative_xp = track.cumulative_xp;
        let mut stages_demoted: u32 = 0;
        let mut major_demoted = false;
        let action_label: &str;

        if reset_track {
            action_label = "reset_track";
            track.major_stage_index = 0;
            track.sub_stage = 1;
            track.current_xp = 0;
            track.cumulative_xp = 0;
            track.at_bottleneck = false;
            track.bottleneck_reason.clear();
            track.status = "active".to_string();
            if let Some(ref lad) = ladder {
                if let Some(st0) = lad.stages.first() {
                    track.major_stage_name = st0.name.clone();
                    track.sub_stage_max = st0.sub_stages.max(1);
                    track.sub_stage_label = st0.format_sub_stage(1);
                    track.xp_to_next = st0.xp_for_sub_stage(1);
                    track.check_modifier = st0.passive_modifier;
                    track.active_properties = st0.properties.clone();
                    track.halt_conditions = st0.halt_conditions.clone();
                }
            } else {
                track.sub_stage_label = if track.sub_stage_max > 1 {
                    format!("Layer 1/{}", track.sub_stage_max)
                } else {
                    String::new()
                };
            }
        } else if reset_stage_xp {
            action_label = "reset_stage_xp";
            track.cumulative_xp = track.cumulative_xp.saturating_sub(track.current_xp);
            track.current_xp = 0;
            track.at_bottleneck = false;
            track.bottleneck_reason.clear();
            if track.status == "completed" {
                track.status = "active".to_string();
            }
        } else {
            action_label = "adjust_xp";
            if let Some(d) = delta_xp {
                if d < 0 {
                    let mut rem = d.unsigned_abs();
                    track.cumulative_xp = track.cumulative_xp.saturating_sub(rem);
                    if rem <= track.current_xp {
                        track.current_xp -= rem;
                        rem = 0;
                    } else if !allow_demote {
                        track.current_xp = 0;
                        rem = 0;
                    } else {
                        rem -= track.current_xp;
                        track.current_xp = 0;
                        while rem > 0 {
                            if track.sub_stage > 1 {
                                track.sub_stage -= 1;
                                let prev_need = ladder
                                    .as_ref()
                                    .and_then(|l| l.stages.get(track.major_stage_index as usize))
                                    .map(|s| {
                                        track.sub_stage_label = s.format_sub_stage(track.sub_stage);
                                        s.xp_for_sub_stage(track.sub_stage)
                                    })
                                    .unwrap_or_else(|| {
                                        track.sub_stage_label = if track.sub_stage_max > 1 {
                                            format!("Layer {}/{}", track.sub_stage, track.sub_stage_max)
                                        } else {
                                            String::new()
                                        };
                                        track.xp_to_next.max(1)
                                    });
                                track.xp_to_next = prev_need;
                                stages_demoted += 1;
                                if rem < prev_need {
                                    track.current_xp = prev_need - rem;
                                    rem = 0;
                                } else {
                                    rem -= prev_need;
                                }
                            } else if track.major_stage_index > 0 {
                                track.major_stage_index -= 1;
                                major_demoted = true;
                                stages_demoted += 1;
                                if let Some(ref lad) = ladder {
                                    if let Some(prev_st) = lad.stages.get(track.major_stage_index as usize) {
                                        track.major_stage_name = prev_st.name.clone();
                                        track.sub_stage_max = prev_st.sub_stages.max(1);
                                        track.sub_stage = track.sub_stage_max;
                                        track.sub_stage_label = prev_st.format_sub_stage(track.sub_stage);
                                        track.xp_to_next = prev_st.xp_for_sub_stage(track.sub_stage);
                                        track.check_modifier = prev_st.passive_modifier;
                                        track.halt_conditions = prev_st.halt_conditions.clone();
                                        // Recompute cumulative properties up to this major stage
                                        let mut props = Vec::new();
                                        for st_i in &lad.stages[..=(track.major_stage_index as usize)] {
                                            for p in &st_i.properties {
                                                if !props.contains(p) {
                                                    props.push(p.clone());
                                                }
                                            }
                                        }
                                        track.active_properties = props;
                                    }
                                } else {
                                    track.sub_stage = track.sub_stage_max.max(1);
                                }
                                let prev_need = track.xp_to_next.max(1);
                                if rem < prev_need {
                                    track.current_xp = prev_need - rem;
                                    rem = 0;
                                } else {
                                    rem -= prev_need;
                                }
                            } else {
                                track.current_xp = 0;
                                rem = 0;
                            }
                        }
                    }
                    let _ = rem;
                }
            }
            if let Some(v) = set_current_xp {
                track.current_xp = v;
            }
            if let Some(v) = set_cumulative_xp {
                track.cumulative_xp = v;
            }
            if track.current_xp < track.xp_to_next {
                track.at_bottleneck = false;
                track.bottleneck_reason.clear();
                if track.status == "completed" {
                    track.status = "active".to_string();
                }
            }
        }

        track.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_character_progression(&track)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let new_stage_str = track.format_stage();
        let summary = format!(
            "{} '{}' [{}] {} ({} XP, total {}) -> {} ({} XP, total {}){}",
            proj.baseline.name,
            track.name,
            action_label,
            old_stage_str,
            old_current_xp,
            old_cumulative_xp,
            new_stage_str,
            track.current_xp,
            track.cumulative_xp,
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );
        let ev = self.db.append_event_causal(
            proj.current.current_location.clone(),
            vec![cid.clone(), track.track_id.clone()],
            summary.clone(),
            EventPayload::ProgressionResolved {
                actor_id: cid.clone(),
                progression_kind: format!("{action_label}:{}", track.track_id),
                outcome: new_stage_str.clone(),
                notes: summary.clone(),
            },
            EventCausalMeta {
                actor_id: Some(cid),
                causal_event_ids: vec![],
                visibility: "public".to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok((
            serde_json::json!({
                "event_seq": ev.seq,
                "action": action_label,
                "summary": summary,
                "old_current_xp": old_current_xp,
                "old_cumulative_xp": old_cumulative_xp,
                "stages_demoted": stages_demoted,
                "major_demoted": major_demoted,
                "track": track
            }),
            ev,
        ))
    }

    /// Removes a progression track from a character (e.g., mistakenly enrolled track) with full audit logging.
    pub fn remove_progression_track(
        &self,
        character_id: &str,
        track_id: &str,
        reason: &str,
    ) -> Result<WorldEvent> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let tid = slugify(track_id);
        let existing = self
            .db
            .get_character_progression(&cid, &tid)?
            .ok_or_else(|| anyhow!("Progression track '{tid}' not enrolled on '{cid}'"))?;
        self.db.delete_character_progression(&cid, &tid)?;
        let summary = format!(
            "Removed progression track '{}' ({}) from '{}'{}",
            existing.name,
            existing.format_stage(),
            cid,
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );
        let ev = self.db.append_event_causal(
            None,
            vec![cid.clone(), tid.clone()],
            summary.clone(),
            EventPayload::ProgressionResolved {
                actor_id: cid.clone(),
                progression_kind: format!("remove:{tid}"),
                outcome: "removed".to_string(),
                notes: summary,
            },
            EventCausalMeta {
                actor_id: Some(cid),
                causal_event_ids: vec![],
                visibility: "public".to_string(),
            },
        )?;
        self.maybe_compile()?;
        Ok(ev)
    }

    // -------------------------------------------------------------------------
    // Dedicated Player Character Sheet (Avatar/Body/Measurements, Outfits,
    // Titles, Segregated Stat/Job/Proficiency XP, and Innate Traits)
    // -------------------------------------------------------------------------

    pub fn resolve_primary_pc_id(&self, character_id: Option<&str>) -> Result<String> {
        if let Some(raw) = character_id.map(str::trim).filter(|s| !s.is_empty()) {
            return resolve_character_id(&self.db, raw);
        }
        let all = self.db.list_character_baselines()?;
        if let Some(pc) = all.iter().find(|c| c.kind.eq_ignore_ascii_case("pc")) {
            return Ok(pc.id.clone());
        }
        if let Some(first) = all.first() {
            return Ok(first.id.clone());
        }
        bail!("No characters exist in this campaign yet. Create a PC first.")
    }

    pub fn get_player_character_sheet(
        &self,
        character_id: Option<&str>,
    ) -> Result<serde_json::Value> {
        let cid = self.resolve_primary_pc_id(character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let profile = self.get_character_profile(&cid)?;
        let visual_prompt = profile
            .avatar
            .synthesize_visual_prompt(&proj.baseline.name, &proj.baseline.archetype);
        let progression_tracks = self.db.list_character_progressions(&cid)?;
        let progression_ladders = self.db.list_progression_ladders()?;
        let rels = self.db.list_relations_for_entity(&cid)?;
        let injuries = self.db.list_injuries(Some(&cid), false)?;
        let currency_balances = self.db.list_currency_balances(&cid)?;
        let catalog_items: Vec<CatalogItem> = self
            .db
            .list_catalog_items()?
            .into_iter()
            .filter(|i| {
                i.current_holder_id
                    .as_deref()
                    .map(|h| h.eq_ignore_ascii_case(&cid))
                    .unwrap_or(false)
            })
            .collect();
        let manuals: Vec<serde_json::Value> = self
            .db
            .list_texts()?
            .into_iter()
            .map(|t| {
                serde_json::json!({
                    "id": t.id,
                    "title": t.title,
                    "medium": t.medium,
                    "subject": t.subject,
                    "read_status": t.read_status
                })
            })
            .collect();
        let images: Vec<serde_json::Value> = self
            .db
            .list_attachments()?
            .into_iter()
            .filter(|a| a.mime_type.starts_with("image/"))
            .map(|a| {
                serde_json::json!({
                    "id": a.id,
                    "title": a.title,
                    "mime_type": a.mime_type,
                    "relative_path": a.relative_path
                })
            })
            .collect();
        let all_baselines = self.db.list_character_baselines()?;
        let all_pcs: Vec<serde_json::Value> = all_baselines
            .iter()
            .filter(|b| b.kind.eq_ignore_ascii_case("pc"))
            .map(|b| {
                serde_json::json!({
                    "id": b.id,
                    "name": b.name,
                    "title": b.title,
                    "archetype": b.archetype,
                    "kind": b.kind
                })
            })
            .collect();
        let all_characters: Vec<serde_json::Value> = all_baselines
            .iter()
            .map(|b| {
                serde_json::json!({
                    "id": b.id,
                    "name": b.name,
                    "title": b.title,
                    "archetype": b.archetype,
                    "kind": b.kind
                })
            })
            .collect();

        let mut equipment_fit: std::collections::BTreeMap<String, crate::autonomy::GarmentFitReport> =
            std::collections::BTreeMap::new();
        for it in &proj.current.equipment {
            if let Some(rep) = crate::autonomy::evaluate_garment_fit(it, &profile.avatar) {
                equipment_fit.insert(it.name.clone(), rep);
            }
        }
        let apparel_summary = crate::autonomy::evaluate_equipped_apparel_fit(
            &proj.current.equipment,
            &profile.avatar,
        );

        let routines = self.db.list_routines(Some(&cid)).unwrap_or_default();
        let active_signals = self
            .list_active_signals(
                Some("gm"),
                proj.current.current_location.as_deref(),
                std::slice::from_ref(&cid),
                true,
            )
            .unwrap_or_default();

        Ok(serde_json::json!({
            "character_id": cid,
            "character": proj,
            "profile": profile,
            "equipment_fit": equipment_fit,
            "apparel_summary": apparel_summary,
            "visual_prompt": visual_prompt,
            "progression_tracks": progression_tracks,
            "progression_ladders": progression_ladders,
            "routines": routines,
            "active_signals": active_signals,
            "manuals": manuals,
            "images": images,
            "catalog_items": catalog_items,
            "currency_balances": currency_balances,
            "injuries": injuries,
            "relations": rels,
            "all_pcs": all_pcs,
            "all_characters": all_characters
        }))
    }

    pub fn manage_player_character_action(
        &self,
        character_id: Option<&str>,
        action: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let act = action.trim().to_lowercase();
        if act == "get_sheet" || act == "get" {
            return self.get_player_character_sheet(character_id);
        }

        let cid = self.resolve_primary_pc_id(character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let mut profile = self.get_character_profile(&cid)?;
        let reason = args
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let now = self.db.get_campaign_meta()?.clock.format_short();

        let (summary, fields_updated, extra_payload) = match act.as_str() {
            "update_avatar" => {
                let mut updated = Vec::new();
                macro_rules! set_av_str {
                    ($key:expr, $field:expr) => {
                        if let Some(s) = args.get($key).and_then(|v| v.as_str()) {
                            $field = s.to_string();
                            updated.push($key.to_string());
                        }
                    };
                }
                set_av_str!("avatar_summary", profile.avatar.avatar_summary);
                set_av_str!("appearance_stage", profile.avatar.appearance_stage);
                set_av_str!("height", profile.avatar.height);
                set_av_str!("weight", profile.avatar.weight);
                set_av_str!("baseline_height", profile.avatar.baseline_height);
                set_av_str!("baseline_weight", profile.avatar.baseline_weight);
                set_av_str!("chronological_age", profile.avatar.chronological_age);
                set_av_str!("apparent_age", profile.avatar.apparent_age);
                set_av_str!("body_type_or_build", profile.avatar.body_type_or_build);
                set_av_str!("silhouette", profile.avatar.silhouette);
                set_av_str!("posture_and_gait", profile.avatar.posture_and_gait);
                set_av_str!("proportions_summary", profile.avatar.proportions_summary);
                set_av_str!("clothing_fit", profile.avatar.clothing_fit);
                set_av_str!("perceived_change_note", profile.avatar.perceived_change_note);
                set_av_str!("transformation_trajectory", profile.avatar.transformation_trajectory);
                set_av_str!("face_shape_and_features", profile.avatar.face_shape_and_features);
                set_av_str!("eyes", profile.avatar.eyes);
                set_av_str!("skin_tone_and_texture", profile.avatar.skin_tone_and_texture);
                set_av_str!("hair", profile.avatar.hair);
                set_av_str!("facial_hair", profile.avatar.facial_hair);
                set_av_str!("voice_and_speech", profile.avatar.voice_and_speech);
                set_av_str!("scent_or_aura", profile.avatar.scent_or_aura);
                set_av_str!("visual_prompt_anchor", profile.avatar.visual_prompt_anchor);
                set_av_str!("active_outfit", profile.avatar.active_outfit);
                // Also allow syncing common identity/body fields cleanly in the same call
                set_av_str!("gender", profile.gender);
                set_av_str!("species", profile.species);
                set_av_str!("age_or_birthdate", profile.age_or_birthdate);
                set_av_str!("body_constitution", profile.body_constitution);
                set_av_str!("body_tempering", profile.body_tempering);

                if let Some(v) = args.get("lifespan_years") {
                    if v.is_null() {
                        profile.avatar.lifespan_years = None;
                    } else if let Some(n) = v.as_u64() {
                        profile.avatar.lifespan_years = if n == 0 { None } else { Some(n as u32) };
                    }
                    updated.push("lifespan_years".to_string());
                }
                if let Some(v) = args.get("portrait_attachment_id") {
                    if v.is_null() {
                        profile.avatar.portrait_attachment_id = None;
                    } else if let Some(s) = v.as_str() {
                        profile.avatar.portrait_attachment_id = if s.trim().is_empty() {
                            None
                        } else {
                            Some(s.trim().to_string())
                        };
                    }
                    updated.push("portrait_attachment_id".to_string());
                }
                if let Some(obj) = args.get("measurements").and_then(|v| v.as_object()) {
                    let replace = args
                        .get("replace_measurements")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if replace {
                        profile.avatar.measurements.clear();
                    }
                    for (mk, mv) in obj {
                        let norm_k = crate::autonomy::normalize_measurement_key(mk);
                        if norm_k.is_empty() {
                            continue;
                        }
                        let val_s = mv
                            .as_str()
                            .map(|s| s.trim().to_string())
                            .unwrap_or_else(|| mv.to_string());
                        if val_s.is_empty() {
                            profile.avatar.measurements.remove(mk.trim());
                            profile.avatar.measurements.remove(&norm_k);
                        } else {
                            if mk.trim() != norm_k {
                                profile.avatar.measurements.remove(mk.trim());
                            }
                            profile.avatar.measurements.insert(norm_k, val_s);
                        }
                    }
                    updated.push("measurements".to_string());
                }
                if let Some(obj) = args.get("baseline_measurements").and_then(|v| v.as_object()) {
                    let replace = args
                        .get("replace_baseline_measurements")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if replace {
                        profile.avatar.baseline_measurements.clear();
                    }
                    for (mk, mv) in obj {
                        let norm_k = crate::autonomy::normalize_measurement_key(mk);
                        if norm_k.is_empty() {
                            continue;
                        }
                        let val_s = mv
                            .as_str()
                            .map(|s| s.trim().to_string())
                            .unwrap_or_else(|| mv.to_string());
                        if val_s.is_empty() {
                            profile.avatar.baseline_measurements.remove(mk.trim());
                            profile.avatar.baseline_measurements.remove(&norm_k);
                        } else {
                            if mk.trim() != norm_k {
                                profile.avatar.baseline_measurements.remove(mk.trim());
                            }
                            profile.avatar.baseline_measurements.insert(norm_k, val_s);
                        }
                    }
                    updated.push("baseline_measurements".to_string());
                }
                if let Some(sm) = args.get("set_measurement").and_then(|v| v.as_object()) {
                    if let (Some(k), Some(v)) = (
                        sm.get("key").and_then(|x| x.as_str()),
                        sm.get("value").and_then(|x| x.as_str()),
                    ) {
                        let norm_k = crate::autonomy::normalize_measurement_key(k);
                        if !norm_k.is_empty() {
                            if v.trim().is_empty() {
                                profile.avatar.measurements.remove(k.trim());
                                profile.avatar.measurements.remove(&norm_k);
                            } else {
                                if k.trim() != norm_k {
                                    profile.avatar.measurements.remove(k.trim());
                                }
                                profile
                                    .avatar
                                    .measurements
                                    .insert(norm_k, v.trim().to_string());
                            }
                            updated.push("measurements".to_string());
                        }
                    }
                }
                if let Some(sm) = args
                    .get("set_baseline_measurement")
                    .and_then(|v| v.as_object())
                {
                    if let (Some(k), Some(v)) = (
                        sm.get("key").and_then(|x| x.as_str()),
                        sm.get("value").and_then(|x| x.as_str()),
                    ) {
                        let norm_k = crate::autonomy::normalize_measurement_key(k);
                        if !norm_k.is_empty() {
                            if v.trim().is_empty() {
                                profile.avatar.baseline_measurements.remove(k.trim());
                                profile.avatar.baseline_measurements.remove(&norm_k);
                            } else {
                                if k.trim() != norm_k {
                                    profile.avatar.baseline_measurements.remove(k.trim());
                                }
                                profile
                                    .avatar
                                    .baseline_measurements
                                    .insert(norm_k, v.trim().to_string());
                            }
                            updated.push("baseline_measurements".to_string());
                        }
                    }
                }
                if let Some(rm) = args.get("remove_measurement").and_then(|v| v.as_str()) {
                    let norm_k = crate::autonomy::normalize_measurement_key(rm);
                    profile.avatar.measurements.remove(rm.trim());
                    profile.avatar.measurements.remove(&norm_k);
                    updated.push("measurements".to_string());
                }
                if let Some(rm) = args
                    .get("remove_baseline_measurement")
                    .and_then(|v| v.as_str())
                {
                    let norm_k = crate::autonomy::normalize_measurement_key(rm);
                    profile.avatar.baseline_measurements.remove(rm.trim());
                    profile.avatar.baseline_measurements.remove(&norm_k);
                    updated.push("baseline_measurements".to_string());
                }
                if args
                    .get("clear_checkpoints")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    profile.avatar.transformation_log.clear();
                    updated.push("transformation_log".to_string());
                }
                if let Some(arr) = args.get("transformation_log").and_then(|v| v.as_array()) {
                    let mut parsed = Vec::new();
                    for item in arr {
                        if let Ok(mut cp) =
                            serde_json::from_value::<crate::autonomy::AppearanceCheckpoint>(
                                item.clone(),
                            )
                        {
                            let mut norm_m = std::collections::BTreeMap::new();
                            for (k, v) in cp.measurements {
                                let nk = crate::autonomy::normalize_measurement_key(&k);
                                if !nk.is_empty() && !v.trim().is_empty() {
                                    norm_m.insert(nk, v.trim().to_string());
                                }
                            }
                            cp.measurements = norm_m;
                            parsed.push(cp);
                        }
                    }
                    profile.avatar.transformation_log = parsed;
                    updated.push("transformation_log".to_string());
                }
                if let Some(cp_val) = args.get("record_checkpoint").and_then(|v| v.as_object()) {
                    let stage = cp_val
                        .get("stage")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            if !profile.avatar.appearance_stage.is_empty() {
                                profile.avatar.appearance_stage.clone()
                            } else {
                                "Checkpoint".to_string()
                            }
                        });
                    let world_time = cp_val
                        .get("world_time")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| now.clone());
                    let summary = cp_val
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .unwrap_or("")
                        .to_string();
                    let height = cp_val
                        .get("height")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| profile.avatar.height.clone());
                    let weight = cp_val
                        .get("weight")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| profile.avatar.weight.clone());
                    let meas = if let Some(obj) =
                        cp_val.get("measurements").and_then(|v| v.as_object())
                    {
                        obj.iter()
                            .filter_map(|(k, v)| {
                                let nk = crate::autonomy::normalize_measurement_key(k);
                                let vs = v.as_str().map(str::trim).unwrap_or("").to_string();
                                if nk.is_empty() || vs.is_empty() {
                                    None
                                } else {
                                    Some((nk, vs))
                                }
                            })
                            .collect()
                    } else {
                        profile.avatar.measurements.clone()
                    };
                    let cp = crate::autonomy::AppearanceCheckpoint {
                        stage: stage.clone(),
                        world_time,
                        summary,
                        height,
                        weight,
                        measurements: meas,
                    };
                    if let Some(ex) = profile
                        .avatar
                        .transformation_log
                        .iter_mut()
                        .find(|x| x.stage.eq_ignore_ascii_case(&stage))
                    {
                        *ex = cp;
                    } else {
                        profile.avatar.transformation_log.push(cp);
                    }
                    updated.push("transformation_log".to_string());
                }
                // Keep legacy height_and_build synced for backward compatibility
                if !profile.avatar.height.is_empty() || !profile.avatar.body_type_or_build.is_empty() {
                    profile.height_and_build = [
                        profile.avatar.height.as_str(),
                        profile.avatar.weight.as_str(),
                        profile.avatar.body_type_or_build.as_str(),
                    ]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" • ");
                }

                let vp = profile
                    .avatar
                    .synthesize_visual_prompt(&proj.baseline.name, &proj.baseline.archetype);
                (
                    format!(
                        "Updated avatar & physical profile for {} ({})",
                        proj.baseline.name,
                        if updated.is_empty() {
                            "avatar".to_string()
                        } else {
                            updated.join(", ")
                        }
                    ),
                    updated,
                    serde_json::json!({ "avatar": profile.avatar, "visual_prompt": vp }),
                )
            }

            "upsert_marker" => {
                let location = args
                    .get("location")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let description = args
                    .get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if location.is_empty() && description.is_empty() {
                    bail!("upsert_marker requires 'location' or 'description'");
                }
                let kind = args
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("scar")
                    .trim()
                    .to_string();
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(slugify)
                    .unwrap_or_else(|| slugify(&format!("{kind}-{location}-{description}")));
                let marker = PhysicalMarker {
                    id: id.clone(),
                    kind: kind.clone(),
                    location: location.clone(),
                    description: description.clone(),
                    origin_or_story: args
                        .get("origin_or_story")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    visible_when_clothed: args
                        .get("visible_when_clothed")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true),
                };
                if let Some(ex) = profile
                    .avatar
                    .permanent_markers
                    .iter_mut()
                    .find(|m| m.id == id)
                {
                    *ex = marker.clone();
                } else {
                    profile.avatar.permanent_markers.push(marker.clone());
                }
                (
                    format!(
                        "Recorded permanent marker ({kind} on {location}) for {}",
                        proj.baseline.name
                    ),
                    vec!["avatar.permanent_markers".to_string()],
                    serde_json::json!({ "marker": marker, "permanent_markers": profile.avatar.permanent_markers }),
                )
            }

            "remove_marker" => {
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("remove_marker requires 'id'"))?;
                let slug = slugify(id);
                let before = profile.avatar.permanent_markers.len();
                profile
                    .avatar
                    .permanent_markers
                    .retain(|m| m.id != id && m.id != slug);
                if profile.avatar.permanent_markers.len() == before {
                    bail!("Permanent marker '{id}' not found on '{cid}'");
                }
                (
                    format!("Removed permanent marker '{id}' from {}", proj.baseline.name),
                    vec!["avatar.permanent_markers".to_string()],
                    serde_json::json!({ "removed": true, "permanent_markers": profile.avatar.permanent_markers }),
                )
            }

            "upsert_outfit" => {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| anyhow!("upsert_outfit requires 'name'"))?
                    .to_string();
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(slugify)
                    .unwrap_or_else(|| slugify(&name));
                let existing = profile
                    .avatar
                    .outfits
                    .iter()
                    .find(|o| o.id == id || o.name.eq_ignore_ascii_case(&name))
                    .cloned();
                let outfit = CharacterOutfit {
                    id: id.clone(),
                    name: name.clone(),
                    occasion: args
                        .get("occasion")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|o| o.occasion.clone()))
                        .unwrap_or_default(),
                    description: args
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|o| o.description.clone()))
                        .unwrap_or_default(),
                    headwear: args
                        .get("headwear")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|o| o.headwear.clone()))
                        .unwrap_or_default(),
                    outer_layer: args
                        .get("outer_layer")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|o| o.outer_layer.clone()))
                        .unwrap_or_default(),
                    top_layer: args
                        .get("top_layer")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|o| o.top_layer.clone()))
                        .unwrap_or_default(),
                    bottoms: args
                        .get("bottoms")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|o| o.bottoms.clone()))
                        .unwrap_or_default(),
                    footwear: args
                        .get("footwear")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|o| o.footwear.clone()))
                        .unwrap_or_default(),
                    accessories: args
                        .get("accessories")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|o| o.accessories.clone()))
                        .unwrap_or_default(),
                    color_palette: args
                        .get("color_palette")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|o| o.color_palette.clone()))
                        .unwrap_or_default(),
                    notes: args
                        .get("notes")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|o| o.notes.clone()))
                        .unwrap_or_default(),
                };
                if let Some(ex) = profile
                    .avatar
                    .outfits
                    .iter_mut()
                    .find(|o| o.id == id || o.name.eq_ignore_ascii_case(&name))
                {
                    *ex = outfit.clone();
                } else {
                    profile.avatar.outfits.push(outfit.clone());
                }
                if args.get("set_active").and_then(|v| v.as_bool()).unwrap_or(false)
                    || profile.avatar.active_outfit.is_empty()
                {
                    profile.avatar.active_outfit = outfit.id.clone();
                }
                (
                    format!("Saved outfit '{}' for {}", outfit.name, proj.baseline.name),
                    vec!["avatar.outfits".to_string()],
                    serde_json::json!({ "outfit": outfit, "active_outfit": profile.avatar.active_outfit, "outfits": profile.avatar.outfits }),
                )
            }

            "set_active_outfit" => {
                let target = args
                    .get("id")
                    .or_else(|| args.get("name"))
                    .or_else(|| args.get("active_outfit"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("set_active_outfit requires 'id' or 'name'"))?
                    .trim();
                let resolved = profile
                    .avatar
                    .outfits
                    .iter()
                    .find(|o| o.id.eq_ignore_ascii_case(target) || o.name.eq_ignore_ascii_case(target))
                    .map(|o| o.id.clone())
                    .unwrap_or_else(|| target.to_string());
                profile.avatar.active_outfit = resolved.clone();
                (
                    format!("Changed active outfit for {} to '{}'", proj.baseline.name, resolved),
                    vec!["avatar.active_outfit".to_string()],
                    serde_json::json!({ "active_outfit": resolved }),
                )
            }

            "remove_outfit" => {
                let id = args
                    .get("id")
                    .or_else(|| args.get("name"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("remove_outfit requires 'id' or 'name'"))?;
                let slug = slugify(id);
                let before = profile.avatar.outfits.len();
                profile
                    .avatar
                    .outfits
                    .retain(|o| o.id != id && o.id != slug && !o.name.eq_ignore_ascii_case(id));
                if profile.avatar.outfits.len() == before {
                    bail!("Outfit '{id}' not found on '{cid}'");
                }
                if profile.avatar.active_outfit == id || profile.avatar.active_outfit == slug {
                    profile.avatar.active_outfit = profile
                        .avatar
                        .outfits
                        .first()
                        .map(|o| o.id.clone())
                        .unwrap_or_default();
                }
                (
                    format!("Removed outfit '{id}' from {}", proj.baseline.name),
                    vec!["avatar.outfits".to_string()],
                    serde_json::json!({ "removed": true, "outfits": profile.avatar.outfits }),
                )
            }

            "upsert_title" => {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| anyhow!("upsert_title requires 'name'"))?
                    .to_string();
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(slugify)
                    .unwrap_or_else(|| slugify(&name));
                let existing = profile
                    .titles
                    .iter()
                    .find(|t| t.id == id || t.name.eq_ignore_ascii_case(&name))
                    .cloned();
                let stat_modifiers = if let Some(obj) =
                    args.get("stat_modifiers").and_then(|v| v.as_object())
                {
                    let mut m = std::collections::BTreeMap::new();
                    for (k, v) in obj {
                        if let Some(n) = v.as_i64() {
                            m.insert(k.clone(), n);
                        }
                    }
                    m
                } else {
                    existing
                        .as_ref()
                        .map(|t| t.stat_modifiers.clone())
                        .unwrap_or_default()
                };
                let title = CharacterTitle {
                    id: id.clone(),
                    name: name.clone(),
                    kind: args
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|t| t.kind.clone()))
                        .unwrap_or_else(|| "title".to_string()),
                    equipped: args
                        .get("equipped")
                        .and_then(|v| v.as_bool())
                        .or_else(|| existing.as_ref().map(|t| t.equipped))
                        .unwrap_or(true),
                    source: args
                        .get("source")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|t| t.source.clone()))
                        .unwrap_or_default(),
                    mechanical_effect: args
                        .get("mechanical_effect")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|t| t.mechanical_effect.clone()))
                        .unwrap_or_default(),
                    stat_modifiers,
                    visibility: args
                        .get("visibility")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|t| t.visibility.clone()))
                        .unwrap_or_else(|| "public".to_string()),
                    notes: args
                        .get("notes")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|t| t.notes.clone()))
                        .unwrap_or_default(),
                };
                if let Some(ex) = profile
                    .titles
                    .iter_mut()
                    .find(|t| t.id == id || t.name.eq_ignore_ascii_case(&name))
                {
                    *ex = title.clone();
                } else {
                    profile.titles.push(title.clone());
                }
                (
                    format!(
                        "Saved title '{}' [{}] on {}",
                        title.name,
                        if title.equipped { "equipped" } else { "stored" },
                        proj.baseline.name
                    ),
                    vec!["titles".to_string()],
                    serde_json::json!({ "title": title, "titles": profile.titles }),
                )
            }

            "toggle_title" => {
                let id = args
                    .get("id")
                    .or_else(|| args.get("name"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("toggle_title requires 'id' or 'name'"))?;
                let slug = slugify(id);
                let t = profile
                    .titles
                    .iter_mut()
                    .find(|t| t.id == id || t.id == slug || t.name.eq_ignore_ascii_case(id))
                    .ok_or_else(|| anyhow!("Title '{id}' not found on '{cid}'"))?;
                t.equipped = args
                    .get("equipped")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(!t.equipped);
                let out_t = t.clone();
                (
                    format!(
                        "{} title '{}' on {}",
                        if out_t.equipped { "Equipped" } else { "Unequipped" },
                        out_t.name,
                        proj.baseline.name
                    ),
                    vec!["titles".to_string()],
                    serde_json::json!({ "title": out_t, "titles": profile.titles }),
                )
            }

            "remove_title" => {
                let id = args
                    .get("id")
                    .or_else(|| args.get("name"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("remove_title requires 'id' or 'name'"))?;
                let slug = slugify(id);
                let before = profile.titles.len();
                profile
                    .titles
                    .retain(|t| t.id != id && t.id != slug && !t.name.eq_ignore_ascii_case(id));
                if profile.titles.len() == before {
                    bail!("Title '{id}' not found on '{cid}'");
                }
                (
                    format!("Removed title '{id}' from {}", proj.baseline.name),
                    vec!["titles".to_string()],
                    serde_json::json!({ "removed": true, "titles": profile.titles }),
                )
            }

            "adjust_stat_xp" => {
                // Optional free stat point pool adjustments
                if let Some(sp) = args.get("set_unspent_stat_points").and_then(|v| v.as_i64()) {
                    profile.unspent_stat_points = sp.max(0);
                }
                if let Some(d) = args.get("unspent_stat_points_delta").and_then(|v| v.as_i64()) {
                    profile.unspent_stat_points = (profile.unspent_stat_points + d).max(0);
                }

                let stat_opt = args
                    .get("stat")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());

                if let Some(stat_name) = stat_opt {
                    let spend_point = args
                        .get("spend_stat_point")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if spend_point {
                        if profile.unspent_stat_points <= 0 {
                            bail!("No unspent stat points available to spend on '{stat_name}'");
                        }
                        profile.unspent_stat_points -= 1;
                        self.modify_stat(
                            &cid,
                            &stat_name,
                            Some(1),
                            None,
                            if reason.is_empty() {
                                "Spent 1 unspent stat point"
                            } else {
                                &reason
                            },
                        )?;
                        let summary = format!(
                            "{} spent 1 free stat point -> +1 {} (remaining free points: {})",
                            proj.baseline.name, stat_name, profile.unspent_stat_points
                        );
                        (
                            summary,
                            vec!["unspent_stat_points".to_string(), format!("stat:{stat_name}")],
                            serde_json::json!({
                                "unspent_stat_points": profile.unspent_stat_points,
                                "stat_training": profile.stat_training
                            }),
                        )
                    } else {
                        let entry = profile
                            .stat_training
                            .entry(stat_name.clone())
                            .or_insert_with(|| StatTrainingEntry {
                                stat: stat_name.clone(),
                                current_xp: 0,
                                xp_to_next: 100,
                                cumulative_xp: 0,
                                points_gained_from_training: 0,
                                training_notes: String::new(),
                            });
                        if let Some(n) = args.get("xp_to_next").and_then(|v| v.as_u64()) {
                            entry.xp_to_next = n.max(1);
                        }
                        if let Some(tn) = args.get("training_notes").and_then(|v| v.as_str()) {
                            entry.training_notes = tn.to_string();
                        }

                        let old_xp = entry.current_xp;
                        let mut points_promoted: i64 = 0;
                        let auto_promote = args
                            .get("auto_promote")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(true);

                        if args.get("reset_xp").and_then(|v| v.as_bool()).unwrap_or(false) {
                            entry.cumulative_xp = entry.cumulative_xp.saturating_sub(entry.current_xp);
                            entry.current_xp = 0;
                        } else if let Some(v) = args.get("set_xp").and_then(|v| v.as_u64()) {
                            entry.current_xp = v;
                            if let Some(c) = args.get("set_cumulative_xp").and_then(|v| v.as_u64()) {
                                entry.cumulative_xp = c;
                            }
                        } else if let Some(d) = args
                            .get("delta_xp")
                            .or_else(|| args.get("xp"))
                            .and_then(|v| v.as_i64())
                        {
                            if d >= 0 {
                                let add = d as u64;
                                entry.current_xp = entry.current_xp.saturating_add(add);
                                entry.cumulative_xp = entry.cumulative_xp.saturating_add(add);
                                if auto_promote {
                                    while entry.current_xp >= entry.xp_to_next.max(1) {
                                        let need = entry.xp_to_next.max(1);
                                        entry.current_xp -= need;
                                        entry.points_gained_from_training += 1;
                                        points_promoted += 1;
                                        entry.xp_to_next = (need as f64 * 1.25).round() as u64;
                                    }
                                }
                            } else {
                                let sub = d.unsigned_abs();
                                entry.current_xp = entry.current_xp.saturating_sub(sub);
                                entry.cumulative_xp = entry.cumulative_xp.saturating_sub(sub);
                            }
                        }

                        let entry_snapshot = entry.clone();
                        if points_promoted > 0 {
                            self.modify_stat(
                                &cid,
                                &stat_name,
                                Some(points_promoted),
                                None,
                                if reason.is_empty() {
                                    "Stat Training XP threshold reached"
                                } else {
                                    &reason
                                },
                            )?;
                        }
                        let summary = format!(
                            "{} Stat Training XP [{}]: {} -> {} / {} XP (total {}){}",
                            proj.baseline.name,
                            stat_name,
                            old_xp,
                            entry_snapshot.current_xp,
                            entry_snapshot.xp_to_next,
                            entry_snapshot.cumulative_xp,
                            if points_promoted > 0 {
                                format!(" (+{points_promoted} {stat_name}!)")
                            } else {
                                String::new()
                            }
                        );
                        (
                            summary,
                            vec![format!("stat_training.{stat_name}")],
                            serde_json::json!({
                                "entry": entry_snapshot,
                                "points_promoted": points_promoted,
                                "unspent_stat_points": profile.unspent_stat_points,
                                "stat_training": profile.stat_training
                            }),
                        )
                    }
                } else {
                    (
                        format!(
                            "Updated unspent stat points for {} -> {}",
                            proj.baseline.name, profile.unspent_stat_points
                        ),
                        vec!["unspent_stat_points".to_string()],
                        serde_json::json!({ "unspent_stat_points": profile.unspent_stat_points }),
                    )
                }
            }

            "upsert_job" => {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| anyhow!("upsert_job requires 'name'"))?
                    .to_string();
                let job_id = args
                    .get("job_id")
                    .or_else(|| args.get("id"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(slugify)
                    .unwrap_or_else(|| slugify(&name));
                let existing = profile
                    .jobs
                    .iter()
                    .find(|j| j.job_id == job_id || j.name.eq_ignore_ascii_case(&name))
                    .cloned();
                let job = CharacterJob {
                    job_id: job_id.clone(),
                    name: name.clone(),
                    role: args
                        .get("role")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|j| j.role.clone()))
                        .unwrap_or_else(|| "primary".to_string()),
                    tier_or_rank: args
                        .get("tier_or_rank")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|j| j.tier_or_rank.clone()))
                        .unwrap_or_default(),
                    level: args
                        .get("level")
                        .and_then(|v| v.as_u64())
                        .map(|n| (n as u32).max(1))
                        .or_else(|| existing.as_ref().map(|j| j.level))
                        .unwrap_or(1),
                    current_xp: args
                        .get("current_xp")
                        .and_then(|v| v.as_u64())
                        .or_else(|| existing.as_ref().map(|j| j.current_xp))
                        .unwrap_or(0),
                    xp_to_next: args
                        .get("xp_to_next")
                        .and_then(|v| v.as_u64())
                        .map(|n| n.max(1))
                        .or_else(|| existing.as_ref().map(|j| j.xp_to_next))
                        .unwrap_or(250),
                    cumulative_xp: args
                        .get("cumulative_xp")
                        .and_then(|v| v.as_u64())
                        .or_else(|| existing.as_ref().map(|j| j.cumulative_xp))
                        .unwrap_or(0),
                    active: args
                        .get("active")
                        .and_then(|v| v.as_bool())
                        .or_else(|| existing.as_ref().map(|j| j.active))
                        .unwrap_or(true),
                    governing_stats: args
                        .get("governing_stats")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .or_else(|| existing.as_ref().map(|j| j.governing_stats.clone()))
                        .unwrap_or_default(),
                    perks_and_features: args
                        .get("perks_and_features")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .or_else(|| existing.as_ref().map(|j| j.perks_and_features.clone()))
                        .unwrap_or_default(),
                    notes: args
                        .get("notes")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|j| j.notes.clone()))
                        .unwrap_or_default(),
                };
                if let Some(ex) = profile
                    .jobs
                    .iter_mut()
                    .find(|j| j.job_id == job_id || j.name.eq_ignore_ascii_case(&name))
                {
                    *ex = job.clone();
                } else {
                    profile.jobs.push(job.clone());
                }
                (
                    format!(
                        "Saved job '{}' [{} Lv {}] on {}",
                        job.name, job.role, job.level, proj.baseline.name
                    ),
                    vec!["jobs".to_string()],
                    serde_json::json!({ "job": job, "jobs": profile.jobs }),
                )
            }

            "adjust_job_xp" => {
                let target = args
                    .get("job_id")
                    .or_else(|| args.get("id"))
                    .or_else(|| args.get("name"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("adjust_job_xp requires 'job_id' (or 'name')"))?;
                let slug = slugify(target);
                let job = profile
                    .jobs
                    .iter_mut()
                    .find(|j| j.job_id == target || j.job_id == slug || j.name.eq_ignore_ascii_case(target))
                    .ok_or_else(|| anyhow!("Job '{target}' not found on '{cid}'"))?;

                let old_lvl = job.level;
                let old_xp = job.current_xp;
                let allow_demote = args
                    .get("allow_demote")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let mut levels_changed: i64 = 0;

                if args.get("reset_job").and_then(|v| v.as_bool()).unwrap_or(false) {
                    job.level = 1;
                    job.current_xp = 0;
                    job.cumulative_xp = 0;
                    job.xp_to_next = 250;
                    levels_changed = 1 - (old_lvl as i64);
                } else if args
                    .get("reset_stage_xp")
                    .or_else(|| args.get("reset_xp"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    job.cumulative_xp = job.cumulative_xp.saturating_sub(job.current_xp);
                    job.current_xp = 0;
                } else {
                    if let Some(lvl) = args.get("set_level").and_then(|v| v.as_u64()) {
                        job.level = (lvl as u32).max(1);
                        levels_changed = (job.level as i64) - (old_lvl as i64);
                    }
                    if let Some(v) = args.get("set_xp").and_then(|v| v.as_u64()) {
                        job.current_xp = v;
                    }
                    if let Some(c) = args.get("set_cumulative_xp").and_then(|v| v.as_u64()) {
                        job.cumulative_xp = c;
                    }
                    if let Some(d) = args
                        .get("delta_xp")
                        .or_else(|| args.get("xp"))
                        .and_then(|v| v.as_i64())
                    {
                        if d >= 0 {
                            let add = d as u64;
                            job.current_xp = job.current_xp.saturating_add(add);
                            job.cumulative_xp = job.cumulative_xp.saturating_add(add);
                            while job.current_xp >= job.xp_to_next.max(1) {
                                let need = job.xp_to_next.max(1);
                                job.current_xp -= need;
                                job.level += 1;
                                levels_changed += 1;
                                job.xp_to_next = ((need as f64) * 1.35).round() as u64;
                            }
                        } else {
                            let mut rem = d.unsigned_abs();
                            job.cumulative_xp = job.cumulative_xp.saturating_sub(rem);
                            if rem <= job.current_xp {
                                job.current_xp -= rem;
                            } else if !allow_demote {
                                job.current_xp = 0;
                            } else {
                                rem -= job.current_xp;
                                job.current_xp = 0;
                                while rem > 0 && job.level > 1 {
                                    job.level -= 1;
                                    levels_changed -= 1;
                                    let prev_need =
                                        ((job.xp_to_next as f64) / 1.35).round().max(100.0) as u64;
                                    job.xp_to_next = prev_need;
                                    if rem < prev_need {
                                        job.current_xp = prev_need - rem;
                                        rem = 0;
                                    } else {
                                        rem -= prev_need;
                                    }
                                }
                            }
                        }
                    }
                }
                let out_job = job.clone();
                (
                    format!(
                        "{} Job XP [{}]: Lv {} ({} XP) -> Lv {} ({}/{} XP, total {})",
                        proj.baseline.name,
                        out_job.name,
                        old_lvl,
                        old_xp,
                        out_job.level,
                        out_job.current_xp,
                        out_job.xp_to_next,
                        out_job.cumulative_xp
                    ),
                    vec!["jobs".to_string()],
                    serde_json::json!({ "job": out_job, "levels_changed": levels_changed, "jobs": profile.jobs }),
                )
            }

            "remove_job" => {
                let id = args
                    .get("job_id")
                    .or_else(|| args.get("id"))
                    .or_else(|| args.get("name"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("remove_job requires 'job_id' (or 'name')"))?;
                let slug = slugify(id);
                let before = profile.jobs.len();
                profile
                    .jobs
                    .retain(|j| j.job_id != id && j.job_id != slug && !j.name.eq_ignore_ascii_case(id));
                if profile.jobs.len() == before {
                    bail!("Job '{id}' not found on '{cid}'");
                }
                (
                    format!("Removed job '{id}' from {}", proj.baseline.name),
                    vec!["jobs".to_string()],
                    serde_json::json!({ "removed": true, "jobs": profile.jobs }),
                )
            }

            "upsert_proficiency" => {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| anyhow!("upsert_proficiency requires 'name'"))?
                    .to_string();
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(slugify)
                    .unwrap_or_else(|| slugify(&name));
                let existing = profile
                    .proficiencies
                    .iter()
                    .find(|p| p.id == id || p.name.eq_ignore_ascii_case(&name))
                    .cloned();
                let prof = CharacterProficiency {
                    id: id.clone(),
                    name: name.clone(),
                    category: args
                        .get("category")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|p| p.category.clone()))
                        .unwrap_or_else(|| "tool".to_string()),
                    rank: args
                        .get("rank")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|p| p.rank.clone()))
                        .unwrap_or_else(|| "Proficient".to_string()),
                    xp: args
                        .get("xp")
                        .and_then(|v| v.as_u64())
                        .or_else(|| existing.as_ref().map(|p| p.xp))
                        .unwrap_or(0),
                    xp_to_next: args
                        .get("xp_to_next")
                        .and_then(|v| v.as_u64())
                        .map(|n| n.max(1))
                        .or_else(|| existing.as_ref().map(|p| p.xp_to_next))
                        .unwrap_or(100),
                    bonus: args
                        .get("bonus")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|p| p.bonus.clone()))
                        .unwrap_or_default(),
                    notes: args
                        .get("notes")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|p| p.notes.clone()))
                        .unwrap_or_default(),
                };
                if let Some(ex) = profile
                    .proficiencies
                    .iter_mut()
                    .find(|p| p.id == id || p.name.eq_ignore_ascii_case(&name))
                {
                    *ex = prof.clone();
                } else {
                    profile.proficiencies.push(prof.clone());
                }
                (
                    format!(
                        "Saved proficiency '{}' [{} • {}] on {}",
                        prof.name, prof.category, prof.rank, proj.baseline.name
                    ),
                    vec!["proficiencies".to_string()],
                    serde_json::json!({ "proficiency": prof, "proficiencies": profile.proficiencies }),
                )
            }

            "adjust_proficiency_xp" => {
                let target = args
                    .get("id")
                    .or_else(|| args.get("name"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("adjust_proficiency_xp requires 'id' or 'name'"))?;
                let slug = slugify(target);
                let prof = profile
                    .proficiencies
                    .iter_mut()
                    .find(|p| p.id == target || p.id == slug || p.name.eq_ignore_ascii_case(target))
                    .ok_or_else(|| anyhow!("Proficiency '{target}' not found on '{cid}'"))?;
                let old_xp = prof.xp;
                let old_rank = prof.rank.clone();
                let recalc = args
                    .get("recalc_rank")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                if args.get("reset_xp").and_then(|v| v.as_bool()).unwrap_or(false) {
                    prof.xp = 0;
                } else if let Some(v) = args.get("set_xp").and_then(|v| v.as_u64()) {
                    prof.xp = v;
                } else if let Some(d) = args
                    .get("delta_xp")
                    .or_else(|| args.get("xp"))
                    .and_then(|v| v.as_i64())
                {
                    prof.xp = if d >= 0 {
                        prof.xp.saturating_add(d as u64)
                    } else {
                        prof.xp.saturating_sub(d.unsigned_abs())
                    };
                }
                if recalc {
                    let (new_rank, next_xp, bonus) = match prof.xp {
                        0..=24 => ("Untrained", 25, "+0"),
                        25..=99 => ("Familiar", 100, "+1"),
                        100..=249 => ("Proficient", 250, "+2"),
                        250..=499 => ("Expertise", 500, "+4"),
                        500..=999 => ("Master", 1000, "+6"),
                        _ => ("Grandmaster", 1000, "+8"),
                    };
                    prof.rank = new_rank.to_string();
                    prof.xp_to_next = next_xp;
                    prof.bonus = bonus.to_string();
                }
                let out_p = prof.clone();
                (
                    format!(
                        "{} Proficiency XP [{}]: {} ({}) -> {} ({})",
                        proj.baseline.name, out_p.name, old_xp, old_rank, out_p.xp, out_p.rank
                    ),
                    vec!["proficiencies".to_string()],
                    serde_json::json!({ "proficiency": out_p, "proficiencies": profile.proficiencies }),
                )
            }

            "remove_proficiency" => {
                let id = args
                    .get("id")
                    .or_else(|| args.get("name"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("remove_proficiency requires 'id' or 'name'"))?;
                let slug = slugify(id);
                let before = profile.proficiencies.len();
                profile
                    .proficiencies
                    .retain(|p| p.id != id && p.id != slug && !p.name.eq_ignore_ascii_case(id));
                if profile.proficiencies.len() == before {
                    bail!("Proficiency '{id}' not found on '{cid}'");
                }
                (
                    format!("Removed proficiency '{id}' from {}", proj.baseline.name),
                    vec!["proficiencies".to_string()],
                    serde_json::json!({ "removed": true, "proficiencies": profile.proficiencies }),
                )
            }

            "upsert_trait" => {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| anyhow!("upsert_trait requires 'name'"))?
                    .to_string();
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(slugify)
                    .unwrap_or_else(|| slugify(&name));
                let existing = profile
                    .traits_and_perks
                    .iter()
                    .find(|t| t.id == id || t.name.eq_ignore_ascii_case(&name))
                    .cloned();
                let tr = CharacterTrait {
                    id: id.clone(),
                    name: name.clone(),
                    category: args
                        .get("category")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|t| t.category.clone()))
                        .unwrap_or_else(|| "physique".to_string()),
                    rank_or_grade: args
                        .get("rank_or_grade")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|t| t.rank_or_grade.clone()))
                        .unwrap_or_default(),
                    active: args
                        .get("active")
                        .and_then(|v| v.as_bool())
                        .or_else(|| existing.as_ref().map(|t| t.active))
                        .unwrap_or(true),
                    effect: args
                        .get("effect")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|t| t.effect.clone()))
                        .unwrap_or_default(),
                    notes: args
                        .get("notes")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| existing.as_ref().map(|t| t.notes.clone()))
                        .unwrap_or_default(),
                };
                if let Some(ex) = profile
                    .traits_and_perks
                    .iter_mut()
                    .find(|t| t.id == id || t.name.eq_ignore_ascii_case(&name))
                {
                    *ex = tr.clone();
                } else {
                    profile.traits_and_perks.push(tr.clone());
                }
                (
                    format!(
                        "Saved trait '{}' [{}] on {}",
                        tr.name, tr.category, proj.baseline.name
                    ),
                    vec!["traits_and_perks".to_string()],
                    serde_json::json!({ "trait": tr, "traits_and_perks": profile.traits_and_perks }),
                )
            }

            "remove_trait" => {
                let id = args
                    .get("id")
                    .or_else(|| args.get("name"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("remove_trait requires 'id' or 'name'"))?;
                let slug = slugify(id);
                let before = profile.traits_and_perks.len();
                profile
                    .traits_and_perks
                    .retain(|t| t.id != id && t.id != slug && !t.name.eq_ignore_ascii_case(id));
                if profile.traits_and_perks.len() == before {
                    bail!("Trait '{id}' not found on '{cid}'");
                }
                (
                    format!("Removed trait '{id}' from {}", proj.baseline.name),
                    vec!["traits_and_perks".to_string()],
                    serde_json::json!({ "removed": true, "traits_and_perks": profile.traits_and_perks }),
                )
            }

            other => bail!(
                "Unknown manage_player_character action '{other}'. Expected: get_sheet, update_avatar, upsert_marker, remove_marker, upsert_outfit, set_active_outfit, remove_outfit, upsert_title, toggle_title, remove_title, adjust_stat_xp, upsert_job, adjust_job_xp, remove_job, upsert_proficiency, adjust_proficiency_xp, remove_proficiency, upsert_trait, remove_trait."
            ),
        };

        profile.updated_at_world_time = now;
        self.db.upsert_character_profile(&profile)?;
        let full_summary = if reason.is_empty() {
            summary
        } else {
            format!("{summary} — {reason}")
        };
        let ev = self.db.append_event(
            proj.current.current_location,
            vec![cid.clone()],
            full_summary.clone(),
            EventPayload::CharacterProfileUpdated {
                character_id: cid.clone(),
                fields_updated,
            },
        )?;
        if act == "update_avatar" {
            let _ = self.sync_apparel_discomfort(&cid, 0);
        }
        self.maybe_compile()?;
        let mut out = extra_payload;
        if let Some(obj) = out.as_object_mut() {
            obj.insert("event_seq".to_string(), serde_json::json!(ev.seq));
            obj.insert("summary".to_string(), serde_json::json!(full_summary));
            obj.insert("character_id".to_string(), serde_json::json!(cid));
        }
        Ok(out)
    }

    // -------------------------------------------------------------------------
    // Living World: Prominent Figure Mobility, Business Travel & Networking
    // -------------------------------------------------------------------------

    fn all_world_routes_merged(
        &self,
        all_places: &std::collections::BTreeMap<String, PlaceProjection>,
    ) -> Vec<TravelRoute> {
        let mut routes = self.db.list_travel_routes().unwrap_or_default();
        let existing_pairs: std::collections::BTreeSet<(String, String)> = routes
            .iter()
            .flat_map(|r| {
                let mut pairs = vec![(r.from_place_id.clone(), r.to_place_id.clone())];
                if r.bidirectional {
                    pairs.push((r.to_place_id.clone(), r.from_place_id.clone()));
                }
                pairs
            })
            .collect();
        for synth in synthesize_routes_from_places(all_places) {
            if !existing_pairs.contains(&(synth.from_place_id.clone(), synth.to_place_id.clone())) {
                routes.push(synth);
            }
        }
        routes
    }

    fn npc_is_mobile_alive(proj: &CharacterProjection) -> bool {
        if proj.baseline.kind.eq_ignore_ascii_case("pc") {
            return false;
        }
        let st = proj.current.alive_status.to_lowercase();
        !matches!(
            st.as_str(),
            "dead" | "deceased" | "incapacitated" | "imprisoned" | "missing" | "coma" | "unconscious"
        )
    }

    fn infer_npc_mobility_mode(
        proj: &CharacterProjection,
        profile: &CharacterProfileExt,
        faction_leaders: &std::collections::BTreeSet<String>,
        auto_include_prominent: bool,
    ) -> Option<(String, Vec<String>)> {
        if !Self::npc_is_mobile_alive(proj) {
            return None;
        }
        let mode_l = profile.mobility.mobility_mode.trim().to_lowercase();
        if mode_l == "stationary" {
            return None;
        }
        if mode_l != "auto" && !mode_l.is_empty() {
            let purposes = if !profile.mobility.purposes.is_empty() {
                profile.mobility.purposes.clone()
            } else {
                default_purposes_for_mode(&mode_l)
            };
            return Some((mode_l, purposes));
        }

        let explicit = profile.mobility.prominent
            || profile.mobility.active_trip.is_some()
            || !profile.mobility.preferred_destinations.is_empty();

        if !explicit && !auto_include_prominent {
            return None;
        }

        let hay = format!(
            "{} {} {} {} {} {}",
            proj.baseline.title,
            proj.baseline.archetype,
            proj.baseline.tags.join(" "),
            profile.occupation,
            profile.social_class,
            proj.baseline.faction
        )
        .to_lowercase();

        let inferred = if hay.contains("merchant")
            || hay.contains("trader")
            || hay.contains("guildmaster")
            || hay.contains("factor")
            || hay.contains("caravan")
            || hay.contains("broker")
            || hay.contains("quartermaster")
            || hay.contains("smuggler")
        {
            "business_circuit"
        } else if hay.contains("envoy")
            || hay.contains("diplomat")
            || hay.contains("ambassador")
            || hay.contains("magistrate")
            || hay.contains("prefect")
            || hay.contains("lord")
            || hay.contains("noble")
            || hay.contains("councilor")
            || hay.contains("archivist")
            || hay.contains("scholar")
            || hay.contains("emissary")
        {
            "networking_envoy"
        } else if hay.contains("elder")
            || hay.contains("sect master")
            || hay.contains("abbot")
            || hay.contains("commander")
            || hay.contains("captain")
            || hay.contains("inspector")
            || hay.contains("warden")
            || hay.contains("steward")
            || hay.contains("marshal")
            || faction_leaders.contains(&proj.baseline.id)
        {
            "inspector"
        } else if hay.contains("wanderer")
            || hay.contains("courier")
            || hay.contains("messenger")
            || hay.contains("scout")
            || hay.contains("mercenary")
            || hay.contains("peddler")
            || hay.contains("bard")
            || hay.contains("ronin")
        {
            "wanderer"
        } else if explicit {
            "business_circuit"
        } else {
            return None;
        };

        let purposes = if !profile.mobility.purposes.is_empty() {
            profile.mobility.purposes.clone()
        } else {
            default_purposes_for_mode(inferred)
        };
        Some((inferred.to_string(), purposes))
    }

    pub fn configure_npc_mobility(
        &self,
        character_id: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        let mut profile = self.get_character_profile(&cid)?;
        let m = &mut profile.mobility;

        if let Some(p) = args.get("prominent").and_then(|v| v.as_bool()) {
            m.prominent = p;
        }
        if let Some(mode) = args.get("mobility_mode").and_then(|v| v.as_str()) {
            m.mobility_mode = mode.trim().to_lowercase();
            if m.mobility_mode != "stationary" && args.get("prominent").is_none() {
                m.prominent = true;
            }
        }
        if let Some(home) = args.get("home_place_id").and_then(|v| v.as_str()) {
            let trimmed = home.trim();
            m.home_place_id = if trimmed.is_empty() {
                None
            } else {
                Some(resolve_place_id(&self.db, trimmed).unwrap_or_else(|_| slugify(trimmed)))
            };
        } else if m.home_place_id.is_none() {
            m.home_place_id = proj
                .current
                .current_location
                .clone()
                .or_else(|| proj.baseline.initial_location.clone());
        }
        if let Some(arr) = args.get("preferred_destinations").and_then(|v| v.as_array()) {
            m.preferred_destinations = arr
                .iter()
                .filter_map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()))
                .map(|s| resolve_place_id(&self.db, s).unwrap_or_else(|_| slugify(s)))
                .collect();
        }
        if let Some(arr) = args.get("purposes").and_then(|v| v.as_array()) {
            m.purposes = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect();
        }
        if let Some(d) = args.get("trip_cadence_days").and_then(|v| v.as_u64()) {
            m.trip_cadence_days = (d as u32).max(1);
        }
        if let Some(d) = args.get("stay_duration_days").and_then(|v| v.as_u64()) {
            m.stay_duration_days = (d as u32).max(1);
        }
        if let Some(sp) = args.get("travel_speed_mode").and_then(|v| v.as_str()) {
            if !sp.trim().is_empty() {
                m.travel_speed_mode = sp.trim().to_lowercase();
            }
        }
        if let Some(arr) = args.get("entourage_ids").and_then(|v| v.as_array()) {
            m.entourage_ids = arr
                .iter()
                .filter_map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()))
                .map(|s| resolve_character_id(&self.db, s).unwrap_or_else(|_| slugify(s)))
                .collect();
        }
        if let Some(st) = args.get("current_status").and_then(|v| v.as_str()) {
            m.current_status = st.trim().to_lowercase();
            if m.current_status == "at_home" {
                m.active_trip = None;
            }
        }

        let now = self.db.get_campaign_meta()?.clock.format_short();
        profile.updated_at_world_time = now;
        self.db.upsert_character_profile(&profile)?;
        let summary = format!(
            "Configured mobility for {} [prominent={}, mode={}, home={}, stops={}]",
            proj.baseline.name,
            profile.mobility.prominent,
            profile.mobility.mobility_mode,
            profile
                .mobility
                .home_place_id
                .as_deref()
                .unwrap_or("none"),
            profile.mobility.preferred_destinations.len()
        );
        let ev = self.db.append_event(
            proj.current.current_location,
            vec![cid.clone()],
            summary.clone(),
            EventPayload::CharacterProfileUpdated {
                character_id: cid.clone(),
                fields_updated: vec!["mobility".to_string()],
            },
        )?;
        self.maybe_compile()?;
        Ok(serde_json::json!({
            "event_seq": ev.seq,
            "summary": summary,
            "character_id": cid,
            "mobility": profile.mobility
        }))
    }

    pub fn dispatch_npc_trip(
        &self,
        character_id: &str,
        destination_place_id: &str,
        purpose: &str,
        details: &str,
        stay_duration_days: Option<u32>,
        transport_mode: Option<&str>,
    ) -> Result<serde_json::Value> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let dest = resolve_place_id(&self.db, destination_place_id)?;
        let proj = load_character_projection(&self.db, &cid, None)?;
        if !Self::npc_is_mobile_alive(&proj) {
            bail!(
                "Character '{}' ({}, {}) cannot be dispatched on an autonomous NPC trip",
                proj.baseline.name,
                proj.baseline.kind,
                proj.current.alive_status
            );
        }
        let origin = proj
            .current
            .current_location
            .clone()
            .or_else(|| proj.baseline.initial_location.clone())
            .ok_or_else(|| anyhow!("Character '{}' has no current location to depart from", cid))?;
        if origin == dest {
            bail!("'{}' is already at '{}'", proj.baseline.name, dest);
        }

        let meta = self.db.get_campaign_meta()?;
        let all_places = load_all_place_projections(&self.db, None)?;
        let routes = self.all_world_routes_merged(&all_places);
        let mut profile = self.get_character_profile(&cid)?;
        let mode = transport_mode
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_lowercase())
            .unwrap_or_else(|| profile.mobility.travel_speed_mode.clone());

        let (legs, _km, hours_f) = compute_shortest_route(
            &routes,
            &origin,
            &dest,
            &mode,
            &meta.clock.season,
            &meta.clock.global_weather,
            1.0,
            false,
        )
        .ok_or_else(|| {
            anyhow!(
                "No open travel route exists from '{origin}' to '{dest}' (all connecting routes may be blocked/destroyed)"
            )
        })?;

        let mut detour_note = String::new();
        let direct_closed = routes.iter().any(|r| {
            ((r.from_place_id == origin && r.to_place_id == dest)
                || (r.bidirectional && r.from_place_id == dest && r.to_place_id == origin))
                && (r.status.eq_ignore_ascii_case("blocked")
                    || r.status.eq_ignore_ascii_case("destroyed"))
        });
        if direct_closed {
            detour_note = format!("Detoured around closed direct road {origin} -> {dest}");
        }

        let travel_hours = (hours_f.ceil() as u64).max(2);
        let now_h = meta.clock.total_hours();
        let eta_h = now_h + (travel_hours as i64);
        let eta_clock = meta.clock.clock_after_hours(travel_hours);
        let stay_days = stay_duration_days.unwrap_or(profile.mobility.stay_duration_days.max(1));
        let ret_h = eta_h + (stay_days as i64) * 24;
        let ret_clock = eta_clock.clock_after_hours((stay_days as u64) * 24);
        let waypoints: Vec<String> = legs.iter().map(|l| l.to_place_id.clone()).collect();
        let clean_purpose = if purpose.trim().is_empty() {
            "business".to_string()
        } else {
            purpose.trim().to_string()
        };

        let trip = NpcTripState {
            origin_place_id: origin.clone(),
            destination_place_id: dest.clone(),
            route_waypoints: waypoints,
            purpose: clean_purpose.clone(),
            details: details.trim().to_string(),
            departed_at_world_time: meta.clock.format_short(),
            departed_total_hours: now_h,
            eta_world_time: eta_clock.format_short(),
            eta_total_hours: eta_h,
            return_after_total_hours: ret_h,
            return_after_world_time: ret_clock.format_short(),
            transport_mode: mode.clone(),
            detour_note: detour_note.clone(),
        };

        if profile.mobility.home_place_id.is_none() {
            profile.mobility.home_place_id = Some(origin.clone());
        }
        profile.mobility.prominent = true;
        profile.mobility.current_status = "in_transit".to_string();
        profile.mobility.active_trip = Some(trip.clone());
        profile.updated_at_world_time = meta.clock.format_short();
        self.db.upsert_character_profile(&profile)?;

        let summary = format!(
            "{} departed {} for {} ({}, ~{}h by {}, ETA {}){}{}",
            proj.baseline.name,
            origin,
            dest,
            clean_purpose,
            travel_hours,
            mode,
            trip.eta_world_time,
            if details.trim().is_empty() {
                String::new()
            } else {
                format!(" — {}", details.trim())
            },
            if detour_note.is_empty() {
                String::new()
            } else {
                format!(" [{detour_note}]")
            }
        );
        let ev = self.db.append_event(
            Some(origin.clone()),
            vec![cid.clone(), origin, dest],
            summary.clone(),
            EventPayload::NarrativeEvent {
                title: format!("Departure: {} ({})", proj.baseline.name, clean_purpose),
                details: summary.clone(),
                tags: vec!["npc_movement".into(), clean_purpose],
            },
        )?;
        self.maybe_compile()?;
        Ok(serde_json::json!({
            "event_seq": ev.seq,
            "summary": summary,
            "character_id": cid,
            "trip": trip
        }))
    }

    fn resolve_npc_arrival(
        &self,
        cid: &str,
        proj_name: &str,
        profile: &mut CharacterProfileExt,
        now_h: i64,
        now_str: &str,
        all_chars: &std::collections::BTreeMap<String, CharacterProjection>,
        all_places: &std::collections::BTreeMap<String, PlaceProjection>,
    ) -> Result<serde_json::Value> {
        let trip = profile
            .mobility
            .active_trip
            .clone()
            .ok_or_else(|| anyhow!("No active trip to resolve on '{cid}'"))?;
        let dest = trip.destination_place_id.clone();
        let dest_name = all_places
            .get(&dest)
            .map(|p| p.baseline.name.clone())
            .unwrap_or_else(|| dest.clone());

        // Move traveler + entourage into destination
        let move_reason = format!(
            "Arrived for {}{}",
            trip.purpose,
            if trip.details.is_empty() {
                String::new()
            } else {
                format!(" ({})", trip.details)
            }
        );
        let _ = self.move_character(cid, &dest, &move_reason);
        for eid in &profile.mobility.entourage_ids {
            if all_chars
                .get(eid)
                .map(Self::npc_is_mobile_alive)
                .unwrap_or(false)
            {
                let _ = self.move_character(eid, &dest, &format!("Accompanying {proj_name}"));
            }
        }

        let mut engagement_opt = None;
        let mut rumor_shared: Option<String> = None;

        if trip.purpose.eq_ignore_ascii_case("return_home") {
            profile.mobility.current_status = "at_home".to_string();
            profile.mobility.active_trip = None;
            profile.mobility.last_trip_ended_total_hours = Some(now_h);
        } else {
            profile.mobility.current_status = "visiting".to_string();
            // Find counterparts present at destination for organic business / networking engagement
            let counterparts: Vec<(&String, &CharacterProjection)> = all_chars
                .iter()
                .filter(|(other_id, other_p)| {
                    other_id.as_str() != cid
                        && !profile.mobility.entourage_ids.contains(*other_id)
                        && Self::npc_is_mobile_alive(other_p)
                        && other_p.current.current_location.as_deref() == Some(dest.as_str())
                })
                .take(2)
                .collect();

            let cp_ids: Vec<String> = counterparts.iter().map(|(id, _)| (*id).clone()).collect();
            let cp_names: Vec<String> = counterparts
                .iter()
                .map(|(_, p)| p.baseline.name.clone())
                .collect();

            let eng_summary = if !cp_names.is_empty() {
                format!(
                    "{proj_name} arrived in {dest_name} for {} and networked with {}",
                    trip.purpose,
                    cp_names.join(" & ")
                )
            } else {
                format!(
                    "{proj_name} arrived in {dest_name} to conduct {} business",
                    trip.purpose
                )
            };

            let eng = NpcEngagement {
                world_time: now_str.to_string(),
                place_id: dest.clone(),
                purpose: trip.purpose.clone(),
                counterparts: cp_ids.clone(),
                summary: eng_summary.clone(),
                visibility: "public".to_string(),
            };
            profile.mobility.recent_engagements.push(eng.clone());
            if profile.mobility.recent_engagements.len() > 10 {
                let drop_n = profile.mobility.recent_engagements.len() - 10;
                profile.mobility.recent_engagements.drain(0..drop_n);
            }

            // Record mirrored engagement on counterparts + organic rumor propagation (1 public claim max)
            let traveler_beliefs = self.db.list_beliefs_for_actor(cid).unwrap_or_default();
            let all_claims = self.db.list_knowledge_claims().unwrap_or_default();
            for cp_id in &cp_ids {
                if let Ok(mut cp_prof) = self.get_character_profile(cp_id) {
                    cp_prof.mobility.recent_engagements.push(NpcEngagement {
                        world_time: now_str.to_string(),
                        place_id: dest.clone(),
                        purpose: trip.purpose.clone(),
                        counterparts: vec![cid.to_string()],
                        summary: eng_summary.clone(),
                        visibility: "public".to_string(),
                    });
                    if cp_prof.mobility.recent_engagements.len() > 10 {
                        let drop_n = cp_prof.mobility.recent_engagements.len() - 10;
                        cp_prof.mobility.recent_engagements.drain(0..drop_n);
                    }
                    cp_prof.updated_at_world_time = now_str.to_string();
                    let _ = self.db.upsert_character_profile(&cp_prof);
                }

                // Propagate up to 1 non-concealed public claim the traveler knows that the counterpart doesn't
                if rumor_shared.is_none() && !traveler_beliefs.is_empty() {
                    let cp_known: std::collections::BTreeSet<String> = self
                        .db
                        .list_beliefs_for_actor(cp_id)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|b| b.claim_id)
                        .collect();
                    for tb in &traveler_beliefs {
                        if tb.concealing || tb.confidence < 55 || cp_known.contains(&tb.claim_id) {
                            continue;
                        }
                        if let Some(cl) = all_claims.iter().find(|c| {
                            c.id == tb.claim_id
                                && c.superseded_by_id.is_none()
                                && c.visibility != "gm_secret"
                        }) {
                            let propagated = ActorBelief {
                                claim_id: cl.id.clone(),
                                actor_id: cp_id.clone(),
                                perceived_claim: if tb.perceived_claim.is_empty() {
                                    cl.claim.clone()
                                } else {
                                    tb.perceived_claim.clone()
                                },
                                stance: "believes".to_string(),
                                confidence: tb.confidence.saturating_sub(10).max(40),
                                source: format!("networked_with:{cid}"),
                                source_trustworthy: Some(true),
                                concealing: false,
                                outdated: tb.outdated,
                                learned_at_world_time: now_str.to_string(),
                                notes: format!("Exchanged during {} in {dest_name}", trip.purpose),
                            };
                            if self.db.upsert_actor_belief(&propagated).is_ok() {
                                rumor_shared = Some(format!(
                                    "{proj_name} shared '{}' with {cp_id} in {dest_name}",
                                    cl.subject
                                ));
                            }
                            break;
                        }
                    }
                }
            }

            let mut involved = vec![cid.to_string(), dest.clone()];
            involved.extend(cp_ids.iter().cloned());
            let _ = self.db.append_event(
                Some(dest.clone()),
                involved,
                eng_summary.clone(),
                EventPayload::NarrativeEvent {
                    title: format!("Arrival & Networking: {proj_name} in {dest_name}"),
                    details: eng_summary,
                    tags: vec!["npc_movement".into(), trip.purpose.clone()],
                },
            );
            engagement_opt = Some(eng);
        }

        profile.updated_at_world_time = now_str.to_string();
        self.db.upsert_character_profile(profile)?;

        Ok(serde_json::json!({
            "event": if trip.purpose == "return_home" { "returned_home" } else { "arrived" },
            "character_id": cid,
            "name": proj_name,
            "destination": dest,
            "purpose": trip.purpose,
            "engagement": engagement_opt,
            "rumor_shared": rumor_shared
        }))
    }

    /// Simulates prominent NPC business travel, diplomatic/networking circuits, arrivals, departures,
    /// route-disruption detours/stranding, and organic rumor propagation across elapsed in-world hours.
    pub fn tick_npc_movements(
        &self,
        elapsed_hours: u64,
        scope_character_ids: &[String],
        auto_include_prominent: bool,
        seed: u64,
    ) -> Result<serde_json::Value> {
        let meta = self.db.get_campaign_meta()?;
        let now_h = meta.clock.total_hours();
        let now_str = meta.clock.format_short();
        let all_places = load_all_place_projections(&self.db, None)?;
        if all_places.len() < 2 {
            return Ok(serde_json::json!({
                "events_count": 0,
                "events": [],
                "note": "Need at least 2 connected places for NPC travel."
            }));
        }
        let routes = self.all_world_routes_merged(&all_places);
        if routes.is_empty() {
            return Ok(serde_json::json!({
                "events_count": 0,
                "events": [],
                "note": "No travel routes defined between places yet."
            }));
        }

        let all_chars = load_all_character_projections(&self.db, None)?;
        let mut faction_leaders = std::collections::BTreeSet::new();
        for fac in self.db.list_factions().unwrap_or_default() {
            for m in &fac.members {
                let rl = m.rank.to_lowercase();
                if rl.contains("leader")
                    || rl.contains("elder")
                    || rl.contains("master")
                    || rl.contains("captain")
                    || rl.contains("envoy")
                    || rl.contains("steward")
                    || rl.contains("chief")
                {
                    faction_leaders.insert(m.character_id.clone());
                }
            }
        }

        let scope_set: std::collections::BTreeSet<String> = scope_character_ids
            .iter()
            .map(|s| resolve_character_id(&self.db, s).unwrap_or_else(|_| slugify(s)))
            .collect();

        // Collect all eligible prominent / active-trip NPCs
        let mut candidates: Vec<(String, String, Vec<String>)> = Vec::new();
        for (cid, proj) in &all_chars {
            if !scope_set.is_empty() && !scope_set.contains(cid) {
                // Even when scoped, always finish arrivals for NPCs already in transit or visiting!
                let prof = self.get_character_profile(cid)?;
                if prof.mobility.active_trip.is_none() && !prof.mobility.prominent {
                    continue;
                }
            }
            let prof = self.get_character_profile(cid)?;
            if let Some((mode, purposes)) =
                Self::infer_npc_mobility_mode(proj, &prof, &faction_leaders, auto_include_prominent)
            {
                candidates.push((cid.clone(), mode, purposes));
            }
        }

        let mut events_out: Vec<serde_json::Value> = Vec::new();

        for (idx, (cid, effective_mode, purposes)) in candidates.iter().enumerate() {
            let proj = match all_chars.get(cid) {
                Some(p) => p,
                None => continue,
            };
            let mut profile = self.get_character_profile(cid)?;
            if profile.mobility.home_place_id.is_none() {
                profile.mobility.home_place_id = proj
                    .current
                    .current_location
                    .clone()
                    .or_else(|| proj.baseline.initial_location.clone());
            }
            let home_id = match profile.mobility.home_place_id.clone() {
                Some(h) => h,
                None => continue,
            };

            // PHASE A: Complete in-transit trips whose ETA has arrived
            if profile.mobility.current_status == "in_transit" {
                if let Some(ref tr) = profile.mobility.active_trip {
                    if tr.eta_total_hours <= now_h {
                        if let Ok(ev_json) = self.resolve_npc_arrival(
                            cid,
                            &proj.baseline.name,
                            &mut profile,
                            now_h,
                            &now_str,
                            &all_chars,
                            &all_places,
                        ) {
                            events_out.push(ev_json);
                        }
                    }
                }
            }

            // PHASE B: End visits whose stay duration has elapsed -> depart to next stop or home
            if profile.mobility.current_status == "visiting" {
                let due_to_leave = profile
                    .mobility
                    .active_trip
                    .as_ref()
                    .map(|t| t.return_after_total_hours <= now_h)
                    .unwrap_or(true);
                if due_to_leave {
                    let cur_loc = profile
                        .mobility
                        .active_trip
                        .as_ref()
                        .map(|t| t.destination_place_id.clone())
                        .or_else(|| proj.current.current_location.clone())
                        .unwrap_or_else(|| home_id.clone());

                    // Decide next leg: next stop in business_circuit (if any unvisited) or return_home
                    let mut next_dest = home_id.clone();
                    let mut next_purpose = "return_home".to_string();
                    let mut next_details = format!("Returning to {home_id}");

                    if effective_mode == "business_circuit"
                        && profile.mobility.preferred_destinations.len() >= 2
                        && cur_loc != home_id
                    {
                        let next_idx = profile.mobility.circuit_cursor + 1;
                        if next_idx < profile.mobility.preferred_destinations.len() {
                            let cand = profile.mobility.preferred_destinations[next_idx].clone();
                            if cand != cur_loc {
                                profile.mobility.circuit_cursor = next_idx;
                                next_dest = cand;
                                next_purpose = purposes
                                    .get(next_idx % purposes.len().max(1))
                                    .cloned()
                                    .unwrap_or_else(|| "business".to_string());
                                next_details = "Continuing business circuit".to_string();
                            }
                        } else {
                            profile.mobility.circuit_cursor = 0;
                        }
                    }

                    if cur_loc == next_dest {
                        profile.mobility.current_status = "at_home".to_string();
                        profile.mobility.active_trip = None;
                        profile.mobility.last_trip_ended_total_hours = Some(now_h);
                        profile.updated_at_world_time = now_str.clone();
                        self.db.upsert_character_profile(&profile)?;
                    } else if let Some((legs, _km, hours_f)) = compute_shortest_route(
                        &routes,
                        &cur_loc,
                        &next_dest,
                        &profile.mobility.travel_speed_mode,
                        &meta.clock.season,
                        &meta.clock.global_weather,
                        1.0,
                        false,
                    ) {
                        let travel_h = (hours_f.ceil() as u64).max(2);
                        let dep_h = profile
                            .mobility
                            .active_trip
                            .as_ref()
                            .map(|t| t.return_after_total_hours.min(now_h))
                            .unwrap_or(now_h);
                        let eta_h = dep_h + (travel_h as i64);
                        let stay_d = profile.mobility.stay_duration_days.max(1);
                        let eta_delta = (eta_h - now_h).max(0) as u64;
                        let eta_clock = meta.clock.clock_after_hours(eta_delta);
                        let ret_clock = eta_clock.clock_after_hours((stay_d as u64) * 24);
                        let trip = NpcTripState {
                            origin_place_id: cur_loc.clone(),
                            destination_place_id: next_dest.clone(),
                            route_waypoints: legs.iter().map(|l| l.to_place_id.clone()).collect(),
                            purpose: next_purpose.clone(),
                            details: next_details,
                            departed_at_world_time: now_str.clone(),
                            departed_total_hours: dep_h,
                            eta_world_time: eta_clock.format_short(),
                            eta_total_hours: eta_h,
                            return_after_total_hours: eta_h + (stay_d as i64) * 24,
                            return_after_world_time: ret_clock.format_short(),
                            transport_mode: profile.mobility.travel_speed_mode.clone(),
                            detour_note: String::new(),
                        };
                        profile.mobility.current_status = "in_transit".to_string();
                        profile.mobility.active_trip = Some(trip);
                        profile.updated_at_world_time = now_str.clone();
                        self.db.upsert_character_profile(&profile)?;

                        if eta_h <= now_h {
                            if let Ok(ev_json) = self.resolve_npc_arrival(
                                cid,
                                &proj.baseline.name,
                                &mut profile,
                                now_h,
                                &now_str,
                                &all_chars,
                                &all_places,
                            ) {
                                events_out.push(ev_json);
                            }
                        } else {
                            events_out.push(serde_json::json!({
                                "event": "departed",
                                "character_id": cid,
                                "name": proj.baseline.name,
                                "from": cur_loc,
                                "to": next_dest,
                                "purpose": next_purpose,
                                "eta": eta_clock.format_short()
                            }));
                        }
                    } else {
                        // All routes back home are blocked/destroyed -> figure is stranded!
                        profile.mobility.current_status = "stranded".to_string();
                        if let Some(ref mut tr) = profile.mobility.active_trip {
                            tr.detour_note = format!(
                                "Stranded in {cur_loc}: no open route to {next_dest} (roads blocked/destroyed)"
                            );
                        }
                        profile.updated_at_world_time = now_str.clone();
                        self.db.upsert_character_profile(&profile)?;
                        let note = format!(
                            "{} is stranded in {cur_loc} — route to {next_dest} is severed",
                            proj.baseline.name
                        );
                        let _ = self.db.append_event(
                            Some(cur_loc.clone()),
                            vec![cid.clone(), cur_loc.clone(), next_dest.clone()],
                            note.clone(),
                            EventPayload::NarrativeEvent {
                                title: format!("Traveler Stranded: {}", proj.baseline.name),
                                details: note.clone(),
                                tags: vec!["npc_movement".into(), "stranded".into()],
                            },
                        );
                        events_out.push(serde_json::json!({
                            "event": "stranded",
                            "character_id": cid,
                            "name": proj.baseline.name,
                            "at": cur_loc,
                            "intended_destination": next_dest,
                            "note": note
                        }));
                    }
                }
            }

            // PHASE C: Prominent figures at home whose cadence window has elapsed embark on business/networking travel
            if profile.mobility.current_status == "at_home" || profile.mobility.current_status == "stranded" {
                let cadence_h = (profile.mobility.trip_cadence_days.max(1) as i64) * 24;
                // Deterministic stagger so prominent NPCs spread out naturally instead of all leaving the same hour
                let h_mix = seed
                    .wrapping_add((idx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
                    .wrapping_add(cid.bytes().fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64)));
                let should_depart = match profile.mobility.last_trip_ended_total_hours {
                    Some(last_h) => (now_h - last_h) >= cadence_h,
                    None => {
                        // First-ever trip: departs if elapsed_hours >= stagger window (at most cadence_h)
                        let stagger = (h_mix % (cadence_h.max(24) as u64)).min(48) as i64;
                        (elapsed_hours as i64) >= stagger || profile.mobility.prominent
                    }
                };

                if should_depart {
                    let origin = proj
                        .current
                        .current_location
                        .clone()
                        .unwrap_or_else(|| home_id.clone());

                    // Build ranked destination candidates
                    let mut dest_pool: Vec<String> = Vec::new();
                    if !profile.mobility.preferred_destinations.is_empty() {
                        let cur_i = profile.mobility.circuit_cursor % profile.mobility.preferred_destinations.len();
                        for offset in 0..profile.mobility.preferred_destinations.len() {
                            let p = &profile.mobility.preferred_destinations[(cur_i + offset) % profile.mobility.preferred_destinations.len()];
                            if p != &origin && !dest_pool.contains(p) {
                                dest_pool.push(p.clone());
                            }
                        }
                    } else {
                        // For networking_envoy: prioritize places currently hosting other alive NPCs!
                        if effective_mode == "networking_envoy" {
                            for (oid, op) in &all_chars {
                                if oid != cid && Self::npc_is_mobile_alive(op) {
                                    if let Some(ref oloc) = op.current.current_location {
                                        if oloc != &origin && !dest_pool.contains(oloc) {
                                            dest_pool.push(oloc.clone());
                                        }
                                    }
                                }
                            }
                        }
                        // Add all other places in the world (filtered by real Dijkstra reachability below!)
                        for pid in all_places.keys() {
                            if pid != &origin && !dest_pool.contains(pid) {
                                dest_pool.push(pid.clone());
                            }
                        }
                    }

                    // Filter to reachable destinations via Dijkstra
                    let mut reachable: Vec<(String, Vec<String>, u64, String)> = Vec::new();
                    for cand in &dest_pool {
                        if let Some((legs, _km, hours_f)) = compute_shortest_route(
                            &routes,
                            &origin,
                            cand,
                            &profile.mobility.travel_speed_mode,
                            &meta.clock.season,
                            &meta.clock.global_weather,
                            1.0,
                            false,
                        ) {
                            let direct_closed = routes.iter().any(|r| {
                                ((r.from_place_id == origin && r.to_place_id == *cand)
                                    || (r.bidirectional && r.from_place_id == *cand && r.to_place_id == origin))
                                    && (r.status.eq_ignore_ascii_case("blocked")
                                        || r.status.eq_ignore_ascii_case("destroyed"))
                            });
                            let detour = if direct_closed {
                                format!("Detoured around closed road {origin} -> {cand}")
                            } else {
                                String::new()
                            };
                            reachable.push((
                                cand.clone(),
                                legs.iter().map(|l| l.to_place_id.clone()).collect(),
                                (hours_f.ceil() as u64).max(2),
                                detour,
                            ));
                        }
                    }

                    if !reachable.is_empty() {
                        let pick_idx = if !profile.mobility.preferred_destinations.is_empty()
                            || effective_mode == "networking_envoy"
                        {
                            0
                        } else {
                            (h_mix as usize) % reachable.len()
                        };
                        let (dest, waypoints, travel_h, detour_note) = reachable.remove(pick_idx);
                        let purpose = purposes
                            .get((h_mix as usize) % purposes.len().max(1))
                            .cloned()
                            .unwrap_or_else(|| "business".to_string());
                        let stay_d = profile.mobility.stay_duration_days.max(1);
                        // Depart partway through the elapsed window if elapsed_hours > travel_h so arrivals feel natural
                        let dep_offset = if elapsed_hours > travel_h {
                            (elapsed_hours - travel_h).min(elapsed_hours / 2)
                        } else {
                            0
                        };
                        let dep_h = now_h - (elapsed_hours.saturating_sub(dep_offset) as i64);
                        let eta_h = dep_h + (travel_h as i64);
                        let eta_clock = if eta_h > now_h {
                            meta.clock.clock_after_hours((eta_h - now_h) as u64)
                        } else {
                            meta.clock.clone()
                        };
                        let ret_h = eta_h + (stay_d as i64) * 24;
                        let ret_clock = eta_clock.clock_after_hours((stay_d as u64) * 24);
                        let details = format!("Scheduled {effective_mode} trip ({purpose})");

                        let trip = NpcTripState {
                            origin_place_id: origin.clone(),
                            destination_place_id: dest.clone(),
                            route_waypoints: waypoints,
                            purpose: purpose.clone(),
                            details: details.clone(),
                            departed_at_world_time: now_str.clone(),
                            departed_total_hours: dep_h,
                            eta_world_time: eta_clock.format_short(),
                            eta_total_hours: eta_h,
                            return_after_total_hours: ret_h,
                            return_after_world_time: ret_clock.format_short(),
                            transport_mode: profile.mobility.travel_speed_mode.clone(),
                            detour_note: detour_note.clone(),
                        };

                        profile.mobility.prominent = true;
                        profile.mobility.current_status = "in_transit".to_string();
                        profile.mobility.active_trip = Some(trip);
                        profile.updated_at_world_time = now_str.clone();
                        self.db.upsert_character_profile(&profile)?;

                        let dep_summary = format!(
                            "{} departed {} for {} ({}, ~{}h by {}){}",
                            proj.baseline.name,
                            origin,
                            dest,
                            purpose,
                            travel_h,
                            profile.mobility.travel_speed_mode,
                            if detour_note.is_empty() {
                                String::new()
                            } else {
                                format!(" [{detour_note}]")
                            }
                        );
                        let _ = self.db.append_event(
                            Some(origin.clone()),
                            vec![cid.clone(), origin.clone(), dest.clone()],
                            dep_summary.clone(),
                            EventPayload::NarrativeEvent {
                                title: format!("Business/Networking Departure: {}", proj.baseline.name),
                                details: dep_summary,
                                tags: vec!["npc_movement".into(), purpose.clone()],
                            },
                        );

                        if eta_h <= now_h {
                            if let Ok(ev_json) = self.resolve_npc_arrival(
                                cid,
                                &proj.baseline.name,
                                &mut profile,
                                now_h,
                                &now_str,
                                &all_chars,
                                &all_places,
                            ) {
                                events_out.push(ev_json);
                            }
                        } else {
                            events_out.push(serde_json::json!({
                                "event": "departed",
                                "character_id": cid,
                                "name": proj.baseline.name,
                                "from": origin,
                                "to": dest,
                                "purpose": purpose,
                                "eta": eta_clock.format_short(),
                                "detour_note": detour_note
                            }));
                        }
                    }
                }
            }
        }

        if !events_out.is_empty() {
            self.maybe_compile()?;
        }
        Ok(serde_json::json!({
            "world_time": now_str,
            "elapsed_hours": elapsed_hours,
            "events_count": events_out.len(),
            "events": events_out
        }))
    }

    pub fn list_npc_movements(&self) -> Result<serde_json::Value> {
        let meta = self.db.get_campaign_meta()?;
        let all_chars = load_all_character_projections(&self.db, None)?;
        let all_places = load_all_place_projections(&self.db, None)?;
        let mut faction_leaders = std::collections::BTreeSet::new();
        for fac in self.db.list_factions().unwrap_or_default() {
            for m in &fac.members {
                let rl = m.rank.to_lowercase();
                if rl.contains("leader")
                    || rl.contains("elder")
                    || rl.contains("master")
                    || rl.contains("captain")
                    || rl.contains("envoy")
                    || rl.contains("steward")
                {
                    faction_leaders.insert(m.character_id.clone());
                }
            }
        }

        let mut figures = Vec::new();
        let mut recent_engagements = Vec::new();
        for (cid, proj) in &all_chars {
            if proj.baseline.kind.eq_ignore_ascii_case("pc") {
                continue;
            }
            let prof = self.get_character_profile(cid)?;
            let inferred = Self::infer_npc_mobility_mode(proj, &prof, &faction_leaders, true);
            let is_prominent = prof.mobility.prominent || inferred.is_some();
            let effective_mode = inferred
                .as_ref()
                .map(|(m, _)| m.clone())
                .unwrap_or_else(|| prof.mobility.mobility_mode.clone());
            for eng in &prof.mobility.recent_engagements {
                recent_engagements.push(serde_json::json!({
                    "character_id": cid,
                    "name": proj.baseline.name,
                    "world_time": eng.world_time,
                    "place_id": eng.place_id,
                    "purpose": eng.purpose,
                    "counterparts": eng.counterparts,
                    "summary": eng.summary
                }));
            }
            figures.push(serde_json::json!({
                "character_id": cid,
                "name": proj.baseline.name,
                "title": proj.baseline.title,
                "archetype": proj.baseline.archetype,
                "faction": proj.baseline.faction,
                "alive_status": proj.current.alive_status,
                "current_location": proj.current.current_location,
                "prominent": is_prominent,
                "effective_mode": effective_mode,
                "mobility": prof.mobility
            }));
        }

        let places: Vec<serde_json::Value> = all_places
            .values()
            .map(|p| serde_json::json!({ "id": p.baseline.id, "name": p.baseline.name, "place_type": p.baseline.place_type }))
            .collect();

        Ok(serde_json::json!({
            "world_time": meta.clock.format_short(),
            "total_hours": meta.clock.total_hours(),
            "figures": figures,
            "recent_engagements": recent_engagements,
            "places": places
        }))
    }

    // -------------------------------------------------------------------------
    // Sparse Multi-Scale 2D (x, y) Map Grids & Coordinate Atlas
    // -------------------------------------------------------------------------

    pub fn upsert_map_grid(&self, mut grid: MapGrid) -> Result<(MapGrid, WorldEvent)> {
        if grid.name.trim().is_empty() {
            bail!("Map grid `name` cannot be empty");
        }
        grid.id = if grid.id.trim().is_empty() {
            slugify(&grid.name)
        } else {
            slugify(&grid.id)
        };
        grid.width = grid.width.clamp(4, 100);
        grid.height = grid.height.clamp(4, 100);
        if grid.cell_scale_label.trim().is_empty() {
            grid.cell_scale_label = "5 km".to_string();
        }
        if grid.default_terrain.trim().is_empty() {
            grid.default_terrain = "wilderness".to_string();
        }
        if let Some(ref p) = grid.parent_place_id {
            let t = p.trim();
            grid.parent_place_id = if t.is_empty() {
                None
            } else {
                Some(resolve_place_id(&self.db, t).unwrap_or_else(|_| slugify(t)))
            };
        }
        grid.updated_at_world_time = self.db.get_campaign_meta()?.clock.format_short();
        self.db.upsert_map_grid(&grid)?;
        let summary = format!(
            "Saved 2D map grid '{}' (`{}`) [{} • {}x{}, 1 cell = {}]",
            grid.name, grid.id, grid.scale_kind, grid.width, grid.height, grid.cell_scale_label
        );
        let ev = self.db.append_event(
            grid.parent_place_id.clone(),
            vec![grid.id.clone()],
            summary.clone(),
            EventPayload::NarrativeEvent {
                title: format!("Map Grid Updated: {}", grid.name),
                details: summary,
                tags: vec!["map_grid".into(), grid.scale_kind.clone()],
            },
        )?;
        self.maybe_compile()?;
        Ok((grid, ev))
    }

    pub fn list_map_grids_overview(&self) -> Result<serde_json::Value> {
        let grids = self.db.list_map_grids()?;
        let places: Vec<serde_json::Value> = load_all_place_projections(&self.db, None)?
            .values()
            .map(|p| {
                serde_json::json!({
                    "id": p.baseline.id,
                    "name": p.baseline.name,
                    "place_type": p.baseline.place_type,
                    "terrain": p.baseline.terrain,
                    "faction": p.current.controlling_faction
                })
            })
            .collect();
        let factions: Vec<serde_json::Value> = self
            .db
            .list_factions()
            .unwrap_or_default()
            .into_iter()
            .map(|f| serde_json::json!({ "id": f.id, "name": f.name }))
            .collect();
        Ok(serde_json::json!({
            "count": grids.len(),
            "grids": grids,
            "places": places,
            "factions": factions
        }))
    }

    pub fn get_map_grid_snapshot(
        &self,
        grid_id: &str,
        center: Option<(i32, i32)>,
        radius: Option<i32>,
        include_gm_secrets: bool,
    ) -> Result<serde_json::Value> {
        let gid = slugify(grid_id);
        let grid = self
            .db
            .get_map_grid(&gid)?
            .ok_or_else(|| anyhow!("Map grid '{gid}' not found"))?;
        let raw_cells = self.db.list_map_cells(&gid)?;

        let mut place_coords: std::collections::BTreeMap<String, (i32, i32)> =
            std::collections::BTreeMap::new();
        for c in &raw_cells {
            if let Some(ref pid) = c.place_id {
                place_coords.insert(pid.clone(), (c.x, c.y));
            }
        }

        let mut cells_out: Vec<MapGridCell> = raw_cells
            .iter()
            .filter(|c| {
                if let (Some((cx, cy)), Some(r)) = (center, radius) {
                    (c.x - cx).abs() <= r && (c.y - cy).abs() <= r
                } else {
                    true
                }
            })
            .filter(|c| include_gm_secrets || !c.fog_status.eq_ignore_ascii_case("unexplored"))
            .cloned()
            .collect();
        if !include_gm_secrets {
            for c in &mut cells_out {
                c.gm_notes.clear();
            }
        }

        let neighborhood_lines = if let Some((cx, cy)) = center {
            format_grid_neighborhood_lines(&grid, &raw_cells, cx, cy, include_gm_secrets)
        } else {
            vec![]
        };

        let all_places = load_all_place_projections(&self.db, None)?;
        let routes_all = self.all_world_routes_merged(&all_places);
        let grid_routes: Vec<serde_json::Value> = routes_all
            .iter()
            .filter_map(|r| {
                let (x1, y1) = place_coords.get(&r.from_place_id).copied()?;
                let (x2, y2) = place_coords.get(&r.to_place_id).copied()?;
                Some(serde_json::json!({
                    "id": r.id,
                    "from_place_id": r.from_place_id,
                    "to_place_id": r.to_place_id,
                    "x1": x1, "y1": y1,
                    "x2": x2, "y2": y2,
                    "status": r.status,
                    "distance_km": r.distance_km,
                    "terrain": r.terrain
                }))
            })
            .collect();

        // Map live character & traveling NPC pins onto cell coordinates
        let all_chars = load_all_character_projections(&self.db, None)?;
        let mut pins: Vec<serde_json::Value> = Vec::new();
        for (cid, cp) in &all_chars {
            let prof = self.get_character_profile(cid).ok();
            let in_transit_trip = prof.as_ref().and_then(|p| {
                if p.mobility.current_status == "in_transit" {
                    p.mobility.active_trip.as_ref()
                } else {
                    None
                }
            });
            if let Some(tr) = in_transit_trip {
                if let (Some(&(x1, y1)), Some(&(x2, y2))) = (
                    place_coords.get(&tr.origin_place_id),
                    place_coords.get(&tr.destination_place_id),
                ) {
                    pins.push(serde_json::json!({
                        "character_id": cid,
                        "name": cp.baseline.name,
                        "kind": cp.baseline.kind,
                        "status": "in_transit",
                        "x": (x1 as f64 + x2 as f64) / 2.0,
                        "y": (y1 as f64 + y2 as f64) / 2.0,
                        "origin": tr.origin_place_id,
                        "destination": tr.destination_place_id,
                        "purpose": tr.purpose,
                        "eta": tr.eta_world_time
                    }));
                    continue;
                }
            }
            if let Some(ref loc) = cp.current.current_location {
                if let Some(&(px, py)) = place_coords.get(loc) {
                    pins.push(serde_json::json!({
                        "character_id": cid,
                        "name": cp.baseline.name,
                        "kind": cp.baseline.kind,
                        "status": prof
                            .as_ref()
                            .map(|p| p.mobility.current_status.as_str())
                            .unwrap_or("present"),
                        "x": px as f64,
                        "y": py as f64,
                        "place_id": loc
                    }));
                }
            }
        }

        let placed_places: Vec<serde_json::Value> = place_coords
            .iter()
            .map(|(pid, (px, py))| {
                let pname = all_places
                    .get(pid)
                    .map(|p| p.baseline.name.clone())
                    .unwrap_or_else(|| pid.clone());
                serde_json::json!({ "id": pid, "name": pname, "x": px, "y": py })
            })
            .collect();

        Ok(serde_json::json!({
            "grid": grid,
            "cell_count": cells_out.len(),
            "cells": cells_out,
            "placed_places": placed_places,
            "routes": grid_routes,
            "character_pins": pins,
            "neighborhood_summary": neighborhood_lines
        }))
    }

    pub fn upsert_map_cells(
        &self,
        grid_id: &str,
        mut cells: Vec<MapGridCell>,
        reason: &str,
    ) -> Result<serde_json::Value> {
        let gid = slugify(grid_id);
        let grid = self
            .db
            .get_map_grid(&gid)?
            .ok_or_else(|| anyhow!("Map grid '{gid}' not found"))?;
        let now = self.db.get_campaign_meta()?.clock.format_short();
        let mut saved = Vec::new();
        let mut involved = vec![gid.clone()];

        for mut cell in cells.drain(..) {
            if cell.x < 0 || cell.y < 0 || cell.x >= grid.width as i32 || cell.y >= grid.height as i32 {
                bail!(
                    "Cell ({}, {}) out of bounds for grid '{}' ({}x{})",
                    cell.x,
                    cell.y,
                    gid,
                    grid.width,
                    grid.height
                );
            }
            cell.grid_id = gid.clone();
            if cell.terrain.trim().is_empty() {
                cell.terrain = grid.default_terrain.clone();
            }
            if cell.fog_status.trim().is_empty() {
                cell.fog_status = "charted".to_string();
            }
            if let Some(ref pid_raw) = cell.place_id {
                let t = pid_raw.trim();
                if t.is_empty() {
                    cell.place_id = None;
                } else {
                    let rpid = resolve_place_id(&self.db, t).unwrap_or_else(|_| slugify(t));
                    cell.place_id = Some(rpid.clone());
                    if cell.label.trim().is_empty() {
                        if let Ok(Some(pb)) = self.db.get_place_baseline(&rpid) {
                            cell.label = pb.name;
                        }
                    }
                    // Keep PlaceSpatialExt.coordinates & map_region synchronized!
                    let mut sp = self
                        .db
                        .get_place_spatial(&rpid)?
                        .unwrap_or_else(|| PlaceSpatialExt {
                            place_id: rpid.clone(),
                            ..Default::default()
                        });
                    sp.coordinates = Some([cell.x as f64, cell.y as f64]);
                    sp.map_region = gid.clone();
                    sp.updated_at_world_time = now.clone();
                    self.db.upsert_place_spatial(&sp)?;
                    if !involved.contains(&rpid) {
                        involved.push(rpid);
                    }
                }
            }
            if let Some(ref fac) = cell.faction_control {
                if fac.trim().is_empty() {
                    cell.faction_control = None;
                }
            }
            cell.updated_at_world_time = now.clone();
            self.db.upsert_map_cell(&cell)?;
            saved.push(cell);
        }

        let summary = format!(
            "Updated {} cell(s) on map grid '{}'{}",
            saved.len(),
            grid.name,
            if reason.is_empty() {
                String::new()
            } else {
                format!(" — {reason}")
            }
        );
        let ev = self.db.append_event(
            None,
            involved,
            summary.clone(),
            EventPayload::NarrativeEvent {
                title: format!("Map Grid Cells Updated: {}", grid.name),
                details: summary.clone(),
                tags: vec!["map_grid".into()],
            },
        )?;
        self.maybe_compile()?;
        Ok(serde_json::json!({
            "event_seq": ev.seq,
            "summary": summary,
            "updated_count": saved.len(),
            "cells": saved
        }))
    }

    pub fn clear_map_cell(&self, grid_id: &str, x: i32, y: i32) -> Result<serde_json::Value> {
        let gid = slugify(grid_id);
        let existed = self.db.delete_map_cell(&gid, x, y)?;
        let summary = format!("Cleared cell ({x}, {y}) on map grid '{gid}'");
        let ev = self.db.append_event(
            None,
            vec![gid.clone()],
            summary.clone(),
            EventPayload::NarrativeEvent {
                title: "Map Grid Cell Cleared".to_string(),
                details: summary.clone(),
                tags: vec!["map_grid".into()],
            },
        )?;
        self.maybe_compile()?;
        Ok(serde_json::json!({
            "event_seq": ev.seq,
            "cleared": existed,
            "summary": summary
        }))
    }

    /// Places existing campaign places onto a grid using existing PlaceSpatialExt coordinates or clean spacing.
    pub fn auto_layout_places_on_grid(&self, grid_id: &str) -> Result<serde_json::Value> {
        let gid = slugify(grid_id);
        let grid = self
            .db
            .get_map_grid(&gid)?
            .ok_or_else(|| anyhow!("Map grid '{gid}' not found"))?;
        let existing = self.db.list_map_cells(&gid)?;
        let already_placed: std::collections::BTreeSet<String> = existing
            .iter()
            .filter_map(|c| c.place_id.clone())
            .collect();
        let mut occupied: std::collections::BTreeSet<(i32, i32)> =
            existing.iter().map(|c| (c.x, c.y)).collect();

        let places = load_all_place_projections(&self.db, None)?;
        let mut to_save = Vec::new();
        let mut cursor = 0usize;
        let cols = ((grid.width as usize).saturating_sub(2) / 3).max(1);

        for (pid, pp) in &places {
            if already_placed.contains(pid) {
                continue;
            }
            // Prefer existing PlaceSpatialExt coordinates if inside grid bounds and free
            let mut chosen: Option<(i32, i32)> = None;
            if let Ok(Some(sp)) = self.db.get_place_spatial(pid) {
                if let Some([fx, fy]) = sp.coordinates {
                    let ix = (fx.round() as i32).clamp(0, (grid.width as i32) - 1);
                    let iy = (fy.round() as i32).clamp(0, (grid.height as i32) - 1);
                    if !occupied.contains(&(ix, iy)) {
                        chosen = Some((ix, iy));
                    }
                }
            }
            while chosen.is_none() {
                let gx = (1 + (cursor % cols) * 3) as i32;
                let gy = (1 + (cursor / cols) * 3) as i32;
                cursor += 1;
                if gx < grid.width as i32 && gy < grid.height as i32 && !occupied.contains(&(gx, gy))
                {
                    chosen = Some((gx, gy));
                } else if cursor > (grid.width * grid.height) as usize {
                    // Fallback scan any free cell
                    'scan: for y in 0..grid.height as i32 {
                        for x in 0..grid.width as i32 {
                            if !occupied.contains(&(x, y)) {
                                chosen = Some((x, y));
                                break 'scan;
                            }
                        }
                    }
                    break;
                }
            }
            if let Some((x, y)) = chosen {
                occupied.insert((x, y));
                let t = pp.baseline.terrain.trim().to_lowercase();
                let terrain = if t.is_empty() {
                    if pp.baseline.place_type.to_lowercase().contains("settlement")
                        || pp.baseline.place_type.to_lowercase().contains("city")
                        || pp.baseline.place_type.to_lowercase().contains("port")
                    {
                        "urban".to_string()
                    } else {
                        grid.default_terrain.clone()
                    }
                } else {
                    t
                };
                to_save.push(MapGridCell {
                    grid_id: gid.clone(),
                    x,
                    y,
                    terrain,
                    elevation: 0,
                    label: pp.baseline.name.clone(),
                    place_id: Some(pid.clone()),
                    landmark_name: String::new(),
                    faction_control: pp.current.controlling_faction.clone(),
                    fog_status: "charted".to_string(),
                    passable: true,
                    hazards: vec![],
                    notes: pp.baseline.place_type.clone(),
                    gm_notes: String::new(),
                    updated_at_world_time: String::new(),
                });
            }
        }

        if to_save.is_empty() {
            return Ok(serde_json::json!({
                "placed_count": 0,
                "summary": "All campaign places are already placed on this grid."
            }));
        }
        self.upsert_map_cells(&gid, to_save, "Auto-placed existing campaign places")
    }

    // -------------------------------------------------------------------------
    // Active Engine Signal Bus, Declarative Stage Rules, Auto-Clocks & Routines
    // -------------------------------------------------------------------------

    pub fn emit_signal(
        &self,
        sig: crate::signals::EngineSignal,
    ) -> Result<crate::signals::EngineSignal> {
        let now_str = self
            .db
            .get_campaign_meta()
            .map(|m| m.clock.format_short())
            .unwrap_or_default();
        let norm = sig.normalized(&now_str);
        self.db.upsert_signal(&norm)?;
        Ok(norm)
    }

    pub fn list_active_signals(
        &self,
        perspective: Option<&str>,
        place_id: Option<&str>,
        character_ids: &[String],
        include_gm_secrets: bool,
    ) -> Result<Vec<crate::signals::EngineSignal>> {
        let meta = self.db.get_campaign_meta()?;
        let all_chars = load_all_character_projections(&self.db, None)?;
        let all_places = load_all_place_projections(&self.db, None)?;
        let resolved_place = place_id
            .filter(|s| !s.trim().is_empty())
            .map(|p| resolve_place_id(&self.db, p).unwrap_or_else(|_| slugify(p)));

        let focus_set_opt: Option<std::collections::BTreeSet<String>> =
            if resolved_place.is_none() && character_ids.is_empty() {
                None
            } else {
                let mut set = std::collections::BTreeSet::new();
                if let Some(ref pid) = resolved_place {
                    set.insert(pid.clone());
                    if let Some(pp) = all_places.get(pid) {
                        for cid in &pp.current.present_characters {
                            set.insert(cid.clone());
                        }
                    }
                }
                for raw_cid in character_ids {
                    if !raw_cid.trim().is_empty() {
                        let cid = resolve_character_id(&self.db, raw_cid)
                            .unwrap_or_else(|_| slugify(raw_cid));
                        set.insert(cid);
                    }
                }
                Some(set)
            };

        let raw_persp = perspective
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("gm");
        let is_gm = raw_persp.eq_ignore_ascii_case("gm");
        let pov_actor: Option<String> = if is_gm || raw_persp.eq_ignore_ascii_case("player") {
            None
        } else {
            Some(resolve_character_id(&self.db, raw_persp).unwrap_or_else(|_| slugify(raw_persp)))
        };
        let pc_ids: std::collections::BTreeSet<String> = all_chars
            .values()
            .filter(|c| c.baseline.kind.eq_ignore_ascii_case("pc"))
            .map(|c| c.baseline.id.clone())
            .collect();

        let signals = crate::signals::evaluate_live_signals(
            &self.db,
            &all_chars,
            &all_places,
            focus_set_opt.as_ref(),
            resolved_place.as_deref(),
            &meta.clock.format_short(),
        )?;
        Ok(crate::signals::filter_signals_for_perspective(
            signals,
            raw_persp,
            pov_actor.as_deref(),
            &pc_ids,
            is_gm && include_gm_secrets,
        ))
    }

    pub fn update_signal(
        &self,
        id_or_dedup: &str,
        action_or_status: &str,
    ) -> Result<Option<crate::signals::EngineSignal>> {
        let act = action_or_status.trim().to_ascii_lowercase();
        if act == "delete" || act == "clear" {
            let _ = self.db.delete_signal(id_or_dedup)?;
            return Ok(None);
        }
        let new_status = match act.as_str() {
            "ack" | "acknowledge" | "acknowledged" => "acknowledged",
            "resolve" | "resolved" | "dismiss" => "resolved",
            "reopen" | "active" => "active",
            other => bail!("Unknown signal status/action '{other}'. Expected acknowledge, resolve, active, or delete."),
        };
        let now_str = self
            .db
            .get_campaign_meta()
            .map(|m| m.clock.format_short())
            .unwrap_or_default();
        if let Some(updated) = self.db.update_signal_status(id_or_dedup, new_status, &now_str)? {
            return Ok(Some(updated));
        }
        // Materialize live-derived signal so acknowledging/resolving suppresses it cleanly!
        if let Ok(live) = self.list_active_signals(Some("gm"), None, &[], true) {
            if let Some(mut found) = live
                .into_iter()
                .find(|s| s.id == id_or_dedup || s.dedup_key == id_or_dedup)
            {
                found.status = new_status.to_string();
                found.updated_at_world_time = now_str;
                self.db.upsert_signal(&found)?;
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    pub fn upsert_routine(
        &self,
        mut routine: crate::routines::CharacterRoutine,
    ) -> Result<(crate::routines::CharacterRoutine, WorldEvent)> {
        let cid = resolve_character_id(&self.db, &routine.character_id)?;
        routine.character_id = cid.clone();
        if routine.name.trim().is_empty() {
            bail!("Routine `name` cannot be empty");
        }
        if routine.id.trim().is_empty() {
            routine.id = slugify(&format!("{cid}-{}", routine.name));
        } else {
            routine.id = slugify(&routine.id);
        }
        routine.cadence_hours = routine.cadence_hours.max(1);
        if let Some(ref req_loc) = routine.require_location_id {
            let t = req_loc.trim();
            routine.require_location_id = if t.is_empty() {
                None
            } else {
                Some(resolve_place_id(&self.db, t).unwrap_or_else(|_| slugify(t)))
            };
        }
        routine.min_currency = routine
            .min_currency
            .into_iter()
            .map(|(k, v)| (normalize_currency_key(&k), v))
            .collect();
        routine.currency_deltas = routine
            .currency_deltas
            .into_iter()
            .map(|(k, v)| (normalize_currency_key(&k), v))
            .collect();

        let meta = self.db.get_campaign_meta()?;
        if routine.last_tick_total_hours.is_none() {
            if let Some(existing) = self.db.get_routine(&routine.id)? {
                routine.last_tick_total_hours = existing.last_tick_total_hours;
            }
            if routine.last_tick_total_hours.is_none() {
                routine.last_tick_total_hours = Some(meta.clock.total_hours());
            }
        }
        routine.updated_at_world_time = meta.clock.format_short();
        self.db.upsert_routine(&routine)?;

        let proj = load_character_projection(&self.db, &cid, None)?;
        let summary = format!(
            "Configured standing routine '{}' [{} | every {}h | {}] for {}",
            routine.name,
            routine.category,
            routine.cadence_hours,
            if routine.enabled { "enabled" } else { "disabled" },
            proj.baseline.name
        );
        let ev = self.db.append_event(
            proj.current.current_location,
            vec![cid, routine.id.clone()],
            summary.clone(),
            EventPayload::NarrativeEvent {
                title: format!("Standing Routine Saved: {}", routine.name),
                details: summary,
                tags: vec!["routine".into(), routine.category.clone()],
            },
        )?;
        self.maybe_compile()?;
        Ok((routine, ev))
    }

    pub fn list_routines(
        &self,
        character_id: Option<&str>,
    ) -> Result<Vec<crate::routines::CharacterRoutine>> {
        let resolved = match character_id.filter(|s| !s.trim().is_empty()) {
            Some(raw) => Some(resolve_character_id(&self.db, raw).unwrap_or_else(|_| slugify(raw))),
            None => None,
        };
        self.db.list_routines(resolved.as_deref())
    }

    pub fn delete_routine(&self, routine_id: &str) -> Result<bool> {
        let rid = slugify(routine_id);
        let removed = self.db.delete_routine(&rid)?;
        let _ = self.db.delete_signal(&format!("routine_paused:{rid}"));
        Ok(removed)
    }

    pub(crate) fn tick_auto_clocks(
        &self,
        prev_total_hours: i64,
        new_total_hours: i64,
        now_str: &str,
    ) -> Result<Vec<String>> {
        let mut digests = Vec::new();
        for mut clock in self.db.list_clocks()? {
            if !clock.status.eq_ignore_ascii_case("active") {
                continue;
            }
            let Some(cadence) = clock.cadence_hours.filter(|&h| h > 0) else {
                continue;
            };
            let anchor = clock.last_tick_total_hours.unwrap_or(prev_total_hours);
            if new_total_hours <= anchor {
                continue;
            }
            let elapsed = (new_total_hours - anchor) as u64;
            let cycles = elapsed / cadence;
            if cycles == 0 {
                if clock.last_tick_total_hours.is_none() {
                    clock.last_tick_total_hours = Some(anchor);
                    self.db.upsert_clock(&clock)?;
                }
                continue;
            }

            clock.last_tick_total_hours = Some(anchor + (cycles * cadence) as i64);
            let raw_filled = clock.filled + (cycles as i64);
            let mut filled_triggered = false;
            if raw_filled >= clock.segments {
                filled_triggered = true;
                if clock.auto_reset {
                    clock.filled = raw_filled % clock.segments.max(1);
                    clock.status = "active".to_string();
                } else {
                    clock.filled = clock.segments;
                    clock.status = "completed".to_string();
                }
            } else {
                clock.filled = raw_filled;
            }
            self.db.upsert_clock(&clock)?;

            let summary = format!(
                "Auto-ticked clock '{}' (+{} segment(s)) -> {}/{} [{}]{}",
                clock.name,
                cycles,
                clock.filled,
                clock.segments,
                clock.status,
                if filled_triggered && clock.auto_reset {
                    " [FILLED & AUTO-RESET]"
                } else if filled_triggered {
                    " [COMPLETED!]"
                } else {
                    ""
                }
            );
            let ev = self.db.append_event(
                None,
                clock.related_ids.clone(),
                summary.clone(),
                EventPayload::ClockTicked {
                    clock_id: clock.id.clone(),
                    delta: cycles as i64,
                    set_status: Some(clock.status.clone()),
                    reason: format!("Automatic {cadence}h time-cadence tick"),
                },
            )?;
            if filled_triggered {
                let _ = self.emit_signal(crate::signals::EngineSignal {
                    id: String::new(),
                    dedup_key: format!("clock_filled:{}:{}", clock.id, ev.seq),
                    code: "CLOCK_FILLED".to_string(),
                    severity: "critical".to_string(),
                    category: "clock".to_string(),
                    title: format!("Clock Filled: '{}' ({}/{})", clock.name, clock.segments, clock.segments),
                    summary: format!(
                        "Timed clock '{}' reached completion at {now_str}! {}",
                        clock.name,
                        if clock.consequence_on_fill.is_empty() {
                            clock.description.as_str()
                        } else {
                            clock.consequence_on_fill.as_str()
                        }
                    ),
                    entity_ids: clock.related_ids.clone(),
                    location_id: None,
                    suggested_actions: vec![format!("Enact consequence for '{}' in scene.", clock.name)],
                    visibility: "gm_secret".to_string(),
                    status: "active".to_string(),
                    source_event_seq: Some(ev.seq),
                    created_at_world_time: now_str.to_string(),
                    updated_at_world_time: now_str.to_string(),
                });
            }
            digests.push(summary);
        }
        Ok(digests)
    }

    pub(crate) fn tick_character_routines(
        &self,
        prev_total_hours: i64,
        new_total_hours: i64,
        now_str: &str,
    ) -> Result<Vec<String>> {
        let mut digests = Vec::new();
        let routines = self.db.list_routines(None)?;
        if routines.is_empty() {
            return Ok(digests);
        }

        for mut r in routines {
            if !r.enabled {
                continue;
            }
            let cadence = r.cadence_hours.max(1);
            let anchor = r.last_tick_total_hours.unwrap_or(prev_total_hours);
            if new_total_hours <= anchor {
                continue;
            }
            let elapsed = (new_total_hours - anchor) as u64;
            let cycles = elapsed / cadence;
            if cycles == 0 {
                if r.last_tick_total_hours.is_none() {
                    r.last_tick_total_hours = Some(anchor);
                    self.db.upsert_routine(&r)?;
                }
                continue;
            }

            let Ok(proj) = load_character_projection(&self.db, &r.character_id, None) else {
                continue;
            };
            let cid = proj.baseline.id.clone();

            // 1. Evaluate guard conditions
            let mut guard_fail: Option<String> = None;
            if let Some(ref req_loc) = r.require_location_id {
                if proj.current.current_location.as_deref() != Some(req_loc.as_str()) {
                    guard_fail = Some(format!(
                        "{} is at '{}' instead of required '{}'",
                        proj.baseline.name,
                        proj.current.current_location.as_deref().unwrap_or("unknown"),
                        req_loc
                    ));
                }
            }
            if guard_fail.is_none() {
                if let Some(ref min_inj) = r.halt_on_injury_severity {
                    let thresh = crate::routines::injury_severity_rank(min_inj);
                    if thresh > 0 {
                        let active_inj = self.db.list_injuries(Some(&cid), false).unwrap_or_default();
                        if let Some(hit) = active_inj.iter().find(|i| {
                            !i.status.eq_ignore_ascii_case("healed")
                                && crate::routines::injury_severity_rank(&i.severity) >= thresh
                        }) {
                            guard_fail = Some(format!(
                                "Halted due to {} injury '{}'",
                                hit.severity, hit.name
                            ));
                        }
                    }
                }
            }
            if guard_fail.is_none() && !r.halt_if_conditions.is_empty() {
                for hc in &r.halt_if_conditions {
                    let hcl = hc.trim().to_lowercase();
                    if !hcl.is_empty() {
                        if let Some(c) = proj
                            .current
                            .conditions
                            .iter()
                            .find(|c| c.name.to_lowercase().contains(&hcl))
                        {
                            guard_fail = Some(format!("Halted due to active condition '{}'", c.name));
                            break;
                        }
                    }
                }
            }
            if guard_fail.is_none() {
                for (curr, min_amt) in &r.min_currency {
                    let cur_key = normalize_currency_key(curr);
                    let mut bal = self.db.get_currency_balance(&cid, &cur_key)?;
                    if bal < *min_amt {
                        if self
                            .try_make_currency_change(&cid, &cur_key, *min_amt, now_str)?
                            .is_some()
                        {
                            bal = self.db.get_currency_balance(&cid, &cur_key)?;
                        }
                    }
                    if bal < *min_amt {
                        guard_fail = Some(format!(
                            "Insufficient {cur_key} reserve (has {bal}, requires {min_amt})"
                        ));
                        break;
                    }
                }
            }

            // Check negative currency_deltas affordability across cycles
            let mut eff_cycles = cycles;
            if guard_fail.is_none() {
                for (curr, delta) in &r.currency_deltas {
                    if *delta < 0 {
                        let cost_per = delta.unsigned_abs() as i64;
                        let total_cost = cost_per.saturating_mul(eff_cycles as i64);
                        let cur_key = normalize_currency_key(curr);
                        let mut bal = self.db.get_currency_balance(&cid, &cur_key)?;
                        if bal < total_cost {
                            let _ = self.try_make_currency_change(&cid, &cur_key, total_cost, now_str);
                            bal = self.db.get_currency_balance(&cid, &cur_key)?;
                        }
                        let affordable = (bal.max(0) / cost_per.max(1)) as u64;
                        if affordable == 0 {
                            guard_fail = Some(format!(
                                "Insufficient {cur_key} to pay routine cost ({cost_per} {cur_key}/cycle; balance {bal})"
                            ));
                            break;
                        }
                        eff_cycles = eff_cycles.min(affordable);
                    }
                }
            }

            if let Some(reason_msg) = guard_fail {
                r.paused_reason = reason_msg.clone();
                r.last_tick_total_hours = Some(anchor + (cycles * cadence) as i64);
                r.updated_at_world_time = now_str.to_string();
                self.db.upsert_routine(&r)?;
                let _ = self.emit_signal(crate::signals::EngineSignal {
                    id: String::new(),
                    dedup_key: format!("routine_paused:{}", r.id),
                    code: "ROUTINE_PAUSED".to_string(),
                    severity: "warning".to_string(),
                    category: "routine".to_string(),
                    title: format!("Routine Paused: '{}' ({})", r.name, proj.baseline.name),
                    summary: reason_msg,
                    entity_ids: vec![cid],
                    location_id: proj.current.current_location,
                    suggested_actions: vec!["Address the blocking condition/location/funds to resume routine ticks.".to_string()],
                    visibility: "player".to_string(),
                    status: "active".to_string(),
                    source_event_seq: None,
                    created_at_world_time: now_str.to_string(),
                    updated_at_world_time: now_str.to_string(),
                });
                continue;
            }

            // Clear any prior paused notice
            if !r.paused_reason.is_empty() {
                r.paused_reason.clear();
                let _ = self.db.delete_signal(&format!("routine_paused:{}", r.id));
            }

            let mut applied_parts: Vec<String> = Vec::new();
            let reason_tag = format!("Routine '{}' ({} cycle(s))", r.name, eff_cycles);

            // Apply currency deltas
            for (curr, delta) in &r.currency_deltas {
                if *delta == 0 {
                    continue;
                }
                let total_d = delta.saturating_mul(eff_cycles as i64);
                let cur_key = normalize_currency_key(curr);
                if total_d > 0 {
                    if let Some(ref cp_owner) = r.counterparty_id {
                        let cpo = slugify(cp_owner);
                        let cp_bal = self.db.get_currency_balance(&cpo, &cur_key)?;
                        if cp_bal >= total_d {
                            let _ = self.transfer_currency(&cpo, &cid, total_d, &cur_key, &reason_tag);
                        } else {
                            let cur_bal = self.db.get_currency_balance(&cid, &cur_key)?;
                            self.db
                                .set_currency_balance(&cid, &cur_key, cur_bal + total_d, now_str)?;
                        }
                    } else {
                        let cur_bal = self.db.get_currency_balance(&cid, &cur_key)?;
                        self.db
                            .set_currency_balance(&cid, &cur_key, cur_bal + total_d, now_str)?;
                    }
                    applied_parts.push(format!("+{total_d} {cur_key}"));
                } else {
                    let cost = total_d.unsigned_abs() as i64;
                    let mut cur_bal = self.db.get_currency_balance(&cid, &cur_key)?;
                    if cur_bal < cost {
                        let _ = self.try_make_currency_change(&cid, &cur_key, cost, now_str);
                        cur_bal = self.db.get_currency_balance(&cid, &cur_key)?;
                    }
                    let new_bal = (cur_bal - cost).max(0);
                    self.db.set_currency_balance(&cid, &cur_key, new_bal, now_str)?;
                    if let Some(ref cp_owner) = r.counterparty_id {
                        let cpo = slugify(cp_owner);
                        let cpb = self.db.get_currency_balance(&cpo, &cur_key)?;
                        self.db.set_currency_balance(&cpo, &cur_key, cpb + cost, now_str)?;
                    }
                    applied_parts.push(format!("-{cost} {cur_key}"));
                }
            }

            // Apply resource deltas
            for (res_name, delta) in &r.resource_deltas {
                if *delta != 0 {
                    let total_d = delta.saturating_mul(eff_cycles as i64);
                    let _ = self.modify_resource(&cid, res_name, Some(total_d), None, None, None, &reason_tag);
                    applied_parts.push(format!("{res_name} {total_d:+}"));
                }
            }

            // Apply progression XP
            for (track_id, xp_per) in &r.progression_xp {
                let total_xp = xp_per.saturating_mul(eff_cycles);
                if total_xp > 0 {
                    if let Ok((resp, _)) = self.advance_progression_xp(&cid, track_id, total_xp, &reason_tag) {
                        let stg = resp["track"]["sub_stage_label"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .or_else(|| resp["track"]["major_stage_name"].as_str())
                            .unwrap_or(track_id);
                        applied_parts.push(format!("{track_id} +{total_xp} XP ({stg})"));
                    }
                }
            }

            // Apply skill XP
            for (sk_name, xp_per) in &r.skill_xp {
                let total_xp = xp_per.saturating_mul(eff_cycles);
                if total_xp > 0 {
                    if let Ok((prog, _)) = self.grant_skill_xp(&cid, sk_name, total_xp, None, &reason_tag) {
                        applied_parts.push(format!("{} +{total_xp} XP ({})", prog.skill_name, prog.new_rank));
                    }
                }
            }

            // Apply job XP
            for (job_id, xp_per) in &r.job_xp {
                let total_xp = xp_per.saturating_mul(eff_cycles);
                if total_xp > 0 {
                    let _ = self.manage_player_character_action(
                        Some(&cid),
                        "adjust_job_xp",
                        &serde_json::json!({
                            "job_id": job_id,
                            "delta_xp": total_xp as i64,
                            "reason": reason_tag
                        }),
                    );
                    applied_parts.push(format!("job:{job_id} +{total_xp} XP"));
                }
            }

            // Apply stat training XP
            for (stat_name, xp_per) in &r.stat_training_xp {
                let total_xp = xp_per.saturating_mul(eff_cycles);
                if total_xp > 0 {
                    let _ = self.manage_player_character_action(
                        Some(&cid),
                        "adjust_stat_xp",
                        &serde_json::json!({
                            "stat": stat_name,
                            "delta_xp": total_xp as i64,
                            "reason": reason_tag
                        }),
                    );
                    applied_parts.push(format!("stat:{stat_name} +{total_xp} XP"));
                }
            }

            r.last_tick_total_hours = Some(anchor + (cycles * cadence) as i64);
            r.updated_at_world_time = now_str.to_string();
            self.db.upsert_routine(&r)?;

            if !applied_parts.is_empty() {
                let digest = format!(
                    "{} — Routine '{}' x{} cycle(s): {}",
                    proj.baseline.name,
                    r.name,
                    eff_cycles,
                    applied_parts.join(", ")
                );
                let ev = self.db.append_event(
                    proj.current.current_location.clone(),
                    vec![cid.clone(), r.id.clone()],
                    digest.clone(),
                    EventPayload::NarrativeEvent {
                        title: format!("Routine Completed: {}", r.name),
                        details: digest.clone(),
                        tags: vec!["routine".into(), r.category.clone()],
                    },
                )?;
                let _ = self.emit_signal(crate::signals::EngineSignal {
                    id: String::new(),
                    dedup_key: format!("routine_digest:{}", r.id),
                    code: "ROUTINE_DIGEST".to_string(),
                    severity: "info".to_string(),
                    category: "routine".to_string(),
                    title: format!("{} — '{}' ({} cycle(s))", proj.baseline.name, r.name, eff_cycles),
                    summary: applied_parts.join(", "),
                    entity_ids: vec![cid],
                    location_id: proj.current.current_location,
                    suggested_actions: vec![],
                    visibility: "player".to_string(),
                    status: "active".to_string(),
                    source_event_seq: Some(ev.seq),
                    created_at_world_time: now_str.to_string(),
                    updated_at_world_time: now_str.to_string(),
                });
                digests.push(digest);
            }
        }
        Ok(digests)
    }

    pub(crate) fn lookup_stage_rule(
        track: &CharacterProgressionTrack,
        ladder: Option<&ProgressionLadder>,
        major_idx: u32,
        sub_stage: u32,
    ) -> Option<crate::routines::StageTransitionRule> {
        let composite_key = format!("{major_idx}:{sub_stage}");
        if let Some(r) = track.stage_rules.get(&composite_key) {
            return Some(r.clone());
        }
        if major_idx == 0 {
            if let Some(r) = track.stage_rules.get(&sub_stage.to_string()) {
                return Some(r.clone());
            }
        }
        // Fallback: if track has a single major stage or keyed by 1-based stage name/number
        if let Some(r) = track.stage_rules.get(&sub_stage.to_string()) {
            if ladder.map_or(true, |l| l.stages.len() <= 1) {
                return Some(r.clone());
            }
        }
        if let Some(lad) = ladder {
            if let Some(st) = lad.stages.get(major_idx as usize) {
                if let Some(r) = st.sub_stage_rules.get(&sub_stage) {
                    return Some(r.clone());
                }
                if sub_stage == 1 {
                    if let Some(ref r) = st.stage_rule {
                        return Some(r.clone());
                    }
                }
            }
        }
        None
    }

    pub fn apply_stage_transition_rule(
        &self,
        character_id: &str,
        track_name: &str,
        stage_label: &str,
        rule: &crate::routines::StageTransitionRule,
        source_event_seq: Option<i64>,
    ) -> Result<serde_json::Value> {
        let cid = resolve_character_id(&self.db, character_id)?;
        let now_str = self.db.get_campaign_meta()?.clock.format_short();
        let proj = load_character_projection(&self.db, &cid, None)?;
        let mut prof = self.get_character_profile(&cid)?;
        let mut changes: Vec<String> = Vec::new();
        let mut avatar_changed = false;

        // Preserve Day-1 baseline measurements/height/weight automatically before applying first transformation!
        if !rule.height.trim().is_empty() {
            if prof.avatar.baseline_height.trim().is_empty() && !prof.avatar.height.trim().is_empty() {
                prof.avatar.baseline_height = prof.avatar.height.clone();
            }
            prof.avatar.height = rule.height.trim().to_string();
            avatar_changed = true;
            changes.push(format!("Height -> {}", prof.avatar.height));
        }
        if !rule.weight.trim().is_empty() {
            if prof.avatar.baseline_weight.trim().is_empty() && !prof.avatar.weight.trim().is_empty() {
                prof.avatar.baseline_weight = prof.avatar.weight.clone();
            }
            prof.avatar.weight = rule.weight.trim().to_string();
            avatar_changed = true;
            changes.push(format!("Weight -> {}", prof.avatar.weight));
        }
        if !rule.measurements.is_empty() {
            if prof.avatar.baseline_measurements.is_empty() && !prof.avatar.measurements.is_empty() {
                prof.avatar.baseline_measurements = prof.avatar.measurements.clone();
            }
            let mut m_list = Vec::new();
            for (k, v) in &rule.measurements {
                let nk = crate::autonomy::normalize_measurement_key(k);
                if !nk.is_empty() && !v.trim().is_empty() {
                    prof.avatar.measurements.insert(nk.clone(), v.trim().to_string());
                    m_list.push(format!("{nk} {}", v.trim()));
                }
            }
            if !m_list.is_empty() {
                avatar_changed = true;
                changes.push(format!("Measurements: {}", m_list.join(", ")));
            }
        }

        if !rule.appearance_stage.trim().is_empty() {
            prof.avatar.appearance_stage = rule.appearance_stage.trim().to_string();
            avatar_changed = true;
        }
        if !rule.avatar_summary.trim().is_empty() {
            prof.avatar.avatar_summary = rule.avatar_summary.trim().to_string();
            avatar_changed = true;
        }
        if !rule.body_type_or_build.trim().is_empty() {
            prof.avatar.body_type_or_build = rule.body_type_or_build.trim().to_string();
            avatar_changed = true;
        }
        if !rule.silhouette.trim().is_empty() {
            prof.avatar.silhouette = rule.silhouette.trim().to_string();
            avatar_changed = true;
        }
        if !rule.posture_and_gait.trim().is_empty() {
            prof.avatar.posture_and_gait = rule.posture_and_gait.trim().to_string();
            avatar_changed = true;
        }
        if !rule.proportions_summary.trim().is_empty() {
            prof.avatar.proportions_summary = rule.proportions_summary.trim().to_string();
            avatar_changed = true;
        }
        if !rule.clothing_fit.trim().is_empty() {
            prof.avatar.clothing_fit = rule.clothing_fit.trim().to_string();
            avatar_changed = true;
        }
        if !rule.face_shape_and_features.trim().is_empty() {
            prof.avatar.face_shape_and_features = rule.face_shape_and_features.trim().to_string();
            avatar_changed = true;
        }
        if !rule.skin_tone_and_texture.trim().is_empty() {
            prof.avatar.skin_tone_and_texture = rule.skin_tone_and_texture.trim().to_string();
            avatar_changed = true;
        }
        if !rule.hair.trim().is_empty() {
            prof.avatar.hair = rule.hair.trim().to_string();
            avatar_changed = true;
        }
        if let Some(ref fh) = rule.facial_hair {
            prof.avatar.facial_hair = fh.trim().to_string();
            avatar_changed = true;
            changes.push(if prof.avatar.facial_hair.is_empty() {
                "Facial hair cleared".to_string()
            } else {
                format!("Facial hair -> {}", prof.avatar.facial_hair)
            });
        }
        if !rule.voice_and_speech.trim().is_empty() {
            prof.avatar.voice_and_speech = rule.voice_and_speech.trim().to_string();
            avatar_changed = true;
        }
        if !rule.perceived_change_note.trim().is_empty() {
            prof.avatar.perceived_change_note = rule.perceived_change_note.trim().to_string();
            avatar_changed = true;
        }
        if !rule.next_transformation_trajectory.trim().is_empty() {
            prof.avatar.transformation_trajectory = rule.next_transformation_trajectory.trim().to_string();
        }

        if rule.unspent_stat_points_delta != 0 {
            prof.unspent_stat_points = (prof.unspent_stat_points + rule.unspent_stat_points_delta).max(0);
            changes.push(format!(
                "Unspent stat points {:+} (now {})",
                rule.unspent_stat_points_delta, prof.unspent_stat_points
            ));
        }

        for gt in &rule.grant_traits {
            let tid = if gt.id.trim().is_empty() {
                slugify(&gt.name)
            } else {
                slugify(&gt.id)
            };
            let tr_obj = CharacterTrait {
                id: tid.clone(),
                name: gt.name.clone(),
                category: gt.category.clone(),
                rank_or_grade: gt.rank_or_grade.clone(),
                active: gt.active,
                effect: gt.effect.clone(),
                notes: gt.notes.clone(),
            };
            if let Some(ex) = prof
                .traits_and_perks
                .iter_mut()
                .find(|x| x.id == tid || x.name.eq_ignore_ascii_case(&gt.name))
            {
                *ex = tr_obj;
            } else {
                prof.traits_and_perks.push(tr_obj);
            }
            changes.push(format!("Trait granted: {}", gt.name));
        }

        if avatar_changed || !rule.checkpoint_summary.trim().is_empty() {
            let cp_stage = if !rule.appearance_stage.trim().is_empty() {
                rule.appearance_stage.trim().to_string()
            } else {
                format!("{track_name} — {stage_label}")
            };
            let cp = crate::autonomy::AppearanceCheckpoint {
                stage: cp_stage.clone(),
                world_time: now_str.clone(),
                summary: if !rule.checkpoint_summary.trim().is_empty() {
                    rule.checkpoint_summary.trim().to_string()
                } else {
                    changes.join("; ")
                },
                height: prof.avatar.height.clone(),
                weight: prof.avatar.weight.clone(),
                measurements: prof.avatar.measurements.clone(),
            };
            if let Some(ex) = prof
                .avatar
                .transformation_log
                .iter_mut()
                .find(|x| x.stage.eq_ignore_ascii_case(&cp_stage))
            {
                *ex = cp;
            } else {
                prof.avatar.transformation_log.push(cp);
            }
            if !prof.avatar.height.is_empty() || !prof.avatar.body_type_or_build.is_empty() {
                prof.height_and_build = [
                    prof.avatar.height.as_str(),
                    prof.avatar.weight.as_str(),
                    prof.avatar.body_type_or_build.as_str(),
                ]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" • ");
            }
        }

        prof.updated_at_world_time = now_str.clone();
        self.db.upsert_character_profile(&prof)?;

        // Stat overrides & deltas
        let reason_note = format!("Stage rule: {track_name} ({stage_label})");
        for (st_k, st_v) in &rule.stat_set {
            let _ = self.modify_stat(&cid, st_k, None, Some(*st_v), &reason_note)?;
            changes.push(format!("{st_k} = {st_v}"));
        }
        for (st_k, st_d) in &rule.stat_deltas {
            if *st_d != 0 {
                let _ = self.modify_stat(&cid, st_k, Some(*st_d), None, &reason_note)?;
                changes.push(format!("{st_k} {st_d:+}"));
            }
        }

        // Remove & upsert conditions
        for rc in &rule.remove_conditions {
            if !rc.trim().is_empty() {
                let _ = self.remove_condition(&cid, rc.trim(), &reason_note);
                changes.push(format!("Cleared condition '{rc}'"));
            }
        }
        for uc in &rule.upsert_conditions {
            if !uc.name.trim().is_empty() {
                let _ = self.add_condition(
                    &cid,
                    StatusCondition {
                        name: uc.name.clone(),
                        kind: uc.kind.clone(),
                        severity: uc.severity.clone(),
                        duration: uc.duration.clone(),
                        source: if uc.source.is_empty() {
                            format!("{track_name} ({stage_label})")
                        } else {
                            uc.source.clone()
                        },
                        notes: uc.notes.clone(),
                    },
                )?;
                changes.push(format!("Condition: {} ({})", uc.name, uc.severity));
            }
        }

        // Re-evaluate worn garment fit & discomfort immediately against the updated body!
        let _ = self.sync_apparel_discomfort(&cid, 0);

        // Clear any stale manual sync alarm on this character
        for s in self.db.list_signals(Some("active")).unwrap_or_default() {
            if s.code == "APPEARANCE_SYNC_DUE" && s.entity_ids.contains(&cid) {
                let _ = self.db.update_signal_status(&s.id, "resolved", &now_str);
            }
        }

        let mut sig_summary = if changes.is_empty() {
            format!("Applied stage rule for {} at {stage_label}.", proj.baseline.name)
        } else {
            format!("Auto-applied: {}.", changes.join(" | "))
        };
        if !rule.narrative_directive.trim().is_empty() {
            sig_summary.push_str(&format!(" Directive: {}", rule.narrative_directive.trim()));
        }

        let emitted = self.emit_signal(crate::signals::EngineSignal {
            id: String::new(),
            dedup_key: format!("stage_rule_applied:{cid}:{}:{}", slugify(track_name), slugify(stage_label)),
            code: "STAGE_RULE_APPLIED".to_string(),
            severity: "critical".to_string(),
            category: "progression".to_string(),
            title: format!("{} — {} Transition Applied ({stage_label})", proj.baseline.name, track_name),
            summary: sig_summary.clone(),
            entity_ids: vec![cid.clone()],
            location_id: proj.current.current_location,
            suggested_actions: vec![],
            visibility: rule.signal_visibility.clone(),
            status: "active".to_string(),
            source_event_seq,
            created_at_world_time: now_str.clone(),
            updated_at_world_time: now_str,
        })?;

        Ok(serde_json::json!({
            "stage": stage_label,
            "changes": changes,
            "narrative_directive": rule.narrative_directive,
            "signal_id": emitted.id
        }))
    }

    pub(crate) fn award_coupled_activity_xp(
        &self,
        cid: &str,
        skill_name: &str,
        explicit_stat: Option<&str>,
        skill_xp_gained: u64,
        ruleset: &RulesetProfile,
        source_event_seq: Option<i64>,
    ) -> Result<(Option<(String, u64)>, Vec<(String, u64)>)> {
        let proj = load_character_projection(&self.db, cid, None)?;
        let mut prof = self.get_character_profile(cid)?;
        let now_str = self.db.get_campaign_meta()?.clock.format_short();
        let mut changed = false;

        // 1. Governing Stat Training XP trickle (35% of skill XP)
        let gov_stat: Option<String> = explicit_stat
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                proj.current
                    .skills
                    .iter()
                    .find(|s| s.name.eq_ignore_ascii_case(skill_name))
                    .and_then(|s| s.governing_stat.clone())
                    .filter(|s| !s.trim().is_empty())
            })
            .or_else(|| {
                lookup_skill_definition(ruleset, skill_name).map(|d| d.governing_stat.clone())
            })
            .map(|raw_s| {
                crate::mechanics::find_character_stat(&proj.current.stats, &raw_s)
                    .map(|(k, _)| k.clone())
                    .unwrap_or(raw_s)
            });

        let mut stat_xp_out: Option<(String, u64)> = None;
        if let Some(ref st_name) = gov_stat {
            let st_xp = ((skill_xp_gained as f64) * 0.35).ceil().max(1.0) as u64;
            let entry = prof
                .stat_training
                .entry(st_name.clone())
                .or_insert_with(|| StatTrainingEntry {
                    stat: st_name.clone(),
                    current_xp: 0,
                    xp_to_next: crate::routines::stat_training_threshold_for_points(0),
                    cumulative_xp: 0,
                    points_gained_from_training: 0,
                    training_notes: String::new(),
                });
            entry.current_xp = entry.current_xp.saturating_add(st_xp);
            entry.cumulative_xp = entry.cumulative_xp.saturating_add(st_xp);
            let mut earned_pts: i64 = 0;
            while entry.current_xp >= entry.xp_to_next.max(1) {
                let need = entry.xp_to_next.max(1);
                entry.current_xp -= need;
                entry.points_gained_from_training += 1;
                earned_pts += 1;
                entry.xp_to_next = crate::routines::stat_training_threshold_for_points(
                    entry.points_gained_from_training,
                );
            }
            if earned_pts > 0 {
                if ruleset.auto_apply_stat_points {
                    let _ = self.modify_stat(
                        cid,
                        st_name,
                        Some(earned_pts),
                        None,
                        "Earned from sustained activity Stat Training XP",
                    );
                } else {
                    prof.unspent_stat_points += earned_pts;
                }
                let _ = self.emit_signal(crate::signals::EngineSignal {
                    id: String::new(),
                    dedup_key: format!("stat_pt_earned:{cid}:{st_name}:{}", entry.points_gained_from_training),
                    code: "STAT_POINT_EARNED".to_string(),
                    severity: "info".to_string(),
                    category: "progression".to_string(),
                    title: format!("{} — Earned +{earned_pts} Stat Point ({st_name} Training)", proj.baseline.name),
                    summary: if ruleset.auto_apply_stat_points {
                        format!("{}'s {st_name} increased by +{earned_pts} from sustained practice.", proj.baseline.name)
                    } else {
                        format!(
                            "{} completed {st_name} training threshold (+{earned_pts} unspent stat point(s), total {}).",
                            proj.baseline.name, prof.unspent_stat_points
                        )
                    },
                    entity_ids: vec![cid.to_string()],
                    location_id: proj.current.current_location.clone(),
                    suggested_actions: vec![],
                    visibility: "player".to_string(),
                    status: "active".to_string(),
                    source_event_seq,
                    created_at_world_time: now_str.clone(),
                    updated_at_world_time: now_str.clone(),
                });
            }
            stat_xp_out = Some((st_name.clone(), st_xp));
            changed = true;
        }

        // 2. Matching Active CharacterJob XP trickle (50% of skill XP)
        let mut job_xp_out: Vec<(String, u64)> = Vec::new();
        let sk_lower = skill_name.trim().to_lowercase();
        let gov_lower = gov_stat.as_deref().unwrap_or("").to_lowercase();
        let job_xp = ((skill_xp_gained as f64) * 0.50).ceil().max(1.0) as u64;
        for job in prof.jobs.iter_mut().filter(|j| j.active) {
            let matches_stat = !gov_lower.is_empty()
                && job
                    .governing_stats
                    .iter()
                    .any(|gs| gs.eq_ignore_ascii_case(&gov_lower));
            let matches_text = job.name.to_lowercase().contains(&sk_lower)
                || sk_lower.contains(&job.name.to_lowercase())
                || job.notes.to_lowercase().contains(&sk_lower)
                || job
                    .perks_and_features
                    .iter()
                    .any(|p| p.to_lowercase().contains(&sk_lower));
            if matches_stat || matches_text {
                let old_lvl = job.level;
                job.current_xp = job.current_xp.saturating_add(job_xp);
                job.cumulative_xp = job.cumulative_xp.saturating_add(job_xp);
                while job.current_xp >= job.xp_to_next.max(1) {
                    let need = job.xp_to_next.max(1);
                    job.current_xp -= need;
                    job.level += 1;
                    job.xp_to_next = ((need as f64) * 1.35).round().max(100.0) as u64;
                }
                if job.level > old_lvl {
                    let _ = self.emit_signal(crate::signals::EngineSignal {
                        id: String::new(),
                        dedup_key: format!("job_lvl_up:{cid}:{}:{}", job.job_id, job.level),
                        code: "JOB_LEVEL_UP".to_string(),
                        severity: "warning".to_string(),
                        category: "progression".to_string(),
                        title: format!("{} — Job '{}' Reached Level {}!", proj.baseline.name, job.name, job.level),
                        summary: format!(
                            "{}'s '{}' advanced from Lv {} to Lv {}.",
                            proj.baseline.name, job.name, old_lvl, job.level
                        ),
                        entity_ids: vec![cid.to_string()],
                        location_id: proj.current.current_location.clone(),
                        suggested_actions: vec![],
                        visibility: "player".to_string(),
                        status: "active".to_string(),
                        source_event_seq,
                        created_at_world_time: now_str.clone(),
                        updated_at_world_time: now_str.clone(),
                    });
                }
                job_xp_out.push((job.name.clone(), job_xp));
                changed = true;
            }
        }

        if changed {
            prof.updated_at_world_time = now_str;
            self.db.upsert_character_profile(&prof)?;
        }
        Ok((stat_xp_out, job_xp_out))
    }

    /// Attempts to satisfy `needed_amount` in `target_currency` by converting other denominations
    /// within the **exact same currency family** (`currency_family`) using `RulesetProfile.currency_rates`.
    /// Preserves 100% exact integer wealth in base units (zero silent creation or loss).
    pub(crate) fn try_make_currency_change(
        &self,
        owner_id: &str,
        target_currency: &str,
        needed_amount: i64,
        now_str: &str,
    ) -> Result<Option<String>> {
        if needed_amount <= 0 {
            return Ok(None);
        }
        let ruleset = self.get_ruleset_profile_or_default(None)?;
        let Some(&target_rate) = ruleset.currency_rates.get(target_currency) else {
            return Ok(None);
        };
        if target_rate <= 0 {
            return Ok(None);
        }
        let fam = currency_family(target_currency);
        let balances = self.db.list_currency_balances(owner_id)?;
        let cur_target_bal = balances.get(target_currency).copied().unwrap_or(0);
        if cur_target_bal >= needed_amount {
            return Ok(None);
        }

        // Gather all other currencies held by `owner_id` in the same family with positive rates
        let mut other_coins: Vec<(String, i64, i64)> = balances
            .iter()
            .filter(|(c, &amt)| {
                amt > 0
                    && c.as_str() != target_currency
                    && currency_family(c) == fam
                    && ruleset.currency_rates.get(c.as_str()).copied().unwrap_or(0) > 0
            })
            .map(|(c, &amt)| {
                let r = ruleset.currency_rates[c.as_str()];
                (c.clone(), amt, r)
            })
            .collect();
        if other_coins.is_empty() {
            return Ok(None);
        }

        let deficit_target = needed_amount - cur_target_bal;
        let deficit_base: i128 = (deficit_target as i128) * (target_rate as i128);
        let other_total_base: i128 = other_coins
            .iter()
            .map(|(_, amt, r)| (*amt as i128) * (*r as i128))
            .sum();
        if other_total_base < deficit_base {
            return Ok(None);
        }

        // Prefer consolidating smaller coins first, then breaking the smallest sufficient higher coin
        other_coins.sort_by_key(|(_, _, r)| *r);
        let mut gathered_base: i128 = 0;
        let mut spent_parts: Vec<String> = Vec::new();
        let mut updated_others: Vec<(String, i64)> = Vec::new();

        for (coin_name, avail, rate) in other_coins {
            if gathered_base >= deficit_base {
                break;
            }
            let still_needed_base = deficit_base - gathered_base;
            let rate_128 = rate as i128;
            let needed_units = ((still_needed_base + rate_128 - 1) / rate_128).min(avail as i128) as i64;
            if needed_units > 0 {
                gathered_base += (needed_units as i128) * rate_128;
                updated_others.push((coin_name.clone(), avail - needed_units));
                spent_parts.push(format!("{needed_units} {coin_name}"));
            }
        }

        if gathered_base < deficit_base {
            return Ok(None);
        }

        // Convert gathered_base into as many `target_currency` units as possible, returning any sub-unit remainder in base currency (rate == 1)
        let target_rate_128 = target_rate as i128;
        let added_target = (gathered_base / target_rate_128) as i64;
        let remainder_base = (gathered_base % target_rate_128) as i64;

        let base_unit_key: Option<String> = if remainder_base > 0 {
            let found = ruleset
                .currency_rates
                .iter()
                .find(|(k, &r)| r == 1 && currency_family(k) == fam)
                .map(|(k, _)| k.clone());
            if found.is_none() {
                // Cannot represent remainder without losing value; abort conversion safely
                return Ok(None);
            }
            found
        } else {
            None
        };

        for (coin_name, new_amt) in updated_others {
            self.db
                .set_currency_balance(owner_id, &coin_name, new_amt, now_str)?;
        }
        let new_target_bal = cur_target_bal + added_target;
        self.db
            .set_currency_balance(owner_id, target_currency, new_target_bal, now_str)?;

        let mut recv_parts = vec![format!("+{added_target} {target_currency}")];
        if let Some(ref bkey) = base_unit_key {
            let cur_b = self.db.get_currency_balance(owner_id, bkey)?;
            self.db
                .set_currency_balance(owner_id, bkey, cur_b + remainder_base, now_str)?;
            recv_parts.push(format!("+{remainder_base} {bkey} change"));
        }

        Ok(Some(format!(
            "Auto-exchanged {} -> {}",
            spent_parts.join(" + "),
            recv_parts.join(", ")
        )))
    }
}

fn default_purposes_for_mode(mode: &str) -> Vec<String> {
    match mode {
        "business_circuit" => vec!["business".into(), "procurement".into(), "networking".into()],
        "networking_envoy" => vec!["networking".into(), "diplomacy".into(), "business".into()],
        "inspector" => vec!["inspection".into(), "sect_business".into(), "networking".into()],
        "wanderer" => vec!["networking".into(), "rumor_gathering".into(), "business".into()],
        "local_commuter" => vec!["business".into(), "errands".into()],
        _ => vec!["business".into(), "networking".into()],
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
