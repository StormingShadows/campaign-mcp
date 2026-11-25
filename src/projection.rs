use crate::db::CampaignDb;
use crate::models::{
    CharacterBaseline, CharacterCurrent, CharacterProjection, EventPayload, PlaceBaseline,
    PlaceCurrent, PlaceProjection, ResourcePool, WorldClock, WorldEvent,
};
use anyhow::{anyhow, Result};
use std::collections::BTreeMap;

/// Deterministically projects a Character's live status from its immutable baseline
/// plus all events affecting it (optionally capped at `as_of_seq`).
pub fn project_character(
    baseline: &CharacterBaseline,
    events: &[WorldEvent],
) -> CharacterProjection {
    let mut current = CharacterCurrent {
        id: baseline.id.clone(),
        name: baseline.name.clone(),
        alive_status: "Active".to_string(),
        current_location: baseline.initial_location.clone(),
        stats: baseline.initial_stats.clone(),
        resources: baseline.initial_resources.clone(),
        conditions: Vec::new(),
        equipment: baseline.initial_equipment.clone(),
        skills: baseline.initial_skills.clone(),
        techniques: baseline.initial_techniques.clone(),
        active_goals: baseline.initial_goals.clone(),
        completed_goals: Vec::new(),
        last_event_seq: 0,
        last_world_time: baseline.created_at_world_time.clone(),
    };

    let mut relevant_events = Vec::new();

    for ev in events {
        let touches_char = ev.affected_entities.iter().any(|e| e == &baseline.id)
            || event_targets_character(&ev.payload, &baseline.id);
        if !touches_char {
            continue;
        }

        current.last_event_seq = ev.seq;
        current.last_world_time = ev.world_time.clone();
        relevant_events.push(ev.clone());

        match &ev.payload {
            EventPayload::CharacterMoved {
                character_id,
                to_place,
                ..
            } if character_id == &baseline.id => {
                current.current_location = Some(to_place.clone());
            }
            EventPayload::ResourceChanged {
                character_id,
                resource,
                delta,
                set_current,
                set_max,
                unit,
                remove,
                ..
            } if character_id == &baseline.id => {
                if *remove {
                    current.resources.remove(resource);
                } else {
                    let entry = current
                        .resources
                        .entry(resource.clone())
                        .or_insert_with(|| ResourcePool {
                            current: 0,
                            max: 100,
                            unit: unit.clone().unwrap_or_default(),
                        });
                    if let Some(u) = unit {
                        if !u.is_empty() {
                            entry.unit = u.clone();
                        }
                    }
                    if let Some(new_max) = set_max {
                        entry.max = *new_max;
                    }
                    if let Some(val) = set_current {
                        entry.current = *val;
                    } else if let Some(d) = delta {
                        entry.current += *d;
                    }
                    if entry.max > 0 && entry.current > entry.max {
                        entry.current = entry.max;
                    }
                    if entry.current < 0 {
                        entry.current = 0;
                    }
                }
            }
            EventPayload::StatChanged {
                character_id,
                stat,
                delta,
                set_value,
                ..
            } if character_id == &baseline.id => {
                let entry = current.stats.entry(stat.clone()).or_insert(0);
                if let Some(val) = set_value {
                    *entry = *val;
                } else if let Some(d) = delta {
                    *entry += *d;
                }
            }
            EventPayload::CharacterStatusChanged {
                character_id,
                alive_status,
                ..
            } if character_id == &baseline.id => {
                current.alive_status = alive_status.clone();
            }
            EventPayload::ConditionAdded {
                character_id,
                condition,
            } if character_id == &baseline.id => {
                if let Some(existing) = current
                    .conditions
                    .iter_mut()
                    .find(|c| c.name.eq_ignore_ascii_case(&condition.name))
                {
                    *existing = condition.clone();
                } else {
                    current.conditions.push(condition.clone());
                }
            }
            EventPayload::ConditionRemoved {
                character_id,
                condition_name,
                ..
            } if character_id == &baseline.id => {
                current
                    .conditions
                    .retain(|c| !c.name.eq_ignore_ascii_case(condition_name));
            }
            EventPayload::ItemAcquired { character_id, item } if character_id == &baseline.id => {
                if let Some(existing) = current
                    .equipment
                    .iter_mut()
                    .find(|i| i.name.eq_ignore_ascii_case(&item.name))
                {
                    existing.quantity += item.quantity;
                    if item.equipped {
                        existing.equipped = true;
                    }
                    if item.slot.is_some() {
                        existing.slot = item.slot.clone();
                    }
                    if !item.description.is_empty() {
                        existing.description = item.description.clone();
                    }
                    if item.durability.is_some() {
                        existing.durability = item.durability.clone();
                    }
                    if !item.tags.is_empty() {
                        for t in &item.tags {
                            if !existing.tags.iter().any(|et| et.eq_ignore_ascii_case(t)) {
                                existing.tags.push(t.clone());
                            }
                        }
                    }
                    if !item.garment_measurements.is_empty() {
                        for (mk, mv) in &item.garment_measurements {
                            if mv.trim().is_empty() {
                                existing.garment_measurements.remove(mk);
                            } else {
                                existing
                                    .garment_measurements
                                    .insert(mk.clone(), mv.trim().to_string());
                            }
                        }
                    }
                    if !item.garment_cut.trim().is_empty() {
                        existing.garment_cut = item.garment_cut.trim().to_string();
                    }
                    if !item.garment_taper.trim().is_empty() {
                        existing.garment_taper = item.garment_taper.trim().to_string();
                    }
                } else {
                    current.equipment.push(item.clone());
                }
            }
            EventPayload::ItemRemoved {
                character_id,
                item_name,
                quantity,
                ..
            } if character_id == &baseline.id => {
                if let Some(existing) = current
                    .equipment
                    .iter_mut()
                    .find(|i| i.name.eq_ignore_ascii_case(item_name))
                {
                    existing.quantity -= *quantity;
                }
                current.equipment.retain(|i| i.quantity > 0);
            }
            EventPayload::ItemEquipped {
                character_id,
                item_name,
                slot,
                equipped,
            } if character_id == &baseline.id => {
                if let Some(existing) = current
                    .equipment
                    .iter_mut()
                    .find(|i| i.name.eq_ignore_ascii_case(item_name))
                {
                    existing.equipped = *equipped;
                    if slot.is_some() {
                        existing.slot = slot.clone();
                    }
                }
            }
            EventPayload::ItemTagsUpdated {
                character_id,
                item_name,
                add_tags,
                remove_tags,
                replace_tags,
                ..
            } if character_id == &baseline.id => {
                if let Some(existing) = current
                    .equipment
                    .iter_mut()
                    .find(|i| i.name.eq_ignore_ascii_case(item_name))
                {
                    if let Some(ref rep) = replace_tags {
                        existing.tags = crate::models::normalize_item_tags(rep);
                    } else {
                        existing.tags = crate::models::normalize_item_tags(&existing.tags);
                    }
                    if !remove_tags.is_empty() {
                        let rem_norm = crate::models::normalize_item_tags(remove_tags);
                        existing.tags.retain(|t| !rem_norm.iter().any(|r| r == t));
                    }
                    if !add_tags.is_empty() {
                        for t in crate::models::normalize_item_tags(add_tags) {
                            if !existing.tags.iter().any(|e| e == &t) {
                                existing.tags.push(t);
                            }
                        }
                    }
                }
            }
            EventPayload::SkillUpdated {
                character_id,
                skill,
            } if character_id == &baseline.id => {
                if let Some(existing) = current
                    .skills
                    .iter_mut()
                    .find(|s| s.name.eq_ignore_ascii_case(&skill.name))
                {
                    *existing = skill.clone();
                } else {
                    current.skills.push(skill.clone());
                }
            }
            EventPayload::TechniqueUpdated {
                character_id,
                technique,
            } if character_id == &baseline.id => {
                if let Some(existing) = current
                    .techniques
                    .iter_mut()
                    .find(|t| t.name.eq_ignore_ascii_case(&technique.name))
                {
                    *existing = technique.clone();
                } else {
                    current.techniques.push(technique.clone());
                }
            }
            EventPayload::GoalUpdated {
                character_id,
                goal,
                completed,
            } if character_id == &baseline.id => {
                if *completed {
                    current
                        .active_goals
                        .retain(|g| !g.eq_ignore_ascii_case(goal));
                    if !current
                        .completed_goals
                        .iter()
                        .any(|g| g.eq_ignore_ascii_case(goal))
                    {
                        current.completed_goals.push(goal.clone());
                    }
                } else if !current
                    .active_goals
                    .iter()
                    .any(|g| g.eq_ignore_ascii_case(goal))
                {
                    current.active_goals.push(goal.clone());
                }
            }
            _ => {}
        }
    }

    CharacterProjection {
        baseline: baseline.clone(),
        current,
        event_log: relevant_events,
    }
}

