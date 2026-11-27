use crate::autonomy::{evaluate_garment_fit, CharacterOutfit, GridPosition};
use crate::engine::CampaignEngine;
use crate::models::{slugify, GraphRelation};
use crate::projection::load_character_projection;
use crate::world_sim::{compute_encumbrance, ItemEconomics};
use serde_json::{json, Value};

fn normalize_currency_simple(c: &str) -> String {
    c.trim().to_ascii_lowercase().replace(' ', "_")
}

fn outfit_item_slots(outfit: &CharacterOutfit) -> Vec<&str> {
    [
        outfit.headwear.as_str(),
        outfit.outer_layer.as_str(),
        outfit.top_layer.as_str(),
        outfit.bottoms.as_str(),
        outfit.footwear.as_str(),
        outfit.accessories.as_str(),
    ]
    .into_iter()
    .flat_map(|s| s.split(&[',', ';', '+'][..]))
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .collect()
}

fn prune_item_from_outfit(outfit: &mut CharacterOutfit, item_name: &str) -> bool {
    let target = item_name.trim().to_ascii_lowercase();
    if target.is_empty() {
        return false;
    }
    let mut changed = false;
    for slot in [
        &mut outfit.headwear,
        &mut outfit.outer_layer,
        &mut outfit.top_layer,
        &mut outfit.bottoms,
        &mut outfit.footwear,
        &mut outfit.accessories,
    ] {
        if slot.trim().is_empty() {
            continue;
        }
        if slot.trim().eq_ignore_ascii_case(&target) {
            slot.clear();
            changed = true;
        } else if slot.contains(',') {
            let parts: Vec<&str> = slot
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case(&target))
                .collect();
            let joined = parts.join(", ");
            if joined != slot.trim() {
                *slot = joined;
                changed = true;
            }
        }
    }
    changed
}

/// Automatic 1st-order and 2nd-order relative synchronization helpers for `CampaignEngine`.
/// Keeps interconnected Chronicle state (Inventory <-> ItemEconomics <-> CatalogItem <->
/// Outfits <-> Garment Fit <-> Encumbrance; Movement <-> Map Fog-of-War <-> NPC Trips;
/// Character Status <-> Routines <-> NPC Trips; Place Control/Routes <-> Factions <->
/// Relations <-> Travel Graph) consistent automatically so the GM model doesn't have to
/// juggle 3-6 manual follow-up calls per mutation.
impl CampaignEngine {
    fn resolve_cid(&self, id_or_name: &str) -> Option<String> {
        self.get_character(id_or_name, None)
            .ok()
            .map(|p| p.baseline.id)
    }

    fn resolve_pid(&self, id_or_name: &str) -> String {
        self.get_place(id_or_name, None)
            .ok()
            .map(|p| p.baseline.id)
            .unwrap_or_else(|| slugify(id_or_name))
    }

    /// Upserts `ItemEconomics` (weight_kg / unit_value / currency) while preserving untouched fields.
    pub fn sync_item_economics_fields(
        &self,
        item_name: &str,
        weight_kg: Option<f64>,
        base_value: Option<i64>,
        currency: Option<&str>,
        volume_liters: Option<f64>,
    ) -> Option<String> {
        if weight_kg.is_none()
            && base_value.is_none()
            && volume_liters.is_none()
            && currency.is_none()
        {
            return None;
        }
        let key = item_name.trim().to_ascii_lowercase();
        if key.is_empty() {
            return None;
        }
        let existing_map = self.db().list_item_economics_map().unwrap_or_default();
        let mut econ = existing_map
            .get(&key)
            .cloned()
            .unwrap_or_else(|| ItemEconomics {
                item_key: key.clone(),
                weight_kg: 0.0,
                volume_liters: 0.0,
                unit_value: 0,
                currency: "gold".to_string(),
                stackable: true,
                quality: "standard".to_string(),
                requirements: String::new(),
            });
        if let Some(w) = weight_kg {
            econ.weight_kg = w.max(0.0);
        }
        if let Some(v) = base_value {
            econ.unit_value = v.max(0);
        }
        if let Some(c) = currency.filter(|s| !s.trim().is_empty()) {
            econ.currency = normalize_currency_simple(c);
        }
        if let Some(vol) = volume_liters {
            econ.volume_liters = vol.max(0.0);
        }
        if self.db().upsert_item_economics(&econ).is_ok() {
            Some(format!(
                "Synced item_economics['{}'] (weight: {:.2} kg, value: {} {})",
                item_name, econ.weight_kg, econ.unit_value, econ.currency
            ))
        } else {
            None
        }
    }

