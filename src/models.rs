use crate::autonomy::RelationshipDimensions;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// In-world calendar and time tracker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorldClock {
    pub era: String,
    pub year: i32,
    pub season: String,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    /// Ordered list of seasons in this world (default: Spring, Summer, Autumn, Winter).
    #[serde(default = "default_seasons")]
    pub seasons: Vec<String>,
    /// Days per season (default: 90).
    #[serde(default = "default_days_per_season")]
    pub days_per_season: u32,
    /// Global weather or atmospheric note.
    #[serde(default)]
    pub global_weather: String,
}

pub fn default_seasons() -> Vec<String> {
    vec![
        "Spring".to_string(),
        "Summer".to_string(),
        "Autumn".to_string(),
        "Winter".to_string(),
    ]
}

pub fn default_days_per_season() -> u32 {
    90
}

impl Default for WorldClock {
    fn default() -> Self {
        Self {
            era: "First Age".to_string(),
            year: 1,
            season: "Spring".to_string(),
            day: 1,
            hour: 8,
            minute: 0,
            seasons: default_seasons(),
            days_per_season: default_days_per_season(),
            global_weather: "Clear".to_string(),
        }
    }
}

impl WorldClock {
    pub fn format_short(&self) -> String {
        format!(
            "{} Y{}, {} Day {}, {:02}:{:02}",
            self.era, self.year, self.season, self.day, self.hour, self.minute
        )
    }

