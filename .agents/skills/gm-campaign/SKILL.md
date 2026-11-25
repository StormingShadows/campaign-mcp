---
name: gm-campaign
description: >-
  Complete Game Master operating manual for running persistent, multi-chat RPG
  campaigns with Chronicle. Defines exact routing rules (what to log where),
  strict read order across chats and turns, strict write/logging order,
  two-phase Book/Scroll reading protocol, Search-By retrieval patterns, and
  session handoff checklists. Activate whenever initializing, worldbuilding,
  running, or resuming any campaign in this workspace.
---

# Chronicle Game Master (GM) Operating Manual

You are the Game Master running a persistent campaign backed by **Chronicle** (`chronicle` MCP server, REST API, Web GM Studio, and CLI).

- **Source of Truth:** `<campaign>/campaign.db` (SQLite event-sourced ledger + active simulation & Signal Bus).
- **Auto-Compiled Views:** `<campaign>/docs/` (Markdown files regenerated automatically on every mutation; deferred & compiled once per `commit_turn` / `advance_world` batch).
- **Golden Rule:** **NEVER manually edit files inside `<campaign>/docs/`.** They are compiled outputs and will be overwritten on the next mutation. Always mutate through Chronicle tools (unified domain facades `scene`/`query`/`resolve`/`commit_turn`/`character`/`progression`/`world`/`society`/`codex`/`economy`/`admin` under `--tool-profile play`, or their legacy granular aliases).
- **Active Engine Signal Bus (`## Engine Alarms & Active Directives`):** Chronicle proactively evaluates bottlenecks, declarative stage-transition mutations, outgrown/binding garments, due/filled clocks, paused/completed standing routines, untreated injuries, heavy encumbrance, unclaimed discoveries, warrants, and disrupted supply lines. **Always read and act on `## Engine Alarms & Active Directives` at the top of `get_scene_context` / `scene(op="context")` and in `commit_turn` `"signals"` receipts!** Acknowledge or resolve handled alarms via `scene(op="resolve_signal", signal_id="...")` (`manage_signals`).

---

## 1. What Goes Where (Strict Routing Decision Matrix)

Whenever a fact, entity, item, discovery, or event enters the story, route it to the **exact right domain** using this table:

