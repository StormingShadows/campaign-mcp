# Chronicle Tool Call & Schema Templates

Concrete payload templates for common GM workflows. All examples use generic placeholders—adapt fields to your campaign's ruleset and setting.

---

## 1. Cold-Start Scene Context Read (`get_scene_context`)

```json
{
  "place_id": "<active-place-slug-or-name>",
  "character_ids": ["<pc-slug>"],
  "scene_text": "<player's latest message or action>",
  "format": "markdown",
  "max_chars": 8000,
  "include_gm_secrets": true,
  "recent_events_limit": 8
}
```

---

## 2. Creating a Character (`create_character`)

Immutable starting fields populate **Section 1 (Initial / Baseline Profile)**; future mutations project into **Section 2 (Current Status)** and append to **Section 3 (Event & Impact Log)**.

```json
{
  "name": "<Character Name>",
  "title": "<Optional Epithet>",
  "kind": "pc",
  "origin": "<Species / Culture>",
  "archetype": "<Class / Role>",
  "faction": "<Starting Faction>",
  "appearance": "<Visual traits>",
  "personality": "<Demeanor & values>",
  "backstory": "<Concise history>",
  "initial_location": "<starting-place-slug>",
  "initial_stats": {
    "Might": 12,
    "Agility": 14,
    "Mind": 13,
    "Spirit": 11
  },
  "initial_resources": {
    "HP": { "current": 30, "max": 30, "unit": "" },
    "Mana": { "current": 20, "max": 20, "unit": "pts" }
  },
  "initial_equipment": [
    {
      "name": "<Weapon Name>",
      "slot": "Weapon",
      "quantity": 1,
      "equipped": true,
      "durability": "Intact",
      "tags": ["melee"],
      "description": "<Weapon traits>"
    }
  ],
  "initial_skills": [
    {
      "name": "<Skill Name>",
      "rank": "Adept",
      "category": "Exploration",
      "modifier": "+3",
      "description": "<What this skill covers>"
    }
  ],
  "initial_techniques": [
    {
      "name": "<Technique or Spell Name>",
      "tier": "Tier I",
      "cost": "5 Mana",
      "cooldown": "1 turn",
      "mastery": "Practiced",
      "effect": "<Mechanical & narrative effect>",
      "description": "<Flavor or origin>"
    }
  ],
  "initial_goals": ["<Starting personal objective>"],
  "tags": ["pc"]
}
```

---

## 3. Creating a Place with Seasonal Profiles & Landmarks (`create_place`)

```json
{
  "name": "<Place Name>",
  "place_type": "settlement",
  "parent_place": "<parent-region-slug>",
  "terrain": "<Primary terrain features>",
  "climate": "<Climate zone>",
  "seasonal_profiles": {
    "Spring": "<Spring terrain/weather effects>",
    "Summer": "<Summer terrain/weather effects>",
    "Autumn": "<Autumn terrain/weather effects>",
    "Winter": "<Winter terrain/weather effects>"
  },
  "description": "<Overview of this place>",
  "initial_weather": "<Current weather>",
  "initial_faction": "<Controlling faction>",
  "initial_danger_level": "Moderate",
  "places_of_significance": [
    {
      "name": "<Landmark Name>",
      "kind": "Library",
      "status": "Intact",
      "description": "<Why this landmark matters>",
      "tags": ["archive", "landmark"]
    }
  ],
  "routes": [
    {
      "destination_id": "<neighbor-place-slug>",
      "travel_time": "3 hours on foot",
      "terrain_difficulty": "Moderate",
      "status": "Open",
      "notes": "<Route hazards or landmarks>"
    }
  ],
  "tags": ["settlement"]
}
```

---

## 4. Cataloging & Reading a Book or Ritual Scroll

### A. Catalog the Artifact (`upsert_text`)
```json
{
  "title": "<Book or Scroll Title>",
  "medium": "scroll",
  "subject": "spell-ritual",
  "author": "<In-world Author or Unknown>",
  "language": "<Language / Script>",
  "condition": "Intact",
  "read_status": "discovered_unread",
  "holder_or_location_id": "<place-or-character-slug>",
  "summary": "<Outward physical description visible BEFORE reading>",
  "content": "### Passage / Ritual Steps\n1. <Step or historical excerpt>\n2. <Step or verse>",
  "mechanics_or_unlocks": [
    "<What reading or deciphering unlocks>"
  ],
  "gm_notes": "<Hidden GM notes: decipher DC, traps, deliberate inaccuracies>",
  "keywords": ["<trigger-keyword-1>", "<trigger-keyword-2>"],
  "tags": ["ritual"],
  "related_ids": ["<linked-place-or-lore-slug>"]
}
```