    pub fn time_of_day(&self) -> &'static str {
        match self.hour {
            5..=7 => "Dawn",
            8..=11 => "Morning",
            12..=14 => "Midday",
            15..=17 => "Afternoon",
            18..=20 => "Dusk",
            21..=23 => "Night",
            _ => "Deep Night",
        }
    }

    /// Advances the world clock by `minutes` and returns `true` if the season changed.
    pub fn advance(&mut self, minutes: u64) -> bool {
        if self.seasons.is_empty() {
            self.seasons = default_seasons();
        }
        let dps = self.days_per_season.max(1) as u64;
        let old_season = self.season.clone();

        let total_minutes = self.minute as u64 + minutes;
        self.minute = (total_minutes % 60) as u32;
        let add_hours = total_minutes / 60;

        let total_hours = self.hour as u64 + add_hours;
        self.hour = (total_hours % 24) as u32;
        let add_days = total_hours / 24;

        let zero_based_day = (self.day.saturating_sub(1)) as u64 + add_days;
        let season_steps = zero_based_day / dps;
        self.day = (zero_based_day % dps) as u32 + 1;

        if season_steps > 0 {
            let current_idx = self
                .seasons
                .iter()
                .position(|s| s.eq_ignore_ascii_case(&self.season))
                .unwrap_or(0) as u64;
            let total_seasons = current_idx + season_steps;
            let num_seasons = self.seasons.len() as u64;
            let add_years = total_seasons / num_seasons;
            let new_idx = (total_seasons % num_seasons) as usize;
            self.season = self.seasons[new_idx].clone();
            self.year += add_years as i32;
        }

        self.season != old_season
    }

    /// Deterministic total elapsed in-world hours for scheduling trips, ETAs, and visit windows.
    pub fn total_hours(&self) -> i64 {
        let num_seasons = self.seasons.len().max(1) as i64;
        let dps = self.days_per_season.max(1) as i64;
        let season_idx = self
            .seasons
            .iter()
            .position(|s| s.eq_ignore_ascii_case(&self.season))
            .unwrap_or(0) as i64;
        let year_days = (self.year as i64) * num_seasons * dps;
        let day_offset = season_idx * dps + (self.day.saturating_sub(1) as i64);
        (year_days + day_offset) * 24 + (self.hour as i64)
    }

    /// Returns a cloned clock advanced by `add_hours` hours (for ETA formatting without mutating self).
    pub fn clock_after_hours(&self, add_hours: u64) -> Self {
        let mut c = self.clone();
        c.advance(add_hours.saturating_mul(60));
        c
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CampaignMeta {
    pub id: String,
    pub name: String,
    pub description: String,
    pub ruleset_notes: String,
    pub clock: WorldClock,
    pub current_session: u32,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourcePool {
    pub current: i64,
    pub max: i64,
    #[serde(default)]
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    #[serde(default)]
    pub slot: Option<String>,
    #[serde(default = "default_qty")]
    pub quantity: i64,
    #[serde(default)]
    pub equipped: bool,
    #[serde(default)]
    pub durability: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub description: String,
    /// Optional dimensional measurements for wearable garments/armor (e.g. Shoulders, Chest, Waist, Hips, Inseam -> "49 cm", "110 cm", "91 cm", "104 cm").
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub garment_measurements: BTreeMap<String, String>,
    /// Optional garment cut / stretch profile: "fitted" | "tailored" | "standard" | "relaxed" | "loose" | "stretch" | "rigid_armor".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub garment_cut: String,
    /// Optional garment taper / silhouette profile: "straight" | "boxy" | "athletic_v" | "tailored_waist" | "hourglass" | "a_line" | "relaxed_drape".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub garment_taper: String,
}

/// Canonical genre-agnostic primary category tags recognized for inventory organization.
pub const CANONICAL_ITEM_CATEGORY_TAGS: &[&str] = &[
    "clothing",
    "armor",
    "weapon",
    "tool",
    "book",
    "document",
    "cultivation",
    "magic",
    "tech",
    "consumable",
    "medicine",
    "food",
    "provisions",
    "camping",
    "travel",
    "artifact",
    "keepsake",
    "key",
    "currency",
    "quest",
    "gear",
];

/// Normalizes a single inventory/catalog tag into clean lowercase kebab-case.
pub fn normalize_item_tag(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in raw.trim().to_lowercase().chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
            prev_dash = false;
        } else if matches!(ch, '-' | '_' | ' ' | '/' | ':') {
            if !out.is_empty() && !prev_dash {
                out.push('-');
                prev_dash = true;
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Normalizes and deduplicates a list of item tags while preserving order.
pub fn normalize_item_tags(tags: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for raw in tags {
        let norm = normalize_item_tag(raw);
        if !norm.is_empty() && !out.iter().any(|e: &String| e == &norm) {
            out.push(norm);
        }
    }
    out
}

/// Returns true if the tag list already contains at least one primary category tag.
pub fn has_primary_item_category_tag(tags: &[String]) -> bool {
    tags.iter().any(|t| {
        let n = normalize_item_tag(t);
        CANONICAL_ITEM_CATEGORY_TAGS
            .iter()
            .any(|c| *c != "gear" && *c == n)
    })
}

/// Deterministically infers clean genre-agnostic category tags from an item's name, slot, type, description, and garment sizing.
pub fn infer_item_tags(
    name: &str,
    slot: Option<&str>,
    item_type: Option<&str>,
    description: &str,
    has_garment_sizing: bool,
) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    let mut push = |t: &str| {
        let n = normalize_item_tag(t);
        if !n.is_empty() && !tags.iter().any(|e| e == &n) {
            tags.push(n);
        }
    };

    let nl = name.to_lowercase();
    let sl = slot.unwrap_or("").to_lowercase();
    let tl = item_type.unwrap_or("").to_lowercase();
    let dl = description.to_lowercase();
    let hay = format!("{nl} {sl} {dl}");
    let has_kw = |text: &str, kw: &str| -> bool {
        if kw.contains(' ') {
            return text.contains(kw);
        }
        text.split(|c: char| !c.is_alphanumeric() && c != '-')
            .any(|w| w == kw || w.strip_suffix('s') == Some(kw) || w.strip_suffix("es") == Some(kw))
    };

    if has_garment_sizing {
        push("clothing");
        push("sized-apparel");
    }

    match tl.as_str() {
        "clothing" | "apparel" | "garment" | "outfit" => push("clothing"),
        "armor" | "armour" => push("armor"),
        "weapon" => push("weapon"),
        "tool" => push("tool"),
        "book" | "manual" | "scroll" => push("book"),
        "record" | "document" => push("document"),
        "artifact" | "relic" => push("artifact"),
        "keepsake" => push("keepsake"),
        "consumable" => push("consumable"),
        "key" => push("key"),
        _ => {}
    }

    if matches!(
        sl.as_str(),
        "outerwear" | "shirt" | "trousers" | "pants" | "footwear" | "headwear" | "legwear" | "waist" | "outfit" | "cloak" | "boots" | "gloves"
    ) || [
        "jacket", "tunic", "under-tunic", "shirt", "trouser", "pants", "cloak", "robe", "coat", "dress", "skirt",
        "boot", "shoe", "belt", "leg-wrap", "glove", "hat", "cap", "scarf",
        "clothing", "attire", "garment", "outfit", "vest", "sash", "mantle", "headwrap",
    ]
    .iter()
    .any(|k| has_kw(&nl, k))
    {
        push("clothing");
    }

    if ["armor", "armour", "mail", "breastplate", "brigandine", "helmet", "helm", "shield", "greaves", "gauntlet"]
        .iter()
        .any(|k| has_kw(&nl, k) || has_kw(&sl, k))
    {
        push("armor");
    }

    if ["sword", "saber", "sabre", "blade", "dagger", "spear", "halberd", "bow", "crossbow", "arrow", "mace", "staff", "pistol", "rifle", "blaster", "club", "whip"]
        .iter()
        .any(|k| has_kw(&nl, k))
    {
        push("weapon");
    }

    if ["hatchet", "axe", "knife", "chisel", "hammer", "saw", "flint", "flint-and-steel", "fire kit", "tinder", "kit", "lockpick", "wrench", "compass", "needle", "engraving", "pliers", "shovel", "crowbar", "tool"]
        .iter()
        .any(|k| has_kw(&nl, k))
    {
        push("tool");
    }

    if ["book", "handbook", "manual", "scroll", "tome", "grimoire", "journal", "diary", "codex", "scripture", "volume"]
        .iter()
        .any(|k| has_kw(&nl, k))
    {
        push("book");
    }

    if ["letter", "vouch", "voucher", "charter", "deed", "archive", "ledger", "map", "contract", "permit", "pass", "record", "edict", "warrant", "certificate"]
        .iter()
        .any(|k| has_kw(&nl, k))
    {
        push("document");
    }

    if ["jade disk", "jade disc", "jade slip", "talisman", "spirit stone", "pill", "elixir", "formation", "cultivation"]
        .iter()
        .any(|k| has_kw(&hay, k))
    {
        push("cultivation");
    } else if ["wand", "rune", "enchanted", "spell", "grimoire", "amulet", "arcane"]
        .iter()
        .any(|k| has_kw(&nl, k))
    {
        push("magic");
    } else if ["cyberdeck", "scanner", "datapad", "medkit", "implant", "holo", "battery", "cell"]
        .iter()
        .any(|k| has_kw(&nl, k))
    {
        push("tech");
    }

    if ["provision", "ration", "food", "bread", "rice", "tea", "wine", "herb", "salve", "potion", "bandage", "ammo", "ammunition", "oil", "candle"]
        .iter()
        .any(|k| has_kw(&nl, k))
    {
        push("consumable");
        if ["provision", "ration", "food", "bread", "rice"].iter().any(|k| has_kw(&nl, k)) {
            push("provisions");
        }
    }

    if ["bedroll", "blanket", "tent", "cord", "rope", "flask", "waterskin", "canteen", "lantern", "torch", "satchel", "backpack", "pack"]
        .iter()
        .any(|k| has_kw(&nl, k))
    {
        push("camping");
    }

    if ["coin pouch", "purse", "money", "bill of exchange", "draft"].iter().any(|k| has_kw(&nl, k)) {
        push("currency");
    }

    if ["seal", "signet", "relic", "heirloom", "keepsake", "ring", "pendant", "locket", "medallion"]
        .iter()
        .any(|k| has_kw(&nl, k))
    {
        push("artifact");
    }

    if has_kw(&nl, "spare") {
        push("spare");
    }

    drop(push);
    if tags.is_empty() {
        tags.push("gear".to_string());
    }
    tags
}

/// Returns a clean human-readable primary group label for organizing long inventories.
pub fn primary_item_group(tags: &[String], slot: Option<&str>, name: &str) -> String {
    let norm = normalize_item_tags(tags);
    let check = |t: &str| norm.iter().any(|x| x == t);
    if check("clothing") {
        return "Clothing & Apparel".to_string();
    }
    if check("armor") {
        return "Armor & Protection".to_string();
    }
    if check("weapon") {
        return "Weapons".to_string();
    }
    if check("tool") || check("tools") || check("craft") || check("carpentry") {
        return "Tools & Craft".to_string();
    }
    if check("book") || check("document") {
        return "Books & Documents".to_string();
    }
    if check("cultivation") || check("magic") || check("tech") {
        return "Cultivation, Magic & Tech".to_string();
    }
    if check("artifact") || check("keepsake") || check("key") || check("quest") {
        return "Artifacts & Keepsakes".to_string();
    }
    if check("medicine") || check("medical") || check("healing") {
        return "Medicine & Healing".to_string();
    }
    if check("consumable") || check("provisions") || check("food") || check("ration") || check("seasoning") || check("drink") {
        return "Food, Provisions & Consumables".to_string();
    }
    if check("camping") || check("travel") || check("travel-supplies") {
        return "Camping & Travel".to_string();
    }
    if check("currency") {
        return "Currency & Pouches".to_string();
    }
    let inferred = infer_item_tags(name, slot, None, "", false);
    if inferred.first().map(String::as_str) != Some("gear") {
        return primary_item_group(&inferred, slot, "");
    }
    "General Gear".to_string()
}

fn default_qty() -> i64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    #[serde(default = "default_rank")]
    pub rank: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub modifier: Option<String>,
    #[serde(default)]
    pub description: String,
    /// Optional governing ability/stat (e.g. "Dexterity", "Wisdom"). Auto-resolved from RulesetProfile.skill_table if omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governing_stat: Option<String>,
    /// Accumulated practice/activity XP driving automatic proficiency & rank progression.
    #[serde(default)]
    pub xp: u64,
    /// Optional explicit proficiency tier (e.g. "untrained", "proficient", "expertise").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proficiency_tier: Option<String>,
}

fn default_rank() -> String {
    "Novice".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Technique {
    pub name: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub cost: String,
    #[serde(default)]
    pub cooldown: String,
    #[serde(default)]
    pub mastery: String,
    #[serde(default)]
    pub effect: String,
    #[serde(default)]
    pub description: String,
    /// Optional linked StoryText ID (e.g. cultivation manual, scroll, grimoire) teaching this technique.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_text_id: Option<String>,
    /// Optional linked CatalogItem ID for a physical manual/artifact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_item_id: Option<String>,
    /// Optional linked parallel progression track ID (e.g. cultivation manual track).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progression_track_id: Option<String>,
    /// Auto-resolved human-readable title of the linked manual/book.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manual_title: String,
    /// Character/GM notes specific to cultivating or practicing this manual technique (breathing cadence, meridian quirks, incompatibilities, comprehension insights).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manual_notes: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatusCondition {
    pub name: String,
    /// e.g. "buff", "debuff", "injury", "curse", "condition"
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

/// Immutable initial/baseline record for a character (Section 1 of the character document).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterBaseline {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub title: String,
    /// e.g. "pc", "npc", "companion", "antagonist"
    #[serde(default = "default_char_kind")]
    pub kind: String,
    #[serde(default)]
    pub origin: String,
    #[serde(default)]
    pub archetype: String,
    #[serde(default)]
    pub faction: String,
    #[serde(default)]
    pub appearance: String,
    #[serde(default)]
    pub personality: String,
    #[serde(default)]
    pub backstory: String,
    #[serde(default)]
    pub initial_location: Option<String>,
    #[serde(default)]
    pub initial_stats: BTreeMap<String, i64>,
    #[serde(default)]
    pub initial_resources: BTreeMap<String, ResourcePool>,
    #[serde(default)]
    pub initial_equipment: Vec<Item>,
    #[serde(default)]
    pub initial_skills: Vec<Skill>,
    #[serde(default)]
    pub initial_techniques: Vec<Technique>,
    #[serde(default)]
    pub initial_goals: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub created_at_world_time: String,
}

fn default_char_kind() -> String {
    "npc".to_string()
}

/// Deterministically projected live character state (Section 2 of the character document).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterCurrent {
    pub id: String,
    pub name: String,
    pub alive_status: String,
    pub current_location: Option<String>,
    pub stats: BTreeMap<String, i64>,
    pub resources: BTreeMap<String, ResourcePool>,
    pub conditions: Vec<StatusCondition>,
    pub equipment: Vec<Item>,
    pub skills: Vec<Skill>,
    pub techniques: Vec<Technique>,
    pub active_goals: Vec<String>,
    pub completed_goals: Vec<String>,
    pub last_event_seq: i64,
    pub last_world_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterProjection {
    pub baseline: CharacterBaseline,
    pub current: CharacterCurrent,
    pub event_log: Vec<WorldEvent>,
}

/// Landmark / place of significance within a Place.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Landmark {
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default = "default_landmark_status")]
    pub status: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_landmark_status() -> String {
    "Intact".to_string()
}

/// Connected route / exit between two places.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Route {
    pub destination_id: String,
    #[serde(default)]
    pub travel_time: String,
    #[serde(default)]
    pub terrain_difficulty: String,
    #[serde(default = "default_route_status")]
    pub status: String,
    #[serde(default)]
    pub notes: String,
}

