use crate::db::CampaignDb;
use crate::models::{
    slugify, CatalogItem, CharacterProjection, EntityHistoryEntry, EventPayload, GraphRelation,
    LoreEntry, PlaceProjection, ProgressClock, StoryAttachment, StoryText, WorldEvent,
};
use crate::projection::{load_all_character_projections, load_all_place_projections};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};

/// Multi-dimensional "Search By" filter for Characters.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CharacterSearchFilter {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub alive_status: Option<String>,
    #[serde(default)]
    pub location_id: Option<String>,
    #[serde(default)]
    pub faction: Option<String>,
    #[serde(default)]
    pub has_item: Option<String>,
    #[serde(default)]
    pub equipped_only: Option<bool>,
    #[serde(default)]
    pub has_skill: Option<String>,
    #[serde(default)]
    pub has_technique: Option<String>,
    #[serde(default)]
    pub has_condition: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub stat_name: Option<String>,
    #[serde(default)]
    pub stat_min: Option<i64>,
    #[serde(default)]
    pub resource_name: Option<String>,
    #[serde(default)]
    pub resource_below_pct: Option<u8>,
}

/// Multi-dimensional "Search By" filter for Places.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlaceSearchFilter {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub place_type: Option<String>,
    #[serde(default)]
    pub parent_place: Option<String>,
    #[serde(default)]
    pub terrain: Option<String>,
    #[serde(default)]
    pub climate: Option<String>,
    #[serde(default)]
    pub season_effect: Option<String>,
    #[serde(default)]
    pub weather: Option<String>,
    #[serde(default)]
    pub faction: Option<String>,
    #[serde(default)]
    pub danger_level: Option<String>,
    #[serde(default)]
    pub has_landmark: Option<String>,
    #[serde(default)]
    pub landmark_status: Option<String>,
    #[serde(default)]
    pub has_condition: Option<String>,
    #[serde(default)]
    pub has_character: Option<String>,
    #[serde(default)]
    pub connected_to: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
}