### B. Record Study / Deciphering / Pickup (`record_text_reading`)
```json
{
  "text_id": "<text-slug>",
  "reader_id": "<character-slug-or-name>",
  "read_status": "deciphered",
  "holder_or_location_id": "<character-slug-if-taken>",
  "notes": "Translated the ciphered margins during camp"
}
```

---

## 5. Atomic Multi-Step Turn Commit (`commit_turn`)

Follow the strict write order: **Time → Movement → Character Deltas → Place Updates → Texts/Lore → Relations/Clocks → Narrative Event**.

```json
{
  "actions": [
    {
      "tool": "advance_time",
      "arguments": { "minutes": 30, "reason": "Skirmish and exploration" }
    },
    {
      "tool": "move_character",
      "arguments": {
        "character_id": "<pc-slug>",
        "to_place_id": "<destination-place-slug>",
        "travel_notes": "Crossed the northern bridge"
      }
    },
    {
      "tool": "modify_character_resource",
      "arguments": {
        "character_id": "<pc-slug>",
        "resource": "HP",
        "delta": -8,
        "reason": "Struck by guardian trap"
      }
    },
    {
      "tool": "update_character_condition",
      "arguments": {
        "character_id": "<pc-slug>",
        "action": "add",
        "name": "Bleeding",
        "kind": "injury",
        "severity": "Minor",
        "duration": "Until bandaged",
        "source": "Stone dart trap",
        "notes": "-1 to physical checks until treated"
      }
    },
    {
      "tool": "update_place",
      "arguments": {
        "place_id": "<destination-place-slug>",
        "action": "landmark",
        "landmark_name": "<Landmark Name>",
        "landmark_status": "Unsealed",
        "description": "Vault doors now stand ajar."
      }
    },
    {
      "tool": "manage_clock",
      "arguments": {
        "action": "tick",
        "clock_id": "<quest-clock-slug>",
        "delta": 1,
        "reason": "Entered the inner vault"
      }
    },
    {
      "tool": "log_event",
      "arguments": {
        "title": "Vault Unsealed",
        "details": "<Concise summary of the story beat>",
        "location_id": "<destination-place-slug>",
        "affected_entities": ["<pc-slug>", "<destination-place-slug>"]
      }
    }
  ]
}
```

---

## 6. Copying an Attached File into Campaign Docs (`import_attachment`)

When the player attaches a map, handout, portrait, PDF, or world-building file to the chat, copy it into `<campaign>/docs/attachments/files/` to get a permanent addressable reference:

```json
{
  "title": "<Document or Map Title>",
  "source_path": "/path/to/attached/world-map.png",
  "category": "map",
  "summary": "Annotated map of the northern frontier passes and watchtowers.",
  "extracted_notes": "Shows three mountain passes, a collapsed bridge north of the river, and an unmarked shrine symbol east of the ridge.",
  "visibility": "public",
  "keywords": ["map", "frontier", "passes"],
  "tags": ["cartography"],
  "related_ids": ["<region-place-slug>"]
}
```
*(If no local `source_path` exists, pass `"filename": "notes.md"` + `"text_content": "..."` for text/Markdown, or `"filename": "map.png"` + `"base64_content": "..."` for binaries. Retrieve anytime via `get_attachment({ "attachment_id": "<id>" })`, `fetch({ "id": "attachment:<id>" })`, or MCP resource `chronicle://attachments/<id>`.)*

---

## 7. Cataloging & Transferring Carryable Items (`upsert_item` & `transfer_catalog_item`)

Physical books, manuals, scrolls, and records created via `upsert_text` auto-sync as carryable items. Use `upsert_item` for weapons, tools, artifacts, keepsakes, heirlooms, or standalone books/manuals with full provenance:

```json
{
  "name": "<Item or Relic Name>",
  "item_type": "keepsake",
  "rarity_or_tier": "Heirloom",
  "slot": "Belt",
  "equipped": true,
  "condition_or_durability": "Intact",
  "origin_and_maker": "<Who forged/crafted it and when>",
  "previous_owners": ["<First Owner>", "<Second Owner>"],
  "current_holder_id": "<character-or-place-slug>",
  "significance": "<Why this item matters to the story or sects>",
  "properties_and_effects": "<Mechanical bonus or special property>",
  "description": "<Physical appearance>",
  "linked_text_id": "<optional-story-text-slug>",
  "keywords": ["<keyword-1>", "<keyword-2>"],
  "tags": ["heirloom"]
}
```