fn default_route_status() -> String {
    "Open".to_string()
}

/// Immutable initial/baseline record for a Place (Section 1 of the place document).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceBaseline {
    pub id: String,
    pub name: String,
    /// e.g. "region", "settlement", "district", "dungeon", "wilderness", "building"
    #[serde(default = "default_place_type")]
    pub place_type: String,
    #[serde(default)]
    pub parent_place: Option<String>,
    #[serde(default)]
    pub terrain: String,
    #[serde(default)]
    pub climate: String,
    /// Map of Season name -> seasonal effects/description (Spring, Summer, Autumn, Winter, etc.)
    #[serde(default)]
    pub seasonal_profiles: BTreeMap<String, String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub initial_weather: String,
    #[serde(default)]
    pub initial_faction: Option<String>,
    #[serde(default)]
    pub initial_danger_level: String,
    #[serde(default)]
    pub places_of_significance: Vec<Landmark>,
    #[serde(default)]
    pub routes: Vec<Route>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub created_at_world_time: String,
}

fn default_place_type() -> String {
    "location".to_string()
}

/// Deterministically projected live place state (Section 2 of the place document).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceCurrent {
    pub id: String,
    pub name: String,
    pub current_season: String,
    pub current_weather: String,
    pub controlling_faction: Option<String>,
    pub danger_level: String,
    pub active_conditions: Vec<String>,
    pub landmarks: Vec<Landmark>,
    pub routes: Vec<Route>,
    pub present_characters: Vec<String>,
    pub last_event_seq: i64,
    pub last_world_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceProjection {
    pub baseline: PlaceBaseline,
    pub current: PlaceCurrent,
    pub event_log: Vec<WorldEvent>,
}