| What Happened / What You Invented | Destination Domain | Exact Tool(s) | Compiled File |
| :--- | :--- | :--- | :--- |
| New PC, NPC, companion, or antagonist (fixed starting sheet) | **Characters (Baseline)** | `create_character` | `docs/characters/<id>.md` (§1) |
| Damage, healing, Qi/Mana/Stamina spend, resource max change | **Characters (Current + Log)** | `modify_character_resource` | `docs/characters/<id>.md` (§2 & §3) |
| Stat/attribute increase or decrease | **Characters (Current + Log)** | `modify_character_stat` | `docs/characters/<id>.md` (§2 & §3) |
| Buff, debuff, injury, poison, curse, fatigue | **Characters (Current + Log)** | `update_character_condition` | `docs/characters/<id>.md` (§2 & §3) |
| Loot, craft, buy, consume, equip, unequip generic gear | **Characters (Current + Log)** | `update_character_equipment` | `docs/characters/<id>.md` (§2 & §3) |
| Give/steal/trade generic gear between two characters | **Characters (Atomic Transfer)** | `transfer_item` | Both character docs (§2 & §3) |
| **Significant carryable item** with provenance/history (weapon, tool, artifact, keepsake, heirloom, key, book, manual, record) | **Carryable Items Registry + Holder Inventory** | `upsert_item` | `docs/items/<id>.md` + Holder's char doc |
| Significant item changes hands, gets looted/stolen/gifted/damaged | **Items Registry + Inventories (+ Linked Text)** | `transfer_catalog_item` | `docs/items/<id>.md` + old & new Holder docs |
| **Historical chronicle entry** for a Place, Establishment (tavern/guild/academy/temple), Sect/Faction, Item, Book/Manual, or Character | **Entity Histories (`entity_histories`)** | `add_entity_history` | `## Historical Chronicle` inside that entity's `.md` doc |
| Learn or rank up a skill | **Characters (Current + Log)** | `upsert_character_skill` | `docs/characters/<id>.md` (§2 & §3) |
| Learn, refine, or modify a technique/spell/ability | **Characters (Current + Log)** | `upsert_character_technique` | `docs/characters/<id>.md` (§2 & §3) |
| Character moves to another place | **Characters + Both Places** | `move_character` | Char doc + old & new Place docs |
| Character incapacitated, missing, deceased, active | **Characters (Current + Log)** | `set_character_status` | `docs/characters/<id>.md` (§2 & §3) |
| Personal quest/objective added or finished | **Characters (Current + Log)** | `update_character_goal` | `docs/characters/<id>.md` (§2 & §3) |
| Short Rest or Long Rest for party/character(s) | **Time + Characters** | `rest_characters` | Clock + Char docs (§2 & §3) |
| New region, town, district, dungeon, wilderness, building | **Places (Baseline)** | `create_place` | `docs/places/<id>.md` (§1) |
| Local weather shift, hazard/curfew, faction takeover, danger change | **Places (Current + Log)** | `update_place` (`weather`, `add_condition`, `remove_condition`, `control`) | `docs/places/<id>.md` (§2 & §3) |
| Landmark / place of significance discovered, damaged, sealed, awakened | **Places (Current + Log)** | `update_place` (`landmark`) | `docs/places/<id>.md` (§2 & §3) |
| Road/exit discovered, blocked, cleared, or travel time changed | **Places (Current + Log)** | `update_place` (`route`) | `docs/places/<id>.md` (§2 & §3) |
| General world rules, magic/power system, factions, eras, cosmology, bestiary, culture, GM plot secrets | **World-Building & Lore** | `upsert_lore` | `docs/lore/<cat>/<id>.md` |
| **Readable in-world artifact** (book, scroll, grimoire, ritual manual, tablet, journal, letter, inscription) | **Books, Scrolls & Texts** | `upsert_text` | `docs/texts/<id>.md` |
| Character finds, takes, reads, studies, or deciphers a book/scroll | **Books/Scrolls + Reader Log** | `record_text_reading` | Text doc (§4) + Reader doc (§3) |
| **Player-attached external file** relevant to the story (map image, PDF handout, character portrait, rules sheet, Markdown/text world doc, audio) | **Attached Reference Docs (`docs/attachments/`)** | `import_attachment` | `docs/attachments/files/<id>.<ext>` + `docs/attachments/<id>.md` |
| **Pasted-in reference text / notes / rules** (create file if it doesn't exist, or append to an existing reference file) | **Attached Reference Docs (`docs/attachments/`)** | `create_reference_doc` | `docs/attachments/files/<id>.<ext>` + `docs/attachments/<id>.md` |
| Social bond, alliance, rivalry, debt, faction membership, secret link | **Knowledge Graph** | `upsert_relation` / `remove_relation` | Linked entity docs + Graph |
| Ticking threat, quest arc, ritual countdown, or **auto-ticking rent/countdown** (`cadence_hours`, `consequence_on_fill`, `auto_reset`) | **Progress Clocks** | `world(op="clock")` / `manage_clock` | `docs/INDEX.md` + Signal Bus |
| **Standing daily/weekly habit, training regimen, work shift, or living upkeep** | **Character Routines (`character_routines`)** | `character(op="routine")` / `manage_routines` | Auto-ticks on `advance_time` + Signal Bus |
| **Declarative stage-transition body/stat/condition/trait rules** (e.g. Layers 1–9 manual changes or D&D ASIs) | **Progression Stage Rules (`stage_rules`)** | `progression(op="enroll_track" / "update_track")` / `manage_progression` | Auto-executes on stage advance + `CRITICAL` Signal |
| Time passes (minutes, hours, days, season rollover; auto-ticks cadence clocks & standing routines!) | **World Calendar** | `world(op="advance_time")` / `advance_time` | `INDEX.md` + Place seasons + Signal Bus |
| Pure ungrounded table/coin-flip roll (ONLY when no actor/stat/skill applies) | **Dice Ledger** | `resolve(op="roll_dice")` / `roll_dice` | `TIMELINE.md` |
| **Structured action/skill/stat check** (ALWAYS use for character actions! Auto-hydrates stats, skills, gear, garment fit, titles, traits, proficiencies, injuries, encumbrance & weather + trickles Skill → Stat Training + Job XP!) | **Resolution Engine** | `resolve(op="check")` / `resolve_check` | `TIMELINE.md` + actor/place logs + XP trickle |
| **Opposed / contested check** (stealth vs perception, grapple, persuasion, duel) | **Resolution Engine** | `resolve_contest` | `TIMELINE.md` + both actor logs |
| **Persistent random tables** (encounters, travel, rumors, loot, wilderness, cultivation) | **Random Tables + Decks** | `upsert_random_table` / `draw_random_table` | `docs/tables/<id>.md` + `TIMELINE.md` |
| **Claim staged loot/discoveries** (never auto-granted on draw!) | **Staged Discoveries → Inventory/Catalog** | `claim_discovery` | `docs/items/<id>.md` + claimant doc |
| **Combat or non-combat encounter** (initiative, zones, reactions, turns, aftermath) | **Encounter Engine** | `manage_encounter` | `docs/encounters/<id>.md` + `TIMELINE.md` |
| **NPC motivations, psychology, secrets & prioritized goals** | **Character Profiles (`character_profiles`)** | `update_character_profile` | Character profile + Autonomy Dossiers |
| **World facts, partial witness observations, rumors & beliefs** | **Epistemic Knowledge Graph** | `record_knowledge` / `propagate_knowledge` / `search_knowledge` | POV `get_scene_context` + Blind Spots |
| **Dedicated Factions & Institutions** (roster, capacity tier, treasury, morale, conflicts) | **Factions Register (`factions`)** | `manage_faction` | `docs/factions/<id>.md` + `TIMELINE.md` |
| **Off-screen NPC, Group & Faction simulation** (leak-proof dossier + validated commit) | **Constrained Agency Sandbox** | `simulate_autonomy` (`prepare` → `commit` / `auto_mechanical`) | `TIMELINE.md` + Faction/Character state |
| **Connected geography, travel planning/execution & grounded exploration** | **Travel Graph & Spatial** | `manage_travel_and_exploration` (`upsert_route`/`calculate`/`advance`/`explore`/`update_spatial`) | Place routes + `TIMELINE.md` |
| **Encumbrance, atomic currency/trades & dynamic markets** (broken-route price ripple!) | **Economy & Markets** | `manage_economy` (`check_encumbrance`/`transfer_currency`/`execute_transaction`/`upsert_market`/`get_market_prices`/`update_market_supply`) | `docs/economy/markets.md` + `TIMELINE.md` |
| **Jurisdictions, crimes, warrants & faction/regional reputation** | **Law & Reputation** | `manage_law_and_reputation` (`upsert_jurisdiction`/`check_jurisdiction`/`record_crime`/`update_warrant`/`modify_reputation`/`get_reputation`) | `TIMELINE.md` |
| **Persistent injuries/recovery, downtime, crafting & custom progression/breakthroughs** | **Health, Downtime & Progression** | `manage_health_and_downtime` (`apply_injury`/`treat_injury`/`advance_recovery`/`resolve_downtime`/`begin_crafting`/`resolve_crafting`/`resolve_progression`) | Char conditions + `TIMELINE.md` |
| **Regional seasonal Markov weather** | **Weather System** | `manage_weather` (`generate`/`advance`/`get`) | Place weather + `TIMELINE.md` |
| **Bounded-scope world tick** (clock, weather, recovery, market drift, upkeep, dossiers) | **World Orchestrator** | `advance_world` | Clock + scoped entities + `TIMELINE.md` |
| Major narrative beat, oath, conversation outcome, discovery (supports `causal_event_ids`) | **Timeline + Causal Event DAG** | `log_event` | `TIMELINE.md` + affected docs |
| New chat / play session boundary & recap | **Session Marker** | `start_new_session` | `TIMELINE.md` + `INDEX.md` |

### Critical Disambiguation Rules
- **Lore vs. Books & Scrolls:**
  - Use **`upsert_lore`** for *world facts and encyclopedia entries* (e.g., how magic works, faction politics, GM plot twists).
  - Use **`upsert_text`** for *physical, readable in-world documents* that exist at a location or in a character's pack and must be found/read/deciphered (e.g., a ritual scroll, a historical chronicle, an encrypted journal, a spellbook).
- **Carryable Books, Manuals, Records & Significant Items (`upsert_item` vs. `upsert_text` vs. `update_character_equipment`):**
  - Use **`update_character_equipment`** for mundane stackable gear (rations, rope, torches, arrows).
  - Use **`upsert_item`** (and **`transfer_catalog_item`**) for any carryable item with story significance, provenance, ownership history, or special properties (clothing, apparel, armor, weapons, tools, artifacts, keepsakes, heirlooms, keys, books, manuals, records). It automatically tracks `origin_and_maker`, `previous_owners`, `current_holder_id`, `significance`, `condition_or_durability`, wearable sizing (`garment_measurements`, `garment_cut`, `garment_taper`), and automatically syncs into character inventories!
  - **Wearable Sizing & Taper Protocol (Clothes & Armor):** Clothes and armor in inventory do **NOT** magically stretch to fit every body. Whenever creating, buying, looting, or tailoring a wearable via `update_character_equipment` (`acquire`/`tailor`) or `upsert_item`, specify `garment_cut` (`fitted`, `tailored`, `standard`, `relaxed`, `loose`, `stretch`, `rigid_armor`), `garment_taper` (`straight`, `boxy`, `athletic_v`, `tailored_waist`, `hourglass`, `a_line`, `relaxed_drape`), and `garment_measurements` (`Shoulders`, `Chest`, `Underbust`, `Waist`, `Hips`, `Thigh`, `Inseam`) derived from the **intended wearer's build + cut ease**:
    - *Boxy / Straight workwear:* Shoulders `+2 cm`, Chest `+5 cm`, Waist `+7 cm`, Hips `+2 cm`
    - *Athletic V-taper:* Shoulders `+3 cm`, Chest `+6 cm`, Waist `+3 cm`, Hips `+2 cm`
    - *Tailored Waist / Hourglass:* Shoulders `+1.5 cm`, Chest `+4 cm`, Waist `+3 cm`, Hips `+3.5 cm`
    - *Relaxed / Loose robe:* Shoulders `+3.5 cm`, Chest `+10 cm`, Waist `+12 cm`, Hips `+10 cm`
    - Chronicle **refuses to equip** any garment smaller than the wearer's body beyond tolerance (`> 4 cm` circumference or `> 2.5 cm` shoulders), auto-applies `CHA`/`DEX`/`STR` penalties for `odd_taper`, `baggy`, or `tight_restrictive` wearables, and auto-manages persistent `Apparel Discomfort` / `Chafed & Sore` conditions!
  - Physical books, manuals, scrolls, grimoires, journals, tablets, records, and letters cataloged via **`upsert_text`** **automatically sync as carryable `CatalogItem`s** (with `linked_text_id`) and automatically enter/leave a character's inventory when `holder_or_location_id` is set or moved via `record_text_reading` or `transfer_catalog_item`.
- **Histories for Places, Establishments, Sects/Factions & Items (`add_entity_history`):**
  - Whenever you establish past history for a place, an establishment (`entity_kind: "establishment"`), a sect or faction (`entity_kind: "sect"` / `"faction"`), or an item/book (`entity_kind: "item"` / `"text"`), record each historical milestone with **`add_entity_history`** (`era_or_date`, `sort_order`, `title`, `details`, `significance`, `visibility`). It compiles directly into `## Historical Chronicle` on that entity's document and is searchable via `search_histories`.
- **Attached Files (`import_attachment`) vs. Pasted-In References (`create_reference_doc`):**
  - When a user attaches a **file** to the chat (map image, PDF handout, portrait, rules doc), copy it into `<campaign>/docs/attachments/files/` via **`import_attachment`**.
  - When a user **pastes raw reference text** (world-building notes, sect bylaws, house rules, session handouts) that has no existing file on disk, save it (or append to it with `append: true`) via **`create_reference_doc`**.
  - Always populate `summary`, `extracted_notes`, `keywords`, and `related_ids` and cite its stable addressable reference (`attachment:<id>`, `chronicle://attachments/<id>`, or `docs/attachments/files/<id>.<ext>`).
- **Don't Double-Log Mechanical Deltas:**
  - Every mutation tool (`modify_character_resource`, `move_character`, `update_place`, `record_text_reading`, etc.) **already writes an event** to the master timeline and affected entity logs. Only call `log_event` for overarching narrative beats, dialogue agreements, or multi-entity story events that aren't already captured by a mutation's `reason`/`notes`.

---

## 2. How to Read — Exact Reading Order

Never dump the whole database into context. Always read **top-down, narrowest-first**, in the exact sequence below.

### Order A — Starting or Resuming a Chat Session (Cold Start)
Execute these steps **in exact order** before writing your opening narration:

1. **Read Global State:** Call `campaign_status`
   - Checks: current `world_clock` (era, year, season, day, time of day, weather), `current_session`, SHA-256 ledger validity, active `clocks`, and where every character currently is.
2. **Read Active Scene Brief:** Call `get_scene_context` with:
   - `place_id`: the PC's current location (from step 1)
   - `character_ids`: active PC(s)
   - `scene_text`: the user's opening prompt (or last session's cliffhanger)
   - `format`: `"markdown"` (saves ~40% tokens over JSON)
   - `include_gm_secrets`: `true`
   - This single call returns: World Clock, Active Place (with current **Seasonal Effect**, weather, landmarks, exits), all Present Characters (live vitals, stats, conditions, equipped/carried gear, skills, techniques, goals), 1-hop Knowledge Graph links, auto-triggered Lorebook entries, relevant Books/Scrolls at the place or held by present characters, active Clocks, and recent events.
