use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

/// Cached strict schema specification for an underlying Chronicle MCP tool.
#[derive(Debug, Clone)]
pub struct ToolSchemaSpec {
    pub tool: String,
    pub description: String,
    pub required: Vec<String>,
    pub accepted_keys: BTreeSet<String>,
    pub input_schema: Value,
}

/// Top-level routing keys forwarded by domain facades that are always stripped or ignored
/// when validating an underlying resolved tool's arguments.
const ROUTING_META_KEYS: &[&str] = &["op", "sub_action", "mode"];

/// Extra verified handler keys / serde aliases that may not appear in legacy `inputSchema.properties`
/// (including new cohesive cascade parameters such as `weight_kg` on equipment/transactions and
/// grid placement on `create_place`).
fn extra_runtime_keys(tool: &str) -> &'static [&'static str] {
    match tool {
        "get_scene_context" => &[
            "place_id",
            "location_id",
            "character_ids",
            "scene_text",
            "recent_events_limit",
            "include_gm_secrets",
            "perspective",
            "format",
            "max_chars",
        ],
        "create_character" => &[
            "id",
            "name",
            "kind",
            "title",
            "archetype",
            "faction",
            "appearance",
            "personality",
            "backstory",
            "secrets",
            "initial_location",
            "initial_stats",
            "initial_resources",
            "initial_equipment",
            "initial_skills",
            "initial_techniques",
            "initial_goals",
            "tags",
            "notes",
        ],
        "get_character" => &[
            "character_id",
            "as_of_seq",
            "include_full_event_log",
            "event_log_limit",
            "perspective",
            "include_gm_secrets",
        ],
        "update_character_equipment" => &[
            "action",
            "character_id",
            "name",
            "item_name",
            "slot",
            "quantity",
            "equipped",
            "durability",
            "tags",
            "tag",
            "tag_match",
            "exclude_tags",
            "query",
            "limit",
            "add_tags",
            "remove_tags",
            "set_tags",
            "replace_tags",
            "items",
            "description",
            "garment_measurements",
            "garment_cut",
            "garment_taper",
            "weight_kg",
            "base_value",
            "unit_value",
            "currency",
            "volume_liters",
            "sync_catalog",
            "reason",
        ],
        "update_character_condition" => &[
            "action",
            "character_id",
            "name",
            "kind",
            "severity",
            "duration",
            "source",
            "notes",
            "reason",
        ],
        "upsert_character_skill" => &[
            "character_id",
            "name",
            "category",
            "rank",
            "governing_stat",
            "modifier",
            "description",
            "proficiency_tier",
            "delta_xp",
            "set_xp",
            "reset_xp",
            "profile",
            "notes",
            "reason",
        ],
        "get_attachment" => &[
            "attachment_id",
            "include_text_content",
            "max_chars",
            "offset_chars",
            "perspective",
            "include_gm_secrets",
        ],
        "upsert_character_technique" => &[
            "character_id",
            "name",
            "tier",
            "mastery",
            "cost",
            "cooldown",
            "effect",
            "description",
            "progression_track_id",
            "source_text_id",
            "source_item_id",
            "manual_title",
            "manual_notes",
            "reason",
        ],
        "update_character_profile" => &[
            "character_id",
            "get_only",
            "gender",
            "species",
            "age_or_birthdate",
            "body_constitution",
            "body_tempering",
            "temperament",
            "moral_alignment",
            "values",
            "desires",
            "fears",
            "taboos",
            "flaws",
            "speech_style",
            "mannerisms",
            "daily_routine",
            "long_term_agenda",
            "known_aliases",
            "gm_notes",
            "reason",
        ],
        "manage_player_character" => &[
            "action",
            "character_id",
            "reason",
            // Avatar & measurements
            "avatar_summary",
            "appearance_stage",
            "height",
            "weight",
            "baseline_height",
            "baseline_weight",
            "chronological_age",
            "apparent_age",
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
            "visual_prompt_anchor",
            "active_outfit",
            "gender",
            "species",
            "age_or_birthdate",
            "body_constitution",
            "body_tempering",
            "lifespan_years",
            "portrait_attachment_id",
            "measurements",
            "replace_measurements",
            "baseline_measurements",
            "replace_baseline_measurements",
            "set_measurement",
            "set_baseline_measurement",
            "remove_measurement",
            "record_checkpoint",
            "checkpoint_summary",
            "transformation_log",
            // Markers
            "id",
            "kind",
            "body_location",
            "description",
            "origin",
            "visibility",
            "acquired_at",
            // Outfits
            "name",
            "occasion",
            "headwear",
            "outer_layer",
            "top_layer",
            "bottoms",
            "footwear",
            "accessories",
            "color_palette",
            "items",
            "style_notes",
            "fit_notes",
            "condition",
            "set_active",
            "outfit_id",
            "sync_equipment",
            // Titles
            "equipped",
            "source",
            "effects",
            "stat_modifiers",
            "notes",
            // Stat XP
            "stat",
            "delta_xp",
            "xp",
            "set_xp",
            "xp_to_next",
            "unspent_stat_points_delta",
            "set_unspent_stat_points",
            "allocate_points",
            // Jobs
            "job_id",
            "role",
            "tier_or_rank",
            "level",
            "current_xp",
            "cumulative_xp",
            "active",
            "governing_stats",
            "perks_and_features",
            // Proficiencies
            "proficiency_id",
            "category",
            "rank",
            "bonus",
            // Traits
            "trait_id",
            "innate",
            "mechanical_effects",
            "passive_bonuses",
        ],
        "manage_progression" => &[
            "action",
            "character_id",
            "track_id",
            "ladder_id",
            "id",
            "name",
            "preset",
            "description",
            "stages",
            "track_kind",
            "kind",
            "major_stage_index",
            "major_stage_name",
            "sub_stage",
            "sub_stage_label",
            "current_xp",
            "set_current_xp",
            "cumulative_xp",
            "set_cumulative_xp",
            "xp",
            "add_xp",
            "delta_xp",
            "reset_stage_xp",
            "reset_track",
            "allow_demote",
            "status",
            "bottleneck_reason",
            "halt_conditions",
            "source_text_id",
            "source_item_id",
            "manual_title",
            "manual_notes",
            "active_properties",
            "check_tags",
            "default_check_tags",
            "stage_rules",
            "rule",
            "gm_notes",
            "notes",
            "profile",
            "stat",
            "skill",
            "difficulty",
            "dc_tier",
            "dc_factors",
            "modifier",
            "advantage",
            "stakes",
            "seed",
            "next_stage_name",
            "reason",
            "visibility",
            "perspective",
            "include_gm_secrets",
        ],
        "create_place" => &[
            "id",
            "name",
            "place_type",
            "region",
            "terrain",
            "climate",
            "description",
            "controlling_faction",
            "danger_level",
            "initial_weather",
            "initial_landmarks",
            "initial_routes",
            "tags",
            "secrets",
            "notes",
            // Cohesive map-grid placement cascade
            "grid_id",
            "x",
            "y",
            "fog_status",
            "passable",
            "hazards",
            "label",
        ],
        "update_place" => &[
            "place_id",
            "action",
            "weather",
            "condition",
            "faction",
            "danger_level",
            "landmark_name",
            "landmark_kind",
            "landmark_status",
            "description",
            "tags",
            "destination_id",
            "travel_time",
            "terrain_difficulty",
            "route_status",
            "notes",
            // Optional map-grid sync
            "grid_id",
            "x",
            "y",
            "fog_status",
        ],
        "manage_clock" => &[
            "action",
            "clock_id",
            "id",
            "name",
            "kind",
            "segments",
            "filled",
            "delta",
            "cadence_hours",
            "auto_reset",
            "description",
            "consequence_on_fill",
            "status",
            "related_ids",
            "reason",
        ],
        "manage_travel_and_exploration" => &[
            "action",
            "id",
            "route_id",
            "name",
            "from_place_id",
            "to_place_id",
            "distance_km",
            "travel_hours",
            "terrain",
            "status",
            "bidirectional",
            "toll_cost",
            "toll_currency",
            "hazards",
            "jurisdiction",
            "patrol_presence",
            "elevation_m",
            "notes",
            "transport_mode",
            "mode",
            "forced_march",
            "pay_tolls_from",
            "character_ids",
            "actor_id",
            "place_id",
            "region_id",
            "parent_place_id",
            "region_x",
            "region_y",
            "skill",
            "difficulty",
            "table_id",
            "hazard_table_id",
            "seed",
            "description",
            "reason",
            // 2D Map Grids
            "grid_id",
            "scale_kind",
            "width",
            "height",
            "cell_scale_label",
            "default_terrain",
            "x",
            "y",
            "label",
            "landmark_name",
            "faction_control",
            "fog_status",
            "passable",
            "cells",
            "center_x",
            "center_y",
            "radius",
            "gm_notes",
            "perspective",
            "include_gm_secrets",
        ],
        "manage_economy" => &[
            "action",
            "owner_id",
            "from_id",
            "to_id",
            "buyer_id",
            "seller_id",
            "character_id",
            "place_id",
            "amount",
            "currency",
            "base_currency",
            "kind",
            "tax_rate_pct",
            "tax_rate",
            "tax_recipient_id",
            "item_name",
            "item_key",
            "name",
            "quantity",
            "item_quantity",
            "weight_kg",
            "volume_liters",
            "base_value",
            "unit_value",
            "base_price",
            "price",
            "stackable",
            "quality",
            "requirements",
            // Cohesive purchase-to-equipment/catalog cascade fields
            "slot",
            "tags",
            "description",
            "durability",
            "garment_measurements",
            "garment_cut",
            "garment_taper",
            "equip_on_buy",
            "catalog_item",
            "item_type",
            // Markets
            "market_name",
            "prosperity",
            "commodities",
            "commodity",
            "commodity_name",
            "supply_delta",
            "demand_delta",
            "notes",
            "reason",
        ],
        "upsert_item" => &[
            "id",
            "name",
            "item_type",
            "current_holder_id",
            "previous_owners",
            "origin_and_maker",
            "significance",
            "properties_and_effects",
            "condition_or_durability",
            "rarity_or_tier",
            "slot",
            "quantity",
            "equipped",
            "linked_text_id",
            "description",
            "garment_measurements",
            "garment_cut",
            "garment_taper",
            "weight_kg",
            "base_value",
            "unit_value",
            "currency",
            "volume_liters",
            "keywords",
            "tags",
            "related_ids",
            "gm_notes",
            "notes",
        ],
        "tag_catalog_item" => &[
            "item_id",
            "id",
            "tags",
            "add_tags",
            "remove_tags",
            "set_tags",
            "replace_tags",
            "notes",
            "reason",
        ],
        "upsert_relation" => &[
            "source_id",
            "source_kind",
            "relation_type",
            "relationship",
            "target_id",
            "target_kind",
            "strength",
            "dimensions",
            "direction",
            "visibility",
            "obligations",
            "origin_event_id",
            "notes",
        ],
        "record_knowledge" => &[
            "claim_id",
            "id",
            "subject",
            "claim",
            "truth_status",
            "origin_source",
            "source",
            "source_event_id",
            "visibility",
            "known_by",
            "disbelieved_by",
            "belief",
            "beliefs",
            "holders",
            "perceived_claim",
            "stance",
            "learned_at",
            "source_trustworthy",
            "supersedes_claim_id",
            "tags",
            "related_ids",
            "notes",
        ],
        "propagate_knowledge" => &[
            "claim_id",
            "from_actor_id",
            "to_actor_ids",
            "stance",
            "distorted_claim",
            "notes",
        ],
        "manage_faction" => &[
            "action",
            "faction_id",
            "id",
            "name",
            "faction_type",
            "origins",
            "leadership_model",
            "ideology",
            "established_territory",
            "territory",
            "controlled_places",
            "institutional_structure",
            "capacity_tier",
            "leader_id",
            "members",
            "resources",
            "resource",
            "reputation",
            "objectives",
            "internal_conflicts",
            "active_schemes",
            "delta",
            "set_value",
            "goals",
            "active_operations",
            "morale",
            "cohesion",
            "treasury",
            "status",
            "visibility",
            "lore_id",
            "character_id",
            "role",
            "rank",
            "loyalty_note",
            "tags",
            "notes",
            "gm_notes",
            "reason",
        ],
        "manage_law_and_reputation" => &[
            "action",
            "jurisdiction_id",
            "id",
            "name",
            "place_id",
            "place_ids",
            "controlling_faction_id",
            "enforcement_level",
            "laws",
            "penalties",
            "crime_id",
            "suspect_id",
            "offense",
            "severity",
            "evidence",
            "witnesses",
            "bounty",
            "currency",
            "warrant_status",
            "entity_id",
            "target_group_id",
            "reputation_delta",
            "delta",
            "set_value",
            "standing",
            "notes",
            "reason",
        ],
        "simulate_autonomy" => &[
            "action",
            "sub_action",
            "scope_type",
            "actor_ids",
            "character_id",
            "faction_id",
            "location_id",
            "destination_place_id",
            "uncharted_label",
            "purpose",
            "purposes",
            "duration_days",
            "duration_hours",
            "stay_days",
            "mode",
            "visibility",
            "details",
            "notes",
            "outcomes",
            "actions",
            "mobility_mode",
            "prominent",
            "roam_strategy",
            "narrative_role",
            "home_place_id",
            "roam_grid_id",
            "roam_terrains",
            "wild_roam_chance",
            "pc_proximity_bias",
            "gm_convergence_alert",
            "convergence_radius_km",
            "incognito_alias",
            "conveyance",
            "preferred_destinations",
            "trip_cadence_days",
            "stay_duration_days",
            "travel_speed_mode",
            "entourage_ids",
            "seed",
        ],
        "manage_health_and_downtime" => &[
            "action",
            "character_id",
            "injury_id",
            "name",
            "body_location",
            "severity",
            "recovery_days",
            "source_event_id",
            "days_reduced",
            "days",
            "active_only",
            "activity",
            "stat",
            "skill",
            "technique",
            "progression_track",
            "progression_kind",
            "equipment",
            "dc_tier",
            "dc_factors",
            "difficulty",
            "modifier",
            "advantage",
            "stakes",
            "profile",
            "dice_override",
            "situational_modifiers",
            "seed",
            "complications",
            "project_id",
            "hours_worked",
            "total_hours_required",
            "materials",
            "output_item_name",
            "notes",
            "reason",
        ],
        "resolve_check" => &[
            "actor_id",
            "action",
            "stat",
            "skill",
            "technique",
            "progression_track",
            "equipment",
            "dc_tier",
            "dc_factors",
            "difficulty",
            "modifier",
            "situational_modifiers",
            "advantage",
            "stakes",
            "profile",
            "dice_override",
            "auto_hydrate",
            "award_skill_xp",
            "dry_run",
            "visibility",
            "seed",
        ],
        "update_campaign_meta" => &[
            "name",
            "campaign_name",
            "description",
            "ruleset_notes",
            "ruleset",
            "notes",
            "protocol",
            "gm_notes",
            "append_ruleset_notes",
            "append_ruleset",
            "append_notes",
            "append_protocol",
            "global_weather",
            "reason",
        ],
        "commit_turn" => &[
            "advance_minutes",
            "time_reason",
            "actions",
            "log",
            "narrative_title",
            "narrative_details",
            "location_id",
            "affected_entities",
            "tags",
            "visibility",
            "causal_event_ids",
            "compact",
        ],
        "manage_signals" => &[
            "op",
            "action",
            "signal_id",
            "id",
            "dedup_key",
            "perspective",
            "place_id",
            "character_ids",
            "include_derived",
            "code",
            "severity",
            "category",
            "title",
            "summary",
            "entity_ids",
            "location_id",
            "suggested_actions",
            "visibility",
        ],
        "manage_routines" => &[
            "op",
            "action",
            "character_id",
            "routine_id",
            "id",
            "routine",
            "name",
            "category",
            "cadence_hours",
            "enabled",
            "visibility",
            "notes",
        ],
        "get_lore" | "search_lore" | "get_text" | "search_texts" | "search_all" => &[
            "lore_id",
            "text_id",
            "query",
            "category",
            "subject",
            "medium",
            "read_status",
            "holder_or_location_id",
            "tag",
            "visibility",
            "perspective",
            "include_gm_secrets",
            "limit",
        ],
        _ => &[],
    }
}