Transferring a carryable item automatically appends the former holder to `previous_owners`, updates both characters' inventories, and syncs any linked `StoryText`:

```json
{
  "item_id": "<item-slug>",
  "to_holder_id": "<new-character-or-place-slug>",
  "condition_or_durability": "Chipped blade",
  "notes": "Handed over after the duel at the guildhall"
}
```

---

## 8. Recording Histories for Places, Establishments, Sects & Items (`add_entity_history`)

```json
{
  "entity_id": "<place-establishment-sect-or-item-slug>",
  "entity_kind": "sect",
  "era_or_date": "Second Age, Year 140",
  "sort_order": 20,
  "title": "<Historical Event Title>",
  "details": "<What happened, who was involved, and its aftermath>",
  "significance": "<Why this historical milestone still matters today>",
  "visibility": "public",
  "related_ids": ["<linked-place-or-item-slug>"]
}
```
*(Valid `entity_kind` values: `"place"`, `"establishment"`, `"sect"`, `"faction"`, `"item"`, `"text"`, `"character"`, `"lore"`.)*

---

## 9. Creating or Appending a Pasted-In Reference File (`create_reference_doc`)

When the user pastes reference notes, house rules, or world excerpts with no pre-existing file on disk, create the file (or append to it with `"append": true`) inside `<campaign>/docs/attachments/files/`:

```json
{
  "title": "<Reference Document Title>",
  "filename": "custom-notes.md",
  "content": "# Heading\nPasted reference text...",
  "append": false,
  "category": "world-doc",
  "summary": "<Why this reference matters>",
  "keywords": ["<keyword-1>", "<keyword-2>"],
  "related_ids": ["<entity-slug>"]
}
```

---

## 10. Diagnostics, Reclassification, Merging & Cleanup

### A. Audit Campaign Health (`diagnose_campaign`)
```json
{}
```

### B. Move Mislabeled Entry to the Right Register (`reclassify_entity`)
```json
{
  "from_register": "lore",
  "entity_id": "<mislabeled-slug>",
  "to_register": "text",
  "overrides": {
    "medium": "grimoire",
    "subject": "spell-ritual",
    "holder_or_location_id": "<place-or-character-slug>"
  },
  "reason": "Moved readable in-world grimoire from lore into books & scrolls"
}
```

### C. Rename a Slug or Merge a Duplicate (`rename_or_merge_entity`)
```json
{
  "register": "lore",
  "old_id": "<duplicate-slug>",
  "new_id": "<canonical-slug>",
  "allow_merge": true,
  "reason": "Merged duplicate lore entry and updated all cross-references"
}
```

### D. Safe Campaign Hygiene (`cleanup_campaign`)
```json
{
  "dry_run": true
}
```
*(Inspect `actions`, then call with `"dry_run": false` to prune orphaned `.md` files, sync carryable texts/items/inventories, deduplicate tags/keywords/links, and normalize casing.)*

---

## 11. "Search By" Query Patterns

- **Find injured party members:** `search_characters({ "kind": "pc", "resource_name": "HP", "resource_below_pct": 50 })`
- **Find who holds a specific item/key:** `search_characters({ "has_item": "key" })`
- **Find carryable items previously owned by someone:** `search_items({ "previous_owner": "<owner-name-or-slug>" })`
- **Find carryable readable books/manuals:** `search_items({ "has_linked_text": true })`
- **Find recorded histories of a sect, establishment, place, or item:** `search_histories({ "entity_kind": "sect", "query": "<keyword>" })`
- **Find places with sealed landmarks in a terrain:** `search_places({ "terrain": "forest", "landmark_status": "Sealed" })`
- **Find unread ritual scrolls at a location:** `search_texts({ "medium": "scroll", "subject": "spell-ritual", "read_status": "discovered_unread", "holder_or_location_id": "<place-slug>" })`
- **Find everything a character has read:** `search_texts({ "read_by": "<character-slug>" })`
- **Find hidden GM lore linked to an entity:** `search_lore({ "visibility": "gm_secret", "related_to": "<entity-slug>" })`
- **Find 2-hop allies/enemies of a character:** `search_relations({ "entity_id": "<character-slug>", "hops": 2 })`
- **Find all events from a specific season:** `search_history({ "world_time_contains": "Winter", "limit": 30 })`
- **Search everything at once:** `search_all({ "query": "<keyword>" })`