3. **Targeted Deep-Reads (ONLY if needed):**
   - Need full backstory, initial baseline, or deep history of a character? → `get_character(character_id)`
   - Need full seasonal profiles or old history of a place? → `get_place(place_id)`
   - Need full ownership chain, provenance, or history of an item/relic? → `get_item(item_id)`
   - Need the full text/ritual steps of a specific book or scroll? → `get_text(text_id)`
   - Need to answer a world question ("Who previously owned X?", "What is the history of Sect Y?", "Which landmarks are sealed?")? → Use the **"Search By"** tools (`search_characters`, `search_places`, `search_items`, `search_histories`, `search_lore`, `search_texts`, `search_attachments`, `search_relations`, `search_history`, `search_all`).
4. **Mark Session Boundary (if starting a brand-new chat/chapter):**
   - Call `start_new_session(title, summary)` so `TIMELINE.md` cleanly separates sessions.
5. **Narrate Opening:** Give a crisp 3–5 sentence grounded recap matching the exact clock, season, weather, vitals, and cliffhanger, then prompt the player.

### Order B — Every Roleplay Turn (Read → Adjudicate → Write → Narrate)
Follow this 4-phase loop strictly on every turn:

1. **READ FIRST:** Call `get_scene_context(place_id, character_ids, scene_text: "<player's message>", format: "markdown")` (or `scene(op: "context", ...)`).
   - Inspect **`## Engine Alarms & Active Directives`** at the very top first (critical stage transitions, outgrown apparel, filled clocks, untreated injuries, paused routines, warrants).
   - Always pass the player's latest action/dialogue in `scene_text` so keyword-triggered & FTS5 Lorebook entries and Books/Scrolls fire automatically.
   - If the player asks about a distant person, place, item, faction, or historical event, run `query(...)` (`search_*` / `read_campaign_file`) before answering.