fn build_schema_registry() -> HashMap<String, ToolSchemaSpec> {
    let mut map = HashMap::new();
    let all = crate::mcp::all_tool_definitions();
    if let Some(arr) = all.as_array() {
        for tool_def in arr {
            let Some(name) = tool_def.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let description = tool_def
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let input_schema = tool_def
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object", "properties": {} }));
            let mut accepted_keys = BTreeSet::new();
            if let Some(props) = input_schema.get("properties").and_then(|v| v.as_object()) {
                for k in props.keys() {
                    accepted_keys.insert(k.clone());
                }
            }
            for &extra in extra_runtime_keys(name) {
                accepted_keys.insert(extra.to_string());
            }
            let required = input_schema
                .get("required")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            map.insert(
                name.to_string(),
                ToolSchemaSpec {
                    tool: name.to_string(),
                    description,
                    required,
                    accepted_keys,
                    input_schema,
                },
            );
        }
    }
    map
}

pub fn schema_registry() -> &'static HashMap<String, ToolSchemaSpec> {
    static REGISTRY: OnceLock<HashMap<String, ToolSchemaSpec>> = OnceLock::new();
    REGISTRY.get_or_init(build_schema_registry)
}

/// Strict unknown-key validator invoked centrally before executing any resolved Chronicle tool.
/// Rejects unrecognized/guessed keys with an actionable, self-correcting list of accepted keys.
pub fn validate_tool_args(tool_name: &str, args: &Value) -> Result<()> {
    // Domain facades validate once they resolve to their concrete underlying tool.
    if matches!(
        tool_name,
        "scene"
            | "query"
            | "resolve"
            | "character"
            | "progression"
            | "world"
            | "society"
            | "codex"
            | "economy"
            | "admin"
    ) {
        return Ok(());
    }
    let Some(spec) = schema_registry().get(tool_name) else {
        return Ok(());
    };
    let Some(obj) = args.as_object() else {
        return Ok(());
    };
    // Tools with intentionally empty property maps (like `campaign_status`) reject non-routing keys.
    let mut unknown: Vec<String> = Vec::new();
    for key in obj.keys() {
        if ROUTING_META_KEYS.contains(&key.as_str()) {
            continue;
        }
        if !spec.accepted_keys.contains(key) {
            unknown.push(key.clone());
        }
    }
    if unknown.is_empty() {
        return Ok(());
    }
    unknown.sort();
    let accepted: Vec<&str> = spec.accepted_keys.iter().map(|s| s.as_str()).collect();
    Err(anyhow!(
        "Rejected unknown parameter(s) [{}] for '{}'. Accepted parameters: [{}]. Tip: Call `<domain>(op='help', target_op='{}')` or `query(op='describe_op', tool='{}')` for exact field types & cascade rules.",
        unknown.join(", "),
        tool_name,
        accepted.join(", "),
        tool_name,
        tool_name
    ))
}