    /// 1st- and 2nd-order cascade after `update_character_equipment` (acquire/tailor/equip/unequip/remove).
    pub fn cascade_after_equipment_change(
        &self,
        character_id: &str,
        item_name: &str,
        action: &str,
        weight_kg: Option<f64>,
        base_value: Option<i64>,
        currency: Option<&str>,
        volume_liters: Option<f64>,
    ) -> (Vec<String>, Option<Value>) {
        let mut cascades = Vec::new();
        let Some(cid) = self.resolve_cid(character_id) else {
            return (cascades, None);
        };
        let Ok(proj) = load_character_projection(self.db(), &cid, None) else {
            return (cascades, None);
        };
        let live_item = proj
            .current
            .equipment
            .iter()
            .find(|i| i.name.eq_ignore_ascii_case(item_name))
            .cloned();
        let removed_to_zero = matches!(action, "remove" | "consume") && live_item.is_none();

        // 1st-order A: Sync item_economics if any economic/weight fields were supplied
        if let Some(note) = self.sync_item_economics_fields(
            item_name,
            weight_kg,
            base_value,
            currency,
            volume_liters,
        ) {
            cascades.push(note);
        }

        // 1st-order B: Sync matching CatalogItem held by this character
        let item_slug = slugify(item_name);
        if let Ok(catalog) = self.db().list_catalog_items() {
            if let Some(mut cat) = catalog.into_iter().find(|c| {
                c.current_holder_id.as_deref() == Some(cid.as_str())
                    && (c.id == item_slug || c.name.eq_ignore_ascii_case(item_name))
            }) {
                let now = self
                    .db()
                    .get_campaign_meta()
                    .map(|m| m.clock.format_short())
                    .unwrap_or_default();
                cat.updated_at_world_time = now;
                if removed_to_zero {
                    if !cat.previous_owners.iter().any(|p| p == &cid) {
                        cat.previous_owners.push(cid.clone());
                    }
                    cat.current_holder_id = None;
                    cat.equipped = false;
                    if self.db().upsert_catalog_item(&cat).is_ok() {
                        cascades.push(format!(
                            "Detached matching catalog item '{}' from '{}' (recorded in previous_owners)",
                            cat.id, cid
                        ));
                    }
                    if let Some(ref tid) = cat.linked_text_id {
                        if let Ok(Some(mut txt)) = self.db().get_text(tid) {
                            txt.holder_or_location_id = None;
                            let _ = self.db().upsert_text(&txt);
                            cascades.push(format!(
                                "Updated linked text '{}' holder -> none",
                                txt.id
                            ));
                        }
                    }
                } else if let Some(ref live) = live_item {
                    let mut changed = false;
                    if cat.equipped != live.equipped {
                        cat.equipped = live.equipped;
                        changed = true;
                    }
                    if !live.garment_measurements.is_empty()
                        && cat.garment_measurements != live.garment_measurements
                    {
                        cat.garment_measurements = live.garment_measurements.clone();
                        changed = true;
                    }
                    if !live.garment_cut.is_empty() && cat.garment_cut != live.garment_cut {
                        cat.garment_cut = live.garment_cut.clone();
                        changed = true;
                    }
                    if !live.garment_taper.is_empty() && cat.garment_taper != live.garment_taper {
                        cat.garment_taper = live.garment_taper.clone();
                        changed = true;
                    }
                    if !live.tags.is_empty() && cat.tags != live.tags {
                        cat.tags = live.tags.clone();
                        changed = true;
                    }
                    if let Some(ref dur) = live.durability {
                        if !dur.is_empty() && cat.condition_or_durability != *dur {
                            cat.condition_or_durability = dur.clone();
                            changed = true;
                        }
                    }
                    if changed && self.db().upsert_catalog_item(&cat).is_ok() {
                        cascades.push(format!(
                            "Synced matching catalog item '{}' (equipped={}, sizing/tags/durability)",
                            cat.id, cat.equipped
                        ));
                    }
                }
            }
        }

        // 2nd-order A: If removed to 0, prune dead item name from pc_sheet outfits
        if removed_to_zero {
            if let Ok(mut prof) = self.get_character_profile(&cid) {
                let mut pruned_outfits = Vec::new();
                for outfit in &mut prof.avatar.outfits {
                    if prune_item_from_outfit(outfit, item_name) {
                        pruned_outfits.push(outfit.name.clone());
                    }
                }
                if !pruned_outfits.is_empty() && self.db().upsert_character_profile(&prof).is_ok() {
                    cascades.push(format!(
                        "Pruned removed item '{}' from outfit(s): {}",
                        item_name,
                        pruned_outfits.join(", ")
                    ));
                }
            }
        }

        // 2nd-order B: Compute live encumbrance snapshot
        let econ_map = self.db().list_item_economics_map().unwrap_or_default();
        let enc = compute_encumbrance(&proj, &econ_map);
        let enc_val = json!({
            "total_weight_kg": enc.total_weight_kg,
            "stored_weight_kg": enc.stored_weight_kg,
            "carry_capacity_kg": enc.carry_capacity_kg,
            "load_tier": enc.load_tier,
            "physical_check_penalty": enc.physical_check_penalty
        });
        if enc.load_tier == "heavy" || enc.load_tier == "overloaded" {
            cascades.push(format!(
                "Encumbrance alert: '{}' is now {} ({:.2}/{:.1} kg, physical penalty {:+})",
                cid, enc.load_tier, enc.total_weight_kg, enc.carry_capacity_kg, enc.physical_check_penalty
            ));
        }

        (cascades, Some(enc_val))
    }