/// Deterministically projects a Place's live status from its baseline, world events,
/// current world clock, and the list of characters currently located there.
pub fn project_place(
    baseline: &PlaceBaseline,
    events: &[WorldEvent],
    world_clock: &WorldClock,
    present_characters: Vec<String>,
) -> PlaceProjection {
    let mut current = PlaceCurrent {
        id: baseline.id.clone(),
        name: baseline.name.clone(),
        current_season: world_clock.season.clone(),
        current_weather: if baseline.initial_weather.is_empty() {
            world_clock.global_weather.clone()
        } else {
            baseline.initial_weather.clone()
        },
        controlling_faction: baseline.initial_faction.clone(),
        danger_level: baseline.initial_danger_level.clone(),
        active_conditions: Vec::new(),
        landmarks: baseline.places_of_significance.clone(),
        routes: baseline.routes.clone(),
        present_characters,
        last_event_seq: 0,
        last_world_time: baseline.created_at_world_time.clone(),
    };

    let mut relevant_events = Vec::new();

    for ev in events {
        let touches_place = ev.location_id.as_deref() == Some(baseline.id.as_str())
            || ev.affected_entities.iter().any(|e| e == &baseline.id)
            || event_targets_place(&ev.payload, &baseline.id);
        if !touches_place {
            continue;
        }

        current.last_event_seq = ev.seq;
        current.last_world_time = ev.world_time.clone();
        relevant_events.push(ev.clone());

        match &ev.payload {
            EventPayload::PlaceWeatherChanged {
                place_id, weather, ..
            } if place_id == &baseline.id => {
                current.current_weather = weather.clone();
            }
            EventPayload::PlaceConditionChanged {
                place_id,
                condition,
                active,
                ..
            } if place_id == &baseline.id => {
                if *active {
                    if !current
                        .active_conditions
                        .iter()
                        .any(|c| c.eq_ignore_ascii_case(condition))
                    {
                        current.active_conditions.push(condition.clone());
                    }
                } else {
                    current
                        .active_conditions
                        .retain(|c| !c.eq_ignore_ascii_case(condition));
                }
            }
            EventPayload::PlaceControlChanged {
                place_id,
                faction,
                danger_level,
                ..
            } if place_id == &baseline.id => {
                if faction.is_some() {
                    current.controlling_faction = faction.clone();
                }
                if let Some(d) = danger_level {
                    current.danger_level = d.clone();
                }
            }
            EventPayload::LandmarkUpdated { place_id, landmark } if place_id == &baseline.id => {
                if let Some(existing) = current
                    .landmarks
                    .iter_mut()
                    .find(|l| l.name.eq_ignore_ascii_case(&landmark.name))
                {
                    *existing = landmark.clone();
                } else {
                    current.landmarks.push(landmark.clone());
                }
            }
            EventPayload::RouteUpdated { place_id, route } if place_id == &baseline.id => {
                if let Some(existing) = current
                    .routes
                    .iter_mut()
                    .find(|r| r.destination_id.eq_ignore_ascii_case(&route.destination_id))
                {
                    *existing = route.clone();
                } else {
                    current.routes.push(route.clone());
                }
            }
            _ => {}
        }
    }

    PlaceProjection {
        baseline: baseline.clone(),
        current,
        event_log: relevant_events,
    }
}