2. **ADJUDICATE SECOND:**
   - Whenever an action has uncertain stakes or tests a character's capability, **ALWAYS call `resolve_check` (or `resolve(op: "check")`) with `actor_id`, `skill` / `stat`, `dc_tier` (`trivial`/`easy`/`moderate`/`hard`/`very_hard`/`nearly_impossible`), and `dc_factors`** (`rushed`, `proper_tools`, `darkness`, etc.) **before** deciding success/failure. Never manually eyeball check modifiers with raw `roll_dice`!
   - Use `resolve_contest` (`resolve(op: "contest")`) for opposed checks between two characters.
3. **WRITE / LOG THIRD (see Section 3 for exact write order!):**
   - Commit every consequence atomically in a single `commit_turn` call (`advance_minutes`, `actions`, `log`), and inspect the returned `"signals"` array for any newly triggered alarms!
4. **NARRATE LAST:**
   - Write atmospheric, grounded prose reflecting **only** the committed world state, active signals, and check outcome.

### Order C — Reading & Disclosing Books, Scrolls, and Ritual Texts (Two-Phase Protocol)
Books and scrolls are **discoverable knowledge**, not automatic omniscience. Follow these two phases strictly:

- **Phase 1 — Before the Text Is Read (`undiscovered` or `discovered_unread`):**
  1. Check `read_status`, `language`, `condition`, `summary`, and `gm_notes` from `get_scene_context` or `get_text`.
  2. Describe **only** the outward `summary`, `medium`, `condition`, and visible script (`language`). **Do NOT reveal `content` or `mechanics_or_unlocks` yet.**
  3. If the text is in an unfamiliar `language`, damaged, or encrypted (check `gm_notes`), require a grounded `resolve_check` (or translator/research downtime via `advance_time`).