/// Separate World-Building & Lore entry (outside characters & places).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoreEntry {
    pub id: String,
    pub title: String,
    /// e.g. "world", "history", "magic-system", "factions", "bestiary", "artifacts", "culture", "cosmology", "rules"
    pub category: String,
    #[serde(default)]
    pub summary: String,
    pub body: String,
    /// Keywords that auto-trigger this lore entry during `get_scene_context`.
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// "public" | "rumor" | "gm_secret"
    #[serde(default = "default_visibility")]
    pub visibility: String,
    /// If true, always whispered into scene context (use sparingly for core world laws).
    #[serde(default)]
    pub always_include: bool,
    /// Slugs of related characters, places, or other lore entries.
    #[serde(default)]
    pub related_ids: Vec<String>,
    #[serde(default)]
    pub created_at_world_time: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_visibility() -> String {
    "public".to_string()
}

/// Specialized in-world Book, Scroll, Grimoire, Ritual Manual, Tablet, Journal, or Letter
/// that characters can discover, carry, read, and decipher to understand history or spell rituals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryText {
    pub id: String,
    pub title: String,
    /// e.g. "book", "scroll", "grimoire", "tablet", "journal", "letter", "ritual-manual", "inscription"
    #[serde(default = "default_text_medium")]
    pub medium: String,
    /// e.g. "history", "spell-ritual", "prophecy", "technique-manual", "alchemy", "law", "correspondence"
    #[serde(default = "default_text_subject")]
    pub subject: String,
    #[serde(default)]
    pub author: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default = "default_text_condition")]
    pub condition: String,
    /// "undiscovered" | "discovered_unread" | "partially_read" | "read" | "deciphered"
    #[serde(default = "default_read_status")]
    pub read_status: String,
    /// Character IDs who have read/studied this text.
    #[serde(default)]
    pub read_by: Vec<String>,
    /// Current physical location (place_id) or carrier (character_id).
    #[serde(default)]
    pub holder_or_location_id: Option<String>,
    /// Outward cover/appearance overview (safe before reading).
    #[serde(default)]
    pub summary: String,
    /// Actual readable in-world passages, chapters, ritual steps, or excerpts (Markdown).
    pub content: String,
    /// Explicit RPG unlocks or revelations granted upon reading/deciphering.
    #[serde(default)]
    pub mechanics_or_unlocks: Vec<String>,
    /// Hidden GM-only notes (e.g., cipher DC, traps, deliberate historical lies, missing pages).
    #[serde(default)]
    pub gm_notes: String,
    /// Keywords that auto-surface this text in `get_scene_context`.
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub related_ids: Vec<String>,
    #[serde(default)]
    pub created_at_world_time: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_text_medium() -> String {
    "book".to_string()
}

