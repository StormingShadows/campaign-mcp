use crate::autonomy::evaluate_equipped_apparel_fit;
use crate::db::CampaignDb;
use crate::models::{slugify, CharacterProjection, PlaceProjection};
use crate::world_sim::{
    compute_encumbrance, evaluate_market_prices, grid_cell_scale_km, synthesize_routes_from_places,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// First-class active engine signal, alarm, or GM/player directive emitted by Chronicle's
/// deterministic simulation & state-machine rules.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EngineSignal {
    pub id: String,
    /// Deterministic deduplication key so repeated ticks update an existing active signal instead of spamming duplicates.
    #[serde(default)]
    pub dedup_key: String,
    /// Machine-readable signal code (e.g. "STAGE_RULE_APPLIED", "PROGRESSION_BOTTLENECK", "OUTGROWN_GARMENT", "CLOCK_FILLED").
    pub code: String,
    /// "critical" | "warning" | "info"
    #[serde(default = "default_signal_severity")]
    pub severity: String,
    /// "progression" | "appearance" | "apparel" | "clock" | "routine" | "health" | "economy" | "law" | "world" | "discovery"
    #[serde(default = "default_signal_category")]
    pub category: String,
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub entity_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
    #[serde(default)]
    pub suggested_actions: Vec<String>,
    /// "public" | "player" | "gm_secret"
    #[serde(default = "default_signal_visibility")]
    pub visibility: String,
    /// "active" | "acknowledged" | "resolved"
    #[serde(default = "default_signal_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event_seq: Option<i64>,
    #[serde(default)]
    pub created_at_world_time: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_signal_severity() -> String {
    "info".to_string()
}

fn default_signal_category() -> String {
    "world".to_string()
}

fn default_signal_visibility() -> String {
    "public".to_string()
}

fn default_signal_status() -> String {
    "active".to_string()
}

pub fn severity_rank(sev: &str) -> u8 {
    match sev.trim().to_ascii_lowercase().as_str() {
        "critical" => 3,
        "warning" => 2,
        _ => 1,
    }
}

impl EngineSignal {
    pub fn normalized(mut self, world_time: &str) -> Self {
        if self.dedup_key.trim().is_empty() {
            let suffix = if self.entity_ids.is_empty() {
                slugify(&self.title)
            } else {
                self.entity_ids.join("-")
            };
            self.dedup_key = format!("{}:{}", self.code.trim().to_ascii_lowercase(), suffix);
        }
        if self.id.trim().is_empty() {
            self.id = slugify(&self.dedup_key);
        } else {
            self.id = slugify(&self.id);
        }
        self.severity = match self.severity.trim().to_ascii_lowercase().as_str() {
            "critical" => "critical".to_string(),
            "warning" | "warn" => "warning".to_string(),
            _ => "info".to_string(),
        };
        if self.visibility.trim().is_empty() {
            self.visibility = "public".to_string();
        }
        if self.status.trim().is_empty() {
            self.status = "active".to_string();
        }
        if self.created_at_world_time.trim().is_empty() {
            self.created_at_world_time = world_time.to_string();
        }
        if self.updated_at_world_time.trim().is_empty() {
            self.updated_at_world_time = world_time.to_string();
        }
        self
    }
}

/// Combines persistent event-driven signals stored in `engine_signals` with deterministic
/// live-state evaluators across progression, apparel fit, clocks, injuries, encumbrance,
/// discoveries, warrants, and disrupted supply routes.
pub fn evaluate_live_signals(
    db: &CampaignDb,
    all_chars: &BTreeMap<String, CharacterProjection>,
    all_places: &BTreeMap<String, PlaceProjection>,
    focus_entities: Option<&BTreeSet<String>>,
    active_place_id: Option<&str>,
    world_time: &str,
) -> Result<Vec<EngineSignal>> {
    let all_stored = db.list_signals(None)?;
    let mut suppressed_dedup: BTreeSet<String> = BTreeSet::new();
    let mut by_dedup: BTreeMap<String, EngineSignal> = BTreeMap::new();

    let pc_ids: BTreeSet<String> = all_chars
        .values()
        .filter(|c| c.baseline.kind.eq_ignore_ascii_case("pc"))
        .map(|c| c.baseline.id.clone())
        .collect();

    let matches_focus = |entity_ids: &[String], loc_id: Option<&str>, sev: &str| -> bool {
        let Some(focus) = focus_entities else {
            return true;
        };
        if sev == "critical" && entity_ids.iter().any(|e| pc_ids.contains(e)) {
            return true;
        }
        if entity_ids.is_empty() && loc_id.is_none() {
            return true;
        }
        if let Some(l) = loc_id {
            if focus.contains(l) || active_place_id == Some(l) {
                return true;
            }
        }
        entity_ids.iter().any(|e| focus.contains(e) || pc_ids.contains(e))
    };

    for sig in all_stored {
        if sig.status == "acknowledged" || sig.status == "resolved" {
            if !sig.dedup_key.is_empty() {
                suppressed_dedup.insert(sig.dedup_key);
            }
            continue;
        }
        if sig.status == "active" && matches_focus(&sig.entity_ids, sig.location_id.as_deref(), &sig.severity) {
            let key = if sig.dedup_key.is_empty() {
                sig.id.clone()
            } else {
                sig.dedup_key.clone()
            };
            by_dedup.insert(key, sig);
        }
    }

    let mut push_derived = |sig: EngineSignal| {
        let norm = sig.normalized(world_time);
        if suppressed_dedup.contains(&norm.dedup_key) {
            return;
        }
        if !matches_focus(&norm.entity_ids, norm.location_id.as_deref(), &norm.severity) {
            return;
        }
        by_dedup.entry(norm.dedup_key.clone()).or_insert(norm);
    };

    let econ_map = db.list_item_economics_map().unwrap_or_default();

    // 1. Character-level live checks (PCs + scene-focused characters)
    for (cid, cp) in all_chars {
        let is_pc = pc_ids.contains(cid);
        let in_focus = is_pc || focus_entities.map_or(true, |f| f.contains(cid));
        if !in_focus {
            continue;
        }

        // 1a. Progression bottlenecks & >=75% approaching threshold
        if let Ok(tracks) = db.list_character_progressions(cid) {
            for tr in tracks {
                if matches!(tr.status.as_str(), "completed" | "crippled") {
                    continue;
                }
                if tr.at_bottleneck {
                    push_derived(EngineSignal {
                        id: String::new(),
                        dedup_key: format!("prog_bottleneck:{cid}:{}", tr.track_id),
                        code: "PROGRESSION_BOTTLENECK".to_string(),
                        severity: "warning".to_string(),
                        category: "progression".to_string(),
                        title: format!("{} — '{}' at Bottleneck ({})", cp.baseline.name, tr.name, tr.format_stage()),
                        summary: if tr.bottleneck_reason.is_empty() {
                            format!("{} reached peak {} ({}/{} XP). Breakthrough required to advance.", cp.baseline.name, tr.format_stage(), tr.current_xp, tr.xp_to_next)
                        } else {
                            format!("{} ({}): {}", cp.baseline.name, tr.format_stage(), tr.bottleneck_reason)
                        },
                        entity_ids: vec![cid.clone(), tr.track_id.clone()],
                        location_id: cp.current.current_location.clone(),
                        suggested_actions: vec![format!(
                            "progression(action='attempt_breakthrough', character_id='{cid}', track_id='{}')",
                            tr.track_id
                        )],
                        visibility: "player".to_string(),
                        status: "active".to_string(),
                        source_event_seq: None,
                        created_at_world_time: world_time.to_string(),
                        updated_at_world_time: world_time.to_string(),
                    });
                } else if tr.status == "active" && tr.xp_to_next > 0 {
                    let pct = (tr.current_xp.saturating_mul(100)) / tr.xp_to_next.max(1);
                    if pct >= 75 {
                        push_derived(EngineSignal {
                            id: String::new(),
                            dedup_key: format!("prog_near:{cid}:{}:{}:{}", tr.track_id, tr.major_stage_index, tr.sub_stage),
                            code: "PROGRESSION_NEARING_THRESHOLD".to_string(),
                            severity: "info".to_string(),
                            category: "progression".to_string(),
                            title: format!("{} — '{}' Nearing Next Stage ({pct}%)", cp.baseline.name, tr.name),
                            summary: format!(
                                "{} is at {} with {}/{} XP ({} XP remaining to next layer/stage).",
                                cp.baseline.name,
                                tr.format_stage(),
                                tr.current_xp,
                                tr.xp_to_next,
                                tr.xp_to_next.saturating_sub(tr.current_xp)
                            ),
                            entity_ids: vec![cid.clone(), tr.track_id.clone()],
                            location_id: cp.current.current_location.clone(),
                            suggested_actions: vec![],
                            visibility: "player".to_string(),
                            status: "active".to_string(),
                            source_event_seq: None,
                            created_at_world_time: world_time.to_string(),
                            updated_at_world_time: world_time.to_string(),
                        });
                    }
                }
            }
        }

        // 1b. Equipped garment outgrown / binding discomfort & unspent stat points
        if let Ok(Some(prof)) = db.get_character_profile(cid) {
            let fit = evaluate_equipped_apparel_fit(&cp.current.equipment, &prof.avatar);
            if !fit.outgrown_items.is_empty() {
                push_derived(EngineSignal {
                    id: String::new(),
                    dedup_key: format!("outgrown_garment:{cid}:{}", fit.outgrown_items.join(",")),
                    code: "OUTGROWN_GARMENT".to_string(),
                    severity: "critical".to_string(),
                    category: "apparel".to_string(),
                    title: format!("{} Has Outgrown Equipped Apparel!", cp.baseline.name),
                    summary: format!(
                        "Equipped garment(s) [{}] no longer fit {}'s current body dimensions ({}). Active penalties apply until unequipped, altered, or replaced.",
                        fit.outgrown_items.join(", "),
                        cp.baseline.name,
                        fit.brief
                    ),
                    entity_ids: vec![cid.clone()],
                    location_id: cp.current.current_location.clone(),
                    suggested_actions: vec![format!(
                        "character(action='equipment', sub_action='unequip', character_id='{cid}', item_name='{}')",
                        fit.outgrown_items[0]
                    )],
                    visibility: "public".to_string(),
                    status: "active".to_string(),
                    source_event_seq: None,
                    created_at_world_time: world_time.to_string(),
                    updated_at_world_time: world_time.to_string(),
                });
            } else if matches!(fit.discomfort_tier.as_str(), "binding_chafing" | "awkward_drape") {
                let is_binding = fit.discomfort_tier == "binding_chafing";
                push_derived(EngineSignal {
                    id: String::new(),
                    dedup_key: format!("apparel_discomfort:{cid}:{}", fit.discomfort_tier),
                    code: "APPAREL_DISCOMFORT".to_string(),
                    severity: if is_binding { "warning" } else { "info" }.to_string(),
                    category: "apparel".to_string(),
                    title: if is_binding {
                        format!("{} — Tight / Binding Equipped Apparel", cp.baseline.name)
                    } else {
                        format!("{} — Ill-Fitting / Odd-Tapered Equipped Apparel", cp.baseline.name)
                    },
                    summary: format!("{}'s worn clothing fits poorly: {}", cp.baseline.name, fit.brief),
                    entity_ids: vec![cid.clone()],
                    location_id: cp.current.current_location.clone(),
                    suggested_actions: vec!["Tailor garment measurements or switch to better-proportioned attire.".to_string()],
                    visibility: "public".to_string(),
                    status: "active".to_string(),
                    source_event_seq: None,
                    created_at_world_time: world_time.to_string(),
                    updated_at_world_time: world_time.to_string(),
                });
            }

            if is_pc && prof.unspent_stat_points > 0 {
                push_derived(EngineSignal {
                    id: String::new(),
                    dedup_key: format!("unspent_stat_pts:{cid}:{}", prof.unspent_stat_points),
                    code: "UNSPENT_STAT_POINTS".to_string(),
                    severity: "info".to_string(),
                    category: "progression".to_string(),
                    title: format!("{} — {} Unspent Stat Point(s) Available", cp.baseline.name, prof.unspent_stat_points),
                    summary: format!(
                        "{} has {} free attribute point(s) ready to allocate.",
                        cp.baseline.name, prof.unspent_stat_points
                    ),
                    entity_ids: vec![cid.clone()],
                    location_id: cp.current.current_location.clone(),
                    suggested_actions: vec![format!(
                        "character(action='pc_sheet', sub_action='adjust_stat_xp', character_id='{cid}', stat='<Stat>', spend_stat_point=true)"
                    )],
                    visibility: "player".to_string(),
                    status: "active".to_string(),
                    source_event_seq: None,
                    created_at_world_time: world_time.to_string(),
                    updated_at_world_time: world_time.to_string(),
                });
            }
        }

        // 1c. Persistent untreated / critical injuries
        if let Ok(injuries) = db.list_injuries(Some(cid), false) {
            for inj in injuries {
                let sev_l = inj.severity.to_ascii_lowercase();
                if sev_l == "critical" || (!inj.treated && matches!(sev_l.as_str(), "moderate" | "severe")) {
                    let sig_sev = if sev_l == "critical" || sev_l == "severe" {
                        "critical"
                    } else {
                        "warning"
                    };
                    push_derived(EngineSignal {
                        id: String::new(),
                        dedup_key: format!("untreated_injury:{cid}:{}", inj.id),
                        code: "UNTREATED_INJURY".to_string(),
                        severity: sig_sev.to_string(),
                        category: "health".to_string(),
                        title: format!(
                            "{} — {} Injury: '{}' ({})",
                            cp.baseline.name,
                            if inj.treated { "Critical" } else { "Untreated" },
                            inj.name,
                            inj.severity
                        ),
                        summary: format!(
                            "{} has {} ({}, {} days recovery remaining{}). Imposes check penalties until treated/healed.",
                            cp.baseline.name,
                            inj.name,
                            if inj.body_location.is_empty() { "body" } else { &inj.body_location },
                            inj.recovery_days_remaining,
                            if inj.treated { ", treated" } else { ", UNTREATED" }
                        ),
                        entity_ids: vec![cid.clone(), inj.id.clone()],
                        location_id: cp.current.current_location.clone(),
                        suggested_actions: vec![format!(
                            "character(action='health', sub_action='treat_injury', injury_id='{}', character_id='{cid}')",
                            inj.id
                        )],
                        visibility: "public".to_string(),
                        status: "active".to_string(),
                        source_event_seq: None,
                        created_at_world_time: world_time.to_string(),
                        updated_at_world_time: world_time.to_string(),
                    });
                }
            }
        }

        // 1d. Heavy / Overloaded Encumbrance
        let enc = compute_encumbrance(cp, &econ_map);
        if enc.load_tier == "overloaded" || enc.load_tier == "heavy" {
            push_derived(EngineSignal {
                id: String::new(),
                dedup_key: format!("encumbrance:{cid}:{}", enc.load_tier),
                code: "HEAVY_ENCUMBRANCE".to_string(),
                severity: if enc.load_tier == "overloaded" { "critical" } else { "warning" }.to_string(),
                category: "economy".to_string(),
                title: format!(
                    "{} — {} Load ({:.1}/{:.1} kg)",
                    cp.baseline.name,
                    enc.load_tier.to_uppercase(),
                    enc.total_weight_kg,
                    enc.carry_capacity_kg
                ),
                summary: format!(
                    "{} is carrying {:.1} kg{} (capacity {:.1} kg): travel speed x{:.2}, physical check penalty {:+}.",
                    cp.baseline.name,
                    enc.total_weight_kg,
                    if enc.stored_weight_kg > 0.0 {
                        format!(" ({:.1} kg stored separately)", enc.stored_weight_kg)
                    } else {
                        String::new()
                    },
                    enc.carry_capacity_kg,
                    enc.travel_speed_multiplier,
                    enc.physical_check_penalty
                ),
                entity_ids: vec![cid.clone()],
                location_id: cp.current.current_location.clone(),
                suggested_actions: vec!["Drop, stash, or tag items left at lodging/camp as 'stored' via character(op='equipment', sub_action='tag') to exclude them from carried load.".to_string()],
                visibility: "player".to_string(),
                status: "active".to_string(),
                source_event_seq: None,
                created_at_world_time: world_time.to_string(),
                updated_at_world_time: world_time.to_string(),
            });
        }
    }

    // 2. Clocks (filled active clocks or 1 segment away from completion)
    if let Ok(clocks) = db.list_clocks() {
        for cl in clocks {
            if !cl.status.eq_ignore_ascii_case("active") && !(cl.status.eq_ignore_ascii_case("completed") && !cl.consequence_on_fill.is_empty()) {
                continue;
            }
            if cl.status.eq_ignore_ascii_case("active") && cl.filled >= cl.segments {
                push_derived(EngineSignal {
                    id: String::new(),
                    dedup_key: format!("clock_filled:{}", cl.id),
                    code: "CLOCK_FILLED".to_string(),
                    severity: "critical".to_string(),
                    category: "clock".to_string(),
                    title: format!("Clock Filled: '{}' ({}/{})", cl.name, cl.filled, cl.segments),
                    summary: format!(
                        "Clock '{}' ({}) reached {}/{}. {}",
                        cl.name,
                        cl.kind,
                        cl.filled,
                        cl.segments,
                        if cl.consequence_on_fill.is_empty() {
                            cl.description.as_str()
                        } else {
                            cl.consequence_on_fill.as_str()
                        }
                    ),
                    entity_ids: cl.related_ids.clone(),
                    location_id: None,
                    suggested_actions: vec![format!(
                        "Resolve '{}' consequence in narrative and mark clock status='completed' or reset.",
                        cl.name
                    )],
                    visibility: "gm_secret".to_string(),
                    status: "active".to_string(),
                    source_event_seq: None,
                    created_at_world_time: world_time.to_string(),
                    updated_at_world_time: world_time.to_string(),
                });
            } else if cl.status.eq_ignore_ascii_case("active")
                && cl.segments >= 4
                && cl.filled == cl.segments - 1
            {
                push_derived(EngineSignal {
                    id: String::new(),
                    dedup_key: format!("clock_imminent:{}:{}", cl.id, cl.filled),
                    code: "CLOCK_NEAR_COMPLETION".to_string(),
                    severity: "warning".to_string(),
                    category: "clock".to_string(),
                    title: format!("Clock Imminent: '{}' ({}/{})", cl.name, cl.filled, cl.segments),
                    summary: format!(
                        "Clock '{}' is 1 segment from completion ({}/{}). {}",
                        cl.name, cl.filled, cl.segments, cl.description
                    ),
                    entity_ids: cl.related_ids.clone(),
                    location_id: None,
                    suggested_actions: vec![],
                    visibility: "gm_secret".to_string(),
                    status: "active".to_string(),
                    source_event_seq: None,
                    created_at_world_time: world_time.to_string(),
                    updated_at_world_time: world_time.to_string(),
                });
            }
        }
    }

    // 3. Pending Staged Loot / Discoveries
    if let Ok(discs) = db.list_staged_discoveries(None) {
        for d in discs {
            if d.status == "pending" || d.status == "partially_claimed" {
                let unclaimed_cnt = d.items.iter().filter(|i| i.claimed_by.is_none()).count();
                if unclaimed_cnt > 0 {
                    push_derived(EngineSignal {
                        id: String::new(),
                        dedup_key: format!("staged_discovery:{}", d.id),
                        code: "UNCLAIMED_DISCOVERY".to_string(),
                        severity: "info".to_string(),
                        category: "discovery".to_string(),
                        title: format!("Unclaimed Discovery Receipt `{}` ({} item(s))", d.id, unclaimed_cnt),
                        summary: format!(
                            "Staged discovery from '{}' has {} unclaimed item(s) waiting for explicit pickup.",
                            d.source_description, unclaimed_cnt
                        ),
                        entity_ids: vec![d.id.clone()],
                        location_id: d.location_id.clone(),
                        suggested_actions: vec![format!(
                            "economy(action='claim_discovery', discovery_id='{}', character_id='<cid>')",
                            d.id
                        )],
                        visibility: "gm_secret".to_string(),
                        status: "active".to_string(),
                        source_event_seq: None,
                        created_at_world_time: world_time.to_string(),
                        updated_at_world_time: world_time.to_string(),
                    });
                }
            }
        }
    }

    // 4. Active Warrants at Present Jurisdictions
    if let (Ok(jurisdictions), Ok(crimes)) = (db.list_jurisdictions(), db.list_crimes(None, None)) {
        for cr in crimes {
            if cr.warrant_status == "warrant_issued" {
                if let Some(cp) = all_chars.get(&cr.suspect_id) {
                    if let Some(ref cur_loc) = cp.current.current_location {
                        let in_jur = jurisdictions
                            .iter()
                            .find(|j| j.id == cr.jurisdiction_id)
                            .is_some_and(|j| j.place_ids.is_empty() || j.place_ids.contains(cur_loc));
                        if in_jur {
                            push_derived(EngineSignal {
                                id: String::new(),
                                dedup_key: format!("active_warrant:{}:{}", cr.id, cur_loc),
                                code: "ACTIVE_WARRANT".to_string(),
                                severity: "critical".to_string(),
                                category: "law".to_string(),
                                title: format!("Active Warrant for {} in `{}`!", cp.baseline.name, cr.jurisdiction_id),
                                summary: format!(
                                    "{} is currently at `{}` under an active warrant for '{}' (severity: {}, bounty: {} {}).",
                                    cp.baseline.name, cur_loc, cr.offense, cr.severity, cr.bounty, cr.bounty_currency
                                ),
                                entity_ids: vec![cr.suspect_id.clone(), cr.jurisdiction_id.clone()],
                                location_id: Some(cur_loc.clone()),
                                suggested_actions: vec!["Patrols/guards may recognize or question the suspect depending on disguise/visibility.".to_string()],
                                visibility: "gm_secret".to_string(),
                                status: "active".to_string(),
                                source_event_seq: None,
                                created_at_world_time: world_time.to_string(),
                                updated_at_world_time: world_time.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    // 5. Disrupted Market Supply Lines at Active Place
    if let Some(pid) = active_place_id {
        if let Ok(Some(market)) = db.get_market(pid) {
            if let Ok(meta) = db.get_campaign_meta() {
                let mut routes = db.list_travel_routes().unwrap_or_default();
                if routes.is_empty() {
                    routes = synthesize_routes_from_places(all_places);
                }
                let weather = all_places
                    .get(pid)
                    .map(|p| p.current.current_weather.as_str())
                    .filter(|w| !w.is_empty())
                    .unwrap_or(&meta.clock.global_weather);
                let rep = evaluate_market_prices(&market, &routes, &meta.clock.season, weather);
                if !rep.disrupted_supply_lines.is_empty() {
                    push_derived(EngineSignal {
                        id: String::new(),
                        dedup_key: format!("market_disrupted:{pid}"),
                        code: "MARKET_SUPPLY_DISRUPTED".to_string(),
                        severity: "warning".to_string(),
                        category: "economy".to_string(),
                        title: format!("Supply Line Disruption at '{}'", market.name),
                        summary: rep.disrupted_supply_lines.join(" | "),
                        entity_ids: vec![pid.to_string()],
                        location_id: Some(pid.to_string()),
                        suggested_actions: vec![format!("economy(action='get_market_prices', place_id='{pid}')")],
                        visibility: "public".to_string(),
                        status: "active".to_string(),
                        source_event_seq: None,
                        created_at_world_time: world_time.to_string(),
                        updated_at_world_time: world_time.to_string(),
                    });
                }
            }
        }
    }

    // 6. Recurring Figure / Patron / Rival / Fixer Convergence Radar (strictly GM-secret!)
    let mut watched_npcs: Vec<(&String, &CharacterProjection, crate::autonomy::CharacterProfileExt)> =
        Vec::new();
    for (cid, cp) in all_chars {
        let st = cp.current.alive_status.to_ascii_lowercase();
        if cp.baseline.kind.eq_ignore_ascii_case("pc")
            || matches!(
                st.as_str(),
                "dead" | "deceased" | "incapacitated" | "imprisoned" | "missing" | "coma" | "unconscious"
            )
        {
            continue;
        }
        if let Ok(Some(prof)) = db.get_character_profile(cid) {
            if prof.mobility.gm_convergence_alert {
                watched_npcs.push((cid, cp, prof));
            }
        }
    }

    if !watched_npcs.is_empty() {
        let pcs: Vec<(&String, &CharacterProjection, String)> = all_chars
            .iter()
            .filter(|(_, cp)| cp.baseline.kind.eq_ignore_ascii_case("pc"))
            .filter_map(|(id, cp)| {
                cp.current
                    .current_location
                    .clone()
                    .or_else(|| active_place_id.map(|s| s.to_string()))
                    .map(|loc| (id, cp, loc))
            })
            .collect();

        if !pcs.is_empty() {
            let mut routes = db.list_travel_routes().unwrap_or_default();
            if routes.is_empty() {
                routes = synthesize_routes_from_places(all_places);
            }
            let grids = db.list_map_grids().unwrap_or_default();
            let mut settlement_parent: BTreeMap<String, String> = BTreeMap::new();
            let mut grid_place_xy: BTreeMap<String, BTreeMap<String, (i32, i32)>> =
                BTreeMap::new();
            let mut grid_scale_km: BTreeMap<String, f64> = BTreeMap::new();

            for (pid, p) in all_places {
                if let Some(ref parent) = p.baseline.parent_place {
                    if !parent.trim().is_empty() {
                        settlement_parent.insert(pid.clone(), parent.clone());
                    }
                }
            }
            for g in &grids {
                if let Some(km) = grid_cell_scale_km(&g.cell_scale_label) {
                    grid_scale_km.insert(g.id.clone(), km);
                }
                if let Ok(cells) = db.list_map_cells(&g.id) {
                    let entry = grid_place_xy.entry(g.id.clone()).or_default();
                    for c in &cells {
                        if let Some(ref pid) = c.place_id {
                            entry.insert(pid.clone(), (c.x, c.y));
                            if g.scale_kind.eq_ignore_ascii_case("settlement") {
                                if let Some(ref parent) = g.parent_place_id {
                                    settlement_parent
                                        .entry(pid.clone())
                                        .or_insert_with(|| parent.clone());
                                }
                            }
                        }
                    }
                }
            }
            for r in &routes {
                if r.distance_km <= 2.0 {
                    if all_places.get(&r.to_place_id).is_some_and(|p| {
                        p.baseline.place_type.eq_ignore_ascii_case("settlement")
                    }) {
                        settlement_parent
                            .entry(r.from_place_id.clone())
                            .or_insert_with(|| r.to_place_id.clone());
                    }
                    if r.bidirectional
                        && all_places.get(&r.from_place_id).is_some_and(|p| {
                            p.baseline.place_type.eq_ignore_ascii_case("settlement")
                        })
                    {
                        settlement_parent
                            .entry(r.to_place_id.clone())
                            .or_insert_with(|| r.from_place_id.clone());
                    }
                }
            }

            let parent_of = |pid: &str| -> String {
                settlement_parent
                    .get(pid)
                    .cloned()
                    .unwrap_or_else(|| pid.to_string())
            };

            let compass_bearing = |dx: i32, dy: i32| -> &'static str {
                if dx == 0 && dy == 0 {
                    return "here";
                }
                let angle = (dy as f64).atan2(dx as f64).to_degrees();
                // Map grid y increases southward
                match angle {
                    a if (-22.5..22.5).contains(&a) => "E",
                    a if (22.5..67.5).contains(&a) => "SE",
                    a if (67.5..112.5).contains(&a) => "S",
                    a if (112.5..157.5).contains(&a) => "SW",
                    a if !(-157.5..157.5).contains(&a) => "W",
                    a if (-157.5..-112.5).contains(&a) => "NW",
                    a if (-112.5..-67.5).contains(&a) => "N",
                    _ => "NE",
                }
            };

            for (npc_id, npc_proj, prof) in &watched_npcs {
                let m = &prof.mobility;
                let npc_loc = npc_proj.current.current_location.as_deref();
                let npc_settlement = npc_loc.map(&parent_of);
                let radius_km = m.convergence_radius_km.clamp(1.0, 500.0);

                for (pc_id, pc_proj, pc_loc) in &pcs {
                    let pc_settlement = parent_of(pc_loc);
                    let mut match_sev: Option<&'static str> = None;
                    let mut proximity_desc = String::new();
                    let mut sig_loc: Option<String> = npc_loc.map(|s| s.to_string());

                    // 1. Same exact place (when not currently roaming out in the wilds)
                    if m.current_status != "roaming_wild" && npc_loc == Some(pc_loc.as_str()) {
                        match_sev = Some("warning");
                        proximity_desc = format!(
                            "at the exact same location (`{pc_loc}`) as {}",
                            pc_proj.baseline.name
                        );
                    }
                    // 2. Same settlement / district
                    else if m.current_status != "roaming_wild"
                        && npc_settlement.as_deref() == Some(pc_settlement.as_str())
                    {
                        match_sev = Some("warning");
                        proximity_desc = format!(
                            "in the same settlement (`{pc_settlement}`) as {} (NPC at `{}`; PC at `{pc_loc}`)",
                            pc_proj.baseline.name,
                            npc_loc.unwrap_or("unknown")
                        );
                    }
                    // 3. Regional 2D map grid distance <= convergence_radius_km
                    else {
                        let mut best_grid_match: Option<(f64, String, i32, i32, &'static str, String)> =
                            None;
                        for (gid, pmap) in &grid_place_xy {
                            let Some(&ckm) = grid_scale_km.get(gid) else {
                                continue;
                            };
                            let pc_xy = pmap
                                .get(pc_loc)
                                .or_else(|| pmap.get(&pc_settlement))
                                .copied();
                            let Some((px, py)) = pc_xy else {
                                continue;
                            };
                            let npc_xy_and_label: Option<((i32, i32), String)> =
                                if let Some(ref gp) = m.current_grid_pos {
                                    if gp.grid_id == *gid {
                                        let lbl = if !gp.label.trim().is_empty() {
                                            gp.label.clone()
                                        } else {
                                            format!("{} wilds", gp.terrain)
                                        };
                                        Some(((gp.x, gp.y), lbl))
                                    } else {
                                        None
                                    }
                                } else {
                                    npc_loc
                                        .and_then(|nl| pmap.get(nl).copied())
                                        .or_else(|| {
                                            npc_settlement
                                                .as_deref()
                                                .and_then(|ns| pmap.get(ns).copied())
                                        })
                                        .map(|xy| {
                                            (
                                                xy,
                                                npc_loc
                                                    .unwrap_or(pc_settlement.as_str())
                                                    .to_string(),
                                            )
                                        })
                                };

                            if let Some(((nx, ny), nlabel)) = npc_xy_and_label {
                                let dist_km = ((nx - px) as f64).hypot((ny - py) as f64) * ckm;
                                if dist_km <= radius_km {
                                    let bearing = compass_bearing(nx - px, ny - py);
                                    if best_grid_match
                                        .as_ref()
                                        .is_none_or(|(best_d, ..)| dist_km < *best_d)
                                    {
                                        best_grid_match = Some((
                                            dist_km,
                                            gid.clone(),
                                            nx,
                                            ny,
                                            bearing,
                                            nlabel,
                                        ));
                                    }
                                }
                            }
                        }

                        if let Some((dist_km, gid, nx, ny, bearing, nlabel)) = best_grid_match {
                            match_sev = Some(if dist_km <= 12.0 { "warning" } else { "info" });
                            proximity_desc = format!(
                                "~{:.0} km {bearing} of {} on `{gid}` at ({nx},{ny}) [{nlabel}]",
                                dist_km.round(),
                                pc_proj.baseline.name
                            );
                            sig_loc = Some(pc_loc.clone());
                        } else if m.current_status != "roaming_wild" {
                            // 4. 1 passable route hop away from PC's place or settlement
                            let one_hop = routes.iter().any(|r| {
                                if !r.is_passable() {
                                    return false;
                                }
                                let n_matches = |id: &str| {
                                    npc_loc == Some(id) || npc_settlement.as_deref() == Some(id)
                                };
                                let p_matches = |id: &str| id == pc_loc || id == pc_settlement;
                                (n_matches(&r.from_place_id) && p_matches(&r.to_place_id))
                                    || (r.bidirectional
                                        && n_matches(&r.to_place_id)
                                        && p_matches(&r.from_place_id))
                            });
                            if one_hop {
                                match_sev = Some("info");
                                proximity_desc = format!(
                                    "one route hop from {} (NPC at `{}`; PC at `{pc_loc}`)",
                                    pc_proj.baseline.name,
                                    npc_loc.unwrap_or("unknown")
                                );
                            }
                        }
                    }

                    if let Some(sev) = match_sev {
                        let role_label = if m.narrative_role.trim().is_empty() {
                            "RECURRING FIGURE".to_string()
                        } else {
                            m.narrative_role.trim().to_ascii_uppercase()
                        };
                        let conv_frag = m
                            .conveyance
                            .as_ref()
                            .filter(|c| !c.label.trim().is_empty())
                            .map(|c| format!(" Traveling via {} [{}].", c.label.trim(), c.signature))
                            .unwrap_or_default();
                        let alias_frag = m
                            .incognito_alias
                            .as_deref()
                            .filter(|a| !a.trim().is_empty())
                            .map(|a| format!(" Cover alias: '{a}'."))
                            .unwrap_or_default();
                        let activity_frag = m
                            .current_excursion_note
                            .as_deref()
                            .filter(|s| !s.trim().is_empty())
                            .map(|s| format!(" Activity: {s}."))
                            .or_else(|| {
                                m.active_trip
                                    .as_ref()
                                    .map(|t| format!(" Motive: {}.", t.purpose))
                            })
                            .unwrap_or_default();

                        let gm_prompt = match m.narrative_role.trim().to_ascii_lowercase().as_str()
                        {
                            "patron" => "GM choice: cross paths on your terms (test, pressure, bargain, or torment the PC), drop a conveyance/presence omen, or let them pass unseen.",
                            "rival" => "GM choice: stage a competitive clash over a lead/resource, leave proof they arrived first, or let them pass unseen.",
                            "fixer" => "GM choice: surface a brokered rumor, shady deal, or leverage play via an intermediary, or let them pass unseen.",
                            "hunter" => "GM choice: tighten pursuit tension (watchers spotted, locals questioned, narrowed trail) or hold the strike for later.",
                            _ => "GM choice: trigger an organic crossing, plant a subtle sighting/rumor, or let them pass unseen.",
                        };

                        push_derived(EngineSignal {
                            id: String::new(),
                            dedup_key: format!("npc_convergence:{npc_id}:{pc_id}"),
                            code: "NPC_CONVERGENCE_OPPORTUNITY".to_string(),
                            severity: sev.to_string(),
                            category: "world".to_string(),
                            title: format!(
                                "[{role_label}] {} Near {}",
                                npc_proj.baseline.name, pc_proj.baseline.name
                            ),
                            summary: format!(
                                "{} (`{npc_id}`, status: {}) is {proximity_desc}.{activity_frag}{conv_frag}{alias_frag}",
                                npc_proj.baseline.name, m.current_status
                            ),
                            entity_ids: vec![(*npc_id).clone(), (*pc_id).clone()],
                            location_id: sig_loc,
                            suggested_actions: vec![gm_prompt.to_string()],
                            visibility: "gm_secret".to_string(),
                            status: "active".to_string(),
                            source_event_seq: None,
                            created_at_world_time: world_time.to_string(),
                            updated_at_world_time: world_time.to_string(),
                        });
                    }
                }
            }
        }
    }

    let mut out: Vec<EngineSignal> = by_dedup.into_values().collect();
    out.sort_by(|a, b| {
        severity_rank(&b.severity)
            .cmp(&severity_rank(&a.severity))
            .then_with(|| b.source_event_seq.unwrap_or(0).cmp(&a.source_event_seq.unwrap_or(0)))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(out)
}

/// Filters active engine signals strictly according to epistemic perspective:
/// - `gm` (with `include_gm_secrets=true`): sees all signals (`public`, `player`, `gm_secret`).
/// - `player`: sees `public` and `player` signals (never `gm_secret`).
/// - `<character_id>`: sees `public` signals, plus `player` signals ONLY if `<character_id>` is a PC or explicitly listed in `entity_ids`.
pub fn filter_signals_for_perspective(
    signals: Vec<EngineSignal>,
    perspective: &str,
    pov_actor_id: Option<&str>,
    pc_ids: &BTreeSet<String>,
    include_gm_secrets: bool,
) -> Vec<EngineSignal> {
    if perspective.eq_ignore_ascii_case("gm") && include_gm_secrets {
        return signals;
    }
    let is_player_pov = perspective.eq_ignore_ascii_case("player")
        || pov_actor_id.is_some_and(|aid| pc_ids.contains(aid));

    signals
        .into_iter()
        .filter(|s| match s.visibility.as_str() {
            "gm_secret" => false,
            "player" => {
                if is_player_pov {
                    true
                } else if let Some(aid) = pov_actor_id {
                    s.entity_ids.iter().any(|e| e == aid)
                } else {
                    false
                }
            }
            _ => true,
        })
        .collect()
}

pub fn render_signals_markdown(signals: &[EngineSignal]) -> String {
    if signals.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let _ = writeln!(out, "## Engine Alarms & Active Directives ({})", signals.len());
    let _ = writeln!(
        out,
        "> *(Ledger advisories for the GM — you hold full narrative authority to invoke, time, adapt, or override any story hook.)*"
    );
    for s in signals {
        let badge = match s.severity.as_str() {
            "critical" => "[CRITICAL]",
            "warning" => "[WARNING]",
            _ => "[INFO]",
        };
        let sec_badge = if s.visibility.eq_ignore_ascii_case("gm_secret") {
            " [GM SECRET — DO NOT DISCLOSE]"
        } else {
            ""
        };
        let _ = write!(
            out,
            "- **{badge}{sec_badge} `{}` — {}:** {}",
            s.code, s.title, s.summary
        );
        if !s.suggested_actions.is_empty() {
            let _ = write!(out, " *(Next: {})*", s.suggested_actions.join(" ; "));
        }
        let _ = writeln!(out);
    }
    let _ = writeln!(out);
    out
}
