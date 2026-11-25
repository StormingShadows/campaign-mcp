use crate::models::slugify;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ============================================================================
// 1. Structured Character Motivations, Psychology & Prioritized Goals
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StructuredGoal {
    pub goal_id: String,
    pub description: String,
    #[serde(default = "default_priority")]
    pub priority: u8,
    #[serde(default)]
    pub progress: i64,
    #[serde(default = "default_target")]
    pub target: i64,
    #[serde(default)]
    pub deadline: Option<String>,
    #[serde(default)]
    pub resources_required: Vec<String>,
    #[serde(default)]
    pub known_to: Vec<String>,
    /// "active" | "completed" | "abandoned" | "blocked"
    #[serde(default = "default_goal_status")]
    pub status: String,
    #[serde(default)]
    pub notes: String,
}

fn default_priority() -> u8 {
    5
}

fn default_target() -> i64 {
    10
}

fn default_goal_status() -> String {
    "active".to_string()
}

/// Extended structured profile stored alongside immutable `CharacterBaseline`.
/// Covers Identity, Physical, Psychological/Motivations, Social obligations, and Prioritized Goals.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CharacterProfileExt {
    pub character_id: String,
    // Identity
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub age_or_birthdate: String,
    #[serde(default)]
    pub gender: String,
    #[serde(default)]
    pub species: String,
    #[serde(default)]
    pub occupation: String,
    #[serde(default)]
    pub social_class: String,
    // Physical
    #[serde(default)]
    pub height_and_build: String,
    #[serde(default)]
    pub injuries_and_disabilities: Vec<String>,
    #[serde(default)]
    pub body_constitution: String,
    #[serde(default)]
    pub body_tempering: String,
    // Psychological & Autonomy Drivers
    #[serde(default)]
    pub temperament: String,
    #[serde(default)]
    pub long_term_motivations: Vec<String>,
    #[serde(default)]
    pub values_and_principles: Vec<String>,
    #[serde(default)]
    pub fears: Vec<String>,
    #[serde(default)]
    pub loyalties: Vec<String>,
    #[serde(default)]
    pub ambitions: Vec<String>,
    #[serde(default)]
    pub commitments: Vec<String>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub habits: Vec<String>,
    /// "cautious" | "measured" | "bold" | "reckless"
    #[serde(default = "default_risk_tolerance")]
    pub risk_tolerance: String,
    #[serde(default)]
    pub stress: i64,
    #[serde(default = "default_morale")]
    pub morale: i64,
    #[serde(default)]
    pub current_plans: Vec<String>,
    // Social
    #[serde(default)]
    pub family: Vec<String>,
    #[serde(default)]
    pub obligations: Vec<String>,
    #[serde(default)]
    pub reputation_notes: String,
    // Structured Prioritized Goals
    #[serde(default)]
    pub structured_goals: Vec<StructuredGoal>,
    // Dedicated Player Character / Deep Character Sheet Architecture
    #[serde(default)]
    pub avatar: AvatarBodyProfile,
    #[serde(default)]
    pub titles: Vec<CharacterTitle>,
    #[serde(default)]
    pub unspent_stat_points: i64,
    /// Properly segregated Attribute / Stat Training XP (distinct from Job XP, Skill XP, and Cultivation XP).
    #[serde(default)]
    pub stat_training: BTreeMap<String, StatTrainingEntry>,
    /// Properly segregated Job / Class / Occupation XP tracks (primary, secondary, trade, sect roles).
    #[serde(default)]
    pub jobs: Vec<CharacterJob>,
    /// Formal Proficiencies (weapons, armor, tools, crafts, languages, saves, elemental affinities).
    #[serde(default)]
    pub proficiencies: Vec<CharacterProficiency>,
    /// Constitutional / Innate Traits (physique, spirit root, bloodline, feats, talents, cyberware, flaws).
    #[serde(default)]
    pub traits_and_perks: Vec<CharacterTrait>,
    /// Living-world NPC mobility, business circuits, diplomatic/networking travel & engagements.
    #[serde(default)]
    pub mobility: NpcMobilityProfile,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_risk_tolerance() -> String {
    "measured".to_string()
}

fn default_morale() -> i64 {
    60
}

// ============================================================================
// 1b. Dedicated Player Character Sheet: Avatar/Body, Measurements, Wardrobe,
//     Titles, Segregated Stat/Job/Proficiency XP, and Innate Traits
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PhysicalMarker {
    #[serde(default)]
    pub id: String,
    /// "scar" | "tattoo" | "birthmark" | "brand" | "prosthetic" | "cultivation_mark" | "other"
    #[serde(default = "default_marker_kind")]
    pub kind: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub origin_or_story: String,
    #[serde(default = "default_true_au")]
    pub visible_when_clothed: bool,
}

fn default_marker_kind() -> String {
    "scar".to_string()
}