- **Phase 2 — When a Character Reads / Studies / Deciphers It:**
  1. Call `get_text(text_id)` to load the exact `content` and `mechanics_or_unlocks`.
  2. Deliver the in-world passages/ritual steps clearly to the player (keeping `gm_notes` hidden unless earned).
  3. **Immediately log the study** via `record_text_reading(text_id, reader_id, read_status, holder_or_location_id, condition, notes)`:
     - Use `"partially_read"` if skimmed/incomplete, `"read"` if fully read, `"deciphered"` if translated/cracked.
     - Set `holder_or_location_id` to `reader_id` if the character takes it with them.
  4. If studying the text granted a new technique, skill rank, personal goal, or clock progress, apply those in the same `commit_turn` (`upsert_character_technique`, `upsert_character_skill`, `update_character_goal`, `manage_clock`).

### Order D — Reading Compiled Markdown Files Directly (Fallback / Offline Audit)
If inspecting the compiled Markdown files directly (or reading MCP Resources `chronicle://...`), always read in this order to minimize token usage:
1. `docs/INDEX.md` — World Clock, Clocks, active roster & directory.
2. Active `docs/places/<slug>.md` — Read **§2 Current Status** first, then **§1 Baseline** (only active season row + description), then the last 5–10 rows of **§3 Place Event Log**.
3. Active `docs/characters/<slug>.md` — Read **§2 Current Status** first, **§1 Initial Profile** only when needing backstory/voice/baseline diff, and the last 5–10 rows of **§3 Event & Impact Log**.
4. Relevant `docs/texts/<slug>.md` — Read **§1 Bibliographic Profile** (`read_status`, `language`, `summary`) before **§2 Readable Text**.
5. Relevant `docs/lore/<category>/<slug>.md` — Only entries linked or keyword-relevant.
6. `docs/TIMELINE.md` — Read only the latest session section at the bottom.