/// Multi-dimensional "Search By" filter for World-Building & Lore.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoreSearchFilter {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub related_to: Option<String>,
    #[serde(default)]
    pub always_include: Option<bool>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Multi-dimensional "Search By" filter for Books, Scrolls, Grimoires & Story Texts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TextSearchFilter {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub medium: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub read_status: Option<String>,
    #[serde(default)]
    pub read_by: Option<String>,
    #[serde(default)]
    pub holder_or_location_id: Option<String>,
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub related_to: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Multi-dimensional "Search By" filter for Attached Reference Docs & Media (`docs/attachments/`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AttachmentSearchFilter {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub related_to: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Multi-dimensional "Search By" filter for Cataloged Carryable Items
/// (weapons, tools, artifacts, keepsakes, books, manuals, scrolls, records).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemSearchFilter {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub item_type: Option<String>,
    #[serde(default)]
    pub rarity_or_tier: Option<String>,
    #[serde(default)]
    pub current_holder_id: Option<String>,
    #[serde(default)]
    pub previous_owner: Option<String>,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub has_linked_text: Option<bool>,
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    /// Multi-tag filter (e.g. ["clothing", "winter-gear"]). Matched using `tag_match` ("all" default or "any").
    #[serde(default)]
    pub tags: Vec<String>,
    /// "all" (AND, default) or "any" (OR) when `tags` has multiple entries.
    #[serde(default)]
    pub tag_match: Option<String>,
    /// Exclude items carrying any of these tags.
    #[serde(default)]
    pub exclude_tags: Vec<String>,
    #[serde(default)]
    pub equipped: Option<bool>,
    #[serde(default)]
    pub related_to: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Multi-dimensional filter for searching character equipment & carried inventories by tag, slot, equip state, sizing, or text.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InventorySearchFilter {
    /// Optional character slug; if omitted, searches across all characters' inventories.
    #[serde(default)]
    pub character_id: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// "all" (AND, default) or "any" (OR)
    #[serde(default)]
    pub tag_match: Option<String>,
    #[serde(default)]
    pub exclude_tags: Vec<String>,
    #[serde(default)]
    pub equipped: Option<bool>,
    #[serde(default)]
    pub slot: Option<String>,
    #[serde(default)]
    pub has_sizing: Option<bool>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventorySearchHit {
    pub character_id: String,
    pub character_name: String,
    pub primary_group: String,
    pub item: crate::models::Item,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InventorySearchResult {
    pub count: usize,
    pub total_inventory_count: usize,
    pub tag_facets: std::collections::BTreeMap<String, usize>,
    pub group_counts: std::collections::BTreeMap<String, usize>,
    pub items: Vec<InventorySearchHit>,
}

/// Multi-dimensional "Search By" filter for Entity Histories
/// (places, establishments, sects/factions, items, books/manuals, characters).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HistorySearchFilter {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub entity_id: Option<String>,
    #[serde(default)]
    pub entity_kind: Option<String>,
    #[serde(default)]
    pub era_or_date: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub related_to: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Multi-dimensional "Search By" filter for Events / Campaign Timeline.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventSearchFilter {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub entity_id: Option<String>,
    #[serde(default)]
    pub location_id: Option<String>,
    #[serde(default)]
    pub event_type: Option<String>,
    #[serde(default)]
    pub session_number: Option<u32>,
    #[serde(default)]
    pub world_time_contains: Option<String>,
    #[serde(default)]
    pub seq_from: Option<i64>,
    #[serde(default)]
    pub seq_to: Option<i64>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Multi-dimensional "Search By" filter for Knowledge Graph Relations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelationSearchFilter {
    #[serde(default)]
    pub entity_id: Option<String>,
    #[serde(default)]
    pub source_id: Option<String>,
    #[serde(default)]
    pub target_id: Option<String>,
    #[serde(default)]
    pub relation_type: Option<String>,
    #[serde(default)]
    pub min_strength: Option<i64>,
    #[serde(default)]
    pub max_strength: Option<i64>,
    #[serde(default)]
    pub query: Option<String>,
    /// Graph traversal depth from `entity_id` (1 = direct edges, 2 = friends-of-friends).
    #[serde(default)]
    pub hops: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterSearchHit {
    pub id: String,
    pub name: String,
    pub title: String,
    pub kind: String,
    pub archetype: String,
    pub faction: String,
    pub alive_status: String,
    pub current_location: Option<String>,
    pub conditions: Vec<String>,
    pub tags: Vec<String>,
    pub doc_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceSearchHit {
    pub id: String,
    pub name: String,
    pub place_type: String,
    pub terrain: String,
    pub climate: String,
    pub current_season: String,
    pub current_weather: String,
    pub danger_level: String,
    pub controlling_faction: Option<String>,
    pub present_characters: Vec<String>,
    pub landmark_count: usize,
    pub tags: Vec<String>,
    pub doc_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmniSearchResult {
    pub query: String,
    pub characters: Vec<CharacterSearchHit>,
    pub places: Vec<PlaceSearchHit>,
    pub lore: Vec<LoreEntry>,
    pub texts: Vec<StoryText>,
    pub items: Vec<CatalogItem>,
    pub histories: Vec<EntityHistoryEntry>,
    pub attachments: Vec<StoryAttachment>,
    pub relations: Vec<GraphRelation>,
    pub clocks: Vec<ProgressClock>,
    pub events: Vec<WorldEvent>,
}

/// Resolves an exact slug, case-insensitive name, or unique prefix/substring to a Character ID.
pub fn resolve_character_id(db: &CampaignDb, input: &str) -> Result<String> {
    let raw = input.trim();
    if raw.is_empty() {
        bail!("Character identifier cannot be empty");
    }
    let slug = slugify(raw);
    if db.get_character_baseline(&slug)?.is_some() {
        return Ok(slug);
    }
    let all = db.list_character_baselines()?;
    let low = raw.to_lowercase();

    // Exact name match
    if let Some(m) = all.iter().find(|c| c.name.to_lowercase() == low) {
        return Ok(m.id.clone());
    }
    // Partial match on id, name, or title
    let matches: Vec<_> = all
        .iter()
        .filter(|c| {
            c.id.contains(&slug)
                || c.name.to_lowercase().contains(&low)
                || c.title.to_lowercase().contains(&low)
        })
        .collect();

    match matches.len() {
        1 => Ok(matches[0].id.clone()),
        0 => Err(anyhow!(
            "Character '{raw}' not found. Available characters: {}",
            all.iter()
                .map(|c| format!("{} ({})", c.name, c.id))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => Err(anyhow!(
            "Ambiguous character '{raw}'. Did you mean one of: {}?",
            matches
                .iter()
                .map(|c| format!("{} ({})", c.name, c.id))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Resolves an exact slug, case-insensitive name, or unique substring to a Place ID.
pub fn resolve_place_id(db: &CampaignDb, input: &str) -> Result<String> {
    let raw = input.trim();
    if raw.is_empty() {
        bail!("Place identifier cannot be empty");
    }
    let slug = slugify(raw);
    if db.get_place_baseline(&slug)?.is_some() {
        return Ok(slug);
    }
    let all = db.list_place_baselines()?;
    let low = raw.to_lowercase();

    if let Some(m) = all.iter().find(|p| p.name.to_lowercase() == low) {
        return Ok(m.id.clone());
    }
    let matches: Vec<_> = all
        .iter()
        .filter(|p| p.id.contains(&slug) || p.name.to_lowercase().contains(&low))
        .collect();

    match matches.len() {
        1 => Ok(matches[0].id.clone()),
        0 => Err(anyhow!(
            "Place '{raw}' not found. Available places: {}",
            all.iter()
                .map(|p| format!("{} ({})", p.name, p.id))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        _ => Err(anyhow!(
            "Ambiguous place '{raw}'. Did you mean one of: {}?",
            matches
                .iter()
                .map(|p| format!("{} ({})", p.name, p.id))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

pub fn search_characters(
    db: &CampaignDb,
    filter: &CharacterSearchFilter,
) -> Result<Vec<CharacterProjection>> {
    let all = load_all_character_projections(db, None)?;
    let mut results = Vec::new();

    for cp in all.into_values() {
        if !matches_character(&cp, filter) {
            continue;
        }
        results.push(cp);
    }
    Ok(results)
}

fn matches_character(cp: &CharacterProjection, f: &CharacterSearchFilter) -> bool {
    let b = &cp.baseline;
    let c = &cp.current;

    if let Some(ref q) = f.query {
        let ql = q.trim().to_lowercase();
        if !ql.is_empty() {
            let hay = format!(
                "{} {} {} {} {} {} {} {} {} {}",
                b.id,
                b.name,
                b.title,
                b.origin,
                b.archetype,
                b.faction,
                b.appearance,
                b.personality,
                b.backstory,
                b.notes
            )
            .to_lowercase();
            let skill_match = c
                .skills
                .iter()
                .any(|s| s.name.to_lowercase().contains(&ql));
            let tech_match = c
                .techniques
                .iter()
                .any(|t| t.name.to_lowercase().contains(&ql));
            let item_match = c
                .equipment
                .iter()
                .any(|i| i.name.to_lowercase().contains(&ql));
            if !hay.contains(&ql) && !skill_match && !tech_match && !item_match {
                return false;
            }
        }
    }

    if let Some(ref kind) = f.kind {
        if !kind.trim().is_empty() && !b.kind.eq_ignore_ascii_case(kind.trim()) {
            return false;
        }
    }

    if let Some(ref st) = f.alive_status {
        if !st.trim().is_empty()
            && !c
                .alive_status
                .to_lowercase()
                .contains(&st.trim().to_lowercase())
        {
            return false;
        }
    }

    if let Some(ref loc) = f.location_id {
        let loc_slug = slugify(loc);
        if !loc.trim().is_empty()
            && c.current_location
                .as_deref()
                .map(|l| !l.eq_ignore_ascii_case(&loc_slug) && !l.contains(&loc_slug))
                .unwrap_or(true)
        {
            return false;
        }
    }

    if let Some(ref fac) = f.faction {
        if !fac.trim().is_empty()
            && !b
                .faction
                .to_lowercase()
                .contains(&fac.trim().to_lowercase())
        {
            return false;
        }
    }

    if let Some(ref item) = f.has_item {
        let il = item.trim().to_lowercase();
        if !il.is_empty() {
            let eq_only = f.equipped_only.unwrap_or(false);
            let found = c.equipment.iter().any(|i| {
                (!eq_only || i.equipped)
                    && (i.name.to_lowercase().contains(&il)
                        || i.tags.iter().any(|t| t.to_lowercase().contains(&il)))
            });
            if !found {
                return false;
            }
        }
    }

    if let Some(ref sk) = f.has_skill {
        let sl = sk.trim().to_lowercase();
        if !sl.is_empty()
            && !c.skills.iter().any(|s| {
                s.name.to_lowercase().contains(&sl)
                    || s.category.to_lowercase().contains(&sl)
                    || s.rank.to_lowercase().contains(&sl)
            })
        {
            return false;
        }
    }

    if let Some(ref tech) = f.has_technique {
        let tl = tech.trim().to_lowercase();
        if !tl.is_empty()
            && !c.techniques.iter().any(|t| {
                t.name.to_lowercase().contains(&tl)
                    || t.tier.to_lowercase().contains(&tl)
                    || t.effect.to_lowercase().contains(&tl)
            })
        {
            return false;
        }
    }

    if let Some(ref cond) = f.has_condition {
        let cl = cond.trim().to_lowercase();
        if !cl.is_empty()
            && !c.conditions.iter().any(|cd| {
                cd.name.to_lowercase().contains(&cl) || cd.kind.to_lowercase().contains(&cl)
            })
        {
            return false;
        }
    }

    if let Some(ref tag) = f.tag {
        let tl = tag.trim().to_lowercase();
        if !tl.is_empty() && !b.tags.iter().any(|t| t.to_lowercase().contains(&tl)) {
            return false;
        }
    }

    if let Some(ref stat_name) = f.stat_name {
        if !stat_name.trim().is_empty() {
            let min_val = f.stat_min.unwrap_or(i64::MIN);
            let found = c
                .stats
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(stat_name.trim()))
                .map(|(_, v)| *v >= min_val)
                .unwrap_or(false);
            if !found {
                return false;
            }
        }
    }

    if let Some(ref res_name) = f.resource_name {
        if !res_name.trim().is_empty() {
            let max_pct = f.resource_below_pct.unwrap_or(100) as f64;
            let found = c
                .resources
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(res_name.trim()))
                .map(|(_, pool)| {
                    if pool.max <= 0 {
                        false
                    } else {
                        ((pool.current as f64 / pool.max as f64) * 100.0) <= max_pct
                    }
                })
                .unwrap_or(false);
            if !found {
                return false;
            }
        }
    }

    true
}

pub fn search_places(db: &CampaignDb, filter: &PlaceSearchFilter) -> Result<Vec<PlaceProjection>> {
    let all = load_all_place_projections(db, None)?;
    // Precompute bidirectional neighbor set when `connected_to` is requested (checks both place routes & explicit travel_routes)
    let connected_neighbors: Option<BTreeSet<String>> = filter
        .connected_to
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|dest| {
            let ds = slugify(dest);
            let mut set = BTreeSet::new();
            for (pid, pp) in &all {
                let is_target = pid.eq_ignore_ascii_case(&ds) || pid.contains(&ds);
                for r in &pp.current.routes {
                    let rdest = slugify(&r.destination_id);
                    if is_target {
                        set.insert(rdest);
                    } else if rdest.eq_ignore_ascii_case(&ds) || rdest.contains(&ds) {
                        set.insert(pid.clone());
                    }
                }
            }
            if let Ok(explicit_routes) = db.list_travel_routes() {
                for tr in explicit_routes {
                    if tr.from_place_id.eq_ignore_ascii_case(&ds) || tr.from_place_id.contains(&ds) {
                        set.insert(tr.to_place_id);
                    } else if tr.to_place_id.eq_ignore_ascii_case(&ds) || tr.to_place_id.contains(&ds) {
                        set.insert(tr.from_place_id);
                    }
                }
            }
            set
        });

    let mut results = Vec::new();
    for pp in all.into_values() {
        if let Some(ref neighbors) = connected_neighbors {
            if !neighbors.contains(&pp.baseline.id) {
                continue;
            }
        }
        if !matches_place(&pp, filter) {
            continue;
        }
        results.push(pp);
    }
    Ok(results)
}

fn matches_place(pp: &PlaceProjection, f: &PlaceSearchFilter) -> bool {
    let b = &pp.baseline;
    let c = &pp.current;

    if let Some(ref q) = f.query {
        let ql = q.trim().to_lowercase();
        if !ql.is_empty() {
            let hay = format!(
                "{} {} {} {} {} {} {} {}",
                b.id, b.name, b.place_type, b.terrain, b.climate, b.description, b.notes,
                c.current_weather
            )
            .to_lowercase();
            let lm_match = c
                .landmarks
                .iter()
                .any(|l| l.name.to_lowercase().contains(&ql) || l.description.to_lowercase().contains(&ql));
            if !hay.contains(&ql) && !lm_match {
                return false;
            }
        }
    }

    if let Some(ref pt) = f.place_type {
        if !pt.trim().is_empty() && !b.place_type.eq_ignore_ascii_case(pt.trim()) {
            return false;
        }
    }

    if let Some(ref parent) = f.parent_place {
        let ps = slugify(parent);
        if !parent.trim().is_empty()
            && b.parent_place
                .as_deref()
                .map(|p| !p.eq_ignore_ascii_case(&ps) && !p.contains(&ps))
                .unwrap_or(true)
        {
            return false;
        }
    }

    if let Some(ref terr) = f.terrain {
        if !terr.trim().is_empty()
            && !b
                .terrain
                .to_lowercase()
                .contains(&terr.trim().to_lowercase())
        {
            return false;
        }
    }

    if let Some(ref clim) = f.climate {
        if !clim.trim().is_empty()
            && !b
                .climate
                .to_lowercase()
                .contains(&clim.trim().to_lowercase())
        {
            return false;
        }
    }

    if let Some(ref se) = f.season_effect {
        let sl = se.trim().to_lowercase();
        if !sl.is_empty()
            && !b
                .seasonal_profiles
                .iter()
                .any(|(k, v)| k.to_lowercase().contains(&sl) || v.to_lowercase().contains(&sl))
        {
            return false;
        }
    }

    if let Some(ref w) = f.weather {
        if !w.trim().is_empty()
            && !c
                .current_weather
                .to_lowercase()
                .contains(&w.trim().to_lowercase())
        {
            return false;
        }
    }

    if let Some(ref fac) = f.faction {
        let fl = fac.trim().to_lowercase();
        if !fl.is_empty()
            && c.controlling_faction
                .as_deref()
                .map(|cf| !cf.to_lowercase().contains(&fl))
                .unwrap_or(true)
        {
            return false;
        }
    }

    if let Some(ref dl) = f.danger_level {
        if !dl.trim().is_empty()
            && !c
                .danger_level
                .to_lowercase()
                .contains(&dl.trim().to_lowercase())
        {
            return false;
        }
    }

    if let Some(ref lm) = f.has_landmark {
        let ll = lm.trim().to_lowercase();
        if !ll.is_empty()
            && !c.landmarks.iter().any(|l| {
                l.name.to_lowercase().contains(&ll)
                    || l.kind.to_lowercase().contains(&ll)
                    || l.tags.iter().any(|t| t.to_lowercase().contains(&ll))
            })
        {
            return false;
        }
    }

    if let Some(ref ls) = f.landmark_status {
        let lsl = ls.trim().to_lowercase();
        if !lsl.is_empty()
            && !c
                .landmarks
                .iter()
                .any(|l| l.status.to_lowercase().contains(&lsl))
        {
            return false;
        }
    }

    if let Some(ref cond) = f.has_condition {
        let cl = cond.trim().to_lowercase();
        if !cl.is_empty()
            && !c
                .active_conditions
                .iter()
                .any(|ac| ac.to_lowercase().contains(&cl))
        {
            return false;
        }
    }

    if let Some(ref ch) = f.has_character {
        let cs = slugify(ch);
        if !ch.trim().is_empty()
            && !c
                .present_characters
                .iter()
                .any(|pc| pc.eq_ignore_ascii_case(&cs) || pc.contains(&cs))
        {
            return false;
        }
    }

    // `connected_to` is evaluated bidirectionally in `search_places` (including explicit travel_routes).
    let _ = &f.connected_to;

    if let Some(ref tag) = f.tag {
        let tl = tag.trim().to_lowercase();
        if !tl.is_empty() && !b.tags.iter().any(|t| t.to_lowercase().contains(&tl)) {
            return false;
        }
    }

    true
}

pub fn search_lore_filtered(db: &CampaignDb, filter: &LoreSearchFilter) -> Result<Vec<LoreEntry>> {
    let limit = filter.limit.unwrap_or(25);
    let base: Vec<LoreEntry> = if let Some(ref q) = filter.query {
        if !q.trim().is_empty() {
            let fts = db.search_lore_fts(q, 100)?;
            if !fts.is_empty() {
                fts
            } else {
                // Fallback substring match for short/punctuation queries FTS5 skips
                let ql = q.trim().to_lowercase();
                db.list_lore(None)?
                    .into_iter()
                    .filter(|l| {
                        l.title.to_lowercase().contains(&ql)
                            || l.summary.to_lowercase().contains(&ql)
                            || l.body.to_lowercase().contains(&ql)
                    })
                    .collect()
            }
        } else {
            db.list_lore(None)?
        }
    } else {
        db.list_lore(None)?
    };

    let mut out = Vec::new();
    for l in base {
        if let Some(ref cat) = filter.category {
            if !cat.trim().is_empty() && !l.category.eq_ignore_ascii_case(cat.trim()) {
                continue;
            }
        }
        if let Some(ref vis) = filter.visibility {
            if !vis.trim().is_empty() && !l.visibility.eq_ignore_ascii_case(vis.trim()) {
                continue;
            }
        }
        if let Some(ref kw) = filter.keyword {
            let kl = kw.trim().to_lowercase();
            if !kl.is_empty() && !l.keywords.iter().any(|k| k.to_lowercase().contains(&kl)) {
                continue;
            }
        }
        if let Some(ref tag) = filter.tag {
            let tl = tag.trim().to_lowercase();
            if !tl.is_empty() && !l.tags.iter().any(|t| t.to_lowercase().contains(&tl)) {
                continue;
            }
        }
        if let Some(ref rel) = filter.related_to {
            let rs = slugify(rel);
            if !rel.trim().is_empty() && !l.related_ids.iter().any(|r| r.contains(&rs)) {
                continue;
            }
        }
        if let Some(ai) = filter.always_include {
            if l.always_include != ai {
                continue;
            }
        }
        out.push(l);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

pub fn search_texts_filtered(db: &CampaignDb, filter: &TextSearchFilter) -> Result<Vec<StoryText>> {
    let limit = filter.limit.unwrap_or(25);
    let base: Vec<StoryText> = if let Some(ref q) = filter.query {
        if !q.trim().is_empty() {
            let fts = db.search_texts_fts(q, 100)?;
            if !fts.is_empty() {
                fts
            } else {
                let ql = q.trim().to_lowercase();
                db.list_texts()?
                    .into_iter()
                    .filter(|t| {
                        t.title.to_lowercase().contains(&ql)
                            || t.author.to_lowercase().contains(&ql)
                            || t.summary.to_lowercase().contains(&ql)
                            || t.content.to_lowercase().contains(&ql)
                    })
                    .collect()
            }
        } else {
            db.list_texts()?
        }
    } else {
        db.list_texts()?
    };

    let mut out = Vec::new();
    for t in base {
        if let Some(ref m) = filter.medium {
            if !m.trim().is_empty() && !t.medium.to_lowercase().contains(&m.trim().to_lowercase()) {
                continue;
            }
        }
        if let Some(ref s) = filter.subject {
            if !s.trim().is_empty() && !t.subject.to_lowercase().contains(&s.trim().to_lowercase())
            {
                continue;
            }
        }
        if let Some(ref a) = filter.author {
            if !a.trim().is_empty() && !t.author.to_lowercase().contains(&a.trim().to_lowercase()) {
                continue;
            }
        }
        if let Some(ref lang) = filter.language {
            if !lang.trim().is_empty()
                && !t
                    .language
                    .to_lowercase()
                    .contains(&lang.trim().to_lowercase())
            {
                continue;
            }
        }
        if let Some(ref cond) = filter.condition {
            if !cond.trim().is_empty()
                && !t
                    .condition
                    .to_lowercase()
                    .contains(&cond.trim().to_lowercase())
            {
                continue;
            }
        }
        if let Some(ref rs) = filter.read_status {
            if !rs.trim().is_empty() && !t.read_status.eq_ignore_ascii_case(rs.trim()) {
                continue;
            }
        }
        if let Some(ref rb) = filter.read_by {
            let rbs = slugify(rb);
            if !rb.trim().is_empty() && !t.read_by.iter().any(|r| r.contains(&rbs)) {
                continue;
            }
        }
        if let Some(ref loc) = filter.holder_or_location_id {
            let ls = slugify(loc);
            if !loc.trim().is_empty()
                && t.holder_or_location_id
                    .as_deref()
                    .map(|h| !h.contains(&ls))
                    .unwrap_or(true)
            {
                continue;
            }
        }
        if let Some(ref kw) = filter.keyword {
            let kl = kw.trim().to_lowercase();
            if !kl.is_empty() && !t.keywords.iter().any(|k| k.to_lowercase().contains(&kl)) {
                continue;
            }
        }
        if let Some(ref tag) = filter.tag {
            let tl = tag.trim().to_lowercase();
            if !tl.is_empty() && !t.tags.iter().any(|tg| tg.to_lowercase().contains(&tl)) {
                continue;
            }
        }
        if let Some(ref rel) = filter.related_to {
            let rs = slugify(rel);
            if !rel.trim().is_empty() && !t.related_ids.iter().any(|r| r.contains(&rs)) {
                continue;
            }
        }
        out.push(t);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

pub fn search_attachments_filtered(
    db: &CampaignDb,
    filter: &AttachmentSearchFilter,
) -> Result<Vec<StoryAttachment>> {
    let limit = filter.limit.unwrap_or(25);
    let base: Vec<StoryAttachment> = if let Some(ref q) = filter.query {
        if !q.trim().is_empty() {
            let fts = db.search_attachments_fts(q, 100)?;
            if !fts.is_empty() {
                fts
            } else {
                let ql = q.trim().to_lowercase();
                db.list_attachments()?
                    .into_iter()
                    .filter(|a| {
                        a.title.to_lowercase().contains(&ql)
                            || a.original_filename.to_lowercase().contains(&ql)
                            || a.summary.to_lowercase().contains(&ql)
                            || a.extracted_notes.to_lowercase().contains(&ql)
                    })
                    .collect()
            }
        } else {
            db.list_attachments()?
        }
    } else {
        db.list_attachments()?
    };

    let mut out = Vec::new();
    for a in base {
        if let Some(ref cat) = filter.category {
            if !cat.trim().is_empty()
                && !a.category.to_lowercase().contains(&cat.trim().to_lowercase())
            {
                continue;
            }
        }
        if let Some(ref mt) = filter.mime_type {
            if !mt.trim().is_empty()
                && !a.mime_type.to_lowercase().contains(&mt.trim().to_lowercase())
            {
                continue;
            }
        }
        if let Some(ref vis) = filter.visibility {
            if !vis.trim().is_empty() && !a.visibility.eq_ignore_ascii_case(vis.trim()) {
                continue;
            }
        }
        if let Some(ref kw) = filter.keyword {
            let kl = kw.trim().to_lowercase();
            if !kl.is_empty() && !a.keywords.iter().any(|k| k.to_lowercase().contains(&kl)) {
                continue;
            }
        }
        if let Some(ref tag) = filter.tag {
            let tl = tag.trim().to_lowercase();
            if !tl.is_empty() && !a.tags.iter().any(|t| t.to_lowercase().contains(&tl)) {
                continue;
            }
        }
        if let Some(ref rel) = filter.related_to {
            let rs = slugify(rel);
            if !rel.trim().is_empty() && !a.related_ids.iter().any(|r| r.contains(&rs)) {
                continue;
            }
        }
        out.push(a);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

pub fn search_items_filtered(db: &CampaignDb, filter: &ItemSearchFilter) -> Result<Vec<CatalogItem>> {
    let limit = filter.limit.unwrap_or(25);
    let base: Vec<CatalogItem> = if let Some(ref q) = filter.query {
        if !q.trim().is_empty() {
            let fts = db.search_catalog_items_fts(q, 100)?;
            if !fts.is_empty() {
                fts
            } else {
                let ql = q.trim().to_lowercase();
                db.list_catalog_items()?
                    .into_iter()
                    .filter(|i| {
                        i.name.to_lowercase().contains(&ql)
                            || i.origin_and_maker.to_lowercase().contains(&ql)
                            || i.significance.to_lowercase().contains(&ql)
                            || i.description.to_lowercase().contains(&ql)
                            || i.properties_and_effects.to_lowercase().contains(&ql)
                    })
                    .collect()
            }
        } else {
            db.list_catalog_items()?
        }
    } else {
        db.list_catalog_items()?
    };

    let mut out = Vec::new();
    for item in base {
        if let Some(ref it) = filter.item_type {
            if !it.trim().is_empty()
                && !item
                    .item_type
                    .to_lowercase()
                    .contains(&it.trim().to_lowercase())
            {
                continue;
            }
        }
        if let Some(ref rar) = filter.rarity_or_tier {
            if !rar.trim().is_empty()
                && !item
                    .rarity_or_tier
                    .to_lowercase()
                    .contains(&rar.trim().to_lowercase())
            {
                continue;
            }
        }
        if let Some(ref h) = filter.current_holder_id {
            let hs = slugify(h);
            if !h.trim().is_empty()
                && item
                    .current_holder_id
                    .as_deref()
                    .map(|cur| !cur.contains(&hs))
                    .unwrap_or(true)
            {
                continue;
            }
        }
        if let Some(ref po) = filter.previous_owner {
            let pol = po.trim().to_lowercase();
            let pos = slugify(po);
            if !pol.is_empty()
                && !item
                    .previous_owners
                    .iter()
                    .any(|o| o.to_lowercase().contains(&pol) || slugify(o).contains(&pos))
            {
                continue;
            }
        }
        if let Some(ref cond) = filter.condition {
            if !cond.trim().is_empty()
                && !item
                    .condition_or_durability
                    .to_lowercase()
                    .contains(&cond.trim().to_lowercase())
            {
                continue;
            }
        }
        if let Some(has_text) = filter.has_linked_text {
            if item.linked_text_id.is_some() != has_text {
                continue;
            }
        }
        if let Some(ref kw) = filter.keyword {
            let kl = kw.trim().to_lowercase();
            if !kl.is_empty()
                && !item.keywords.iter().any(|k| k.to_lowercase().contains(&kl))
            {
                continue;
            }
        }
        if let Some(ref tag) = filter.tag {
            let tl = crate::models::normalize_item_tag(tag);
            if !tl.is_empty()
                && !item
                    .tags
                    .iter()
                    .any(|t| crate::models::normalize_item_tag(t).contains(&tl))
            {
                continue;
            }
        }
        if !filter.tags.is_empty() {
            let wanted = crate::models::normalize_item_tags(&filter.tags);
            if !wanted.is_empty() {
                let item_norm = crate::models::normalize_item_tags(&item.tags);
                let match_any = filter
                    .tag_match
                    .as_deref()
                    .map(|m| m.eq_ignore_ascii_case("any") || m.eq_ignore_ascii_case("or"))
                    .unwrap_or(false);
                let ok = if match_any {
                    wanted.iter().any(|w| item_norm.iter().any(|t| t == w || t.contains(w)))
                } else {
                    wanted.iter().all(|w| item_norm.iter().any(|t| t == w || t.contains(w)))
                };
                if !ok {
                    continue;
                }
            }
        }
        if !filter.exclude_tags.is_empty() {
            let excl = crate::models::normalize_item_tags(&filter.exclude_tags);
            let item_norm = crate::models::normalize_item_tags(&item.tags);
            if excl.iter().any(|x| item_norm.iter().any(|t| t == x)) {
                continue;
            }
        }
        if let Some(eq) = filter.equipped {
            if item.equipped != eq {
                continue;
            }
        }
        if let Some(ref rel) = filter.related_to {
            let rs = slugify(rel);
            if !rel.trim().is_empty() && !item.related_ids.iter().any(|r| r.contains(&rs)) {
                continue;
            }
        }
        out.push(item);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

pub fn compute_catalog_tag_facets(items: &[CatalogItem]) -> std::collections::BTreeMap<String, usize> {
    let mut facets = std::collections::BTreeMap::new();
    for item in items {
        for tag in crate::models::normalize_item_tags(&item.tags) {
            *facets.entry(tag).or_insert(0) += 1;
        }
    }
    facets
}

pub fn search_character_inventory(
    db: &CampaignDb,
    filter: &InventorySearchFilter,
) -> Result<InventorySearchResult> {
    let chars: Vec<CharacterProjection> = if let Some(ref cid) = filter.character_id {
        let trimmed = cid.trim();
        if trimmed.is_empty() {
            load_all_character_projections(db, None)?
                .into_values()
                .collect()
        } else {
            let target_slug = slugify(trimmed);
            let resolved = db
                .list_character_baselines()?
                .into_iter()
                .find(|c| {
                    c.id == target_slug
                        || c.id.eq_ignore_ascii_case(trimmed)
                        || c.name.eq_ignore_ascii_case(trimmed)
                })
                .map(|c| c.id)
                .unwrap_or(target_slug);
            vec![crate::projection::load_character_projection(
                db, &resolved, None,
            )?]
        }
    } else {
        load_all_character_projections(db, None)?
            .into_values()
            .collect()
    };

    let mut total_inventory_count = 0usize;
    let mut tag_facets: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut group_counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut hits: Vec<InventorySearchHit> = Vec::new();

    let mut wanted_tags = crate::models::normalize_item_tags(&filter.tags);
    if let Some(ref single) = filter.tag {
        let n = crate::models::normalize_item_tag(single);
        if !n.is_empty() && !wanted_tags.iter().any(|w| w == &n) {
            wanted_tags.push(n);
        }
    }
    let excl_tags = crate::models::normalize_item_tags(&filter.exclude_tags);
    let match_any = filter
        .tag_match
        .as_deref()
        .map(|m| m.eq_ignore_ascii_case("any") || m.eq_ignore_ascii_case("or"))
        .unwrap_or(false);
    let ql = filter
        .query
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .unwrap_or_default();
    let slot_l = filter
        .slot
        .as_deref()
        .map(|s| s.trim().to_lowercase())
        .unwrap_or_default();
    let limit = filter.limit.unwrap_or(200);

    for cp in chars {
        for item in cp.current.equipment {
            total_inventory_count += 1;
            let norm_tags = crate::models::normalize_item_tags(&item.tags);
            let group = crate::models::primary_item_group(
                &norm_tags,
                item.slot.as_deref(),
                &item.name,
            );
            for t in &norm_tags {
                *tag_facets.entry(t.clone()).or_insert(0) += 1;
            }
            *group_counts.entry(group.clone()).or_insert(0) += 1;

            if let Some(eq) = filter.equipped {
                if item.equipped != eq {
                    continue;
                }
            }
            if let Some(hs) = filter.has_sizing {
                let sized = !item.garment_measurements.is_empty()
                    || !item.garment_cut.trim().is_empty()
                    || !item.garment_taper.trim().is_empty();
                if sized != hs {
                    continue;
                }
            }
            if !slot_l.is_empty() {
                let s_ok = item
                    .slot
                    .as_deref()
                    .map(|s| s.to_lowercase().contains(&slot_l))
                    .unwrap_or(false);
                if !s_ok {
                    continue;
                }
            }
            if !ql.is_empty() {
                let matched_q = item.name.to_lowercase().contains(&ql)
                    || item.description.to_lowercase().contains(&ql)
                    || item
                        .slot
                        .as_deref()
                        .map(|s| s.to_lowercase().contains(&ql))
                        .unwrap_or(false)
                    || norm_tags.iter().any(|t| t.contains(&ql))
                    || group.to_lowercase().contains(&ql)
                    || item.garment_cut.to_lowercase().contains(&ql)
                    || item.garment_taper.to_lowercase().contains(&ql);
                if !matched_q {
                    continue;
                }
            }
            if !wanted_tags.is_empty() {
                let ok = if match_any {
                    wanted_tags
                        .iter()
                        .any(|w| norm_tags.iter().any(|t| t == w || t.contains(w)))
                } else {
                    wanted_tags
                        .iter()
                        .all(|w| norm_tags.iter().any(|t| t == w || t.contains(w)))
                };
                if !ok {
                    continue;
                }
            }
            if !excl_tags.is_empty()
                && excl_tags
                    .iter()
                    .any(|x| norm_tags.iter().any(|t| t == x))
            {
                continue;
            }

            if hits.len() < limit {
                hits.push(InventorySearchHit {
                    character_id: cp.baseline.id.clone(),
                    character_name: cp.baseline.name.clone(),
                    primary_group: group,
                    item,
                });
            }
        }
    }

    Ok(InventorySearchResult {
        count: hits.len(),
        total_inventory_count,
        tag_facets,
        group_counts,
        items: hits,
    })
}

pub fn search_histories_filtered(
    db: &CampaignDb,
    filter: &HistorySearchFilter,
) -> Result<Vec<EntityHistoryEntry>> {
    let limit = filter.limit.unwrap_or(40);
    let base: Vec<EntityHistoryEntry> = if let Some(ref q) = filter.query {
        if !q.trim().is_empty() {
            let fts = db.search_histories_fts(q, 100)?;
            if !fts.is_empty() {
                fts
            } else {
                let ql = q.trim().to_lowercase();
                db.list_all_histories()?
                    .into_iter()
                    .filter(|h| {
                        h.title.to_lowercase().contains(&ql)
                            || h.details.to_lowercase().contains(&ql)
                            || h.significance.to_lowercase().contains(&ql)
                            || h.era_or_date.to_lowercase().contains(&ql)
                    })
                    .collect()
            }
        } else {
            db.list_all_histories()?
        }
    } else if let Some(ref eid) = filter.entity_id {
        if !eid.trim().is_empty() {
            db.list_histories_for_entity(&slugify(eid))?
        } else {
            db.list_all_histories()?
        }
    } else {
        db.list_all_histories()?
    };

    let mut out = Vec::new();
    for h in base {
        if let Some(ref eid) = filter.entity_id {
            let es = slugify(eid);
            if !eid.trim().is_empty() && !h.entity_id.contains(&es) {
                continue;
            }
        }
        if let Some(ref ek) = filter.entity_kind {
            if !ek.trim().is_empty() && !h.entity_kind.eq_ignore_ascii_case(ek.trim()) {
                continue;
            }
        }
        if let Some(ref era) = filter.era_or_date {
            if !era.trim().is_empty()
                && !h
                    .era_or_date
                    .to_lowercase()
                    .contains(&era.trim().to_lowercase())
            {
                continue;
            }
        }
        if let Some(ref vis) = filter.visibility {
            if !vis.trim().is_empty() && !h.visibility.eq_ignore_ascii_case(vis.trim()) {
                continue;
            }
        }
        if let Some(ref rel) = filter.related_to {
            let rs = slugify(rel);
            if !rel.trim().is_empty() && !h.related_ids.iter().any(|r| r.contains(&rs)) {
                continue;
            }
        }
        out.push(h);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

pub fn search_events_filtered(
    db: &CampaignDb,
    filter: &EventSearchFilter,
) -> Result<Vec<WorldEvent>> {
    let limit = filter.limit.unwrap_or(50);
    let base: Vec<WorldEvent> = if let Some(ref q) = filter.query {
        if !q.trim().is_empty() {
            let fts = db.search_events_fts(q, 200)?;
            if !fts.is_empty() {
                fts
            } else {
                let ql = q.trim().to_lowercase();
                db.list_all_events()?
                    .into_iter()
                    .filter(|e| e.summary.to_lowercase().contains(&ql))
                    .collect()
            }
        } else {
            db.list_all_events()?
        }
    } else if let Some(ref eid) = filter.entity_id {
        if !eid.trim().is_empty() {
            db.list_events_for_entity(&slugify(eid))?
        } else {
            db.list_all_events()?
        }
    } else {
        db.list_all_events()?
    };

    let mut out = Vec::new();
    for ev in base {
        if let Some(ref eid) = filter.entity_id {
            let es = slugify(eid);
            if !eid.trim().is_empty() && !ev.affected_entities.iter().any(|a| a.contains(&es)) {
                continue;
            }
        }
        if let Some(ref loc) = filter.location_id {
            let ls = slugify(loc);
            if !loc.trim().is_empty()
                && ev
                    .location_id
                    .as_deref()
                    .map(|l| !l.contains(&ls))
                    .unwrap_or(true)
            {
                continue;
            }
        }
        if let Some(ref et) = filter.event_type {
            let etl = et.trim().to_lowercase();
            if !etl.is_empty() && !event_type_slug(&ev.payload).contains(&etl) {
                continue;
            }
        }
        if let Some(sn) = filter.session_number {
            if ev.session_number != sn {
                continue;
            }
        }
        if let Some(ref wt) = filter.world_time_contains {
            if !wt.trim().is_empty()
                && !ev
                    .world_time
                    .to_lowercase()
                    .contains(&wt.trim().to_lowercase())
            {
                continue;
            }
        }
        if let Some(sf) = filter.seq_from {
            if ev.seq < sf {
                continue;
            }
        }
        if let Some(st) = filter.seq_to {
            if ev.seq > st {
                continue;
            }
        }
        out.push(ev);
    }

    // Keep chronological order and return up to `limit` most recent matching events
    out.sort_by_key(|e| e.seq);
    if out.len() > limit {
        out = out.split_off(out.len() - limit);
    }
    Ok(out)
}

pub fn search_relations_filtered(
    db: &CampaignDb,
    filter: &RelationSearchFilter,
) -> Result<Vec<GraphRelation>> {
    let all = db.list_all_relations()?;
    let hops = filter.hops.unwrap_or(1).clamp(1, 3);

    let candidate_edges: Vec<GraphRelation> = if let Some(ref eid) = filter.entity_id {
        if !eid.trim().is_empty() {
            let start = slugify(eid);
            let mut visited_nodes: BTreeSet<String> = BTreeSet::new();
            let mut queue: VecDeque<(String, u8)> = VecDeque::new();
            visited_nodes.insert(start.clone());
            queue.push_back((start, 0));

            let mut collected: Vec<GraphRelation> = Vec::new();
            let mut seen_edges: BTreeSet<(String, String, String)> = BTreeSet::new();

            while let Some((node, depth)) = queue.pop_front() {
                if depth >= hops {
                    continue;
                }
                for rel in &all {
                    if rel.source_id == node || rel.target_id == node {
                        let key = (
                            rel.source_id.clone(),
                            rel.relation_type.clone(),
                            rel.target_id.clone(),
                        );
                        if seen_edges.insert(key) {
                            collected.push(rel.clone());
                        }
                        let neighbor = if rel.source_id == node {
                            &rel.target_id
                        } else {
                            &rel.source_id
                        };
                        if visited_nodes.insert(neighbor.clone()) {
                            queue.push_back((neighbor.clone(), depth + 1));
                        }
                    }
                }
            }
            collected
        } else {
            all
        }
    } else {
        all
    };

    let mut out = Vec::new();
    for r in candidate_edges {
        if let Some(ref s) = filter.source_id {
            if !s.trim().is_empty() && !r.source_id.contains(&slugify(s)) {
                continue;
            }
        }
        if let Some(ref t) = filter.target_id {
            if !t.trim().is_empty() && !r.target_id.contains(&slugify(t)) {
                continue;
            }
        }
        if let Some(ref rt) = filter.relation_type {
            if !rt.trim().is_empty()
                && !r
                    .relation_type
                    .to_lowercase()
                    .contains(&rt.trim().to_lowercase())
            {
                continue;
            }
        }
        if let Some(min_s) = filter.min_strength {
            if r.strength.map(|v| v < min_s).unwrap_or(true) {
                continue;
            }
        }
        if let Some(max_s) = filter.max_strength {
            if r.strength.map(|v| v > max_s).unwrap_or(true) {
                continue;
            }
        }
        if let Some(ref q) = filter.query {
            let ql = q.trim().to_lowercase();
            if !ql.is_empty()
                && !r.source_id.contains(&ql)
                && !r.target_id.contains(&ql)
                && !r.relation_type.to_lowercase().contains(&ql)
                && !r.notes.to_lowercase().contains(&ql)
            {
                continue;
            }
        }
        out.push(r);
    }

    Ok(out)
}

pub fn search_all(db: &CampaignDb, query: &str, limit_per_kind: usize) -> Result<OmniSearchResult> {
    let q = query.trim();
    let ql = q.to_lowercase();

    let chars = search_characters(
        db,
        &CharacterSearchFilter {
            query: Some(q.to_string()),
            ..Default::default()
        },
    )?
    .into_iter()
    .take(limit_per_kind)
    .map(character_to_hit)
    .collect();

    let places = search_places(
        db,
        &PlaceSearchFilter {
            query: Some(q.to_string()),
            ..Default::default()
        },
    )?
    .into_iter()
    .take(limit_per_kind)
    .map(place_to_hit)
    .collect();

    let lore = search_lore_filtered(
        db,
        &LoreSearchFilter {
            query: Some(q.to_string()),
            limit: Some(limit_per_kind),
            ..Default::default()
        },
    )?;

    let texts = search_texts_filtered(
        db,
        &TextSearchFilter {
            query: Some(q.to_string()),
            limit: Some(limit_per_kind),
            ..Default::default()
        },
    )?;

    let items = search_items_filtered(
        db,
        &ItemSearchFilter {
            query: Some(q.to_string()),
            limit: Some(limit_per_kind),
            ..Default::default()
        },
    )?;

    let histories = search_histories_filtered(
        db,
        &HistorySearchFilter {
            query: Some(q.to_string()),
            limit: Some(limit_per_kind),
            ..Default::default()
        },
    )?;

    let attachments = search_attachments_filtered(
        db,
        &AttachmentSearchFilter {
            query: Some(q.to_string()),
            limit: Some(limit_per_kind),
            ..Default::default()
        },
    )?;

    let relations = search_relations_filtered(
        db,
        &RelationSearchFilter {
            query: Some(q.to_string()),
            ..Default::default()
        },
    )?
    .into_iter()
    .take(limit_per_kind)
    .collect();

    let clocks = db
        .list_clocks()?
        .into_iter()
        .filter(|c| {
            ql.is_empty()
                || c.name.to_lowercase().contains(&ql)
                || c.description.to_lowercase().contains(&ql)
                || c.kind.to_lowercase().contains(&ql)
                || c.id.contains(&ql)
        })
        .take(limit_per_kind)
        .collect();

    let events = search_events_filtered(
        db,
        &EventSearchFilter {
            query: Some(q.to_string()),
            limit: Some(limit_per_kind),
            ..Default::default()
        },
    )?;

    Ok(OmniSearchResult {
        query: q.to_string(),
        characters: chars,
        places,
        lore,
        texts,
        items,
        histories,
        attachments,
        relations,
        clocks,
        events,
    })
}

pub fn character_to_hit(cp: CharacterProjection) -> CharacterSearchHit {
    CharacterSearchHit {
        doc_path: format!("docs/characters/{}.md", cp.baseline.id),
        id: cp.baseline.id,
        name: cp.baseline.name,
        title: cp.baseline.title,
        kind: cp.baseline.kind,
        archetype: cp.baseline.archetype,
        faction: cp.baseline.faction,
        alive_status: cp.current.alive_status,
        current_location: cp.current.current_location,
        conditions: cp
            .current
            .conditions
            .into_iter()
            .map(|c| c.name)
            .collect(),
        tags: cp.baseline.tags,
    }
}

pub fn place_to_hit(pp: PlaceProjection) -> PlaceSearchHit {
    PlaceSearchHit {
        doc_path: format!("docs/places/{}.md", pp.baseline.id),
        id: pp.baseline.id,
        name: pp.baseline.name,
        place_type: pp.baseline.place_type,
        terrain: pp.baseline.terrain,
        climate: pp.baseline.climate,
        current_season: pp.current.current_season,
        current_weather: pp.current.current_weather,
        danger_level: pp.current.danger_level,
        controlling_faction: pp.current.controlling_faction,
        present_characters: pp.current.present_characters,
        landmark_count: pp.current.landmarks.len(),
        tags: pp.baseline.tags,
    }
}

pub fn event_type_slug(p: &EventPayload) -> &'static str {
    match p {
        EventPayload::CampaignStarted { .. } => "campaign_started",
        EventPayload::SessionMarker { .. } => "session_marker",
        EventPayload::TimeAdvanced { .. } => "time_advanced",
        EventPayload::CharacterCreated { .. } => "character_created",
        EventPayload::CharacterMoved { .. } => "character_moved",
        EventPayload::ResourceChanged { .. } => "resource_changed",
        EventPayload::StatChanged { .. } => "stat_changed",
        EventPayload::CharacterStatusChanged { .. } => "character_status_changed",
        EventPayload::ConditionAdded { .. } => "condition_added",
        EventPayload::ConditionRemoved { .. } => "condition_removed",
        EventPayload::ItemAcquired { .. } => "item_acquired",
        EventPayload::ItemRemoved { .. } => "item_removed",
        EventPayload::ItemEquipped { .. } => "item_equipped",
        EventPayload::ItemTagsUpdated { .. } => "item_tags_updated",
        EventPayload::SkillUpdated { .. } => "skill_updated",
        EventPayload::TechniqueUpdated { .. } => "technique_updated",
        EventPayload::GoalUpdated { .. } => "goal_updated",
        EventPayload::PlaceCreated { .. } => "place_created",
        EventPayload::PlaceWeatherChanged { .. } => "place_weather_changed",
        EventPayload::PlaceConditionChanged { .. } => "place_condition_changed",
        EventPayload::PlaceControlChanged { .. } => "place_control_changed",
        EventPayload::LandmarkUpdated { .. } => "landmark_updated",
        EventPayload::RouteUpdated { .. } => "route_updated",
        EventPayload::RelationUpserted { .. } => "relation_upserted",
        EventPayload::RelationRemoved { .. } => "relation_removed",
        EventPayload::ClockCreated { .. } => "clock_created",
        EventPayload::ClockTicked { .. } => "clock_ticked",
        EventPayload::LoreRecorded { .. } => "lore_recorded",
        EventPayload::TextRecorded { .. } => "text_recorded",
        EventPayload::TextStudied { .. } => "text_studied",
        EventPayload::AttachmentRecorded { .. } => "attachment_recorded",
        EventPayload::ItemCataloged { .. } => "item_cataloged",
        EventPayload::CatalogItemTransferred { .. } => "catalog_item_transferred",
        EventPayload::EntityHistoryAdded { .. } => "entity_history_added",
        EventPayload::EntityEdited { .. } => "entity_edited",
        EventPayload::EntityReclassified { .. } => "entity_reclassified",
        EventPayload::EntityRenamedOrMerged { .. } => "entity_renamed_or_merged",
        EventPayload::EntityDeleted { .. } => "entity_deleted",
        EventPayload::DiceRolled { .. } => "dice_rolled",
        EventPayload::RulesetProfileUpserted { .. } => "ruleset_profile_upserted",
        EventPayload::CheckResolved { .. } => "check_resolved",
        EventPayload::ContestResolved { .. } => "contest_resolved",
        EventPayload::RandomTableUpserted { .. } => "random_table_upserted",
        EventPayload::RandomTableDrawn { .. } => "random_table_drawn",
        EventPayload::DiscoveryClaimed { .. } => "discovery_claimed",
        EventPayload::EncounterStarted { .. } => "encounter_started",
        EventPayload::EncounterActionResolved { .. } => "encounter_action_resolved",
        EventPayload::EncounterEnded { .. } => "encounter_ended",
        EventPayload::CharacterProfileUpdated { .. } => "character_profile_updated",
        EventPayload::KnowledgeRecorded { .. } => "knowledge_recorded",
        EventPayload::KnowledgePropagated { .. } => "knowledge_propagated",
        EventPayload::FactionRecorded { .. } => "faction_recorded",
        EventPayload::AutonomySimulated { .. } => "autonomy_simulated",
        EventPayload::TravelResolved { .. } => "travel_resolved",
        EventPayload::AreaExplored { .. } => "area_explored",
        EventPayload::TransactionExecuted { .. } => "transaction_executed",
        EventPayload::MarketUpdated { .. } => "market_updated",
        EventPayload::LawOrReputationUpdated { .. } => "law_or_reputation_updated",
        EventPayload::InjuryUpdated { .. } => "injury_updated",
        EventPayload::DowntimeOrCraftingResolved { .. } => "downtime_or_crafting_resolved",
        EventPayload::ProgressionResolved { .. } => "progression_resolved",
        EventPayload::WorldAdvanced { .. } => "world_advanced",
        EventPayload::NarrativeEvent { .. } => "narrative_event",
    }
}
