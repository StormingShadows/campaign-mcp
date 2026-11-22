use crate::models::{slugify, CharacterProjection, PlaceProjection};
use anyhow::{bail, Result};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

// ============================================================================
// 1. Connected Geography, Spatial Info & Graph Travel Pathfinding
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaceSpatialExt {
    pub place_id: String,
    #[serde(default)]
    pub coordinates: Option<[f64; 2]>,
    #[serde(default)]
    pub map_region: String,
    #[serde(default)]
    pub ownership: String,
    #[serde(default)]
    pub jurisdiction: String,
    #[serde(default)]
    pub population: Option<u64>,
    #[serde(default)]
    pub infrastructure: Vec<String>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub resource_deposits: Vec<String>,
    #[serde(default)]
    pub customs: Vec<String>,
    #[serde(default)]
    pub updated_at_world_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TravelRoute {
    #[serde(default)]
    pub id: String,
    pub from_place_id: String,
    pub to_place_id: String,
    #[serde(default = "default_true")]
    pub bidirectional: bool,
    #[serde(default = "default_distance_km")]
    pub distance_km: f64,
    #[serde(default = "default_terrain")]
    pub terrain: String,
    #[serde(default)]
    pub elevation_gain_m: i64,
    #[serde(default = "default_difficulty")]
    pub difficulty: i64,
    /// Weather sensitivity multiplier (0.5 sheltered road .. 2.0 exposed mountain ridge).
    #[serde(default = "default_one_f64")]
    pub weather_exposure: f64,
    #[serde(default)]
    pub seasonal_multipliers: BTreeMap<String, f64>,
    #[serde(default)]
    pub transport_modes: Vec<String>,
    #[serde(default)]
    pub hazards: Vec<String>,
    #[serde(default)]
    pub jurisdiction: String,
    #[serde(default)]
    pub patrol_presence: String,
    #[serde(default)]
    pub toll_cost: i64,
    #[serde(default = "default_currency")]
    pub toll_currency: String,
    /// "open" | "hazardous" | "blocked" | "destroyed"
    #[serde(default = "default_route_open")]
    pub status: String,
    /// True when distance_km was synthesized from prose travel_time rather than explicitly configured.
    #[serde(default)]
    pub distance_estimated: bool,
    /// Preserved canonical prose travel time (e.g. "Two days on foot") if synthesized from place routes.
    #[serde(default)]
    pub canonical_travel_time: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_true() -> bool {
    true
}
fn default_distance_km() -> f64 {
    15.0
}
fn default_terrain() -> String {
    "road".to_string()
}
fn default_difficulty() -> i64 {
    10
}
fn default_one_f64() -> f64 {
    1.0
}
fn default_currency() -> String {
    "copper_cash".to_string()
}
fn default_route_open() -> String {
    "open".to_string()
}

impl TravelRoute {
    pub fn normalize(mut self) -> Result<Self> {
        self.from_place_id = slugify(&self.from_place_id);
        self.to_place_id = slugify(&self.to_place_id);
        if self.from_place_id == self.to_place_id {
            bail!("Travel route cannot connect a place to itself");
        }
        if self.id.trim().is_empty() {
            self.id = format!("{}-to-{}", self.from_place_id, self.to_place_id);
        } else {
            self.id = slugify(&self.id);
        }
        self.distance_km = self.distance_km.max(0.5);
        self.weather_exposure = self.weather_exposure.clamp(0.25, 3.0);
        self.status = self.status.trim().to_lowercase();
        if self.status.is_empty() {
            self.status = "open".to_string();
        }
        Ok(self)
    }

    pub fn is_passable(&self) -> bool {
        !matches!(self.status.as_str(), "blocked" | "destroyed" | "impassable")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TravelLeg {
    pub route_id: String,
    pub from_place_id: String,
    pub to_place_id: String,
    pub distance_km: f64,
    pub terrain: String,
    pub status: String,
    pub estimated_hours: f64,
    pub toll_cost: i64,
    pub toll_currency: String,
    pub hazards: Vec<String>,
    pub jurisdiction: String,
    pub patrol_presence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TravelPlan {
    pub from_place_id: String,
    pub to_place_id: String,
    pub viable: bool,
    pub transport_mode: String,
    pub forced_march: bool,
    pub season: String,
    pub weather: String,
    pub legs: Vec<TravelLeg>,
    pub total_distance_km: f64,
    pub total_travel_hours: f64,
    pub rest_stops_required: u32,
    pub total_elapsed_hours: f64,
    pub rations_required: i64,
    pub tolls_by_currency: BTreeMap<String, i64>,
    pub hazards: Vec<String>,
    pub rerouted_around: Vec<String>,
    pub encumbrance_notes: Vec<String>,
    pub injury_fatigue_notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

#[derive(Clone, PartialEq)]
struct PathNode {
    cost_hours: f64,
    place_id: String,
}

impl Eq for PathNode {}

impl Ord for PathNode {
    fn cmp(&self, other: &Self) -> Ordering {
        // Min-heap by cost_hours
        other
            .cost_hours
            .partial_cmp(&self.cost_hours)
            .unwrap_or(Ordering::Equal)
            .then_with(|| self.place_id.cmp(&other.place_id))
    }
}

impl PartialOrd for PathNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

pub fn base_speed_kmh(transport_mode: &str) -> f64 {
    match transport_mode.trim().to_lowercase().as_str() {
        "mount" | "horse" | "riding" => 8.0,
        "cart" | "wagon" | "caravan" => 4.2,
        "boat" | "riverboat" | "ship" => 7.0,
        "flying" | "sword-flight" | "airship" => 35.0,
        _ => 4.5, // foot
    }
}

pub fn terrain_speed_factor(terrain: &str) -> f64 {
    let t = terrain.to_lowercase();
    if t.contains("imperial") || t.contains("paved") || t.contains("highway") {
        1.15
    } else if t.contains("road") || t.contains("plains") || t.contains("valley") {
        1.0
    } else if t.contains("hill") || t.contains("forest") || t.contains("trail") {
        0.75
    } else if t.contains("mountain") || t.contains("ridge") || t.contains("swamp") || t.contains("marsh") {
        0.55
    } else if t.contains("cliff") || t.contains("glacier") || t.contains("dense jungle") {
        0.4
    } else {
        0.85
    }
}

pub fn weather_speed_factor(weather: &str, exposure: f64) -> f64 {
    let w = weather.to_lowercase();
    let raw_penalty = if w.contains("blizzard") || w.contains("typhoon") || w.contains("tempest") {
        0.55
    } else if w.contains("storm") || w.contains("heavy rain") || w.contains("ice") || w.contains("snow") {
        0.35
    } else if w.contains("rain") || w.contains("fog") || w.contains("mud") || w.contains("wind") {
        0.15
    } else {
        0.0
    };
    (1.0 - (raw_penalty * exposure).min(0.75)).max(0.25)
}

/// Shortest-path Dijkstra over live `TravelRoute` graph.
/// When `ignore_closures == true`, computes the baseline intact-network shortest path (used to detect
/// detours and compute automatic economic route-disruption surcharges!).
pub fn compute_shortest_route(
    routes: &[TravelRoute],
    from_id: &str,
    to_id: &str,
    transport_mode: &str,
    season: &str,
    weather: &str,
    speed_multiplier: f64,
    ignore_closures: bool,
) -> Option<(Vec<TravelLeg>, f64, f64)> {
    if from_id == to_id {
        return Some((vec![], 0.0, 0.0));
    }
    let mut adj: BTreeMap<String, Vec<(&TravelRoute, String)>> = BTreeMap::new();
    for r in routes {
        if !ignore_closures && !r.is_passable() {
            continue;
        }
        adj.entry(r.from_place_id.clone())
            .or_default()
            .push((r, r.to_place_id.clone()));
        if r.bidirectional {
            adj.entry(r.to_place_id.clone())
                .or_default()
                .push((r, r.from_place_id.clone()));
        }
    }

    let base_spd = base_speed_kmh(transport_mode) * speed_multiplier.clamp(0.25, 2.0);
    let mut dist: BTreeMap<String, f64> = BTreeMap::new();
    let mut prev: BTreeMap<String, (String, TravelLeg)> = BTreeMap::new();
    let mut heap = BinaryHeap::new();

    dist.insert(from_id.to_string(), 0.0);
    heap.push(PathNode {
        cost_hours: 0.0,
        place_id: from_id.to_string(),
    });

    while let Some(PathNode { cost_hours, place_id }) = heap.pop() {
        if place_id == to_id {
            break;
        }
        if let Some(&best) = dist.get(&place_id) {
            if cost_hours > best + 1e-6 {
                continue;
            }
        }
        if let Some(edges) = adj.get(&place_id) {
            for (route, next_id) in edges {
                let terr_f = terrain_speed_factor(&route.terrain);
                let w_f = weather_speed_factor(weather, route.weather_exposure);
                let seas_f = route
                    .seasonal_multipliers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(season))
                    .map(|(_, v)| (*v).clamp(0.25, 2.0))
                    .unwrap_or(1.0);
                let hazard_f = if route.status == "hazardous" { 0.8 } else { 1.0 };
                let elev_extra_hours = (route.elevation_gain_m.max(0) as f64) / 600.0;
                let eff_spd = (base_spd * terr_f * w_f * seas_f * hazard_f).max(0.5);
                let eff_km = if route.distance_km > 0.0 {
                    route.distance_km
                } else {
                    // Unconfigured synthesized route: use nominal 10 km fallback for graph ordering only
                    10.0
                };
                let leg_hours = ((eff_km / eff_spd) + elev_extra_hours).max(0.25);
                let next_cost = cost_hours + leg_hours;

                let is_better = dist.get(next_id).map(|&c| next_cost < c).unwrap_or(true);
                if is_better {
                    dist.insert(next_id.clone(), next_cost);
                    prev.insert(
                        next_id.clone(),
                        (
                            place_id.clone(),
                            TravelLeg {
                                route_id: route.id.clone(),
                                from_place_id: place_id.clone(),
                                to_place_id: next_id.clone(),
                                distance_km: route.distance_km,
                                terrain: route.terrain.clone(),
                                status: route.status.clone(),
                                estimated_hours: (leg_hours * 10.0).round() / 10.0,
                                toll_cost: route.toll_cost,
                                toll_currency: route.toll_currency.clone(),
                                hazards: route.hazards.clone(),
                                jurisdiction: route.jurisdiction.clone(),
                                patrol_presence: route.patrol_presence.clone(),
                            },
                        ),
                    );
                    heap.push(PathNode {
                        cost_hours: next_cost,
                        place_id: next_id.clone(),
                    });
                }
            }
        }
    }

    if !prev.contains_key(to_id) {
        return None;
    }

    let mut legs = Vec::new();
    let mut cur = to_id.to_string();
    while cur != from_id {
        let (p, leg) = prev.remove(&cur)?;
        legs.push(leg);
        cur = p;
    }
    legs.reverse();
    let total_km: f64 = legs.iter().map(|l| l.distance_km).sum();
    let total_hours: f64 = legs.iter().map(|l| l.estimated_hours).sum();
    Some((legs, (total_km * 10.0).round() / 10.0, (total_hours * 10.0).round() / 10.0))
}

// ============================================================================
// 2. Economy: Encumbrance, Atomic Multi-Currency, Contracts & Dynamic Markets
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemEconomics {
    pub item_key: String,
    #[serde(default)]
    pub weight_kg: f64,
    #[serde(default)]
    pub volume_liters: f64,
    #[serde(default)]
    pub unit_value: i64,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default = "default_true")]
    pub stackable: bool,
    #[serde(default = "default_quality")]
    pub quality: String,
    #[serde(default)]
    pub requirements: String,
}

fn default_quality() -> String {
    "standard".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncumbranceReport {
    pub character_id: String,
    pub total_weight_kg: f64,
    pub carry_capacity_kg: f64,
    pub ratio: f64,
    /// "light" | "moderate" | "heavy" | "overloaded"
    pub load_tier: String,
    pub travel_speed_multiplier: f64,
    pub physical_check_penalty: i64,
    pub item_weights: Vec<(String, f64)>,
}

pub fn compute_encumbrance(
    proj: &CharacterProjection,
    econ_map: &BTreeMap<String, ItemEconomics>,
) -> EncumbranceReport {
    // Derive base carry capacity from Strength/Body/Physique stat if present (default 35 kg for mortal adult)
    let str_stat = proj
        .current
        .stats
        .iter()
        .find(|(k, _)| {
            let kl = k.to_lowercase();
            kl.contains("str") || kl.contains("body") || kl.contains("phys") || kl.contains("might")
        })
        .map(|(_, v)| *v)
        .unwrap_or(10);
    let capacity_kg = (20.0 + (str_stat.max(1) as f64) * 2.5).max(15.0);

    let mut total = 0.0f64;
    let mut breakdown = Vec::new();
    for it in &proj.current.equipment {
        let key = slugify(&it.name);
        let unit_w = econ_map
            .get(&key)
            .map(|e| e.weight_kg)
            .unwrap_or(0.5); // sensible 0.5 kg default for uncataloged gear
        let w = (unit_w * it.quantity.max(1) as f64 * 100.0).round() / 100.0;
        total += w;
        breakdown.push((format!("{} x{}", it.name, it.quantity), w));
    }
    total = (total * 100.0).round() / 100.0;
    let ratio = ((total / capacity_kg) * 100.0).round() / 100.0;
    let (tier, spd, pen) = if ratio <= 0.5 {
        ("light", 1.0, 0)
    } else if ratio <= 0.85 {
        ("moderate", 0.9, -1)
    } else if ratio <= 1.15 {
        ("heavy", 0.7, -2)
    } else {
        ("overloaded", 0.4, -4)
    };
    EncumbranceReport {
        character_id: proj.baseline.id.clone(),
        total_weight_kg: total,
        carry_capacity_kg: capacity_kg,
        ratio,
        load_tier: tier.to_string(),
        travel_speed_multiplier: spd,
        physical_check_penalty: pen,
        item_weights: breakdown,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketCommodity {
    pub name: String,
    #[serde(default)]
    pub category: String,
    pub base_price: i64,
    #[serde(default = "default_currency")]
    pub currency: String,
    /// Current local stock (units)
    #[serde(default = "default_supply")]
    pub supply: i64,
    /// Local baseline demand (units)
    #[serde(default = "default_supply")]
    pub demand: i64,
    #[serde(default)]
    pub daily_production: i64,
    #[serde(default)]
    pub daily_consumption: i64,
    /// External place IDs supplying this commodity (used for live broken-route ripple!).
    #[serde(default)]
    pub imported_from: Vec<String>,
    #[serde(default)]
    pub seasonal_multipliers: BTreeMap<String, f64>,
    #[serde(default)]
    pub monopoly_surcharge_pct: i64,
}

fn default_supply() -> i64 {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementMarket {
    pub place_id: String,
    pub name: String,
    #[serde(default)]
    pub political_modifier_pct: i64,
    #[serde(default)]
    pub tax_rate_pct: i64,
    #[serde(default)]
    pub supply_origins: Vec<String>,
    #[serde(default)]
    pub commodities: Vec<MarketCommodity>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputedCommodityPrice {
    pub name: String,
    pub category: String,
    pub currency: String,
    pub base_price: i64,
    pub live_price: i64,
    pub supply: i64,
    pub demand: i64,
    pub supply_demand_multiplier: f64,
    pub seasonal_multiplier: f64,
    pub route_disruption_multiplier: f64,
    pub disrupted_routes: Vec<String>,
    pub tax_and_political_pct: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketPriceReport {
    pub place_id: String,
    pub market_name: String,
    pub season: String,
    pub weather: String,
    pub commodities: Vec<ComputedCommodityPrice>,
    pub disrupted_supply_lines: Vec<String>,
}

/// Computes live dynamic market prices, automatically reflecting supply/demand, season,
/// taxes/monopolies, AND live broken/destroyed bridges or blocked trade routes!
pub fn evaluate_market_prices(
    market: &SettlementMarket,
    routes: &[TravelRoute],
    season: &str,
    weather: &str,
) -> MarketPriceReport {
    let mut all_disrupted = BTreeSet::new();
    let mut prices = Vec::new();

    for c in &market.commodities {
        let sd_ratio = (c.demand.max(1) as f64) / (c.supply.max(10) as f64);
        let sd_mult = sd_ratio.clamp(0.4, 4.0);
        let seas_mult = c
            .seasonal_multipliers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(season))
            .map(|(_, v)| (*v).clamp(0.5, 3.0))
            .unwrap_or(1.0);

        // Compute live route-disruption surcharge from imported_from (or market.supply_origins)
        let origins: &[String] = if !c.imported_from.is_empty() {
            &c.imported_from
        } else {
            &market.supply_origins
        };
        let mut route_mult = 1.0f64;
        let mut disrupted = Vec::new();
        if !origins.is_empty() {
            let mut sum_mult = 0.0f64;
            for orig in origins {
                let baseline = compute_shortest_route(
                    routes,
                    orig,
                    &market.place_id,
                    "cart",
                    season,
                    "Clear",
                    1.0,
                    true,
                );
                let live = compute_shortest_route(
                    routes,
                    orig,
                    &market.place_id,
                    "cart",
                    season,
                    weather,
                    1.0,
                    false,
                );
                match (baseline, live) {
                    (Some((base_legs, base_km, _)), Some((_, live_km, _))) => {
                        if live_km > base_km + 0.5 {
                            let ratio = (live_km / base_km.max(1.0)).clamp(1.0, 3.0);
                            let surcharge = 1.0 + (ratio - 1.0) * 0.65;
                            sum_mult += surcharge;
                            let broken: Vec<String> = base_legs
                                .iter()
                                .filter(|l| !matches!(l.status.as_str(), "open" | "hazardous"))
                                .map(|l| format!("{} ({})", l.route_id, l.status))
                                .collect();
                            let msg = format!(
                                "Supply line from '{orig}' detoured ({base_km:.1}km -> {live_km:.1}km; disrupted: {})",
                                if broken.is_empty() {
                                    "closure".to_string()
                                } else {
                                    broken.join(", ")
                                }
                            );
                            disrupted.push(msg.clone());
                            all_disrupted.insert(msg);
                        } else {
                            sum_mult += 1.0;
                        }
                    }
                    (Some((base_legs, _, _)), None) => {
                        // Completely cut off!
                        sum_mult += 1.75;
                        let broken: Vec<String> = base_legs
                            .iter()
                            .filter(|l| !matches!(l.status.as_str(), "open" | "hazardous"))
                            .map(|l| format!("{} ({})", l.route_id, l.status))
                            .collect();
                        let msg = format!(
                            "Supply line from '{orig}' CUT OFF (no passable route; blocked: {})",
                            broken.join(", ")
                        );
                        disrupted.push(msg.clone());
                        all_disrupted.insert(msg);
                    }
                    _ => {
                        sum_mult += 1.0;
                    }
                }
            }
            route_mult = (sum_mult / origins.len() as f64).clamp(1.0, 3.0);
        }

        let extra_pct = market.political_modifier_pct + market.tax_rate_pct + c.monopoly_surcharge_pct;
        let extra_mult = (1.0 + (extra_pct as f64 / 100.0)).max(0.25);
        let live_price = ((c.base_price as f64) * sd_mult * seas_mult * route_mult * extra_mult)
            .round()
            .max(1.0) as i64;

        prices.push(ComputedCommodityPrice {
            name: c.name.clone(),
            category: c.category.clone(),
            currency: c.currency.clone(),
            base_price: c.base_price,
            live_price,
            supply: c.supply,
            demand: c.demand,
            supply_demand_multiplier: (sd_mult * 100.0).round() / 100.0,
            seasonal_multiplier: (seas_mult * 100.0).round() / 100.0,
            route_disruption_multiplier: (route_mult * 100.0).round() / 100.0,
            disrupted_routes: disrupted,
            tax_and_political_pct: extra_pct,
        });
    }

    MarketPriceReport {
        place_id: market.place_id.clone(),
        market_name: market.name.clone(),
        season: season.to_string(),
        weather: weather.to_string(),
        commodities: prices,
        disrupted_supply_lines: all_disrupted.into_iter().collect(),
    }
}

// ============================================================================
// 3. Law, Jurisdictions, Crime & Reputation
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JurisdictionRecord {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub controlling_faction_id: Option<String>,
    #[serde(default)]
    pub place_ids: Vec<String>,
    #[serde(default)]
    pub laws: Vec<String>,
    #[serde(default)]
    pub tax_rate_pct: i64,
    #[serde(default)]
    pub enforcement_strength: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrimeRecord {
    #[serde(default)]
    pub id: String,
    pub jurisdiction_id: String,
    pub suspect_id: String,
    pub offense: String,
    #[serde(default = "default_crime_severity")]
    pub severity: String,
    /// "reported" | "investigating" | "warrant_issued" | "captured" | "resolved" | "pardoned"
    #[serde(default = "default_warrant_status")]
    pub warrant_status: String,
    #[serde(default)]
    pub bounty: i64,
    #[serde(default = "default_currency")]
    pub bounty_currency: String,
    #[serde(default)]
    pub witnesses: Vec<String>,
    #[serde(default)]
    pub source_event_id: Option<i64>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub recorded_at_world_time: String,
}

fn default_crime_severity() -> String {
    "moderate".to_string()
}
fn default_warrant_status() -> String {
    "reported".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReputationStanding {
    pub entity_id: String,
    pub target_group_id: String,
    pub score: i64,
    pub tier: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

pub fn reputation_tier_label(score: i64) -> &'static str {
    match score {
        i64::MIN..=-60 => "Despised",
        -59..=-25 => "Hostile",
        -24..=-10 => "Distrusted",
        -9..=9 => "Neutral",
        10..=29 => "Friendly",
        30..=59 => "Respected",
        60..=84 => "Honored",
        _ => "Revered",
    }
}

// ============================================================================
// 4. Phase 4: Persistent Injuries/Recovery, Crafting & Markov Weather
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InjuryRecord {
    pub id: String,
    pub character_id: String,
    pub name: String,
    /// "minor" | "moderate" | "severe" | "critical"
    #[serde(default = "default_crime_severity")]
    pub severity: String,
    #[serde(default)]
    pub body_location: String,
    #[serde(default)]
    pub complications: Vec<String>,
    #[serde(default)]
    pub treated: bool,
    pub recovery_days_total: i64,
    pub recovery_days_remaining: i64,
    /// "active" | "healing" | "healed"
    #[serde(default = "default_injury_status")]
    pub status: String,
    #[serde(default)]
    pub source_event_id: Option<i64>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub inflicted_at_world_time: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_injury_status() -> String {
    "active".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CraftingProject {
    #[serde(default)]
    pub id: String,
    pub crafter_id: String,
    pub item_name: String,
    #[serde(default = "default_craft_type")]
    pub item_type: String,
    #[serde(default)]
    pub materials_used: Vec<String>,
    #[serde(default)]
    pub skill_name: String,
    #[serde(default = "default_difficulty")]
    pub difficulty: i64,
    pub required_hours: u64,
    #[serde(default)]
    pub hours_completed: u64,
    /// "in_progress" | "completed" | "failed"
    #[serde(default = "default_craft_status")]
    pub status: String,
    #[serde(default)]
    pub quality: String,
    #[serde(default)]
    pub properties_and_effects: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub started_at_world_time: String,
}

fn default_craft_type() -> String {
    "gear".to_string()
}
fn default_craft_status() -> String {
    "in_progress".to_string()
}

/// Deterministic seasonal Markov weather generator respecting climate & current season.
pub fn generate_seasonal_weather(
    season: &str,
    climate: &str,
    current_weather: &str,
    seed: u64,
) -> String {
    let mut rng = StdRng::seed_from_u64(seed);
    let s = season.to_lowercase();
    let c = climate.to_lowercase();

    // 30% weather persistence so transitions feel natural
    if !current_weather.trim().is_empty() && rng.gen_range(0..100) < 30 {
        return current_weather.to_string();
    }

    let pool: &[(&str, u32)] = if c.contains("desert") || c.contains("arid") {
        &[
            ("Dry Heat", 45),
            ("Clear & Arid", 30),
            ("Dusty Wind", 15),
            ("Sandstorm", 10),
        ]
    } else if s.contains("winter") {
        &[
            ("Freezing Clear", 25),
            ("Overcast & Cold", 25),
            ("Light Snow", 25),
            ("Icy Wind", 15),
            ("Blizzard", 10),
        ]
    } else if s.contains("spring") {
        &[
            ("Mild & Clear", 30),
            ("Morning Mist", 25),
            ("Light Rain", 25),
            ("Heavy Rain", 15),
            ("Spring Storm", 5),
        ]
    } else if s.contains("summer") {
        &[
            ("Warm & Clear", 35),
            ("Humid & Overcast", 25),
            ("Afternoon Rain", 20),
            ("Summer Storm", 15),
            ("Scorching Heat", 5),
        ]
    } else {
        // Autumn / default
        &[
            ("Crisp & Clear", 30),
            ("Cool Wind", 25),
            ("Overcast", 20),
            ("Cold Rain", 15),
            ("Thick Fog", 10),
        ]
    };

    let total: u32 = pool.iter().map(|(_, w)| *w).sum();
    let mut roll = rng.gen_range(1..=total);
    for (label, w) in pool {
        if roll <= *w {
            return (*label).to_string();
        }
        roll -= *w;
    }
    "Clear".to_string()
}

/// Checks if a character has any injury/fatigue conditions affecting overland travel speed.
pub fn travel_injury_factor(proj: &CharacterProjection) -> (f64, Vec<String>) {
    let mut factor = 1.0f64;
    let mut notes = Vec::new();
    for c in &proj.current.conditions {
        let kl = c.kind.to_lowercase();
        let sl = c.severity.to_lowercase();
        if matches!(kl.as_str(), "injury" | "wound" | "fatigue" | "exhaustion" | "poison") {
            let pen = match sl.as_str() {
                "critical" => 0.45,
                "severe" => 0.30,
                "moderate" => 0.15,
                _ => 0.08,
            };
            factor *= 1.0 - pen;
            notes.push(format!("{}: {} ({})", proj.baseline.name, c.name, c.severity));
        }
    }
    (factor.clamp(0.35, 1.0), notes)
}

/// Helper to build fallback TravelRoutes from legacy `PlaceCurrent.routes` when explicit routes aren't added yet.
/// Parses both numeric and English-word travel durations ("Two days on foot", "1-2 days", "30 km") and
/// explicitly marks synthesized/unconfigured distances with `distance_estimated = true` instead of silently faking 18 km!
pub fn synthesize_routes_from_places(places: &BTreeMap<String, PlaceProjection>) -> Vec<TravelRoute> {
    let mut out = Vec::new();
    for (pid, pp) in places {
        for r in &pp.current.routes {
            let parsed_km = parse_distance_or_hours_km(&r.travel_time, &r.notes);
            let (km, estimated) = match parsed_km {
                Some((val, est)) => (val.max(0.5), est),
                None => (0.0, true), // Never silently invent 18 km!
            };
            out.push(TravelRoute {
                id: format!("{pid}-to-{}", slugify(&r.destination_id)),
                from_place_id: pid.clone(),
                to_place_id: slugify(&r.destination_id),
                bidirectional: false,
                distance_km: km,
                terrain: if r.terrain_difficulty.is_empty() {
                    pp.baseline.terrain.clone()
                } else {
                    r.terrain_difficulty.clone()
                },
                elevation_gain_m: 0,
                difficulty: 10,
                weather_exposure: 1.0,
                seasonal_multipliers: BTreeMap::new(),
                transport_modes: vec!["foot".to_string(), "mount".to_string(), "cart".to_string()],
                hazards: vec![],
                jurisdiction: String::new(),
                patrol_presence: String::new(),
                toll_cost: 0,
                toll_currency: "copper_cash".to_string(),
                status: r.status.to_lowercase(),
                distance_estimated: estimated,
                canonical_travel_time: r.travel_time.clone(),
                notes: r.notes.clone(),
                updated_at_world_time: String::new(),
            });
        }
    }
    out
}

/// Extracts distance in km from prose travel time/notes (returns `Some((km, is_estimated))` or `None`).
pub fn parse_distance_or_hours_km(travel_time: &str, notes: &str) -> Option<(f64, bool)> {
    let combined = format!("{travel_time} {notes}").to_lowercase();
    if combined.trim().is_empty() {
        return None;
    }

    // 1. Check for explicit distance units (km / li / miles)
    if let Some(val) = extract_number_before_unit(&combined, &["km", "kilometer", "kilometre"]) {
        return Some((val, false));
    }
    if let Some(val) = extract_number_before_unit(&combined, &["mile", "miles", "mi"]) {
        return Some(((val * 1.609 * 10.0).round() / 10.0, false));
    }
    if let Some(val) = extract_number_before_unit(&combined, &[" li", "li "]) {
        return Some(((val * 0.5 * 10.0).round() / 10.0, false));
    }

    // 2. Parse duration (days, weeks, hours, minutes) including English number words & ranges!
    let hours = parse_hours_hint(travel_time).or_else(|| parse_hours_hint(notes))?;
    // 1 standard walking hour ≈ 4.5 km (8 walking hours/day = 36 km/day)
    Some((((hours * 4.5) * 10.0).round() / 10.0, true))
}

fn extract_number_before_unit(text: &str, units: &[&str]) -> Option<f64> {
    for unit in units {
        if let Some(pos) = text.find(unit) {
            let prefix = text[..pos].trim();
            if let Some(tok) = prefix.split_whitespace().last() {
                if let Some(n) = parse_word_or_number(tok) {
                    return Some(n);
                }
            }
        }
    }
    None
}

pub fn parse_hours_hint(s: &str) -> Option<f64> {
    let lower = s.to_lowercase();
    let cleaned: String = lower
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { ' ' })
        .collect();
    let tokens: Vec<&str> = cleaned.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }

    // Detect unit multiplier (hours per unit)
    let unit_hours = if lower.contains("fortnight") {
        14.0 * 8.0
    } else if lower.contains("week") {
        7.0 * 8.0
    } else if lower.contains("day") {
        8.0 // 8 marching hours per travel day
    } else if lower.contains("hour") || lower.contains("hr") {
        1.0
    } else if lower.contains("min") {
        1.0 / 60.0
    } else {
        return None;
    };

    // Collect numeric/word quantities (handling ranges like "1-2 days" or "two to three days")
    let mut nums = Vec::new();
    for tok in &tokens {
        if let Some((a, b)) = tok.split_once('-') {
            if let (Some(na), Some(nb)) = (parse_word_or_number(a), parse_word_or_number(b)) {
                nums.push((na + nb) / 2.0);
                continue;
            }
        }
        if let Some(n) = parse_word_or_number(tok) {
            nums.push(n);
        }
    }

    let qty = if nums.is_empty() {
        // Phrases like "half a day", "a day", "an hour", "several days"
        if lower.contains("half") {
            0.5
        } else if lower.contains("several") || lower.contains("few") {
            3.0
        } else if lower.contains("couple") {
            2.0
        } else {
            1.0
        }
    } else if nums.len() >= 2 && (lower.contains(" to ") || lower.contains(" or ")) {
        (nums[0] + nums[1]) / 2.0
    } else {
        nums[0]
    };

    Some(qty * unit_hours)
}

fn parse_word_or_number(tok: &str) -> Option<f64> {
    if let Ok(n) = tok.parse::<f64>() {
        return Some(n);
    }
    match tok {
        "half" => Some(0.5),
        "quarter" => Some(0.25),
        "one" | "single" => Some(1.0),
        "two" | "couple" => Some(2.0),
        "three" | "few" => Some(3.0),
        "four" => Some(4.0),
        "five" => Some(5.0),
        "six" => Some(6.0),
        "seven" => Some(7.0),
        "eight" => Some(8.0),
        "nine" => Some(9.0),
        "ten" => Some(10.0),
        "eleven" => Some(11.0),
        "twelve" | "dozen" => Some(12.0),
        "fifteen" => Some(15.0),
        "twenty" => Some(20.0),
        "thirty" => Some(30.0),
        _ => None,
    }
}