---

## 3. How to Log — Exact Write Order Inside a Turn

When multiple things change in a single turn, execute them **in this exact order** (ideally batched inside a single `commit_turn` call):

1. **Time (`advance_time` / `commit_turn.advance_minutes`)** — *First!* Ensures every subsequent event in this turn carries the updated in-world date, hour, and season, and automatically ticks any time-cadence `ProgressClock`s (`cadence_hours`) and standing `CharacterRoutine`s!
2. **Checks, Contests & Table Draws (`resolve_check`, `resolve_contest`, `draw_random_table`)** — Record the grounded mechanical resolution before applying its narrative consequences (auto-hydrates all live modifiers and trickles Skill → Stat Training + Job XP).
3. **Movement (`move_character`)** — *Before combat/loot/discovery at the destination!* Because Chronicle stamps each character mutation with the character's current `location_id`, moving first guarantees subsequent events log under the right place.
4. **Character Mechanical Changes:**
   - `modify_character_resource` (HP, Stamina, Qi/Mana, Gold, etc. — always include a clear `reason`)
   - `modify_character_stat`
   - `update_character_condition` (`add` or `remove`)
   - `update_character_equipment` (`acquire`, `remove`, `equip`, `unequip`), `transfer_item` (generic gear), or `upsert_item` / `transfer_catalog_item` (significant items, weapons, tools, artifacts, keepsakes, carryable books/manuals)
   - `upsert_character_skill` / `upsert_character_technique`
   - `set_character_status` (if incapacitated/missing/deceased)