fn default_text_subject() -> String {
    "history".to_string()
}

fn default_language() -> String {
    "Common".to_string()
}

fn default_text_condition() -> String {
    "Intact".to_string()
}

fn default_read_status() -> String {
    "undiscovered".to_string()
}

/// Story-relevant attached reference document or media file copied into
/// `<campaign>/docs/attachments/` with stable addressable references.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryAttachment {
    pub id: String,
    pub title: String,
    pub original_filename: String,
    pub stored_filename: String,
    /// Portable forward-slash relative path inside the campaign dir (e.g. `docs/attachments/world-map.png`).
    pub relative_path: String,
    /// e.g. "map", "handout", "portrait", "rules", "world-doc", "notes", "audio", "other"
    #[serde(default = "default_attachment_category")]
    pub category: String,
    #[serde(default = "default_mime")]
    pub mime_type: String,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub summary: String,
    /// Searchable text excerpt or key facts extracted by the agent from the file (great for PDFs/images).
    #[serde(default)]
    pub extracted_notes: String,
    /// "public" | "rumor" | "gm_secret"
    #[serde(default = "default_visibility")]
    pub visibility: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub related_ids: Vec<String>,
    #[serde(default)]
    pub source_origin: String,
    #[serde(default)]
    pub created_at_world_time: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_attachment_category() -> String {
    "world-doc".to_string()
}

fn default_mime() -> String {
    "application/octet-stream".to_string()
}

/// First-class cataloged carryable item: weapons, armor, tools, artifacts, keepsakes,
/// books, manuals, scrolls, records, keys, and relics with full ownership & provenance tracking.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogItem {
    pub id: String,
    pub name: String,
    /// e.g. "clothing", "apparel", "armor", "weapon", "tool", "artifact", "keepsake", "book", "manual", "scroll", "record", "relic", "key", "consumable"
    #[serde(default = "default_item_type")]
    pub item_type: String,
    #[serde(default)]
    pub rarity_or_tier: String,
    #[serde(default)]
    pub slot: Option<String>,
    #[serde(default = "default_qty")]
    pub quantity: i64,
    #[serde(default)]
    pub equipped: bool,
    #[serde(default = "default_text_condition")]
    pub condition_or_durability: String,
    /// Who created/forged/wrote this item and where it originated.
    #[serde(default)]
    pub origin_and_maker: String,
    /// Ordered list of previous owners (names or entity slugs).
    #[serde(default)]
    pub previous_owners: Vec<String>,
    /// Current character carrying it OR place where it is stored/resting.
    #[serde(default)]
    pub current_holder_id: Option<String>,
    /// Narrative/historical significance of this item.
    #[serde(default)]
    pub significance: String,
    /// Mechanical properties, enchantments, bonuses, or tool capabilities.
    #[serde(default)]
    pub properties_and_effects: String,
    #[serde(default)]
    pub description: String,
    /// Optional link to a `StoryText` (`docs/texts/<id>.md`) when this carryable item is a book, manual, scroll, or record.
    #[serde(default)]
    pub linked_text_id: Option<String>,
    #[serde(default)]
    pub gm_notes: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub related_ids: Vec<String>,
    /// Optional dimensional measurements for wearable garments/armor (e.g. Shoulders, Chest, Waist, Hips, Thigh, Inseam).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub garment_measurements: BTreeMap<String, String>,
    /// Optional garment cut/stretch profile: "fitted" | "tailored" | "standard" | "relaxed" | "loose" | "stretch" | "rigid_armor".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub garment_cut: String,
    /// Optional garment taper/silhouette profile: "straight" | "boxy" | "athletic_v" | "tailored_waist" | "hourglass" | "a_line" | "relaxed_drape".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub garment_taper: String,
    #[serde(default)]
    pub created_at_world_time: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_item_type() -> String {
    "gear".to_string()
}