pub fn load_all_character_projections(
    db: &CampaignDb,
    as_of_seq: Option<i64>,
) -> Result<BTreeMap<String, CharacterProjection>> {
    let baselines = db.list_character_baselines()?;
    let events = match as_of_seq {
        Some(seq) => db.list_events_up_to(seq)?,
        None => db.list_all_events()?,
    };
    Ok(project_characters_bucketed(&baselines, &events))
}

fn project_characters_bucketed(
    baselines: &[CharacterBaseline],
    events: &[WorldEvent],
) -> BTreeMap<String, CharacterProjection> {
    let mut buckets: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::with_capacity(baselines.len());
    for b in baselines {
        buckets.insert(b.id.clone(), Vec::new());
    }

    for (idx, ev) in events.iter().enumerate() {
        let mut touched: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for eid in &ev.affected_entities {
            if buckets.contains_key(eid.as_str()) {
                touched.insert(eid.as_str());
            }
        }
        if let Some(cid) = payload_character_target(&ev.payload) {
            if buckets.contains_key(cid) {
                touched.insert(cid);
            }
        }
        for cid in touched {
            if let Some(vec) = buckets.get_mut(cid) {
                vec.push(idx);
            }
        }
    }

    let mut map = BTreeMap::new();
    for b in baselines {
        let slice_events: Vec<WorldEvent> = buckets
            .get(&b.id)
            .map(|idxs| idxs.iter().map(|&i| events[i].clone()).collect())
            .unwrap_or_default();
        let proj = project_character(b, &slice_events);
        map.insert(b.id.clone(), proj);
    }
    map
}