    /// 1st- and 2nd-order cascade after `transfer_item(from -> to)`.
    pub fn cascade_after_item_transfer(
        &self,
        from_id: &str,
        to_id: &str,
        item_name: &str,
    ) -> Vec<String> {
        let mut cascades = Vec::new();
        let Some(fid) = self.resolve_cid(from_id) else {
            return cascades;
        };
        let Some(tid) = self.resolve_cid(to_id) else {
            return cascades;
        };
        let item_slug = slugify(item_name);

        // 1st-order: Transfer matching CatalogItem (and linked StoryText) if held by sender
        if let Ok(catalog) = self.db().list_catalog_items() {
            if let Some(mut cat) = catalog.into_iter().find(|c| {
                c.current_holder_id.as_deref() == Some(fid.as_str())
                    && (c.id == item_slug || c.name.eq_ignore_ascii_case(item_name))
            }) {
                let now = self
                    .db()
                    .get_campaign_meta()
                    .map(|m| m.clock.format_short())
                    .unwrap_or_default();
                if !cat.previous_owners.iter().any(|p| p == &fid) {
                    cat.previous_owners.push(fid.clone());
                }
                cat.current_holder_id = Some(tid.clone());
                cat.equipped = false;
                cat.updated_at_world_time = now;
                if self.db().upsert_catalog_item(&cat).is_ok() {
                    cascades.push(format!(
                        "Transferred matching catalog item '{}' holder '{}' -> '{}'",
                        cat.id, fid, tid
                    ));
                }
                if let Some(ref text_id) = cat.linked_text_id {
                    if let Ok(Some(mut txt)) = self.db().get_text(text_id) {
                        txt.holder_or_location_id = Some(tid.clone());
                        let _ = self.db().upsert_text(&txt);
                        cascades.push(format!(
                            "Synced linked text '{}' holder -> '{}'",
                            txt.id, tid
                        ));
                    }
                }
            }
        }

        // 2nd-order: Prune from sender's outfits if sender no longer carries it
        if let Ok(from_proj) = load_character_projection(self.db(), &fid, None) {
            let still_has = from_proj
                .current
                .equipment
                .iter()
                .any(|i| i.name.eq_ignore_ascii_case(item_name));
            if !still_has {
                if let Ok(mut prof) = self.get_character_profile(&fid) {
                    let mut pruned = Vec::new();
                    for outfit in &mut prof.avatar.outfits {
                        if prune_item_from_outfit(outfit, item_name) {
                            pruned.push(outfit.name.clone());
                        }
                    }
                    if !pruned.is_empty() && self.db().upsert_character_profile(&prof).is_ok() {
                        cascades.push(format!(
                            "Pruned '{}' from {}'s outfit(s): {}",
                            item_name,
                            fid,
                            pruned.join(", ")
                        ));
                    }
                }
            }
        }

        cascades
    }