/// Enriches missing-field or deserialization errors with the tool's exact accepted parameter list
/// so the model self-corrects on the very next call instead of guessing keys or falling back to prose.
pub fn enrich_tool_error(tool_name: &str, err: anyhow::Error) -> anyhow::Error {
    if matches!(
        tool_name,
        "scene"
            | "query"
            | "resolve"
            | "character"
            | "progression"
            | "world"
            | "society"
            | "codex"
            | "economy"
            | "admin"
    ) {
        return err;
    }
    let msg = err.to_string();
    if msg.contains("Accepted parameters") || msg.contains("Rejected unknown parameter") {
        return err;
    }
    let looks_like_schema_err = msg.contains("Missing '")
        || msg.contains("missing field")
        || msg.contains("requires '")
        || msg.contains("Invalid '")
        || msg.contains("Unknown action")
        || msg.contains("unknown variant");
    if !looks_like_schema_err {
        return err;
    }
    if let Some(spec) = schema_registry().get(tool_name) {
        let accepted: Vec<&str> = spec.accepted_keys.iter().map(|s| s.as_str()).collect();
        if !accepted.is_empty() {
            return anyhow!(
                "{msg} | Accepted parameters for '{tool_name}': [{}]. Call `query(op='describe_op', tool='{tool_name}')` for full schema.",
                accepted.join(", ")
            );
        }
    }
    err
}

