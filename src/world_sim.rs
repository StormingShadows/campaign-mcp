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
    #[serde(alias = "item_name", alias = "name")]
    pub item_key: String,
    #[serde(default)]
    pub weight_kg: f64,
    #[serde(default, alias = "bulk")]
    pub volume_liters: f64,
    #[serde(default, alias = "base_price", alias = "price")]
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

/// Heuristic fallback item weight (in kg) when an item does not have an explicit `ItemEconomics` entry.
pub fn estimate_item_weight_kg(it: &crate::models::Item) -> f64 {
    let mut hay = it.name.to_ascii_lowercase();
    if let Some(ref sl) = it.slot {
        hay.push(' ');
        hay.push_str(&sl.to_ascii_lowercase());
    }
    for t in &it.tags {
        hay.push(' ');
        hay.push_str(&t.to_ascii_lowercase());
    }
    if hay.contains("plate") || hay.contains("heavy armor") || hay.contains("anvil") {
        18.0
    } else if hay.contains("chainmail") || hay.contains("scale mail") || hay.contains("medium armor") || hay.contains("timber") || hay.contains("beam") {
        10.0
    } else if hay.contains("greatsword") || hay.contains("halberd") || hay.contains("maul") || hay.contains("heavy weapon") || hay.contains("ingot") || hay.contains("ore") {
        4.5
    } else if hay.contains("leather armor") || hay.contains("brigandine") || hay.contains("shield") || hay.contains("tool kit") || hay.contains("carpenter") {
        3.5
    } else if hay.contains("sword") || hay.contains("saber") || hay.contains("axe") || hay.contains("hatchet") || hay.contains("spear") || hay.contains("bow") || hay.contains("rifle") || hay.contains("hammer") || hay.contains("saw") || hay.contains("weapon") || hay.contains("bedroll") || hay.contains("blanket") {
        1.4
    } else if hay.contains("jacket") || hay.contains("coat") || hay.contains("robe") || hay.contains("tunic") || hay.contains("trousers") || hay.contains("pants") || hay.contains("boots") || hay.contains("cloak") || hay.contains("clothing") || hay.contains("apparel") {
        0.8
    } else if hay.contains("coin") || hay.contains("cash") || hay.contains("tael") || hay.contains("scroll") || hay.contains("letter") || hay.contains("slip") || hay.contains("ring") || hay.contains("pill") || hay.contains("needle") {
        0.08
    } else {
        0.5
    }
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
            .unwrap_or_else(|| estimate_item_weight_kg(it));
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
        let sd_ratio = (c.demand.max(1) as f64) / (c.supply.max(1) as f64);
        let sd_mult = sd_ratio.powf(0.55).clamp(0.55, 2.50);
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

// ============================================================================
// 7. Stateful Progression System (Xianxia Realms 1-9, Parallel Manuals,
//    Sci-Fi Tiers, D&D Levels, Bottlenecks & Breakthroughs)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressionStageDef {
    /// Display name of this major realm, tier, or level bracket (e.g. "Qi Condensation", "Level 5", "Psionic Grade II").
    pub name: String,
    /// Number of sub-stages inside this major stage (e.g. 9 for Xianxia Layers 1..=9, 1 for flat D&D levels).
    #[serde(default = "default_sub_stages")]
    pub sub_stages: u32,
    /// Prefix for sub-stage formatting (e.g. "Layer", "Stage", "Grade"; empty omits sub-stage label when sub_stages == 1).
    #[serde(default = "default_sub_stage_prefix")]
    pub sub_stage_prefix: String,
    /// Optional explicit names for each sub-stage (e.g. ["Early", "Mid", "Late", "Peak"]).
    #[serde(default)]
    pub sub_stage_names: Vec<String>,
    /// XP required per sub-stage (used when `xp_thresholds` is empty).
    #[serde(default = "default_xp_per_sub")]
    pub xp_per_sub_stage: u64,
    /// Optional per-sub-stage XP costs (length == sub_stages) for non-linear curves.
    #[serde(default)]
    pub xp_thresholds: Vec<u64>,
    /// When true, advancing past the final sub-stage stops at a bottleneck until `attempt_breakthrough` succeeds!
    #[serde(default = "default_true_ws")]
    pub breakthrough_required_at_peak: bool,
    /// Grounded DC tier for breakthrough (e.g. "moderate", "hard", "very_hard", "nearly_impossible").
    #[serde(default)]
    pub breakthrough_dc_tier: Option<String>,
    /// Optional explicit breakthrough DC (overrides tier if set).
    #[serde(default)]
    pub breakthrough_dc: Option<i64>,
    /// Mechanical check modifier granted at this major stage (e.g. +1, +2, +4).
    #[serde(default)]
    pub passive_modifier: i64,
    /// Optional lifespan benchmark (for cultivation realms / longevity settings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifespan_years: Option<u32>,
    /// Narrative/mechanical properties unlocked upon entering this stage.
    #[serde(default)]
    pub properties: Vec<String>,
    /// Binding halt-conditions or prerequisites for this stage (e.g. Pavilion check-ins, tribulations, cyberware tolerances).
    #[serde(default)]
    pub halt_conditions: Vec<String>,
    /// Optional declarative state-machine bundle executed automatically when entering sub_stage 1 of this major stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage_rule: Option<crate::routines::StageTransitionRule>,
    /// Optional per-sub-stage (1-based) declarative rules executed automatically when entering each minor layer/grade.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sub_stage_rules: BTreeMap<u32, crate::routines::StageTransitionRule>,
}