/// Structured historical chronicle entry for any Place, Establishment, Sect/Faction,
/// Carryable Item (weapon/tool/keepsake/book/manual/record), Lore entry, or Character.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityHistoryEntry {
    #[serde(default)]
    pub id: i64,
    pub entity_id: String,
    /// "place" | "establishment" | "sect" | "faction" | "item" | "text" | "character" | "lore"
    #[serde(default = "default_entity_kind")]
    pub entity_kind: String,
    #[serde(default)]
    pub era_or_date: String,
    #[serde(default)]
    pub sort_order: i64,
    pub title: String,
    pub details: String,
    #[serde(default)]
    pub significance: String,
    #[serde(default)]
    pub related_ids: Vec<String>,
    /// "public" | "rumor" | "gm_secret"
    #[serde(default = "default_visibility")]
    pub visibility: String,
    #[serde(default)]
    pub recorded_at_world_time: String,
}

fn default_entity_kind() -> String {
    "place".to_string()
}

/// Typed Knowledge Graph edge linking any two entities (character, place, lore, faction).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRelation {
    pub source_id: String,
    pub source_kind: String,
    pub relation_type: String,
    pub target_id: String,
    pub target_kind: String,
    /// Optional disposition/strength (-100 to +100 or custom metric).
    #[serde(default)]
    pub strength: Option<i64>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub updated_at_world_time: String,
    /// Optional multi-dimensional relationship vector (trust, respect, affection, fear, resentment, dependence).
    #[serde(default, skip_serializing_if = "RelationshipDimensions::is_empty")]
    pub dimensions: RelationshipDimensions,
    /// "directed" (default) | "mutual"
    #[serde(default = "default_rel_direction")]
    pub direction: String,
    /// "public" | "gm_secret" (defaults to "public" for legacy relations without hidden dimensions; set "gm_secret" to hide scores from player POV).
    #[serde(default = "default_rel_visibility")]
    pub visibility: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_event_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub obligations: Vec<String>,
}

fn default_rel_direction() -> String {
    "directed".to_string()
}

fn default_rel_visibility() -> String {
    "public".to_string()
}

/// Blades-in-the-Dark style Progress Clock / Quest / Threat / Timed Countdown Tracker.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProgressClock {
    pub id: String,
    pub name: String,
    /// e.g. "quest", "threat", "faction", "ritual", "travel", "training", "upkeep"
    #[serde(default = "default_clock_kind")]
    pub kind: String,
    pub filled: i64,
    pub segments: i64,
    #[serde(default = "default_clock_status")]
    pub status: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub related_ids: Vec<String>,
    /// Optional automatic time-ticking cadence in in-world hours (e.g. `24` for a daily rent/countdown clock).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cadence_hours: Option<u64>,
    /// Last `WorldClock::total_hours()` anchor used for automatic cadence ticking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tick_total_hours: Option<i64>,
    /// Consequence / alarm directive surfaced on the Signal Bus when this clock fills.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub consequence_on_fill: String,
    /// When true (e.g. recurring monthly rent or weekly tithe), automatically resets `filled = 0` after firing `CLOCK_FILLED`!
    #[serde(default)]
    pub auto_reset: bool,
}

fn default_clock_kind() -> String {
    "quest".to_string()
}

fn default_clock_status() -> String {
    "active".to_string()
}

/// Immutable, append-only event record in the SQLite event sourcing ledger.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldEvent {
    pub seq: i64,
    pub session_number: u32,
    pub world_time: String,
    pub real_timestamp: String,
    pub location_id: Option<String>,
    pub affected_entities: Vec<String>,
    pub summary: String,
    pub payload: EventPayload,
    pub prev_hash: String,
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub causal_event_ids: Vec<i64>,
    #[serde(default = "default_event_visibility")]
    pub visibility: String,
}

