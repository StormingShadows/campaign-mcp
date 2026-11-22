use crate::autonomy::FactionRecord;
use crate::db::CampaignDb;
use crate::mechanics::{EncounterState, RandomTable};
use crate::models::{
    slugify, CatalogItem, CharacterProjection, EntityHistoryEntry, EventPayload, GraphRelation,
    LoreEntry, PlaceProjection, ResourcePool, StoryAttachment, StoryText, WorldEvent,
};
use crate::projection::{load_all_character_projections, load_all_place_projections};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CompileReport {
    pub docs_dir: PathBuf,
    pub character_files: usize,
    pub place_files: usize,
    pub lore_files: usize,
    pub text_files: usize,
    pub item_files: usize,
    pub attachment_files: usize,
    pub total_events: usize,
}

/// Compiles the entire SQLite event-sourced campaign into clean, cross-linked,
/// human- & agent-referencable Markdown documents under `<campaign_dir>/docs/`.
pub fn compile_all_docs(db: &CampaignDb, docs_dir: &Path) -> Result<CompileReport> {
    let chars_dir = docs_dir.join("characters");
    let places_dir = docs_dir.join("places");
    let lore_dir = docs_dir.join("lore");
    let texts_dir = docs_dir.join("texts");
    let items_dir = docs_dir.join("items");
    let attach_dir = docs_dir.join("attachments");

    fs::create_dir_all(&chars_dir)?;
    fs::create_dir_all(&places_dir)?;
    fs::create_dir_all(&lore_dir)?;
    fs::create_dir_all(&texts_dir)?;
    fs::create_dir_all(&items_dir)?;
    fs::create_dir_all(&attach_dir)?;

    let meta = db.get_campaign_meta()?;
    let characters = load_all_character_projections(db, None)?;
    let places = load_all_place_projections(db, None)?;
    let lore_entries = db.list_lore(None)?;
    let story_texts = db.list_texts()?;
    let catalog_items = db.list_catalog_items()?;
    let attachments = db.list_attachments()?;
    let random_tables = db.list_random_tables()?;
    let encounters = db.list_encounters(None)?;
    let factions = db.list_factions()?;
    let markets = db.list_markets()?;
    let all_events = db.list_all_events()?;
    let clocks = db.list_clocks()?;
    let (chain_valid, event_count) = db.verify_event_chain()?;

    // Build lookup maps for clean human-readable cross-linking
    let mut name_map: BTreeMap<String, (String, String)> = BTreeMap::new(); // id -> (name, rel_path_from_docs)
    for (id, cp) in &characters {
        name_map.insert(
            id.clone(),
            (cp.baseline.name.clone(), format!("characters/{id}.md")),
        );
    }
    for (id, pp) in &places {
        name_map.insert(
            id.clone(),
            (pp.baseline.name.clone(), format!("places/{id}.md")),
        );
    }
    for l in &lore_entries {
        let cat_slug = slugify(&l.category);
        name_map.insert(
            l.id.clone(),
            (l.title.clone(), format!("lore/{cat_slug}/{}.md", l.id)),
        );
    }
    for t in &story_texts {
        name_map.insert(t.id.clone(), (t.title.clone(), format!("texts/{}.md", t.id)));
    }
    for it in &catalog_items {
        // Do not overwrite a StoryText with the same ID if linked
        name_map
            .entry(it.id.clone())
            .or_insert_with(|| (it.name.clone(), format!("items/{}.md", it.id)));
    }
    for a in &attachments {
        name_map.insert(
            a.id.clone(),
            (a.title.clone(), format!("attachments/{}.md", a.id)),
        );
    }
    for f in &factions {
        name_map.insert(f.id.clone(), (f.name.clone(), format!("factions/{}.md", f.id)));
    }

    // 1. Compile Characters
    for (id, cp) in &characters {
        let relations = db.list_relations_for_entity(id)?;
        let histories = db.list_histories_for_entity(id)?;
        let md = render_character_markdown(cp, &relations, &histories, &name_map);
        let path = chars_dir.join(format!("{id}.md"));
        fs::write(&path, md).with_context(|| format!("Failed writing {}", path.display()))?;
    }

    // 2. Compile Places (including Establishments)
    for (id, pp) in &places {
        let relations = db.list_relations_for_entity(id)?;
        let histories = db.list_histories_for_entity(id)?;
        let md = render_place_markdown(pp, &relations, &histories, &name_map);
        let path = places_dir.join(format!("{id}.md"));
        fs::write(&path, md).with_context(|| format!("Failed writing {}", path.display()))?;
    }

    // 3. Compile World-Building & Lore (including Sects, Factions & Establishments)
    for l in &lore_entries {
        let cat_slug = slugify(&l.category);
        let cat_dir = lore_dir.join(&cat_slug);
        fs::create_dir_all(&cat_dir)?;
        let events = db.list_events_for_entity(&l.id)?;
        let relations = db.list_relations_for_entity(&l.id)?;
        let histories = db.list_histories_for_entity(&l.id)?;
        let md = render_lore_markdown(l, &relations, &histories, &events, &name_map);
        let path = cat_dir.join(format!("{}.md", l.id));
        fs::write(&path, md).with_context(|| format!("Failed writing {}", path.display()))?;
    }

    // 4. Compile Books, Scrolls & Story Texts
    for t in &story_texts {
        let events = db.list_events_for_entity(&t.id)?;
        let relations = db.list_relations_for_entity(&t.id)?;
        let histories = db.list_histories_for_entity(&t.id)?;
        let md = render_text_markdown(t, &relations, &histories, &events, &name_map);
        let path = texts_dir.join(format!("{}.md", t.id));
        fs::write(&path, md).with_context(|| format!("Failed writing {}", path.display()))?;
    }

    // 5. Compile Carryable Items, Tools, Weapons, Artifacts, Keepsakes & Books/Manuals
    for it in &catalog_items {
        let events = db.list_events_for_entity(&it.id)?;
        let relations = db.list_relations_for_entity(&it.id)?;
        let histories = db.list_histories_for_entity(&it.id)?;
        let md = render_item_markdown(it, &relations, &histories, &events, &name_map);
        let path = items_dir.join(format!("{}.md", it.id));
        fs::write(&path, md).with_context(|| format!("Failed writing {}", path.display()))?;
    }

    // 6. Compile Attached Reference Docs & Media Companion Cards
    for a in &attachments {
        let events = db.list_events_for_entity(&a.id)?;
        let relations = db.list_relations_for_entity(&a.id)?;
        let md = render_attachment_markdown(a, &relations, &events, &name_map);
        let path = attach_dir.join(format!("{}.md", a.id));
        fs::write(&path, md).with_context(|| format!("Failed writing {}", path.display()))?;
    }

    // 6b. Compile Random Tables (`docs/tables/<id>.md`)
    if !random_tables.is_empty() {
        let tables_dir = docs_dir.join("tables");
        fs::create_dir_all(&tables_dir)?;
        for t in &random_tables {
            let md = render_random_table_markdown(t);
            let path = tables_dir.join(format!("{}.md", t.id));
            fs::write(&path, md).with_context(|| format!("Failed writing {}", path.display()))?;
        }
    }

    // 6c. Compile Encounters (`docs/encounters/<id>.md`)
    if !encounters.is_empty() {
        let enc_dir = docs_dir.join("encounters");
        fs::create_dir_all(&enc_dir)?;
        for enc in &encounters {
            let md = render_encounter_markdown(enc, &name_map);
            let path = enc_dir.join(format!("{}.md", enc.id));
            fs::write(&path, md).with_context(|| format!("Failed writing {}", path.display()))?;
        }
    }

    // 6d. Compile Dedicated Factions (`docs/factions/<id>.md`)
    if !factions.is_empty() {
        let fac_dir = docs_dir.join("factions");
        fs::create_dir_all(&fac_dir)?;
        for f in &factions {
            let rels = db.list_relations_for_entity(&f.id)?;
            let histories = db.list_histories_for_entity(&f.id)?;
            let events = db.list_events_for_entity(&f.id)?;
            let md = render_faction_markdown(f, &rels, &histories, &events, &name_map);
            let path = fac_dir.join(format!("{}.md", f.id));
            fs::write(&path, md).with_context(|| format!("Failed writing {}", path.display()))?;
        }
    }

    // 6e. Compile Settlement Markets (`docs/economy/markets.md`)
    if !markets.is_empty() {
        let econ_dir = docs_dir.join("economy");
        fs::create_dir_all(&econ_dir)?;
        let routes = db.list_travel_routes()?;
        let md = render_markets_markdown(&markets, &routes, &meta.clock.season, &meta.clock.global_weather);
        fs::write(econ_dir.join("markets.md"), md)?;
    }

    // 7. Compile Master INDEX.md
    let index_md = render_index_markdown(
        &meta,
        &characters,
        &places,
        &lore_entries,
        &story_texts,
        &catalog_items,
        &attachments,
        &clocks,
        chain_valid,
        event_count,
        &name_map,
    );
    fs::write(docs_dir.join("INDEX.md"), index_md)?;

    // 8. Compile Master TIMELINE.md
    let timeline_md = render_timeline_markdown(&meta, &all_events, chain_valid, &name_map);
    fs::write(docs_dir.join("TIMELINE.md"), timeline_md)?;

    Ok(CompileReport {
        docs_dir: docs_dir.to_path_buf(),
        character_files: characters.len(),
        place_files: places.len(),
        lore_files: lore_entries.len(),
        text_files: story_texts.len(),
        item_files: catalog_items.len(),
        attachment_files: attachments.len(),
        total_events: all_events.len(),
    })
}