    /// 2nd-order cascade after `update_avatar`: re-evaluates all equipped garments against
    /// updated measurements and surfaces any garment fit alerts immediately.
    pub fn cascade_after_avatar_update(&self, character_id: &str) -> (Vec<String>, Vec<Value>) {
        let mut cascades = Vec::new();
        let mut fit_reports = Vec::new();
        let Some(cid) = self.resolve_cid(character_id) else {
            return (cascades, fit_reports);
        };
        let Ok(proj) = load_character_projection(self.db(), &cid, None) else {
            return (cascades, fit_reports);
        };
        let Ok(prof) = self.get_character_profile(&cid) else {
            return (cascades, fit_reports);
        };
        for it in proj.current.equipment.iter().filter(|i| i.equipped) {
            if let Some(rep) = evaluate_garment_fit(it, &prof.avatar) {
                if rep.fit_status != "fits_well" {
                    cascades.push(format!(
                        "Garment fit cascade: '{}' is now '{}' ({})",
                        it.name, rep.fit_status, rep.summary
                    ));
                }
                if let Ok(v) = serde_json::to_value(&rep) {
                    fit_reports.push(v);
                }
            }
        }
        (cascades, fit_reports)
    }

    /// 1st- & 2nd-order cascade after `set_active_outfit`: optionally auto-equips items in the
    /// active outfit's clothing slots and unequips clothing pieces belonging exclusively to other
    /// outfits, then evaluates fit for every newly equipped garment.
    pub fn cascade_after_outfit_change(
        &self,
        character_id: &str,
        outfit_id: &str,
        sync_equipment: bool,
    ) -> (Vec<String>, Vec<Value>) {
        let mut cascades = Vec::new();
        if !sync_equipment {
            return self.cascade_after_avatar_update(character_id);
        }
        let Some(cid) = self.resolve_cid(character_id) else {
            return (cascades, Vec::new());
        };
        let Ok(prof) = self.get_character_profile(&cid) else {
            return (cascades, Vec::new());
        };
        let Some(active) = prof
            .avatar
            .outfits
            .iter()
            .find(|o| o.id == outfit_id || o.name.eq_ignore_ascii_case(outfit_id))
        else {
            return (cascades, Vec::new());
        };
        let active_items_l: Vec<String> = outfit_item_slots(active)
            .into_iter()
            .map(|s| s.to_ascii_lowercase())
            .collect();
        let other_outfit_items_l: Vec<String> = prof
            .avatar
            .outfits
            .iter()
            .filter(|o| o.id != active.id)
            .flat_map(|o| outfit_item_slots(o).into_iter().map(|s| s.to_ascii_lowercase()))
            .collect();

        if let Ok(proj) = load_character_projection(self.db(), &cid, None) {
            let mut equipped_now = Vec::new();
            let mut unequipped_now = Vec::new();
            for inv_item in &proj.current.equipment {
                let nl = inv_item.name.trim().to_ascii_lowercase();
                let matches_active = active_items_l
                    .iter()
                    .any(|x| x == &nl || x.contains(&nl) || nl.contains(x.as_str()));
                let matches_other = other_outfit_items_l
                    .iter()
                    .any(|x| x == &nl || x.contains(&nl) || nl.contains(x.as_str()));
                if matches_active {
                    if !inv_item.equipped {
                        if self
                            .equip_item(&cid, &inv_item.name, inv_item.slot.clone(), true)
                            .is_ok()
                        {
                            equipped_now.push(inv_item.name.clone());
                        }
                    }
                } else if inv_item.equipped && matches_other {
                    if self
                        .equip_item(&cid, &inv_item.name, inv_item.slot.clone(), false)
                        .is_ok()
                    {
                        unequipped_now.push(inv_item.name.clone());
                    }
                }
            }
            if !equipped_now.is_empty() {
                cascades.push(format!(
                    "Auto-equipped outfit items: {}",
                    equipped_now.join(", ")
                ));
            }
            if !unequipped_now.is_empty() {
                cascades.push(format!(
                    "Auto-unequipped replaced outfit items: {}",
                    unequipped_now.join(", ")
                ));
            }
        }

        let (fit_cascades, fit_reports) = self.cascade_after_avatar_update(&cid);
        cascades.extend(fit_cascades);
        (cascades, fit_reports)
    }