pub fn load_character_projection(
    db: &CampaignDb,
    character_id: &str,
    as_of_seq: Option<i64>,
) -> Result<CharacterProjection> {
    let baseline = db
        .get_character_baseline(character_id)?
        .ok_or_else(|| anyhow!("Character '{character_id}' not found"))?;
    let events = match as_of_seq {
        Some(seq) => db.list_events_up_to(seq)?,
        None => db.list_events_for_entity(character_id)?,
    };
    Ok(project_character(&baseline, &events))
}

pub fn load_all_place_projections(
    db: &CampaignDb,
    as_of_seq: Option<i64>,
) -> Result<BTreeMap<String, PlaceProjection>> {
    let meta = db.get_campaign_meta()?;
    let char_baselines = db.list_character_baselines()?;
    let place_baselines = db.list_place_baselines()?;
    let events = match as_of_seq {
        Some(seq) => db.list_events_up_to(seq)?,
        None => db.list_all_events()?,
    };

    let char_projections = project_characters_bucketed(&char_baselines, &events);

    // Derive world clock at as_of_seq if needed
    let mut clock = meta.clock.clone();
    if as_of_seq.is_some() {
        for ev in &events {
            if let EventPayload::TimeAdvanced { new_clock, .. } = &ev.payload {
                clock = new_clock.clone();
            }
        }
    }

    let mut place_buckets: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::with_capacity(place_baselines.len());
    let mut present_by_place: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::with_capacity(place_baselines.len());
    for pb in &place_baselines {
        place_buckets.insert(pb.id.clone(), Vec::new());
        present_by_place.insert(pb.id.clone(), Vec::new());
    }
    for cp in char_projections.values() {
        if let Some(ref loc) = cp.current.current_location {
            if let Some(list) = present_by_place.get_mut(loc.as_str()) {
                list.push(cp.baseline.id.clone());
            }
        }
    }

    for (idx, ev) in events.iter().enumerate() {
        let mut touched: std::collections::HashSet<&str> = std::collections::HashSet::new();
        if let Some(ref lid) = ev.location_id {
            if place_buckets.contains_key(lid.as_str()) {
                touched.insert(lid.as_str());
            }
        }
        for eid in &ev.affected_entities {
            if place_buckets.contains_key(eid.as_str()) {
                touched.insert(eid.as_str());
            }
        }
        for pid in payload_place_targets(&ev.payload) {
            if place_buckets.contains_key(pid) {
                touched.insert(pid);
            }
        }
        for pid in touched {
            if let Some(vec) = place_buckets.get_mut(pid) {
                vec.push(idx);
            }
        }
    }

    let mut map = BTreeMap::new();
    for pb in place_baselines {
        let mut present = present_by_place.remove(&pb.id).unwrap_or_default();
        present.sort();
        let slice_events: Vec<WorldEvent> = place_buckets
            .get(&pb.id)
            .map(|idxs| idxs.iter().map(|&i| events[i].clone()).collect())
            .unwrap_or_default();
        let proj = project_place(&pb, &slice_events, &clock, present);
        map.insert(pb.id.clone(), proj);
    }
    Ok(map)
}