/// Detects when structured numeric/key-value specs (weight, measurements, garment sizing, stats)
/// are shoved into prose fields (`notes`/`description`/`appearance`/`backstory`) while the real
/// structured fields were left empty.
pub fn detect_unstructured_in_structured(tool_name: &str, args: &Value) -> Vec<String> {
    let mut warnings = Vec::new();
    let Some(obj) = args.as_object() else {
        return warnings;
    };
    let prose = [
        obj.get("description").and_then(|v| v.as_str()).unwrap_or(""),
        obj.get("notes").and_then(|v| v.as_str()).unwrap_or(""),
        obj.get("appearance").and_then(|v| v.as_str()).unwrap_or(""),
        obj.get("backstory").and_then(|v| v.as_str()).unwrap_or(""),
    ]
    .join("\n")
    .to_ascii_lowercase();

    if prose.trim().is_empty() {
        return warnings;
    }

    let has_kv_token = |tokens: &[&str]| -> bool {
        tokens.iter().any(|tok| prose.contains(tok))
    };

    if matches!(
        tool_name,
        "update_character_equipment" | "upsert_item" | "manage_economy"
    ) {
        let has_weight_field = obj.get("weight_kg").and_then(|v| v.as_f64()).is_some();
        if !has_weight_field
            && has_kv_token(&["weight:", "mass:", "weighs ", "weight =", "wt:"])
            && (prose.contains("kg") || prose.contains("lb") || prose.contains("jin") || prose.contains(" g"))
        {
            warnings.push(
                "Structured-field lint: detected weight text in description/notes while 'weight_kg' was omitted. Pass numeric 'weight_kg' directly so Chronicle automatically syncs item_economics & encumbrance."
                    .to_string(),
            );
        }
        let has_garment_field = obj
            .get("garment_measurements")
            .and_then(|v| v.as_object())
            .map(|m| !m.is_empty())
            .unwrap_or(false);
        if !has_garment_field
            && has_kv_token(&["chest:", "waist:", "shoulders:", "inseam:", "sleeve:", "hips:"])
        {
            warnings.push(
                "Structured-field lint: detected garment measurements in description/notes while 'garment_measurements' was omitted. Pass 'garment_measurements' as a structured map so Chronicle evaluates garment fit automatically."
                    .to_string(),
            );
        }
    }

    if matches!(tool_name, "manage_player_character" | "create_character") {
        let has_meas = obj
            .get("measurements")
            .and_then(|v| v.as_object())
            .map(|m| !m.is_empty())
            .unwrap_or(false);
        if !has_meas
            && has_kv_token(&["chest:", "waist:", "hips:", "shoulders:", "neck:", "bicep:"])
        {
            warnings.push(
                "Structured-field lint: detected body measurements in prose while 'measurements' map was omitted. Pass structured 'measurements' so garment fit & progression cascades stay in sync."
                    .to_string(),
            );
        }
    }

    if tool_name == "create_character" {
        let has_stats = obj
            .get("initial_stats")
            .and_then(|v| v.as_object())
            .map(|m| !m.is_empty())
            .unwrap_or(false);
        if !has_stats
            && has_kv_token(&["str:", "dex:", "con:", "int:", "wis:", "cha:", "agi:"])
        {
            warnings.push(
                "Structured-field lint: detected stat lines in prose while 'initial_stats' was omitted. Pass 'initial_stats' as a structured map."
                    .to_string(),
            );
        }
    }

    warnings
}