fn default_sub_stages() -> u32 {
    9
}

fn default_sub_stage_prefix() -> String {
    "Layer".to_string()
}

fn default_xp_per_sub() -> u64 {
    100
}

fn default_true_ws() -> bool {
    true
}

impl ProgressionStageDef {
    pub fn xp_for_sub_stage(&self, sub_stage_1based: u32) -> u64 {
        let idx = sub_stage_1based.saturating_sub(1) as usize;
        if let Some(&v) = self.xp_thresholds.get(idx) {
            return v.max(1);
        }
        self.xp_per_sub_stage.max(1)
    }

    pub fn format_sub_stage(&self, sub_stage: u32) -> String {
        if self.sub_stages <= 1 {
            return String::new();
        }
        let idx = sub_stage.saturating_sub(1) as usize;
        if let Some(custom) = self.sub_stage_names.get(idx) {
            return custom.clone();
        }
        if self.sub_stage_prefix.trim().is_empty() {
            format!("{sub_stage}/{}", self.sub_stages)
        } else {
            format!("{} {sub_stage}/{}", self.sub_stage_prefix, self.sub_stages)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressionLadder {
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// e.g. "realm_ladder", "parallel_manual", "body_refinement", "character_level", "scifi_tier"
    #[serde(default = "default_ladder_kind")]
    pub kind: String,
    #[serde(default)]
    pub description: String,
    /// Optional linked StoryText ID if this ladder belongs to a specific cultivation manual / codex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_text_id: Option<String>,
    /// Optional linked CatalogItem ID if this manual is tracked as a physical carryable relic/tome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_item_id: Option<String>,
    /// Auto-resolved human-readable title of the linked manual/scroll.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manual_title: String,
    /// Canonical notes about cultivating this manual (meridian paths, affinity, incompatibilities, quirks).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manual_notes: String,
    /// Skills/tags where tracks on this ladder automatically contribute their `check_modifier`.
    #[serde(default)]
    pub default_check_tags: Vec<String>,
    pub stages: Vec<ProgressionStageDef>,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_ladder_kind() -> String {
    "realm_ladder".to_string()
}

impl ProgressionLadder {
    pub fn from_preset(preset: &str, custom_id: Option<&str>) -> anyhow::Result<Self> {
        let now = String::new();
        match preset.trim().to_lowercase().as_str() {
            "xianxia_standard" | "xianxia" | "cultivation" => Ok(Self {
                id: custom_id.unwrap_or("qi-cultivation").to_string(),
                name: "Orthodox Qi Cultivation Ladder (1-9 Layers)".to_string(),
                kind: "realm_ladder".to_string(),
                description: "Standard xianxia major realms with 9 minor layers each, bottleneck breakthroughs at Layer 9, and non-spendable cumulative Cultivation XP.".to_string(),
                source_text_id: None,
                source_item_id: None,
                manual_title: String::new(),
                manual_notes: String::new(),
                default_check_tags: vec!["qi-sense".into(), "meditation".into(), "cultivation".into(), "formation".into()],
                stages: vec![
                    ProgressionStageDef {
                        name: "Mortal".into(),
                        sub_stages: 1,
                        sub_stage_prefix: String::new(),
                        sub_stage_names: vec!["Unawakened".into()],
                        xp_per_sub_stage: 50,
                        xp_thresholds: vec![],
                        breakthrough_required_at_peak: true,
                        breakthrough_dc_tier: Some("moderate".into()),
                        breakthrough_dc: None,
                        passive_modifier: 0,
                        lifespan_years: Some(80),
                        properties: vec!["Mortal meridians (unopened)".into()],
                        halt_conditions: vec!["Must sense ambient qi and open initiating meridians".into()],
                        stage_rule: None,
                        sub_stage_rules: BTreeMap::new(),
                    },
                    ProgressionStageDef {
                        name: "Qi Condensation".into(),
                        sub_stages: 9,
                        sub_stage_prefix: "Layer".into(),
                        sub_stage_names: vec![],
                        xp_per_sub_stage: 100,
                        xp_thresholds: vec![100, 150, 200, 300, 400, 550, 750, 1000, 1300],
                        breakthrough_required_at_peak: true,
                        breakthrough_dc_tier: Some("hard".into()),
                        breakthrough_dc: None,
                        passive_modifier: 1,
                        lifespan_years: Some(120),
                        properties: vec!["Qi Sensing & Meridian Circulation".into(), "Basic Spiritual Sense".into()],
                        halt_conditions: vec!["Layer 9 peak requires Foundation Establishment breakthrough".into()],
                        stage_rule: None,
                        sub_stage_rules: BTreeMap::new(),
                    },
                    ProgressionStageDef {
                        name: "Foundation Establishment".into(),
                        sub_stages: 9,
                        sub_stage_prefix: "Layer".into(),
                        sub_stage_names: vec![],
                        xp_per_sub_stage: 1500,
                        xp_thresholds: vec![1500, 2000, 2500, 3200, 4000, 5000, 6200, 7600, 9200],
                        breakthrough_required_at_peak: true,
                        breakthrough_dc_tier: Some("very_hard".into()),
                        breakthrough_dc: None,
                        passive_modifier: 3,
                        lifespan_years: Some(220),
                        properties: vec!["Liquefied True Qi".into(), "Artifact Resonance & Short Flight".into()],
                        halt_conditions: vec!["Layer 9 peak requires Golden Core condensation".into()],
                        stage_rule: None,
                        sub_stage_rules: BTreeMap::new(),
                    },
                    ProgressionStageDef {
                        name: "Golden Core".into(),
                        sub_stages: 9,
                        sub_stage_prefix: "Layer".into(),
                        sub_stage_names: vec![],
                        xp_per_sub_stage: 12000,
                        xp_thresholds: vec![],
                        breakthrough_required_at_peak: true,
                        breakthrough_dc_tier: Some("very_hard".into()),
                        breakthrough_dc: Some(26),
                        passive_modifier: 5,
                        lifespan_years: Some(500),
                        properties: vec!["Crystallized Golden Core".into(), "Extended Spiritual Sense & Aerial Mastery".into()],
                        halt_conditions: vec!["Layer 9 peak requires Nascent Soul formation".into()],
                        stage_rule: None,
                        sub_stage_rules: BTreeMap::new(),
                    },
                    ProgressionStageDef {
                        name: "Nascent Soul".into(),
                        sub_stages: 9,
                        sub_stage_prefix: "Layer".into(),
                        sub_stage_names: vec![],
                        xp_per_sub_stage: 50000,
                        xp_thresholds: vec![],
                        breakthrough_required_at_peak: true,
                        breakthrough_dc_tier: Some("nearly_impossible".into()),
                        breakthrough_dc: None,
                        passive_modifier: 7,
                        lifespan_years: Some(1000),
                        properties: vec!["Manifest Nascent Soul".into(), "Domain Projection & Instant Teleportation".into()],
                        halt_conditions: vec![],
                        stage_rule: None,
                        sub_stage_rules: BTreeMap::new(),
                    },
                ],
                updated_at_world_time: now,
            }),
            "body_tempering" | "body_refinement" => Ok(Self {
                id: custom_id.unwrap_or("body-tempering").to_string(),
                name: "Parallel Body Tempering Discipline (1-9 Layers)".to_string(),
                kind: "body_refinement".to_string(),
                description: "Parallel physical refinement track practiced alongside qi cultivation; enhances physical resilience and exertion without replacing the main qi ladder.".to_string(),
                source_text_id: None,
                source_item_id: None,
                manual_title: String::new(),
                manual_notes: String::new(),
                default_check_tags: vec!["athletics".into(), "physical-exertion".into(), "body-tempering".into(), "endurance".into()],
                stages: vec![
                    ProgressionStageDef {
                        name: "Skin & Sinew Conditioning".into(),
                        sub_stages: 9,
                        sub_stage_prefix: "Layer".into(),
                        sub_stage_names: vec![],
                        xp_per_sub_stage: 80,
                        xp_thresholds: vec![40, 60, 90, 140, 210, 320, 480, 720, 1080],
                        breakthrough_required_at_peak: true,
                        breakthrough_dc_tier: Some("moderate".into()),
                        breakthrough_dc: None,
                        passive_modifier: 1,
                        lifespan_years: None,
                        properties: vec!["Hardened fascia & reduced fatigue".into()],
                        halt_conditions: vec!["Over-training without restorative diet causes strain injury".into()],
                        stage_rule: None,
                        sub_stage_rules: BTreeMap::new(),
                    },
                    ProgressionStageDef {
                        name: "Bone & Marrow Forging".into(),
                        sub_stages: 9,
                        sub_stage_prefix: "Layer".into(),
                        sub_stage_names: vec![],
                        xp_per_sub_stage: 250,
                        xp_thresholds: vec![150, 225, 340, 510, 760, 1140, 1710, 2560, 3840],
                        breakthrough_required_at_peak: true,
                        breakthrough_dc_tier: Some("hard".into()),
                        breakthrough_dc: None,
                        passive_modifier: 2,
                        lifespan_years: None,
                        properties: vec!["Dense skeletal frame & high impact tolerance".into()],
                        halt_conditions: vec![],
                        stage_rule: None,
                        sub_stage_rules: BTreeMap::new(),
                    },
                    ProgressionStageDef {
                        name: "Viscera & Blood Purification".into(),
                        sub_stages: 9,
                        sub_stage_prefix: "Layer".into(),
                        sub_stage_names: vec![],
                        xp_per_sub_stage: 800,
                        xp_thresholds: vec![500, 750, 1125, 1690, 2530, 3800, 5700, 8550, 12800],
                        breakthrough_required_at_peak: true,
                        breakthrough_dc_tier: Some("very_hard".into()),
                        breakthrough_dc: None,
                        passive_modifier: 4,
                        lifespan_years: None,
                        properties: vec!["High toxin resistance & rapid natural recovery".into()],
                        halt_conditions: vec![],
                        stage_rule: None,
                        sub_stage_rules: BTreeMap::new(),
                    },
                ],
                updated_at_world_time: now,
            }),
            "dnd_level_20" | "dnd5e" | "level_20" => {
                let xp_table: [u64; 20] = [
                    300, 600, 1800, 3800, 7500, 9000, 11000, 14000, 16000, 21000,
                    15000, 20000, 20000, 25000, 30000, 30000, 40000, 40000, 50000, 50000,
                ];
                let stages = (1u32..=20)
                    .map(|lvl| {
                        let prof = 2 + ((lvl - 1) / 4) as i64;
                        let asi_rule = if matches!(lvl, 4 | 8 | 12 | 16 | 19) {
                            Some(crate::routines::StageTransitionRule {
                                unspent_stat_points_delta: 2,
                                narrative_directive: format!(
                                    "Reached Level {lvl} Ability Score Improvement (+2 unspent stat points unlocked)."
                                ),
                                ..Default::default()
                            })
                        } else {
                            None
                        };
                        ProgressionStageDef {
                            name: format!("Level {lvl}"),
                            sub_stages: 1,
                            sub_stage_prefix: String::new(),
                            sub_stage_names: vec![],
                            xp_per_sub_stage: xp_table[(lvl - 1) as usize],
                            xp_thresholds: vec![],
                            breakthrough_required_at_peak: false,
                            breakthrough_dc_tier: None,
                            breakthrough_dc: None,
                            passive_modifier: prof,
                            lifespan_years: None,
                            properties: vec![format!("Proficiency Tier +{prof}")],
                            halt_conditions: vec![],
                            stage_rule: asi_rule,
                            sub_stage_rules: BTreeMap::new(),
                        }
                    })
                    .collect();
                Ok(Self {
                    id: custom_id.unwrap_or("character-level").to_string(),
                    name: "Standard RPG Character Level (1-20)".to_string(),
                    kind: "character_level".to_string(),
                    description: "Classic 1-20 level progression with automatic level-ups and scaling proficiency (+2 to +6).".to_string(),
                    source_text_id: None,
                    source_item_id: None,
                    manual_title: String::new(),
                    manual_notes: String::new(),
                    default_check_tags: vec![],
                    stages,
                    updated_at_world_time: now,
                })
            }
            "scifi_tier" | "scifi" | "cyberpunk" => Ok(Self {
                id: custom_id.unwrap_or("scifi-tier").to_string(),
                name: "Sci-Fi Operational / Neural Tier Ladder (Grades 1-5)".to_string(),
                kind: "scifi_tier".to_string(),
                description: "Modular sci-fi progression for psionic grades, neural-link clearance, or operative tiers with 5 sub-grades per tier.".to_string(),
                source_text_id: None,
                source_item_id: None,
                manual_title: String::new(),
                manual_notes: String::new(),
                default_check_tags: vec!["hacking".into(), "piloting".into(), "psionics".into(), "engineering".into()],
                stages: (1u32..=5)
                    .map(|tier| ProgressionStageDef {
                        name: format!("Tier {tier}"),
                        sub_stages: 5,
                        sub_stage_prefix: "Grade".into(),
                        sub_stage_names: vec![],
                        xp_per_sub_stage: 200 * (tier as u64),
                        xp_thresholds: vec![],
                        breakthrough_required_at_peak: true,
                        breakthrough_dc_tier: Some(match tier {
                            1 => "easy",
                            2 => "moderate",
                            3 => "hard",
                            4 => "very_hard",
                            _ => "nearly_impossible",
                        }.into()),
                        breakthrough_dc: None,
                        passive_modifier: tier as i64,
                        lifespan_years: None,
                        properties: vec![format!("Tier {tier} neural/clearance bandwidth unlocked")],
                        halt_conditions: vec![format!("Requires Tier {} certification/integration check at Grade 5", tier + 1)],
                        stage_rule: None,
                        sub_stage_rules: BTreeMap::new(),
                    })
                    .collect(),
                updated_at_world_time: now,
            }),
            other => anyhow::bail!(
                "Unknown progression ladder preset '{other}'. Supported: xianxia_standard, body_tempering, dnd_level_20, scifi_tier"
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterProgressionTrack {
    pub character_id: String,
    pub track_id: String,
    pub name: String,
    /// e.g. "primary_realm" | "parallel_manual" | "body_refinement" | "level" | "discipline"
    #[serde(default = "default_track_kind")]
    pub track_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ladder_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_text_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_item_id: Option<String>,
    /// Auto-resolved human-readable title of the linked cultivation manual/scroll.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manual_title: String,
    /// Notes specific to the particular manual being cultivated (breathing rhythm, meridian notes, incompatibilities, comprehension insights).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub manual_notes: String,
    #[serde(default)]
    pub major_stage_index: u32,
    pub major_stage_name: String,
    #[serde(default = "default_sub_one")]
    pub sub_stage: u32,
    #[serde(default = "default_sub_stages")]
    pub sub_stage_max: u32,
    #[serde(default)]
    pub sub_stage_label: String,
    #[serde(default)]
    pub current_xp: u64,
    #[serde(default = "default_xp_per_sub")]
    pub xp_to_next: u64,
    #[serde(default)]
    pub cumulative_xp: u64,
    #[serde(default)]
    pub at_bottleneck: bool,
    #[serde(default)]
    pub bottleneck_reason: String,
    #[serde(default)]
    pub halt_conditions: Vec<String>,
    #[serde(default)]
    pub active_properties: Vec<String>,
    #[serde(default)]
    pub check_modifier: i64,
    #[serde(default)]
    pub check_tags: Vec<String>,
    /// "active" | "stalled" | "halted" | "completed" | "crippled"
    #[serde(default = "default_track_status")]
    pub status: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub gm_notes: String,
    /// Optional per-track declarative stage rules keyed by `"<sub_stage>"` (e.g. `"6"`) or `"<major_idx>:<sub_stage>"` (e.g. `"0:6"`).
    /// Overrides any ladder-wide `ProgressionStageDef` rule for manual-specific transformations or curses!
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub stage_rules: BTreeMap<String, crate::routines::StageTransitionRule>,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_track_kind() -> String {
    "primary_realm".to_string()
}

fn default_sub_one() -> u32 {
    1
}

fn default_track_status() -> String {
    "active".to_string()
}

impl CharacterProgressionTrack {
    pub fn format_stage(&self) -> String {
        if self.sub_stage_max <= 1 {
            self.major_stage_name.clone()
        } else if !self.sub_stage_label.is_empty() {
            format!("{} ({})", self.major_stage_name, self.sub_stage_label)
        } else {
            format!(
                "{} (Layer {}/{})",
                self.major_stage_name, self.sub_stage, self.sub_stage_max
            )
        }
    }
}

// ============================================================================
// 8. Sparse Multi-Scale 2D (x, y) Map Grids & Compass Neighborhood Helpers
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapGrid {
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// "world" | "region" | "settlement" | "dungeon" | "space_sector" | "tactical"
    #[serde(default = "default_grid_scale_kind")]
    pub scale_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_place_id: Option<String>,
    #[serde(default = "default_grid_w")]
    pub width: u32,
    #[serde(default = "default_grid_h")]
    pub height: u32,
    /// Human-readable scale per cell (e.g. "5 km", "100 m", "1 AU", "10 ft").
    #[serde(default = "default_cell_scale")]
    pub cell_scale_label: String,
    /// Default biome/terrain inherited by unpainted sparse cells (avoids empty-grid tax!).
    #[serde(default = "default_grid_terrain")]
    pub default_terrain: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_grid_scale_kind() -> String {
    "region".to_string()
}

fn default_grid_w() -> u32 {
    16
}

fn default_grid_h() -> u32 {
    12
}

fn default_cell_scale() -> String {
    "5 km".to_string()
}

fn default_grid_terrain() -> String {
    "wilderness".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MapGridCell {
    pub grid_id: String,
    pub x: i32,
    pub y: i32,
    #[serde(default = "default_grid_terrain")]
    pub terrain: String,
    #[serde(default)]
    pub elevation: i32,
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place_id: Option<String>,
    #[serde(default)]
    pub landmark_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub faction_control: Option<String>,
    /// "charted" | "surveyed" | "glimpsed" | "unexplored"
    #[serde(default = "default_fog_status")]
    pub fog_status: String,
    #[serde(default = "default_true")]
    pub passable: bool,
    #[serde(default)]
    pub hazards: Vec<String>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub gm_notes: String,
    #[serde(default)]
    pub updated_at_world_time: String,
}

fn default_fog_status() -> String {
    "charted".to_string()
}

/// Converts a 2D map offset `(dx, dy)` (with +x = East, +y = South) into an 8-point compass bearing.
pub fn compass_bearing(dx: i32, dy: i32) -> &'static str {
    if dx == 0 && dy == 0 {
        return "HERE";
    }
    let angle = (dy as f64).atan2(dx as f64).to_degrees(); // 0=E, 90=S, -90=N, +-180=W
    if (-22.5..22.5).contains(&angle) {
        "E"
    } else if (22.5..67.5).contains(&angle) {
        "SE"
    } else if (67.5..112.5).contains(&angle) {
        "S"
    } else if (112.5..157.5).contains(&angle) {
        "SW"
    } else if !(-157.5..157.5).contains(&angle) {
        "W"
    } else if (-157.5..-112.5).contains(&angle) {
        "NW"
    } else if (-112.5..-67.5).contains(&angle) {
        "N"
    } else {
        "NE"
    }
}

/// Produces a compact, token-safe compass neighborhood summary around `(cx, cy)` for LLM context & docs.
/// Strictly strips `gm_notes` and conceals `unexplored` cells when `include_gm_secrets == false`.
pub fn format_grid_neighborhood_lines(
    grid: &MapGrid,
    cells: &[MapGridCell],
    cx: i32,
    cy: i32,
    include_gm_secrets: bool,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!(
        "**Map Grid:** {} (`{}` | {} | {}x{}, 1 cell = {} | default: {}) — Position `({}, {})`",
        grid.name,
        grid.id,
        grid.scale_kind,
        grid.width,
        grid.height,
        grid.cell_scale_label,
        grid.default_terrain,
        cx,
        cy
    ));

    // 1. Immediate 3x3 adjacent non-empty cells (N, NE, E, SE, S, SW, W, NW)
    let dirs = [
        (0, -1, "N"),
        (1, -1, "NE"),
        (1, 0, "E"),
        (1, 1, "SE"),
        (0, 1, "S"),
        (-1, 1, "SW"),
        (-1, 0, "W"),
        (-1, -1, "NW"),
    ];
    let mut adj = Vec::new();
    for (dx, dy, lbl) in dirs {
        if let Some(c) = cells.iter().find(|c| c.x == cx + dx && c.y == cy + dy) {
            if !include_gm_secrets && c.fog_status.eq_ignore_ascii_case("unexplored") {
                adj.push(format!("{lbl} ({},{}): Unexplored", c.x, c.y));
                continue;
            }
            let mut desc = c.terrain.clone();
            if let Some(ref pid) = c.place_id {
                desc.push_str(&format!(" [Place: `{pid}`]"));
            }
            if !c.landmark_name.is_empty() {
                desc.push_str(&format!(" [Landmark: {}]", c.landmark_name));
            } else if !c.label.is_empty() {
                desc.push_str(&format!(" [{}]", c.label));
            }
            if !c.passable {
                desc.push_str(" (IMPASSABLE)");
            }
            if !c.hazards.is_empty() {
                desc.push_str(&format!(" (Hazards: {})", c.hazards.join(", ")));
            }
            if include_gm_secrets && !c.gm_notes.is_empty() {
                desc.push_str(&format!(" *(GM: {})*", c.gm_notes));
            }
            adj.push(format!("{lbl} ({},{}): {desc}", c.x, c.y));
        }
    }
    if !adj.is_empty() {
        lines.push(format!("- **Adjacent Cells (3x3):** {}", adj.join(" | ")));
    }

    // 2. Nearest placed cities / landmarks on the same grid (beyond immediate cell)
    let mut distant: Vec<(f64, String)> = Vec::new();
    for c in cells {
        if c.x == cx && c.y == cy {
            continue;
        }
        if c.place_id.is_none() && c.landmark_name.is_empty() && c.label.is_empty() {
            continue;
        }
        if !include_gm_secrets && c.fog_status.eq_ignore_ascii_case("unexplored") {
            continue;
        }
        let dx = c.x - cx;
        let dy = c.y - cy;
        let dist = ((dx * dx + dy * dy) as f64).sqrt();
        if dist <= 1.5 {
            continue; // already covered in 3x3 adjacent list
        }
        let bearing = compass_bearing(dx, dy);
        let name = if let Some(ref pid) = c.place_id {
            if !c.label.is_empty() {
                format!("{} (`{pid}`)", c.label)
            } else {
                format!("`{pid}`")
            }
        } else if !c.landmark_name.is_empty() {
            c.landmark_name.clone()
        } else {
            c.label.clone()
        };
        distant.push((
            dist,
            format!("{name} at ({},{}) — {:.1} cells {bearing}", c.x, c.y, dist),
        ));
    }
    distant.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
    if !distant.is_empty() {
        let top: Vec<String> = distant.into_iter().take(6).map(|(_, s)| s).collect();
        lines.push(format!("- **Bearings to Nearby Landmarks/Places:** {}", top.join(" | ")));
    }
    lines
}