pub fn load_place_projection(
    db: &CampaignDb,
    place_id: &str,
    as_of_seq: Option<i64>,
) -> Result<PlaceProjection> {
    if as_of_seq.is_none() {
        if let Some(pb) = db.get_place_baseline(place_id)? {
            let meta = db.get_campaign_meta()?;
            let char_projections = load_all_character_projections(db, None)?;
            let mut present: Vec<String> = char_projections
                .values()
                .filter(|cp| cp.current.current_location.as_deref() == Some(pb.id.as_str()))
                .map(|cp| cp.baseline.id.clone())
                .collect();
            present.sort();
            // Include events linked in event_entities OR having location_id == place_id
            let events = db.list_events_for_place_or_entity(place_id)?;
            return Ok(project_place(&pb, &events, &meta.clock, present));
        }
    }
    let all = load_all_place_projections(db, as_of_seq)?;
    all.get(place_id)
        .cloned()
        .ok_or_else(|| anyhow!("Place '{place_id}' not found"))
}

fn payload_character_target(payload: &EventPayload) -> Option<&str> {
    match payload {
        EventPayload::CharacterCreated { character_id }
        | EventPayload::CharacterMoved { character_id, .. }
        | EventPayload::ResourceChanged { character_id, .. }
        | EventPayload::StatChanged { character_id, .. }
        | EventPayload::CharacterStatusChanged { character_id, .. }
        | EventPayload::ConditionAdded { character_id, .. }
        | EventPayload::ConditionRemoved { character_id, .. }
        | EventPayload::ItemAcquired { character_id, .. }
        | EventPayload::ItemRemoved { character_id, .. }
        | EventPayload::ItemEquipped { character_id, .. }
        | EventPayload::ItemTagsUpdated { character_id, .. }
        | EventPayload::SkillUpdated { character_id, .. }
        | EventPayload::TechniqueUpdated { character_id, .. }
        | EventPayload::GoalUpdated { character_id, .. } => Some(character_id.as_str()),
        EventPayload::DiceRolled { actor_id, .. } => actor_id.as_deref(),
        _ => None,
    }
}

fn payload_place_targets(payload: &EventPayload) -> Vec<&str> {
    match payload {
        EventPayload::PlaceCreated { place_id }
        | EventPayload::PlaceWeatherChanged { place_id, .. }
        | EventPayload::PlaceConditionChanged { place_id, .. }
        | EventPayload::PlaceControlChanged { place_id, .. }
        | EventPayload::LandmarkUpdated { place_id, .. }
        | EventPayload::RouteUpdated { place_id, .. } => vec![place_id.as_str()],
        EventPayload::CharacterMoved {
            from_place,
            to_place,
            ..
        } => {
            let mut v = vec![to_place.as_str()];
            if let Some(fp) = from_place.as_deref() {
                v.push(fp);
            }
            v
        }
        _ => Vec::new(),
    }
}

fn event_targets_character(payload: &EventPayload, char_id: &str) -> bool {
    match payload {
        EventPayload::CharacterCreated { character_id }
        | EventPayload::CharacterMoved { character_id, .. }
        | EventPayload::ResourceChanged { character_id, .. }
        | EventPayload::StatChanged { character_id, .. }
        | EventPayload::CharacterStatusChanged { character_id, .. }
        | EventPayload::ConditionAdded { character_id, .. }
        | EventPayload::ConditionRemoved { character_id, .. }
        | EventPayload::ItemAcquired { character_id, .. }
        | EventPayload::ItemRemoved { character_id, .. }
        | EventPayload::ItemEquipped { character_id, .. }
        | EventPayload::ItemTagsUpdated { character_id, .. }
        | EventPayload::SkillUpdated { character_id, .. }
        | EventPayload::TechniqueUpdated { character_id, .. }
        | EventPayload::GoalUpdated { character_id, .. } => character_id == char_id,
        EventPayload::DiceRolled { actor_id, .. } => actor_id.as_deref() == Some(char_id),
        _ => false,
    }
}

fn event_targets_place(payload: &EventPayload, place_id: &str) -> bool {
    match payload {
        EventPayload::PlaceCreated { place_id: pid }
        | EventPayload::PlaceWeatherChanged { place_id: pid, .. }
        | EventPayload::PlaceConditionChanged { place_id: pid, .. }
        | EventPayload::PlaceControlChanged { place_id: pid, .. }
        | EventPayload::LandmarkUpdated { place_id: pid, .. }
        | EventPayload::RouteUpdated { place_id: pid, .. } => pid == place_id,
        EventPayload::CharacterMoved {
            from_place,
            to_place,
            ..
        } => from_place.as_deref() == Some(place_id) || to_place == place_id,
        _ => false,
    }
}