/// Resolves a domain + target_op pair (or direct legacy tool name) to its underlying tool name.
pub fn resolve_tool_name_for_help(domain: &str, target_op: Option<&str>) -> Option<String> {
    let d = domain.trim().to_ascii_lowercase();
    let op = target_op.map(|s| s.trim().to_ascii_lowercase());
    let is_domain_facade = matches!(
        d.as_str(),
        "scene"
            | "query"
            | "resolve"
            | "character"
            | "progression"
            | "world"
            | "society"
            | "codex"
            | "economy"
            | "admin"
    );
    let Some(op_s) = op else {
        if !is_domain_facade && schema_registry().contains_key(&d) {
            return Some(d);
        }
        return None;
    };
    if schema_registry().contains_key(&op_s)
        && !matches!(
            op_s.as_str(),
            "scene"
                | "query"
                | "resolve"
                | "character"
                | "progression"
                | "world"
                | "society"
                | "codex"
                | "economy"
                | "admin"
        )
    {
        return Some(op_s);
    }
    let mapped = match (d.as_str(), op_s.as_str()) {
        ("scene", "context") => "get_scene_context",
        ("scene", "status") => "campaign_status",
        ("scene", "signals" | "ack_signal" | "resolve_signal" | "emit_signal") => "manage_signals",
        ("scene", "start_session") => "start_new_session",
        ("query", "read_file") => "read_campaign_file",
        ("query", "search_events") => "search_history",
        ("resolve", "check") => "resolve_check",
        ("resolve", "contest") => "resolve_contest",
        ("resolve", "draw_table") => "draw_random_table",
        ("resolve", "roll_dice" | "roll") => "roll_dice",
        ("resolve", "ruleset") => "upsert_ruleset_profile",
        ("resolve", "random_table") => "upsert_random_table",
        ("resolve", "encounter") => "manage_encounter",
        ("character", "create") => "create_character",
        ("character", "get") => "get_character",
        ("character", "move") => "move_character",
        ("character", "resource") => "modify_character_resource",
        ("character", "stat") => "modify_character_stat",
        ("character", "status") => "set_character_status",
        ("character", "condition") => "update_character_condition",
        ("character", "equipment") => "update_character_equipment",
        ("character", "transfer_item") => "transfer_item",
        ("character", "skill") => "upsert_character_skill",
        ("character", "technique") => "upsert_character_technique",
        ("character", "goal") => "update_character_goal",
        ("character", "rest") => "rest_characters",
        ("character", "profile") => "update_character_profile",
        ("character", "pc_sheet") => "manage_player_character",
        ("character", "health") => "manage_health_and_downtime",
        ("character", "routine") => "manage_routines",
        ("progression", _) => "manage_progression",
        ("world", "advance_time" | "time") => "advance_time",
        ("world", "advance_world") => "advance_world",
        ("world", "create_place") => "create_place",
        ("world", "get_place") => "get_place",
        ("world", "update_place") => "update_place",
        ("world", "weather") => "manage_weather",
        ("world", "clock") => "manage_clock",
        ("world", "travel" | "set_cell" | "upsert_grid" | "get_grid" | "upsert_route") => {
            "manage_travel_and_exploration"
        }
        ("society", "relation") => "upsert_relation",
        ("society", "remove_relation") => "remove_relation",
        ("society", "record_knowledge") => "record_knowledge",
        ("society", "propagate_knowledge") => "propagate_knowledge",
        ("society", "faction") => "manage_faction",
        ("society", "law") => "manage_law_and_reputation",
        ("society", "autonomy") => "simulate_autonomy",
        ("codex", "upsert_lore" | "lore") => "upsert_lore",
        ("codex", "upsert_text" | "text") => "upsert_text",
        ("codex", "record_text_reading") => "record_text_reading",
        ("codex", "add_history") => "add_entity_history",
        ("codex", "import_attachment") => "import_attachment",
        ("codex", "create_reference_doc") => "create_reference_doc",
        ("codex", "log_event") => "log_event",
        ("economy", "upsert_item") => "upsert_item",
        ("economy", "tag_item") => "tag_catalog_item",
        ("economy", "transfer_catalog_item") => "transfer_catalog_item",
        ("economy", "claim_discovery") => "claim_discovery",
        ("economy", _) => "manage_economy",
        ("admin", "diagnose") => "diagnose_campaign",
        ("admin", "cleanup") => "cleanup_campaign",
        ("admin", "backup") => "backup_campaign",
        ("admin", "restore_backup") => "restore_backup",
        ("admin", "rollback") => "rollback_to_event",
        ("admin", "compile_docs") => "compile_docs",
        ("admin", "update_meta") => "update_campaign_meta",
        ("admin", "reclassify") => "reclassify_entity",
        ("admin", "rename_or_merge") => "rename_or_merge_entity",
        ("admin", "delete_entity") => "delete_entity",
        _ => return None,
    };
    Some(mapped.to_string())
}