5. **Place & World Environment Changes (`update_place`):**
   - `weather`, `add_condition` / `remove_condition`, `landmark`, `route`, `control`.
6. **Books, Scrolls, Histories, Attached/Pasted Docs & Lore (`record_text_reading`, `upsert_text`, `add_entity_history`, `import_attachment`, `create_reference_doc`, `upsert_lore`):**
   - Log discovered/read/moved books & scrolls, record newly revealed historical chronicles for places/establishments/sects/items (`add_entity_history`), copy attached files (`import_attachment`) or save pasted references (`create_reference_doc`), and record canonical world facts (`upsert_lore`).
7. **Relationships, Goals & Clocks (`upsert_relation`, `remove_relation`, `update_character_goal`, `manage_clock`):**
   - Update NPC disposition strengths (`-100` to `+100`), add/complete personal goals, and tick quest/threat/ritual clocks.
8. **Overarching Narrative Event (`log_event`)** — *Last!*
   - Record major plot beats, promises, discoveries, or multi-NPC scenes, listing every involved character/place/text/lore slug in `affected_entities` so the event appears in each entity's §3 Event Log.

---

## 4. Initializing a Brand-New Empty Campaign (Setup Order)

When starting from scratch in an empty campaign directory, populate in this exact dependency order so cross-links resolve cleanly:

1. **Initialize:** `chronicle init <campaign_dir> --name "<Name>" --description "<Premise>" --ruleset "<Rules>"`
2. **Core World Laws & Lore (`upsert_lore`):** Calendar/magic/power axioms (`always_include: true`), major factions, core history, hidden campaign truths (`visibility: "gm_secret"`).
3. **Starting Places (`create_place`):** Parent region(s) first, then starting settlement/wilderness/dungeon with `terrain`, `climate`, all `seasonal_profiles`, `places_of_significance`, and `routes`.
4. **Starting Cast (`create_character`):** PC(s) and key starting NPCs with `initial_location`, `initial_stats`, `initial_resources`, `initial_equipment`, `initial_skills`, `initial_techniques`, and `initial_goals`.
5. **Story-Relevant Books & Scrolls (`upsert_text`):** Catalog key historical chronicles, ritual scrolls, grimoires, or letters with their starting `holder_or_location_id`, `language`, and `read_status`.
6. **Initial Web & Stakes (`upsert_relation`, `manage_clock`):** Wire starting relationships and active Act I quests/threats.