fn default_event_visibility() -> String {
    "public".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventPayload {
    CampaignStarted {
        name: String,
    },
    SessionMarker {
        session_number: u32,
        title: String,
        summary: String,
    },
    TimeAdvanced {
        minutes: u64,
        new_clock: WorldClock,
        season_changed: bool,
    },
    CharacterCreated {
        character_id: String,
    },
    CharacterMoved {
        character_id: String,
        from_place: Option<String>,
        to_place: String,
        #[serde(default)]
        travel_notes: String,
    },
    ResourceChanged {
        character_id: String,
        resource: String,
        #[serde(default)]
        delta: Option<i64>,
        #[serde(default)]
        set_current: Option<i64>,
        #[serde(default)]
        set_max: Option<i64>,
        #[serde(default)]
        unit: Option<String>,
        #[serde(default)]
        remove: bool,
        #[serde(default)]
        reason: String,
    },
    StatChanged {
        character_id: String,
        stat: String,
        #[serde(default)]
        delta: Option<i64>,
        #[serde(default)]
        set_value: Option<i64>,
        #[serde(default)]
        reason: String,
    },
    CharacterStatusChanged {
        character_id: String,
        alive_status: String,
        #[serde(default)]
        reason: String,
    },
    ConditionAdded {
        character_id: String,
        condition: StatusCondition,
    },
    ConditionRemoved {
        character_id: String,
        condition_name: String,
        #[serde(default)]
        reason: String,
    },
    ItemAcquired {
        character_id: String,
        item: Item,
    },
    ItemRemoved {
        character_id: String,
        item_name: String,
        quantity: i64,
        #[serde(default)]
        reason: String,
    },
    ItemEquipped {
        character_id: String,
        item_name: String,
        #[serde(default)]
        slot: Option<String>,
        equipped: bool,
    },
    ItemTagsUpdated {
        character_id: String,
        item_name: String,
        #[serde(default)]
        add_tags: Vec<String>,
        #[serde(default)]
        remove_tags: Vec<String>,
        #[serde(default)]
        replace_tags: Option<Vec<String>>,
        #[serde(default)]
        reason: String,
    },
    SkillUpdated {
        character_id: String,
        skill: Skill,
    },
    TechniqueUpdated {
        character_id: String,
        technique: Technique,
    },
    GoalUpdated {
        character_id: String,
        goal: String,
        completed: bool,
    },
    PlaceCreated {
        place_id: String,
    },
    PlaceWeatherChanged {
        place_id: String,
        weather: String,
        #[serde(default)]
        notes: String,
    },
    PlaceConditionChanged {
        place_id: String,
        condition: String,
        active: bool,
        #[serde(default)]
        notes: String,
    },
    PlaceControlChanged {
        place_id: String,
        faction: Option<String>,
        danger_level: Option<String>,
        #[serde(default)]
        notes: String,
    },
    LandmarkUpdated {
        place_id: String,
        landmark: Landmark,
    },
    RouteUpdated {
        place_id: String,
        route: Route,
    },
    RelationUpserted {
        relation: GraphRelation,
    },
    RelationRemoved {
        source_id: String,
        relation_type: String,
        target_id: String,
        #[serde(default)]
        reason: String,
    },
    ClockCreated {
        clock: ProgressClock,
    },
    ClockTicked {
        clock_id: String,
        delta: i64,
        #[serde(default)]
        set_status: Option<String>,
        #[serde(default)]
        reason: String,
    },
    LoreRecorded {
        lore_id: String,
        action: String,
        #[serde(default)]
        notes: String,
    },
    TextRecorded {
        text_id: String,
        action: String,
        #[serde(default)]
        notes: String,
    },
    TextStudied {
        text_id: String,
        #[serde(default)]
        reader_id: Option<String>,
        #[serde(default)]
        read_status: Option<String>,
        #[serde(default)]
        holder_or_location_id: Option<String>,
        #[serde(default)]
        condition: Option<String>,
        #[serde(default)]
        notes: String,
    },
    AttachmentRecorded {
        attachment_id: String,
        relative_path: String,
        action: String,
        #[serde(default)]
        notes: String,
    },
    ItemCataloged {
        item_id: String,
        action: String,
        #[serde(default)]
        holder_id: Option<String>,
        #[serde(default)]
        notes: String,
    },
    CatalogItemTransferred {
        item_id: String,
        #[serde(default)]
        from_holder_id: Option<String>,
        #[serde(default)]
        to_holder_id: Option<String>,
        #[serde(default)]
        condition_or_durability: Option<String>,
        #[serde(default)]
        notes: String,
    },
    EntityHistoryAdded {
        history_id: i64,
        entity_id: String,
        entity_kind: String,
        era_or_date: String,
        title: String,
    },
    EntityEdited {
        register: String,
        entity_id: String,
        #[serde(default)]
        fields_changed: Vec<String>,
        #[serde(default)]
        reason: String,
    },
    EntityReclassified {
        from_register: String,
        from_id: String,
        to_register: String,
        to_id: String,
        #[serde(default)]
        reason: String,
    },
    EntityRenamedOrMerged {
        register: String,
        old_id: String,
        new_id: String,
        merged: bool,
        references_updated: usize,
        #[serde(default)]
        reason: String,
    },
    EntityDeleted {
        register: String,
        entity_id: String,
        #[serde(default)]
        reason: String,
    },
    DiceRolled {
        expression: String,
        rolls: Vec<i64>,
        kept: Vec<i64>,
        modifier: i64,
        total: i64,
        #[serde(default)]
        actor_id: Option<String>,
        #[serde(default)]
        purpose: String,
    },
    RulesetProfileUpserted {
        profile_id: String,
        resolution_mode: String,
        base_dice: String,
        version: u32,
    },
    CheckResolved {
        #[serde(default)]
        actor_id: Option<String>,
        action: String,
        profile_id: String,
        resolution_mode: String,
        expression: String,
        seed: u64,
        roll: i64,
        modifier: i64,
        total: i64,
        difficulty: i64,
        margin: i64,
        outcome: String,
        #[serde(default)]
        stakes: String,
        #[serde(default)]
        modifiers_json: serde_json::Value,
    },
    ContestResolved {
        action: String,
        profile_id: String,
        seed: u64,
        #[serde(default)]
        winner_id: Option<String>,
        margin: i64,
        is_tie: bool,
        #[serde(default)]
        stakes: String,
        #[serde(default)]
        participants_json: serde_json::Value,
    },
    RandomTableUpserted {
        table_id: String,
        name: String,
        category: String,
        version: u32,
        entry_count: usize,
    },
    RandomTableDrawn {
        table_id: String,
        table_version: u32,
        seed: u64,
        replacement: bool,
        count: usize,
        #[serde(default)]
        staged_discovery_id: Option<String>,
        #[serde(default)]
        results: Vec<String>,
    },
    DiscoveryClaimed {
        discovery_id: String,
        claimant_id: String,
        #[serde(default)]
        claimed_items: Vec<String>,
        #[serde(default)]
        notes: String,
    },
    EncounterStarted {
        encounter_id: String,
        title: String,
        encounter_type: String,
        #[serde(default)]
        participants: Vec<String>,
    },
    EncounterActionResolved {
        encounter_id: String,
        round: u32,
        actor_id: String,
        action_type: String,
        description: String,
        #[serde(default)]
        target_ids: Vec<String>,
    },
    EncounterEnded {
        encounter_id: String,
        title: String,
        rounds: u32,
        #[serde(default)]
        outcome: String,
        #[serde(default)]
        aftermath: String,
    },
    CharacterProfileUpdated {
        character_id: String,
        #[serde(default)]
        fields_updated: Vec<String>,
    },
    KnowledgeRecorded {
        claim_id: String,
        subject: String,
        truth_status: String,
        #[serde(default)]
        actors_updated: Vec<String>,
    },
    KnowledgePropagated {
        claim_id: String,
        #[serde(default)]
        from_actor_id: Option<String>,
        to_actor_ids: Vec<String>,
        deceptive: bool,
        #[serde(default)]
        notes: String,
    },
    FactionRecorded {
        faction_id: String,
        name: String,
        action: String,
        #[serde(default)]
        notes: String,
    },
    AutonomySimulated {
        scope_type: String,
        #[serde(default)]
        faction_id: Option<String>,
        #[serde(default)]
        actor_ids: Vec<String>,
        duration_hours: u64,
        actions_count: usize,
        #[serde(default)]
        summary: String,
    },
    TravelResolved {
        from_place_id: String,
        to_place_id: String,
        transport_mode: String,
        distance_km: f64,
        elapsed_hours: f64,
        #[serde(default)]
        travelers: Vec<String>,
        #[serde(default)]
        rerouted_around: Vec<String>,
    },
    AreaExplored {
        place_id: String,
        mode: String,
        #[serde(default)]
        actor_id: Option<String>,
        #[serde(default)]
        findings: Vec<String>,
    },
    TransactionExecuted {
        kind: String,
        #[serde(default)]
        from_id: Option<String>,
        #[serde(default)]
        to_id: Option<String>,
        amount: i64,
        currency: String,
        #[serde(default)]
        item_name: Option<String>,
        #[serde(default)]
        notes: String,
    },
    MarketUpdated {
        place_id: String,
        action: String,
        #[serde(default)]
        notes: String,
    },
    LawOrReputationUpdated {
        kind: String,
        subject_id: String,
        #[serde(default)]
        target_id: String,
        #[serde(default)]
        summary: String,
    },
    InjuryUpdated {
        injury_id: String,
        character_id: String,
        action: String,
        severity: String,
        days_remaining: i64,
    },
    DowntimeOrCraftingResolved {
        kind: String,
        actor_id: String,
        #[serde(default)]
        project_id: Option<String>,
        #[serde(default)]
        outcome: String,
    },
    ProgressionResolved {
        actor_id: String,
        progression_kind: String,
        outcome: String,
        #[serde(default)]
        notes: String,
    },
    WorldAdvanced {
        duration_hours: u64,
        #[serde(default)]
        scoped_places: Vec<String>,
        #[serde(default)]
        scoped_factions: Vec<String>,
        #[serde(default)]
        scoped_characters: Vec<String>,
        #[serde(default)]
        summary: String,
    },
    NarrativeEvent {
        title: String,
        details: String,
        #[serde(default)]
        tags: Vec<String>,
    },
}

/// Sanitize any name or ID into a filesystem-safe lowercase slug.
pub fn slugify(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_dash = false;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "unnamed".to_string()
    } else {
        out
    }
}