/// On-demand schema & cascade introspector:
/// - `<domain>(op='help')` returns organized operation groups, compact signatures, and automatic 1st/2nd-order cascades.
/// - `<domain>(op='help', target_op='...')` or `query(op='describe_op', tool='...')` returns the exact full JSON schema.
pub fn describe_domain_help(
    domain: &str,
    target_op: Option<&str>,
    sub_action: Option<&str>,
) -> Value {
    if let Some(resolved) = resolve_tool_name_for_help(domain, target_op) {
        if let Some(spec) = schema_registry().get(&resolved) {
            let accepted: Vec<&str> = spec.accepted_keys.iter().map(|s| s.as_str()).collect();
            return json!({
                "domain": domain,
                "resolved_tool": spec.tool,
                "sub_action": sub_action,
                "description": spec.description,
                "required": spec.required,
                "accepted_keys": accepted,
                "inputSchema": spec.input_schema,
                "cascades": cascades_for_domain(domain)
            });
        }
    }

    let mut groups: BTreeMap<&str, Value> = BTreeMap::new();
    match domain.trim().to_ascii_lowercase().as_str() {
        "character" => {
            groups.insert("1_lifecycle_and_state", json!([
                "create(name, kind, title?, archetype?, faction?, appearance?, personality?, backstory?, initial_location?, initial_stats?, initial_resources?, initial_equipment?)",
                "get(character_id, as_of_seq?)",
                "sync_state(character_id, location_id?, alive_status?, stats?, resources?, add_conditions?, remove_conditions?, avatar?, profile?, reason?)  <-- ONE-CALL composite update with full cascades!",
                "move(character_id, to_place_id, travel_notes?)",
                "status(character_id, alive_status, reason?)",
                "rest(character_ids, rest_type, advance_minutes?, clear_conditions?)"
            ]));
            groups.insert("2_vitals_stats_and_abilities", json!([
                "resource(character_id, resource, delta?|set_current?|set_max?, unit?, remove?, reason?)",
                "stat(character_id, stat, delta?|set_value?, reason?)",
                "condition(sub_action=add|remove, character_id, name, kind?, severity?, duration?, source?, notes?)",
                "skill(character_id, name, category?, rank?, governing_stat?, delta_xp?|set_xp?)",
                "technique(character_id, name, tier?, mastery?, cost?, effect?, progression_track_id?, source_text_id?)",
                "goal(character_id, goal)"
            ]));
            groups.insert("3_inventory_apparel_and_outfits", json!([
                "equipment(sub_action=acquire|tailor|remove|equip|unequip|tag|auto_tag, character_id, name, slot?, quantity?, equipped?, durability?, tags?, weight_kg?, base_value?, currency?, garment_measurements?, garment_cut?, garment_taper?, description?)",
                "transfer_item(from_character_id, to_character_id, item_name, quantity?, reason?)",
                "pc_sheet(sub_action=upsert_outfit|set_active_outfit|remove_outfit, character_id, name/outfit_id, items?, sync_equipment?)"
            ]));
            groups.insert("4_body_avatar_and_pc_sheet", json!([
                "pc_sheet(sub_action=get_sheet|update_avatar|upsert_marker|remove_marker|upsert_title|toggle_title|adjust_stat_xp|upsert_job|adjust_job_xp|upsert_proficiency|upsert_trait, character_id, ...)",
                "profile(character_id, temperament?, values?, desires?, fears?, speech_style?, daily_routine?, long_term_agenda?)"
            ]));
            groups.insert("5_health_crafting_and_routines", json!([
                "health(sub_action=apply_injury|treat_injury|advance_recovery|list_injuries|resolve_downtime|begin_crafting|resolve_crafting, character_id, ...)",
                "routine(sub_action=list|upsert|delete, character_id, routine?)"
            ]));
        }
        "economy" => {
            groups.insert("1_trade_and_currency", json!([
                "execute_transaction(kind, buyer_id, seller_id, amount, currency, item_name?, quantity?, weight_kg?, base_value?, slot?, tags?, garment_measurements?, equip_on_buy?, catalog_item?, tax_rate_pct?, notes?)  <-- ONE-CALL atomic buy + deliver + weight/economics + catalog + encumbrance!",
                "get_balances(owner_id)",
                "set_balance(owner_id, currency, amount)",
                "transfer_currency(from_id, to_id, amount, currency, reason?)",
                "check_encumbrance(character_id)"
            ]));
            groups.insert("2_items_catalog_and_loot", json!([
                "upsert_item(name, item_type, current_holder_id?, weight_kg?, base_value?, currency?, slot?, tags?, garment_measurements?, origin_and_maker?, significance?, properties_and_effects?, condition_or_durability?)",
                "transfer_catalog_item(item_id, to_holder_id?, equipped?, condition_or_durability?, notes?)",
                "tag_item(item_id, tags?|add_tags?|remove_tags?|set_tags?)",
                "set_item_economics(item_name, weight_kg, unit_value?, currency?, volume_liters?)",
                "claim_discovery(discovery_id, claimant_id, item_indices?, status?)"
            ]));
            groups.insert("3_markets", json!([
                "upsert_market(place_id, name?, base_currency?, prosperity?, commodities)",
                "get_market_prices(place_id)",
                "update_market_supply(place_id, commodity, supply_delta?, demand_delta?, reason?)"
            ]));
        }
        "world" => {
            groups.insert("1_time_and_simulation", json!([
                "advance_time(minutes, global_weather?, reason?)",
                "advance_world(duration_hours, place_ids?, faction_ids?, character_ids?, tick_weather?, tick_recovery?, tick_markets?, tick_upkeep?, tick_npc_movements?)",
                "weather(sub_action=get|generate|advance, place_id, seed?)",
                "clock(sub_action=create|tick|update|list, name/clock_id, segments?, delta?, cadence_hours?, consequence_on_fill?, auto_reset?)"
            ]));
            groups.insert("2_places_and_control", json!([
                "create_place(name, place_type, terrain, region?, climate?, description?, controlling_faction?, danger_level?, grid_id?, x?, y?, fog_status?)",
                "get_place(place_id, as_of_seq?)",
                "update_place(sub_action=weather|add_condition|remove_condition|control|landmark|route, place_id, ...)"
            ]));
            groups.insert("3_routes_exploration_and_2d_grids", json!([
                "travel(sub_action=upsert_route|list_routes|calculate|advance|explore|update_spatial, ...)",
                "travel(sub_action=upsert_grid|list_grids|get_grid|set_cell|clear_cell|auto_layout_grid, grid_id, x?, y?, terrain?, label?, place_id?, fog_status?, cells?)"
            ]));
        }
        "society" => {
            groups.insert("1_relations_and_factions", json!([
                "relation(source_id, relation_type, target_id, strength?, dimensions?, direction?, visibility?, obligations?, notes?)",
                "remove_relation(source_id, relation_type, target_id, reason?)",
                "faction(sub_action=upsert|get|list|modify_resource|add_member, faction_id/name, ...)"
            ]));
            groups.insert("2_epistemic_knowledge", json!([
                "record_knowledge(subject, claim, truth_status?, visibility?, known_by?, disbelieved_by?, beliefs?, notes?)",
                "propagate_knowledge(claim_id, from_actor_id, to_actor_ids, stance?, distorted_claim?, notes?)"
            ]));
            groups.insert("3_law_reputation_and_npc_autonomy", json!([
                "law(sub_action=upsert_jurisdiction|check_jurisdiction|record_crime|update_warrant|modify_reputation|get_reputation, ...)",
                "autonomy(sub_action=prepare|commit|auto_mechanical|configure_mobility|dispatch_trip|tick_movements|list_movements, ...)"
            ]));
        }
        "codex" => {
            groups.insert("1_lore_and_texts", json!([
                "upsert_lore(title, body, id?, category?, summary?, visibility?, always_include?, keywords?, tags?, related_ids?)",
                "upsert_text(title, content, id?, medium?, subject?, author?, language?, condition?, read_status?, holder_or_location_id?, summary?, mechanics_or_unlocks?, gm_notes?, keywords?, tags?)",
                "record_text_reading(text_id, reader_id, read_status?, holder_or_location_id?, condition?, notes?)"
            ]));
            groups.insert("2_histories_docs_and_events", json!([
                "add_history(entity_id, entity_kind, title, details, era_or_date?, significance?, sort_order?, visibility?)",
                "import_attachment(title, source_path|text_content|base64_content, category?, summary?, visibility?)",
                "create_reference_doc(title, content, id?, append?, category?, summary?, visibility?)",
                "log_event(title, details, actor_id?, location_id?, affected_entities?, tags?, visibility?)"
            ]));
        }
        "progression" => {
            groups.insert("1_tracks_and_ladders", json!([
                "get_tracks(character_id)",
                "define_ladder(name, preset?, stages?, description?)",
                "list_ladders()",
                "enroll_track(character_id, name, ladder_id?, track_kind?, source_text_id?, manual_title?, manual_notes?, stage_rules?)"
            ]));
            groups.insert("2_xp_breakthroughs_and_rules", json!([
                "advance_xp(character_id, track_id, xp, reason?)",
                "adjust_xp|revert_xp|reset_xp(character_id, track_id, delta_xp?|set_current_xp?, reason?)",
                "attempt_breakthrough(character_id, track_id, stat?, skill?, dc_tier?, difficulty?, modifier?, stakes?)",
                "update_track(character_id, track_id, status?, bottleneck_reason?, halt_conditions?, stage_rules?, manual_notes?, gm_notes?)",
                "apply_stage_rule(character_id, track_id, major_stage_index?, sub_stage?, rule?)",
                "remove_track(character_id, track_id, reason?)"
            ]));
        }
        "resolve" => {
            groups.insert("1_resolution_and_encounters", json!([
                "check(actor_id, action, stat?, skill?, technique?, progression_track?, equipment?, dc_tier?, dc_factors?, difficulty?, modifier?, advantage?, stakes?, dry_run?)",
                "contest(action, initiator, defender, stakes?, tie_breaker?)",
                "draw_table(table_id, count?, stage_discovery?, seed?)",
                "roll_dice(expression, actor_id?, purpose?, seed?)",
                "encounter(sub_action=start|log_action|next_turn|update_participant|end|get|list, ...)",
                "ruleset(id?, name?, preset?, stat_modifier_mode?, auto_skill_xp?, list?)",
                "random_table(id?, name?, entries?, list?)"
            ]));
        }
        "admin" => {
            groups.insert("1_meta_and_maintenance", json!([
                "update_meta(name?, description?, ruleset_notes?, append_ruleset_notes?, global_weather?, reason?)",
                "diagnose()",
                "cleanup(dry_run?)",
                "backup(label?)",
                "restore_backup(backup_filename)",
                "rollback(to_seq)",
                "compile_docs()"
            ]));
            groups.insert("2_entity_refactoring", json!([
                "reclassify(entity_id, from_register, to_register, ...)",
                "rename_or_merge(register, old_id, new_id, ...)",
                "delete_entity(register, entity_id, reason?)",
                "update_character_baseline(character_id, ...)",
                "update_place_baseline(place_id, ...)",
                "update_entity_history(history_id, ...)",
                "update_attachment(attachment_id, ...)"
            ]));
        }
        _ => {
            groups.insert("overview", json!([
                "Pass target_op='<op_name>' (or call query(op='describe_op', tool='<tool_name>')) for the full exact JSON schema and accepted keys."
            ]));
        }
    }

    json!({
        "domain": domain,
        "tip": "Call again with target_op='<op>' (e.g., op='help', target_op='equipment') for the full parameter schema and types.",
        "operation_groups": groups,
        "automatic_cascades": cascades_for_domain(domain)
    })
}