fn default_true_au() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CharacterOutfit {
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// e.g. "adventuring" | "sect_robes" | "formal" | "casual" | "disguise" | "combat"
    #[serde(default)]
    pub occasion: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub headwear: String,
    #[serde(default)]
    pub outer_layer: String,
    #[serde(default)]
    pub top_layer: String,
    #[serde(default)]
    pub bottoms: String,
    #[serde(default)]
    pub footwear: String,
    #[serde(default)]
    pub accessories: String,
    #[serde(default)]
    pub color_palette: String,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AppearanceCheckpoint {
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub world_time: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub height: String,
    #[serde(default)]
    pub weight: String,
    #[serde(default)]
    pub measurements: BTreeMap<String, String>,
}

/// Canonicalizes common body measurement keys so case/underscore variants
/// (e.g. `upper_arm`, `UpperArm`, `Upper Arm`) never create duplicate rows.
pub fn normalize_measurement_key(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let compact: String = trimmed
        .chars()
        .map(|c| if c == '_' || c == '-' { ' ' } else { c.to_ascii_lowercase() })
        .collect();
    let squashed = compact.split_whitespace().collect::<Vec<_>>().join(" ");
    match squashed.as_str() {
        "shoulder" | "shoulders" | "shoulder width" => "Shoulders".to_string(),
        "chest" | "chest / bust" | "chest/bust" | "bust" | "bust / chest" => "Chest".to_string(),
        "underbust" | "under bust" | "ribcage" => "Underbust".to_string(),
        "waist" | "waistline" => "Waist".to_string(),
        "hip" | "hips" | "hips / seat" | "hips/seat" => "Hips".to_string(),
        "neck" => "Neck".to_string(),
        "upper arm" | "upperarm" | "bicep" | "biceps" => "Upper Arm".to_string(),
        "forearm" | "forearms" => "Forearm".to_string(),
        "wrist" | "wrists" => "Wrist".to_string(),
        "thigh" | "thighs" => "Thigh".to_string(),
        "calf" | "calves" => "Calf".to_string(),
        "arm span" | "armspan" | "arm span / reach" | "reach" => "Arm Span".to_string(),
        "inseam" | "inseam / legs" | "leg inseam" => "Inseam".to_string(),
        "head to body ratio" | "head-to-body ratio" | "proportions ratio" => {
            "Head-to-Body Ratio".to_string()
        }
        _ => trimmed.to_string(),
    }
}

/// Canonical anatomical display order for measurements.
pub fn measurement_sort_rank(key: &str) -> usize {
    match normalize_measurement_key(key).as_str() {
        "Shoulders" => 1,
        "Chest" => 2,
        "Underbust" => 3,
        "Waist" => 4,
        "Hips" => 5,
        "Neck" => 6,
        "Upper Arm" => 7,
        "Forearm" => 8,
        "Wrist" => 9,
        "Thigh" => 10,
        "Calf" => 11,
        "Arm Span" => 12,
        "Inseam" => 13,
        "Head-to-Body Ratio" => 14,
        _ => 100,
    }
}

fn parse_leading_measure(raw: &str) -> Option<(f64, String)> {
    let s = raw.trim().trim_start_matches('~').trim();
    let s = s
        .strip_prefix("approx.")
        .or_else(|| s.strip_prefix("approx"))
        .or_else(|| s.strip_prefix("about"))
        .map(str::trim)
        .unwrap_or(s);
    let mut num_end = 0usize;
    let mut seen_digit = false;
    let mut seen_dot = false;
    for (idx, ch) in s.char_indices() {
        if ch.is_ascii_digit() {
            seen_digit = true;
            num_end = idx + ch.len_utf8();
        } else if ch == '.' && !seen_dot && seen_digit {
            seen_dot = true;
            num_end = idx + 1;
        } else if (ch == '+' || ch == '-') && idx == 0 {
            num_end = 1;
        } else {
            break;
        }
    }
    if !seen_digit {
        return None;
    }
    let val: f64 = s[..num_end].parse().ok()?;
    let rest = s[num_end..].trim();
    let unit = rest
        .split(|c: char| c.is_whitespace() || c == ';' || c == ',' || c == '(')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    Some((val, unit))
}

fn format_compact_number(v: f64) -> String {
    let rounded = (v * 10.0).round() / 10.0;
    if (rounded - rounded.round()).abs() < 1e-6 {
        format!("{}", rounded.round() as i64)
    } else {
        format!("{rounded:.1}")
    }
}

/// Auto-computes a concise numeric delta between a baseline and current measurement string
/// (e.g. `"91 cm"` vs `"88 cm"` -> `Some("-3 cm")`, `"104 cm"` vs `"105 cm"` -> `Some("+1 cm")`).
pub fn compute_measurement_delta(baseline: &str, current: &str) -> Option<String> {
    let (b_val, b_unit) = parse_leading_measure(baseline)?;
    let (c_val, c_unit) = parse_leading_measure(current)?;
    if !b_unit.is_empty() && !c_unit.is_empty() && !b_unit.eq_ignore_ascii_case(&c_unit) {
        return None;
    }
    let diff = c_val - b_val;
    if diff.abs() < 0.05 {
        return Some("—".to_string());
    }
    let unit = if !c_unit.is_empty() { c_unit } else { b_unit };
    let sign = if diff > 0.0 { "+" } else { "-" };
    let mag = format_compact_number(diff.abs());
    if unit.is_empty() {
        Some(format!("{sign}{mag}"))
    } else {
        Some(format!("{sign}{mag} {unit}"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AvatarBodyProfile {
    /// Core prose description of the character's CURRENT observable physical presence (strictly absolute, present-tense, stranger's-eye view).
    #[serde(default)]
    pub avatar_summary: String,
    /// Optional stage/phase tag for a transforming character (e.g. "Still Water Layer 5/9 — Low-Gradient Flow").
    #[serde(default)]
    pub appearance_stage: String,
    #[serde(default)]
    pub height: String,
    #[serde(default)]
    pub weight: String,
    /// Original Day-1 / pre-transformation baseline height for delta tracking.
    #[serde(default)]
    pub baseline_height: String,
    /// Original Day-1 / pre-transformation baseline weight for delta tracking.
    #[serde(default)]
    pub baseline_weight: String,
    #[serde(default)]
    pub chronological_age: String,
    #[serde(default)]
    pub apparent_age: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifespan_years: Option<u32>,
    #[serde(default)]
    pub body_type_or_build: String,
    #[serde(default)]
    pub silhouette: String,
    #[serde(default)]
    pub posture_and_gait: String,
    #[serde(default)]
    pub proportions_summary: String,
    /// Concise present-tense description of how current clothing sits and drapes on the character's body.
    #[serde(default)]
    pub clothing_fit: String,
    /// Optional 1-sentence player-safe subjective note of ongoing physical drift (shown in Player/PC & GM POVs, hidden from other NPCs).
    #[serde(default)]
    pub perceived_change_note: String,
    /// Optional GM-only active transformation vector & next-stage update trigger/recipe (strictly hidden from Player & NPC POVs).
    #[serde(default)]
    pub transformation_trajectory: String,
    /// Structured CURRENT physical measurements (canonical keys -> clean scalar values, e.g. "Shoulders": "49 cm").
    #[serde(default)]
    pub measurements: BTreeMap<String, String>,
    /// Structured BASELINE (Day-1 / pre-transformation) measurements for automatic delta calculation.
    #[serde(default)]
    pub baseline_measurements: BTreeMap<String, String>,
    #[serde(default)]
    pub face_shape_and_features: String,
    #[serde(default)]
    pub eyes: String,
    #[serde(default)]
    pub skin_tone_and_texture: String,
    #[serde(default)]
    pub hair: String,
    #[serde(default)]
    pub facial_hair: String,
    #[serde(default)]
    pub voice_and_speech: String,
    #[serde(default)]
    pub scent_or_aura: String,
    /// Optional linked image attachment ID in `Docs & Media` for visual portrait rendering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portrait_attachment_id: Option<String>,
    /// Reusable AI avatar/image visual-consistency prompt anchor.
    #[serde(default)]
    pub visual_prompt_anchor: String,
    /// Currently worn outfit ID or name.
    #[serde(default)]
    pub active_outfit: String,
    #[serde(default)]
    pub permanent_markers: Vec<PhysicalMarker>,
    #[serde(default)]
    pub outfits: Vec<CharacterOutfit>,
    /// Structured historical checkpoints for a transforming body.
    #[serde(default)]
    pub transformation_log: Vec<AppearanceCheckpoint>,
}

impl AvatarBodyProfile {
    pub fn is_empty(&self) -> bool {
        self.avatar_summary.is_empty()
            && self.appearance_stage.is_empty()
            && self.height.is_empty()
            && self.weight.is_empty()
            && self.baseline_height.is_empty()
            && self.baseline_weight.is_empty()
            && self.body_type_or_build.is_empty()
            && self.silhouette.is_empty()
            && self.proportions_summary.is_empty()
            && self.clothing_fit.is_empty()
            && self.perceived_change_note.is_empty()
            && self.transformation_trajectory.is_empty()
            && self.measurements.is_empty()
            && self.baseline_measurements.is_empty()
            && self.face_shape_and_features.is_empty()
            && self.eyes.is_empty()
            && self.skin_tone_and_texture.is_empty()
            && self.hair.is_empty()
            && self.voice_and_speech.is_empty()
            && self.permanent_markers.is_empty()
            && self.outfits.is_empty()
            && self.transformation_log.is_empty()
            && self.portrait_attachment_id.is_none()
            && self.lifespan_years.is_none()
    }

    /// Returns merged canonical measurement rows `(key, baseline, current, delta)` sorted in anatomical order.
    pub fn measurement_rows(&self) -> Vec<(String, String, String, String)> {
        let mut keys: Vec<String> = self
            .baseline_measurements
            .keys()
            .chain(self.measurements.keys())
            .cloned()
            .collect();
        keys.sort_by(|a, b| {
            measurement_sort_rank(a)
                .cmp(&measurement_sort_rank(b))
                .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
        });
        keys.dedup();
        keys.into_iter()
            .map(|k| {
                let b = self.baseline_measurements.get(&k).cloned().unwrap_or_default();
                let c = self.measurements.get(&k).cloned().unwrap_or_default();
                let delta = if !b.is_empty() && !c.is_empty() {
                    compute_measurement_delta(&b, &c).unwrap_or_else(|| "—".to_string())
                } else {
                    "—".to_string()
                };
                (k, b, c, delta)
            })
            .collect()
    }

    /// Formats a strictly absolute, present-tense, stranger's-eye appearance brief
    /// (zero baseline numbers, zero comparative phrasing, safe for new NPC encounters).
    pub fn format_observable_brief(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut parts: Vec<String> = Vec::new();
        let hw: Vec<&str> = [self.height.as_str(), self.weight.as_str()]
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect();
        if !hw.is_empty() {
            parts.push(hw.join(", "));
        }
        // Include compact current torso dimensions (Shoulders/Chest/Waist/Hips) if populated
        let core_dims: Vec<String> = ["Shoulders", "Chest", "Waist", "Hips"]
            .into_iter()
            .filter_map(|k| {
                self.measurements
                    .get(k)
                    .filter(|v| !v.trim().is_empty())
                    .map(|v| format!("{k} {}", v.trim()))
            })
            .collect();
        if !core_dims.is_empty() {
            parts.push(core_dims.join(" / "));
        }
        if !self.avatar_summary.trim().is_empty() {
            parts.push(self.avatar_summary.trim().to_string());
        } else if !self.body_type_or_build.trim().is_empty() {
            parts.push(self.body_type_or_build.trim().to_string());
        }
        if !self.clothing_fit.trim().is_empty() {
            parts.push(format!("Fit: {}", self.clothing_fit.trim()));
        }
        if let Some(o) = self.active_outfit_ref() {
            parts.push(format!("Wearing: {}", o.name));
        }
        parts.join(" | ")
    }

    /// Formats a compact 1-line auto-computed delta vs. baseline for GM / returning-NPC reference ONLY.
    /// Lists only metrics that have actually shifted from baseline (plus stage tag if present).
    pub fn format_delta_brief(&self) -> String {
        let mut diffs: Vec<String> = Vec::new();
        if !self.baseline_height.trim().is_empty() && !self.height.trim().is_empty() {
            if let Some(d) = compute_measurement_delta(&self.baseline_height, &self.height) {
                if d != "—" {
                    diffs.push(format!(
                        "Height {}→{} ({d})",
                        self.baseline_height.trim(),
                        self.height.trim()
                    ));
                }
            }
        }
        if !self.baseline_weight.trim().is_empty() && !self.weight.trim().is_empty() {
            if let Some(d) = compute_measurement_delta(&self.baseline_weight, &self.weight) {
                if d != "—" {
                    diffs.push(format!(
                        "Weight {}→{} ({d})",
                        self.baseline_weight.trim(),
                        self.weight.trim()
                    ));
                }
            }
        }
        for (k, b, c, d) in self.measurement_rows() {
            if !b.is_empty() && !c.is_empty() && d != "—" {
                diffs.push(format!("{k} {}→{} ({d})", b.trim(), c.trim()));
            } else if b.is_empty() && !c.is_empty() && !self.baseline_measurements.is_empty() {
                diffs.push(format!("{k} new: {}", c.trim()));
            }
        }
        let mut out = Vec::new();
        if !self.appearance_stage.trim().is_empty() {
            out.push(format!("Stage: {}", self.appearance_stage.trim()));
        }
        if !diffs.is_empty() {
            out.push(diffs.join(" | "));
        }
        out.join(" — ")
    }

    pub fn active_outfit_ref(&self) -> Option<&CharacterOutfit> {
        if self.active_outfit.trim().is_empty() {
            return self.outfits.first();
        }
        let target = self.active_outfit.trim();
        self.outfits
            .iter()
            .find(|o| o.id.eq_ignore_ascii_case(target) || o.name.eq_ignore_ascii_case(target))
            .or_else(|| self.outfits.first())
    }

    pub fn synthesize_visual_prompt(&self, name: &str, archetype: &str) -> String {
        if !self.visual_prompt_anchor.trim().is_empty() {
            return self.visual_prompt_anchor.trim().to_string();
        }
        let mut parts = Vec::new();
        let subj = if archetype.trim().is_empty() {
            name.to_string()
        } else {
            format!("{name} ({archetype})")
        };
        parts.push(subj);
        if !self.apparent_age.is_empty() {
            parts.push(format!("apparent age {}", self.apparent_age));
        }
        if !self.height.is_empty() || !self.body_type_or_build.is_empty() {
            parts.push(
                [self.height.as_str(), self.body_type_or_build.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if !self.proportions_summary.is_empty() {
            parts.push(self.proportions_summary.clone());
        }
        if !self.face_shape_and_features.is_empty() {
            parts.push(self.face_shape_and_features.clone());
        }
        if !self.eyes.is_empty() {
            parts.push(format!("eyes: {}", self.eyes));
        }
        if !self.skin_tone_and_texture.is_empty() {
            parts.push(format!("skin: {}", self.skin_tone_and_texture));
        }
        if !self.hair.is_empty() {
            parts.push(format!("hair: {}", self.hair));
        }
        let vis_markers: Vec<String> = self
            .permanent_markers
            .iter()
            .filter(|m| m.visible_when_clothed)
            .map(|m| format!("{} on {} ({})", m.kind, m.location, m.description))
            .collect();
        if !vis_markers.is_empty() {
            parts.push(format!("markers: {}", vis_markers.join("; ")));
        }
        if let Some(o) = self.active_outfit_ref() {
            let outfit_desc = if !o.description.is_empty() {
                o.description.clone()
            } else {
                [
                    o.outer_layer.as_str(),
                    o.top_layer.as_str(),
                    o.bottoms.as_str(),
                    o.footwear.as_str(),
                    o.accessories.as_str(),
                ]
                .iter()
                .filter(|s| !s.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(", ")
            };
            if !outfit_desc.is_empty() {
                parts.push(format!("wearing {} ({})", o.name, outfit_desc));
            }
        }
        parts.join("; ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CharacterTitle {
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// "title" | "epithet" | "rank" | "honorific" | "system_title" | "blessing" | "curse"
    #[serde(default = "default_title_kind")]
    pub kind: String,
    #[serde(default)]
    pub equipped: bool,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub mechanical_effect: String,
    #[serde(default)]
    pub stat_modifiers: BTreeMap<String, i64>,
    /// "public" | "faction" | "hidden" | "gm_secret"
    #[serde(default = "default_public_vis")]
    pub visibility: String,
    #[serde(default)]
    pub notes: String,
}

fn default_title_kind() -> String {
    "title".to_string()
}

fn default_public_vis() -> String {
    "public".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatTrainingEntry {
    pub stat: String,
    #[serde(default)]
    pub current_xp: u64,
    #[serde(default = "default_stat_xp_next")]
    pub xp_to_next: u64,
    #[serde(default)]
    pub cumulative_xp: u64,
    #[serde(default)]
    pub points_gained_from_training: i64,
    #[serde(default)]
    pub training_notes: String,
}

fn default_stat_xp_next() -> u64 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharacterJob {
    #[serde(default)]
    pub job_id: String,
    pub name: String,
    /// "primary" | "secondary" | "occupation" | "trade" | "sect_role"
    #[serde(default = "default_job_role")]
    pub role: String,
    #[serde(default)]
    pub tier_or_rank: String,
    #[serde(default = "default_one_u32")]
    pub level: u32,
    #[serde(default)]
    pub current_xp: u64,
    #[serde(default = "default_job_xp_next")]
    pub xp_to_next: u64,
    #[serde(default)]
    pub cumulative_xp: u64,
    #[serde(default = "default_true_au")]
    pub active: bool,
    #[serde(default)]
    pub governing_stats: Vec<String>,
    #[serde(default)]
    pub perks_and_features: Vec<String>,
    #[serde(default)]
    pub notes: String,
}

fn default_job_role() -> String {
    "primary".to_string()
}

fn default_one_u32() -> u32 {
    1
}

fn default_job_xp_next() -> u64 {
    250
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharacterProficiency {
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// "weapon" | "armor" | "tool" | "craft" | "language" | "save" | "affinity" | "other"
    #[serde(default = "default_prof_cat")]
    pub category: String,
    /// "Untrained" | "Familiar" | "Proficient" | "Expertise" | "Master" | "Grandmaster"
    #[serde(default = "default_prof_rank")]
    pub rank: String,
    #[serde(default)]
    pub xp: u64,
    #[serde(default = "default_stat_xp_next")]
    pub xp_to_next: u64,
    #[serde(default)]
    pub bonus: String,
    #[serde(default)]
    pub notes: String,
}

fn default_prof_cat() -> String {
    "tool".to_string()
}

fn default_prof_rank() -> String {
    "Proficient".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CharacterTrait {
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// "physique" | "spirit_root" | "bloodline" | "feat" | "talent" | "flaw" | "cyberware" | "blessing"
    #[serde(default = "default_trait_cat")]
    pub category: String,
    #[serde(default)]
    pub rank_or_grade: String,
    #[serde(default = "default_true_au")]
    pub active: bool,
    #[serde(default)]
    pub effect: String,
    #[serde(default)]
    pub notes: String,
}

fn default_trait_cat() -> String {
    "trait".to_string()
}

// ============================================================================
// 1c. Living World NPC Mobility, Business Circuits & Networking Engagements
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NpcTripState {
    pub origin_place_id: String,
    pub destination_place_id: String,
    #[serde(default)]
    pub route_waypoints: Vec<String>,
    /// e.g. "business" | "networking" | "diplomacy" | "inspection" | "procurement" | "sect_business" | "return_home"
    #[serde(default = "default_trip_purpose")]
    pub purpose: String,
    #[serde(default)]
    pub details: String,
    #[serde(default)]
    pub departed_at_world_time: String,
    #[serde(default)]
    pub departed_total_hours: i64,
    #[serde(default)]
    pub eta_world_time: String,
    #[serde(default)]
    pub eta_total_hours: i64,
    /// When visiting destination, earliest total_hours before departing for next stop/home.
    #[serde(default)]
    pub return_after_total_hours: i64,
    #[serde(default)]
    pub return_after_world_time: String,
    #[serde(default = "default_transport_mode")]
    pub transport_mode: String,
    #[serde(default)]
    pub detour_note: String,
}

fn default_trip_purpose() -> String {
    "business".to_string()
}

fn default_transport_mode() -> String {
    "mount".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NpcEngagement {
    pub world_time: String,
    pub place_id: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(default)]
    pub counterparts: Vec<String>,
    pub summary: String,
    #[serde(default = "default_public_vis")]
    pub visibility: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NpcMobilityProfile {
    /// Whether this NPC is a prominent figure who travels for business, networking, inspections, etc.
    #[serde(default)]
    pub prominent: bool,
    /// "auto" | "stationary" | "local_commuter" | "business_circuit" | "networking_envoy" | "inspector" | "wanderer"
    #[serde(default = "default_mobility_mode")]
    pub mobility_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_place_id: Option<String>,
    /// Named circuit stops or regular business/networking hubs (auto-discovers connected routes if empty).
    #[serde(default)]
    pub preferred_destinations: Vec<String>,
    /// Typical travel motives (e.g. "business", "networking", "diplomacy", "procurement", "inspection").
    #[serde(default)]
    pub purposes: Vec<String>,
    /// Typical days spent at home between outgoing business/networking trips.
    #[serde(default = "default_trip_cadence_days")]
    pub trip_cadence_days: u32,
    /// Typical days spent visiting a destination before returning or moving to next circuit stop.
    #[serde(default = "default_stay_duration_days")]
    pub stay_duration_days: u32,
    #[serde(default = "default_transport_mode")]
    pub travel_speed_mode: String,
    /// Optional aides, bodyguards, or apprentices who travel with this figure.
    #[serde(default)]
    pub entourage_ids: Vec<String>,
    /// "at_home" | "in_transit" | "visiting" | "stranded"
    #[serde(default = "default_mobility_status")]
    pub current_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_trip: Option<NpcTripState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_trip_ended_total_hours: Option<i64>,
    #[serde(default)]
    pub circuit_cursor: usize,
    #[serde(default)]
    pub recent_engagements: Vec<NpcEngagement>,
}

fn default_mobility_mode() -> String {
    "auto".to_string()
}

fn default_trip_cadence_days() -> u32 {
    7
}

fn default_stay_duration_days() -> u32 {
    3
}

fn default_mobility_status() -> String {
    "at_home".to_string()
}

impl Default for NpcMobilityProfile {
    fn default() -> Self {
        Self {
            prominent: false,
            mobility_mode: default_mobility_mode(),
            home_place_id: None,
            preferred_destinations: Vec::new(),
            purposes: Vec::new(),
            trip_cadence_days: default_trip_cadence_days(),
            stay_duration_days: default_stay_duration_days(),
            travel_speed_mode: default_transport_mode(),
            entourage_ids: Vec::new(),
            current_status: default_mobility_status(),
            active_trip: None,
            last_trip_ended_total_hours: None,
            circuit_cursor: 0,
            recent_engagements: Vec::new(),
        }
    }
}

impl NpcMobilityProfile {
    pub fn is_configured(&self) -> bool {
        self.prominent
            || self.mobility_mode != "auto"
            || self.home_place_id.is_some()
            || !self.preferred_destinations.is_empty()
            || self.active_trip.is_some()
            || !self.recent_engagements.is_empty()
    }
}

// ============================================================================
// 2. Epistemic Knowledge & Belief System (Fog-of-War & Partial/Distorted Truth)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeClaim {
    pub id: String,
    /// Entity ID or topic this claim is about (e.g. "yan-shichuan", "old-stone-bridge").
    pub subject: String,
    /// Canonical objective statement of the fact or circulating rumor.
    pub claim: String,
    /// "true" | "false" | "partial" | "unverified"
    #[serde(default = "default_truth_status")]
    pub truth_status: String,
    /// "public" | "local" | "restricted" | "gm_secret"
    #[serde(default = "default_claim_visibility")]
    pub visibility: String,
    #[serde(default)]
    pub origin_source: String,
    #[serde(default)]
    pub source_event_id: Option<i64>,
    /// If superseded by a newer claim ID, actors holding only this claim automatically hold outdated knowledge.
    #[serde(default)]
    pub superseded_by_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub related_ids: Vec<String>,
    #[serde(default)]
    pub created_at_world_time: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_truth_status() -> String {
    "true".to_string()
}

fn default_claim_visibility() -> String {
    "restricted".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorBelief {
    pub claim_id: String,
    pub actor_id: String,
    /// "observed" | "testimony" | "documented" | "rumor" | "inferred" | "suspected" | "disbelieved" | "misinformed" | "concealing"
    #[serde(default = "default_belief_stance")]
    pub stance: String,
    /// 0..=100 confidence score
    #[serde(default = "default_confidence")]
    pub confidence: u8,
    /// What this specific actor actually thinks (supports partial observation or distorted rumor!).
    /// If empty, defaults to the canonical claim text.
    #[serde(default)]
    pub perceived_claim: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub source_trustworthy: Option<bool>,
    #[serde(default)]
    pub concealing: bool,
    #[serde(default)]
    pub outdated: bool,
    #[serde(default)]
    pub learned_at_world_time: String,
    #[serde(default)]
    pub notes: String,
}

fn default_belief_stance() -> String {
    "observed".to_string()
}

fn default_confidence() -> u8 {
    80
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeRecordView {
    pub claim: KnowledgeClaim,
    pub beliefs: Vec<ActorBelief>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KnowledgeSearchFilter {
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub actor_id: Option<String>,
    /// Return claims that this actor does NOT know yet (blind-spot finder!).
    #[serde(default)]
    pub not_known_by: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub stance: Option<String>,
    #[serde(default)]
    pub truth_status: Option<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub min_confidence: Option<u8>,
    #[serde(default)]
    pub include_outdated: Option<bool>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Compact POV belief entry rendered inside perspective-safe scene contexts and autonomy dossiers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorKnowledgeBrief {
    pub claim_id: String,
    pub actor_id: String,
    pub subject: String,
    pub perceived_claim: String,
    pub stance: String,
    pub confidence: u8,
    pub source: String,
    pub concealing: bool,
    pub outdated: bool,
    pub learned_at: String,
}

// ============================================================================
// 3. Multi-Dimensional Hidden Relationships
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct RelationshipDimensions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub respect: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affection: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fear: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resentment: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependence: Option<i64>,
}

impl RelationshipDimensions {
    pub fn is_empty(&self) -> bool {
        self.trust.is_none()
            && self.respect.is_none()
            && self.affection.is_none()
            && self.fear.is_none()
            && self.resentment.is_none()
            && self.dependence.is_none()
    }

    pub fn format_compact(&self) -> String {
        let mut parts = Vec::new();
        if let Some(v) = self.trust {
            parts.push(format!("trust:{v}"));
        }
        if let Some(v) = self.respect {
            parts.push(format!("respect:{v}"));
        }
        if let Some(v) = self.affection {
            parts.push(format!("affection:{v}"));
        }
        if let Some(v) = self.fear {
            parts.push(format!("fear:{v}"));
        }
        if let Some(v) = self.resentment {
            parts.push(format!("resentment:{v}"));
        }
        if let Some(v) = self.dependence {
            parts.push(format!("dependence:{v}"));
        }
        parts.join(", ")
    }
}

// ============================================================================
// 4. First-Class Dedicated Factions Register
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FactionMember {
    pub character_id: String,
    #[serde(default = "default_member_rank")]
    pub rank: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub loyalty_note: String,
}

fn default_member_rank() -> String {
    "Member".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactionRecord {
    // Baseline / Institutional Identity
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// e.g. "bandit-band", "sect", "merchant-guild", "militia", "caravan", "noble-house"
    #[serde(default = "default_faction_type")]
    pub faction_type: String,
    #[serde(default)]
    pub origins: String,
    #[serde(default)]
    pub leadership_model: String,
    #[serde(default)]
    pub ideology: String,
    #[serde(default)]
    pub established_territory: Vec<String>,
    #[serde(default)]
    pub institutional_structure: String,
    /// Realistic operational tier (e.g. "micro-band (5-15)", "local-guild", "regional-sect")
    #[serde(default = "default_capacity_tier")]
    pub capacity_tier: String,
    // Current Evolving State
    #[serde(default)]
    pub leader_id: Option<String>,
    #[serde(default)]
    pub members: Vec<FactionMember>,
    #[serde(default)]
    pub territory: Vec<String>,
    #[serde(default)]
    pub resources: BTreeMap<String, i64>,
    #[serde(default)]
    pub reputation: BTreeMap<String, i64>,
    #[serde(default)]
    pub objectives: Vec<StructuredGoal>,
    #[serde(default)]
    pub internal_conflicts: Vec<String>,
    #[serde(default = "default_morale")]
    pub morale: i64,
    #[serde(default)]
    pub active_schemes: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub created_at_world_time: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_faction_type() -> String {
    "organization".to_string()
}

fn default_capacity_tier() -> String {
    "local".to_string()
}

impl FactionRecord {
    pub fn normalize(mut self) -> Self {
        self.id = if self.id.trim().is_empty() {
            slugify(&self.name)
        } else {
            slugify(&self.id)
        };
        if let Some(ref mut l) = self.leader_id {
            if l.trim().is_empty() {
                self.leader_id = None;
            } else {
                *l = slugify(l);
            }
        }
        self.territory = self
            .territory
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .map(|s| slugify(&s))
            .collect();
        self.established_territory = self
            .established_territory
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .map(|s| slugify(&s))
            .collect();
        for m in &mut self.members {
            m.character_id = slugify(&m.character_id);
        }
        self
    }
}

// ============================================================================
// 5. Two-Phase Constrained Agency Sandbox (Solo NPC, Group & Faction Autonomy)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorAutonomyBrief {
    pub character_id: String,
    pub name: String,
    pub alive_status: String,
    pub location_id: Option<String>,
    pub active_conditions: Vec<String>,
    pub carried_items: Vec<String>,
    pub resources: BTreeMap<String, String>,
    pub risk_tolerance: String,
    pub morale: i64,
    pub stress: i64,
    pub motivations: Vec<String>,
    pub fears: Vec<String>,
    pub current_plans: Vec<String>,
    pub active_goals: Vec<StructuredGoal>,
    /// STRICTLY ONLY knowledge claims this actor actually holds (zero GM secrets!).
    pub known_claims: Vec<ActorKnowledgeBrief>,
    /// Count of active claims in the campaign/region this actor does NOT know (no text leaked!).
    pub blind_spot_count: usize,
    pub personal_tensions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomyDossier {
    pub scope_type: String,
    pub world_time: String,
    pub duration_hours: u64,
    pub location_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faction: Option<FactionRecord>,
    pub actors: Vec<ActorAutonomyBrief>,
    pub reachable_places: Vec<String>,
    pub projected_upkeep: BTreeMap<String, i64>,
    pub institutional_relations: Vec<String>,
    pub surrounding_public_events: Vec<String>,
    pub capacity_constraints: Vec<String>,
    /// Explicit list of claim_ids this scope is legally allowed to act upon during commit.
    pub allowed_claim_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomyActionInput {
    pub actor_id: String,
    pub action: String,
    #[serde(default)]
    pub outcome_summary: String,
    /// Optional claim IDs this action relies on — validated against actor's actual beliefs!
    #[serde(default)]
    pub relied_on_claim_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactionResourceDelta {
    pub resource: String,
    pub delta: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterResourceDelta {
    pub character_id: String,
    pub resource: String,
    pub delta: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalProgressUpdate {
    pub owner_id: String,
    pub goal_id: String,
    #[serde(default)]
    pub progress_delta: Option<i64>,
    #[serde(default)]
    pub set_status: Option<String>,
    #[serde(default)]
    pub notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomyCommitRequest {
    pub scope_type: String,
    #[serde(default)]
    pub faction_id: Option<String>,
    #[serde(default)]
    pub actor_ids: Vec<String>,
    #[serde(default)]
    pub location_id: Option<String>,
    #[serde(default)]
    pub duration_hours: u64,
    #[serde(default)]
    pub actions: Vec<AutonomyActionInput>,
    #[serde(default)]
    pub faction_resource_deltas: Vec<FactionResourceDelta>,
    #[serde(default)]
    pub character_resource_deltas: Vec<CharacterResourceDelta>,
    #[serde(default)]
    pub goal_updates: Vec<GoalProgressUpdate>,
    #[serde(default)]
    pub morale_delta: Option<i64>,
    #[serde(default)]
    pub add_internal_conflicts: Vec<String>,
    #[serde(default)]
    pub resolve_internal_conflicts: Vec<String>,
    #[serde(default)]
    pub allow_debt: bool,
    #[serde(default)]
    pub advance_world_clock: bool,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub causal_event_ids: Vec<i64>,
}

/// Per-dimension difference between a garment measurement and a character's current body measurement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GarmentDimensionDiff {
    pub key: String,
    pub garment_val: String,
    pub body_val: String,
    pub ease_cm: f64,
    pub zone_status: String,
}

/// Complete fit evaluation for a single wearable item against a character's current body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GarmentFitReport {
    pub item_name: String,
    pub cut: String,
    pub can_equip: bool,
    /// "well_fitted" | "snug" | "loose" | "odd_taper" | "baggy" | "tight_restrictive" | "cannot_fit_too_small"
    pub fit_status: String,
    pub dex_penalty: i64,
    pub str_penalty: i64,
    pub cha_penalty: i64,
    pub taper_spread_cm: f64,
    pub dimensions: Vec<GarmentDimensionDiff>,
    pub blocking_reasons: Vec<String>,
    pub summary: String,
}

/// Aggregated apparel fit summary across all currently equipped sized garments on a character.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct EquippedApparelSummary {
    pub dex_penalty: i64,
    pub str_penalty: i64,
    pub cha_penalty: i64,
    /// "none" | "awkward_drape" | "binding_chafing" | "painful_outgrown"
    #[serde(default)]
    pub discomfort_tier: String,
    pub outgrown_items: Vec<String>,
    pub reports: Vec<GarmentFitReport>,
    pub brief: String,
}

fn measure_to_cm(raw: &str) -> Option<f64> {
    let (val, unit) = parse_leading_measure(raw)?;
    let u = unit.to_lowercase();
    if u.is_empty() || u == "cm" {
        Some(val)
    } else if u == "in" || u == "inch" || u == "inches" || u == "\"" {
        Some(val * 2.54)
    } else if u == "m" {
        Some(val * 100.0)
    } else if u == "mm" {
        Some(val / 10.0)
    } else {
        None
    }
}

/// Evaluates how a wearable garment fits a character's current live body dimensions.
/// Returns `None` if the item has no `garment_measurements` or shares no comparable dimensions with the character.
pub fn evaluate_garment_fit(
    item: &crate::models::Item,
    avatar: &AvatarBodyProfile,
) -> Option<GarmentFitReport> {
    if item.garment_measurements.is_empty() {
        return None;
    }
    let cut_l = item.garment_cut.trim().to_lowercase();
    let is_stretch = matches!(
        cut_l.as_str(),
        "stretch" | "knit" | "elastic" | "laced" | "adjustable" | "wrap"
    );
    let is_rigid = matches!(
        cut_l.as_str(),
        "rigid" | "rigid_armor" | "plate" | "armor" | "metal" | "hard_leather"
    );
    let is_relaxed = matches!(cut_l.as_str(), "relaxed" | "loose" | "oversized" | "robe");

    let mut dims = Vec::new();
    let mut blocking_reasons = Vec::new();
    let mut tight_zones = 0usize;
    let mut snug_zones = 0usize;
    let mut loose_zones = 0usize;
    let mut baggy_zones = 0usize;
    let mut extreme_baggy = false;
    let mut length_short = false;
    let mut length_dragging = false;
    let mut min_norm_ease = f64::INFINITY;
    let mut max_norm_ease = f64::NEG_INFINITY;
    let mut taper_count = 0usize;

    // Sort keys in canonical anatomical order
    let mut keys: Vec<String> = item.garment_measurements.keys().cloned().collect();
    keys.sort_by(|a, b| {
        measurement_sort_rank(a)
            .cmp(&measurement_sort_rank(b))
            .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
    });

    for raw_k in keys {
        let g_str = &item.garment_measurements[&raw_k];
        let norm_k = normalize_measurement_key(&raw_k);
        let b_str_opt: Option<&str> = if norm_k.eq_ignore_ascii_case("Height") {
            if !avatar.height.trim().is_empty() {
                Some(avatar.height.as_str())
            } else {
                None
            }
        } else {
            avatar
                .measurements
                .get(&norm_k)
                .or_else(|| avatar.measurements.get(&raw_k))
                .map(String::as_str)
        };
        let Some(b_str) = b_str_opt else {
            continue;
        };
        let (Some(g_cm), Some(b_cm)) = (measure_to_cm(g_str), measure_to_cm(b_str)) else {
            continue;
        };
        let ease = ((g_cm - b_cm) * 10.0).round() / 10.0;

        let is_width = norm_k == "Shoulders";
        let is_length = matches!(norm_k.as_str(), "Inseam" | "Height" | "Arm Span");

        let zone_status = if is_length {
            if ease < -5.0 {
                length_short = true;
                "short"
            } else if ease > 6.5 {
                length_dragging = true;
                "too_long"
            } else {
                "fitted"
            }
        } else if is_width {
            let too_small = if is_rigid {
                -1.5
            } else if is_stretch {
                -4.0
            } else {
                -2.5
            };
            let tight = if is_rigid {
                0.5
            } else if is_stretch {
                -1.5
            } else {
                -0.5
            };
            let loose = if is_relaxed { 5.5 } else { 3.5 };
            let baggy = if is_relaxed { 8.5 } else { 6.0 };

            let norm_ease = ease * 2.0; // normalize shoulder width to circumference scale for taper spread
            min_norm_ease = min_norm_ease.min(norm_ease);
            max_norm_ease = max_norm_ease.max(norm_ease);
            taper_count += 1;

            if ease < too_small {
                blocking_reasons.push(format!(
                    "{norm_k} too small (garment {g_str} vs body {b_str}, {} cm)",
                    format_compact_number(ease)
                ));
                "too_small"
            } else if ease < tight {
                tight_zones += 1;
                "tight"
            } else if ease < 1.5 {
                snug_zones += 1;
                "snug"
            } else if ease > baggy {
                baggy_zones += 1;
                if ease > baggy + 4.0 {
                    extreme_baggy = true;
                }
                "baggy"
            } else if ease > loose {
                loose_zones += 1;
                "loose"
            } else {
                "fitted"
            }
        } else {
            // Circumference keys: Chest, Underbust, Waist, Hips, Neck, Upper Arm, Forearm, Thigh, Calf
            let too_small = if is_rigid {
                -2.0
            } else if is_stretch {
                -7.0
            } else {
                -4.0
            };
            let tight = if is_rigid {
                1.0
            } else if is_stretch {
                -3.0
            } else {
                -1.0
            };
            let loose = if is_relaxed { 13.0 } else { 9.0 };
            let baggy = if is_relaxed { 19.0 } else { 14.0 };

            if matches!(
                norm_k.as_str(),
                "Chest" | "Underbust" | "Waist" | "Hips" | "Thigh"
            ) {
                min_norm_ease = min_norm_ease.min(ease);
                max_norm_ease = max_norm_ease.max(ease);
                taper_count += 1;
            }

            if ease < too_small {
                blocking_reasons.push(format!(
                    "{norm_k} too small (garment {g_str} vs body {b_str}, {} cm)",
                    format_compact_number(ease)
                ));
                "too_small"
            } else if ease < tight {
                tight_zones += 1;
                "tight"
            } else if ease < 2.0 {
                snug_zones += 1;
                "snug"
            } else if ease > baggy {
                baggy_zones += 1;
                if ease > baggy + 8.0 {
                    extreme_baggy = true;
                }
                "baggy"
            } else if ease > loose {
                loose_zones += 1;
                "loose"
            } else {
                "fitted"
            }
        };

        dims.push(GarmentDimensionDiff {
            key: norm_k,
            garment_val: g_str.clone(),
            body_val: b_str.to_string(),
            ease_cm: ease,
            zone_status: zone_status.to_string(),
        });
    }

    if dims.is_empty() {
        return None;
    }

    let taper_spread_cm = if taper_count >= 2 && max_norm_ease > min_norm_ease {
        ((max_norm_ease - min_norm_ease) * 10.0).round() / 10.0
    } else {
        0.0
    };
    let is_odd_taper = taper_spread_cm >= 10.0 && min_norm_ease < 12.0;
    let can_equip = blocking_reasons.is_empty();

    let mut dex_penalty: i64 = 0;
    let mut str_penalty: i64 = 0;
    let mut cha_penalty: i64 = 0;

    if !can_equip {
        // Already worn garment outgrown past wearable limit!
        dex_penalty = -2;
        str_penalty = -2;
        cha_penalty = -2;
    } else {
        if tight_zones > 0 {
            dex_penalty = dex_penalty.min(if tight_zones >= 2 || is_rigid { -2 } else { -1 });
            str_penalty = str_penalty.min(-1);
            cha_penalty = cha_penalty.min(-1);
        }
        if baggy_zones > 0 {
            dex_penalty = dex_penalty.min(if extreme_baggy { -2 } else { -1 });
            cha_penalty = cha_penalty.min(if extreme_baggy { -2 } else { -1 });
        }
        if is_odd_taper {
            cha_penalty = cha_penalty.min(if taper_spread_cm >= 16.0 { -2 } else { -1 });
            if taper_spread_cm >= 14.0 && min_norm_ease < 1.0 {
                dex_penalty = dex_penalty.min(-1);
            }
        }
        if length_short {
            cha_penalty = cha_penalty.min(-1);
        }
        if length_dragging {
            dex_penalty = dex_penalty.min(-1);
            cha_penalty = cha_penalty.min(-1);
        }
    }

    let fit_status = if !can_equip {
        "cannot_fit_too_small"
    } else if tight_zones > 0 {
        "tight_restrictive"
    } else if baggy_zones > 0 && is_odd_taper {
        "baggy_odd_taper"
    } else if baggy_zones > 0 {
        "baggy"
    } else if is_odd_taper {
        "odd_taper"
    } else if loose_zones > 0 {
        "loose"
    } else if snug_zones > 0 {
        "snug"
    } else {
        "well_fitted"
    }
    .to_string();

    let notable: Vec<String> = dims
        .iter()
        .filter(|d| d.zone_status != "fitted")
        .map(|d| {
            let sign = if d.ease_cm > 0.0 { "+" } else { "" };
            format!(
                "{} {}{} cm ({})",
                d.key,
                sign,
                format_compact_number(d.ease_cm),
                d.zone_status
            )
        })
        .collect();
    let mut pen_parts = Vec::new();
    if dex_penalty < 0 {
        pen_parts.push(format!("DEX {dex_penalty}"));
    }
    if str_penalty < 0 {
        pen_parts.push(format!("STR {str_penalty}"));
    }
    if cha_penalty < 0 {
        pen_parts.push(format!("CHA {cha_penalty}"));
    }
    let summary = if !can_equip {
        format!(
            "TOO SMALL ({}; penalties if forced/outgrown: DEX -2, STR -2, CHA -2)",
            blocking_reasons.join("; ")
        )
    } else if notable.is_empty() && pen_parts.is_empty() {
        "well-fitted".to_string()
    } else {
        let mut s = format!("{fit_status}");
        if !notable.is_empty() {
            s.push_str(&format!(": {}", notable.join(", ")));
        }
        if !pen_parts.is_empty() {
            s.push_str(&format!(" -> {}", pen_parts.join(", ")));
        }
        s
    };

    Some(GarmentFitReport {
        item_name: item.name.clone(),
        cut: if item.garment_cut.is_empty() {
            "standard".to_string()
        } else {
            item.garment_cut.clone()
        },
        can_equip,
        fit_status,
        dex_penalty,
        str_penalty,
        cha_penalty,
        taper_spread_cm,
        dimensions: dims,
        blocking_reasons,
        summary,
    })
}

/// Evaluates all currently equipped garments with dimensional sizing on a character and aggregates
/// bounded mechanical penalties (worst penalty per stat, capped at -3) plus a concise summary string.
pub fn evaluate_equipped_apparel_fit(
    equipment: &[crate::models::Item],
    avatar: &AvatarBodyProfile,
) -> EquippedApparelSummary {
    let mut out = EquippedApparelSummary {
        discomfort_tier: "none".to_string(),
        ..Default::default()
    };
    let mut dex_hits = 0usize;
    let mut str_hits = 0usize;
    let mut cha_hits = 0usize;
    let mut has_tight = false;
    let mut has_awkward = false;
    let mut parts = Vec::new();

    for it in equipment.iter().filter(|i| i.equipped) {
        if let Some(rep) = evaluate_garment_fit(it, avatar) {
            if !rep.can_equip {
                out.outgrown_items.push(rep.item_name.clone());
            } else if rep.fit_status == "tight_restrictive" {
                has_tight = true;
            } else if matches!(
                rep.fit_status.as_str(),
                "odd_taper" | "baggy" | "baggy_odd_taper"
            ) {
                has_awkward = true;
            }
            if rep.dex_penalty < 0 {
                dex_hits += 1;
                out.dex_penalty = out.dex_penalty.min(rep.dex_penalty);
            }
            if rep.str_penalty < 0 {
                str_hits += 1;
                out.str_penalty = out.str_penalty.min(rep.str_penalty);
            }
            if rep.cha_penalty < 0 {
                cha_hits += 1;
                out.cha_penalty = out.cha_penalty.min(rep.cha_penalty);
            }
            if rep.fit_status != "well_fitted" {
                parts.push(format!("{} [{}]", rep.item_name, rep.summary));
            }
            out.reports.push(rep);
        }
    }
    out.discomfort_tier = if !out.outgrown_items.is_empty() {
        "painful_outgrown".to_string()
    } else if has_tight {
        "binding_chafing".to_string()
    } else if has_awkward {
        "awkward_drape".to_string()
    } else {
        "none".to_string()
    };
    // If multiple separate worn layers are ill-fitting on the same axis, add -1 cumulative drag (capped at -3)
    if dex_hits >= 2 {
        out.dex_penalty = (out.dex_penalty - 1).max(-3);
    }
    if str_hits >= 2 {
        out.str_penalty = (out.str_penalty - 1).max(-3);
    }
    if cha_hits >= 2 {
        out.cha_penalty = (out.cha_penalty - 1).max(-3);
    }
    if !parts.is_empty() {
        let mut tot = Vec::new();
        if out.dex_penalty < 0 {
            tot.push(format!("DEX {}", out.dex_penalty));
        }
        if out.str_penalty < 0 {
            tot.push(format!("STR {}", out.str_penalty));
        }
        if out.cha_penalty < 0 {
            tot.push(format!("CHA {}", out.cha_penalty));
        }
        let pen_suffix = if tot.is_empty() {
            String::new()
        } else {
            format!(" | Active Apparel Penalties: {}", tot.join(", "))
        };
        out.brief = format!("{}{pen_suffix}", parts.join(" ; "));
    }
    out
}

#[cfg(test)]
mod avatar_tests {
    use super::*;

    #[test]
    fn test_measurement_key_normalization_and_delta() {
        assert_eq!(normalize_measurement_key("upper_arm"), "Upper Arm");
        assert_eq!(normalize_measurement_key("UpperArm"), "Upper Arm");
        assert_eq!(normalize_measurement_key("shoulders"), "Shoulders");
        assert_eq!(normalize_measurement_key("Chest / Bust"), "Chest");
        assert_eq!(normalize_measurement_key("hips"), "Hips");
        assert_eq!(normalize_measurement_key("Wing Span"), "Wing Span");

        assert_eq!(compute_measurement_delta("91 cm", "88 cm").as_deref(), Some("-3 cm"));
        assert_eq!(compute_measurement_delta("~104 cm", "105 cm").as_deref(), Some("+1 cm"));
        assert_eq!(compute_measurement_delta("49 cm", "49 cm").as_deref(), Some("—"));
        assert_eq!(compute_measurement_delta("82 kg", "80.5 kg").as_deref(), Some("-1.5 kg"));
        assert_eq!(compute_measurement_delta("178 cm", "70 in"), None);
    }

    #[test]
    fn test_dual_view_observable_vs_delta_brief() {
        let mut av = AvatarBodyProfile {
            appearance_stage: "Still Water Layer 5/9 — Low-Gradient Flow".to_string(),
            avatar_summary: "Broad-shouldered craftsman with steady calloused hands and cool, un-flushed olive skin.".to_string(),
            height: "178 cm".to_string(),
            weight: "82 kg".to_string(),
            baseline_height: "178 cm".to_string(),
            baseline_weight: "82 kg".to_string(),
            clothing_fit: "Hemp jacket hangs loose at the waist and sits slightly snug across the hips.".to_string(),
            ..Default::default()
        };
        av.baseline_measurements.insert("Shoulders".into(), "49 cm".into());
        av.baseline_measurements.insert("Chest".into(), "110 cm".into());
        av.baseline_measurements.insert("Waist".into(), "91 cm".into());
        av.baseline_measurements.insert("Hips".into(), "104 cm".into());

        av.measurements.insert("Shoulders".into(), "49 cm".into());
        av.measurements.insert("Chest".into(), "108 cm".into());
        av.measurements.insert("Waist".into(), "88 cm".into());
        av.measurements.insert("Hips".into(), "105 cm".into());

        let obs = av.format_observable_brief();
        assert!(obs.contains("178 cm, 82 kg"));
        assert!(obs.contains("Shoulders 49 cm / Chest 108 cm / Waist 88 cm / Hips 105 cm"));
        assert!(obs.contains("Fit: Hemp jacket"));
        assert!(!obs.contains("91 cm"), "Observable brief must never leak baseline numbers");
        assert!(!obs.contains("Layer 5/9"), "Observable brief must stay stranger's-eye view");

        let delta = av.format_delta_brief();
        assert!(delta.contains("Stage: Still Water Layer 5/9 — Low-Gradient Flow"));
        assert!(delta.contains("Chest 110 cm→108 cm (-2 cm)"));
        assert!(delta.contains("Waist 91 cm→88 cm (-3 cm)"));
        assert!(delta.contains("Hips 104 cm→105 cm (+1 cm)"));
        assert!(!delta.contains("Shoulders"), "Unchanged measurements should be omitted from compact delta");
    }

    #[test]
    fn test_garment_fit_blocking_odd_taper_baggy_and_penalties() {
        let mut av = AvatarBodyProfile {
            height: "178 cm".to_string(),
            ..Default::default()
        };
        av.measurements.insert("Shoulders".into(), "47.5 cm".into());
        av.measurements.insert("Chest".into(), "103 cm".into());
        av.measurements.insert("Waist".into(), "84 cm".into());
        av.measurements.insert("Hips".into(), "105.5 cm".into());

        // 1. Too small at Hips -> MUST block equipping!
        let mut tiny = crate::models::Item {
            name: "Narrow Youth Trousers".into(),
            equipped: false,
            garment_cut: "fitted".into(),
            ..Default::default()
        };
        tiny.garment_measurements.insert("Waist".into(), "86 cm".into());
        tiny.garment_measurements.insert("Hips".into(), "96 cm".into());
        let rep_tiny = evaluate_garment_fit(&tiny, &av).unwrap();
        assert!(!rep_tiny.can_equip);
        assert_eq!(rep_tiny.fit_status, "cannot_fit_too_small");
        assert!(rep_tiny.blocking_reasons[0].contains("Hips"));

        // 2. Odd taper (Day-1 jacket: Chest +11, Waist +14, Hips +0.5 -> spread 13.5 cm => odd_taper CHA -1!)
        let mut day1_jacket = crate::models::Item {
            name: "Day-1 Hemp Jacket".into(),
            equipped: true,
            garment_cut: "standard".into(),
            ..Default::default()
        };
        day1_jacket.garment_measurements.insert("Shoulders".into(), "50 cm".into());
        day1_jacket.garment_measurements.insert("Chest".into(), "114 cm".into());
        day1_jacket.garment_measurements.insert("Waist".into(), "97 cm".into());
        day1_jacket.garment_measurements.insert("Hips".into(), "106 cm".into());
        let rep_odd = evaluate_garment_fit(&day1_jacket, &av).unwrap();
        assert!(rep_odd.can_equip);
        assert_eq!(rep_odd.fit_status, "odd_taper");
        assert!(rep_odd.cha_penalty <= -1);

        // 3. Baggy oversized coat -> DEX & CHA penalties!
        let mut huge_coat = crate::models::Item {
            name: "Giant Cloak-Coat".into(),
            equipped: true,
            garment_cut: "standard".into(),
            ..Default::default()
        };
        huge_coat.garment_measurements.insert("Shoulders".into(), "57 cm".into());
        huge_coat.garment_measurements.insert("Chest".into(), "130 cm".into());
        huge_coat.garment_measurements.insert("Waist".into(), "115 cm".into());
        let rep_bag = evaluate_garment_fit(&huge_coat, &av).unwrap();
        assert!(rep_bag.can_equip);
        assert_eq!(rep_bag.fit_status, "baggy");
        assert!(rep_bag.dex_penalty <= -1);
        assert!(rep_bag.cha_penalty <= -1);

        // 4. Well-tailored Layer 6 coat -> zero penalties!
        let mut tailored = crate::models::Item {
            name: "Tailored Qinghe Jacket".into(),
            equipped: true,
            garment_cut: "tailored".into(),
            ..Default::default()
        };
        tailored.garment_measurements.insert("Shoulders".into(), "49.5 cm".into());
        tailored.garment_measurements.insert("Chest".into(), "108 cm".into());
        tailored.garment_measurements.insert("Waist".into(), "90 cm".into());
        tailored.garment_measurements.insert("Hips".into(), "110 cm".into());
        let rep_fit = evaluate_garment_fit(&tailored, &av).unwrap();
        assert!(rep_fit.can_equip);
        assert_eq!(rep_fit.fit_status, "well_fitted");
        assert_eq!(rep_fit.dex_penalty, 0);
        assert_eq!(rep_fit.str_penalty, 0);
        assert_eq!(rep_fit.cha_penalty, 0);
    }
}
