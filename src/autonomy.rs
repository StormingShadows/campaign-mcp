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