pub fn render_character_markdown(
    proj: &CharacterProjection,
    relations: &[GraphRelation],
    histories: &[EntityHistoryEntry],
    name_map: &BTreeMap<String, (String, String)>,
) -> String {
    let b = &proj.baseline;
    let c = &proj.current;
    let mut out = String::new();

    // YAML Frontmatter for easy reference & Obsidian/tool indexing
    let _ = writeln!(out, "---");
    let _ = writeln!(out, "id: {}", b.id);
    let _ = writeln!(out, "type: character");
    let _ = writeln!(out, "kind: {}", b.kind);
    let _ = writeln!(out, "status: {}", c.alive_status);
    if let Some(ref loc) = c.current_location {
        let _ = writeln!(out, "location: {loc}");
    }
    if !b.tags.is_empty() {
        let _ = writeln!(out, "tags: [{}]", b.tags.join(", "));
    }
    let _ = writeln!(out, "last_event_seq: {}", c.last_event_seq);
    let _ = writeln!(out, "---\n");

    let title_suffix = if b.title.is_empty() {
        String::new()
    } else {
        format!(" — *{}*", b.title)
    };
    let _ = writeln!(out, "# {}{}\n", b.name, title_suffix);
    let _ = writeln!(
        out,
        "> **Document ID:** `{}` | **Kind:** `{}` | [Campaign Index](../INDEX.md) | [Master Timeline](../TIMELINE.md)\n",
        b.id, b.kind
    );

    // =========================================================================
    // SECTION 1: INITIAL / BASELINE PROFILE
    // =========================================================================
    let _ = writeln!(out, "## 1. Initial / Baseline Profile\n");
    let _ = writeln!(out, "| Field | Value |");
    let _ = writeln!(out, "| :--- | :--- |");
    let _ = writeln!(out, "| **Name** | {} |", b.name);
    if !b.title.is_empty() {
        let _ = writeln!(out, "| **Title** | {} |", b.title);
    }
    if !b.origin.is_empty() {
        let _ = writeln!(out, "| **Origin / Species** | {} |", b.origin);
    }
    if !b.archetype.is_empty() {
        let _ = writeln!(out, "| **Archetype / Role** | {} |", b.archetype);
    }
    if !b.faction.is_empty() {
        let _ = writeln!(out, "| **Starting Faction** | {} |", b.faction);
    }
    if let Some(ref loc) = b.initial_location {
        let _ = writeln!(
            out,
            "| **Starting Location** | {} |",
            link_from_subdir(loc, "..", name_map)
        );
    }
    let _ = writeln!(
        out,
        "| **Created At (In-World)** | {} |\n",
        b.created_at_world_time
    );

    if !b.appearance.is_empty() {
        let _ = writeln!(out, "**Appearance:** {}\n", b.appearance);
    }
    if !b.personality.is_empty() {
        let _ = writeln!(out, "**Personality:** {}\n", b.personality);
    }
    if !b.backstory.is_empty() {
        let _ = writeln!(out, "**Backstory:** {}\n", b.backstory);
    }
    if !b.notes.is_empty() {
        let _ = writeln!(out, "**GM Notes:** {}\n", b.notes);
    }

    if !b.initial_stats.is_empty() {
        let _ = writeln!(out, "### Initial Attributes / Stats\n");
        let _ = writeln!(out, "| Attribute | Base Value |");
        let _ = writeln!(out, "| :--- | ---: |");
        for (k, v) in &b.initial_stats {
            let _ = writeln!(out, "| {k} | {v} |");
        }
        let _ = writeln!(out);
    }

    if !b.initial_resources.is_empty() {
        let _ = writeln!(out, "### Initial Resources\n");
        let _ = writeln!(out, "| Resource | Starting | Max |");
        let _ = writeln!(out, "| :--- | ---: | ---: |");
        for (k, r) in &b.initial_resources {
            let u = if r.unit.is_empty() {
                String::new()
            } else {
                format!(" {}", r.unit)
            };
            let _ = writeln!(out, "| {k} | {}{} | {}{} |", r.current, u, r.max, u);
        }
        let _ = writeln!(out);
    }

    if !b.initial_equipment.is_empty() {
        let _ = writeln!(out, "### Initial Equipment & Gear\n");
        let _ = writeln!(out, "| Item | Qty | Slot | Equipped | Details |");
        let _ = writeln!(out, "| :--- | ---: | :--- | :---: | :--- |");
        for item in &b.initial_equipment {
            let slot = item.slot.as_deref().unwrap_or("—");
            let eq = if item.equipped { "Yes" } else { "No" };
            let _ = writeln!(
                out,
                "| **{}** | {} | {} | {} | {} |",
                item.name,
                item.quantity,
                slot,
                eq,
                escape_pipe(&item.description)
            );
        }
        let _ = writeln!(out);
    }

    if !b.initial_skills.is_empty() {
        let _ = writeln!(out, "### Initial Skills\n");
        let _ = writeln!(out, "| Skill | Rank | Category | Modifier | Description |");
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- | :--- |");
        for s in &b.initial_skills {
            let _ = writeln!(
                out,
                "| **{}** | {} | {} | {} | {} |",
                s.name,
                s.rank,
                if s.category.is_empty() { "—" } else { &s.category },
                s.modifier.as_deref().unwrap_or("—"),
                escape_pipe(&s.description)
            );
        }
        let _ = writeln!(out);
    }

    if !b.initial_techniques.is_empty() {
        let _ = writeln!(out, "### Initial Techniques & Abilities\n");
        let _ = writeln!(out, "| Technique | Tier | Cost | Mastery | Effect |");
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- | :--- |");
        for t in &b.initial_techniques {
            let eff = if !t.effect.is_empty() {
                &t.effect
            } else {
                &t.description
            };
            let _ = writeln!(
                out,
                "| **{}** | {} | {} | {} | {} |",
                t.name,
                non_empty(&t.tier),
                non_empty(&t.cost),
                non_empty(&t.mastery),
                escape_pipe(eff)
            );
        }
        let _ = writeln!(out);
    }

    // =========================================================================
    // SECTION 2: CURRENT STATUS (DETERMINISTICALLY PROJECTED)
    // =========================================================================
    let _ = writeln!(out, "---\n");
    let _ = writeln!(out, "## 2. Current Status\n");
    let _ = writeln!(
        out,
        "> **State:** `{}` | **Current Location:** {} | **Last Updated:** {} (Event `#{}`)\n",
        c.alive_status,
        c.current_location
            .as_deref()
            .map(|l| link_from_subdir(l, "..", name_map))
            .unwrap_or_else(|| "*Unknown*".to_string()),
        c.last_world_time,
        c.last_event_seq
    );

    if !c.resources.is_empty() {
        let _ = writeln!(out, "### Vitals & Resources\n");
        let _ = writeln!(out, "| Resource | Current / Max | Bar |");
        let _ = writeln!(out, "| :--- | :--- | :--- |");
        for (k, r) in &c.resources {
            let bar = render_bar(r);
            let u = if r.unit.is_empty() {
                String::new()
            } else {
                format!(" {}", r.unit)
            };
            let _ = writeln!(out, "| **{k}** | `{}/{}`{} | `{bar}` |", r.current, r.max, u);
        }
        let _ = writeln!(out);
    }

    if !c.stats.is_empty() {
        let _ = writeln!(out, "### Current Attributes / Stats\n");
        let _ = writeln!(out, "| Attribute | Current | Change vs Initial |");
        let _ = writeln!(out, "| :--- | ---: | ---: |");
        for (k, v) in &c.stats {
            let base = b.initial_stats.get(k).copied().unwrap_or(0);
            let diff = v - base;
            let delta_str = if diff > 0 {
                format!("+{diff}")
            } else if diff < 0 {
                format!("{diff}")
            } else {
                "—".to_string()
            };
            let _ = writeln!(out, "| {k} | **{v}** | {delta_str} |");
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "### Active Status Effects & Conditions\n");
    if c.conditions.is_empty() {
        let _ = writeln!(out, "*None active.*\n");
    } else {
        let _ = writeln!(out, "| Condition | Kind | Severity | Duration | Source | Notes |");
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- | :--- | :--- |");
        for cond in &c.conditions {
            let _ = writeln!(
                out,
                "| **{}** | `{}` | {} | {} | {} | {} |",
                cond.name,
                cond.kind,
                non_empty(&cond.severity),
                non_empty(&cond.duration),
                non_empty(&cond.source),
                escape_pipe(&cond.notes)
            );
        }
        let _ = writeln!(out);
    }

    // Split Equipment into Equipped Gear & Carried Inventory
    let equipped: Vec<_> = c.equipment.iter().filter(|i| i.equipped).collect();
    let carried: Vec<_> = c.equipment.iter().filter(|i| !i.equipped).collect();

    let _ = writeln!(out, "### Current Equipment & Inventory\n");
    if !equipped.is_empty() {
        let _ = writeln!(out, "**Equipped Gear:**\n");
        let _ = writeln!(out, "| Slot | Item | Qty | Durability | Notes |");
        let _ = writeln!(out, "| :--- | :--- | ---: | :--- | :--- |");
        for item in &equipped {
            let _ = writeln!(
                out,
                "| {} | **{}** | {} | {} | {} |",
                item.slot.as_deref().unwrap_or("Equipped"),
                item.name,
                item.quantity,
                item.durability.as_deref().unwrap_or("—"),
                escape_pipe(&item.description)
            );
        }
        let _ = writeln!(out);
    }
    if !carried.is_empty() {
        let _ = writeln!(out, "**Carried Inventory:**\n");
        let _ = writeln!(out, "| Item | Qty | Tags | Notes |");
        let _ = writeln!(out, "| :--- | ---: | :--- | :--- |");
        for item in &carried {
            let tags = if item.tags.is_empty() {
                "—".to_string()
            } else {
                item.tags.join(", ")
            };
            let _ = writeln!(
                out,
                "| **{}** | {} | {} | {} |",
                item.name,
                item.quantity,
                tags,
                escape_pipe(&item.description)
            );
        }
        let _ = writeln!(out);
    }
    if equipped.is_empty() && carried.is_empty() {
        let _ = writeln!(out, "*No equipment or carried items.*\n");
    }

    if !c.skills.is_empty() {
        let _ = writeln!(out, "### Current Skills\n");
        let _ = writeln!(out, "| Skill | Rank | Category | Modifier | Description |");
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- | :--- |");
        for s in &c.skills {
            let _ = writeln!(
                out,
                "| **{}** | {} | {} | {} | {} |",
                s.name,
                s.rank,
                non_empty(&s.category),
                s.modifier.as_deref().unwrap_or("—"),
                escape_pipe(&s.description)
            );
        }
        let _ = writeln!(out);
    }

    if !c.techniques.is_empty() {
        let _ = writeln!(out, "### Current Techniques & Abilities\n");
        let _ = writeln!(out, "| Technique | Tier | Cost | Cooldown | Mastery | Effect |");
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- | :--- | :--- |");
        for t in &c.techniques {
            let eff = if !t.effect.is_empty() {
                &t.effect
            } else {
                &t.description
            };
            let _ = writeln!(
                out,
                "| **{}** | {} | {} | {} | {} | {} |",
                t.name,
                non_empty(&t.tier),
                non_empty(&t.cost),
                non_empty(&t.cooldown),
                non_empty(&t.mastery),
                escape_pipe(eff)
            );
        }
        let _ = writeln!(out);
    }

    if !c.active_goals.is_empty() || !c.completed_goals.is_empty() {
        let _ = writeln!(out, "### Goals & Objectives\n");
        for g in &c.active_goals {
            let _ = writeln!(out, "- [ ] {g}");
        }
        for g in &c.completed_goals {
            let _ = writeln!(out, "- [x] {g}");
        }
        let _ = writeln!(out);
    }

    if !relations.is_empty() {
        let _ = writeln!(out, "### Relationships & Knowledge Graph Links\n");
        let _ = writeln!(out, "| Relation | Target | Strength | Notes |");
        let _ = writeln!(out, "| :--- | :--- | ---: | :--- |");
        for r in relations {
            let (dir, other_id) = if r.source_id == b.id {
                (r.relation_type.clone(), r.target_id.clone())
            } else {
                (format!("← {}", r.relation_type), r.source_id.clone())
            };
            let strength = r
                .strength
                .map(|s| format!("{s:+}"))
                .unwrap_or_else(|| "—".to_string());
            let _ = writeln!(
                out,
                "| `{}` | {} | {} | {} |",
                dir,
                link_from_subdir(&other_id, "..", name_map),
                strength,
                escape_pipe(&r.notes)
            );
        }
        let _ = writeln!(out);
    }

    append_history_section(&mut out, histories, "..", name_map);

    // =========================================================================
    // SECTION 3: CHRONOLOGICAL EVENT & IMPACT LOG (AT THE END)
    // =========================================================================
    let _ = writeln!(out, "---\n");
    let _ = writeln!(out, "## 3. Event & Impact Log\n");
    if proj.event_log.is_empty() {
        let _ = writeln!(out, "*No recorded events yet.*\n");
    } else {
        let _ = writeln!(
            out,
            "| Seq | Session | In-World Time | Location | Event | Details |"
        );
        let _ = writeln!(out, "| ---: | ---: | :--- | :--- | :--- | :--- |");
        for ev in &proj.event_log {
            let loc = ev
                .location_id
                .as_deref()
                .map(|l| link_from_subdir(l, "..", name_map))
                .unwrap_or_else(|| "—".to_string());
            let _ = writeln!(
                out,
                "| `#{}` | S{} | {} | {} | **{}** | {} |",
                ev.seq,
                ev.session_number,
                ev.world_time,
                loc,
                event_kind_label(&ev.payload),
                escape_pipe(&ev.summary)
            );
        }
        let _ = writeln!(out);
    }

    out
}

pub fn render_place_markdown(
    proj: &PlaceProjection,
    relations: &[GraphRelation],
    histories: &[EntityHistoryEntry],
    name_map: &BTreeMap<String, (String, String)>,
) -> String {
    let b = &proj.baseline;
    let c = &proj.current;
    let mut out = String::new();

    let _ = writeln!(out, "---");
    let _ = writeln!(out, "id: {}", b.id);
    let _ = writeln!(out, "type: place");
    let _ = writeln!(out, "place_type: {}", b.place_type);
    if let Some(ref parent) = b.parent_place {
        let _ = writeln!(out, "parent_place: {parent}");
    }
    let _ = writeln!(out, "season: {}", c.current_season);
    if !b.tags.is_empty() {
        let _ = writeln!(out, "tags: [{}]", b.tags.join(", "));
    }
    let _ = writeln!(out, "last_event_seq: {}", c.last_event_seq);
    let _ = writeln!(out, "---\n");

    let _ = writeln!(out, "# {}\n", b.name);
    let _ = writeln!(
        out,
        "> **Place ID:** `{}` | **Type:** `{}` | [Campaign Index](../INDEX.md) | [Master Timeline](../TIMELINE.md)\n",
        b.id, b.place_type
    );

    // =========================================================================
    // SECTION 1: BASELINE GEOGRAPHY & SIGNIFICANCE
    // =========================================================================
    let _ = writeln!(out, "## 1. Baseline Geography & Significance\n");
    let _ = writeln!(out, "| Attribute | Baseline Detail |");
    let _ = writeln!(out, "| :--- | :--- |");
    if let Some(ref parent) = b.parent_place {
        let _ = writeln!(
            out,
            "| **Parent Region** | {} |",
            link_from_subdir(parent, "..", name_map)
        );
    }
    let _ = writeln!(out, "| **Terrain** | {} |", non_empty(&b.terrain));
    let _ = writeln!(out, "| **Climate** | {} |", non_empty(&b.climate));
    if let Some(ref f) = b.initial_faction {
        let _ = writeln!(out, "| **Initial Faction / Authority** | {f} |");
    }
    if !b.initial_danger_level.is_empty() {
        let _ = writeln!(out, "| **Baseline Danger Level** | {} |", b.initial_danger_level);
    }
    let _ = writeln!(out);

    if !b.description.is_empty() {
        let _ = writeln!(out, "{}\n", b.description);
    }
    if !b.notes.is_empty() {
        let _ = writeln!(out, "**GM Notes:** {}\n", b.notes);
    }

    if !b.seasonal_profiles.is_empty() {
        let _ = writeln!(out, "### Seasonal Profiles\n");
        let _ = writeln!(out, "| Season | Terrain & Environmental Effects |");
        let _ = writeln!(out, "| :--- | :--- |");
        for (season, desc) in &b.seasonal_profiles {
            let active_mark = if season.eq_ignore_ascii_case(&c.current_season) {
                " *(Current)*"
            } else {
                ""
            };
            let _ = writeln!(
                out,
                "| **{}**{} | {} |",
                season,
                active_mark,
                escape_pipe(desc)
            );
        }
        let _ = writeln!(out);
    }

    if !b.places_of_significance.is_empty() {
        let _ = writeln!(out, "### Baseline Places of Significance (Landmarks)\n");
        let _ = writeln!(out, "| Landmark | Kind | Initial Status | Description |");
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- |");
        for l in &b.places_of_significance {
            let _ = writeln!(
                out,
                "| **{}** | {} | {} | {} |",
                l.name,
                non_empty(&l.kind),
                l.status,
                escape_pipe(&l.description)
            );
        }
        let _ = writeln!(out);
    }

    // =========================================================================
    // SECTION 2: CURRENT STATUS
    // =========================================================================
    let _ = writeln!(out, "---\n");
    let _ = writeln!(out, "## 2. Current Status\n");
    let _ = writeln!(
        out,
        "> **Season:** `{}` | **Weather:** {} | **Danger:** {} | **Controlling Faction:** {} | **Last Updated:** {} (Event `#{}`)\n",
        c.current_season,
        non_empty(&c.current_weather),
        non_empty(&c.danger_level),
        c.controlling_faction.as_deref().unwrap_or("Unclaimed"),
        c.last_world_time,
        c.last_event_seq
    );

    // Highlight current seasonal terrain note if defined
    if let Some((_, seasonal_note)) = b
        .seasonal_profiles
        .iter()
        .find(|(s, _)| s.eq_ignore_ascii_case(&c.current_season))
    {
        let _ = writeln!(
            out,
            "**Active Seasonal Condition ({}):** {}\n",
            c.current_season, seasonal_note
        );
    }

    let _ = writeln!(out, "### Active Local Conditions & Modifiers\n");
    if c.active_conditions.is_empty() {
        let _ = writeln!(out, "*No special active conditions.*\n");
    } else {
        for cond in &c.active_conditions {
            let _ = writeln!(out, "- {cond}");
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "### Characters Currently Present\n");
    if c.present_characters.is_empty() {
        let _ = writeln!(out, "*No tracked characters currently here.*\n");
    } else {
        for cid in &c.present_characters {
            let _ = writeln!(out, "- {}", link_from_subdir(cid, "..", name_map));
        }
        let _ = writeln!(out);
    }

    if !c.landmarks.is_empty() {
        let _ = writeln!(out, "### Current Status of Places of Significance\n");
        let _ = writeln!(out, "| Landmark | Kind | Current Status | Notes |");
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- |");
        for l in &c.landmarks {
            let _ = writeln!(
                out,
                "| **{}** | {} | `{}` | {} |",
                l.name,
                non_empty(&l.kind),
                l.status,
                escape_pipe(&l.description)
            );
        }
        let _ = writeln!(out);
    }

    if !c.routes.is_empty() {
        let _ = writeln!(out, "### Connected Routes & Exits\n");
        let _ = writeln!(out, "| Destination | Travel Time | Difficulty | Status | Notes |");
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- | :--- |");
        for r in &c.routes {
            let _ = writeln!(
                out,
                "| {} | {} | {} | `{}` | {} |",
                link_from_subdir(&r.destination_id, "..", name_map),
                non_empty(&r.travel_time),
                non_empty(&r.terrain_difficulty),
                r.status,
                escape_pipe(&r.notes)
            );
        }
        let _ = writeln!(out);
    }

    if !relations.is_empty() {
        let _ = writeln!(out, "### Knowledge Graph Links\n");
        let _ = writeln!(out, "| Relation | Entity | Notes |");
        let _ = writeln!(out, "| :--- | :--- | :--- |");
        for r in relations {
            let (dir, other_id) = if r.source_id == b.id {
                (r.relation_type.clone(), r.target_id.clone())
            } else {
                (format!("← {}", r.relation_type), r.source_id.clone())
            };
            let _ = writeln!(
                out,
                "| `{}` | {} | {} |",
                dir,
                link_from_subdir(&other_id, "..", name_map),
                escape_pipe(&r.notes)
            );
        }
        let _ = writeln!(out);
    }

    append_history_section(&mut out, histories, "..", name_map);

    // =========================================================================
    // SECTION 3: PLACE EVENT & HISTORY LOG (AT THE END)
    // =========================================================================
    let _ = writeln!(out, "---\n");
    let _ = writeln!(out, "## 3. Place Event & History Log\n");
    if proj.event_log.is_empty() {
        let _ = writeln!(out, "*No recorded events at this place yet.*\n");
    } else {
        let _ = writeln!(out, "| Seq | Session | In-World Time | Event | Summary |");
        let _ = writeln!(out, "| ---: | ---: | :--- | :--- | :--- |");
        for ev in &proj.event_log {
            let _ = writeln!(
                out,
                "| `#{}` | S{} | {} | **{}** | {} |",
                ev.seq,
                ev.session_number,
                ev.world_time,
                event_kind_label(&ev.payload),
                escape_pipe(&ev.summary)
            );
        }
        let _ = writeln!(out);
    }

    out
}

pub fn render_lore_markdown(
    lore: &LoreEntry,
    relations: &[GraphRelation],
    histories: &[EntityHistoryEntry],
    events: &[WorldEvent],
    name_map: &BTreeMap<String, (String, String)>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "---");
    let _ = writeln!(out, "id: {}", lore.id);
    let _ = writeln!(out, "type: lore");
    let _ = writeln!(out, "category: {}", lore.category);
    let _ = writeln!(out, "visibility: {}", lore.visibility);
    let _ = writeln!(out, "always_include: {}", lore.always_include);
    if !lore.keywords.is_empty() {
        let _ = writeln!(out, "keywords: [{}]", lore.keywords.join(", "));
    }
    if !lore.tags.is_empty() {
        let _ = writeln!(out, "tags: [{}]", lore.tags.join(", "));
    }
    let _ = writeln!(out, "---\n");

    let _ = writeln!(out, "# {}\n", lore.title);
    let _ = writeln!(
        out,
        "> **Lore ID:** `{}` | **Category:** `{}` | **Visibility:** `{}` | [Campaign Index](../../INDEX.md) | [Master Timeline](../../TIMELINE.md)\n",
        lore.id, lore.category, lore.visibility
    );

    if !lore.summary.is_empty() {
        let _ = writeln!(out, "## Summary\n\n{}\n", lore.summary);
    }

    let _ = writeln!(out, "## World-Building & Reference\n\n{}\n", lore.body);

    if !lore.keywords.is_empty() {
        let _ = writeln!(
            out,
            "**Auto-Trigger Keywords:** {}\n",
            lore.keywords
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    if !lore.related_ids.is_empty() || !relations.is_empty() {
        let _ = writeln!(out, "## Cross-References\n");
        for rid in &lore.related_ids {
            let _ = writeln!(out, "- {}", link_from_subdir(rid, "../..", name_map));
        }
        for r in relations {
            let (dir, other_id) = if r.source_id == lore.id {
                (r.relation_type.clone(), r.target_id.clone())
            } else {
                (format!("← {}", r.relation_type), r.source_id.clone())
            };
            let _ = writeln!(
                out,
                "- `{}` {} — {}",
                dir,
                link_from_subdir(&other_id, "../..", name_map),
                r.notes
            );
        }
        let _ = writeln!(out);
    }

    append_history_section(&mut out, histories, "../..", name_map);

    let _ = writeln!(out, "---\n");
    let _ = writeln!(out, "## Chronicle & Discovery Log\n");
    if events.is_empty() {
        let _ = writeln!(
            out,
            "*Recorded at {} (no subsequent campaign events referencing this lore yet).*\n",
            lore.created_at_world_time
        );
    } else {
        for ev in events {
            let _ = writeln!(
                out,
                "- `[#{}]` **{}** — {}",
                ev.seq, ev.world_time, ev.summary
            );
        }
        let _ = writeln!(out);
    }

    out
}

pub fn render_text_markdown(
    t: &StoryText,
    relations: &[GraphRelation],
    histories: &[EntityHistoryEntry],
    events: &[WorldEvent],
    name_map: &BTreeMap<String, (String, String)>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "---");
    let _ = writeln!(out, "id: {}", t.id);
    let _ = writeln!(out, "type: text");
    let _ = writeln!(out, "medium: {}", t.medium);
    let _ = writeln!(out, "subject: {}", t.subject);
    let _ = writeln!(out, "language: {}", t.language);
    let _ = writeln!(out, "condition: {}", t.condition);
    let _ = writeln!(out, "read_status: {}", t.read_status);
    if let Some(ref loc) = t.holder_or_location_id {
        let _ = writeln!(out, "holder_or_location: {loc}");
    }
    if !t.read_by.is_empty() {
        let _ = writeln!(out, "read_by: [{}]", t.read_by.join(", "));
    }
    if !t.keywords.is_empty() {
        let _ = writeln!(out, "keywords: [{}]", t.keywords.join(", "));
    }
    if !t.tags.is_empty() {
        let _ = writeln!(out, "tags: [{}]", t.tags.join(", "));
    }
    let _ = writeln!(out, "---\n");

    let _ = writeln!(out, "# {}\n", t.title);
    let _ = writeln!(
        out,
        "> **Text ID:** `{}` | **Medium:** `{}` | **Subject:** `{}` | **Status:** `{}` | [Campaign Index](../INDEX.md) | [Master Timeline](../TIMELINE.md)\n",
        t.id, t.medium, t.subject, t.read_status
    );

    let _ = writeln!(out, "## 1. Artifact & Bibliographic Profile\n");
    let _ = writeln!(out, "| Field | Value |");
    let _ = writeln!(out, "| :--- | :--- |");
    let _ = writeln!(out, "| **Medium** | {} |", t.medium);
    let _ = writeln!(out, "| **Subject** | {} |", t.subject);
    let _ = writeln!(out, "| **Author** | {} |", non_empty(&t.author));
    let _ = writeln!(out, "| **Language / Script** | {} |", t.language);
    let _ = writeln!(out, "| **Physical Condition** | {} |", t.condition);
    let _ = writeln!(out, "| **Read Status** | `{}` |", t.read_status);
    let holder = t
        .holder_or_location_id
        .as_deref()
        .map(|h| link_from_subdir(h, "..", name_map))
        .unwrap_or_else(|| "*Unknown*".to_string());
    let _ = writeln!(out, "| **Current Holder / Location** | {holder} |");
    let readers = if t.read_by.is_empty() {
        "*Not yet read by any tracked character*".to_string()
    } else {
        t.read_by
            .iter()
            .map(|r| link_from_subdir(r, "..", name_map))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let _ = writeln!(out, "| **Read / Studied By** | {readers} |\n");

    if !t.summary.is_empty() {
        let _ = writeln!(out, "**Outward Appearance / Overview:** {}\n", t.summary);
    }

    let _ = writeln!(out, "---\n");
    let _ = writeln!(out, "## 2. Readable In-World Text & Passages\n");
    let _ = writeln!(out, "{}\n", t.content);

    if !t.mechanics_or_unlocks.is_empty() {
        let _ = writeln!(out, "## 3. Story Revelations, Rituals & Unlocks\n");
        for u in &t.mechanics_or_unlocks {
            let _ = writeln!(out, "- {u}");
        }
        let _ = writeln!(out);
    }

    if !t.gm_notes.is_empty() {
        let _ = writeln!(out, "> **GM Notes (Hidden Context):** {}\n", t.gm_notes);
    }

    if !t.keywords.is_empty() {
        let _ = writeln!(
            out,
            "**Auto-Trigger Keywords:** {}\n",
            t.keywords
                .iter()
                .map(|k| format!("`{k}`"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    if !t.related_ids.is_empty() || !relations.is_empty() {
        let _ = writeln!(out, "### Cross-References\n");
        for rid in &t.related_ids {
            let _ = writeln!(out, "- {}", link_from_subdir(rid, "..", name_map));
        }
        for r in relations {
            let (dir, other_id) = if r.source_id == t.id {
                (r.relation_type.clone(), r.target_id.clone())
            } else {
                (format!("← {}", r.relation_type), r.source_id.clone())
            };
            let _ = writeln!(
                out,
                "- `{}` {} — {}",
                dir,
                link_from_subdir(&other_id, "..", name_map),
                r.notes
            );
        }
        let _ = writeln!(out);
    }

    append_history_section(&mut out, histories, "..", name_map);

    let _ = writeln!(out, "---\n");
    let _ = writeln!(out, "## 4. Provenance & Reading Log\n");
    if events.is_empty() {
        let _ = writeln!(
            out,
            "*Cataloged at {} (no discovery or reading events logged yet).*\n",
            t.created_at_world_time
        );
    } else {
        for ev in events {
            let _ = writeln!(
                out,
                "- `[#{}]` **{}** — {}",
                ev.seq, ev.world_time, ev.summary
            );
        }
        let _ = writeln!(out);
    }

    out
}

pub fn render_item_markdown(
    it: &CatalogItem,
    relations: &[GraphRelation],
    histories: &[EntityHistoryEntry],
    events: &[WorldEvent],
    name_map: &BTreeMap<String, (String, String)>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "---");
    let _ = writeln!(out, "id: {}", it.id);
    let _ = writeln!(out, "type: item");
    let _ = writeln!(out, "item_type: {}", it.item_type);
    if !it.rarity_or_tier.is_empty() {
        let _ = writeln!(out, "rarity: {}", it.rarity_or_tier);
    }
    if let Some(ref h) = it.current_holder_id {
        let _ = writeln!(out, "current_holder: {h}");
    }
    if let Some(ref tid) = it.linked_text_id {
        let _ = writeln!(out, "linked_text: {tid}");
    }
    if !it.tags.is_empty() {
        let _ = writeln!(out, "tags: [{}]", it.tags.join(", "));
    }
    let _ = writeln!(out, "---\n");

    let _ = writeln!(out, "# {}\n", it.name);
    let _ = writeln!(
        out,
        "> **Item ID:** `{}` | **Type:** `{}` | **Condition:** `{}` | [Campaign Index](../INDEX.md) | [Master Timeline](../TIMELINE.md)\n",
        it.id, it.item_type, it.condition_or_durability
    );

    let _ = writeln!(out, "## 1. Artifact, Ownership & Significance\n");
    let _ = writeln!(out, "| Field | Value |");
    let _ = writeln!(out, "| :--- | :--- |");
    let _ = writeln!(out, "| **Item Type** | `{}` |", it.item_type);
    if !it.rarity_or_tier.is_empty() {
        let _ = writeln!(out, "| **Rarity / Tier** | {} |", it.rarity_or_tier);
    }
    if let Some(ref slot) = it.slot {
        let _ = writeln!(out, "| **Equip Slot** | {slot} |");
    }
    let _ = writeln!(out, "| **Quantity** | {} |", it.quantity);
    let _ = writeln!(out, "| **Condition / Durability** | {} |", it.condition_or_durability);
    let _ = writeln!(
        out,
        "| **Origin & Maker** | {} |",
        non_empty(&it.origin_and_maker)
    );
    let holder = it
        .current_holder_id
        .as_deref()
        .map(|h| link_from_subdir(h, "..", name_map))
        .unwrap_or_else(|| "*Unknown / Unclaimed*".to_string());
    let _ = writeln!(
        out,
        "| **Current Holder / Location** | {}{} |",
        holder,
        if it.equipped { " *(Equipped)*" } else { "" }
    );
    let prev = if it.previous_owners.is_empty() {
        "*None recorded*".to_string()
    } else {
        it.previous_owners
            .iter()
            .map(|o| {
                let slug = slugify(o);
                if name_map.contains_key(&slug) {
                    link_from_subdir(&slug, "..", name_map)
                } else {
                    o.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" → ")
    };
    let _ = writeln!(out, "| **Previous Owners** | {prev} |");
    if let Some(ref tid) = it.linked_text_id {
        let _ = writeln!(
            out,
            "| **Readable Text / Contents** | {} |",
            link_from_subdir(tid, "..", name_map)
        );
    }
    let _ = writeln!(out);

    if !it.description.is_empty() {
        let _ = writeln!(out, "**Appearance & Description:** {}\n", it.description);
    }
    if !it.significance.is_empty() {
        let _ = writeln!(out, "**Story Significance:** {}\n", it.significance);
    }
    if !it.properties_and_effects.is_empty() {
        let _ = writeln!(
            out,
            "**Properties, Mechanics & Uses:** {}\n",
            it.properties_and_effects
        );
    }
    if !it.gm_notes.is_empty() {
        let _ = writeln!(out, "> **GM Notes (Hidden):** {}\n", it.gm_notes);
    }

    append_history_section(&mut out, histories, "..", name_map);

    if !it.related_ids.is_empty() || !relations.is_empty() {
        let _ = writeln!(out, "## Cross-References\n");
        for rid in &it.related_ids {
            let _ = writeln!(out, "- {}", link_from_subdir(rid, "..", name_map));
        }
        for r in relations {
            let (dir, other_id) = if r.source_id == it.id {
                (r.relation_type.clone(), r.target_id.clone())
            } else {
                (format!("← {}", r.relation_type), r.source_id.clone())
            };
            let _ = writeln!(
                out,
                "- `{}` {} — {}",
                dir,
                link_from_subdir(&other_id, "..", name_map),
                r.notes
            );
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "---\n");
    let _ = writeln!(out, "## Live Provenance & Event Log\n");
    if events.is_empty() {
        let _ = writeln!(out, "*Cataloged at {}.*\n", it.created_at_world_time);
    } else {
        for ev in events {
            let _ = writeln!(
                out,
                "- `[#{}]` **{}** — {}",
                ev.seq, ev.world_time, ev.summary
            );
        }
        let _ = writeln!(out);
    }
    out
}

fn append_history_section(
    out: &mut String,
    histories: &[EntityHistoryEntry],
    prefix: &str,
    name_map: &BTreeMap<String, (String, String)>,
) {
    if histories.is_empty() {
        return;
    }
    let _ = writeln!(out, "---\n");
    let _ = writeln!(out, "## Historical Chronicle\n");
    for h in histories {
        let vis = if h.visibility == "gm_secret" {
            " *(GM Secret)*"
        } else if h.visibility == "rumor" {
            " *(Rumor)*"
        } else {
            ""
        };
        let era = if h.era_or_date.is_empty() {
            "Undated".to_string()
        } else {
            h.era_or_date.clone()
        };
        let _ = writeln!(out, "### {} — {}{}\n", era, h.title, vis);
        let _ = writeln!(out, "{}\n", h.details);
        if !h.significance.is_empty() {
            let _ = writeln!(out, "**Significance:** {}\n", h.significance);
        }
        if !h.related_ids.is_empty() {
            let links = h
                .related_ids
                .iter()
                .map(|r| link_from_subdir(r, prefix, name_map))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "**Related:** {links}\n");
        }
    }
}

pub fn render_attachment_markdown(
    a: &StoryAttachment,
    relations: &[GraphRelation],
    events: &[WorldEvent],
    name_map: &BTreeMap<String, (String, String)>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "---");
    let _ = writeln!(out, "id: {}", a.id);
    let _ = writeln!(out, "type: attachment");
    let _ = writeln!(out, "category: {}", a.category);
    let _ = writeln!(out, "visibility: {}", a.visibility);
    let _ = writeln!(out, "mime_type: {}", a.mime_type);
    let _ = writeln!(out, "file: {}", a.stored_filename);
    let _ = writeln!(out, "relative_path: {}", a.relative_path);
    let _ = writeln!(out, "sha256: {}", a.sha256);
    if !a.keywords.is_empty() {
        let _ = writeln!(out, "keywords: [{}]", a.keywords.join(", "));
    }
    if !a.tags.is_empty() {
        let _ = writeln!(out, "tags: [{}]", a.tags.join(", "));
    }
    let _ = writeln!(out, "---\n");

    let _ = writeln!(out, "# {}\n", a.title);
    let _ = writeln!(
        out,
        "> **Attachment ID:** `{}` | **Category:** `{}` | **MCP URI:** `chronicle://attachments/{}` | [Open File](files/{}) | [Campaign Index](../INDEX.md)\n",
        a.id, a.category, a.id, a.stored_filename
    );

    let _ = writeln!(out, "## 1. Addressable File Reference\n");
    let _ = writeln!(out, "| Field | Value |");
    let _ = writeln!(out, "| :--- | :--- |");
    let _ = writeln!(out, "| **Copied File** | [{}](files/{}) |", a.stored_filename, a.stored_filename);
    let _ = writeln!(out, "| **Relative Path** | `{}` |", a.relative_path);
    let _ = writeln!(out, "| **Original Filename** | `{}` |", a.original_filename);
    let _ = writeln!(out, "| **MIME Type** | `{}` |", a.mime_type);
    let _ = writeln!(out, "| **Size** | {} bytes |", a.size_bytes);
    let _ = writeln!(out, "| **SHA-256** | `{}` |", a.sha256);
    let _ = writeln!(out, "| **Visibility** | `{}` |", a.visibility);
    if !a.source_origin.is_empty() {
        let _ = writeln!(out, "| **Origin** | {} |", escape_pipe(&a.source_origin));
    }
    let _ = writeln!(out);

    if a.mime_type.starts_with("image/") {
        let _ = writeln!(out, "![{}](files/{})\n", a.title, a.stored_filename);
    }

    if !a.summary.is_empty() {
        let _ = writeln!(out, "## 2. Summary & Story Relevance\n\n{}\n", a.summary);
    }

    if !a.extracted_notes.is_empty() {
        let _ = writeln!(
            out,
            "## 3. Extracted Notes & Searchable Key Facts\n\n{}\n",
            a.extracted_notes
        );
    }

    if !a.related_ids.is_empty() || !relations.is_empty() {
        let _ = writeln!(out, "## Cross-References\n");
        for rid in &a.related_ids {
            let _ = writeln!(out, "- {}", link_from_subdir(rid, "..", name_map));
        }
        for r in relations {
            let (dir, other_id) = if r.source_id == a.id {
                (r.relation_type.clone(), r.target_id.clone())
            } else {
                (format!("← {}", r.relation_type), r.source_id.clone())
            };
            let _ = writeln!(
                out,
                "- `{}` {} — {}",
                dir,
                link_from_subdir(&other_id, "..", name_map),
                r.notes
            );
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "---\n");
    let _ = writeln!(out, "## Provenance Log\n");
    if events.is_empty() {
        let _ = writeln!(out, "*Attached at {}.*\n", a.created_at_world_time);
    } else {
        for ev in events {
            let _ = writeln!(
                out,
                "- `[#{}]` **{}** — {}",
                ev.seq, ev.world_time, ev.summary
            );
        }
        let _ = writeln!(out);
    }

    out
}

#[allow(clippy::too_many_arguments)]
fn render_index_markdown(
    meta: &crate::models::CampaignMeta,
    characters: &BTreeMap<String, CharacterProjection>,
    places: &BTreeMap<String, PlaceProjection>,
    lore: &[LoreEntry],
    texts: &[StoryText],
    items: &[CatalogItem],
    attachments: &[StoryAttachment],
    clocks: &[crate::models::ProgressClock],
    chain_valid: bool,
    event_count: usize,
    name_map: &BTreeMap<String, (String, String)>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Campaign Codex: {}\n", meta.name);
    if !meta.description.is_empty() {
        let _ = writeln!(out, "{}\n", meta.description);
    }

    let chain_badge = if chain_valid {
        format!("Verified ({event_count} events)")
    } else {
        format!("INTEGRITY WARNING ({event_count} events)")
    };

    let _ = writeln!(out, "## World State Snapshot\n");
    let _ = writeln!(out, "| Field | Current Value |");
    let _ = writeln!(out, "| :--- | :--- |");
    let _ = writeln!(
        out,
        "| **In-World Time** | {} ({}) |",
        meta.clock.format_short(),
        meta.clock.time_of_day()
    );
    let _ = writeln!(out, "| **Season** | {} |", meta.clock.season);
    let _ = writeln!(
        out,
        "| **Global Weather** | {} |",
        non_empty(&meta.clock.global_weather)
    );
    let _ = writeln!(out, "| **Current Session** | Session {} |", meta.current_session);
    let _ = writeln!(out, "| **Event Ledger** | [TIMELINE.md](TIMELINE.md) — {chain_badge} |\n");

    if !meta.ruleset_notes.is_empty() {
        let _ = writeln!(out, "**Ruleset / System Notes:** {}\n", meta.ruleset_notes);
    }

    // Active Progress Clocks / Quests
    let _ = writeln!(out, "## Active Clocks, Quests & Threats\n");
    if clocks.is_empty() {
        let _ = writeln!(out, "*No progress clocks tracked yet.*\n");
    } else {
        let _ = writeln!(out, "| Clock | Kind | Progress | Status | Description |");
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- | :--- |");
        for cl in clocks {
            let bar = render_clock_bar(cl.filled, cl.segments);
            let _ = writeln!(
                out,
                "| **{}** (`{}`) | {} | `{}/{}` `{}` | `{}` | {} |",
                cl.name,
                cl.id,
                cl.kind,
                cl.filled,
                cl.segments,
                bar,
                cl.status,
                escape_pipe(&cl.description)
            );
        }
        let _ = writeln!(out);
    }

    // Characters Directory
    let _ = writeln!(out, "## Characters\n");
    if characters.is_empty() {
        let _ = writeln!(out, "*No characters created yet.*\n");
    } else {
        let _ = writeln!(
            out,
            "| Character | Kind | Status | Current Location | Active Conditions |"
        );
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- | :--- |");
        for (id, cp) in characters {
            let loc = cp
                .current
                .current_location
                .as_deref()
                .map(|l| link_from_root(l, name_map))
                .unwrap_or_else(|| "—".to_string());
            let conds = if cp.current.conditions.is_empty() {
                "—".to_string()
            } else {
                cp.current
                    .conditions
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let _ = writeln!(
                out,
                "| [{}](characters/{}.md) | `{}` | `{}` | {} | {} |",
                cp.baseline.name, id, cp.baseline.kind, cp.current.alive_status, loc, conds
            );
        }
        let _ = writeln!(out);
    }

    // Places Directory
    let _ = writeln!(out, "## Places & Geography\n");
    if places.is_empty() {
        let _ = writeln!(out, "*No places created yet.*\n");
    } else {
        let _ = writeln!(
            out,
            "| Place | Type | Terrain | Weather | Present Characters |"
        );
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- | :--- |");
        for (id, pp) in places {
            let present = if pp.current.present_characters.is_empty() {
                "—".to_string()
            } else {
                pp.current
                    .present_characters
                    .iter()
                    .map(|cid| link_from_root(cid, name_map))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let _ = writeln!(
                out,
                "| [{}](places/{}.md) | `{}` | {} | {} | {} |",
                pp.baseline.name,
                id,
                pp.baseline.place_type,
                escape_pipe(non_empty(&pp.baseline.terrain)),
                escape_pipe(non_empty(&pp.current.current_weather)),
                present
            );
        }
        let _ = writeln!(out);
    }

    // World-Building & Lore Directory (grouped by category)
    let _ = writeln!(out, "## World-Building & Lore\n");
    if lore.is_empty() {
        let _ = writeln!(out, "*No world-building lore entries yet.*\n");
    } else {
        let mut by_cat: BTreeMap<String, Vec<&LoreEntry>> = BTreeMap::new();
        for l in lore {
            by_cat.entry(l.category.clone()).or_default().push(l);
        }
        for (cat, items) in by_cat {
            let _ = writeln!(out, "### {cat}\n");
            for l in items {
                let cat_slug = slugify(&l.category);
                let vis = if l.visibility == "gm_secret" {
                    " *(GM Secret)*"
                } else if l.visibility == "rumor" {
                    " *(Rumor)*"
                } else {
                    ""
                };
                let summary = if l.summary.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", l.summary)
                };
                let _ = writeln!(
                    out,
                    "- [{}](lore/{}/{}.md){}{}",
                    l.title, cat_slug, l.id, vis, summary
                );
            }
            let _ = writeln!(out);
        }
    }

    // Books, Scrolls & Story Texts Directory
    let _ = writeln!(out, "## Books, Scrolls & Story Texts\n");
    if texts.is_empty() {
        let _ = writeln!(out, "*No books, scrolls, or in-world texts cataloged yet.*\n");
    } else {
        let _ = writeln!(
            out,
            "| Title | Medium | Subject | Language | Status | Holder / Location |"
        );
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- | :--- | :--- |");
        for t in texts {
            let holder = t
                .holder_or_location_id
                .as_deref()
                .map(|h| link_from_root(h, name_map))
                .unwrap_or_else(|| "—".to_string());
            let _ = writeln!(
                out,
                "| [{}](texts/{}.md) | `{}` | {} | {} | `{}` | {} |",
                t.title, t.id, t.medium, t.subject, t.language, t.read_status, holder
            );
        }
        let _ = writeln!(out);
    }

    // Carryable Items, Tools, Artifacts, Keepsakes & Books/Manuals
    let _ = writeln!(out, "## Carryable Items, Artifacts, Tools & Keepsakes\n");
    if items.is_empty() {
        let _ = writeln!(out, "*No cataloged carryable items yet.*\n");
    } else {
        let _ = writeln!(
            out,
            "| Item | Type | Condition | Current Holder | Previous Owners | Significance |"
        );
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- | :--- | :--- |");
        for it in items {
            let holder = it
                .current_holder_id
                .as_deref()
                .map(|h| link_from_root(h, name_map))
                .unwrap_or_else(|| "—".to_string());
            let prev = if it.previous_owners.is_empty() {
                "—".to_string()
            } else {
                it.previous_owners.join(" → ")
            };
            let _ = writeln!(
                out,
                "| [{}](items/{}.md) | `{}` | {} | {} | {} | {} |",
                it.name,
                it.id,
                it.item_type,
                it.condition_or_durability,
                holder,
                escape_pipe(&prev),
                escape_pipe(&it.significance)
            );
        }
        let _ = writeln!(out);
    }

    // Attached Reference Docs & Media Directory
    let _ = writeln!(out, "## Attached Reference Docs & Media\n");
    if attachments.is_empty() {
        let _ = writeln!(out, "*No attached reference documents or media files copied yet.*\n");
    } else {
        let _ = writeln!(
            out,
            "| Title | Category | File | Type | Visibility | Summary |"
        );
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- | :--- | :--- |");
        for a in attachments {
            let _ = writeln!(
                out,
                "| [{}](attachments/{}.md) | `{}` | [{}](attachments/files/{}) | `{}` | `{}` | {} |",
                a.title,
                a.id,
                a.category,
                a.stored_filename,
                a.stored_filename,
                a.mime_type,
                a.visibility,
                escape_pipe(&a.summary)
            );
        }
        let _ = writeln!(out);
    }

    out
}

fn render_timeline_markdown(
    meta: &crate::models::CampaignMeta,
    events: &[WorldEvent],
    chain_valid: bool,
    name_map: &BTreeMap<String, (String, String)>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Master Campaign Timeline: {}\n", meta.name);
    let _ = writeln!(
        out,
        "> [Campaign Index](INDEX.md) | **Total Events:** {} | **Ledger Hash Chain:** {}\n",
        events.len(),
        if chain_valid { "VERIFIED" } else { "FAILED" }
    );

    let mut current_session = 0u32;
    for ev in events {
        if ev.session_number != current_session {
            current_session = ev.session_number;
            let _ = writeln!(out, "\n## Session {current_session}\n");
            let _ = writeln!(
                out,
                "| Seq | In-World Time | Location | Type | Summary | Affected |"
            );
            let _ = writeln!(out, "| ---: | :--- | :--- | :--- | :--- | :--- |");
        }
        let loc = ev
            .location_id
            .as_deref()
            .map(|l| link_from_root(l, name_map))
            .unwrap_or_else(|| "—".to_string());
        let affected = if ev.affected_entities.is_empty() {
            "—".to_string()
        } else {
            ev.affected_entities
                .iter()
                .map(|id| link_from_root(id, name_map))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = writeln!(
            out,
            "| `#{}` | {} | {} | `{}` | {} | {} |",
            ev.seq,
            ev.world_time,
            loc,
            event_kind_label(&ev.payload),
            escape_pipe(&ev.summary),
            affected
        );
    }
    let _ = writeln!(out);
    out
}

fn link_from_root(id: &str, name_map: &BTreeMap<String, (String, String)>) -> String {
    if let Some((name, path)) = name_map.get(id) {
        format!("[{name}]({path})")
    } else {
        format!("`{id}`")
    }
}

fn link_from_subdir(
    id: &str,
    prefix: &str,
    name_map: &BTreeMap<String, (String, String)>,
) -> String {
    if let Some((name, path)) = name_map.get(id) {
        format!("[{name}]({prefix}/{path})")
    } else {
        format!("`{id}`")
    }
}

fn render_bar(r: &ResourcePool) -> String {
    let width = 10usize;
    if r.max <= 0 {
        return "──────────".to_string();
    }
    let ratio = (r.current as f64 / r.max as f64).clamp(0.0, 1.0);
    let filled = (ratio * width as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn render_clock_bar(filled: i64, segments: i64) -> String {
    let segs = segments.max(1).min(20) as usize;
    let f = filled.clamp(0, segs as i64) as usize;
    format!("{}{}", "●".repeat(f), "○".repeat(segs - f))
}

fn non_empty(s: &str) -> &str {
    if s.trim().is_empty() {
        "—"
    } else {
        s
    }
}

fn escape_pipe(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

pub fn event_kind_label(p: &EventPayload) -> &'static str {
    match p {
        EventPayload::CampaignStarted { .. } => "Campaign Started",
        EventPayload::SessionMarker { .. } => "Session Marker",
        EventPayload::TimeAdvanced { .. } => "Time Advanced",
        EventPayload::CharacterCreated { .. } => "Character Created",
        EventPayload::CharacterMoved { .. } => "Movement",
        EventPayload::ResourceChanged { .. } => "Resource",
        EventPayload::StatChanged { .. } => "Attribute",
        EventPayload::CharacterStatusChanged { .. } => "Status",
        EventPayload::ConditionAdded { .. } => "+Condition",
        EventPayload::ConditionRemoved { .. } => "-Condition",
        EventPayload::ItemAcquired { .. } => "+Item",
        EventPayload::ItemRemoved { .. } => "-Item",
        EventPayload::ItemEquipped { .. } => "Equipment",
        EventPayload::SkillUpdated { .. } => "Skill",
        EventPayload::TechniqueUpdated { .. } => "Technique",
        EventPayload::GoalUpdated { .. } => "Goal",
        EventPayload::PlaceCreated { .. } => "Place Created",
        EventPayload::PlaceWeatherChanged { .. } => "Weather",
        EventPayload::PlaceConditionChanged { .. } => "Place Condition",
        EventPayload::PlaceControlChanged { .. } => "Territory",
        EventPayload::LandmarkUpdated { .. } => "Landmark",
        EventPayload::RouteUpdated { .. } => "Route",
        EventPayload::RelationUpserted { .. } => "Relation",
        EventPayload::RelationRemoved { .. } => "Relation Ended",
        EventPayload::ClockCreated { .. } => "Clock Created",
        EventPayload::ClockTicked { .. } => "Clock Ticked",
        EventPayload::LoreRecorded { .. } => "Lore",
        EventPayload::TextRecorded { .. } => "Text Cataloged",
        EventPayload::TextStudied { .. } => "Text Studied",
        EventPayload::AttachmentRecorded { .. } => "Doc Attached",
        EventPayload::ItemCataloged { .. } => "Item Cataloged",
        EventPayload::CatalogItemTransferred { .. } => "Item Transferred",
        EventPayload::EntityHistoryAdded { .. } => "History Recorded",
        EventPayload::EntityEdited { .. } => "Edited",
        EventPayload::EntityReclassified { .. } => "Reclassified",
        EventPayload::EntityRenamedOrMerged { .. } => "Renamed/Merged",
        EventPayload::EntityDeleted { .. } => "Deleted",
        EventPayload::DiceRolled { .. } => "Dice Roll",
        EventPayload::RulesetProfileUpserted { .. } => "Ruleset Profile",
        EventPayload::CheckResolved { .. } => "Action Check",
        EventPayload::ContestResolved { .. } => "Contested Check",
        EventPayload::RandomTableUpserted { .. } => "Random Table",
        EventPayload::RandomTableDrawn { .. } => "Table Draw",
        EventPayload::DiscoveryClaimed { .. } => "Loot Claimed",
        EventPayload::EncounterStarted { .. } => "Encounter Started",
        EventPayload::EncounterActionResolved { .. } => "Encounter Action",
        EventPayload::EncounterEnded { .. } => "Encounter Ended",
        EventPayload::CharacterProfileUpdated { .. } => "Character Profile",
        EventPayload::KnowledgeRecorded { .. } => "Knowledge Recorded",
        EventPayload::KnowledgePropagated { .. } => "Knowledge Spread",
        EventPayload::FactionRecorded { .. } => "Faction Updated",
        EventPayload::AutonomySimulated { .. } => "Autonomy Turn",
        EventPayload::TravelResolved { .. } => "Travel",
        EventPayload::AreaExplored { .. } => "Exploration",
        EventPayload::TransactionExecuted { .. } => "Transaction",
        EventPayload::MarketUpdated { .. } => "Market",
        EventPayload::LawOrReputationUpdated { .. } => "Law / Reputation",
        EventPayload::InjuryUpdated { .. } => "Injury / Recovery",
        EventPayload::DowntimeOrCraftingResolved { .. } => "Downtime / Crafting",
        EventPayload::ProgressionResolved { .. } => "Progression",
        EventPayload::WorldAdvanced { .. } => "World Tick",
        EventPayload::NarrativeEvent { .. } => "Event",
    }
}

pub fn render_random_table_markdown(table: &RandomTable) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "---");
    let _ = writeln!(out, "id: {}", table.id);
    let _ = writeln!(out, "type: random_table");
    let _ = writeln!(out, "category: {}", table.category);
    let _ = writeln!(out, "version: {}", table.version);
    let _ = writeln!(out, "is_loot_table: {}", table.is_loot_table);
    let _ = writeln!(out, "---\n");
    let _ = writeln!(out, "# {} (v{})\n", table.name, table.version);
    let _ = writeln!(
        out,
        "> **Table ID:** `{}` | **Category:** `{}` | **Dice:** `{}` | **Loot Staging:** `{}` | [Campaign Index](../INDEX.md)\n",
        table.id,
        table.category,
        table.dice.as_deref().unwrap_or("weighted"),
        table.is_loot_table
    );
    if !table.description.is_empty() {
        let _ = writeln!(out, "{}\n", table.description);
    }
    if !table.context.is_empty() {
        let _ = writeln!(out, "**Contexts:** {}\n", table.context.join(", "));
    }
    let _ = writeln!(out, "| Entry ID | Range | Weight | Rarity | Result | Subtable |");
    let _ = writeln!(out, "| :--- | :--- | ---: | :--- | :--- | :--- |");
    for e in &table.entries {
        let rng = e
            .range
            .map(|[a, b]| format!("{a}-{b}"))
            .unwrap_or_else(|| "—".to_string());
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | `{}` | {} | {} |",
            e.id,
            rng,
            e.weight,
            e.rarity,
            escape_pipe(&e.result),
            e.subtable_id.as_deref().unwrap_or("—")
        );
    }
    let _ = writeln!(out);
    out
}

pub fn render_encounter_markdown(
    enc: &EncounterState,
    name_map: &BTreeMap<String, (String, String)>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "---");
    let _ = writeln!(out, "id: {}", enc.id);
    let _ = writeln!(out, "type: encounter");
    let _ = writeln!(out, "encounter_type: {}", enc.encounter_type);
    let _ = writeln!(out, "status: {}", enc.status);
    let _ = writeln!(out, "round: {}", enc.round);
    let _ = writeln!(out, "---\n");
    let _ = writeln!(out, "# {} [{}]\n", enc.title, enc.encounter_type);
    let loc_str = enc
        .location_id
        .as_deref()
        .map(|l| link_from_subdir(l, "..", name_map))
        .unwrap_or_else(|| "Unspecified".to_string());
    let _ = writeln!(
        out,
        "> **Encounter ID:** `{}` | **Status:** `{}` | **Round:** `{}` | **Location:** {} | [Campaign Index](../INDEX.md)\n",
        enc.id, enc.status, enc.round, loc_str
    );
    if !enc.environmental_notes.is_empty() {
        let _ = writeln!(out, "**Environment:** {}\n", enc.environmental_notes);
    }
    if !enc.zones.is_empty() {
        let _ = writeln!(out, "**Zones:** {}\n", enc.zones.join(" • "));
    }
    let _ = writeln!(out, "## Turn Order & Participants\n");
    let _ = writeln!(out, "| # | Participant | Side | Init | Position | Range | Reaction | Status |");
    let _ = writeln!(out, "| ---: | :--- | :--- | ---: | :--- | :--- | :--- | :--- |");
    for (idx, p) in enc.participants.iter().enumerate() {
        let active_mark = if enc.status == "active" && idx == enc.active_turn_index {
            "▶ "
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "| {} | {}{} | `{}` | {} | {} | {} | {} | `{}` |",
            idx + 1,
            active_mark,
            link_from_subdir(&p.entity_id, "..", name_map),
            p.side,
            p.initiative,
            non_empty(&p.position),
            non_empty(&p.range_band),
            if p.reaction_available { "Ready" } else { "Spent" },
            p.status
        );
    }
    let _ = writeln!(out);
    if !enc.action_log.is_empty() {
        let _ = writeln!(out, "## Encounter Action Log\n");
        for entry in &enc.action_log {
            let _ = writeln!(
                out,
                "- **R{}** `{}` ({}): {}{}",
                entry.round,
                entry.actor_id,
                entry.action_type,
                entry.description,
                entry
                    .event_id
                    .map(|id| format!(" *(event `#{id}`)*"))
                    .unwrap_or_default()
            );
        }
        let _ = writeln!(out);
    }
    if !enc.outcome.is_empty() || !enc.aftermath.is_empty() {
        let _ = writeln!(out, "## Outcome & Aftermath\n");
        if !enc.outcome.is_empty() {
            let _ = writeln!(out, "**Outcome:** {}\n", enc.outcome);
        }
        if !enc.aftermath.is_empty() {
            let _ = writeln!(out, "{}\n", enc.aftermath);
        }
    }
    out
}

pub fn render_faction_markdown(
    f: &FactionRecord,
    relations: &[GraphRelation],
    histories: &[EntityHistoryEntry],
    events: &[WorldEvent],
    name_map: &BTreeMap<String, (String, String)>,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "---");
    let _ = writeln!(out, "id: {}", f.id);
    let _ = writeln!(out, "type: faction");
    let _ = writeln!(out, "faction_type: {}", f.faction_type);
    let _ = writeln!(out, "capacity_tier: {}", f.capacity_tier);
    let _ = writeln!(out, "morale: {}", f.morale);
    let _ = writeln!(out, "---\n");
    let _ = writeln!(out, "# {}\n", f.name);
    let leader_str = f
        .leader_id
        .as_deref()
        .map(|l| link_from_subdir(l, "..", name_map))
        .unwrap_or_else(|| "Unassigned".to_string());
    let _ = writeln!(
        out,
        "> **Faction ID:** `{}` | **Type:** `{}` | **Tier:** `{}` | **Leader:** {} | **Morale:** `{}/100` | [Campaign Index](../INDEX.md)\n",
        f.id, f.faction_type, f.capacity_tier, leader_str, f.morale
    );
    if !f.origins.is_empty() || !f.ideology.is_empty() || !f.leadership_model.is_empty() {
        let _ = writeln!(out, "## 1. Institutional Identity\n");
        if !f.origins.is_empty() {
            let _ = writeln!(out, "- **Origins:** {}", f.origins);
        }
        if !f.leadership_model.is_empty() {
            let _ = writeln!(out, "- **Leadership Model:** {}", f.leadership_model);
        }
        if !f.ideology.is_empty() {
            let _ = writeln!(out, "- **Ideology:** {}", f.ideology);
        }
        if !f.institutional_structure.is_empty() {
            let _ = writeln!(out, "- **Structure:** {}", f.institutional_structure);
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "## 2. Current State, Roster & Resources\n");
    if !f.territory.is_empty() {
        let terr: Vec<String> = f
            .territory
            .iter()
            .map(|t| link_from_subdir(t, "..", name_map))
            .collect();
        let _ = writeln!(out, "**Territory:** {}\n", terr.join(", "));
    }
    if !f.members.is_empty() {
        let _ = writeln!(out, "### Roster ({} members)\n", f.members.len());
        let _ = writeln!(out, "| Member | Rank | Role | Loyalty |");
        let _ = writeln!(out, "| :--- | :--- | :--- | :--- |");
        for m in &f.members {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} |",
                link_from_subdir(&m.character_id, "..", name_map),
                escape_pipe(&m.rank),
                non_empty(&m.role),
                non_empty(&m.loyalty_note)
            );
        }
        let _ = writeln!(out);
    }
    if !f.resources.is_empty() {
        let _ = writeln!(out, "### Resources & Treasury\n");
        for (k, v) in &f.resources {
            let _ = writeln!(out, "- **{k}:** {v}");
        }
        let _ = writeln!(out);
    }
    if !f.objectives.is_empty() {
        let _ = writeln!(out, "### Objectives\n");
        for o in &f.objectives {
            let _ = writeln!(
                out,
                "- `{}` [P{} | {} | {}/{}]: {}",
                o.goal_id, o.priority, o.status, o.progress, o.target, o.description
            );
        }
        let _ = writeln!(out);
    }
    if !f.internal_conflicts.is_empty() {
        let _ = writeln!(out, "### Internal Conflicts\n");
        for c in &f.internal_conflicts {
            let _ = writeln!(out, "- {c}");
        }
        let _ = writeln!(out);
    }
    if !relations.is_empty() {
        let _ = writeln!(out, "### Institutional & Personal Relations\n");
        for r in relations {
            let other = if r.source_id == f.id {
                &r.target_id
            } else {
                &r.source_id
            };
            let _ = writeln!(
                out,
                "- `{}` {} — {}",
                r.relation_type,
                link_from_subdir(other, "..", name_map),
                r.notes
            );
        }
        let _ = writeln!(out);
    }

    append_history_section(&mut out, histories, "..", name_map);

    if !events.is_empty() {
        let _ = writeln!(out, "---\n\n## 3. Faction Event Log\n");
        for ev in events {
            let _ = writeln!(
                out,
                "- `#{}` [{}] **{}**: {}",
                ev.seq,
                ev.world_time,
                event_kind_label(&ev.payload),
                ev.summary
            );
        }
        let _ = writeln!(out);
    }
    out
}

pub fn render_markets_markdown(
    markets: &[crate::world_sim::SettlementMarket],
    routes: &[crate::world_sim::TravelRoute],
    season: &str,
    weather: &str,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# Regional Markets & Commodity Prices\n");
    let _ = writeln!(
        out,
        "> **Season:** `{season}` | **Global Weather:** `{weather}` | [Campaign Index](../INDEX.md)\n"
    );
    for m in markets {
        let rep = crate::world_sim::evaluate_market_prices(m, routes, season, weather);
        let _ = writeln!(out, "## {} (`{}`)\n", m.name, m.place_id);
        if !rep.disrupted_supply_lines.is_empty() {
            let _ = writeln!(out, "> **Supply Disruptions:** {}\n", rep.disrupted_supply_lines.join(" • "));
        }
        let _ = writeln!(out, "| Commodity | Category | Base | Live Price | Currency | Supply | Demand | Route Mult |");
        let _ = writeln!(out, "| :--- | :--- | ---: | ---: | :--- | ---: | ---: | ---: |");
        for c in &rep.commodities {
            let _ = writeln!(
                out,
                "| {} | {} | {} | **{}** | `{}` | {} | {} | x{:.2} |",
                escape_pipe(&c.name),
                non_empty(&c.category),
                c.base_price,
                c.live_price,
                c.currency,
                c.supply,
                c.demand,
                c.route_disruption_multiplier
            );
        }
        let _ = writeln!(out);
    }
    out
}

/// Lists relative paths (under `docs/`) of compiled Markdown files that no longer
/// correspond to any active entity in SQLite (ignores raw files under `docs/attachments/files/`).
pub fn list_orphaned_docs(db: &CampaignDb, docs_dir: &Path) -> Result<Vec<String>> {
    use std::collections::BTreeSet;
    let mut expected: BTreeSet<String> = BTreeSet::new();
    expected.insert("INDEX.md".to_string());
    expected.insert("TIMELINE.md".to_string());

    for c in db.list_character_baselines()? {
        expected.insert(format!("characters/{}.md", c.id));
    }
    for p in db.list_place_baselines()? {
        expected.insert(format!("places/{}.md", p.id));
    }
    for l in db.list_lore(None)? {
        expected.insert(format!("lore/{}/{}.md", slugify(&l.category), l.id));
    }
    for t in db.list_texts()? {
        expected.insert(format!("texts/{}.md", t.id));
    }
    for it in db.list_catalog_items()? {
        expected.insert(format!("items/{}.md", it.id));
    }
    for a in db.list_attachments()? {
        expected.insert(format!("attachments/{}.md", a.id));
    }
    for t in db.list_random_tables()? {
        expected.insert(format!("tables/{}.md", t.id));
    }
    for enc in db.list_encounters(None)? {
        expected.insert(format!("encounters/{}.md", enc.id));
    }
    for f in db.list_factions()? {
        expected.insert(format!("factions/{}.md", f.id));
    }
    if !db.list_markets()?.is_empty() {
        expected.insert("economy/markets.md".to_string());
    }

    let mut orphans = Vec::new();
    if !docs_dir.exists() {
        return Ok(orphans);
    }

    fn walk_md(root: &Path, dir: &Path, expected: &BTreeSet<String>, out: &mut Vec<String>) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                // Never touch raw copied/created user attachment files under docs/attachments/files/
                if path.file_name().and_then(|s| s.to_str()) == Some("files")
                    && path
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|s| s.to_str())
                        == Some("attachments")
                {
                    continue;
                }
                walk_md(root, &path, expected, out)?;
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                if let Ok(rel) = path.strip_prefix(root) {
                    let rel_str = rel.to_string_lossy().replace('\\', "/");
                    if !expected.contains(&rel_str) {
                        out.push(rel_str);
                    }
                }
            }
        }
        Ok(())
    }

    walk_md(docs_dir, docs_dir, &expected, &mut orphans)?;
    orphans.sort();
    Ok(orphans)
}

/// Removes orphaned compiled Markdown files (and empty lore category subfolders) under `docs/`.
pub fn prune_orphaned_docs(db: &CampaignDb, docs_dir: &Path) -> Result<Vec<String>> {
    let orphans = list_orphaned_docs(db, docs_dir)?;
    let mut removed = Vec::new();
    for rel in orphans {
        let target = docs_dir.join(&rel);
        if target.exists() && fs::remove_file(&target).is_ok() {
            removed.push(rel);
        }
    }
    // Remove empty lore category directories if any became empty after reclassification/deletion
    let lore_dir = docs_dir.join("lore");
    if lore_dir.exists() {
        if let Ok(rd) = fs::read_dir(&lore_dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    if let Ok(mut inner) = fs::read_dir(&p) {
                        if inner.next().is_none() {
                            let _ = fs::remove_dir(&p);
                        }
                    }
                }
            }
        }
    }
    Ok(removed)
}
