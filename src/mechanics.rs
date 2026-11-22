use crate::dice::{roll_dice, DiceRollResult};
use crate::models::{slugify, CharacterProjection, PlaceProjection};
use anyhow::{anyhow, bail, Result};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

// ============================================================================
// 1. Declarative, System-Agnostic Ruleset Profiles & Check Resolution
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesetProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// One of: "roll_over" (default), "roll_under", "dice_pool", "step_dice"
    #[serde(default = "default_resolution_mode")]
    pub resolution_mode: String,
    /// Base dice notation, e.g. "1d20", "2d6", "1d100", "3d6", "1d6" (for dice_pool).
    #[serde(default = "default_base_dice")]
    pub base_dice: String,
    #[serde(default)]
    pub advantage_dice: Option<String>,
    #[serde(default)]
    pub disadvantage_dice: Option<String>,
    /// Natural die roll that triggers critical success (e.g., 20 on d20, 1 on d100 roll_under).
    #[serde(default)]
    pub crit_success_natural: Option<i64>,
    /// Natural die roll that triggers critical failure (e.g., 1 on d20, 100 on d100 roll_under).
    #[serde(default)]
    pub crit_failure_natural: Option<i64>,
    /// Beat/miss difficulty by at least this margin to escalate to critical success/failure.
    #[serde(default)]
    pub crit_margin: Option<i64>,
    /// If failing within `1..=partial_band_margin` of difficulty, yields "partial_success".
    #[serde(default)]
    pub partial_band_margin: Option<i64>,
    /// Optional PbtA-style fixed upper band for full success (e.g., 10 on 2d6).
    #[serde(default)]
    pub pbta_full_success_at: Option<i64>,
    /// Optional PbtA-style fixed lower band for partial success (e.g., 7 on 2d6).
    #[serde(default)]
    pub pbta_partial_success_at: Option<i64>,
    /// For "dice_pool" mode: minimum die face that counts as a hit (default 5 on d6).
    #[serde(default)]
    pub pool_success_threshold: Option<i64>,
    /// Optional mapping of skill ranks ("Novice", "Adept", "Master") -> numeric bonus when skill.modifier is absent.
    #[serde(default = "default_rank_bonuses")]
    pub rank_bonuses: BTreeMap<String, i64>,
    /// Optional mapping of condition severity/kind ("minor", "moderate", "severe", "critical", "buff") -> numeric modifier.
    #[serde(default = "default_condition_penalties")]
    pub condition_penalties: BTreeMap<String, i64>,
    /// Whether resolve_check auto-hydrates character stats/skills/conditions/items & place weather by default.
    #[serde(default = "default_true")]
    pub auto_hydrate: bool,
    #[serde(default = "default_version")]
    pub version: u32,
}

fn default_resolution_mode() -> String {
    "roll_over".to_string()
}

fn default_base_dice() -> String {
    "1d20".to_string()
}

fn default_true() -> bool {
    true
}

fn default_version() -> u32 {
    1
}

pub fn default_rank_bonuses() -> BTreeMap<String, i64> {
    let mut m = BTreeMap::new();
    m.insert("untrained".to_string(), -2);
    m.insert("novice".to_string(), 1);
    m.insert("apprentice".to_string(), 2);
    m.insert("journeyman".to_string(), 3);
    m.insert("adept".to_string(), 4);
    m.insert("expert".to_string(), 5);
    m.insert("master".to_string(), 6);
    m.insert("grandmaster".to_string(), 8);
    m.insert("legendary".to_string(), 10);
    m
}

pub fn default_condition_penalties() -> BTreeMap<String, i64> {
    let mut m = BTreeMap::new();
    m.insert("minor".to_string(), -1);
    m.insert("light".to_string(), -1);
    m.insert("moderate".to_string(), -2);
    m.insert("major".to_string(), -3);
    m.insert("severe".to_string(), -4);
    m.insert("critical".to_string(), -6);
    m.insert("buff".to_string(), 2);
    m
}