    /// 2nd-order cascade after `move_character`:
    /// 1) For PCs, auto-charts any 2D map grid cell linked to `to_place_id`.
    /// 2) Clears stale active `npc_movements` trip on manual move & syncs `current_grid_pos`.
    pub fn cascade_after_move_character(
        &self,
        character_id: &str,
        to_place_id: &str,
    ) -> Vec<String> {
        let mut cascades = Vec::new();
        let Some(cid) = self.resolve_cid(character_id) else {
            return cascades;
        };
        let Ok(proj) = load_character_projection(self.db(), &cid, None) else {
            return cascades;
        };
        let pid = self.resolve_pid(to_place_id);

        // 2nd-order A: Reveal fog-of-war on any 2D map grid cell linked to this place if mover is a PC
        let mut mapped_pos: Option<(String, i32, i32, String, String)> = None;
        if let Ok(grids) = self.db().list_map_grids() {
            for g in grids {
                if let Ok(cells) = self.db().list_map_cells(&g.id) {
                    for mut cell in cells {
                        if cell.place_id.as_deref() == Some(pid.as_str()) {
                            if mapped_pos.is_none() {
                                mapped_pos = Some((
                                    g.id.clone(),
                                    cell.x,
                                    cell.y,
                                    cell.terrain.clone(),
                                    cell.label.clone(),
                                ));
                            }
                            if proj.baseline.kind.eq_ignore_ascii_case("pc")
                                && cell.fog_status != "charted"
                            {
                                let prev = cell.fog_status.clone();
                                cell.fog_status = "charted".to_string();
                                if self.db().upsert_map_cell(&cell).is_ok() {
                                    cascades.push(format!(
                                        "Auto-charted map cell {} ({},{}) fog '{}' -> 'charted'",
                                        g.id, cell.x, cell.y, prev
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }

        // 2nd-order B: Clear stale active NPC mobility trip & update grid pos if profile tracks mobility
        if let Ok(mut prof) = self.get_character_profile(&cid) {
            let mut changed = false;
            if prof.mobility.active_trip.is_some() {
                prof.mobility.active_trip = None;
                prof.mobility.current_excursion_note = None;
                let at_home = prof.mobility.home_place_id.as_deref() == Some(pid.as_str());
                prof.mobility.current_status = if at_home {
                    "at_home".to_string()
                } else {
                    "visiting".to_string()
                };
                changed = true;
                cascades.push(format!(
                    "Cleared stale in-transit NPC mobility trip on '{}' (status -> '{}')",
                    cid, prof.mobility.current_status
                ));
            }
            if let Some((ref gid, gx, gy, ref terrain, ref label)) = mapped_pos {
                if prof.mobility.roam_grid_id.as_deref() == Some(gid.as_str()) {
                    prof.mobility.current_grid_pos = Some(GridPosition {
                        grid_id: gid.clone(),
                        x: gx,
                        y: gy,
                        terrain: terrain.clone(),
                        label: label.clone(),
                    });
                    changed = true;
                }
            }
            if changed {
                let _ = self.db().upsert_character_profile(&prof);
            }
        }

        cascades
    }

    /// 2nd-order cascade after `set_character_status`:
    /// Disables active standing routines and cancels active NPC mobility trips when a character
    /// becomes dead, deceased, incapacitated, unconscious, or missing.
    pub fn cascade_after_status_change(
        &self,
        character_id: &str,
        alive_status: &str,
    ) -> Vec<String> {
        let mut cascades = Vec::new();
        let st = alive_status.trim().to_ascii_lowercase();
        let is_dead = matches!(st.as_str(), "dead" | "deceased" | "killed" | "slain");
        let is_halted = is_dead
            || matches!(
                st.as_str(),
                "incapacitated" | "unconscious" | "comatose" | "imprisoned" | "missing"
            );
        if !is_halted {
            return cascades;
        }
        let Some(cid) = self.resolve_cid(character_id) else {
            return cascades;
        };

        // 2nd-order A: Pause standing routines (all if dead; non-gm_secret active work/training if incapacitated)
        if let Ok(routines) = self.db().list_routines(Some(&cid)) {
            let mut paused = Vec::new();
            for mut r in routines {
                if !r.enabled {
                    continue;
                }
                if !is_dead && r.visibility.eq_ignore_ascii_case("gm_secret") {
                    continue;
                }
                r.enabled = false;
                if self.db().upsert_routine(&r).is_ok() {
                    paused.push(r.name.clone());
                }
            }
            if !paused.is_empty() {
                cascades.push(format!(
                    "Auto-paused {} standing routine(s) due to status '{}': {}",
                    paused.len(),
                    alive_status,
                    paused.join(", ")
                ));
            }
        }

        // 2nd-order B: Cancel active NPC mobility trip
        if let Ok(mut prof) = self.get_character_profile(&cid) {
            if prof.mobility.active_trip.is_some()
                || (is_dead && prof.mobility.mobility_mode != "static")
            {
                prof.mobility.active_trip = None;
                prof.mobility.current_status = if is_dead {
                    "inactive".to_string()
                } else {
                    "stranded".to_string()
                };
                if is_dead {
                    prof.mobility.mobility_mode = "static".to_string();
                }
                if self.db().upsert_character_profile(&prof).is_ok() {
                    cascades.push(format!(
                        "Halted NPC mobility for '{}' (status -> '{}')",
                        cid, prof.mobility.current_status
                    ));
                }
            }
        }

        cascades
    }

    /// 1st- and 2nd-order cascade after `update_place(action='control')`:
    /// Keeps `FactionRecord.territory` and social-graph `controls` relations synchronized.
    pub fn cascade_after_place_control(
        &self,
        place_id: &str,
        new_faction: Option<&str>,
        old_faction: Option<&str>,
    ) -> Vec<String> {
        let mut cascades = Vec::new();
        let pid = self.resolve_pid(place_id);
        let Ok(factions) = self.db().list_factions() else {
            return cascades;
        };
        let now = self
            .db()
            .get_campaign_meta()
            .map(|m| m.clock.format_short())
            .unwrap_or_default();

        let find_faction = |label: &str| {
            let sl = slugify(label);
            factions
                .iter()
                .find(|f| f.id == sl || f.name.eq_ignore_ascii_case(label))
                .cloned()
        };

        // Remove stale control from old faction if it changed
        if let Some(old_f_name) = old_faction.filter(|s| !s.trim().is_empty()) {
            let changed = new_faction
                .map(|nf| !nf.eq_ignore_ascii_case(old_f_name) && slugify(nf) != slugify(old_f_name))
                .unwrap_or(false);
            if changed {
                if let Some(mut old_fac) = find_faction(old_f_name) {
                    let before = old_fac.territory.len();
                    old_fac.territory.retain(|p| p != &pid);
                    if old_fac.territory.len() < before {
                        let _ = self.db().upsert_faction(&old_fac);
                        cascades.push(format!(
                            "Removed '{}' from previous faction '{}'.territory",
                            pid, old_fac.id
                        ));
                    }
                    let _ = self.db().delete_relation(&old_fac.id, "controls", &pid);
                }
            }
        }

        // Sync new controlling faction + social graph `controls` relation
        if let Some(new_f_name) = new_faction.filter(|s| !s.trim().is_empty()) {
            if let Some(mut new_fac) = find_faction(new_f_name) {
                if !new_fac.territory.iter().any(|p| p == &pid) {
                    new_fac.territory.push(pid.clone());
                    let _ = self.db().upsert_faction(&new_fac);
                    cascades.push(format!(
                        "Added '{}' to faction '{}'.territory",
                        pid, new_fac.id
                    ));
                }
                let rel = GraphRelation {
                    source_id: new_fac.id.clone(),
                    source_kind: "faction".to_string(),
                    relation_type: "controls".to_string(),
                    target_id: pid.clone(),
                    target_kind: "place".to_string(),
                    strength: Some(80),
                    notes: format!("Controls {}", pid),
                    updated_at_world_time: now,
                    dimensions: Default::default(),
                    direction: "directed".to_string(),
                    visibility: "public".to_string(),
                    obligations: Vec::new(),
                    origin_event_id: None,
                };
                if self.db().upsert_relation(&rel).is_ok() {
                    cascades.push(format!(
                        "Synced graph relation: faction '{}' --[controls]--> place '{}'",
                        new_fac.id, pid
                    ));
                }
            }
        }

        cascades
    }

    /// 1st- and 2nd-order cascade after `update_place(action='route')`:
    /// Synchronizes matching `TravelRoute` edges in the Dijkstra travel graph and warns if
    /// blocking/destroying the route disrupts any active NPC travel trips.
    pub fn cascade_after_place_route_change(
        &self,
        from_place_id: &str,
        to_place_id: &str,
        status: &str,
    ) -> Vec<String> {
        let mut cascades = Vec::new();
        let from_pid = self.resolve_pid(from_place_id);
        let to_pid = self.resolve_pid(to_place_id);
        let norm_status = match status.trim().to_ascii_lowercase().as_str() {
            "blocked" | "closed" | "impassable" => "blocked",
            "destroyed" | "collapsed" | "washed_out" => "destroyed",
            "hazardous" | "dangerous" | "contested" => "hazardous",
            _ => "open",
        };

        if let Ok(routes) = self.db().list_travel_routes() {
            for mut tr in routes {
                let matches = (tr.from_place_id == from_pid && tr.to_place_id == to_pid)
                    || (tr.bidirectional
                        && tr.from_place_id == to_pid
                        && tr.to_place_id == from_pid);
                if matches && tr.status != norm_status {
                    let old_s = tr.status.clone();
                    tr.status = norm_status.to_string();
                    if self.db().upsert_travel_route(&tr).is_ok() {
                        cascades.push(format!(
                            "Synced travel_route '{}' status '{}' -> '{}' (Dijkstra routing & market ripples updated)",
                            tr.id, old_s, norm_status
                        ));
                    }
                }
            }
        }

        // 2nd-order: warn if any active NPC trip is currently using this blocked/destroyed leg
        if matches!(norm_status, "blocked" | "destroyed") {
            if let Ok(chars) = self.list_characters() {
                for ch in chars {
                    if let Ok(prof) = self.get_character_profile(&ch.baseline.id) {
                        if let Some(ref trip) = prof.mobility.active_trip {
                            let touches = (trip.origin_place_id == from_pid
                                && trip.destination_place_id == to_pid)
                                || (trip.origin_place_id == to_pid
                                    && trip.destination_place_id == from_pid)
                                || (trip.route_waypoints.iter().any(|w| w == &from_pid)
                                    && trip.route_waypoints.iter().any(|w| w == &to_pid));
                            if touches {
                                cascades.push(format!(
                                    "Route disruption alert: NPC '{}' is currently in transit on blocked route '{}' <-> '{}'",
                                    ch.baseline.id, from_pid, to_pid
                                ));
                            }
                        }
                    }
                }
            }
        }

        cascades
    }
}