fn cascades_for_domain(domain: &str) -> Vec<&'static str> {
    match domain.trim().to_ascii_lowercase().as_str() {
        "character" => vec![
            "equipment(acquire|tailor): auto-syncs item_economics (weight_kg/base_value), matching CatalogItem sizing/tags/equipped, garment fit report, and live encumbrance.",
            "equipment(remove to 0): auto-detaches matching CatalogItem holder, prunes dead item from pc_sheet outfits, and refreshes encumbrance.",
            "transfer_item: auto-transfers matching CatalogItem (and linked StoryText holder), prunes depleted outfit slots, and checks encumbrance.",
            "pc_sheet(update_avatar): auto-re-evaluates ALL equipped garments against updated measurements and returns garment fit alerts.",
            "pc_sheet(set_active_outfit): auto-equips items in the active outfit & unequips replaced outfit pieces (sync_equipment=true default), then evaluates fit.",
            "move: for PCs, auto-charts the destination's 2D map grid cell fog_status; clears stale active NPC mobility trips.",
            "status(dead|incapacitated): auto-disables active standing routines and cancels in-transit NPC mobility trips."
        ],
        "economy" => vec![
            "execute_transaction(with item_name): atomically moves currency (with auto-change), delivers item with full structured fields (weight_kg, slot, tags, garment_*), upserts item_economics, optionally syncs CatalogItem, and returns encumbrance + fit_report.",
            "upsert_item(with weight_kg/base_value): auto-syncs item_economics alongside character inventory."
        ],
        "world" => vec![
            "create_place(with grid_id+x+y): auto-registers the place on the 2D map grid cell.",
            "update_place(control): auto-syncs FactionRecord.controlled_places and the social graph 'controls' relation (removing old faction's stale link).",
            "update_place(route): auto-syncs matching TravelRoute status in the Dijkstra travel graph and warns if active NPC trips are blocked."
        ],
        _ => vec![
            "All mutations run strict unknown-key validation and return '_cascades' whenever 1st/2nd-order relatives are automatically synchronized."
        ],
    }
}