impl Default for RulesetProfile {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            name: "Default System-Agnostic Check (1d20 Roll-Over)".to_string(),
            description: "Built-in fallback profile: 1d20 + modifiers vs difficulty; natural 20/1 and +/-10 margin crits.".to_string(),
            resolution_mode: "roll_over".to_string(),
            base_dice: "1d20".to_string(),
            advantage_dice: Some("2d20kh1".to_string()),
            disadvantage_dice: Some("2d20kl1".to_string()),
            crit_success_natural: Some(20),
            crit_failure_natural: Some(1),
            crit_margin: Some(10),
            partial_band_margin: None,
            pbta_full_success_at: None,
            pbta_partial_success_at: None,
            pool_success_threshold: None,
            rank_bonuses: default_rank_bonuses(),
            condition_penalties: default_condition_penalties(),
            auto_hydrate: true,
            version: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModifierSource {
    pub source: String,
    pub value: i64,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckRequest {
    #[serde(default)]
    pub actor_id: Option<String>,
    pub action: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub stat: Option<String>,
    #[serde(default)]
    pub skill: Option<String>,
    #[serde(default)]
    pub technique: Option<String>,
    #[serde(default)]
    pub equipment: Vec<String>,
    pub difficulty: i64,
    #[serde(default)]
    pub modifier: i64,
    #[serde(default)]
    pub situational_modifiers: Vec<ModifierSource>,
    /// "none" (default), "advantage", or "disadvantage"
    #[serde(default = "default_advantage")]
    pub advantage: String,
    /// Optional override for profile's base_dice (e.g. step-dice "1d10" or custom pool "5d6")
    #[serde(default)]
    pub dice_override: Option<String>,
    #[serde(default)]
    pub auto_hydrate: Option<bool>,
    #[serde(default)]
    pub stakes: String,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub strict_validation: bool,
    #[serde(default = "default_true")]
    pub log_to_ledger: bool,
    #[serde(default)]
    pub causal_event_ids: Vec<i64>,
    #[serde(default = "default_visibility")]
    pub visibility: String,
}

fn default_advantage() -> String {
    "none".to_string()
}

fn default_visibility() -> String {
    "public".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OddsPreview {
    pub success_pct: f64,
    pub partial_pct: f64,
    pub crit_success_pct: f64,
    pub expected_total: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckResolution {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    pub action: String,
    pub profile_id: String,
    pub resolution_mode: String,
    pub expression: String,
    pub advantage: String,
    pub seed: u64,
    pub rolls: Vec<i64>,
    pub kept: Vec<i64>,
    pub roll: i64,
    pub modifiers: Vec<ModifierSource>,
    pub modifier: i64,
    pub total: i64,
    pub difficulty: i64,
    pub margin: i64,
    pub outcome: String,
    pub degree: String,
    pub stakes: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
    pub consequences: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    pub dry_run: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub odds_preview: Option<OddsPreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContestParticipantInput {
    pub actor_id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub stat: Option<String>,
    #[serde(default)]
    pub skill: Option<String>,
    #[serde(default)]
    pub technique: Option<String>,
    #[serde(default)]
    pub equipment: Vec<String>,
    #[serde(default)]
    pub modifier: i64,
    #[serde(default)]
    pub situational_modifiers: Vec<ModifierSource>,
    #[serde(default = "default_advantage")]
    pub advantage: String,
    #[serde(default)]
    pub dice_override: Option<String>,
    #[serde(default)]
    pub auto_hydrate: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContestRequest {
    pub action: String,
    #[serde(default)]
    pub profile: Option<String>,
    pub participants: Vec<ContestParticipantInput>,
    /// "defender" | "higher_modifier" | "draw" (default: "higher_modifier")
    #[serde(default = "default_tie_breaker")]
    pub tie_breaker: String,
    #[serde(default)]
    pub stakes: String,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub strict_validation: bool,
    #[serde(default = "default_true")]
    pub log_to_ledger: bool,
    #[serde(default)]
    pub causal_event_ids: Vec<i64>,
    #[serde(default = "default_visibility")]
    pub visibility: String,
}

fn default_tie_breaker() -> String {
    "higher_modifier".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContestParticipantResult {
    pub actor_id: String,
    pub role: String,
    pub expression: String,
    pub advantage: String,
    pub seed: u64,
    pub rolls: Vec<i64>,
    pub kept: Vec<i64>,
    pub roll: i64,
    pub modifiers: Vec<ModifierSource>,
    pub modifier: i64,
    pub total: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContestResolution {
    pub action: String,
    pub profile_id: String,
    pub resolution_mode: String,
    pub seed: u64,
    pub participants: Vec<ContestParticipantResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winner_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winner_role: Option<String>,
    pub margin: i64,
    pub is_tie: bool,
    pub tie_breaker_used: String,
    pub stakes: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<i64>,
}

/// Extracts a signed integer modifier like `+2`, `-3`, `+1 to climbing` from free-text fields.
pub fn extract_signed_modifier(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' || bytes[i] == b'-' {
            let sign = if bytes[i] == b'-' { -1 } else { 1 };
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start {
                // Ensure not part of a dice expression like "+1d6"
                if end < bytes.len() && (bytes[end] == b'd' || bytes[end] == b'D') {
                    i = end;
                    continue;
                }
                if let Ok(n) = text[start..end].parse::<i64>() {
                    return Some(sign * n);
                }
            }
        }
        i += 1;
    }
    None
}

/// Result of auto-hydrating modifiers from live character/place state, including explicit
/// validation warnings for missing references or unmapped skill ranks.
#[derive(Debug, Clone, Default)]
pub struct HydrationOutcome {
    pub modifiers: Vec<ModifierSource>,
    pub warnings: Vec<String>,
    pub missing_refs: Vec<String>,
}

/// Computes auto-hydrated modifiers from a character's live projection and their current place!
pub fn collect_hydrated_modifiers(
    profile: &RulesetProfile,
    actor: Option<&CharacterProjection>,
    place: Option<&PlaceProjection>,
    stat: Option<&str>,
    skill: Option<&str>,
    technique: Option<&str>,
    equipment: &[String],
    action_text: &str,
) -> HydrationOutcome {
    let mut mods = Vec::new();
    let mut warnings = Vec::new();
    let mut missing_refs = Vec::new();
    let action_lower = action_text.to_lowercase();

    if actor.is_none() {
        let mut unhydrated = Vec::new();
        if let Some(s) = stat.filter(|s| !s.trim().is_empty()) {
            unhydrated.push(format!("stat '{s}'"));
        }
        if let Some(s) = skill.filter(|s| !s.trim().is_empty()) {
            unhydrated.push(format!("skill '{s}'"));
        }
        if let Some(s) = technique.filter(|s| !s.trim().is_empty()) {
            unhydrated.push(format!("technique '{s}'"));
        }
        if !equipment.is_empty() {
            unhydrated.push(format!("equipment {:?}", equipment));
        }
        if !unhydrated.is_empty() {
            let msg = format!(
                "No actor_id provided; could not auto-hydrate {} (treated as +0).",
                unhydrated.join(", ")
            );
            warnings.push(msg.clone());
            missing_refs.push(msg);
        }
    }

    if let Some(ch) = actor {
        // 1. Stat lookup (case-insensitive)
        if let Some(stat_name) = stat.filter(|s| !s.trim().is_empty()) {
            let target = stat_name.trim().to_lowercase();
            if let Some((k, v)) = ch
                .current
                .stats
                .iter()
                .find(|(k, _)| k.to_lowercase() == target)
            {
                mods.push(ModifierSource {
                    source: format!("stat:{k}"),
                    value: *v,
                    category: "stat".to_string(),
                    notes: format!("Character stat '{k}' = {v}"),
                });
            } else {
                let msg = format!(
                    "Unknown stat '{}' on character '{}' (not found; treated as +0).",
                    stat_name.trim(),
                    ch.baseline.name
                );
                warnings.push(msg.clone());
                missing_refs.push(msg);
            }
        }

        // 2. Skill lookup (exact or substring match)
        if let Some(skill_name) = skill.filter(|s| !s.trim().is_empty()) {
            let target = skill_name.trim().to_lowercase();
            let found = ch
                .current
                .skills
                .iter()
                .find(|s| s.name.to_lowercase() == target)
                .or_else(|| {
                    ch.current.skills.iter().find(|s| {
                        let sn = s.name.to_lowercase();
                        sn.contains(&target) || target.contains(&sn)
                    })
                });
            if let Some(sk) = found {
                let explicit_mod = sk.modifier.as_deref().and_then(extract_signed_modifier);
                let rank_mod = profile
                    .rank_bonuses
                    .get(&sk.rank.trim().to_lowercase())
                    .copied();
                let val = explicit_mod.or(rank_mod).unwrap_or(0);
                if explicit_mod.is_none() && rank_mod.is_none() && !sk.rank.trim().is_empty() {
                    warnings.push(format!(
                        "Skill '{}' on '{}' has rank '{}' with no explicit numeric modifier and no entry in ruleset profile '{}' rank_bonuses (treated as +0). Configure rank_bonuses via upsert_ruleset_profile or set skill.modifier.",
                        sk.name, ch.baseline.name, sk.rank, profile.id
                    ));
                }
                mods.push(ModifierSource {
                    source: format!("skill:{}", sk.name),
                    value: val,
                    category: "skill".to_string(),
                    notes: format!(
                        "Rank: {}{}",
                        sk.rank,
                        sk.modifier
                            .as_deref()
                            .map(|m| format!(" ({m})"))
                            .unwrap_or_default()
                    ),
                });
            } else {
                let msg = format!(
                    "Unknown skill '{}' on character '{}' (not found; treated as +0).",
                    skill_name.trim(),
                    ch.baseline.name
                );
                warnings.push(msg.clone());
                missing_refs.push(msg);
            }
        }

        // 3. Technique lookup
        if let Some(tech_name) = technique.filter(|s| !s.trim().is_empty()) {
            let target = tech_name.trim().to_lowercase();
            let found = ch
                .current
                .techniques
                .iter()
                .find(|t| t.name.to_lowercase() == target)
                .or_else(|| {
                    ch.current.techniques.iter().find(|t| {
                        let tn = t.name.to_lowercase();
                        tn.contains(&target) || target.contains(&tn)
                    })
                });
            if let Some(tech) = found {
                let val = extract_signed_modifier(&tech.effect)
                    .or_else(|| extract_signed_modifier(&tech.description))
                    .unwrap_or(0);
                mods.push(ModifierSource {
                    source: format!("technique:{}", tech.name),
                    value: val,
                    category: "technique".to_string(),
                    notes: if tech.effect.is_empty() {
                        format!("Mastery: {}", tech.mastery)
                    } else {
                        tech.effect.clone()
                    },
                });
            } else {
                let msg = format!(
                    "Unknown technique '{}' on character '{}' (not found; treated as +0).",
                    tech_name.trim(),
                    ch.baseline.name
                );
                warnings.push(msg.clone());
                missing_refs.push(msg);
            }
        }

        // 4. Equipment lookup (explicit list OR equipped items with relevant modifiers)
        let explicit_eq: Vec<String> = equipment
            .iter()
            .map(|e| e.trim().to_lowercase())
            .filter(|e| !e.is_empty())
            .collect();
        let mut matched_explicit: Vec<bool> = vec![false; explicit_eq.len()];
        for item in &ch.current.equipment {
            let item_lower = item.name.to_lowercase();
            let mut explicitly_requested = false;
            for (idx, e) in explicit_eq.iter().enumerate() {
                if item_lower == *e || item_lower.contains(e) || e.contains(&item_lower) {
                    explicitly_requested = true;
                    matched_explicit[idx] = true;
                }
            }
            let tagged_mod = item
                .tags
                .iter()
                .find_map(|t| extract_signed_modifier(t))
                .or_else(|| extract_signed_modifier(&item.description));

            if explicitly_requested {
                let mut val = tagged_mod.unwrap_or(0);
                let mut notes = item.description.clone();
                if let Some(ref dur) = item.durability {
                    let dl = dur.to_lowercase();
                    if dl.contains("broken") || dl.contains("ruined") || dl.contains("shattered") {
                        val = val.min(0) - 2;
                        notes = format!("Item durability is '{dur}' (-2 penalty). {notes}");
                    } else if dl.contains("damaged") || dl.contains("frayed") || dl.contains("worn")
                    {
                        val -= 1;
                        notes = format!("Item durability is '{dur}' (-1). {notes}");
                    }
                }
                mods.push(ModifierSource {
                    source: format!("equipment:{}", item.name),
                    value: val,
                    category: "equipment".to_string(),
                    notes: notes.trim().to_string(),
                });
            } else if item.equipped {
                if let Some(val) = tagged_mod {
                    // Only auto-apply equipped item bonuses if item name/tags/desc match skill or action keywords
                    let skill_lower = skill.unwrap_or("").to_lowercase();
                    let matches_context = (!skill_lower.is_empty()
                        && (item.description.to_lowercase().contains(&skill_lower)
                            || item
                                .tags
                                .iter()
                                .any(|t| t.to_lowercase().contains(&skill_lower))))
                        || item.tags.iter().any(|t| {
                            let tl = t.to_lowercase();
                            tl.len() >= 4 && action_lower.contains(&tl)
                        });
                    if matches_context && val != 0 {
                        mods.push(ModifierSource {
                            source: format!("equipment:{}", item.name),
                            value: val,
                            category: "equipment".to_string(),
                            notes: item.description.clone(),
                        });
                    }
                }
            }
        }
        for (idx, matched) in matched_explicit.into_iter().enumerate() {
            if !matched {
                let raw = equipment
                    .iter()
                    .filter(|e| !e.trim().is_empty())
                    .nth(idx)
                    .map(|s| s.trim())
                    .unwrap_or(explicit_eq[idx].as_str());
                let msg = format!(
                    "Unknown equipment item '{}' on character '{}' (not carried; treated as +0).",
                    raw, ch.baseline.name
                );
                warnings.push(msg.clone());
                missing_refs.push(msg);
            }
        }

        // 5. Character live conditions & injuries
        for cond in &ch.current.conditions {
            let kind_l = cond.kind.trim().to_lowercase();
            let sev_l = cond.severity.trim().to_lowercase();
            let explicit_val = extract_signed_modifier(&cond.notes)
                .or_else(|| extract_signed_modifier(&cond.severity));
            let val = if let Some(v) = explicit_val {
                v
            } else if kind_l == "buff" || kind_l == "boon" || kind_l == "blessing" {
                profile.condition_penalties.get("buff").copied().unwrap_or(1)
            } else if matches!(
                kind_l.as_str(),
                "injury" | "debuff" | "curse" | "fatigue" | "exhaustion" | "poison" | "wound"
            ) || !sev_l.is_empty()
            {
                profile
                    .condition_penalties
                    .get(&sev_l)
                    .copied()
                    .unwrap_or_else(|| {
                        if kind_l == "injury" || kind_l == "wound" || kind_l == "debuff" {
                            -2
                        } else {
                            0
                        }
                    })
            } else {
                0
            };
            if val != 0 {
                mods.push(ModifierSource {
                    source: format!("condition:{}", cond.name),
                    value: val,
                    category: kind_l,
                    notes: format!(
                        "Severity: {}{}",
                        if cond.severity.is_empty() {
                            "unspecified"
                        } else {
                            cond.severity.as_str()
                        },
                        if cond.notes.is_empty() {
                            String::new()
                        } else {
                            format!(" — {}", cond.notes)
                        }
                    ),
                });
            }
        }
    }

    // 6. Live location weather & environmental conditions
    if let Some(pl) = place {
        let weather_l = pl.current.current_weather.to_lowercase();
        if let Some(v) = extract_signed_modifier(&pl.current.current_weather) {
            if v != 0 {
                mods.push(ModifierSource {
                    source: format!("weather:{}", pl.current.current_weather),
                    value: v,
                    category: "environment".to_string(),
                    notes: format!("Active weather at {}", pl.baseline.name),
                });
            }
        } else if !weather_l.is_empty() {
            let env_pen = if weather_l.contains("blizzard")
                || weather_l.contains("typhoon")
                || weather_l.contains("tempest")
                || weather_l.contains("hurricane")
            {
                -3
            } else if weather_l.contains("storm")
                || weather_l.contains("heavy rain")
                || weather_l.contains("downpour")
                || weather_l.contains("icy")
                || weather_l.contains("ice")
                || weather_l.contains("gale")
                || weather_l.contains("thick fog")
            {
                -2
            } else if weather_l.contains("rain")
                || weather_l.contains("wet")
                || weather_l.contains("slick")
                || weather_l.contains("snow")
                || weather_l.contains("fog")
                || weather_l.contains("mist")
                || weather_l.contains("wind")
            {
                -1
            } else {
                0
            };
            if env_pen != 0 {
                mods.push(ModifierSource {
                    source: format!("weather:{}", pl.current.current_weather),
                    value: env_pen,
                    category: "environment".to_string(),
                    notes: format!("Live weather at {}", pl.baseline.name),
                });
            }
        }

        for pcond in &pl.current.active_conditions {
            if let Some(v) = extract_signed_modifier(pcond) {
                if v != 0 {
                    mods.push(ModifierSource {
                        source: format!("place_condition:{pcond}"),
                        value: v,
                        category: "environment".to_string(),
                        notes: format!("Active condition at {}", pl.baseline.name),
                    });
                }
            } else {
                let pcl = pcond.to_lowercase();
                if pcl.contains("slippery")
                    || pcl.contains("loose")
                    || pcl.contains("flooded")
                    || pcl.contains("dark")
                    || pcl.contains("crumbling")
                    || pcl.contains("muddy")
                    || pcl.contains("toxic")
                    || pcl.contains("miasma")
                {
                    mods.push(ModifierSource {
                        source: format!("place_condition:{pcond}"),
                        value: -1,
                        category: "environment".to_string(),
                        notes: format!("Hazardous terrain/condition at {}", pl.baseline.name),
                    });
                }
            }
        }
    }

    HydrationOutcome {
        modifiers: mods,
        warnings,
        missing_refs,
    }
}

/// Derives the dice expression to roll for a given profile, advantage mode, and optional dice override.
pub fn resolve_dice_expression(
    profile: &RulesetProfile,
    advantage: &str,
    dice_override: Option<&str>,
    total_modifier: i64,
) -> Result<String> {
    let adv = advantage.trim().to_lowercase();
    let mode = profile.resolution_mode.trim().to_lowercase();

    if mode == "dice_pool" {
        let base = dice_override.unwrap_or(&profile.base_dice);
        let parsed = parse_simple_dice(base)?;
        let adv_delta: i64 = match adv.as_str() {
            "advantage" | "adv" => 1,
            "disadvantage" | "dis" | "disadv" => -1,
            _ => 0,
        };
        let pool_size = (parsed.0 as i64 + total_modifier + adv_delta).max(1) as u32;
        return Ok(format!("{pool_size}d{}", parsed.1));
    }

    if let Some(ov) = dice_override {
        return Ok(apply_advantage_to_expr(ov, &adv));
    }

    match adv.as_str() {
        "advantage" | "adv" => {
            if let Some(ref expr) = profile.advantage_dice {
                return Ok(expr.clone());
            }
            Ok(apply_advantage_to_expr(&profile.base_dice, "advantage"))
        }
        "disadvantage" | "dis" | "disadv" => {
            if let Some(ref expr) = profile.disadvantage_dice {
                return Ok(expr.clone());
            }
            Ok(apply_advantage_to_expr(&profile.base_dice, "disadvantage"))
        }
        _ => Ok(profile.base_dice.clone()),
    }
}

fn parse_simple_dice(expr: &str) -> Result<(u32, u32)> {
    let compact: String = expr
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let d_pos = compact
        .find('d')
        .ok_or_else(|| anyhow!("Invalid dice expression '{expr}'"))?;
    let count: u32 = if d_pos == 0 {
        1
    } else {
        compact[..d_pos].parse()?
    };
    let rest = &compact[d_pos + 1..];
    let sides_end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let sides: u32 = rest[..sides_end].parse()?;
    Ok((count, sides))
}

fn apply_advantage_to_expr(base: &str, adv: &str) -> String {
    if adv != "advantage" && adv != "adv" && adv != "disadvantage" && adv != "dis" && adv != "disadv" {
        return base.to_string();
    }
    if let Ok((count, sides)) = parse_simple_dice(base) {
        let keep_op = if adv.starts_with("adv") { "kh" } else { "kl" };
        return format!("{}d{}{}{}", count + 1, sides, keep_op, count);
    }
    base.to_string()
}

/// Evaluates a single check against a RulesetProfile (pure deterministic function).
pub fn evaluate_check(
    profile: &RulesetProfile,
    req: &CheckRequest,
    hydrated_modifiers: Vec<ModifierSource>,
    warnings: Vec<String>,
    location_id: Option<String>,
) -> Result<CheckResolution> {
    let mut all_mods = hydrated_modifiers;
    if req.modifier != 0 {
        all_mods.push(ModifierSource {
            source: "manual_modifier".to_string(),
            value: req.modifier,
            category: "situational".to_string(),
            notes: "Explicit check modifier".to_string(),
        });
    }
    all_mods.extend(req.situational_modifiers.clone());

    let total_modifier: i64 = all_mods.iter().map(|m| m.value).sum();
    let mode = profile.resolution_mode.trim().to_lowercase();
    let expression = resolve_dice_expression(
        profile,
        &req.advantage,
        req.dice_override.as_deref(),
        total_modifier,
    )?;

    let seed = req.seed.unwrap_or_else(|| {
        let mut rng = StdRng::from_entropy();
        rng.gen::<u64>()
    });

    let odds_preview = if req.dry_run {
        Some(estimate_odds(profile, &expression, total_modifier, req.difficulty))
    } else {
        None
    };

    if req.dry_run {
        return Ok(CheckResolution {
            actor_id: req.actor_id.clone(),
            action: req.action.clone(),
            profile_id: profile.id.clone(),
            resolution_mode: mode,
            expression,
            advantage: req.advantage.clone(),
            seed,
            rolls: vec![],
            kept: vec![],
            roll: 0,
            modifiers: all_mods,
            modifier: total_modifier,
            total: total_modifier,
            difficulty: req.difficulty,
            margin: 0,
            outcome: "preview".to_string(),
            degree: "Dry-run preview (not rolled)".to_string(),
            stakes: req.stakes.clone(),
            location_id,
            consequences: vec![],
            warnings,
            dry_run: true,
            odds_preview,
            event_id: None,
        });
    }

    let dice_res = roll_dice(&expression, Some(seed))?;
    let (roll_val, total, margin, outcome, degree) =
        classify_roll(profile, &dice_res, total_modifier, req.difficulty);

    Ok(CheckResolution {
        actor_id: req.actor_id.clone(),
        action: req.action.clone(),
        profile_id: profile.id.clone(),
        resolution_mode: mode,
        expression,
        advantage: req.advantage.clone(),
        seed,
        rolls: dice_res.rolls,
        kept: dice_res.kept,
        roll: roll_val,
        modifiers: all_mods,
        modifier: total_modifier,
        total,
        difficulty: req.difficulty,
        margin,
        outcome,
        degree,
        stakes: req.stakes.clone(),
        location_id,
        consequences: vec![],
        warnings,
        dry_run: false,
        odds_preview: None,
        event_id: None,
    })
}

fn classify_roll(
    profile: &RulesetProfile,
    dice: &DiceRollResult,
    total_modifier: i64,
    difficulty: i64,
) -> (i64, i64, i64, String, String) {
    let mode = profile.resolution_mode.trim().to_lowercase();
    let natural_single = if dice.kept.len() == 1 {
        Some(dice.kept[0])
    } else {
        None
    };

    match mode.as_str() {
        "roll_under" => {
            // Target = difficulty + modifier (or skill target passed as difficulty + situational modifier)
            let effective_target = difficulty + total_modifier;
            let roll_val = dice.total;
            let margin = effective_target - roll_val; // positive = succeeded by margin
            let nat_crit = profile
                .crit_success_natural
                .is_some_and(|c| natural_single == Some(c) || roll_val <= c);
            let nat_fumble = profile
                .crit_failure_natural
                .is_some_and(|c| natural_single == Some(c) || roll_val >= c);

            let (outcome, degree) = if nat_crit {
                ("critical_success", format!("Critical Success (rolled {roll_val} <= {effective_target})"))
            } else if nat_fumble {
                ("critical_failure", format!("Critical Failure (rolled {roll_val} vs {effective_target})"))
            } else if margin >= 0 {
                if profile.crit_margin.is_some_and(|cm| margin >= cm) {
                    ("critical_success", format!("Critical Success (margin +{margin})"))
                } else {
                    ("success", format!("Success (margin +{margin})"))
                }
            } else if profile.partial_band_margin.is_some_and(|pm| (-margin) <= pm) {
                ("partial_success", format!("Partial Success / Cost (margin {margin})"))
            } else if profile.crit_margin.is_some_and(|cm| (-margin) >= cm) {
                ("critical_failure", format!("Critical Failure (margin {margin})"))
            } else {
                ("failure", format!("Failure (margin {margin})"))
            };
            (roll_val, roll_val, margin, outcome.to_string(), degree)
        }
        "dice_pool" => {
            let threshold = profile.pool_success_threshold.unwrap_or(5);
            let hits = dice.kept.iter().filter(|&&v| v >= threshold).count() as i64;
            let ones = dice.kept.iter().filter(|&&v| v == 1).count() as i64;
            let margin = hits - difficulty;
            let (outcome, degree) = if hits == 0 && ones >= (dice.kept.len() as i64 + 1) / 2 {
                ("critical_failure", format!("Critical Failure (0 hits, {ones} ones)"))
            } else if margin >= 0 {
                if profile.crit_margin.is_some_and(|cm| margin >= cm) || (hits >= difficulty + 2) {
                    ("critical_success", format!("Critical Success ({hits} hits vs {difficulty})"))
                } else {
                    ("success", format!("Success ({hits} hits vs {difficulty})"))
                }
            } else if profile.partial_band_margin.is_some_and(|pm| (-margin) <= pm) && hits > 0 {
                ("partial_success", format!("Partial Success ({hits} hits vs {difficulty})"))
            } else {
                ("failure", format!("Failure ({hits} hits vs {difficulty})"))
            };
            (hits, hits, margin, outcome.to_string(), degree)
        }
        "step_dice" => {
            // Take highest rolled die + modifier vs difficulty (e.g. Savage Worlds / Cortex)
            let highest = dice.kept.iter().copied().max().unwrap_or(0);
            let total = highest + total_modifier;
            let margin = total - difficulty;
            let (outcome, degree) = if highest == 1 && dice.kept.iter().all(|&v| v == 1) {
                ("critical_failure", "Critical Failure (all 1s)".to_string())
            } else if margin >= 0 {
                if profile.crit_margin.is_some_and(|cm| margin >= cm) {
                    ("critical_success", format!("Raise / Critical Success (+{margin})"))
                } else {
                    ("success", format!("Success (+{margin})"))
                }
            } else if profile.partial_band_margin.is_some_and(|pm| (-margin) <= pm) {
                ("partial_success", format!("Partial Success ({margin})"))
            } else {
                ("failure", format!("Failure ({margin})"))
            };
            (highest, total, margin, outcome.to_string(), degree)
        }
        _ => {
            // "roll_over" (covers D20, 2d6+mod PbtA, 3d6+mod, etc.)
            let roll_val = dice.total;
            let total = roll_val + total_modifier;

            // PbtA fixed band support if configured
            if let (Some(full_at), Some(part_at)) =
                (profile.pbta_full_success_at, profile.pbta_partial_success_at)
            {
                let margin = total - full_at;
                let (outcome, degree) = if profile
                    .crit_margin
                    .is_some_and(|cm| total >= full_at + cm)
                {
                    ("critical_success", format!("Strong / Critical Hit ({total})"))
                } else if total >= full_at {
                    ("success", format!("Full Success ({total} >= {full_at})"))
                } else if total >= part_at {
                    (
                        "partial_success",
                        format!("Partial Success / Cost ({total}, band {part_at}..{})", full_at - 1),
                    )
                } else if profile
                    .crit_margin
                    .is_some_and(|cm| total <= part_at - cm)
                {
                    ("critical_failure", format!("Critical Miss ({total})"))
                } else {
                    ("failure", format!("Miss ({total} < {part_at})"))
                };
                return (roll_val, total, margin, outcome.to_string(), degree);
            }

            let margin = total - difficulty;
            let nat_crit = profile
                .crit_success_natural
                .is_some_and(|c| natural_single == Some(c));
            let nat_fumble = profile
                .crit_failure_natural
                .is_some_and(|c| natural_single == Some(c));

            let (outcome, degree) = if nat_crit {
                ("critical_success", format!("Critical Success (Natural {}, total {total})", natural_single.unwrap_or(roll_val)))
            } else if nat_fumble {
                ("critical_failure", format!("Critical Failure (Natural {}, total {total})", natural_single.unwrap_or(roll_val)))
            } else if margin >= 0 {
                if profile.crit_margin.is_some_and(|cm| margin >= cm) {
                    ("critical_success", format!("Critical Success (margin +{margin})"))
                } else {
                    ("success", format!("Success (margin +{margin})"))
                }
            } else if profile.partial_band_margin.is_some_and(|pm| (-margin) <= pm) {
                ("partial_success", format!("Partial Success / Cost (margin {margin})"))
            } else if profile.crit_margin.is_some_and(|cm| (-margin) >= cm) {
                ("critical_failure", format!("Critical Failure (margin {margin})"))
            } else {
                ("failure", format!("Failure (margin {margin})"))
            };
            (roll_val, total, margin, outcome.to_string(), degree)
        }
    }
}

/// Deterministic Monte Carlo odds estimator (2,000 fixed-seed trials) for `dry_run: true` previews.
fn estimate_odds(
    profile: &RulesetProfile,
    expression: &str,
    total_modifier: i64,
    difficulty: i64,
) -> OddsPreview {
    let trials = 1000u64;
    let mut succ = 0u64;
    let mut partial = 0u64;
    let mut crit = 0u64;
    let mut sum_total = 0i64;
    for i in 0..trials {
        if let Ok(d) = roll_dice(expression, Some(0xC4A0_1000 + i)) {
            let (_, total, _, out, _) = classify_roll(profile, &d, total_modifier, difficulty);
            sum_total += total;
            match out.as_str() {
                "critical_success" => {
                    succ += 1;
                    crit += 1;
                }
                "success" => succ += 1,
                "partial_success" => partial += 1,
                _ => {}
            }
        }
    }
    let f = trials as f64;
    OddsPreview {
        success_pct: ((succ as f64 / f) * 1000.0).round() / 10.0,
        partial_pct: ((partial as f64 / f) * 1000.0).round() / 10.0,
        crit_success_pct: ((crit as f64 / f) * 1000.0).round() / 10.0,
        expected_total: ((sum_total as f64 / f) * 10.0).round() / 10.0,
    }
}

// ============================================================================
// 2. Living Random Tables, Persistent Decks & Staged Loot Receipts
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TableEntryConditions {
    #[serde(default)]
    pub seasons: Vec<String>,
    #[serde(default)]
    pub weathers: Vec<String>,
    #[serde(default)]
    pub locations: Vec<String>,
    #[serde(default)]
    pub place_types: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub min_threat: Option<i64>,
    #[serde(default)]
    pub max_threat: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightMultiplier {
    /// Key in DrawContext: "season", "weather", "location", "place_type", "tag", "threat_level"
    pub key: String,
    pub value: String,
    pub multiplier: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LootItemTemplate {
    pub name: String,
    #[serde(default = "default_item_type")]
    pub item_type: String,
    #[serde(default)]
    pub rarity_or_tier: String,
    #[serde(default)]
    pub quantity_dice: Option<String>,
    #[serde(default)]
    pub condition_or_durability: String,
    #[serde(default)]
    pub properties_and_effects: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_item_type() -> String {
    "misc".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RandomTableEntry {
    #[serde(default)]
    pub id: String,
    pub result: String,
    #[serde(default)]
    pub weight: u32,
    #[serde(default)]
    pub range: Option<[i64; 2]>,
    #[serde(default = "default_rarity")]
    pub rarity: String,
    #[serde(default)]
    pub conditions: TableEntryConditions,
    #[serde(default)]
    pub weight_multipliers: Vec<WeightMultiplier>,
    #[serde(default)]
    pub subtable_id: Option<String>,
    #[serde(default)]
    pub loot_template: Option<LootItemTemplate>,
    #[serde(default)]
    pub notes: String,
}

fn default_rarity() -> String {
    "common".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RandomTable {
    pub id: String,
    pub name: String,
    #[serde(default = "default_table_category")]
    pub category: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub context: Vec<String>,
    #[serde(default)]
    pub dice: Option<String>,
    #[serde(default)]
    pub is_loot_table: bool,
    #[serde(default = "default_version")]
    pub version: u32,
    pub entries: Vec<RandomTableEntry>,
    #[serde(default)]
    pub created_at_world_time: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_table_category() -> String {
    "general".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DrawContext {
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub place_type: Option<String>,
    #[serde(default)]
    pub season: Option<String>,
    #[serde(default)]
    pub weather: Option<String>,
    #[serde(default)]
    pub party: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub threat_level: Option<i64>,
    #[serde(default)]
    pub source_description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawTableRequest {
    pub table_id: String,
    #[serde(default)]
    pub context: DrawContext,
    #[serde(default = "default_draw_count")]
    pub count: usize,
    #[serde(default = "default_true")]
    pub replacement: bool,
    /// Optional scope key for persistent without-replacement decks (defaults to location or "global")
    #[serde(default)]
    pub deck_scope: Option<String>,
    #[serde(default)]
    pub reset_deck: bool,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub stage_discovery: Option<bool>,
    #[serde(default = "default_true")]
    pub log_to_ledger: bool,
    #[serde(default)]
    pub causal_event_ids: Vec<i64>,
}

fn default_draw_count() -> usize {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableDrawItem {
    pub entry_id: String,
    pub result: String,
    pub rarity: String,
    pub roll_value: Option<i64>,
    pub effective_weight: u32,
    #[serde(default)]
    pub subtable_results: Vec<TableDrawItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loot_item: Option<StagedDiscoveryItem>,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedDiscoveryItem {
    pub index: usize,
    pub name: String,
    pub item_type: String,
    pub rarity_or_tier: String,
    pub quantity: i64,
    pub condition_or_durability: String,
    pub properties_and_effects: String,
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub claimed_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedDiscovery {
    pub id: String,
    pub source_table_id: String,
    pub table_version: u32,
    #[serde(default)]
    pub location_id: Option<String>,
    pub source_description: String,
    pub seed: u64,
    /// "pending" | "partially_claimed" | "claimed" | "expired"
    pub status: String,
    pub items: Vec<StagedDiscoveryItem>,
    pub created_at_world_time: String,
    #[serde(default)]
    pub draw_event_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawTableOutcome {
    pub table_id: String,
    pub table_name: String,
    pub table_version: u32,
    pub seed: u64,
    pub replacement: bool,
    pub deck_scope: String,
    pub eligible_entry_count: usize,
    pub draws: Vec<TableDrawItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staged_discovery: Option<StagedDiscovery>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<i64>,
}

/// Evaluates whether a table entry is eligible under `DrawContext`, and computes its effective weight.
pub fn compute_entry_weight(entry: &RandomTableEntry, ctx: &DrawContext) -> u32 {
    let c = &entry.conditions;
    if !c.seasons.is_empty() {
        let Some(ref s) = ctx.season else { return 0; };
        if !c.seasons.iter().any(|x| x.eq_ignore_ascii_case(s)) {
            return 0;
        }
    }
    if !c.weathers.is_empty() {
        let Some(ref w) = ctx.weather else { return 0; };
        let wl = w.to_lowercase();
        if !c.weathers.iter().any(|x| wl.contains(&x.to_lowercase())) {
            return 0;
        }
    }
    if !c.locations.is_empty() {
        let Some(ref loc) = ctx.location else { return 0; };
        if !c.locations.iter().any(|x| x.eq_ignore_ascii_case(loc)) {
            return 0;
        }
    }
    if !c.place_types.is_empty() {
        let Some(ref pt) = ctx.place_type else { return 0; };
        if !c.place_types.iter().any(|x| x.eq_ignore_ascii_case(pt)) {
            return 0;
        }
    }
    if !c.tags.is_empty() {
        if !c
            .tags
            .iter()
            .any(|req_tag| ctx.tags.iter().any(|t| t.eq_ignore_ascii_case(req_tag)))
        {
            return 0;
        }
    }
    if let Some(min_t) = c.min_threat {
        if ctx.threat_level.unwrap_or(0) < min_t {
            return 0;
        }
    }
    if let Some(max_t) = c.max_threat {
        if ctx.threat_level.unwrap_or(0) > max_t {
            return 0;
        }
    }

    let base_weight: f64 = if entry.weight > 0 {
        entry.weight as f64
    } else if let Some([lo, hi]) = entry.range {
        (hi - lo + 1).max(1) as f64
    } else {
        1.0
    };

    let mut mult = 1.0f64;
    for wm in &entry.weight_multipliers {
        let matched = match wm.key.trim().to_lowercase().as_str() {
            "season" => ctx
                .season
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case(&wm.value)),
            "weather" => ctx
                .weather
                .as_deref()
                .is_some_and(|w| w.to_lowercase().contains(&wm.value.to_lowercase())),
            "location" => ctx
                .location
                .as_deref()
                .is_some_and(|l| l.eq_ignore_ascii_case(&wm.value)),
            "place_type" => ctx
                .place_type
                .as_deref()
                .is_some_and(|p| p.eq_ignore_ascii_case(&wm.value)),
            "tag" => ctx.tags.iter().any(|t| t.eq_ignore_ascii_case(&wm.value)),
            "threat_level" => ctx
                .threat_level
                .is_some_and(|t| t.to_string() == wm.value.trim()),
            _ => false,
        };
        if matched {
            mult *= wm.multiplier.max(0.0);
        }
    }

    (base_weight * mult).round().max(0.0) as u32
}

/// Evaluates inline dice templates like `"Found {{2d6}} copper coins and {{1d3}} rations"`.
pub fn expand_inline_dice(template: &str, rng: &mut StdRng) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let expr = after[..end].trim();
            let sub_seed = rng.gen::<u64>();
            if let Ok(r) = roll_dice(expr, Some(sub_seed)) {
                out.push_str(&r.total.to_string());
            } else {
                out.push_str("{{");
                out.push_str(expr);
                out.push_str("}}");
            }
            rest = &after[end + 2..];
        } else {
            out.push_str(&rest[start..]);
            return out;
        }
    }
    out.push_str(rest);
    out
}

/// Draws from a `RandomTable`, supporting weighted selection, dice-range matching,
/// stateful without-replacement decks, inline dice expansion, and cycle-safe nested sub-tables.
pub fn execute_table_draw<F>(
    table: &RandomTable,
    ctx: &DrawContext,
    count: usize,
    replacement: bool,
    drawn_ids: &mut BTreeSet<String>,
    rng: &mut StdRng,
    depth: u8,
    visited_tables: &mut BTreeSet<String>,
    lookup_table: &F,
) -> Result<(usize, Vec<TableDrawItem>)>
where
    F: Fn(&str) -> Result<Option<RandomTable>>,
{
    if depth > 5 {
        bail!("Nested random table depth exceeded limit of 5");
    }
    if !visited_tables.insert(table.id.clone()) {
        bail!("Cycle detected in nested random table '{}'", table.id);
    }

    let mut results = Vec::new();
    let mut first_eligible_count = 0usize;

    for draw_idx in 0..count.max(1) {
        let mut eligible: Vec<(&RandomTableEntry, u32)> = Vec::new();
        for entry in &table.entries {
            if !replacement && drawn_ids.contains(&entry.id) {
                continue;
            }
            let w = compute_entry_weight(entry, ctx);
            if w > 0 {
                eligible.push((entry, w));
            }
        }
        if draw_idx == 0 {
            first_eligible_count = eligible.len();
        }
        if eligible.is_empty() {
            break;
        }

        // If table specifies dice AND all eligible entries have explicit ranges AND no custom weight overrides,
        // try direct dice-range draw when replacement=true and all entries are eligible!
        let mut chosen_entry: Option<(&RandomTableEntry, u32, Option<i64>)> = None;
        if let Some(ref dice_expr) = table.dice {
            if replacement
                && eligible.len() == table.entries.len()
                && eligible.iter().all(|(e, _)| e.range.is_some() && e.weight_multipliers.is_empty())
            {
                let sub_seed = rng.gen::<u64>();
                if let Ok(r) = roll_dice(dice_expr, Some(sub_seed)) {
                    if let Some((e, w)) = eligible
                        .iter()
                        .find(|(e, _)| e.range.is_some_and(|[lo, hi]| r.total >= lo && r.total <= hi))
                    {
                        chosen_entry = Some((*e, *w, Some(r.total)));
                    }
                }
            }
        }

        // Weighted fallback / primary selection
        let (entry, eff_w, roll_val) = if let Some(hit) = chosen_entry {
            hit
        } else {
            let total_w: u64 = eligible.iter().map(|(_, w)| *w as u64).sum();
            let pick = rng.gen_range(1..=total_w);
            let mut acc = 0u64;
            let mut picked = (eligible[0].0, eligible[0].1);
            for (e, w) in &eligible {
                acc += *w as u64;
                if pick <= acc {
                    picked = (*e, *w);
                    break;
                }
            }
            (picked.0, picked.1, Some(pick as i64))
        };

        if !replacement {
            drawn_ids.insert(entry.id.clone());
        }

        let rendered_result = expand_inline_dice(&entry.result, rng);

        // Nested sub-table draw if configured
        let mut sub_results = Vec::new();
        if let Some(ref sub_id) = entry.subtable_id {
            if let Some(sub_tbl) = lookup_table(sub_id)? {
                let mut sub_drawn = BTreeSet::new();
                let (_, sub_items) = execute_table_draw(
                    &sub_tbl,
                    ctx,
                    1,
                    true,
                    &mut sub_drawn,
                    rng,
                    depth + 1,
                    visited_tables,
                    lookup_table,
                )?;
                sub_results = sub_items;
            }
        }

        // Build staged loot item if template exists or table is a loot table
        let loot_item = if let Some(ref tmpl) = entry.loot_template {
            let qty = tmpl
                .quantity_dice
                .as_deref()
                .and_then(|d| roll_dice(d, Some(rng.gen::<u64>())).ok())
                .map(|r| r.total.max(1))
                .unwrap_or(1);
            Some(StagedDiscoveryItem {
                index: results.len(),
                name: expand_inline_dice(&tmpl.name, rng),
                item_type: tmpl.item_type.clone(),
                rarity_or_tier: if tmpl.rarity_or_tier.is_empty() {
                    entry.rarity.clone()
                } else {
                    tmpl.rarity_or_tier.clone()
                },
                quantity: qty,
                condition_or_durability: if tmpl.condition_or_durability.is_empty() {
                    "Intact".to_string()
                } else {
                    tmpl.condition_or_durability.clone()
                },
                properties_and_effects: expand_inline_dice(&tmpl.properties_and_effects, rng),
                description: if tmpl.description.is_empty() {
                    rendered_result.clone()
                } else {
                    expand_inline_dice(&tmpl.description, rng)
                },
                tags: tmpl.tags.clone(),
                claimed_by: None,
            })
        } else if table.is_loot_table || table.category.eq_ignore_ascii_case("loot") {
            Some(StagedDiscoveryItem {
                index: results.len(),
                name: rendered_result.clone(),
                item_type: "loot".to_string(),
                rarity_or_tier: entry.rarity.clone(),
                quantity: 1,
                condition_or_durability: "Found".to_string(),
                properties_and_effects: String::new(),
                description: if entry.notes.is_empty() {
                    rendered_result.clone()
                } else {
                    entry.notes.clone()
                },
                tags: vec!["loot".to_string(), entry.rarity.clone()],
                claimed_by: None,
            })
        } else {
            None
        };

        results.push(TableDrawItem {
            entry_id: entry.id.clone(),
            result: rendered_result,
            rarity: entry.rarity.clone(),
            roll_value: roll_val,
            effective_weight: eff_w,
            subtable_results: sub_results,
            loot_item,
            notes: entry.notes.clone(),
        });
    }

    visited_tables.remove(&table.id);
    Ok((first_eligible_count, results))
}

/// Ensures every entry in a RandomTable has a unique, non-empty ID and sensible weight.
pub fn normalize_random_table(mut table: RandomTable) -> Result<RandomTable> {
    if table.name.trim().is_empty() {
        bail!("Random table name cannot be empty");
    }
    if table.id.trim().is_empty() {
        table.id = slugify(&table.name);
    } else {
        table.id = slugify(&table.id);
    }
    if table.entries.is_empty() {
        bail!("Random table '{}' must contain at least one entry", table.id);
    }
    if table.category.eq_ignore_ascii_case("loot") {
        table.is_loot_table = true;
    }
    let mut seen = BTreeSet::new();
    for (idx, e) in table.entries.iter_mut().enumerate() {
        if e.id.trim().is_empty() {
            e.id = format!("entry-{}", idx + 1);
        } else {
            e.id = slugify(&e.id);
        }
        while !seen.insert(e.id.clone()) {
            e.id = format!("{}-{}", e.id, idx + 1);
        }
        if e.weight == 0 {
            if let Some([lo, hi]) = e.range {
                e.weight = (hi - lo + 1).max(1) as u32;
            } else {
                e.weight = 1;
            }
        }
    }
    Ok(table)
}

// ============================================================================
// 3. Encounter & Initiative Engine (Combat & Non-Combat Scenes)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncounterParticipant {
    pub entity_id: String,
    pub name: String,
    /// e.g. "party", "hostile", "neutral", "faction-a"
    #[serde(default = "default_side")]
    pub side: String,
    pub initiative: i64,
    #[serde(default)]
    pub initiative_roll: Option<i64>,
    #[serde(default)]
    pub position: String,
    #[serde(default)]
    pub range_band: String,
    #[serde(default = "default_true")]
    pub reaction_available: bool,
    /// "active" | "incapacitated" | "defeated" | "fled" | "surrendered"
    #[serde(default = "default_part_status")]
    pub status: String,
    #[serde(default)]
    pub notes: String,
}

fn default_side() -> String {
    "neutral".to_string()
}

fn default_part_status() -> String {
    "active".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncounterLogEntry {
    pub round: u32,
    pub actor_id: String,
    pub action_type: String,
    pub description: String,
    #[serde(default)]
    pub target_ids: Vec<String>,
    #[serde(default)]
    pub event_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncounterState {
    pub id: String,
    pub title: String,
    /// "combat" | "negotiation" | "pursuit" | "survival" | "cultivation_contest" | "stealth" | custom
    #[serde(default = "default_enc_type")]
    pub encounter_type: String,
    #[serde(default)]
    pub location_id: Option<String>,
    /// "active" | "ended"
    pub status: String,
    pub round: u32,
    pub active_turn_index: usize,
    #[serde(default)]
    pub zones: Vec<String>,
    #[serde(default)]
    pub environmental_notes: String,
    pub participants: Vec<EncounterParticipant>,
    #[serde(default)]
    pub action_log: Vec<EncounterLogEntry>,
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub aftermath: String,
    pub started_at_world_time: String,
    #[serde(default)]
    pub ended_at_world_time: Option<String>,
}

fn default_enc_type() -> String {
    "combat".to_string()
}

impl EncounterState {
    pub fn sort_initiative(&mut self) {
        self.participants.sort_by(|a, b| {
            b.initiative
                .cmp(&a.initiative)
                .then_with(|| a.entity_id.cmp(&b.entity_id))
        });
    }

    pub fn active_participant(&self) -> Option<&EncounterParticipant> {
        self.participants.get(self.active_turn_index)
    }

    /// Advances to the next active participant, incrementing round on wrap-around and resetting reactions.
    pub fn advance_turn(&mut self) -> Option<String> {
        if self.participants.is_empty() {
            return None;
        }
        let len = self.participants.len();
        for step in 1..=len {
            let next_idx = (self.active_turn_index + step) % len;
            if next_idx <= self.active_turn_index {
                self.round += 1;
            }
            if self.participants[next_idx].status.eq_ignore_ascii_case("active") {
                self.active_turn_index = next_idx;
                self.participants[next_idx].reaction_available = true;
                return Some(self.participants[next_idx].entity_id.clone());
            }
        }
        None
    }
}

/// Optional causal metadata attached to a ledger event.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventCausalMeta {
    #[serde(default)]
    pub actor_id: Option<String>,
    #[serde(default)]
    pub causal_event_ids: Vec<i64>,
    #[serde(default = "default_visibility")]
    pub visibility: String,
}

/// Helper for turning arbitrary JSON into clean ModifierSource entries.
pub fn parse_modifier_sources(v: Option<&Value>) -> Vec<ModifierSource> {
    v.and_then(|val| serde_json::from_value::<Vec<ModifierSource>>(val.clone()).ok())
        .unwrap_or_default()
}