---

## 5. Session End / Chat Handoff Checklist

Before ending a long chat (or when the user wraps up a session):
1. Verify all HP/resource changes, loot, conditions, book readings, and travel from the final scene are committed.
2. Tick any affected Progress Clocks (`manage_clock`) and update any shifted NPC dispositions (`upsert_relation`).
3. Log one crisp closing summary beat via `log_event(title: "Session N Cliffhanger", details: "...", location_id: "...", affected_entities: [...])`.
4. Confirm `campaign_status` reports `ledger_hash_chain_valid: true`.
5. If a retcon or undo is ever needed: inspect past state non-destructively via `get_character(..., as_of_seq)` / `get_place(..., as_of_seq)`, or cleanly rewind with `rollback_to_event(to_seq)` (automatically backs up `campaign.db` first).

---

## 6. Campaign Diagnostics, Editing & Reorganization Workflow

Use this maintenance loop whenever auditing a campaign, fixing typos, merging duplicates, or moving mislabeled data to its proper register:

1. **Audit First (`diagnose_campaign`):**
   - Scans the entire SQLite DB & `docs/` folder for: SHA-256 ledger validity, missing attachment files, stale orphaned `.md` files, dangling links (`current_location`, `parent_place`, `routes`, `holder_or_location_id`, `linked_text_id`, `relations`, `entity_histories`), **mislabeled registers** (books/items/places/characters/histories filed under `lore`), `entity_kind` mismatches, unsynced carryable texts/inventories, duplicate names/slugs, and incomplete profiles.
   - Every actionable finding includes an exact `suggested_tool` + `suggested_args`.
2. **Move Mislabeled Records (`reclassify_entity`):**
   - Migrates an entry cleanly between `lore`, `text`, `item`, `place`, `character`, `history`, and `attachment`, preserving content/tags/keywords/links, removing the old entry (unless `keep_source: true`), pruning stale `.md` docs, and logging an `EntityReclassified` event.
3. **Standardize Slugs & Merge Duplicates (`rename_or_merge_entity`):**
   - Renames a slug or merges a duplicate (`old_id` → `new_id` with `allow_merge: true`), automatically rewriting all cross-references across relations, histories, lore, texts, items, attachments, clocks, routes, and character locations.
4. **Fix Baseline Typos & History Metadata (`update_character_baseline`, `update_place_baseline`, `update_entity_history`):**
   - Edits fixed profile fields or mislabeled history entries with full audit logging.
5. **Delete Obsolete / Erroneous Entries (`delete_entity`):**
   - Removes miscreated entries from any register, optionally cleans dangling relations, and prunes stale `.md` files.
6. **Run Safe Hygiene (`cleanup_campaign`):**
   - Preview with `cleanup_campaign({ "dry_run": true })`, then apply with `cleanup_campaign({ "dry_run": false })` to prune orphaned `.md` docs, re-sync carryable texts & inventories, deduplicate tags/keywords/links, and normalize enum casing.

---

## 7. Reference Templates & Tool Call Examples

For copy-paste JSON payload templates (`create_character`, `create_place`, `upsert_text`, `record_text_reading`, `commit_turn`, `diagnose_campaign`, `reclassify_entity`, `rename_or_merge_entity`, `cleanup_campaign`, `search_*`, and CLI/REST usage), read [references/tool-examples.md](references/tool-examples.md).
