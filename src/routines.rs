use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Condition template applied automatically by a `StageTransitionRule`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct StageConditionSpec {
    pub name: String,
    #[serde(default = "default_cond_kind")]
    pub kind: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub duration: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub notes: String,
}

fn default_cond_kind() -> String {
    "condition".to_string()
}

/// Innate/constitutional trait or feat granted automatically by a `StageTransitionRule`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct StageTraitSpec {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default = "default_trait_cat")]
    pub category: String,
    #[serde(default)]
    pub rank_or_grade: String,
    #[serde(default = "default_true_rt")]
    pub active: bool,
    #[serde(default)]
    pub effect: String,
    #[serde(default)]
    pub notes: String,
}

fn default_trait_cat() -> String {
    "trait".to_string()
}

pub fn default_true_rt() -> bool {
    true
}

fn default_signal_vis() -> String {
    "player".to_string()
}

/// Genre-agnostic declarative state-machine bundle triggered automatically when a character
/// enters a progression stage or sub-stage (works for Xianxia body/qi manuals, D&D ASI/class
/// features, Sci-Fi cyberware/gene tiers, Horror corruption stages, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StageTransitionRule {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub appearance_stage: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub avatar_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body_type_or_build: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub silhouette: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub posture_and_gait: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proportions_summary: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub clothing_fit: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub face_shape_and_features: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub skin_tone_and_texture: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hair: String,
    /// `Some("")` explicitly clears facial hair; `None` leaves existing facial hair unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facial_hair: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub voice_and_speech: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub perceived_change_note: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_transformation_trajectory: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub height: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub weight: String,
    /// Canonical body measurements set at this stage (e.g. `{"Shoulders": "47 cm", "Chest": "104 cm", "Waist": "82 cm", "Hips": "106 cm"}`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub measurements: BTreeMap<String, String>,
    /// Exact stat score overrides applied upon entering this stage.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stat_set: BTreeMap<String, i64>,
    /// Signed stat deltas applied upon entering this stage (e.g. `{"Strength": -1, "Dexterity": 1}`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stat_deltas: BTreeMap<String, i64>,
    /// Free attribute points granted at this stage (e.g. `+2` for D&D 5e ASI at levels 4/8/12/16/19, or reassigned points).
    #[serde(default)]
    pub unspent_stat_points_delta: i64,
    /// Status conditions automatically added or updated (supports `[scope: ...]` penalties).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upsert_conditions: Vec<StageConditionSpec>,
    /// Names of status conditions automatically cleared upon entering this stage.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_conditions: Vec<String>,
    /// Constitutional traits, perks, feats, or flaws unlocked at this stage.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grant_traits: Vec<StageTraitSpec>,
    /// Summary recorded into `avatar.transformation_log` checkpoint if avatar/measurements changed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub checkpoint_summary: String,
    /// Directive surfaced on the Signal Bus telling the GM how to portray this transition in prose.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub narrative_directive: String,
    #[serde(default = "default_signal_vis")]
    pub signal_visibility: String,
}

/// Standing recurring habit, work shift, cultivation/training regimen, study schedule, or daily living upkeep
/// that automatically ticks across `advance_time` and `advance_world` when guard conditions hold.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterRoutine {
    #[serde(default)]
    pub id: String,
    pub character_id: String,
    pub name: String,
    /// "training" | "work" | "upkeep" | "study" | "recovery" | "custom"
    #[serde(default = "default_routine_category")]
    pub category: String,
    /// Hours per cycle (default: 24 for daily routines; 168 for weekly).
    #[serde(default = "default_cadence_24")]
    pub cadence_hours: u64,
    #[serde(default = "default_true_rt", alias = "active")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty", alias = "pause_reason")]
    pub paused_reason: String,
    /// Optional location guard: routine pauses automatically if character leaves this place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub require_location_id: Option<String>,
    /// Optional injury guard ("minor" | "moderate" | "severe" | "critical"): pauses if character has an active injury >= this severity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub halt_on_injury_severity: Option<String>,
    /// Optional condition names/substrings that pause this routine (e.g. ["Exhausted", "Captured"]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub halt_if_conditions: Vec<String>,
    /// Minimum currency balances required to run a cycle (e.g. upkeep or paid tutor).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub min_currency: BTreeMap<String, i64>,
    /// Progression track XP awarded per completed cycle (`track_id -> xp`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub progression_xp: BTreeMap<String, u64>,
    /// Skill XP awarded per completed cycle (`skill_name -> xp`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skill_xp: BTreeMap<String, u64>,
    /// Job/Occupation XP awarded per completed cycle (`job_id -> xp`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub job_xp: BTreeMap<String, u64>,
    /// Stat Training XP awarded per completed cycle (`stat_name -> xp`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stat_training_xp: BTreeMap<String, u64>,
    /// Currency deltas per completed cycle (`currency -> signed amount`, e.g. `+40` wages or `-15` daily meals).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub currency_deltas: BTreeMap<String, i64>,
    /// Resource pool deltas per completed cycle (`resource -> signed amount`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub resource_deltas: BTreeMap<String, i64>,
    /// Optional counterparty owner_id for currency_deltas (employer/landlord/treasury).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterparty_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tick_total_hours: Option<i64>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_routine_category() -> String {
    "training".to_string()
}

fn default_cadence_24() -> u64 {
    24
}

pub fn injury_severity_rank(sev: &str) -> u8 {
    match sev.trim().to_ascii_lowercase().as_str() {
        "minor" | "light" => 1,
        "moderate" => 2,
        "severe" | "major" => 3,
        "critical" => 4,
        _ => 0,
    }
}

/// Exponential XP curve for earning attribute points through sustained physical/mental training:
/// `100, 145, 210, 305, 442, 641, ...`
pub fn stat_training_threshold_for_points(points_already_gained: i64) -> u64 {
    let exp = points_already_gained.clamp(0, 30) as i32;
    (100.0 * 1.45_f64.powi(exp)).round().max(100.0) as u64
}
