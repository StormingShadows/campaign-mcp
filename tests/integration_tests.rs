use chronicle::context::SceneContextRequest;
use chronicle::engine::CampaignEngine;
use chronicle::mcp::{execute_tool, export_tool_schemas};
use chronicle::models::{
    CharacterBaseline, GraphRelation, Item, Landmark, LoreEntry, PlaceBaseline, ProgressClock,
    ResourcePool, Route, Skill, StatusCondition, StoryText, Technique, WorldClock,
};
use chronicle::search::{
    CharacterSearchFilter, EventSearchFilter, LoreSearchFilter, PlaceSearchFilter,
    RelationSearchFilter, TextSearchFilter,
};
use chronicle::web::dispatch_http;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};
use tempfile::TempDir;

/// Test-only synthetic fixture (kept strictly in tests/, never compiled into the binary).
fn seed_test_fixture(engine: &mut CampaignEngine) {
    engine.set_auto_compile(false);

    let clock = WorldClock {
        era: "Test Era".into(),
        year: 100,
        season: "Autumn".into(),
        day: 44,
        hour: 16,
        minute: 30,
        seasons: vec!["Spring".into(), "Summer".into(), "Autumn".into(), "Winter".into()],
        days_per_season: 90,
        global_weather: "Overcast".into(),
    };
    engine
        .init_campaign("Fixture Campaign", "Integration test fixture", "2d10 + mod", Some(clock))
        .unwrap();

    let mut place_a_seasons = BTreeMap::new();
    place_a_seasons.insert("Autumn".into(), "Cold mountain winds sweep the ridge.".into());
    place_a_seasons.insert("Winter".into(), "Heavy snow blocks northern passes.".into());
    engine
        .create_place(PlaceBaseline {
            id: "watch-keep".into(),
            name: "Watch Keep".into(),
            place_type: "settlement".into(),
            parent_place: Some("north-marches".into()),
            terrain: "Basalt Ridge".into(),
            climate: "Subalpine".into(),
            seasonal_profiles: place_a_seasons,
            description: "Border fortress.".into(),
            initial_weather: "Crisp Wind".into(),
            initial_faction: Some("Wardens".into()),
            initial_danger_level: "Moderate".into(),
            places_of_significance: vec![Landmark {
                name: "Archive Vault".into(),
                kind: "Library".into(),
                status: "Intact".into(),
                description: "Stone chamber housing border records.".into(),
                tags: vec!["books".into()],
            }],
            routes: vec![Route {
                destination_id: "glass-hollow".into(),
                travel_time: "4 hours".into(),
                terrain_difficulty: "Rough".into(),
                status: "Open".into(),
                notes: "Forest road.".into(),
            }],
            tags: vec!["fortress".into()],
            notes: String::new(),
            created_at_world_time: String::new(),
        })
        .unwrap();

    let mut place_b_seasons = BTreeMap::new();
    place_b_seasons.insert("Autumn".into(), "Brittle leaves amplify footsteps.".into());
    place_b_seasons.insert("Winter".into(), "Deep frost cracks tree bark.".into());
    engine
        .create_place(PlaceBaseline {
            id: "glass-hollow".into(),
            name: "Glass Hollow".into(),
            place_type: "wilderness".into(),
            parent_place: Some("north-marches".into()),
            terrain: "Vitrified Forest".into(),
            climate: "Boreal".into(),
            seasonal_profiles: place_b_seasons,
            description: "Silent silica-bark woods.".into(),
            initial_weather: "Thin Mist".into(),
            initial_faction: None,
            initial_danger_level: "High".into(),
            places_of_significance: vec![Landmark {
                name: "Old Shrine".into(),
                kind: "Ruins".into(),
                status: "Dormant".into(),
                description: "Ring of seven monoliths.".into(),
                tags: vec!["leyline".into()],
            }],
            routes: vec![Route {
                destination_id: "watch-keep".into(),
                travel_time: "4 hours".into(),
                terrain_difficulty: "Uphill".into(),
                status: "Open".into(),
                notes: String::new(),
            }],
            tags: vec!["wilderness".into(), "leyline".into()],
            notes: String::new(),
            created_at_world_time: String::new(),
        })
        .unwrap();

    let mut stats = BTreeMap::new();
    stats.insert("Agility".into(), 15);
    stats.insert("Spirit".into(), 14);
    let mut resources = BTreeMap::new();
    resources.insert("HP".into(), ResourcePool { current: 40, max: 40, unit: String::new() });
    resources.insert("Qi".into(), ResourcePool { current: 30, max: 30, unit: "pts".into() });

    engine
        .create_character(CharacterBaseline {
            id: "aria-vale".into(),
            name: "Aria Vale".into(),
            title: "Wanderer".into(),
            kind: "pc".into(),
            origin: "Human".into(),
            archetype: "Blade Scout".into(),
            faction: "Unaligned".into(),
            appearance: "Dark cloak.".into(),
            personality: "Observant.".into(),
            backstory: "Former scout.".into(),
            initial_location: Some("watch-keep".into()),
            initial_stats: stats,
            initial_resources: resources,
            initial_equipment: vec![Item {
                name: "Iron Blade".into(),
                slot: Some("Weapon".into()),
                quantity: 1,
                equipped: true,
                durability: Some("100%".into()),
                tags: vec!["weapon".into()],
                description: "Balanced blade.".into(),
                ..Default::default()
            }],
            initial_skills: vec![Skill {
                name: "Tracking".into(),
                rank: "Adept".into(),
                category: "Survival".into(),
                modifier: Some("+3".into()),
                description: "Follow faint trails.".into(),
                governing_stat: None,
                xp: 0,
                proficiency_tier: None,
            }],
            initial_techniques: vec![Technique {
                name: "Ash Step".into(),
                tier: "Tier I".into(),
                cost: "6 Qi".into(),
                cooldown: "1 turn".into(),
                mastery: "Practiced".into(),
                effect: "Quick dash.".into(),
                description: String::new(),
                source_text_id: None,
                source_item_id: None,
                progression_track_id: None,
                manual_title: String::new(),
                manual_notes: String::new(),
            }],
            initial_goals: vec!["Investigate the Old Shrine".into()],
            tags: vec!["pc".into()],
            notes: String::new(),
            created_at_world_time: String::new(),
        })
        .unwrap();

    let mut npc_res = BTreeMap::new();
    npc_res.insert("HP".into(), ResourcePool { current: 45, max: 45, unit: String::new() });
    engine
        .create_character(CharacterBaseline {
            id: "archivist-bren".into(),
            name: "Archivist Bren".into(),
            title: "Keeper of Scrolls".into(),
            kind: "npc".into(),
            origin: "Human".into(),
            archetype: "Scholar".into(),
            faction: "Wardens".into(),
            appearance: "Ink-stained cuffs.".into(),
            personality: "Meticulous.".into(),
            backstory: "Curator of the Archive Vault.".into(),
            initial_location: Some("watch-keep".into()),
            initial_stats: BTreeMap::new(),
            initial_resources: npc_res,
            initial_equipment: vec![],
            initial_skills: vec![],
            initial_techniques: vec![Technique {
                name: "Seal Ward".into(),
                tier: "Tier II".into(),
                cost: "8 Qi".into(),
                cooldown: "Scene".into(),
                mastery: "Mastered".into(),
                effect: "Reinforce protective runes.".into(),
                description: String::new(),
                source_text_id: None,
                source_item_id: None,
                progression_track_id: None,
                manual_title: String::new(),
                manual_notes: String::new(),
            }],
            initial_goals: vec![],
            tags: vec!["scholar".into()],
            notes: String::new(),
            created_at_world_time: String::new(),
        })
        .unwrap();

    engine
        .upsert_lore(
            LoreEntry {
                id: "leyline-laws".into(),
                title: "Laws of Leyline Channeling".into(),
                category: "magic-system".into(),
                summary: "Core magic axiom.".into(),
                body: "Channeling beyond equilibrium inflicts Resonance Burn.".into(),
                keywords: vec!["leyline".into(), "qi".into()],
                tags: vec!["magic".into()],
                visibility: "public".into(),
                always_include: true,
                related_ids: vec!["glass-hollow".into()],
                created_at_world_time: String::new(),
                updated_at_world_time: String::new(),
            },
            "Core world law",
        )
        .unwrap();

    engine
        .upsert_lore(
            LoreEntry {
                id: "shrine-secret".into(),
                title: "Secret Origin of the Shrine Echoes".into(),
                category: "secrets".into(),
                summary: "GM Secret about the shrine.".into(),
                body: "The echoes are containment guardians, not invaders.".into(),
                keywords: vec!["echo".into(), "shrine".into()],
                tags: vec!["secret".into()],
                visibility: "gm_secret".into(),
                always_include: false,
                related_ids: vec!["glass-hollow".into()],
                created_at_world_time: String::new(),
                updated_at_world_time: String::new(),
            },
            "Hidden truth",
        )
        .unwrap();

    // Story-relevant Books & Scrolls (Ritual Scroll + Historical Chronicle)
    engine
        .upsert_text(
            StoryText {
                id: "scroll-of-binding".into(),
                title: "Scroll of the Seventh Binding Ritual".into(),
                medium: "scroll".into(),
                subject: "spell-ritual".into(),
                author: "Magister Orin".into(),
                language: "Ancient Runic".into(),
                condition: "Charred Edges".into(),
                read_status: "discovered_unread".into(),
                read_by: vec![],
                holder_or_location_id: Some("glass-hollow".into()),
                summary: "A copper-cased parchment scroll covered in ash-sealed runes.".into(),
                content: "### Ritual Steps\n1. Draw a seven-point circle of obsidian dust.\n2. Speak the harmonic counter-verse at dusk.\n3. Ground excess resonance into running water.".into(),
                mechanics_or_unlocks: vec![
                    "Unlocks Spell Ritual: Harmonic Binding (stabilizes awakened shrines)".into(),
                ],
                gm_notes: "Requires Ancient Runic fluency or Mind check TN 15 to decipher safely.".into(),
                keywords: vec!["binding".into(), "ritual".into(), "shrine".into()],
                tags: vec!["ritual".into(), "ancient".into()],
                related_ids: vec!["glass-hollow".into(), "leyline-laws".into()],
                created_at_world_time: String::new(),
                updated_at_world_time: String::new(),
            },
            "Placed at shrine",
        )
        .unwrap();

    engine
        .upsert_text(
            StoryText {
                id: "annals-of-the-accord".into(),
                title: "Annals of the First Accord".into(),
                medium: "book".into(),
                subject: "history".into(),
                author: "High Chronicler Vael".into(),
                language: "Common".into(),
                condition: "Intact".into(),
                read_status: "read".into(),
                read_by: vec!["archivist-bren".into()],
                holder_or_location_id: Some("watch-keep".into()),
                summary: "Heavy leather-bound volume detailing the founding of the border keeps.".into(),
                content: "In Year 12, seven monoliths were raised to calm the volcanic frontier.".into(),
                mechanics_or_unlocks: vec!["Reveals historical origin of the seven monoliths".into()],
                gm_notes: "Omits the Synod's secret excavation treaty.".into(),
                keywords: vec!["accord".into(), "monoliths".into()],
                tags: vec!["history".into()],
                related_ids: vec!["watch-keep".into()],
                created_at_world_time: String::new(),
                updated_at_world_time: String::new(),
            },
            "Cataloged in Archive Vault",
        )
        .unwrap();

    engine
        .upsert_relation(GraphRelation {
            source_id: "aria-vale".into(),
            source_kind: "character".into(),
            relation_type: "TRUSTS".into(),
            target_id: "archivist-bren".into(),
            target_kind: "character".into(),
            strength: Some(30),
            notes: "Old mentor.".into(),
            updated_at_world_time: String::new(),
            dimensions: Default::default(),
            direction: "directed".into(),
            visibility: "public".into(),
            origin_event_id: None,
            obligations: vec![],
        })
        .unwrap();

    engine
        .create_clock(ProgressClock {
            id: "shrine-pulse".into(),
            name: "Shrine Resonance Surge".into(),
            kind: "quest".into(),
            filled: 2,
            segments: 6,
            status: "active".into(),
            description: "Stabilize the Old Shrine before it fractures.".into(),
            related_ids: vec!["glass-hollow".into(), "aria-vale".into()],
            ..Default::default()
        })
        .unwrap();

    // Play events
    engine.advance_time(240, Some("Ash-Mist".into()), Some("Travel to hollow".into())).unwrap();
    engine.move_character("aria-vale", "glass-hollow", "Scouted forest road").unwrap();
    engine
        .roll_and_optionally_log("2d10+4", Some("aria-vale".into()), Some("glass-hollow".into()), "Shrine ambush", true, Some(42))
        .unwrap();
    engine.modify_resource("aria-vale", "HP", Some(-9), None, None, None, "Echo strike").unwrap();
    engine
        .add_condition(
            "aria-vale",
            StatusCondition {
                name: "Resonance Burn".into(),
                kind: "debuff".into(),
                severity: "Minor".into(),
                duration: "Until rest".into(),
                source: "Overdraw".into(),
                notes: "-1 Spirit".into(),
            },
        )
        .unwrap();
    engine
        .add_item(
            "aria-vale",
            Item {
                name: "Tuning Shard".into(),
                slot: None,
                quantity: 1,
                equipped: false,
                durability: None,
                tags: vec!["relic".into()],
                description: "Hums near monoliths.".into(),
                ..Default::default()
            },
            "Looted at shrine",
        )
        .unwrap();
    engine
        .upsert_technique(
            "aria-vale",
            Technique {
                name: "Ash Step".into(),
                tier: "Tier I".into(),
                cost: "5 Qi".into(),
                cooldown: "1 turn".into(),
                mastery: "Refined".into(),
                effect: "Faster dash.".into(),
                description: String::new(),
                source_text_id: None,
                source_item_id: None,
                progression_track_id: None,
                manual_title: String::new(),
                manual_notes: String::new(),
            },
            "Breakthrough",
        )
        .unwrap();
    engine
        .upsert_landmark(
            "glass-hollow",
            Landmark {
                name: "Old Shrine".into(),
                kind: "Ruins".into(),
                status: "Awakened".into(),
                description: "Monoliths glowing gold.".into(),
                tags: vec!["leyline".into()],
            },
            "Shrine activated",
        )
        .unwrap();

    engine.set_auto_compile(true);
    engine.compile_docs().unwrap();
}

#[test]
fn test_fresh_empty_campaign_initialization() {
    let tmp = TempDir::new().unwrap();
    let engine = CampaignEngine::open(tmp.path()).unwrap();
    let meta = engine
        .init_campaign("Fresh World", "Blank campaign ready for play", "Custom", None)
        .unwrap();
    assert_eq!(meta.name, "Fresh World");
    assert!(engine.list_characters().unwrap().is_empty());
    assert!(engine.list_places().unwrap().is_empty());
    assert!(engine.db().list_lore(None).unwrap().is_empty());
    assert!(engine.db().list_texts().unwrap().is_empty());

    let idx = std::fs::read_to_string(tmp.path().join("docs/INDEX.md")).unwrap();
    assert!(idx.contains("# Campaign Codex: Fresh World"));
    assert!(idx.contains("## Books, Scrolls & Story Texts"));

    let (status, _, body) = dispatch_http(&engine, "GET", "/api/status", b"");
    assert_eq!(status, 200);
    let st: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(st["counts"]["characters"], 0);
    assert_eq!(st["counts"]["story_texts"], 0);
}

#[test]
fn test_projection_markdown_texts_and_hash_chain() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    let (valid, count) = engine.db().verify_event_chain().unwrap();
    assert!(valid);
    assert!(count >= 15);

    // Record character reading & picking up the ritual scroll
    let (updated_text, _) = engine
        .record_text_reading(
            "scroll-of-binding",
            Some("Aria".into()),
            Some("deciphered".into()),
            Some("Aria".into()),
            None,
            "Deciphered the ritual steps by torchlight",
        )
        .unwrap();
    assert_eq!(updated_text.read_status, "deciphered");
    assert!(updated_text.read_by.contains(&"aria-vale".to_string()));
    assert_eq!(updated_text.holder_or_location_id.as_deref(), Some("aria-vale"));

    // Verify Character Markdown has 3 ordered sections + reading event in log
    let char_md = std::fs::read_to_string(tmp.path().join("docs/characters/aria-vale.md")).unwrap();
    let p1 = char_md.find("## 1. Initial / Baseline Profile").unwrap();
    let p2 = char_md.find("## 2. Current Status").unwrap();
    let p3 = char_md.find("## 3. Event & Impact Log").unwrap();
    assert!(p1 < p2 && p2 < p3);
    assert!(char_md.contains("Scroll of the Seventh Binding Ritual"));

    // Verify Place Markdown
    let place_md = std::fs::read_to_string(tmp.path().join("docs/places/glass-hollow.md")).unwrap();
    assert!(place_md.contains("## 1. Baseline Geography & Significance"));
    assert!(place_md.contains("## 2. Current Status"));
    assert!(place_md.contains("## 3. Place Event & History Log"));
    assert!(place_md.contains("[Aria Vale](../characters/aria-vale.md)"));

    // Verify specialized Books/Scrolls Markdown
    let text_md = std::fs::read_to_string(tmp.path().join("docs/texts/scroll-of-binding.md")).unwrap();
    assert!(text_md.contains("## 1. Artifact & Bibliographic Profile"));
    assert!(text_md.contains("## 2. Readable In-World Text & Passages"));
    assert!(text_md.contains("## 3. Story Revelations, Rituals & Unlocks"));
    assert!(text_md.contains("## 4. Provenance & Reading Log"));
    assert!(text_md.contains("Harmonic Binding"));
    assert!(text_md.contains("deciphered"));
}

#[test]
fn test_scene_context_texts_lore_and_season_rollover() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    let ctx = engine
        .get_scene_context(&SceneContextRequest {
            place_id: Some("Glass Hollow".into()),
            character_ids: vec![],
            scene_text: "Aria examines the echo near the shrine ritual scroll.".into(),
            recent_events_limit: 5,
            include_gm_secrets: true,
            perspective: None,
        })
        .unwrap();

    assert!(ctx.triggered_lore.iter().any(|l| l.id == "shrine-secret"));
    assert!(ctx.relevant_texts.iter().any(|t| t.id == "scroll-of-binding"));

    // Player-safe view strips gm_secret lore and text gm_notes
    let safe_ctx = engine
        .get_scene_context(&SceneContextRequest {
            place_id: Some("glass-hollow".into()),
            character_ids: vec![],
            scene_text: "echo ritual".into(),
            recent_events_limit: 5,
            include_gm_secrets: false,
            perspective: None,
        })
        .unwrap();
    assert!(!safe_ctx.triggered_lore.iter().any(|l| l.id == "shrine-secret"));
    let scroll = safe_ctx.relevant_texts.iter().find(|t| t.id == "scroll-of-binding").unwrap();
    assert!(scroll.gm_notes.is_empty());

    // Advance 50 days -> rolls Autumn into Winter
    let (clock, changed, _) = engine.advance_time(50 * 1440, Some("Blizzard".into()), None).unwrap();
    assert!(changed);
    assert_eq!(clock.season, "Winter");
}

#[test]
fn test_time_travel_and_rollback() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    let events = engine.db().list_all_events().unwrap();
    let pre_dmg_seq = events
        .iter()
        .find(|e| e.summary.contains("Echo strike"))
        .unwrap()
        .seq
        - 1;

    let hist = engine.get_character("aria-vale", Some(pre_dmg_seq)).unwrap();
    assert_eq!(hist.current.resources["HP"].current, 40);

    let (backup, _) = engine.rollback_to_seq(pre_dmg_seq).unwrap();
    assert!(backup.exists());
    let rewound = engine.get_character("aria-vale", None).unwrap();
    assert_eq!(rewound.current.resources["HP"].current, 40);
    assert!(rewound.current.conditions.is_empty());
}

#[test]
fn test_search_by_texts_batch_turn_and_ai_exports() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    // Search Characters By
    let injured = engine
        .search_characters_by(&CharacterSearchFilter {
            has_condition: Some("Resonance Burn".into()),
            resource_name: Some("HP".into()),
            resource_below_pct: Some(85),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(injured.len(), 1);

    // Search Places By
    let awakened = engine
        .search_places_by(&PlaceSearchFilter {
            terrain: Some("vitrified".into()),
            landmark_status: Some("Awakened".into()),
            has_character: Some("aria".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(awakened.len(), 1);

    // Search Books/Scrolls By medium, subject, language, read_status, holder
    let ritual_scrolls = engine
        .search_texts_by(&TextSearchFilter {
            medium: Some("scroll".into()),
            subject: Some("spell-ritual".into()),
            language: Some("Runic".into()),
            read_status: Some("discovered_unread".into()),
            holder_or_location_id: Some("glass-hollow".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(ritual_scrolls.len(), 1);
    assert_eq!(ritual_scrolls[0].id, "scroll-of-binding");

    // Search Books read by Archivist Bren
    let bren_books = engine
        .search_texts_by(&TextSearchFilter {
            read_by: Some("archivist-bren".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(bren_books.len(), 1);
    assert_eq!(bren_books[0].id, "annals-of-the-accord");

    // Search Lore, Events, Relations, Omni
    assert_eq!(
        engine
            .search_lore_by(&LoreSearchFilter {
                visibility: Some("gm_secret".into()),
                ..Default::default()
            })
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        engine
            .search_events_by(&EventSearchFilter {
                event_type: Some("dice_rolled".into()),
                ..Default::default()
            })
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        engine
            .search_relations_by(&RelationSearchFilter {
                entity_id: Some("aria-vale".into()),
                min_strength: Some(20),
                hops: Some(2),
                ..Default::default()
            })
            .unwrap()
            .len(),
        1
    );
    let omni = engine.search_omni("ritual", 10).unwrap();
    assert_eq!(omni.texts.len(), 1);

    // Batch commit_turn: record_text_reading + transfer_item + rest_characters
    let batch = execute_tool(
        &engine,
        "commit_turn",
        json!({
            "actions": [
                {
                    "tool": "record_text_reading",
                    "arguments": {
                        "text_id": "scroll-of-binding",
                        "reader_id": "Aria",
                        "read_status": "read",
                        "holder_or_location_id": "Aria"
                    }
                },
                {
                    "tool": "transfer_item",
                    "arguments": {
                        "from_character_id": "Aria",
                        "to_character_id": "Bren",
                        "item_name": "Tuning Shard",
                        "quantity": 1
                    }
                },
                {
                    "tool": "rest_characters",
                    "arguments": {
                        "character_ids": ["Aria"],
                        "rest_type": "long",
                        "clear_conditions": ["Resonance Burn"]
                    }
                }
            ]
        }),
    )
    .unwrap();
    assert_eq!(batch["committed_actions"], 3);

    // Verify universal schema exports (35 tools)
    assert!(export_tool_schemas("openai").unwrap().as_array().unwrap().len() >= 34);
    assert!(export_tool_schemas("anthropic").unwrap().as_array().unwrap().len() >= 34);
}

#[test]
fn test_web_gui_rest_and_mcp_stdio() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    // Web GUI & REST endpoints
    let (status, _, body) = dispatch_http(&engine, "GET", "/", b"");
    assert_eq!(status, 200);
    let html = String::from_utf8(body).unwrap();
    assert!(html.contains("Books &amp; Scrolls"));

    let (status, _, body) = dispatch_http(&engine, "GET", "/api/texts?subject=spell-ritual", b"");
    assert_eq!(status, 200);
    let texts_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(texts_json["count"], 1);
    assert_eq!(texts_json["texts"][0]["id"], "scroll-of-binding");

    let (status, _, body) = dispatch_http(&engine, "GET", "/api/docs/texts/scroll-of-binding.md", b"");
    assert_eq!(status, 200);
    assert!(String::from_utf8(body).unwrap().contains("Seventh Binding Ritual"));

    let (status, _, body) = dispatch_http(&engine, "GET", "/api/openapi.json", b"");
    assert_eq!(status, 200);
    let spec: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(spec["paths"]["/api/tool/upsert_text"].is_object());
    assert!(spec["paths"]["/api/tool/search_texts"].is_object());

    // MCP stdio protocol roundtrip including chronicle://texts/<id> resource
    drop(engine);
    let bin = env!("CARGO_BIN_EXE_chronicle");
    let mut child = Command::new(bin)
        .args(["mcp", "--campaign", tmp.path().to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to launch chronicle mcp");
    {
        let stdin = child.stdin.as_mut().unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-03-26"}}}}"#).unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#).unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{{}}}}"#).unwrap();
        writeln!(stdin, r#"{{"jsonrpc":"2.0","id":3,"method":"resources/read","params":{{"uri":"chronicle://texts/scroll-of-binding"}}}}"#).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "chronicle mcp failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<String> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(lines.len(), 3);
    let r2: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
    assert!(r2["result"]["tools"].as_array().unwrap().len() >= 34);
    let r3: serde_json::Value = serde_json::from_str(&lines[2]).unwrap();
    assert!(r3["result"]["contents"][0]["text"]
        .as_str()
        .unwrap()
        .contains("Scroll of the Seventh Binding Ritual"));
}

#[test]
fn test_chatgpt_streamable_http_mcp_annotations_search_and_fetch() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    // 1. Streamable HTTP: initialize
    let (status, ct, body) = dispatch_http(
        &engine,
        "POST",
        "/mcp",
        br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"chatgpt-test","version":"1.0"}}}"#,
    );
    assert_eq!(status, 200);
    assert!(ct.starts_with("application/json"));
    let init: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "chronicle-rpg-gm");

    // 2. Streamable HTTP: notification returns 202 Accepted with empty body
    let (status, _, body) = dispatch_http(
        &engine,
        "POST",
        "/mcp",
        br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    );
    assert_eq!(status, 202);
    assert!(body.is_empty());

    // 3. Streamable HTTP: tools/list includes annotations on ALL tools + ChatGPT search/fetch
    let (status, _, body) = dispatch_http(
        &engine,
        "POST",
        "/mcp",
        br#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
    );
    assert_eq!(status, 200);
    let list_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let tools = list_resp["result"]["tools"].as_array().unwrap();
    assert!(tools.len() >= 36);
    for t in tools {
        let ann = t.get("annotations").expect("every tool must have MCP annotations for ChatGPT");
        assert!(ann["readOnlyHint"].is_boolean());
        assert!(ann["destructiveHint"].is_boolean());
        assert!(ann["openWorldHint"].is_boolean());
    }
    let search_tool = tools.iter().find(|t| t["name"] == "search").unwrap();
    assert_eq!(search_tool["annotations"]["readOnlyHint"], true);
    let rollback_tool = tools.iter().find(|t| t["name"] == "rollback_to_event").unwrap();
    assert_eq!(rollback_tool["annotations"]["destructiveHint"], true);
    let commit_tool = tools.iter().find(|t| t["name"] == "commit_turn").unwrap();
    assert_eq!(commit_tool["annotations"]["readOnlyHint"], false);

    // 4. ChatGPT Connector standard `search` tool (returns structuredContent + JSON content)
    let (status, _, body) = dispatch_http(
        &engine,
        "POST",
        "/mcp",
        br#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search","arguments":{"query":"ritual"}}}"#,
    );
    assert_eq!(status, 200);
    let search_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(search_resp["result"]["isError"], false);
    let results = search_resp["result"]["structuredContent"]["results"].as_array().unwrap();
    assert!(results.iter().any(|r| r["id"] == "text:scroll-of-binding"));
    let first_url = results[0]["url"].as_str().unwrap();
    assert!(first_url.starts_with("http://127.0.0.1:7878/api/docs/"));
    // Verify text content is also valid JSON matching structuredContent
    let text_json: serde_json::Value =
        serde_json::from_str(search_resp["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text_json["results"].as_array().unwrap().len(), results.len());

    // 5. ChatGPT Connector standard `fetch` tool
    let (status, _, body) = dispatch_http(
        &engine,
        "POST",
        "/mcp",
        br#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"fetch","arguments":{"id":"text:scroll-of-binding"}}}"#,
    );
    assert_eq!(status, 200);
    let fetch_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let sc = &fetch_resp["result"]["structuredContent"];
    assert_eq!(sc["id"], "text:scroll-of-binding");
    assert_eq!(sc["title"], "Scroll of the Seventh Binding Ritual");
    assert!(sc["text"].as_str().unwrap().contains("Harmonic Binding"));
    assert_eq!(sc["metadata"]["medium"], "scroll");

    // 6. Streamable HTTP batch request + DELETE session teardown
    let (status, _, body) = dispatch_http(
        &engine,
        "POST",
        "/mcp",
        br#"[{"jsonrpc":"2.0","id":5,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"}]"#,
    );
    assert_eq!(status, 200);
    let batch: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(batch.as_array().unwrap().len(), 1);
    assert_eq!(batch[0]["id"], 5);

    let (status, _, _) = dispatch_http(&engine, "DELETE", "/mcp", b"");
    assert_eq!(status, 200);
}

#[test]
fn test_attached_reference_docs_copy_and_addressable_retrieval() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    // Create an external source file outside the campaign dir to simulate a chat attachment
    let ext_dir = TempDir::new().unwrap();
    let src_file = ext_dir.path().join("frontier-map-notes.md");
    std::fs::write(
        &src_file,
        "# Frontier Pass Notes\nThree mountain routes lead past Watch Keep into Glass Hollow.",
    )
    .unwrap();

    // 1. Copy external file into campaign via MCP `import_attachment` tool
    let imported = execute_tool(
        &engine,
        "import_attachment",
        json!({
            "title": "Frontier Map Notes",
            "source_path": src_file.to_str().unwrap(),
            "category": "map",
            "summary": "Player-supplied regional route notes.",
            "extracted_notes": "Lists three mountain routes between Watch Keep and Glass Hollow.",
            "keywords": ["frontier", "mountain routes"],
            "related_ids": ["glass-hollow", "watch-keep"]
        }),
    )
    .unwrap();

    assert_eq!(imported["id"], "frontier-map-notes");
    assert_eq!(imported["reference_id"], "attachment:frontier-map-notes");
    assert_eq!(imported["mcp_uri"], "chronicle://attachments/frontier-map-notes");
    assert_eq!(
        imported["relative_path"],
        "docs/attachments/files/frontier-map-notes.md"
    );

    // Verify both the copied raw file AND the companion reference card exist without colliding!
    assert!(tmp
        .path()
        .join("docs/attachments/files/frontier-map-notes.md")
        .exists());
    let card_md =
        std::fs::read_to_string(tmp.path().join("docs/attachments/frontier-map-notes.md"))
            .unwrap();
    assert!(card_md.contains("## 1. Addressable File Reference"));
    assert!(card_md.contains("files/frontier-map-notes.md"));

    // 2. Also test binary base64 attachment (1x1 PNG-like bytes)
    let bin_imported = execute_tool(
        &engine,
        "import_attachment",
        json!({
            "title": "Shrine Sketch",
            "filename": "shrine.png",
            "base64_content": "data:image/png;base64,iVBORw0KGgo=",
            "category": "handout",
            "summary": "Hand-drawn sketch of the seven monoliths.",
            "related_ids": ["glass-hollow"]
        }),
    )
    .unwrap();
    assert_eq!(
        bin_imported["relative_path"],
        "docs/attachments/files/shrine-sketch.png"
    );

    // 3. Retrieve via `get_attachment` & `search_attachments` & `get_scene_context`
    let fetched = execute_tool(
        &engine,
        "get_attachment",
        json!({ "attachment_id": "frontier-map-notes" }),
    )
    .unwrap();
    assert!(fetched["text_content"]
        .as_str()
        .unwrap()
        .contains("Three mountain routes"));

    let found = execute_tool(
        &engine,
        "search_attachments",
        json!({ "category": "map", "related_to": "glass-hollow" }),
    )
    .unwrap();
    assert_eq!(found["count"], 1);

    let ctx = engine
        .get_scene_context(&SceneContextRequest {
            place_id: Some("glass-hollow".into()),
            character_ids: vec![],
            scene_text: "Check the frontier mountain routes".into(),
            recent_events_limit: 5,
            include_gm_secrets: true,
            perspective: None,
        })
        .unwrap();
    assert!(ctx
        .relevant_attachments
        .iter()
        .any(|a| a.id == "frontier-map-notes"));

    // 4. Verify HTTP file serving (/api/attachments/<id>/file) & ChatGPT MCP search/fetch
    let (status, ct, bytes) = dispatch_http(&engine, "GET", "/api/attachments/shrine-sketch/file", b"");
    assert_eq!(status, 200);
    assert_eq!(ct, "image/png");
    assert_eq!(&bytes[..4], &[0x89, b'P', b'N', b'G']);

    let (status, _, body) = dispatch_http(
        &engine,
        "POST",
        "/mcp",
        br#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"fetch","arguments":{"id":"attachment:frontier-map-notes"}}}"#,
    );
    assert_eq!(status, 200);
    let resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(resp["result"]["structuredContent"]["text"]
        .as_str()
        .unwrap()
        .contains("Three mountain routes"));
}

#[test]
fn test_carryable_books_items_histories_and_create_reference_doc() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    // 1. Verify physical books/scrolls/manuals/records created via upsert_text are automatically
    //    synced as carryable CatalogItems AND synced into character inventories when held/transferred!
    let scroll_item = engine.get_catalog_item("scroll-of-binding").unwrap();
    assert_eq!(scroll_item.linked_text_id.as_deref(), Some("scroll-of-binding"));
    assert_eq!(scroll_item.current_holder_id.as_deref(), Some("glass-hollow"));

    // Aria picks up and reads the scroll -> should sync into Aria's inventory AND update CatalogItem previous_owners
    execute_tool(
        &engine,
        "record_text_reading",
        json!({
            "text_id": "scroll-of-binding",
            "reader_id": "aria-vale",
            "read_status": "read",
            "holder_or_location_id": "aria-vale",
            "notes": "Aria tucked the scroll into her satchel."
        }),
    )
    .unwrap();
    let aria = engine.get_character("aria-vale", None).unwrap();
    assert!(
        aria.current
            .equipment
            .iter()
            .any(|e| e.name == "Scroll of the Seventh Binding Ritual"),
        "Carryable book/scroll should appear in Aria's carried inventory"
    );
    let scroll_item_after = engine.get_catalog_item("scroll-of-binding").unwrap();
    assert_eq!(scroll_item_after.current_holder_id.as_deref(), Some("aria-vale"));
    assert!(scroll_item_after
        .previous_owners
        .contains(&"glass-hollow".to_string()));

    // 2. Catalog a weapon/keepsake/artifact with origin, previous owners, significance, and holder
    let cat_res = execute_tool(
        &engine,
        "upsert_item",
        json!({
            "name": "Starlight Compass",
            "item_type": "keepsake",
            "rarity_or_tier": "Heirloom",
            "slot": "Belt",
            "equipped": true,
            "condition_or_durability": "Tarnished but intact",
            "origin_and_maker": "Crafted by Master Horologist Vex in the First Age",
            "previous_owners": ["Captain Sera", "Elder Mirek"],
            "current_holder_id": "aria-vale",
            "significance": "Points toward dormant Ley monoliths regardless of magnetic interference.",
            "properties_and_effects": "+2 to wilderness navigation near Ley anomalies.",
            "description": "A palm-sized brass compass with an obsidian needle.",
            "keywords": ["compass", "starlight", "navigation"],
            "tags": ["heirloom", "tool"]
        }),
    )
    .unwrap();
    assert_eq!(cat_res["item"]["id"], "starlight-compass");
    assert!(tmp.path().join("docs/items/starlight-compass.md").exists());

    // Confirm Starlight Compass synced into Aria's equipment
    let aria2 = engine.get_character("aria-vale", None).unwrap();
    assert!(aria2
        .current
        .equipment
        .iter()
        .any(|e| e.name == "Starlight Compass" && e.equipped));

    // Transfer Starlight Compass from Aria to Archivist Bren -> updates previous_owners & both inventories
    execute_tool(
        &engine,
        "transfer_catalog_item",
        json!({
            "item_id": "starlight-compass",
            "to_holder_id": "archivist-bren",
            "condition_or_durability": "Glass cracked in transit",
            "notes": "Entrusted to Bren for calibration at Watch Keep."
        }),
    )
    .unwrap();
    let compass_after = engine.get_catalog_item("starlight-compass").unwrap();
    assert_eq!(
        compass_after.current_holder_id.as_deref(),
        Some("archivist-bren")
    );
    assert_eq!(
        compass_after.condition_or_durability,
        "Glass cracked in transit"
    );
    assert!(compass_after
        .previous_owners
        .contains(&"aria-vale".to_string()));
    let aria3 = engine.get_character("aria-vale", None).unwrap();
    assert!(!aria3
        .current
        .equipment
        .iter()
        .any(|e| e.name == "Starlight Compass"));
    let bren = engine.get_character("archivist-bren", None).unwrap();
    assert!(bren
        .current
        .equipment
        .iter()
        .any(|e| e.name == "Starlight Compass"));

    // Search items by previous_owner and item_type
    let found_items = execute_tool(
        &engine,
        "search_items",
        json!({
            "item_type": "keepsake",
            "previous_owner": "aria-vale"
        }),
    )
    .unwrap();
    assert_eq!(found_items["count"], 1);
    assert_eq!(found_items["items"][0]["id"], "starlight-compass");

    // 3. Record Historical Chronicles for a Place, an Establishment, a Sect, and an Item
    execute_tool(
        &engine,
        "add_entity_history",
        json!({
            "entity_id": "glass-hollow",
            "entity_kind": "place",
            "era_or_date": "First Age, Year 14",
            "sort_order": 10,
            "title": "The Vitrification Cataclysm",
            "details": "A runaway Ley discharge turned the entire basin into black glass overnight.",
            "significance": "Explains why no flora grows inside the hollow and why echoes linger.",
            "visibility": "public"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "add_entity_history",
        json!({
            "entity_id": "watch-keep",
            "entity_kind": "establishment",
            "era_or_date": "Second Age, Year 88",
            "sort_order": 20,
            "title": "Chartering of the Lantern Guildhall",
            "details": "Three frontier cartographers established Watch Keep's archival wing as neutral ground.",
            "significance": "Grants diplomatic immunity to visiting archivists.",
            "visibility": "public"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "add_entity_history",
        json!({
            "entity_id": "amber-accord-sect",
            "entity_kind": "sect",
            "era_or_date": "Second Age, Year 201",
            "sort_order": 30,
            "title": "The Schism of the Silent Bell",
            "details": "Half the Amber Accord sect broke away after disputing the Seventh Binding oath.",
            "significance": "Splinter cells still hunt relic bearers across the northern ridge.",
            "visibility": "gm_secret"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "add_entity_history",
        json!({
            "entity_id": "starlight-compass",
            "entity_kind": "item",
            "era_or_date": "Second Age, Year 310",
            "sort_order": 40,
            "title": "Lost at the Siege of Ashford",
            "details": "Captain Sera dropped the compass while evacuating civilians through the storm.",
            "significance": "Marks the dent along its eastern bezel.",
            "visibility": "public"
        }),
    )
    .unwrap();

    // Verify compiled docs include ## Historical Chronicle sections
    let place_md =
        std::fs::read_to_string(tmp.path().join("docs/places/glass-hollow.md")).unwrap();
    assert!(place_md.contains("## Historical Chronicle"));
    assert!(place_md.contains("The Vitrification Cataclysm"));

    let item_md =
        std::fs::read_to_string(tmp.path().join("docs/items/starlight-compass.md")).unwrap();
    assert!(item_md.contains("## Historical Chronicle"));
    assert!(item_md.contains("Lost at the Siege of Ashford"));
    assert!(item_md.contains("Captain Sera"));

    // Search histories by entity_kind and FTS5 query
    let sect_hists = execute_tool(
        &engine,
        "search_histories",
        json!({ "entity_kind": "sect", "query": "Schism" }),
    )
    .unwrap();
    assert_eq!(sect_hists["count"], 1);
    assert_eq!(sect_hists["histories"][0]["entity_id"], "amber-accord-sect");

    // 4. Create a new reference doc from pasted content (create if missing + append mode)
    let created_doc = execute_tool(
        &engine,
        "create_reference_doc",
        json!({
            "title": "Sect Charter Notes",
            "filename": "sect-charter.md",
            "content": "# Amber Accord Charter\nArticle I: Never ring the seventh bell without three witnesses.",
            "category": "world-doc",
            "summary": "Pasted reference notes covering the Amber Accord bylaws.",
            "keywords": ["charter", "bylaws"],
            "related_ids": ["amber-accord-sect"]
        }),
    )
    .unwrap();
    assert_eq!(created_doc["id"], "sect-charter-notes");
    assert_eq!(
        created_doc["relative_path"],
        "docs/attachments/files/sect-charter-notes.md"
    );

    // Append more pasted notes to the same reference file
    execute_tool(
        &engine,
        "create_reference_doc",
        json!({
            "id": "sect-charter-notes",
            "title": "Sect Charter Notes",
            "content": "Article II: Relics recovered from Glass Hollow must be logged at Watch Keep.",
            "append": true
        }),
    )
    .unwrap();
    let doc_text = std::fs::read_to_string(
        tmp.path()
            .join("docs/attachments/files/sect-charter-notes.md"),
    )
    .unwrap();
    assert!(doc_text.contains("Article I"));
    assert!(doc_text.contains("Article II"));

    // 5. Verify HTTP REST endpoints (/api/items, /api/items/<id>, /api/histories) + ChatGPT MCP fetch for item:<id>
    let (status, _, body) = dispatch_http(&engine, "GET", "/api/items?item_type=keepsake", b"");
    assert_eq!(status, 200);
    let items_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(items_json["count"], 1);

    let (status, _, body) = dispatch_http(&engine, "GET", "/api/items/starlight-compass", b"");
    assert_eq!(status, 200);
    let item_detail: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(item_detail["histories"].as_array().unwrap().len(), 1);

    let (status, _, body) = dispatch_http(&engine, "GET", "/api/histories?entity_kind=establishment", b"");
    assert_eq!(status, 200);
    let hist_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(hist_json["count"], 1);

    let (status, _, body) = dispatch_http(
        &engine,
        "POST",
        "/mcp",
        br#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"fetch","arguments":{"id":"item:starlight-compass"}}}"#,
    );
    assert_eq!(status, 200);
    let fetch_item: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(fetch_item["result"]["structuredContent"]["text"]
        .as_str()
        .unwrap()
        .contains("Master Horologist Vex"));
}

#[test]
fn test_diagnostics_editing_reclassify_merge_and_cleanup() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    // Deliberately introduce mislabeled data, typos, a history kind mismatch, a duplicate lore entry,
    // a broken relation, and a stale orphaned .md file on disk!
    execute_tool(
        &engine,
        "upsert_lore",
        json!({
            "title": "Grimoire of Ash Echoes",
            "category": "books",
            "summary": "A soot-stained manual accidentally filed in lore.",
            "body": "### Incantation\nSpeak three names before the monolith.",
            "keywords": ["grimoire", "ash", "ash"]
        }),
    )
    .unwrap();

    execute_tool(
        &engine,
        "upsert_lore",
        json!({
            "title": "Sunfire Lantern",
            "category": "artifacts",
            "summary": "A brass lantern that reveals illusions.",
            "body": "Forged during the Second Age."
        }),
    )
    .unwrap();

    // History attached to aria-vale (a character) but mislabeled as entity_kind: "place"
    let bad_hist = execute_tool(
        &engine,
        "add_entity_history",
        json!({
            "entity_id": "aria-vale",
            "entity_kind": "place",
            "era_or_date": "Year 400",
            "title": "Sworn to the Watch",
            "details": "Aria took her oath at Watch Keep."
        }),
    )
    .unwrap();
    let bad_hist_id = bad_hist["history"]["id"].as_i64().unwrap();

    // Duplicate lore entry with same title under a different slug
    execute_tool(
        &engine,
        "upsert_lore",
        json!({
            "id": "leyline-laws-dup",
            "title": "Laws of Leyline Channeling",
            "category": "magic-system",
            "summary": "Duplicate copy",
            "body": "Extra detail about harmonic rebound."
        }),
    )
    .unwrap();

    // Stale orphaned doc file left in docs/
    let orphan_path = tmp.path().join("docs/lore/stale-ghost.md");
    std::fs::write(&orphan_path, "# Ghost Doc\n").unwrap();

    // 1. Run `diagnose_campaign` (and GET /api/diagnostics) -> must catch all of these!
    let diag = execute_tool(&engine, "diagnose_campaign", json!({})).unwrap();
    assert_eq!(diag["healthy"], false);
    let findings = diag["findings"].as_array().unwrap();
    assert!(findings.iter().any(|f| f["code"] == "MISLABELED_LORE_AS_TEXT" && f["entity_id"] == "grimoire-of-ash-echoes"));
    assert!(findings.iter().any(|f| f["code"] == "MISLABELED_LORE_AS_ITEM" && f["entity_id"] == "sunfire-lantern"));
    assert!(findings.iter().any(|f| f["code"] == "HISTORY_KIND_MISMATCH"));
    assert!(findings.iter().any(|f| f["code"] == "DUPLICATE_NAME_SAME_REGISTER" && f["register"] == "lore"));
    assert!(findings.iter().any(|f| f["code"] == "ORPHANED_DOC_FILE" && f["entity_id"] == "lore/stale-ghost.md"));

    let (status, _, body) = dispatch_http(&engine, "GET", "/api/diagnostics", b"");
    assert_eq!(status, 200);
    let http_diag: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(http_diag["healthy"], false);

    // 2. Reclassify mislabeled lore -> text AND lore -> item
    let rc_text = execute_tool(
        &engine,
        "reclassify_entity",
        json!({
            "from_register": "lore",
            "entity_id": "grimoire-of-ash-echoes",
            "to_register": "text",
            "reason": "Move physical book from lore into texts"
        }),
    )
    .unwrap();
    assert_eq!(rc_text["to_id"], "grimoire-of-ash-echoes");
    assert_eq!(rc_text["removed_source"], true);
    assert!(engine.db().get_lore("grimoire-of-ash-echoes").unwrap().is_none());
    assert!(engine.get_text("grimoire-of-ash-echoes").is_ok());
    // Also auto-synced as carryable CatalogItem!
    assert!(engine.get_catalog_item("grimoire-of-ash-echoes").is_ok());

    let rc_item = execute_tool(
        &engine,
        "reclassify_entity",
        json!({
            "from_register": "lore",
            "entity_id": "sunfire-lantern",
            "to_register": "item",
            "overrides": { "item_type": "artifact", "current_holder_id": "aria-vale" },
            "reason": "Move artifact from lore into carryable items"
        }),
    )
    .unwrap();
    assert_eq!(rc_item["to_id"], "sunfire-lantern");
    assert!(engine.db().get_lore("sunfire-lantern").unwrap().is_none());
    let lantern = engine.get_catalog_item("sunfire-lantern").unwrap();
    assert_eq!(lantern.current_holder_id.as_deref(), Some("aria-vale"));

    // 3. Edit character baseline, place baseline, and fix mislabeled entity_history kind
    execute_tool(
        &engine,
        "update_character_baseline",
        json!({
            "character_id": "aria-vale",
            "title": "Senior Echo Scout",
            "reason": "Promoted after expedition"
        }),
    )
    .unwrap();
    assert_eq!(
        engine.get_character("aria-vale", None).unwrap().baseline.title,
        "Senior Echo Scout"
    );

    execute_tool(
        &engine,
        "update_place_baseline",
        json!({
            "place_id": "glass-hollow",
            "climate": "Cold alpine basin",
            "reason": "Clarified climate"
        }),
    )
    .unwrap();
    assert_eq!(
        engine.get_place("glass-hollow", None).unwrap().baseline.climate,
        "Cold alpine basin"
    );

    execute_tool(
        &engine,
        "update_entity_history",
        json!({
            "history_id": bad_hist_id,
            "entity_kind": "character",
            "reason": "Fixed mislabeled entity_kind"
        }),
    )
    .unwrap();
    assert_eq!(
        engine.db().get_entity_history_by_id(bad_hist_id).unwrap().unwrap().entity_kind,
        "character"
    );

    // 4. Merge duplicate lore entry into canonical `leyline-laws`
    let merged = execute_tool(
        &engine,
        "rename_or_merge_entity",
        json!({
            "register": "lore",
            "old_id": "leyline-laws-dup",
            "new_id": "leyline-laws",
            "allow_merge": true,
            "reason": "Merged duplicate lore entry"
        }),
    )
    .unwrap();
    assert_eq!(merged["merged"], true);
    assert!(engine.db().get_lore("leyline-laws-dup").unwrap().is_none());
    assert!(engine
        .db()
        .get_lore("leyline-laws")
        .unwrap()
        .unwrap()
        .body
        .contains("Extra detail about harmonic rebound"));

    // 5. Run cleanup_campaign (dry_run first, then apply) and verify orphan doc pruned & campaign healthy!
    let preview = execute_tool(&engine, "cleanup_campaign", json!({ "dry_run": true })).unwrap();
    assert_eq!(preview["dry_run"], true);
    let applied = execute_tool(&engine, "cleanup_campaign", json!({ "dry_run": false })).unwrap();
    assert_eq!(applied["dry_run"], false);
    assert!(!orphan_path.exists());

    // 6. Test clean delete_entity (remove the temporary history entry)
    let deleted = execute_tool(
        &engine,
        "delete_entity",
        json!({
            "register": "history",
            "entity_id": bad_hist_id.to_string(),
            "reason": "Cleanup test entry"
        }),
    )
    .unwrap();
    assert_eq!(deleted["deleted"], true);
    assert!(engine.db().get_entity_history_by_id(bad_hist_id).unwrap().is_none());

    // Also fix the 2 broken parent_place references ("north-marches") caught by diagnostics!
    execute_tool(
        &engine,
        "create_place",
        json!({
            "id": "north-marches",
            "name": "North Marches",
            "place_type": "region",
            "terrain": "Subalpine frontier highlands"
        }),
    )
    .unwrap();

    // 7. Re-run diagnostics: 0 errors, 0 warnings, healthy == true, and SHA-256 chain still 100% valid!
    let diag_after = execute_tool(&engine, "diagnose_campaign", json!({})).unwrap();
    assert_eq!(diag_after["error_count"], 0);
    assert_eq!(diag_after["warning_count"], 0);
    assert_eq!(diag_after["healthy"], true);
    assert_eq!(diag_after["ledger_hash_chain_valid"], true);

    // 8. Test update_campaign_meta, get_lore, manage_clock(update), and backup_campaign
    let meta_res = execute_tool(
        &engine,
        "update_campaign_meta",
        json!({
            "name": "Echoes of the North Marches",
            "description": "High-frontier investigation campaign",
            "ruleset_notes": "2d10 + skill mod",
            "reason": "Replaced placeholder campaign metadata"
        }),
    )
    .unwrap();
    assert_eq!(meta_res["campaign"]["name"], "Echoes of the North Marches");

    let lore_res = execute_tool(&engine, "get_lore", json!({ "lore_id": "leyline-laws" })).unwrap();
    assert_eq!(lore_res["lore"]["id"], "leyline-laws");
    assert!(lore_res["doc_path"].as_str().unwrap().contains("leyline-laws.md"));

    let clock_upd = execute_tool(
        &engine,
        "manage_clock",
        json!({
            "action": "update",
            "clock_id": "shrine-pulse",
            "segments": 8,
            "description": "Expanded threat track for the northern monoliths."
        }),
    )
    .unwrap();
    assert_eq!(clock_upd["clock"]["segments"], 8);

    let backup_res = execute_tool(&engine, "backup_campaign", json!({ "label": "pre-audit" })).unwrap();
    let backup_path = std::path::PathBuf::from(backup_res["backup_path"].as_str().unwrap());
    assert!(backup_path.exists());
    let backup_filename = backup_path.file_name().unwrap().to_str().unwrap().to_string();

    // 9. Verify rollback_to_event cleanly unwinds newly created lore, histories, items, relations AND prunes stale .md docs!
    let seq_before_temp = engine.db().list_all_events().unwrap().last().unwrap().seq;
    execute_tool(
        &engine,
        "upsert_lore",
        json!({
            "id": "temp-guild-rollback-test",
            "title": "Temporary Guild",
            "category": "commerce",
            "body": "Created solely to test full-register rollback."
        }),
    )
    .unwrap();
    let hist_temp = execute_tool(
        &engine,
        "add_entity_history",
        json!({
            "entity_id": "temp-guild-rollback-test",
            "entity_kind": "faction",
            "era_or_date": "Year 1",
            "title": "Temp Founding",
            "details": "Should vanish on rollback."
        }),
    )
    .unwrap();
    let hist_temp_id = hist_temp["history"]["id"].as_i64().unwrap();
    assert!(tmp
        .path()
        .join("docs/lore/commerce/temp-guild-rollback-test.md")
        .exists());

    execute_tool(
        &engine,
        "rollback_to_event",
        json!({ "to_seq": seq_before_temp }),
    )
    .unwrap();
    assert!(engine.db().get_lore("temp-guild-rollback-test").unwrap().is_none());
    assert!(engine.db().get_entity_history_by_id(hist_temp_id).unwrap().is_none());
    assert!(!tmp
        .path()
        .join("docs/lore/commerce/temp-guild-rollback-test.md")
        .exists());

    // 10. Verify restore_backup lists snapshots and restores bit-for-bit on a live connection
    let list_res = execute_tool(&engine, "restore_backup", json!({})).unwrap();
    assert!(list_res["count"].as_u64().unwrap() >= 1);
    let restore_res = execute_tool(
        &engine,
        "restore_backup",
        json!({ "backup_filename": backup_filename }),
    )
    .unwrap();
    assert!(restore_res["total_events"].as_u64().unwrap() >= 1);
}

#[test]
fn test_oauth21_pkce_dcr_and_protected_hosted_mcp() {
    use chronicle::oauth::{base64url_nopad, OAuthStore};
    use chronicle::web::dispatch_http_with_oauth;
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};

    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    let oauth = Arc::new(Mutex::new(OAuthStore::new("gm-secret-passphrase".to_string())));
    let origin = "https://chronicle-colab.trycloudflare.com";

    // 1. Unauthenticated POST /mcp must return 401 + WWW-Authenticate pointing to RFC 9728 metadata
    let (status, _, hdrs, _) = dispatch_http_with_oauth(
        &engine,
        &oauth,
        origin,
        "POST",
        "/mcp",
        BTreeMap::new(),
        br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
    );
    assert_eq!(status, 401);
    let www = hdrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("www-authenticate"))
        .map(|(_, v)| v.as_str())
        .unwrap();
    assert!(www.contains("resource_metadata=\"https://chronicle-colab.trycloudflare.com/.well-known/oauth-protected-resource\""));

    // 2. Discover RFC 9728 Protected Resource Metadata & RFC 8414 Authorization Server Metadata
    let (status, _, _, body) = dispatch_http_with_oauth(
        &engine,
        &oauth,
        origin,
        "GET",
        "/.well-known/oauth-protected-resource",
        BTreeMap::new(),
        b"",
    );
    assert_eq!(status, 200);
    let prm: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(prm["resource"], "https://chronicle-colab.trycloudflare.com/mcp");

    let (status, _, _, body) = dispatch_http_with_oauth(
        &engine,
        &oauth,
        origin,
        "GET",
        "/.well-known/oauth-authorization-server",
        BTreeMap::new(),
        b"",
    );
    assert_eq!(status, 200);
    let asm: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(asm["token_endpoint"], "https://chronicle-colab.trycloudflare.com/oauth/token");
    assert_eq!(asm["registration_endpoint"], "https://chronicle-colab.trycloudflare.com/oauth/register");

    // 3. RFC 7591 Dynamic Client Registration
    let (status, _, _, body) = dispatch_http_with_oauth(
        &engine,
        &oauth,
        origin,
        "POST",
        "/oauth/register",
        BTreeMap::new(),
        br#"{"client_name":"ChatGPT Remote MCP","redirect_uris":["https://chatgpt.com/connector_platform_oauth_redirect"],"token_endpoint_auth_method":"none"}"#,
    );
    assert_eq!(status, 201);
    let reg: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let client_id = reg["client_id"].as_str().unwrap().to_string();

    // 4. Generate PKCE S256 verifier + challenge and load /oauth/authorize consent screen
    let verifier = "test-pkce-verifier-string-with-high-entropy-1234567890";
    let challenge = base64url_nopad(&Sha256::digest(verifier.as_bytes()));

    let auth_get_url = format!(
        "/oauth/authorize?response_type=code&client_id={client_id}&redirect_uri=https%3A%2F%2Fchatgpt.com%2Fconnector_platform_oauth_redirect&code_challenge={challenge}&code_challenge_method=S256&state=xyz123"
    );
    let (status, _, _, body) = dispatch_http_with_oauth(
        &engine,
        &oauth,
        origin,
        "GET",
        &auth_get_url,
        BTreeMap::new(),
        b"",
    );
    assert_eq!(status, 200);
    assert!(String::from_utf8(body).unwrap().contains("ChatGPT Remote MCP"));

    // 5. Submit consent form with WRONG passphrase -> 403 Forbidden
    let bad_form = format!(
        "client_id={client_id}&redirect_uri=https%3A%2F%2Fchatgpt.com%2Fconnector_platform_oauth_redirect&state=xyz123&scope=mcp%3Atools&code_challenge={challenge}&code_challenge_method=S256&passphrase=wrong&decision=approve"
    );
    let (status, _, _, _) = dispatch_http_with_oauth(
        &engine,
        &oauth,
        origin,
        "POST",
        "/oauth/authorize",
        BTreeMap::new(),
        bad_form.as_bytes(),
    );
    assert_eq!(status, 403);

    // 6. Submit consent form with CORRECT passphrase -> 302 redirect with ?code=...&state=xyz123
    let ok_form = format!(
        "client_id={client_id}&redirect_uri=https%3A%2F%2Fchatgpt.com%2Fconnector_platform_oauth_redirect&state=xyz123&scope=mcp%3Atools&code_challenge={challenge}&code_challenge_method=S256&passphrase=gm-secret-passphrase&decision=approve"
    );
    let (status, _, hdrs, _) = dispatch_http_with_oauth(
        &engine,
        &oauth,
        origin,
        "POST",
        "/oauth/authorize",
        BTreeMap::new(),
        ok_form.as_bytes(),
    );
    assert_eq!(status, 302);
    let loc = hdrs
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("location"))
        .map(|(_, v)| v.as_str())
        .unwrap();
    assert!(loc.contains("state=xyz123"));
    let code = loc
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_string();

    // 7. Exchange code + PKCE verifier at POST /oauth/token
    let token_form = format!(
        "grant_type=authorization_code&code={code}&redirect_uri=https%3A%2F%2Fchatgpt.com%2Fconnector_platform_oauth_redirect&client_id={client_id}&code_verifier={verifier}"
    );
    let (status, _, _, body) = dispatch_http_with_oauth(
        &engine,
        &oauth,
        origin,
        "POST",
        "/oauth/token",
        BTreeMap::new(),
        token_form.as_bytes(),
    );
    assert_eq!(status, 200);
    let tok: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let access_token = tok["access_token"].as_str().unwrap().to_string();
    let refresh_token = tok["refresh_token"].as_str().unwrap().to_string();

    // 8. Authenticated POST /mcp succeeds with the OAuth access_token!
    let mut auth_hdrs = BTreeMap::new();
    auth_hdrs.insert("authorization".to_string(), format!("Bearer {access_token}"));
    let (status, _, _, body) = dispatch_http_with_oauth(
        &engine,
        &oauth,
        origin,
        "POST",
        "/mcp",
        auth_hdrs,
        br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"campaign_status","arguments":{}}}"#,
    );
    assert_eq!(status, 200);
    let mcp_resp: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(mcp_resp["result"]["isError"], false);

    // 9. Refresh token rotation works too
    let refresh_form = format!("grant_type=refresh_token&refresh_token={refresh_token}");
    let (status, _, _, body) = dispatch_http_with_oauth(
        &engine,
        &oauth,
        origin,
        "POST",
        "/oauth/token",
        BTreeMap::new(),
        refresh_form.as_bytes(),
    );
    assert_eq!(status, 200);
    let tok2: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(tok2["access_token"].as_str().unwrap().starts_with("chr_at_"));
}

#[test]
fn test_chunked_transfer_encoding_and_well_known_variants() {
    use chronicle::oauth::OAuthStore;
    use chronicle::web::{dispatch_http_with_oauth, read_chunked_body};
    use std::collections::BTreeMap;
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    // 1. Verify RFC 9112 chunked transfer decoding (used by cloudflared for HTTP/2 -> HTTP/1.1 POSTs)
    let raw_chunked = b"11;ext=1\r\n{\"jsonrpc\":\"2.0\",\r\n16\r\n\"id\":1,\"method\":\"ping\"\r\n1\r\n}\r\n0\r\nX-Trailer: ok\r\n\r\n";
    let mut cursor = Cursor::new(&raw_chunked[..]);
    let decoded = read_chunked_body(&mut cursor).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(parsed["method"], "ping");
    assert_eq!(parsed["id"], 1);

    // 2. Verify additional OAuth discovery path variants, HEAD /health, /mcp/ trailing slash & SSE-only Accept
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);
    let oauth = Arc::new(Mutex::new(OAuthStore::new("gm-pass".to_string())));
    let origin = "https://chronicle.example.com";

    for path in [
        "/.well-known/oauth-protected-resource/mcp",
        "/.well-known/oauth-authorization-server/mcp",
        "/mcp/.well-known/oauth-authorization-server",
        "/.well-known/openid-configuration/mcp",
        "/mcp/.well-known/openid-configuration",
    ] {
        let (status, _, _, body) = dispatch_http_with_oauth(
            &engine,
            &oauth,
            origin,
            "GET",
            path,
            BTreeMap::new(),
            b"",
        );
        assert_eq!(status, 200, "Discovery path {path} should return 200");
        assert!(!body.is_empty());
    }

    let (status, _, _, body) = dispatch_http_with_oauth(
        &engine,
        &oauth,
        origin,
        "HEAD",
        "/health",
        BTreeMap::new(),
        b"",
    );
    assert_eq!(status, 200);
    assert!(body.is_empty());

    // Trailing slash /mcp/ must also enforce auth (401 without token)
    let (status, _, _, _) = dispatch_http_with_oauth(
        &engine,
        &oauth,
        origin,
        "POST",
        "/mcp/",
        BTreeMap::new(),
        br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#,
    );
    assert_eq!(status, 401);

    // SSE-only Accept header on POST /mcp wraps JSON-RPC response in text/event-stream
    let mut sse_hdrs = BTreeMap::new();
    sse_hdrs.insert("accept".to_string(), "text/event-stream".to_string());
    let (status, ct, _, body) = {
        // Test unauthenticated SSE via dispatch_http (no OAuth) or mint token via OAuth
        let (st, _, _, reg_body) = dispatch_http_with_oauth(
            &engine,
            &oauth,
            origin,
            "POST",
            "/oauth/register",
            BTreeMap::new(),
            br#"{"client_name":"SSE Client","redirect_uris":["https://example.com/cb"]}"#,
        );
        assert_eq!(st, 201);
        let cid = serde_json::from_slice::<serde_json::Value>(&reg_body).unwrap()["client_id"]
            .as_str()
            .unwrap()
            .to_string();
        let form = format!("client_id={cid}&redirect_uri=https%3A%2F%2Fexample.com%2Fcb&passphrase=gm-pass&decision=approve");
        let (st, _, hdrs, _) = dispatch_http_with_oauth(
            &engine,
            &oauth,
            origin,
            "POST",
            "/oauth/authorize",
            BTreeMap::new(),
            form.as_bytes(),
        );
        assert_eq!(st, 302);
        let loc = hdrs.iter().find(|(k, _)| k.eq_ignore_ascii_case("location")).unwrap().1.clone();
        let code = loc.split("code=").nth(1).unwrap().split('&').next().unwrap().to_string();
        let tok_form = format!("grant_type=authorization_code&code={code}&redirect_uri=https%3A%2F%2Fexample.com%2Fcb");
        let (st, _, _, tok_body) = dispatch_http_with_oauth(
            &engine,
            &oauth,
            origin,
            "POST",
            "/oauth/token",
            BTreeMap::new(),
            tok_form.as_bytes(),
        );
        assert_eq!(st, 200);
        let at = serde_json::from_slice::<serde_json::Value>(&tok_body).unwrap()["access_token"]
            .as_str()
            .unwrap()
            .to_string();
        sse_hdrs.insert("authorization".to_string(), format!("Bearer {at}"));
        dispatch_http_with_oauth(
            &engine,
            &oauth,
            origin,
            "POST",
            "/mcp/",
            sse_hdrs,
            br#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#,
        )
    };
    assert_eq!(status, 200);
    assert!(ct.starts_with("text/event-stream"));
    let body_str = String::from_utf8(body).unwrap();
    assert!(body_str.starts_with("event: message\ndata: "));
    assert!(body_str.contains("\"id\":7"));
}

#[test]
fn test_phase1_resolution_profiles_tables_staged_loot_and_encounters() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    // =========================================================================
    // ACCEPTANCE TEST A: Mortal Skill Check with live skill, injury, gear & wet weather
    // =========================================================================
    // Give Aria a climbing skill, a hemp rope (+1), a moderate injury (-2), and wet stormy ridge weather
    execute_tool(
        &engine,
        "upsert_character_skill",
        json!({
            "character_id": "aria-vale",
            "name": "Climbing and rough terrain",
            "rank": "Apprentice",
            "modifier": "+2",
            "category": "Physical"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "update_character_equipment",
        json!({
            "character_id": "aria-vale",
            "action": "add",
            "name": "Hemp Climbing Rope",
            "equipped": true,
            "tags": ["climbing", "+1"],
            "description": "Braided hemp rope (+1 to climbing checks)"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "update_character_condition",
        json!({
            "character_id": "aria-vale",
            "action": "add",
            "name": "Sprained Wrist",
            "kind": "injury",
            "severity": "moderate",
            "notes": "Throbs when gripping wet rock"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "update_place",
        json!({
            "place_id": "glass-hollow",
            "action": "weather",
            "weather": "Heavy Rain"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "update_place",
        json!({
            "place_id": "glass-hollow",
            "action": "add_condition",
            "condition": "Loose Wet Ridge"
        }),
    )
    .unwrap();

    // 1a. Dry-run preview computes itemized hydrated modifiers & odds WITHOUT logging an event
    let events_before = engine.db().list_all_events().unwrap().len();
    let preview = execute_tool(
        &engine,
        "resolve_check",
        json!({
            "actor_id": "aria-vale",
            "action": "Climb the loose wet ridge path",
            "skill": "Climbing and rough terrain",
            "equipment": ["Hemp Climbing Rope"],
            "difficulty": 14,
            "stakes": "Failure may cause a fall and injury",
            "dry_run": true
        }),
    )
    .unwrap();
    assert_eq!(preview["dry_run"], true);
    assert_eq!(preview["outcome"], "preview");
    assert!(preview["event_id"].is_null());
    assert!(preview["odds_preview"]["success_pct"].as_f64().is_some());
    assert_eq!(engine.db().list_all_events().unwrap().len(), events_before);

    // Check every expected hydrated modifier is present in the breakdown!
    let mods = preview["modifiers"].as_array().unwrap();
    assert!(mods.iter().any(|m| m["source"] == "skill:Climbing and rough terrain" && m["value"] == 2));
    assert!(mods.iter().any(|m| m["source"] == "equipment:Hemp Climbing Rope" && m["value"] == 1));
    assert!(mods.iter().any(|m| m["source"] == "condition:Sprained Wrist" && m["value"] == -2));
    assert!(mods.iter().any(|m| m["source"] == "weather:Heavy Rain" && m["value"] == -2));
    assert!(mods.iter().any(|m| m["source"] == "place_condition:Loose Wet Ridge" && m["value"] == -1));
    let expected_sum: i64 = mods.iter().map(|m| m["value"].as_i64().unwrap()).sum();
    assert_eq!(preview["modifier"], expected_sum);

    // 1b. Live deterministic seeded check + causal consequence link (Acceptance Test G)
    let check_res = execute_tool(
        &engine,
        "resolve_check",
        json!({
            "actor_id": "aria-vale",
            "action": "Climb the loose wet ridge path",
            "skill": "Climbing and rough terrain",
            "equipment": ["Hemp Climbing Rope"],
            "difficulty": 14,
            "stakes": "Failure may cause a fall and injury",
            "seed": 424242,
            "log_to_ledger": true
        }),
    )
    .unwrap();
    let check_ev_id = check_res["event_id"].as_i64().unwrap();
    assert_eq!(check_res["seed"], 424242);
    assert_eq!(check_res["modifier"], expected_sum);

    // Link a follow-up narrative consequence via causal_event_ids
    let consequence_ev = execute_tool(
        &engine,
        "log_event",
        json!({
            "title": "Ridge Climb Consequence",
            "details": "Shale slides beneath Aria's boots during the climb.",
            "actor_id": "aria-vale",
            "location_id": "glass-hollow",
            "causal_event_ids": [check_ev_id]
        }),
    )
    .unwrap();
    let children = engine.db().list_causal_children(check_ev_id).unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].seq, consequence_ev["event_seq"].as_i64().unwrap());

    // Confirm injury persists in subsequent scene context retrieval (Acceptance Test G)
    let ctx = execute_tool(
        &engine,
        "get_scene_context",
        json!({ "place_id": "glass-hollow", "character_ids": ["aria-vale"], "format": "markdown" }),
    )
    .unwrap();
    let md_str = ctx.as_str().unwrap();
    assert!(md_str.contains("Sprained Wrist"));

    // =========================================================================
    // SYSTEM-AGNOSTIC PROFILES (PbtA 2d6, Roll-Under d100, Dice Pool Nd6, Step Dice)
    // =========================================================================
    execute_tool(
        &engine,
        "upsert_ruleset_profile",
        json!({
            "id": "pbta-2d6",
            "name": "PbtA 2d6 Tiered",
            "resolution_mode": "roll_over",
            "base_dice": "2d6",
            "pbta_full_success_at": 10,
            "pbta_partial_success_at": 7,
            "auto_hydrate": false
        }),
    )
    .unwrap();
    let pbta = execute_tool(
        &engine,
        "resolve_check",
        json!({
            "action": "Read a tense room",
            "profile": "pbta-2d6",
            "difficulty": 10,
            "modifier": 1,
            "seed": 11
        }),
    )
    .unwrap();
    assert!(matches!(
        pbta["outcome"].as_str().unwrap(),
        "critical_success" | "success" | "partial_success" | "failure" | "critical_failure"
    ));

    execute_tool(
        &engine,
        "upsert_ruleset_profile",
        json!({
            "id": "brp-d100",
            "name": "BRP Percentile Roll-Under",
            "resolution_mode": "roll_under",
            "base_dice": "1d100",
            "crit_success_natural": 1,
            "crit_failure_natural": 100,
            "auto_hydrate": false
        }),
    )
    .unwrap();
    let d100 = execute_tool(
        &engine,
        "resolve_check",
        json!({
            "action": "Decipher ancient seal",
            "profile": "brp-d100",
            "difficulty": 65,
            "seed": 77
        }),
    )
    .unwrap();
    assert_eq!(d100["resolution_mode"], "roll_under");

    execute_tool(
        &engine,
        "upsert_ruleset_profile",
        json!({
            "id": "shadow-pool",
            "name": "d6 Dice Pool",
            "resolution_mode": "dice_pool",
            "base_dice": "4d6",
            "pool_success_threshold": 5,
            "auto_hydrate": false
        }),
    )
    .unwrap();
    let pool = execute_tool(
        &engine,
        "resolve_check",
        json!({
            "action": "Channel formation energy",
            "profile": "shadow-pool",
            "difficulty": 2,
            "modifier": 1,
            "advantage": "advantage",
            "seed": 99
        }),
    )
    .unwrap();
    assert_eq!(pool["expression"], "6d6"); // 4 base + 1 mod + 1 adv

    // =========================================================================
    // CONTESTED / OPPOSED CHECK (`resolve_contest`)
    // =========================================================================
    let contest = execute_tool(
        &engine,
        "resolve_contest",
        json!({
            "action": "Sneak past Archivist Bren",
            "initiator": { "actor_id": "aria-vale", "role": "sneaker", "modifier": 3 },
            "defender": { "actor_id": "archivist-bren", "role": "watcher", "modifier": 1 },
            "stakes": "Alarm raised if caught",
            "seed": 2026
        }),
    )
    .unwrap();
    assert_eq!(contest["participants"].as_array().unwrap().len(), 2);
    assert!(contest["event_id"].as_i64().is_some());

    // =========================================================================
    // ACCEPTANCE TEST B: Contextual Loot Draw + Staged Discovery + Atomic Claim + Stateful Deck
    // =========================================================================
    execute_tool(
        &engine,
        "upsert_random_table",
        json!({
            "id": "cart-trinkets",
            "name": "Merchant Cart Hidden Pouch",
            "category": "loot",
            "entries": [
                {
                    "id": "silver-ingot",
                    "result": "Stamped silver trade ingot ({{1d3}} taels)",
                    "weight": 10,
                    "rarity": "uncommon",
                    "loot_template": {
                        "name": "Stamped Silver Ingot",
                        "item_type": "treasure",
                        "rarity_or_tier": "Uncommon",
                        "quantity_dice": "1d3",
                        "description": "Merchant-stamped silver ingot"
                    }
                }
            ]
        }),
    )
    .unwrap();

    let tbl_res = execute_tool(
        &engine,
        "upsert_random_table",
        json!({
            "id": "merchant-cart-loot",
            "name": "Salt & Grain Merchant Cart Cargo",
            "category": "loot",
            "context": ["Old Imperial Road", "Merchant Cart"],
            "entries": [
                {
                    "id": "coarse-salt",
                    "result": "{{2d4}} sacks of coarse river salt",
                    "weight": 40,
                    "rarity": "common",
                    "conditions": { "tags": ["merchant-cart"] },
                    "loot_template": {
                        "name": "Coarse River Salt Sack",
                        "item_type": "trade-good",
                        "quantity_dice": "2d4",
                        "description": "Heavy burlap sack of river salt"
                    }
                },
                {
                    "id": "hidden-pouch",
                    "result": "False-bottom driver lockbox",
                    "weight": 15,
                    "rarity": "rare",
                    "conditions": { "tags": ["merchant-cart"] },
                    "subtable_id": "cart-trinkets",
                    "loot_template": {
                        "name": "Driver's Iron Lockbox",
                        "item_type": "container",
                        "rarity_or_tier": "Rare",
                        "description": "Pried from beneath the merchant's seat"
                    }
                },
                {
                    "id": "winter-furs-only",
                    "result": "Northern sable pelts",
                    "weight": 50,
                    "rarity": "uncommon",
                    "conditions": { "seasons": ["Winter"] }
                }
            ]
        }),
    )
    .unwrap();
    assert!(tmp.path().join("docs/tables/merchant-cart-loot.md").exists());
    assert_eq!(tbl_res["table"]["version"], 1);

    // Draw 2 items without replacement in Spring -> winter-furs-only MUST be filtered out!
    let aria_items_before = engine.get_character("aria-vale", None).unwrap().current.equipment.len();
    let draw = execute_tool(
        &engine,
        "draw_random_table",
        json!({
            "table_id": "merchant-cart-loot",
            "context": {
                "location": "glass-hollow",
                "tags": ["merchant-cart"],
                "party": ["aria-vale"],
                "source_description": "Overturned salt merchant cart on Old Imperial Road"
            },
            "count": 2,
            "replacement": false,
            "seed": 48219
        }),
    )
    .unwrap();
    assert_eq!(draw["eligible_entry_count"], 2); // Only the 2 merchant-cart entries, NOT Winter-only pelts
    assert_eq!(draw["draws"].as_array().unwrap().len(), 2);
    // CRITICAL: Loot MUST NOT be auto-granted into Aria's inventory on draw!
    assert_eq!(
        engine.get_character("aria-vale", None).unwrap().current.equipment.len(),
        aria_items_before
    );
    let disc_id = draw["staged_discovery"]["id"].as_str().unwrap().to_string();
    assert_eq!(draw["staged_discovery"]["status"], "pending");

    // Claim only item #0 into Aria's inventory
    let claim = execute_tool(
        &engine,
        "claim_discovery",
        json!({
            "discovery_id": disc_id,
            "claimant_id": "aria-vale",
            "item_indices": [0],
            "notes": "Aria loaded the cargo onto her pack mule."
        }),
    )
    .unwrap();
    assert_eq!(claim["claimed_items"].as_array().unwrap().len(), 1);
    assert_eq!(claim["discovery"]["status"], "partially_claimed");
    assert!(
        engine.get_character("aria-vale", None).unwrap().current.equipment.len()
            > aria_items_before
    );

    // Double-claiming item #0 MUST fail atomically!
    let dup_err = execute_tool(
        &engine,
        "claim_discovery",
        json!({
            "discovery_id": disc_id,
            "claimant_id": "archivist-bren",
            "item_indices": [0]
        }),
    );
    assert!(dup_err.is_err(), "Re-claiming an already claimed discovery item must fail");

    // Stateful without-replacement deck check: drawing again from the same deck_scope yields 0 entries until reset!
    let empty_draw = execute_tool(
        &engine,
        "draw_random_table",
        json!({
            "table_id": "merchant-cart-loot",
            "context": { "location": "glass-hollow", "tags": ["merchant-cart"] },
            "count": 1,
            "replacement": false,
            "seed": 55
        }),
    )
    .unwrap();
    assert_eq!(empty_draw["draws"].as_array().unwrap().len(), 0);
    let reset_draw = execute_tool(
        &engine,
        "draw_random_table",
        json!({
            "table_id": "merchant-cart-loot",
            "context": { "location": "glass-hollow", "tags": ["merchant-cart"] },
            "count": 1,
            "replacement": false,
            "reset_deck": true,
            "seed": 55
        }),
    )
    .unwrap();
    assert_eq!(reset_draw["draws"].as_array().unwrap().len(), 1);

    // =========================================================================
    // ENCOUNTER & INITIATIVE MANAGEMENT (Non-Combat + Live State Sync)
    // =========================================================================
    let enc_start = execute_tool(
        &engine,
        "manage_encounter",
        json!({
            "action": "start",
            "title": "Toll Bridge Standoff",
            "encounter_type": "negotiation",
            "location_id": "glass-hollow",
            "zones": ["Bridge Approach", "Toll Gate", "Riverbank"],
            "initiative_dice": "1d20",
            "seed": 1337,
            "participants": [
                { "entity_id": "aria-vale", "name": "Aria Vale", "side": "party", "initiative": 3, "position": "Bridge Approach" },
                { "entity_id": "archivist-bren", "name": "Archivist Bren", "side": "neutral", "initiative": 1, "position": "Toll Gate" }
            ]
        }),
    )
    .unwrap();
    let enc_id = enc_start["encounter"]["id"].as_str().unwrap().to_string();
    assert!(tmp.path().join(format!("docs/encounters/{enc_id}.md")).exists());

    // Take an action with a resource delta & condition, then spend a reaction
    execute_tool(
        &engine,
        "manage_encounter",
        json!({
            "action": "act",
            "encounter_id": enc_id,
            "actor_id": "aria-vale",
            "action_type": "action",
            "description": "Offers silver toll while stepping closer to the gate",
            "target_ids": ["archivist-bren"],
            "new_position": "Toll Gate",
            "resource_deltas": [{ "character_id": "aria-vale", "resource": "Stamina", "delta": -2 }],
            "advance_turn": true
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "manage_encounter",
        json!({
            "action": "act",
            "encounter_id": enc_id,
            "actor_id": "aria-vale",
            "action_type": "reaction",
            "description": "Catches Bren's ledger before the wind blows it away"
        }),
    )
    .unwrap();
    // Second reaction in the same round must be blocked!
    let second_react = execute_tool(
        &engine,
        "manage_encounter",
        json!({
            "action": "act",
            "encounter_id": enc_id,
            "actor_id": "aria-vale",
            "action_type": "reaction",
            "description": "Tries a second reaction illegally"
        }),
    );
    assert!(second_react.is_err());

    // Advance turn -> wraps to Round 2 and refreshes reaction!
    let adv = execute_tool(
        &engine,
        "manage_encounter",
        json!({ "action": "advance_turn", "encounter_id": enc_id, "advance_world_minutes": 1 }),
    )
    .unwrap();
    assert!(adv["round"].as_u64().unwrap() >= 1);

    // End encounter
    let ended = execute_tool(
        &engine,
        "manage_encounter",
        json!({
            "action": "end",
            "encounter_id": enc_id,
            "outcome": "Safe passage negotiated",
            "aftermath": "Both sides agreed to share road intelligence."
        }),
    )
    .unwrap();
    assert_eq!(ended["encounter"]["status"], "ended");

    // Verify full SHA-256 hash chain integrity across all events!
    let (chain_ok, total_events) = engine.db().verify_event_chain().unwrap();
    assert!(chain_ok, "SHA-256 event hash chain must remain valid");
    assert!(total_events > 10);
}

#[test]
fn test_phase2_epistemic_knowledge_autonomy_factions_and_pov_context() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    // Create a merchant witness character for Acceptance Test C & an additional band member for Tests D & E
    execute_tool(
        &engine,
        "create_character",
        json!({
            "id": "merchant-lin",
            "name": "Merchant Lin",
            "kind": "npc",
            "initial_location": "glass-hollow",
            "archetype": "Salt Merchant"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "create_character",
        json!({
            "id": "gao-dahu",
            "name": "Gao Dahu",
            "kind": "npc",
            "initial_location": "watch-keep",
            "archetype": "Camp Lieutenant"
        }),
    )
    .unwrap();

    // =========================================================================
    // ACCEPTANCE TEST C & H: Partial Witness Knowledge & Hard GM/PC Secret Wall
    // =========================================================================
    // 1. GM-secret claim about Aria's hidden motive & stolen manual (ONLY Aria knows, GM secret!)
    execute_tool(
        &engine,
        "record_knowledge",
        json!({
            "claim_id": "aria-secret-manual-plan",
            "subject": "aria-vale",
            "claim": "Aria secretly plans to trade the stolen binding scroll to a hidden sect contact.",
            "truth_status": "true",
            "visibility": "gm_secret",
            "known_by": ["aria-vale"],
            "stance": "concealing",
            "confidence": 100
        }),
    )
    .unwrap();

    // 2. Partial observation by Merchant Lin (saw masked ambushers, NOT identities or motives!)
    execute_tool(
        &engine,
        "record_knowledge",
        json!({
            "claim_id": "road-ambush-01",
            "subject": "glass-hollow",
            "claim": "Aria Vale and Gao Dahu ambushed the salt cart using the north pine ridge.",
            "truth_status": "true",
            "visibility": "restricted",
            "beliefs": [
                {
                    "claim_id": "",
                    "actor_id": "merchant-lin",
                    "stance": "observed",
                    "confidence": 85,
                    "perceived_claim": "Two masked figures in soot-stained cloaks ambushed my cart near the fork.",
                    "source": "direct-observation"
                }
            ]
        }),
    )
    .unwrap();

    // 3. Multi-dimensional GM-secret relationship (must NOT leak to player/character POV!)
    execute_tool(
        &engine,
        "upsert_relation",
        json!({
            "source_id": "aria-vale",
            "source_kind": "character",
            "relation_type": "SWORN_SIBLINGS",
            "target_id": "gao-dahu",
            "target_kind": "character",
            "dimensions": { "trust": 75, "respect": 80, "resentment": 30, "fear": 5 },
            "direction": "directed",
            "visibility": "gm_secret",
            "notes": "Dependable second; quietly resents recent hesitation."
        }),
    )
    .unwrap();

    // 4. Test POV Isolation for Merchant Lin (Acceptance Test C):
    //    Lin MUST see ONLY his partial observation ("Two masked figures..."),
    //    and MUST NOT see Aria's identity, Aria's secret manual plan, or hidden relationship dimensions!
    let lin_ctx = execute_tool(
        &engine,
        "get_scene_context",
        json!({
            "place_id": "glass-hollow",
            "character_ids": ["merchant-lin", "aria-vale"],
            "perspective": "merchant-lin",
            "format": "markdown"
        }),
    )
    .unwrap();
    let lin_md = lin_ctx.as_str().unwrap();
    assert!(lin_md.contains("Two masked figures in soot-stained cloaks"));
    assert!(!lin_md.contains("aria-secret-manual-plan"));
    assert!(!lin_md.contains("hidden sect contact"));
    assert!(!lin_md.contains("resentment:30"));
    assert!(!lin_md.contains("GM EYES ONLY"));

    // 5. Test GM Perspective: MUST surface BOTH NPC blind spots AND fenced [GM EYES ONLY] secrets!
    let gm_ctx = execute_tool(
        &engine,
        "get_scene_context",
        json!({
            "place_id": "glass-hollow",
            "character_ids": ["merchant-lin", "aria-vale"],
            "perspective": "gm",
            "format": "markdown"
        }),
    )
    .unwrap();
    let gm_md = gm_ctx.as_str().unwrap();
    assert!(gm_md.contains("GM EYES ONLY"));
    assert!(gm_md.contains("Epistemic Blind Spots"));
    assert!(gm_md.contains("`merchant-lin` does NOT know `aria-secret-manual-plan`"));

    // 6. Test Epistemic Propagation Honesty Guard:
    //    Merchant Lin does NOT know `aria-secret-manual-plan`, so truthfully propagating it MUST fail!
    let illegal_prop = execute_tool(
        &engine,
        "propagate_knowledge",
        json!({
            "claim_id": "aria-secret-manual-plan",
            "from_actor_id": "merchant-lin",
            "to_actor_ids": ["archivist-bren"]
        }),
    );
    assert!(illegal_prop.is_err(), "Unaware NPC cannot truthfully propagate unknown claim");

    //    Merchant Lin CAN propagate what he actually observed (with confidence decay + rumor drift!)
    let ok_prop = execute_tool(
        &engine,
        "propagate_knowledge",
        json!({
            "claim_id": "road-ambush-01",
            "from_actor_id": "merchant-lin",
            "to_actor_ids": ["archivist-bren"],
            "confidence_decay": 15,
            "notes": "Lin reported the ambush at Watch Keep."
        }),
    )
    .unwrap();
    assert_eq!(ok_prop["propagated_beliefs"][0]["confidence"], 70);
    assert!(ok_prop["propagated_beliefs"][0]["perceived_claim"]
        .as_str()
        .unwrap()
        .contains("Two masked figures"));

    // 7. Blind-spot search (`not_known_by`) & outdated claim superseding
    let blind = execute_tool(
        &engine,
        "search_knowledge",
        json!({ "not_known_by": "merchant-lin" }),
    )
    .unwrap();
    assert!(blind["records"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["claim"]["id"] == "aria-secret-manual-plan"));

    execute_tool(
        &engine,
        "record_knowledge",
        json!({
            "claim_id": "road-ambush-01-updated",
            "subject": "glass-hollow",
            "claim": "The ambushed salt cart was actually a decoy.",
            "supersedes_claim_id": "road-ambush-01",
            "known_by": ["aria-vale"]
        }),
    )
    .unwrap();
    let lin_known = execute_tool(
        &engine,
        "search_knowledge",
        json!({ "actor_id": "merchant-lin" }),
    )
    .unwrap();
    assert_eq!(lin_known["records"][0]["beliefs"][0]["outdated"], true);

    // =========================================================================
    // ACCEPTANCE TEST D & E: Structured NPC Motivations, Dedicated Factions & Autonomy Sandbox
    // =========================================================================
    execute_tool(
        &engine,
        "update_character_profile",
        json!({
            "character_id": "gao-dahu",
            "occupation": "Camp Lieutenant",
            "risk_tolerance": "bold",
            "morale": 55,
            "long_term_motivations": ["Keep the camp fed through Winter", "Avoid attracting sect enforcers"],
            "fears": ["Starvation", "Cultivator retaliation"],
            "secrets": ["Secretly stashed 30 silver taels beneath the charcoal kiln"],
            "upsert_goal": {
                "goal_id": "secure-winter-grain",
                "description": "Stockpile 60 units of grain before Winter arrives",
                "priority": 9,
                "progress": 20,
                "target": 60,
                "status": "active"
            }
        }),
    )
    .unwrap();

    // Dedicated Faction Register (`manage_faction`)
    let fac_res = execute_tool(
        &engine,
        "manage_faction",
        json!({
            "action": "create",
            "id": "blackpine-boars",
            "name": "Blackpine Boars",
            "faction_type": "bandit-band",
            "capacity_tier": "micro-band (2 active in fixture)",
            "leader_id": "aria-vale",
            "members": [
                { "character_id": "aria-vale", "rank": "Chief", "role": "Leader" },
                { "character_id": "gao-dahu", "rank": "Second", "role": "Camp Quartermaster" }
            ],
            "territory": ["watch-keep"],
            "resources": { "grain": 25, "silver_taels": 40 },
            "morale": 65,
            "objectives": [
                {
                    "goal_id": "hold-ridge-road",
                    "description": "Keep rival bands off the charcoal road",
                    "priority": 8,
                    "progress": 1,
                    "target": 5,
                    "status": "active"
                }
            ],
            "internal_conflicts": ["Gao challenges Aria's caution on toll raids"]
        }),
    )
    .unwrap();
    assert!(tmp.path().join("docs/factions/blackpine-boars.md").exists());
    assert_eq!(fac_res["faction"]["resources"]["grain"], 25);

    // Phase A (`mode: "prepare"`): Leak-proof autonomy dossier for Gao Dahu & Blackpine Boars over 3 days
    let dossier = execute_tool(
        &engine,
        "simulate_autonomy",
        json!({
            "mode": "prepare",
            "scope_type": "faction",
            "faction_id": "blackpine-boars",
            "actor_ids": ["gao-dahu"],
            "duration_days": 3
        }),
    )
    .unwrap();
    // Gao Dahu MUST NOT receive Aria's secret manual plan inside his autonomy dossier!
    let gao_brief = &dossier["actors"][0];
    assert_eq!(gao_brief["character_id"], "gao-dahu");
    assert!(gao_brief["blind_spot_count"].as_u64().unwrap() >= 1);
    assert!(!gao_brief["known_claims"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["claim_id"] == "aria-secret-manual-plan"));
    // Capacity constraints & projected upkeep must be computed deterministically
    assert!(dossier["projected_upkeep"]["daily_rations"].as_i64().unwrap() >= 3);
    assert!(dossier["capacity_constraints"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s.as_str().unwrap().contains("micro-band")));

    // Phase B Validator Guard 1: Gao trying to act on Aria's unknown secret MUST be blocked!
    let epistemic_fail = execute_tool(
        &engine,
        "simulate_autonomy",
        json!({
            "mode": "commit",
            "scope_type": "npc",
            "actor_ids": ["gao-dahu"],
            "duration_days": 3,
            "actions": [
                {
                    "actor_id": "gao-dahu",
                    "action": "Confronts Aria's secret sect contact",
                    "relied_on_claim_ids": ["aria-secret-manual-plan"]
                }
            ]
        }),
    );
    assert!(epistemic_fail.is_err(), "Autonomy commit must reject actions relying on unlearned claims");

    // Phase B Validator Guard 2: Spending more silver than faction owns MUST be blocked!
    let resource_fail = execute_tool(
        &engine,
        "simulate_autonomy",
        json!({
            "mode": "commit",
            "scope_type": "faction",
            "faction_id": "blackpine-boars",
            "duration_days": 3,
            "faction_resource_deltas": [{ "resource": "silver_taels", "delta": -999 }]
        }),
    );
    assert!(resource_fail.is_err(), "Autonomy commit must block spending non-existent faction resources");

    // Phase B Valid Commit: Gao independently fortifies camp, consumes 6 grain, advances his goal!
    let committed = execute_tool(
        &engine,
        "simulate_autonomy",
        json!({
            "mode": "commit",
            "scope_type": "faction",
            "faction_id": "blackpine-boars",
            "actor_ids": ["gao-dahu"],
            "duration_days": 3,
            "actions": [
                {
                    "actor_id": "gao-dahu",
                    "action": "Organizes charcoal barter and posts double lookouts along the ridge road",
                    "outcome_summary": "Traded charcoal surplus for extra grain reserves."
                }
            ],
            "faction_resource_deltas": [
                { "resource": "grain", "delta": 10 },
                { "resource": "silver_taels", "delta": -5 }
            ],
            "goal_updates": [
                { "owner_id": "gao-dahu", "goal_id": "secure-winter-grain", "progress_delta": 10 },
                { "owner_id": "blackpine-boars", "goal_id": "hold-ridge-road", "progress_delta": 1 }
            ],
            "morale_delta": 5
        }),
    )
    .unwrap();
    assert_eq!(committed["committed"], true);
    assert_eq!(committed["faction"]["resources"]["grain"], 35);
    assert_eq!(committed["faction"]["resources"]["silver_taels"], 35);
    assert_eq!(committed["faction"]["morale"], 70);

    // Verify SHA-256 chain integrity across all Phase 1 + Phase 2 events!
    let (chain_ok, _) = engine.db().verify_event_chain().unwrap();
    assert!(chain_ok, "SHA-256 hash chain must remain valid after Phase 2");
}

#[test]
fn test_phase3_and_phase4_travel_ripple_economy_law_injuries_crafting_and_advance_world() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    // Create a third settlement for our bridge + detour geography network
    execute_tool(
        &engine,
        "create_place",
        json!({
            "id": "river-market",
            "name": "River Market Town",
            "place_type": "settlement",
            "terrain": "River Valley",
            "climate": "Temperate",
            "initial_weather": "Clear",
            "description": "Bustling riverside trade hub reliant on Watch Keep grain shipments."
        }),
    )
    .unwrap();

    // =========================================================================
    // ACCEPTANCE TEST F: Destroyed Bridge -> Rerouted Travel + Automatic Price Spike!
    // =========================================================================
    execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "upsert_route",
            "id": "old-stone-bridge",
            "from_place_id": "watch-keep",
            "to_place_id": "river-market",
            "bidirectional": true,
            "distance_km": 10.0,
            "terrain": "paved road",
            "toll_cost": 5,
            "toll_currency": "copper_cash",
            "status": "open"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "upsert_route",
            "id": "north-ridge-trail",
            "from_place_id": "watch-keep",
            "to_place_id": "glass-hollow",
            "bidirectional": true,
            "distance_km": 20.0,
            "terrain": "mountain ridge",
            "elevation_gain_m": 600,
            "status": "open"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "upsert_route",
            "id": "hollow-valley-track",
            "from_place_id": "glass-hollow",
            "to_place_id": "river-market",
            "bidirectional": true,
            "distance_km": 25.0,
            "terrain": "forest trail",
            "status": "open"
        }),
    )
    .unwrap();

    // Configure River Market importing Grain from Watch Keep
    execute_tool(
        &engine,
        "manage_economy",
        json!({
            "action": "upsert_market",
            "place_id": "river-market",
            "name": "River Market Exchange",
            "supply_origins": ["watch-keep"],
            "commodities": [
                {
                    "name": "Grain",
                    "category": "food",
                    "base_price": 20,
                    "currency": "copper_cash",
                    "supply": 100,
                    "demand": 100,
                    "daily_production": 5,
                    "daily_consumption": 15,
                    "imported_from": ["watch-keep"]
                }
            ]
        }),
    )
    .unwrap();
    assert!(tmp.path().join("docs/economy/markets.md").exists());

    // Measure intact travel & intact market price BEFORE bridge destruction
    let plan_before = execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "calculate",
            "from_place_id": "watch-keep",
            "to_place_id": "river-market",
            "character_ids": ["aria-vale"],
            "transport_mode": "cart"
        }),
    )
    .unwrap();
    assert_eq!(plan_before["viable"], true);
    assert_eq!(plan_before["total_distance_km"], 10.0);
    assert!(plan_before["rerouted_around"].as_array().unwrap().is_empty());

    let prices_before = execute_tool(
        &engine,
        "manage_economy",
        json!({ "action": "get_market_prices", "place_id": "river-market" }),
    )
    .unwrap();
    let grain_price_before = prices_before["commodities"][0]["live_price"].as_i64().unwrap();
    assert_eq!(grain_price_before, 20);

    // DESTROY OLD STONE BRIDGE!
    execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "upsert_route",
            "id": "old-stone-bridge",
            "from_place_id": "watch-keep",
            "to_place_id": "river-market",
            "bidirectional": true,
            "distance_km": 10.0,
            "terrain": "paved road",
            "status": "destroyed",
            "notes": "Bridge spans collapsed during flood."
        }),
    )
    .unwrap();

    // Travel MUST automatically detour via Glass Hollow (45 km) & cite destroyed bridge!
    let plan_after = execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "calculate",
            "from_place_id": "watch-keep",
            "to_place_id": "river-market",
            "character_ids": ["aria-vale"],
            "transport_mode": "cart"
        }),
    )
    .unwrap();
    assert_eq!(plan_after["viable"], true);
    assert_eq!(plan_after["total_distance_km"], 45.0);
    assert!(plan_after["rerouted_around"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s.as_str().unwrap().contains("old-stone-bridge")));

    // Market price for imported Grain MUST automatically spike due to the destroyed bridge!
    let prices_after = execute_tool(
        &engine,
        "manage_economy",
        json!({ "action": "get_market_prices", "place_id": "river-market" }),
    )
    .unwrap();
    let grain_price_after = prices_after["commodities"][0]["live_price"].as_i64().unwrap();
    assert!(
        grain_price_after > grain_price_before,
        "Destroyed bridge must raise grain price (before={grain_price_before}, after={grain_price_after})"
    );
    assert!(!prices_after["disrupted_supply_lines"].as_array().unwrap().is_empty());

    // Advance travel along the detour & explore destination
    let adv_travel = execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "advance",
            "from_place_id": "watch-keep",
            "to_place_id": "river-market",
            "character_ids": ["aria-vale"],
            "transport_mode": "foot"
        }),
    )
    .unwrap();
    assert!(adv_travel["event_seq"].as_i64().is_some());
    assert_eq!(
        engine.get_character("aria-vale", None).unwrap().current.current_location.as_deref(),
        Some("river-market")
    );

    let explored = execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "explore",
            "place_id": "glass-hollow",
            "actor_id": "aria-vale",
            "mode": "scout",
            "difficulty": 10,
            "seed": 42
        }),
    )
    .unwrap();
    assert!(explored["event_seq"].as_i64().is_some());

    // =========================================================================
    // ATOMIC CURRENCY, TAXED TRANSACTIONS & ENCUMBRANCE
    // =========================================================================
    execute_tool(
        &engine,
        "manage_economy",
        json!({
            "action": "set_balance",
            "owner_id": "aria-vale",
            "currency": "silver_taels",
            "amount": 50,
            "reason": "Starting purse"
        }),
    )
    .unwrap();

    // Insufficient funds transaction MUST fail atomically (balance unchanged!)
    let bad_tx = execute_tool(
        &engine,
        "manage_economy",
        json!({
            "action": "execute_transaction",
            "buyer_id": "aria-vale",
            "seller_id": "river-merchant",
            "amount": 100,
            "currency": "silver_taels",
            "item_name": "Fine Riding Horse"
        }),
    );
    assert!(bad_tx.is_err());
    let bal_after_fail = execute_tool(
        &engine,
        "manage_economy",
        json!({ "action": "get_balances", "owner_id": "aria-vale" }),
    )
    .unwrap();
    assert_eq!(bal_after_fail["balances"]["silver_taels"], 50);

    // Valid taxed purchase succeeds cleanly and delivers item
    let ok_tx = execute_tool(
        &engine,
        "manage_economy",
        json!({
            "action": "execute_transaction",
            "buyer_id": "aria-vale",
            "seller_id": "river-merchant",
            "amount": 20,
            "tax_rate_pct": 10,
            "tax_recipient_id": "river-market-treasury",
            "currency": "silver_taels",
            "item_name": "Heavy Iron Kettle",
            "quantity": 2
        }),
    )
    .unwrap();
    assert_eq!(ok_tx["buyer_balance"], 28); // 50 - 20 - 2 tax
    assert_eq!(ok_tx["seller_balance"], 20);
    assert_eq!(ok_tx["tax_paid"], 2);

    // Encumbrance calculation
    execute_tool(
        &engine,
        "manage_economy",
        json!({
            "action": "set_item_weight",
            "item_key": "Heavy Iron Kettle",
            "weight_kg": 15.0,
            "unit_value": 10
        }),
    )
    .unwrap();
    let enc = execute_tool(
        &engine,
        "manage_economy",
        json!({ "action": "check_encumbrance", "character_id": "aria-vale" }),
    )
    .unwrap();
    assert!(enc["total_weight_kg"].as_f64().unwrap() >= 30.0);

    // =========================================================================
    // LAW, JURISDICTION, CRIME & REPUTATION
    // =========================================================================
    execute_tool(
        &engine,
        "manage_law_and_reputation",
        json!({
            "action": "upsert_jurisdiction",
            "id": "river-magistrate",
            "name": "River Valley Magistracy",
            "place_ids": ["river-market"],
            "laws": ["No armed brigandage on Imperial roads", "10% salt & grain tariff"],
            "tax_rate_pct": 10,
            "enforcement_strength": "County Constables"
        }),
    )
    .unwrap();
    let crime = execute_tool(
        &engine,
        "manage_law_and_reputation",
        json!({
            "action": "record_crime",
            "jurisdiction_id": "river-magistrate",
            "suspect_id": "aria-vale",
            "offense": "Evading road tariff",
            "warrant_status": "warrant_issued",
            "bounty": 30,
            "bounty_currency": "silver_taels"
        }),
    )
    .unwrap();
    assert_eq!(crime["crime"]["warrant_status"], "warrant_issued");

    let rep = execute_tool(
        &engine,
        "manage_law_and_reputation",
        json!({
            "action": "modify_reputation",
            "entity_id": "aria-vale",
            "target_group_id": "river-magistrate",
            "delta": -35,
            "notes": "Tariff evasion warrant issued"
        }),
    )
    .unwrap();
    assert_eq!(rep["reputation"]["tier"], "Hostile");

    // =========================================================================
    // PHASE 4: INJURIES/RECOVERY, CRAFTING, PROGRESSION, WEATHER & ADVANCE_WORLD
    // =========================================================================
    let inj = execute_tool(
        &engine,
        "manage_health_and_downtime",
        json!({
            "action": "apply_injury",
            "character_id": "aria-vale",
            "name": "Cracked Collarbone",
            "severity": "moderate",
            "body_location": "Left Shoulder",
            "recovery_days": 4,
            "notes": "From rocky fall"
        }),
    )
    .unwrap();
    let inj_id = inj["injury"]["id"].as_str().unwrap().to_string();
    // Injury synced immediately to live StatusConditions
    assert!(engine
        .get_character("aria-vale", None)
        .unwrap()
        .current
        .conditions
        .iter()
        .any(|c| c.name == "Cracked Collarbone"));

    // Treat injury (-1 day -> 3 days left)
    let treated = execute_tool(
        &engine,
        "manage_health_and_downtime",
        json!({
            "action": "treat_injury",
            "injury_id": inj_id,
            "days_reduced": 1,
            "notes": "Splinted with medicinal poultice"
        }),
    )
    .unwrap();
    assert_eq!(treated["injury"]["recovery_days_remaining"], 3);

    // Create a faction with grain to test bounded advance_world upkeep + dossier generation
    execute_tool(
        &engine,
        "manage_faction",
        json!({
            "action": "create",
            "id": "river-watch",
            "name": "River Watch Militia",
            "faction_type": "militia",
            "members": [{ "character_id": "archivist-bren", "rank": "Captain" }],
            "territory": ["river-market"],
            "resources": { "grain": 30 }
        }),
    )
    .unwrap();

    // Bounded-scope `advance_world` for 3 days -> heals Cracked Collarbone, rolls weather, drifts market, drains upkeep!
    let world_tick = execute_tool(
        &engine,
        "advance_world",
        json!({
            "duration_days": 3,
            "place_ids": ["river-market"],
            "faction_ids": ["river-watch"],
            "character_ids": ["aria-vale"],
            "tick_weather": true,
            "tick_recovery": true,
            "tick_markets": true,
            "tick_upkeep": true,
            "prepare_autonomy_dossiers": true,
            "seed": 777
        }),
    )
    .unwrap();
    assert!(world_tick["event_seq"].as_i64().is_some());
    assert_eq!(world_tick["autonomy_dossiers"].as_array().unwrap().len(), 1);
    assert_eq!(
        engine.get_faction("river-watch").unwrap().resources["grain"],
        27 // 30 - (1 member * 3 days)
    );
    // Cracked Collarbone healed after 3 days AND automatically removed from live StatusConditions!
    assert!(!engine
        .get_character("aria-vale", None)
        .unwrap()
        .current
        .conditions
        .iter()
        .any(|c| c.name == "Cracked Collarbone"));

    // Crafting + Progression check
    let craft_start = execute_tool(
        &engine,
        "manage_health_and_downtime",
        json!({
            "action": "begin_crafting",
            "crafter_id": "aria-vale",
            "item_name": "Reinforced Climbing Pitons",
            "item_type": "tool",
            "difficulty": 8,
            "required_hours": 4
        }),
    )
    .unwrap();
    let craft_id = craft_start["project"]["id"].as_str().unwrap().to_string();
    let craft_done = execute_tool(
        &engine,
        "manage_health_and_downtime",
        json!({
            "action": "resolve_crafting",
            "project_id": craft_id,
            "hours_worked": 4,
            "seed": 12345
        }),
    )
    .unwrap();
    assert!(matches!(
        craft_done["project"]["status"].as_str().unwrap(),
        "completed" | "failed"
    ));

    let prog = execute_tool(
        &engine,
        "manage_health_and_downtime",
        json!({
            "action": "resolve_progression",
            "character_id": "aria-vale",
            "progression_kind": "body_tempering",
            "difficulty": 12,
            "seed": 888
        }),
    )
    .unwrap();
    assert!(prog["event_seq"].as_i64().is_some());

    // Final SHA-256 ledger integrity check across ALL 4 phases!
    let (chain_ok, total_ev) = engine.db().verify_event_chain().unwrap();
    assert!(chain_ok, "SHA-256 hash chain must remain 100% valid across Phases 1-4");
    assert!(total_ev > 20);
}

#[test]
fn test_audit_fixes_hash_chain_schema_compat_routes_skill_validation_and_secret_wall() {
    use chronicle::maintenance::diagnose_campaign;
    use chronicle::oauth::OAuthStore;
    use rusqlite::Connection;
    use sha2::{Digest, Sha256};

    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    // =========================================================================
    // 1. P0 #3.1: database_identity in campaign_status + backup reporting
    // =========================================================================
    let bkp = execute_tool(&engine, "backup_campaign", json!({ "label": "audit-check" })).unwrap();
    assert!(bkp["backup_path"].as_str().is_some());

    let status = execute_tool(&engine, "campaign_status", json!({})).unwrap();
    assert_eq!(status["ledger_hash_chain_valid"], true);
    let ident = &status["database_identity"];
    assert_eq!(ident["db_fingerprint"].as_str().unwrap().len(), 16);
    assert!(ident["total_events"].as_i64().unwrap() > 0);
    assert!(ident["latest_event_seq"].as_i64().unwrap() > 0);
    assert!(!ident["latest_event_hash"].as_str().unwrap().is_empty());
    assert!(ident["backup_count"].as_u64().unwrap() >= 1);

    // =========================================================================
    // 2. P1 #3.3: Route distance synthesis (English words + no silent 18 km)
    //    AND bidirectional search_places(connected_to)
    // =========================================================================
    execute_tool(
        &engine,
        "create_place",
        json!({
            "id": "jade-river-heartland",
            "name": "Jade River Heartland",
            "place_type": "region",
            "routes": [
                {
                    "destination_id": "blackpine-ridge",
                    "travel_time": "Two days on foot",
                    "route_type": "Mountain road",
                    "danger": "Moderate"
                },
                {
                    "destination_id": "mist-peak",
                    "travel_time": "Uncharted pass",
                    "route_type": "Hidden trail",
                    "danger": "High"
                }
            ]
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "create_place",
        json!({
            "id": "blackpine-ridge",
            "name": "Blackpine Ridge",
            "place_type": "ridge"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "create_place",
        json!({
            "id": "mist-peak",
            "name": "Mist Peak",
            "place_type": "mountain"
        }),
    )
    .unwrap();

    let routes_res = execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({ "action": "list_routes" }),
    )
    .unwrap();
    let routes = routes_res["routes"].as_array().unwrap();
    let r_two_days = routes
        .iter()
        .find(|r| r["to_place_id"] == "blackpine-ridge")
        .expect("synthesized route to blackpine-ridge");
    assert_eq!(r_two_days["distance_estimated"], true);
    assert_eq!(r_two_days["canonical_travel_time"], "Two days on foot");
    let km_two_days = r_two_days["distance_km"].as_f64().unwrap();
    assert!(
        (km_two_days - 72.0).abs() < 1.0,
        "Two days on foot should synthesize ~72 km (not 18 km!), got {km_two_days}"
    );

    let r_uncharted = routes
        .iter()
        .find(|r| r["to_place_id"] == "mist-peak")
        .expect("synthesized route to mist-peak");
    assert_eq!(r_uncharted["distance_estimated"], true);
    assert_eq!(
        r_uncharted["distance_km"].as_f64().unwrap(),
        0.0,
        "Unparseable travel_time must NOT silently fabricate 18 km"
    );

    let plan = execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "calculate",
            "from_place_id": "jade-river-heartland",
            "to_place_id": "blackpine-ridge",
            "mode": "foot"
        }),
    )
    .unwrap();
    assert!(
        !plan["warnings"].as_array().unwrap().is_empty(),
        "Estimated synthesized routes must emit warnings on TravelPlan"
    );

    // Bidirectional connected_to check
    let connected_to_ridge = execute_tool(
        &engine,
        "search_places",
        json!({ "connected_to": "blackpine-ridge" }),
    )
    .unwrap();
    assert!(connected_to_ridge["places"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["id"] == "jade-river-heartland"));

    let connected_to_jade = execute_tool(
        &engine,
        "search_places",
        json!({ "connected_to": "jade-river-heartland" }),
    )
    .unwrap();
    assert!(
        connected_to_jade["places"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == "blackpine-ridge"),
        "search_places(connected_to) must work bidirectionally!"
    );

    // =========================================================================
    // 3. P1 #3.4: Skill/Stat/Technique/Equipment validation + unmapped rank
    // =========================================================================
    execute_tool(
        &engine,
        "upsert_character_skill",
        json!({
            "character_id": "aria-vale",
            "name": "Cloud-Tailoring",
            "rank": "Mortal practical competence",
            "notes": "Custom non-numeric cultivation tier rank"
        }),
    )
    .unwrap();

    // Known skill with unmapped custom rank -> warning emitted, still resolves cleanly
    let custom_rank_check = execute_tool(
        &engine,
        "resolve_check",
        json!({
            "actor_id": "aria-vale",
            "action": "Weave stabilizing cloud-silk seal",
            "skill": "Cloud-Tailoring",
            "difficulty": 12,
            "dry_run": true
        }),
    )
    .unwrap();
    let cr_warns = custom_rank_check["warnings"].as_array().unwrap();
    assert!(
        cr_warns
            .iter()
            .any(|w| w.as_str().unwrap().contains("Mortal practical competence")),
        "Unmapped custom skill rank must emit explicit warning, got {cr_warns:?}"
    );

    // Unknown skill (default non-strict) -> warning emitted in BOTH dry_run and live
    let unknown_dry = execute_tool(
        &engine,
        "resolve_check",
        json!({
            "actor_id": "aria-vale",
            "action": "Invoke nonexistent skill",
            "skill": "Nonexistent Skill",
            "difficulty": 12,
            "dry_run": true
        }),
    )
    .unwrap();
    assert!(unknown_dry["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w.as_str().unwrap().contains("Unknown skill 'Nonexistent Skill'")));

    let unknown_live = execute_tool(
        &engine,
        "resolve_check",
        json!({
            "actor_id": "aria-vale",
            "action": "Invoke nonexistent skill live",
            "skill": "Nonexistent Skill",
            "difficulty": 12,
            "seed": 42,
            "log_to_ledger": false
        }),
    )
    .unwrap();
    assert!(unknown_live["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w.as_str().unwrap().contains("Unknown skill 'Nonexistent Skill'")));

    // Strict validation rejects unknown skill BEFORE rolling (both dry_run and live!)
    let strict_err = execute_tool(
        &engine,
        "resolve_check",
        json!({
            "actor_id": "aria-vale",
            "action": "Strict check with bad skill",
            "skill": "Nonexistent Skill",
            "difficulty": 12,
            "dry_run": true,
            "strict_validation": true
        }),
    );
    assert!(strict_err.is_err(), "strict_validation: true must reject unknown skill");

    // Contest validation
    let contest_warn = execute_tool(
        &engine,
        "resolve_contest",
        json!({
            "action": "Test unknown skill in contest",
            "participants": [
                { "actor_id": "aria-vale", "skill": "Ghostly Fake Skill" },
                { "actor_id": "archivist-bren" }
            ],
            "seed": 99,
            "log_to_ledger": false
        }),
    )
    .unwrap();
    assert!(!contest_warn["warnings"].as_array().unwrap().is_empty());

    let contest_strict_err = execute_tool(
        &engine,
        "resolve_contest",
        json!({
            "action": "Strict contest unknown skill",
            "participants": [
                { "actor_id": "aria-vale", "skill": "Ghostly Fake Skill" },
                { "actor_id": "archivist-bren" }
            ],
            "strict_validation": true,
            "log_to_ledger": false
        }),
    );
    assert!(contest_strict_err.is_err());

    // =========================================================================
    // 4. P1 #3.5: Controlled GM-secret isolation test across lore, claims,
    //    histories, and hidden relation dimensions
    // =========================================================================
    execute_tool(
        &engine,
        "upsert_lore",
        json!({
            "id": "cloud-tailoring-body-mechanism",
            "title": "Cloud-Tailoring Body Mechanism",
            "category": "secret",
            "visibility": "gm_secret",
            "summary": "SECRET_BODY_MECHANISM_MARKER",
            "body": "GM ONLY: SECRET_BODY_MECHANISM_MARKER hidden meridian loop.",
            "keywords": ["cloud-tailoring", "lantern", "glass"]
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "record_knowledge",
        json!({
            "id": "secret-meridian-claim",
            "subject": "Cloud-Tailoring Secret",
            "claim": "SECRET_CLAIM_TRUTH_MARKER",
            "visibility": "gm_secret",
            "related_ids": ["aria-vale", "glass-hollow"],
            "holders": [
                { "holder_id": "archivist-bren", "certainty": "certain", "believed_content": "SECRET_CLAIM_TRUTH_MARKER" }
            ]
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "add_entity_history",
        json!({
            "entity_id": "glass-hollow",
            "entity_kind": "place",
            "title": "Hidden Vault Collapse",
            "details": "SECRET_HISTORY_MARKER",
            "visibility": "gm_secret"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "upsert_relation",
        json!({
            "source_id": "archivist-bren",
            "target_id": "aria-vale",
            "relation_type": "secretly_manipulating",
            "strength": -40,
            "visibility": "gm_secret",
            "notes": "SECRET_RELATION_MARKER",
            "dimensions": { "trust": -30, "fear": 20 }
        }),
    )
    .unwrap();

    let gm_ctx = execute_tool(
        &engine,
        "get_scene_context",
        json!({
            "location_id": "glass-hollow",
            "character_ids": ["aria-vale", "archivist-bren"],
            "scene_text": "Investigating cloud-tailoring and glass lantern echoes",
            "perspective": "gm",
            "include_gm_secrets": true,
            "format": "markdown"
        }),
    )
    .unwrap();
    let gm_md = gm_ctx
        .as_str()
        .or_else(|| gm_ctx["context_markdown"].as_str())
        .or_else(|| gm_ctx["markdown"].as_str())
        .unwrap();
    assert!(gm_md.contains("SECRET_BODY_MECHANISM_MARKER"));

    let pc_ctx = execute_tool(
        &engine,
        "get_scene_context",
        json!({
            "location_id": "glass-hollow",
            "character_ids": ["aria-vale", "archivist-bren"],
            "scene_text": "Investigating cloud-tailoring and glass lantern echoes",
            "perspective": "aria-vale",
            "include_gm_secrets": true,
            "format": "markdown"
        }),
    )
    .unwrap();
    let pc_md = pc_ctx
        .as_str()
        .or_else(|| pc_ctx["context_markdown"].as_str())
        .or_else(|| pc_ctx["markdown"].as_str())
        .unwrap();
    assert!(!pc_md.contains("SECRET_BODY_MECHANISM_MARKER"), "GM-secret lore leaked to PC POV!");
    assert!(!pc_md.contains("SECRET_CLAIM_TRUTH_MARKER"), "GM-secret knowledge claim leaked to PC POV!");
    assert!(!pc_md.contains("SECRET_HISTORY_MARKER"), "GM-secret history leaked to PC POV!");
    assert!(!pc_md.contains("SECRET_RELATION_MARKER"), "GM-secret relation leaked to PC POV!");

    // =========================================================================
    // 5. HTTP Security Hardening: OAuth brute-force lockout + HTTP file guard
    // =========================================================================
    let mut store = OAuthStore::new("super-secret-gm-key".to_string());
    let bad_form = b"client_id=test&redirect_uri=https://example.com/cb&passphrase=wrong&decision=approve";
    for attempt in 1..=4 {
        let r = store.handle_authorize_post(bad_form);
        assert_eq!(r.status, 403, "Attempt {attempt} should return 403");
    }
    let r5 = store.handle_authorize_post(bad_form);
    assert_eq!(r5.status, 429, "5th failed passphrase attempt must trigger 429 lockout");
    assert!(r5.extra_headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("retry-after")));
    // Even the right passphrase is rejected during active lockout
    let r_locked = store.handle_authorize_post(
        b"client_id=test&redirect_uri=https://example.com/cb&passphrase=super-secret-gm-key&decision=approve",
    );
    assert_eq!(r_locked.status, 429);

    // HTTP MCP blocks import_attachment(source_path) by default, while allowing inline text_content
    let secret_file = tmp.path().join("host-secret.txt");
    std::fs::write(&secret_file, "TOP SECRET HOST FILE").unwrap();
    let mcp_blocked = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "import_attachment",
            "arguments": {
                "title": "Exfil Test",
                "source_path": secret_file.to_string_lossy()
            }
        }
    });
    let (st_blk, _, body_blk) = dispatch_http(
        &engine,
        "POST",
        "/mcp",
        serde_json::to_vec(&mcp_blocked).unwrap().as_slice(),
    );
    assert_eq!(st_blk, 200);
    let blk_val: serde_json::Value = serde_json::from_slice(&body_blk).unwrap();
    assert_eq!(blk_val["result"]["isError"], true);
    assert!(blk_val["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("disabled over HTTP"));

    let mcp_inline_ok = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "import_attachment",
            "arguments": {
                "title": "Inline Safe Doc",
                "text_content": "Safe inline campaign handout"
            }
        }
    });
    let (st_ok, _, body_ok) = dispatch_http(
        &engine,
        "POST",
        "/mcp",
        serde_json::to_vec(&mcp_inline_ok).unwrap().as_slice(),
    );
    assert_eq!(st_ok, 200);
    let ok_val: serde_json::Value = serde_json::from_slice(&body_ok).unwrap();
    assert_eq!(ok_val["result"]["isError"], false);

    // =========================================================================
    // 6. P0 #3.2: Pre-Phase-2 raw JSON hash chain compatibility + tamper details
    // =========================================================================
    // Simulate a legacy pre-Phase-2 RelationUpserted row (missing "direction" & "visibility")
    // and recompute the exact chain hashes from that row onward.
    let db_path = engine.db().db_path().to_path_buf();
    {
        let conn = Connection::open(&db_path).unwrap();
        let target_seq: i64 = conn
            .query_row(
                "SELECT seq FROM events WHERE payload_json LIKE '%\"relation_upserted\"%' ORDER BY seq ASC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let raw_payload: String = conn
            .query_row(
                "SELECT payload_json FROM events WHERE seq = ?1",
                [target_seq],
                |r| r.get(0),
            )
            .unwrap();
        let mut pval: serde_json::Value = serde_json::from_str(&raw_payload).unwrap();
        if let Some(rel) = pval.get_mut("relation").and_then(|v| v.as_object_mut()) {
            rel.remove("direction");
            rel.remove("visibility");
            rel.remove("dimensions");
        }
        let legacy_payload = serde_json::to_string(&pval).unwrap();
        conn.execute(
            "UPDATE events SET payload_json = ?1 WHERE seq = ?2",
            rusqlite::params![legacy_payload, target_seq],
        )
        .unwrap();

        // Recompute hashes from seq 1..N using exact stored payload_json (how Chronicle originally hashed them)
        let mut stmt = conn
            .prepare("SELECT seq, session_number, world_time, real_timestamp, location_id, summary, payload_json FROM events ORDER BY seq ASC")
            .unwrap();
        let rows: Vec<(i64, u32, String, String, Option<String>, String, String)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        drop(stmt);

        let mut prev = "GENESIS".to_string();
        for (seq, sess, wt, ts, loc, sum, pjson) in rows {
            let mut hasher = Sha256::new();
            hasher.update(prev.as_bytes());
            hasher.update(b"|");
            hasher.update(sess.to_string().as_bytes());
            hasher.update(b"|");
            hasher.update(wt.as_bytes());
            hasher.update(b"|");
            hasher.update(ts.as_bytes());
            hasher.update(b"|");
            hasher.update(loc.as_deref().unwrap_or("").as_bytes());
            hasher.update(b"|");
            hasher.update(sum.as_bytes());
            hasher.update(b"|");
            hasher.update(pjson.as_bytes());
            let hash: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
            conn.execute(
                "UPDATE events SET prev_hash = ?1, hash = ?2 WHERE seq = ?3",
                rusqlite::params![prev, hash, seq],
            )
            .unwrap();
            prev = hash;
        }
    }

    // Legacy row with missing newly-added default fields MUST still pass chain verification!
    let chain_rep = engine.db().verify_event_chain_detailed().unwrap();
    assert!(
        chain_rep.valid,
        "Legacy pre-Phase-2 RelationUpserted JSON must NOT trigger false LEDGER_CHAIN_INVALID: {chain_rep:?}"
    );
    let diag = diagnose_campaign(&engine).unwrap();
    assert!(
        !diag.findings.iter().any(|i| i.code == "LEDGER_CHAIN_INVALID"),
        "diagnose_campaign must not flag legacy schema evolution as tampering"
    );

    // Real tampering MUST be caught with exact seq, failure_kind, expected_hash, stored_hash!
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE events SET summary = 'TAMPERED SUMMARY' WHERE seq = 2",
            [],
        )
        .unwrap();
    }
    let tampered_rep = engine.db().verify_event_chain_detailed().unwrap();
    assert!(!tampered_rep.valid);
    assert_eq!(tampered_rep.first_failed_seq, Some(2));
    assert_eq!(
        tampered_rep.failure_kind.as_deref(),
        Some("digest_mismatch")
    );
    assert!(tampered_rep.expected_hash.is_some());
    assert!(tampered_rep.stored_hash.is_some());

    // =========================================================================
    // 7. Cross-version restore_backup: restore a pre-Phase-1 backup (10-col
    //    `events`, 5-col `relations`, no Phase 1-4 tables) into a live Phase 1-4 DB!
    // =========================================================================
    let tmp_bak = TempDir::new().unwrap();
    let mut eng_bak = CampaignEngine::open(tmp_bak.path()).unwrap();
    seed_test_fixture(&mut eng_bak);
    let bkp_res = execute_tool(&eng_bak, "backup_campaign", json!({ "label": "legacy-10col" })).unwrap();
    let bkp_path = std::path::PathBuf::from(bkp_res["backup_path"].as_str().unwrap());
    let bkp_name = bkp_path.file_name().unwrap().to_string_lossy().to_string();
    let orig_events = eng_bak.db().verify_event_chain_detailed().unwrap().total_events;

    // Downgrade the backup snapshot's `events` table to 10 columns & `relations` to 5 columns
    // (exactly matching a pre-Phase-1 Chronicle backup!) and drop a Phase-2 table (`factions`).
    {
        let conn = Connection::open(&bkp_path).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             CREATE TABLE events_old (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                session_number INTEGER NOT NULL,
                world_time TEXT NOT NULL,
                real_timestamp TEXT NOT NULL,
                location_id TEXT,
                affected_entities_json TEXT NOT NULL,
                summary TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                prev_hash TEXT NOT NULL,
                hash TEXT NOT NULL
             );
             INSERT INTO events_old (seq, session_number, world_time, real_timestamp, location_id, affected_entities_json, summary, payload_json, prev_hash, hash)
               SELECT seq, session_number, world_time, real_timestamp, location_id, affected_entities_json, summary, payload_json, prev_hash, hash FROM events;
             DROP TABLE events;
             ALTER TABLE events_old RENAME TO events;

             CREATE TABLE relations_old (
                source_id TEXT NOT NULL,
                source_kind TEXT NOT NULL,
                relation_type TEXT NOT NULL,
                target_id TEXT NOT NULL,
                target_kind TEXT NOT NULL,
                strength INTEGER,
                notes TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL,
                PRIMARY KEY (source_id, relation_type, target_id)
             );
             INSERT INTO relations_old (source_id, source_kind, relation_type, target_id, target_kind, strength, notes, updated_at_world_time)
               SELECT source_id, source_kind, relation_type, target_id, target_kind, strength, notes, updated_at_world_time FROM relations;
             DROP TABLE relations;
             ALTER TABLE relations_old RENAME TO relations;

             DROP TABLE IF EXISTS factions;
             PRAGMA foreign_keys = ON;",
        )
        .unwrap();
    }

    // Mutate live DB after backup so we prove restore actually rewinds cleanly
    execute_tool(
        &eng_bak,
        "log_event",
        json!({ "title": "Post-backup event to be rewound", "details": "Should disappear after restore" }),
    )
    .unwrap();
    assert_eq!(
        eng_bak.db().verify_event_chain_detailed().unwrap().total_events,
        orig_events + 1
    );

    // Restoring the 10-column legacy backup into the 13-column live DB MUST succeed cleanly!
    let restored = execute_tool(
        &eng_bak,
        "restore_backup",
        json!({ "backup_filename": bkp_name }),
    )
    .unwrap();
    assert!(restored["restored_from"].as_str().is_some());
    let post_chain = eng_bak.db().verify_event_chain_detailed().unwrap();
    assert!(post_chain.valid, "Hash chain must remain valid after cross-version backup restore");
    assert_eq!(post_chain.total_events, orig_events);

    // =========================================================================
    // 8. ChatGPT MCP Friction & Redaction Mitigations:
    //    - HTTP responses sanitize host filesystem paths -> relative paths
    //    - get_character bounds event_log to 10 by default + reports total_event_count
    //    - commit_turn(compact: true) returns unified world_time, event_seqs & receipt
    //    - search_knowledge is annotated readOnlyHint: true
    // =========================================================================
    for i in 1..=12 {
        execute_tool(
            &eng_bak,
            "modify_character_resource",
            json!({
                "character_id": "aria-vale",
                "resource": "HP",
                "delta": if i % 2 == 0 { 1 } else { -1 },
                "reason": format!("Test log tick {i}")
            }),
        )
        .unwrap();
    }
    let bounded_char = execute_tool(&eng_bak, "get_character", json!({ "character_id": "aria-vale" })).unwrap();
    assert_eq!(bounded_char["character"]["event_log"].as_array().unwrap().len(), 10);
    assert!(bounded_char["total_event_count"].as_u64().unwrap() > 10);
    let full_char = execute_tool(
        &eng_bak,
        "get_character",
        json!({ "character_id": "aria-vale", "event_log_limit": 0 }),
    )
    .unwrap();
    assert!(full_char["character"]["event_log"].as_array().unwrap().len() > 10);

    // Over HTTP (/mcp), local host path prefixes MUST be stripped from campaign_status & get_character
    let status_rpc = json!({
        "jsonrpc": "2.0",
        "id": 99,
        "method": "tools/call",
        "params": { "name": "campaign_status", "arguments": {} }
    });
    let (st_http, _, body_http) = dispatch_http(
        &eng_bak,
        "POST",
        "/mcp",
        serde_json::to_vec(&status_rpc).unwrap().as_slice(),
    );
    assert_eq!(st_http, 200);
    let body_str = String::from_utf8_lossy(&body_http);
    let host_root = tmp_bak.path().display().to_string();
    assert!(
        !body_str.contains(&host_root),
        "Remote HTTP MCP response must not leak local host path '{host_root}': {body_str}"
    );
    assert!(body_str.contains("campaign.db") && body_str.contains("docs"));

    // Compact commit_turn returns unified world_time, event_seqs, and verified receipt
    let batch = execute_tool(
        &eng_bak,
        "commit_turn",
        json!({
            "compact": true,
            "actions": [
                { "tool": "advance_time", "arguments": { "minutes": 30, "reason": "Bridge crossing" } },
                { "tool": "resolve_check", "arguments": { "actor_id": "aria-vale", "action": "Balance on wet stone", "difficulty": 12, "seed": 777 } },
                { "tool": "log_event", "arguments": { "title": "Crossed Bridge", "details": "Safely across." } }
            ]
        }),
    )
    .unwrap();
    assert_eq!(batch["committed_actions"], 3);
    assert_eq!(batch["event_seqs"].as_array().unwrap().len(), 3);
    assert!(!batch["world_time"].as_str().unwrap().is_empty());
    let receipt = batch["receipt"].as_str().unwrap();
    assert!(receipt.contains("advance_time") && receipt.contains("resolve_check") && receipt.contains("log_event"));

    // Verify search_knowledge has readOnlyHint: true
    let tools = chronicle::mcp::all_tool_definitions();
    let sk_tool = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "search_knowledge")
        .unwrap();
    assert_eq!(sk_tool["annotations"]["readOnlyHint"], true);

    // Shorthand 1-call beat checkpointing via commit_turn (advance_minutes + trailing log, no actions required!)
    let ev_before_shorthand = eng_bak.db().verify_event_chain_detailed().unwrap().total_events;
    let beat = execute_tool(
        &eng_bak,
        "commit_turn",
        json!({
            "advance_minutes": 15,
            "time_reason": "Negotiation at tea house",
            "log": {
                "title": "Agreed to Escort Caravan",
                "details": "Aria accepted a 20-silver escort deal with Merchant Wu.",
                "affected_entities": ["aria-vale"],
                "tags": ["deal", "caravan"]
            }
        }),
    )
    .unwrap();
    assert_eq!(beat["committed_actions"], 2);
    assert_eq!(beat["event_seqs"].as_array().unwrap().len(), 2);
    assert_eq!(
        eng_bak.db().verify_event_chain_detailed().unwrap().total_events,
        ev_before_shorthand + 2
    );

    // get_scene_context(format: "markdown") surfaces Last Committed Ledger Event right at the top
    let ctx_md = execute_tool(
        &eng_bak,
        "get_scene_context",
        json!({ "character_ids": ["aria-vale"], "format": "markdown" }),
    )
    .unwrap();
    let ctx_str = ctx_md
        .as_str()
        .or_else(|| ctx_md["context_markdown"].as_str())
        .unwrap();
    assert!(ctx_str.contains("**Last Committed Ledger Event:**"));
    assert!(ctx_str.contains("Agreed to Escort Caravan"));

    // Previously unlogged mutators (set_item_economics, update_place_spatial) now append ledger events
    let ev_before_mut = eng_bak.db().verify_event_chain_detailed().unwrap().total_events;
    execute_tool(
        &eng_bak,
        "manage_economy",
        json!({
            "action": "set_item_weight",
            "item_key": "coarse-salt",
            "weight_kg": 10.0,
            "unit_value": 4,
            "currency": "silver"
        }),
    )
    .unwrap();
    execute_tool(
        &eng_bak,
        "manage_travel_and_exploration",
        json!({
            "action": "update_spatial",
            "place_id": "glass-hollow",
            "x": 12.0,
            "y": 4.5,
            "region_id": "heartland"
        }),
    )
    .unwrap();
    let final_chain = eng_bak.db().verify_event_chain_detailed().unwrap();
    assert!(final_chain.valid);
    assert_eq!(final_chain.total_events, ev_before_mut + 2);
}

#[test]
fn test_verbatim_chunked_file_reads_dnd_skill_table_xp_and_dc_rubric() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);
    engine.compile_docs().unwrap();

    // =========================================================================
    // 1. Verbatim multi-chunk file reads (zero compression, zero truncation loss,
    //    handles .md stored with application/octet-stream, blocks path traversal!)
    // =========================================================================
    let mut big_doc = String::new();
    for line_no in 1..=250 {
        big_doc.push_str(&format!(
            "Line {:03}: Exact verbatim archival record alpha-beta-gamma-delta-epsilon-zeta.\n",
            line_no
        ));
    }
    assert!(big_doc.chars().count() > 15_000);

    let imp = execute_tool(
        &engine,
        "import_attachment",
        json!({
            "id": "archive-dossier",
            "title": "Long Archive Dossier",
            "filename": "archive-dossier.md",
            "mime_type": "application/octet-stream",
            "text_content": big_doc,
            "category": "world-doc",
            "summary": "Large dossier for verbatim chunk testing"
        }),
    )
    .unwrap();
    assert_eq!(imp["id"], "archive-dossier");

    // get_attachment MUST detect .md text even with application/octet-stream MIME and paginate verbatim
    let att_first = execute_tool(
        &engine,
        "get_attachment",
        json!({ "attachment_id": "archive-dossier", "max_chars": 6000 }),
    )
    .unwrap();
    assert_eq!(att_first["verbatim"], true);
    assert_eq!(att_first["text_has_more"], true);
    assert_eq!(att_first["text_next_offset_chars"], 6000);
    assert_eq!(
        att_first["text_total_chars"].as_u64().unwrap() as usize,
        big_doc.chars().count()
    );

    // Multi-request verbatim reconstruction via read_campaign_file MUST match byte-for-byte!
    let mut reassembled = String::new();
    let mut offset = 0u64;
    let mut chunk_calls = 0usize;
    loop {
        chunk_calls += 1;
        let chunk = execute_tool(
            &engine,
            "read_campaign_file",
            json!({
                "target": "attachment:archive-dossier",
                "offset_chars": offset,
                "chunk_chars": 6000
            }),
        )
        .unwrap();
        assert_eq!(chunk["verbatim"], true);
        reassembled.push_str(chunk["content"].as_str().unwrap());
        if chunk["has_more"].as_bool().unwrap() {
            offset = chunk["next_offset_chars"].as_u64().unwrap();
            assert!(chunk["next_call"]["offset_chars"].as_u64().unwrap() == offset);
        } else {
            break;
        }
    }
    assert!(chunk_calls >= 3, "Expected >= 3 chunks for 15k+ char file, got {chunk_calls}");
    assert_eq!(
        reassembled, big_doc,
        "Reassembled multi-chunk read_campaign_file content must be 100% verbatim!"
    );

    // Reading compiled docs verbatim + listing files + blocking path traversal
    let doc_idx = execute_tool(
        &engine,
        "read_campaign_file",
        json!({ "doc_path": "INDEX.md" }),
    )
    .unwrap();
    assert_eq!(doc_idx["verbatim"], true);
    assert!(doc_idx["content"].as_str().unwrap().contains("Aria Vale"));

    let listing = execute_tool(&engine, "read_campaign_file", json!({ "list": true })).unwrap();
    assert!(listing["attachments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["target"] == "attachment:archive-dossier"));

    let traversal_err = execute_tool(
        &engine,
        "read_campaign_file",
        json!({ "target": "doc:../../Windows/win.ini" }),
    );
    assert!(traversal_err.is_err(), "Path traversal must be blocked!");

    // Verify read_campaign_file is annotated readOnlyHint: true
    let tools = chronicle::mcp::all_tool_definitions();
    let rcf_tool = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "read_campaign_file")
        .unwrap();
    assert_eq!(rcf_tool["annotations"]["readOnlyHint"], true);

    // =========================================================================
    // 2. D&D 5e Preset, Ability Modifier Formula, Standard Skill Table,
    //    Grounded DC Rubric + Factors, Skill XP Progression, & Totals > 20!
    // =========================================================================
    let prof_res = execute_tool(
        &engine,
        "upsert_ruleset_profile",
        json!({ "preset": "dnd5e", "id": "default" }),
    )
    .unwrap();
    assert_eq!(prof_res["profile"]["stat_modifier_mode"], "dnd5e");
    assert!(prof_res["profile"]["skill_table"]["stealth"]["governing_stat"] == "Dexterity");
    assert_eq!(prof_res["profile"]["dc_rubric"]["hard"], 20);

    // Give Aria D&D ability scores: Dexterity 18 (+4), Strength 16 (+3)
    execute_tool(
        &engine,
        "modify_character_stat",
        json!({ "character_id": "aria-vale", "stat": "Dexterity", "set_value": 18, "reason": "D&D baseline" }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "modify_character_stat",
        json!({ "character_id": "aria-vale", "stat": "Strength", "set_value": 16, "reason": "D&D baseline" }),
    )
    .unwrap();

    // Untrained standard D&D skill ("Stealth") with grounded DC rubric ("hard" = 20) + factors:
    //   - NO `stat` passed -> auto-hydrates Dexterity 18 -> +4!
    //   - `dc_tier: "hard"` (20) + "rushed" (+2) + "proper_tools" (-2) + custom (+2) = DC 22!
    //   - Auto-awards difficulty-scaled Skill XP on live check!
    let check1 = execute_tool(
        &engine,
        "resolve_check",
        json!({
            "actor_id": "aria-vale",
            "action": "Slip across the magistrate rooftop in silence",
            "skill": "Stealth",
            "dc_tier": "hard",
            "dc_factors": [
                "rushed",
                "proper_tools",
                { "label": "Wet roof tiles", "delta": 2, "reason": "Rain-slicked slate" }
            ],
            "seed": 1337
        }),
    )
    .unwrap();
    assert_eq!(check1["difficulty"], 22);
    assert_eq!(check1["dc_breakdown"]["base_tier"], "hard");
    assert_eq!(check1["dc_breakdown"]["base_dc"], 20);
    assert_eq!(check1["dc_breakdown"]["final_dc"], 22);
    let mods1 = check1["modifiers"].as_array().unwrap();
    assert!(
        mods1.iter().any(|m| m["source"] == "stat:Dexterity" && m["value"] == 4),
        "Dexterity 18 must auto-hydrate as +4 for Stealth! Got: {mods1:?}"
    );
    assert!(
        mods1.iter().any(|m| m["source"] == "skill:Stealth" && m["value"] == 0),
        "Untrained standard skill Stealth should add +0 proficiency, got: {mods1:?}"
    );
    // Automatic Skill XP awarded on live check!
    let prog1 = &check1["skill_progression"];
    assert!(prog1["xp_gained"].as_u64().unwrap() > 0);
    assert_eq!(prog1["skill_name"], "Stealth");

    // Practice / activity XP increases proficiency rank automatically!
    let xp_up = execute_tool(
        &engine,
        "upsert_character_skill",
        json!({
            "character_id": "aria-vale",
            "name": "Stealth",
            "add_xp": 700,
            "reason": "Weeks of rooftop scouting and shadow-step drills"
        }),
    )
    .unwrap();
    assert_eq!(xp_up["progression"]["leveled_up"], true);
    assert_eq!(xp_up["progression"]["new_rank"], "Adept");
    assert_eq!(xp_up["progression"]["new_bonus"], 4);

    // Subsequent d20 check hydrates BOTH Dexterity (+4) AND Stealth Adept (+4) = +8!
    // Find a seed where d20 roll is high enough to confirm total > 20 naturally!
    let mut saw_over_20 = false;
    for s in 1u64..=30 {
        let chk = execute_tool(
            &engine,
            "resolve_check",
            json!({
                "actor_id": "aria-vale",
                "action": "Vanish from watchtower lanterns",
                "skill": "Stealth",
                "dc_tier": "hard",
                "seed": s,
                "log_to_ledger": false
            }),
        )
        .unwrap();
        let roll = chk["roll"].as_i64().unwrap();
        let total = chk["total"].as_i64().unwrap();
        let mods = chk["modifiers"].as_array().unwrap();
        assert!(mods.iter().any(|m| m["source"] == "stat:Dexterity" && m["value"] == 4));
        assert!(mods.iter().any(|m| m["source"] == "skill:Stealth" && m["value"] == 4));
        assert!(total >= roll + 8 || chk["modifier"].as_i64().unwrap() != 8);
        if total > 20 {
            saw_over_20 = true;
            assert!(matches!(chk["outcome"].as_str().unwrap(), "success" | "critical_success"));
            break;
        }
    }
    assert!(saw_over_20, "d20 + stat (+4) + skill proficiency (+4) must exceed 20 on high rolls!");

    // Final ledger chain remains 100% valid
    let chain = engine.db().verify_event_chain_detailed().unwrap();
    assert!(chain.valid);

    // 3. Auto-scaled Skill XP on a 0-100 custom XP curve (no more 1-afternoon Mastery!)
    let curve_100 = vec![
        chronicle::mechanics::SkillXpThreshold { rank: "Unfamiliar".into(), min_xp: 0, bonus: 0 },
        chronicle::mechanics::SkillXpThreshold { rank: "Mastery".into(), min_xp: 100, bonus: 5 },
    ];
    let small_xp = chronicle::mechanics::calculate_skill_xp_gain("success", 15, &curve_100);
    assert!(
        (1..=3).contains(&small_xp),
        "0-100 XP curve should grant ~1-3 XP per moderate check, got {small_xp}"
    );

    // 4. Web GM Studio OAuth Sign-In (`POST /oauth/ui-login` -> unlocks `/api/status`)
    let oauth = std::sync::Arc::new(std::sync::Mutex::new(
        chronicle::oauth::OAuthStore::new("gm-ui-pass-123".to_string()),
    ));
    let (st_unauth, _, _, _) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "https://example.ts.net",
        "GET",
        "/api/status",
        BTreeMap::new(),
        b"",
    );
    assert_eq!(st_unauth, 401);

    let (st_bad, _, _, _) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "https://example.ts.net",
        "POST",
        "/oauth/ui-login",
        BTreeMap::new(),
        br#"{"passphrase":"wrong"}"#,
    );
    assert_eq!(st_bad, 403);

    let (st_login, _, _, login_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "https://example.ts.net",
        "POST",
        "/oauth/ui-login",
        BTreeMap::new(),
        br#"{"passphrase":"gm-ui-pass-123"}"#,
    );
    assert_eq!(st_login, 200);
    let login_json: serde_json::Value = serde_json::from_slice(&login_body).unwrap();
    let tok = login_json["access_token"].as_str().unwrap();
    assert!(tok.starts_with("chr_at_"));

    let mut hdrs = BTreeMap::new();
    hdrs.insert("authorization".to_string(), format!("Bearer {tok}"));
    let (st_ok, _, _, _) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "https://example.ts.net",
        "GET",
        "/api/status",
        hdrs,
        b"",
    );
    assert_eq!(st_ok, 200);
}

#[test]
fn test_stateful_progression_xianxia_parallel_manuals_bottlenecks_and_scifi_levels() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    // 1. Define Xianxia main Qi ladder (1-9 layers), parallel Body Tempering ladder,
    //    custom manual-linked ladder, D&D Level 1-20 ladder, and Sci-Fi Tier ladder
    let qi_lad = execute_tool(
        &engine,
        "manage_progression",
        json!({ "action": "define_ladder", "preset": "xianxia_standard" }),
    )
    .unwrap();
    assert_eq!(qi_lad["ladder"]["id"], "qi-cultivation");
    assert_eq!(qi_lad["ladder"]["stages"][1]["sub_stages"], 9);

    execute_tool(
        &engine,
        "manage_progression",
        json!({ "action": "define_ladder", "preset": "body_tempering" }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "manage_progression",
        json!({ "action": "define_ladder", "preset": "dnd_level_20" }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "manage_progression",
        json!({ "action": "define_ladder", "preset": "scifi_tier" }),
    )
    .unwrap();

    // Custom per-manual parallel progression ladder (1-9 layers with bespoke properties & halt condition)
    execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "define_ladder",
            "ladder_id": "still-water-manual",
            "name": "Still Water Mirroring the Moon",
            "kind": "parallel_manual",
            "source_text_id": "scroll-of-binding",
            "default_check_tags": ["meditation", "calm"],
            "stages": [
                {
                    "name": "Mirror-Breath First Cycle",
                    "sub_stages": 9,
                    "sub_stage_prefix": "Layer",
                    "xp_per_sub_stage": 40,
                    "breakthrough_required_at_peak": true,
                    "breakthrough_dc_tier": "moderate",
                    "passive_modifier": 2,
                    "properties": ["Steady heartbeat under duress", "Cold-water meridian clarity"],
                    "halt_conditions": ["Pavilion check-in required after Layer 5"]
                },
                {
                    "name": "Still-Moon Second Cycle",
                    "sub_stages": 9,
                    "sub_stage_prefix": "Layer",
                    "xp_per_sub_stage": 100,
                    "breakthrough_required_at_peak": true,
                    "breakthrough_dc_tier": "hard",
                    "passive_modifier": 4,
                    "properties": ["Reflective qi deflection"]
                }
            ]
        }),
    )
    .unwrap();

    let ladders = execute_tool(&engine, "manage_progression", json!({ "action": "list_ladders" })).unwrap();
    assert_eq!(ladders["count"], 5);

    // 2. Enroll THREE parallel tracks simultaneously on Aria Vale!
    let qi_en = execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "enroll_track",
            "character_id": "aria-vale",
            "track_id": "qi-cultivation",
            "name": "Orthodox Qi Cultivation",
            "track_kind": "primary_realm",
            "ladder_id": "qi-cultivation",
            "major_stage_index": 1,
            "sub_stage": 1,
            "reason": "Opened meridians into Qi Condensation Layer 1"
        }),
    )
    .unwrap();
    assert_eq!(qi_en["track"]["major_stage_name"], "Qi Condensation");
    assert_eq!(qi_en["track"]["sub_stage"], 1);
    assert_eq!(qi_en["track"]["sub_stage_max"], 9);
    assert_eq!(qi_en["track"]["check_modifier"], 1);

    execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "enroll_track",
            "character_id": "aria-vale",
            "track_id": "body-tempering",
            "name": "Body Tempering",
            "track_kind": "body_refinement",
            "ladder_id": "body-tempering",
            "major_stage_index": 0,
            "sub_stage": 1
        }),
    )
    .unwrap();

    execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "enroll_track",
            "character_id": "aria-vale",
            "track_id": "still-water-manual",
            "name": "Still Water Mirroring the Moon",
            "track_kind": "parallel_manual",
            "ladder_id": "still-water-manual",
            "major_stage_index": 0,
            "sub_stage": 1,
            "gm_notes": "SECRET_BLUEPRINT_MISMATCH_MARKER"
        }),
    )
    .unwrap();

    // 3. Advance Qi Cultivation XP past all 9 layers -> MUST halt cleanly at Layer 9 bottleneck!
    let adv_qi = execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "advance_xp",
            "character_id": "aria-vale",
            "track_id": "qi-cultivation",
            "xp": 5000,
            "reason": "Three years of spirit-vein seclusion"
        }),
    )
    .unwrap();
    assert_eq!(adv_qi["track"]["major_stage_name"], "Qi Condensation");
    assert_eq!(adv_qi["track"]["sub_stage"], 9);
    assert_eq!(adv_qi["hit_bottleneck"], true);
    assert_eq!(adv_qi["at_bottleneck"], true);
    assert_eq!(adv_qi["track"]["cumulative_xp"], 5000);

    // 4. Failed breakthrough (impossibly high DC 50) -> stays at Layer 9 bottleneck
    let bt_fail = execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "attempt_breakthrough",
            "character_id": "aria-vale",
            "track_id": "qi-cultivation",
            "difficulty": 50,
            "seed": 111
        }),
    )
    .unwrap();
    assert_eq!(bt_fail["succeeded"], false);
    assert_eq!(bt_fail["track"]["major_stage_name"], "Qi Condensation");
    assert_eq!(bt_fail["track"]["at_bottleneck"], true);

    // Successful breakthrough (DC 2) -> promotes to Foundation Establishment Layer 1, unlocks Liquefied True Qi & +3 mod!
    let bt_ok = execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "attempt_breakthrough",
            "character_id": "aria-vale",
            "track_id": "qi-cultivation",
            "difficulty": 2,
            "seed": 222
        }),
    )
    .unwrap();
    assert_eq!(bt_ok["succeeded"], true);
    assert_eq!(bt_ok["track"]["major_stage_name"], "Foundation Establishment");
    assert_eq!(bt_ok["track"]["sub_stage"], 1);
    assert_eq!(bt_ok["track"]["at_bottleneck"], false);
    assert_eq!(bt_ok["track"]["check_modifier"], 3);
    assert!(bt_ok["track"]["active_properties"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p.as_str().unwrap().contains("Liquefied True Qi")));

    // 5. resolve_check auto-hydrates active progression track modifiers (+3 Qi Cultivation + +2 Still Water Manual = +5 on meditation!)
    let med_chk = execute_tool(
        &engine,
        "resolve_check",
        json!({
            "actor_id": "aria-vale",
            "action": "Stabilize spiritual meditation inside the spirit vein",
            "dc_tier": "hard",
            "dry_run": true
        }),
    )
    .unwrap();
    let mods = med_chk["modifiers"].as_array().unwrap();
    assert!(
        mods.iter().any(|m| m["source"] == "progression:qi-cultivation" && m["value"] == 3),
        "Expected progression:qi-cultivation (+3) auto-hydrated! Got: {mods:?}"
    );
    assert!(
        mods.iter().any(|m| m["source"] == "progression:still-water-manual" && m["value"] == 2),
        "Expected progression:still-water-manual (+2) auto-hydrated! Got: {mods:?}"
    );

    // 6. Halted track blocks XP advancement until cleared via update_track
    execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "update_track",
            "character_id": "aria-vale",
            "track_id": "body-tempering",
            "status": "halted",
            "reason": "Pavilion safety halt pending check-in"
        }),
    )
    .unwrap();
    let halt_err = execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "advance_xp",
            "character_id": "aria-vale",
            "track_id": "body-tempering",
            "xp": 100
        }),
    );
    assert!(halt_err.is_err(), "Halted track must refuse XP advancement until cleared");

    // 7. Flat D&D Level 1-20 & Sci-Fi Tier 1-5 ladders work identically on another character
    execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "enroll_track",
            "character_id": "archivist-bren",
            "track_id": "character-level",
            "name": "Scholar Level",
            "track_kind": "level",
            "ladder_id": "character-level",
            "major_stage_index": 0
        }),
    )
    .unwrap();
    let lvl_up = execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "advance_xp",
            "character_id": "archivist-bren",
            "track_id": "character-level",
            "xp": 1000,
            "reason": "Solved the Whispering Archive cipher"
        }),
    )
    .unwrap();
    assert_eq!(lvl_up["major_promoted"], true);
    assert_eq!(lvl_up["track"]["major_stage_name"], "Level 3");

    // 8. Compiled character sheet & POV-safe Scene Context verification
    engine.compile_docs().unwrap();
    let aria_md = std::fs::read_to_string(tmp.path().join("docs/characters/aria-vale.md")).unwrap();
    assert!(aria_md.contains("### Progression, Realms & Parallel Tracks"));
    assert!(aria_md.contains("Foundation Establishment"));
    assert!(aria_md.contains("Still Water Mirroring the Moon"));

    let gm_ctx = execute_tool(
        &engine,
        "get_scene_context",
        json!({ "character_ids": ["aria-vale"], "perspective": "gm", "format": "markdown" }),
    )
    .unwrap();
    let gm_str = gm_ctx.as_str().or_else(|| gm_ctx["context_markdown"].as_str()).unwrap();
    assert!(gm_str.contains("Foundation Establishment"));
    assert!(gm_str.contains("SECRET_BLUEPRINT_MISMATCH_MARKER"));

    let pc_ctx = execute_tool(
        &engine,
        "get_scene_context",
        json!({ "character_ids": ["aria-vale"], "perspective": "aria-vale", "include_gm_secrets": true, "format": "markdown" }),
    )
    .unwrap();
    let pc_str = pc_ctx.as_str().or_else(|| pc_ctx["context_markdown"].as_str()).unwrap();
    assert!(pc_str.contains("Foundation Establishment"));
    assert!(
        !pc_str.contains("SECRET_BLUEPRINT_MISMATCH_MARKER"),
        "Progression track gm_notes must NEVER leak to player POV!"
    );

    // 9. Web GM Studio UI & REST Progression endpoints verification
    let oauth = std::sync::Arc::new(std::sync::Mutex::new(
        chronicle::oauth::OAuthStore::new("gm-prog-pass".to_string()),
    ));
    let (st_ui, _, _, ui_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "GET",
        "/",
        BTreeMap::new(),
        b"",
    );
    assert_eq!(st_ui, 200);
    let ui_html = String::from_utf8(ui_body).unwrap();
    assert!(ui_html.contains("id=\"tab-progression\""));
    assert!(ui_html.contains("Progression &amp; Realms"));
    assert!(ui_html.contains("Progression, Realms &amp; Parallel Tracks"));
    assert!(ui_html.contains("quickSkillXp"));

    let (_, _, _, login_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "POST",
        "/oauth/ui-login",
        BTreeMap::new(),
        br#"{"passphrase":"gm-prog-pass"}"#,
    );
    let tok = serde_json::from_slice::<serde_json::Value>(&login_body).unwrap()["access_token"]
        .as_str()
        .unwrap()
        .to_string();
    let mut auth_h = BTreeMap::new();
    auth_h.insert("authorization".to_string(), format!("Bearer {tok}"));

    let (st_char, _, _, char_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "GET",
        "/api/characters/aria-vale",
        auth_h.clone(),
        b"",
    );
    assert_eq!(st_char, 200);
    let char_json: serde_json::Value = serde_json::from_slice(&char_body).unwrap();
    assert_eq!(char_json["progression_tracks"].as_array().unwrap().len(), 3);
    assert_eq!(char_json["progression_ladders"].as_array().unwrap().len(), 5);

    let (st_prog, _, _, prog_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "GET",
        "/api/progression",
        auth_h.clone(),
        b"",
    );
    assert_eq!(st_prog, 200);
    let prog_json: serde_json::Value = serde_json::from_slice(&prog_body).unwrap();
    assert_eq!(prog_json["ladder_count"], 5);
    assert_eq!(prog_json["track_count"], 4);

    let (st_prog_bren, _, _, bren_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "GET",
        "/api/progression?character_id=archivist-bren",
        auth_h,
        b"",
    );
    assert_eq!(st_prog_bren, 200);
    let bren_json: serde_json::Value = serde_json::from_slice(&bren_body).unwrap();
    assert_eq!(bren_json["track_count"], 1);
    assert_eq!(bren_json["tracks"][0]["major_stage_name"], "Level 3");

    // 10. XP Revert, Correction, Stage Demotion, Stage/Full Reset, Remove Track & Skill XP Reset
    // 10a. Revert XP across a sub-stage / major-stage boundary on archivist-bren's Level 3 track (-800 XP -> steps back!)
    let rev_bren = execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "revert_xp",
            "character_id": "archivist-bren",
            "track_id": "character-level",
            "xp": 800,
            "allow_demote": true,
            "reason": "Reverting miscalculated quest XP award"
        }),
    )
    .unwrap();
    assert_eq!(rev_bren["major_demoted"], true);
    assert_eq!(rev_bren["track"]["major_stage_name"], "Level 1");
    assert_eq!(rev_bren["track"]["current_xp"], 200);
    assert_eq!(rev_bren["track"]["cumulative_xp"], 200);

    // 10b. Reset current stage XP on aria-vale's Foundation Establishment track via reset_xp
    execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "advance_xp",
            "character_id": "aria-vale",
            "track_id": "qi-cultivation",
            "xp": 400,
            "reason": "Temporary meditation award to test reset"
        }),
    )
    .unwrap();
    let stg_reset = execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "reset_xp",
            "character_id": "aria-vale",
            "track_id": "qi-cultivation",
            "reset_stage_xp": true,
            "reason": "Clear accidental meditation XP"
        }),
    )
    .unwrap();
    assert_eq!(stg_reset["track"]["current_xp"], 0);
    assert_eq!(stg_reset["track"]["major_stage_name"], "Foundation Establishment");

    // 10c. Full track reset back to Stage 0, Sub-stage 1, 0 XP
    let full_reset = execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "reset_xp",
            "character_id": "archivist-bren",
            "track_id": "character-level",
            "reset_track": true,
            "reason": "Campaign rebalance reset"
        }),
    )
    .unwrap();
    assert_eq!(full_reset["track"]["major_stage_index"], 0);
    assert_eq!(full_reset["track"]["sub_stage"], 1);
    assert_eq!(full_reset["track"]["current_xp"], 0);
    assert_eq!(full_reset["track"]["cumulative_xp"], 0);

    // 10d. Remove a mistakenly enrolled track cleanly
    let rem = execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "remove_track",
            "character_id": "archivist-bren",
            "track_id": "character-level",
            "reason": "Removed test track"
        }),
    )
    .unwrap();
    assert_eq!(rem["removed"], true);
    assert!(engine.list_character_progressions("archivist-bren").unwrap().is_empty());

    // 10e. Skill XP negative delta (revert) & reset_xp recalculate rank/modifier downwards cleanly!
    execute_tool(
        &engine,
        "upsert_character_skill",
        json!({
            "character_id": "aria-vale",
            "name": "Stealth",
            "add_xp": 500,
            "reason": "Over-awarded stealth XP"
        }),
    )
    .unwrap();
    let after_award = engine.get_character("aria-vale", None).unwrap();
    let st_awarded = after_award.current.skills.iter().find(|s| s.name == "Stealth").unwrap();
    assert!(st_awarded.xp >= 500);
    assert_ne!(st_awarded.rank, "Untrained");

    let sk_rev = execute_tool(
        &engine,
        "upsert_character_skill",
        json!({
            "character_id": "aria-vale",
            "name": "Stealth",
            "add_xp": -450,
            "reason": "Reverting accidental 450 XP award"
        }),
    )
    .unwrap();
    assert_eq!(sk_rev["new_xp"], st_awarded.xp - 450);
    assert_eq!(sk_rev["rank_changed"], true);

    let sk_reset = execute_tool(
        &engine,
        "upsert_character_skill",
        json!({
            "character_id": "aria-vale",
            "name": "Stealth",
            "reset_xp": true,
            "reason": "Full skill XP reset"
        }),
    )
    .unwrap();
    assert_eq!(sk_reset["new_xp"], 0);
    assert_eq!(sk_reset["new_rank"], "Untrained");
    assert_eq!(sk_reset["new_modifier"], "+0");

    // 11. Cultivation Manual Linking & Manual Notes on both Progression Tracks and Techniques
    execute_tool(
        &engine,
        "upsert_text",
        json!({
            "id": "still-water-manual",
            "title": "Still Water Mirroring the Moon Codex",
            "medium": "manual",
            "subject": "Water-Attribute Qi Cultivation",
            "summary": "Orthodox calming meridian circulation scripture.",
            "content": "Chapter 1: Settle the breath like winter ice over a deep well.",
            "read_status": "read"
        }),
    )
    .unwrap();

    // Link manual + add cultivation notes onto aria-vale's parallel manual track
    let linked_track = execute_tool(
        &engine,
        "manage_progression",
        json!({
            "action": "update_track",
            "character_id": "aria-vale",
            "track_id": "still-water-manual",
            "source_text_id": "still-water-manual",
            "manual_notes": "Requires slow 9-count exhale; incompatible with fiery Yang-surge arts.",
            "reason": "Linked canonical cultivation manual and breathing notes"
        }),
    )
    .unwrap();
    assert_eq!(linked_track["track"]["source_text_id"], "still-water-manual");
    assert_eq!(
        linked_track["track"]["manual_title"],
        "Still Water Mirroring the Moon Codex"
    );
    assert!(linked_track["track"]["manual_notes"]
        .as_str()
        .unwrap()
        .contains("9-count exhale"));

    // Link the same manual + parallel track + manual notes onto a cultivation technique
    let linked_tech = execute_tool(
        &engine,
        "upsert_character_technique",
        json!({
            "character_id": "aria-vale",
            "name": "Mirror Moon Palm",
            "tier": "Mortal High",
            "cost": "8 Qi",
            "mastery": "Minor Success",
            "effect": "Redirects incoming qi strikes with calm water-phase force.",
            "source_text_id": "still-water-manual",
            "progression_track_id": "still-water-manual",
            "manual_notes": "Must rotate dantian counter-clockwise before releasing palm strike.",
            "reason": "Comprehended from Chapter 1 of Still Water Codex"
        }),
    )
    .unwrap();
    let mirror_palm = linked_tech["techniques"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "Mirror Moon Palm")
        .unwrap();
    assert_eq!(mirror_palm["source_text_id"], "still-water-manual");
    assert_eq!(
        mirror_palm["manual_title"],
        "Still Water Mirroring the Moon Codex"
    );
    assert_eq!(mirror_palm["progression_track_id"], "still-water-manual");
    assert!(mirror_palm["manual_notes"]
        .as_str()
        .unwrap()
        .contains("counter-clockwise"));

    // Verify compiled Markdown & Scene Context include linked manual & manual notes
    engine.compile_docs().unwrap();
    let aria_md_manual =
        std::fs::read_to_string(tmp.path().join("docs/characters/aria-vale.md")).unwrap();
    assert!(aria_md_manual.contains("[Still Water Mirroring the Moon Codex](../texts/still-water-manual.md)"));
    assert!(aria_md_manual.contains("9-count exhale"));
    assert!(aria_md_manual.contains("Mirror Moon Palm"));
    assert!(aria_md_manual.contains("counter-clockwise"));

    let ctx_manual = execute_tool(
        &engine,
        "get_scene_context",
        json!({ "character_ids": ["aria-vale"], "perspective": "aria-vale", "format": "markdown" }),
    )
    .unwrap();
    let ctx_manual_str = ctx_manual
        .as_str()
        .or_else(|| ctx_manual["context_markdown"].as_str())
        .unwrap();
    assert!(ctx_manual_str.contains("Still Water Mirroring the Moon Codex"));
    assert!(ctx_manual_str.contains("9-count exhale"));
    assert!(ctx_manual_str.contains("counter-clockwise"));

    // Verify REST API surfaces `manuals` list for Web GM Studio dropdowns
    let mut auth_h2 = BTreeMap::new();
    auth_h2.insert("authorization".to_string(), format!("Bearer {tok}"));
    let (st_prog2, _, _, prog_body2) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "GET",
        "/api/progression",
        auth_h2,
        b"",
    );
    assert_eq!(st_prog2, 200);
    let prog_json2: serde_json::Value = serde_json::from_slice(&prog_body2).unwrap();
    assert!(prog_json2["manuals"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == "still-water-manual"));

    // Verify new UI controls exist in served HTML
    assert!(ui_html.contains("modalEditTrack"));
    assert!(ui_html.contains("modalEditTechnique"));
    assert!(ui_html.contains("et_source_text"));
    assert!(ui_html.contains("et_manual_notes"));
    assert!(ui_html.contains("tm_manual_notes"));
    assert!(ui_html.contains("adjustTrackXp"));
    assert!(ui_html.contains("resetTrackXp"));
    assert!(ui_html.contains("resetSkillXp"));

    // Final SHA-256 chain validity
    assert!(engine.db().verify_event_chain_detailed().unwrap().valid);
}

#[test]
fn test_dedicated_player_character_sheet_avatar_titles_and_segregated_xp() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    // 1. Update Avatar, Body, Proportions & Measurements (omitting character_id defaults to primary PC aria-vale!)
    let av = execute_tool(
        &engine,
        "manage_player_character",
        json!({
            "action": "update_avatar",
            "avatar_summary": "Lean mountain scout with a runner's frame, weathered hands, and steady slate-grey eyes.",
            "height": "174 cm",
            "weight": "63 kg",
            "chronological_age": "23",
            "apparent_age": "Early 20s",
            "lifespan_years": 120,
            "body_type_or_build": "Lean athletic",
            "silhouette": "Compact runner's frame",
            "proportions_summary": "7.5 heads tall, long reach",
            "eyes": "Slate-grey, sharp focus",
            "hair": "Dark ash-brown, braided back",
            "voice_and_speech": "Low, unhurried cadence",
            "measurements": {
                "Shoulders": "44 cm",
                "Chest": "92 cm",
                "Waist": "71 cm",
                "Inseam": "81 cm"
            },
            "reason": "Initialized canonical PC avatar body profile"
        }),
    )
    .unwrap();
    assert_eq!(av["character_id"], "aria-vale");
    assert_eq!(av["avatar"]["height"], "174 cm");
    assert_eq!(av["avatar"]["measurements"]["Waist"], "71 cm");
    assert!(av["visual_prompt"].as_str().unwrap().contains("174 cm"));

    // 2. Permanent Physical Markers & Modular Wardrobe Outfits
    execute_tool(
        &engine,
        "manage_player_character",
        json!({
            "action": "upsert_marker",
            "kind": "scar",
            "location": "Left forearm",
            "description": "Crescent burn scar from an ash-lamp shard",
            "origin_or_story": "Winter watch at Glass Hollow",
            "visible_when_clothed": true
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "manage_player_character",
        json!({
            "action": "upsert_outfit",
            "name": "Ridge Scout Cloak",
            "occasion": "adventuring",
            "outer_layer": "Waxed charcoal travel cloak",
            "top_layer": "Fitted grey linen tunic",
            "bottoms": "Reinforced riding trousers",
            "footwear": "Soft-soled mountain boots",
            "color_palette": "Charcoal, slate, weathered leather",
            "set_active": true
        }),
    )
    .unwrap();

    // 3. Titles & Epithets (public + GM-secret) & Innate Traits/Physique
    execute_tool(
        &engine,
        "manage_player_character",
        json!({
            "action": "upsert_title",
            "name": "Lantern-Bearer of Glass Hollow",
            "kind": "epithet",
            "equipped": true,
            "source": "Cleared the fog shrine",
            "mechanical_effect": "+1 Persuasion with mountain wardens",
            "visibility": "public"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "manage_player_character",
        json!({
            "action": "upsert_title",
            "name": "Marked by the Sunken Seal",
            "kind": "curse",
            "equipped": true,
            "visibility": "gm_secret",
            "mechanical_effect": "SECRET_TITLE_MARKER"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "manage_player_character",
        json!({
            "action": "upsert_trait",
            "name": "Clear-Stream Meridian Physique",
            "category": "physique",
            "rank_or_grade": "Mortal Upper",
            "effect": "Steady qi circulation under cold weather"
        }),
    )
    .unwrap();

    // 4. Segregated Stat Training XP (auto-promotes +1 live stat, reverts negative XP, spends free stat point)
    let before_con = *engine.get_character("aria-vale", None).unwrap().current.stats.get("Constitution").unwrap_or(&0);
    let st_xp = execute_tool(
        &engine,
        "manage_player_character",
        json!({
            "action": "adjust_stat_xp",
            "stat": "Constitution",
            "delta_xp": 120,
            "training_notes": "Weighted ridge marches",
            "auto_promote": true
        }),
    )
    .unwrap();
    assert_eq!(st_xp["points_promoted"], 1);
    assert_eq!(st_xp["entry"]["current_xp"], 20);
    let after_con = *engine.get_character("aria-vale", None).unwrap().current.stats.get("Constitution").unwrap();
    assert_eq!(after_con, before_con + 1);

    // Award & spend 1 unspent stat point
    execute_tool(
        &engine,
        "manage_player_character",
        json!({ "action": "adjust_stat_xp", "unspent_stat_points_delta": 2 }),
    )
    .unwrap();
    let spent = execute_tool(
        &engine,
        "manage_player_character",
        json!({ "action": "adjust_stat_xp", "stat": "Constitution", "spend_stat_point": true }),
    )
    .unwrap();
    assert_eq!(spent["unspent_stat_points"], 1);

    // 5. Segregated Job / Class XP (advance -> level-up -> negative revert with demotion!)
    execute_tool(
        &engine,
        "manage_player_character",
        json!({
            "action": "upsert_job",
            "name": "Ridge Scout",
            "role": "primary",
            "tier_or_rank": "Journeyman",
            "perks_and_features": ["Silent Footwork", "Trail Sense"]
        }),
    )
    .unwrap();
    let job_up = execute_tool(
        &engine,
        "manage_player_character",
        json!({ "action": "adjust_job_xp", "job_id": "ridge-scout", "delta_xp": 300 }),
    )
    .unwrap();
    assert_eq!(job_up["job"]["level"], 2);
    assert_eq!(job_up["job"]["current_xp"], 50);

    let job_rev = execute_tool(
        &engine,
        "manage_player_character",
        json!({ "action": "adjust_job_xp", "job_id": "ridge-scout", "delta_xp": -100, "allow_demote": true }),
    )
    .unwrap();
    assert_eq!(job_rev["job"]["level"], 1);

    // 6. Formal Proficiencies (Weapon / Tool / Language with XP promote & revert/reset)
    execute_tool(
        &engine,
        "manage_player_character",
        json!({
            "action": "upsert_proficiency",
            "name": "Jian Straight Sword",
            "category": "weapon",
            "rank": "Familiar",
            "xp": 50
        }),
    )
    .unwrap();
    let prof_up = execute_tool(
        &engine,
        "manage_player_character",
        json!({ "action": "adjust_proficiency_xp", "id": "jian-straight-sword", "delta_xp": 210 }),
    )
    .unwrap();
    assert_eq!(prof_up["proficiency"]["rank"], "Expertise");
    assert_eq!(prof_up["proficiency"]["bonus"], "+4");

    let prof_reset = execute_tool(
        &engine,
        "manage_player_character",
        json!({ "action": "adjust_proficiency_xp", "id": "jian-straight-sword", "reset_xp": true }),
    )
    .unwrap();
    assert_eq!(prof_reset["proficiency"]["xp"], 0);
    assert_eq!(prof_reset["proficiency"]["rank"], "Untrained");

    // 7. Compiled Markdown & POV-safe Scene Context verification
    engine.compile_docs().unwrap();
    let md = std::fs::read_to_string(tmp.path().join("docs/characters/aria-vale.md")).unwrap();
    assert!(md.contains("### Avatar, Body, Proportions & Wardrobe"));
    assert!(md.contains("174 cm"));
    assert!(md.contains("Crescent burn scar"));
    assert!(md.contains("Ridge Scout Cloak"));
    assert!(md.contains("### Titles & Epithets"));
    assert!(md.contains("Lantern-Bearer of Glass Hollow"));
    assert!(md.contains("### Stat Training XP"));
    assert!(md.contains("### Jobs, Classes & Occupations"));
    assert!(md.contains("### Formal Proficiencies"));
    assert!(md.contains("Jian Straight Sword"));

    let pc_ctx = execute_tool(
        &engine,
        "get_scene_context",
        json!({ "character_ids": ["aria-vale"], "perspective": "aria-vale", "include_gm_secrets": true, "format": "markdown" }),
    )
    .unwrap();
    let pc_str = pc_ctx.as_str().or_else(|| pc_ctx["context_markdown"].as_str()).unwrap();
    assert!(pc_str.contains("Lantern-Bearer of Glass Hollow"));
    assert!(pc_str.contains("Ridge Scout Cloak"));
    assert!(pc_str.contains("Ridge Scout"));
    assert!(
        !pc_str.contains("SECRET_TITLE_MARKER"),
        "GM-secret titles must NEVER leak to player POV!"
    );

    // 8. REST GET /api/pc & Web GM Studio HTML verification
    let oauth = std::sync::Arc::new(std::sync::Mutex::new(
        chronicle::oauth::OAuthStore::new("gm-pc-pass".to_string()),
    ));
    let (_, _, _, login_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "POST",
        "/oauth/ui-login",
        BTreeMap::new(),
        br#"{"passphrase":"gm-pc-pass"}"#,
    );
    let tok = serde_json::from_slice::<serde_json::Value>(&login_body).unwrap()["access_token"]
        .as_str()
        .unwrap()
        .to_string();
    let mut auth_h = BTreeMap::new();
    auth_h.insert("authorization".to_string(), format!("Bearer {tok}"));

    let (st_pc, _, _, pc_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "GET",
        "/api/pc",
        auth_h,
        b"",
    );
    assert_eq!(st_pc, 200);
    let pc_json: serde_json::Value = serde_json::from_slice(&pc_body).unwrap();
    assert_eq!(pc_json["character_id"], "aria-vale");
    assert_eq!(pc_json["profile"]["avatar"]["height"], "174 cm");
    assert_eq!(pc_json["profile"]["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(pc_json["profile"]["titles"].as_array().unwrap().len(), 2);

    let (st_ui, _, _, ui_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "GET",
        "/",
        BTreeMap::new(),
        b"",
    );
    assert_eq!(st_ui, 200);
    let ui_html = String::from_utf8(ui_body).unwrap();
    assert!(ui_html.contains("id=\"tab-pc\""));
    assert!(ui_html.contains("Player Character"));
    assert!(ui_html.contains("pcSaveAvatar"));
    assert!(ui_html.contains("pcAdjustJobXp"));
    assert!(ui_html.contains("pcAdjustStatXp"));

    assert!(engine.db().verify_event_chain_detailed().unwrap().valid);
}

#[test]
fn test_living_world_prominent_npc_travel_networking_detours_and_rumors() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    // 1. Add a second settlement (`salt-quay`) and a prominent merchant NPC (`merchant-lin`) + open bidirectional route
    execute_tool(
        &engine,
        "create_place",
        json!({
            "id": "salt-quay",
            "name": "Salt Quay",
            "place_type": "port",
            "terrain": "coastal",
            "climate": "temperate",
            "description": "Busy river-port where salt guilds and envoys trade."
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "upsert_route",
            "id": "watch-to-salt",
            "from_place_id": "watch-keep",
            "to_place_id": "salt-quay",
            "bidirectional": true,
            "distance_km": 48.0,
            "terrain": "road",
            "status": "open"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "upsert_route",
            "id": "hollow-to-salt",
            "from_place_id": "glass-hollow",
            "to_place_id": "salt-quay",
            "bidirectional": true,
            "distance_km": 36.0,
            "terrain": "trail",
            "status": "open"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "create_character",
        json!({
            "id": "merchant-lin",
            "name": "Guildmaster Lin",
            "title": "Salt Guild Factor",
            "kind": "npc",
            "archetype": "Merchant",
            "faction": "Salt Guild",
            "initial_location": "salt-quay"
        }),
    )
    .unwrap();

    // Give Archivist Bren a public knowledge claim to carry & spread when networking!
    execute_tool(
        &engine,
        "record_knowledge",
        json!({
            "claim_id": "grain-shortage-north",
            "subject": "Northern Grain Harvest",
            "claim": "Early frost cut the northern valley grain yield by a third.",
            "truth_status": "true",
            "visibility": "public",
            "known_by": ["archivist-bren"],
            "stance": "documented",
            "confidence": 85,
            "source": "archive dispatch"
        }),
    )
    .unwrap();

    // 2. Configure Archivist Bren as a prominent networking envoy and dispatch a trip to Salt Quay
    execute_tool(
        &engine,
        "simulate_autonomy",
        json!({
            "mode": "configure_mobility",
            "character_id": "archivist-bren",
            "prominent": true,
            "mobility_mode": "networking_envoy",
            "home_place_id": "watch-keep",
            "preferred_destinations": ["salt-quay"],
            "trip_cadence_days": 5,
            "stay_duration_days": 2,
            "travel_speed_mode": "mount"
        }),
    )
    .unwrap();

    let disp = execute_tool(
        &engine,
        "simulate_autonomy",
        json!({
            "mode": "dispatch_trip",
            "character_id": "archivist-bren",
            "destination_place_id": "salt-quay",
            "purpose": "networking",
            "details": "Tariff audit & grain intelligence exchange",
            "stay_duration_days": 2,
            "travel_speed_mode": "mount"
        }),
    )
    .unwrap();
    assert_eq!(disp["trip"]["destination_place_id"], "salt-quay");

    // While in transit, `get_scene_context` explicitly shows Bren IN TRANSIT (not a sitting duck!)
    let ctx_transit = execute_tool(
        &engine,
        "get_scene_context",
        json!({ "place_id": "watch-keep", "format": "markdown" }),
    )
    .unwrap();
    let ctx_transit_str = ctx_transit
        .as_str()
        .or_else(|| ctx_transit["context_markdown"].as_str())
        .unwrap();
    assert!(ctx_transit_str.contains("## Living World: Prominent Figures Away / In Transit"));
    assert!(ctx_transit_str.contains("Archivist Bren"));
    assert!(ctx_transit_str.contains("IN TRANSIT"));

    // 3. Advance world time by 24h -> Bren arrives in Salt Quay, networks with Guildmaster Lin, and spreads the grain rumor!
    let tick1 = execute_tool(
        &engine,
        "advance_world",
        json!({
            "duration_hours": 24,
            "place_ids": ["salt-quay"],
            "tick_npc_movements": true,
            "seed": 42
        }),
    )
    .unwrap();
    assert!(tick1["npc_movements"]["events_count"].as_u64().unwrap() >= 1);
    let bren_now = engine.get_character("archivist-bren", None).unwrap();
    assert_eq!(bren_now.current.current_location.as_deref(), Some("salt-quay"));
    let bren_prof = engine.get_character_profile("archivist-bren").unwrap();
    assert_eq!(bren_prof.mobility.current_status, "visiting");
    assert!(
        !bren_prof.mobility.recent_engagements.is_empty(),
        "Arrival must log a business/networking engagement"
    );
    // Guildmaster Lin organically learned the public grain claim from networking with Bren!
    let lin_beliefs = engine.db().list_beliefs_for_actor("merchant-lin").unwrap();
    assert!(
        lin_beliefs.iter().any(|b| b.claim_id == "grain-shortage-north"),
        "Networking arrival should organically propagate public rumors between prominent figures!"
    );

    // 4. Sever ALL routes out of Salt Quay -> after 3 more days, Bren tries to return home and becomes STRANDED (never teleports!)
    execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "upsert_route",
            "id": "watch-to-salt",
            "from_place_id": "watch-keep",
            "to_place_id": "salt-quay",
            "bidirectional": true,
            "distance_km": 48.0,
            "terrain": "road",
            "status": "destroyed"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "upsert_route",
            "id": "hollow-to-salt",
            "from_place_id": "glass-hollow",
            "to_place_id": "salt-quay",
            "bidirectional": true,
            "distance_km": 36.0,
            "terrain": "trail",
            "status": "blocked"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "advance_world",
        json!({
            "duration_days": 3,
            "place_ids": ["salt-quay"],
            "tick_npc_movements": true,
            "seed": 43
        }),
    )
    .unwrap();
    let bren_stranded = engine.get_character_profile("archivist-bren").unwrap();
    assert_eq!(bren_stranded.mobility.current_status, "stranded");
    assert_eq!(
        engine.get_character("archivist-bren", None).unwrap().current.current_location.as_deref(),
        Some("salt-quay"),
        "Stranded NPC must NOT teleport across severed routes!"
    );

    // 5. Compiled Markdown, REST `/api/world/movements`, and Web UI verification
    engine.compile_docs().unwrap();
    let bren_md = std::fs::read_to_string(tmp.path().join("docs/characters/archivist-bren.md")).unwrap();
    assert!(bren_md.contains("### Mobility, Business Travel & Networking"));
    assert!(bren_md.contains("Recent Business & Networking Engagements"));

    let oauth = std::sync::Arc::new(std::sync::Mutex::new(
        chronicle::oauth::OAuthStore::new("gm-mv-pass".to_string()),
    ));
    let (_, _, _, login_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "POST",
        "/oauth/ui-login",
        BTreeMap::new(),
        br#"{"passphrase":"gm-mv-pass"}"#,
    );
    let tok = serde_json::from_slice::<serde_json::Value>(&login_body).unwrap()["access_token"]
        .as_str()
        .unwrap()
        .to_string();
    let mut auth_h = BTreeMap::new();
    auth_h.insert("authorization".to_string(), format!("Bearer {tok}"));
    let (st_mv, _, _, mv_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "GET",
        "/api/world/movements",
        auth_h,
        b"",
    );
    assert_eq!(st_mv, 200);
    let mv_json: serde_json::Value = serde_json::from_slice(&mv_body).unwrap();
    assert!(mv_json["figures"].as_array().unwrap().len() >= 2);
    assert!(!mv_json["recent_engagements"].as_array().unwrap().is_empty());

    let (st_ui, _, _, ui_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "GET",
        "/",
        BTreeMap::new(),
        b"",
    );
    assert_eq!(st_ui, 200);
    let ui_html = String::from_utf8(ui_body).unwrap();
    assert!(ui_html.contains("dashMovementsBody"));
    assert!(ui_html.contains("modalNpcMobility"));
    assert!(ui_html.contains("modalDispatchTrip"));

    assert!(engine.db().verify_event_chain_detailed().unwrap().valid);
}

#[test]
fn test_sparse_multiscale_2d_map_grids_compass_fog_and_atlas_api() {
    let tmp = TempDir::new().unwrap();
    let mut engine = CampaignEngine::open(tmp.path()).unwrap();
    seed_test_fixture(&mut engine);

    // 1. Create two grids at completely different genre scales (Regional 5 km + Sci-Fi Sector 1 AU)
    let reg = execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "upsert_grid",
            "grid_id": "valley-region",
            "name": "Glass Valley Regional Atlas",
            "scale_kind": "region",
            "width": 16,
            "height": 12,
            "cell_scale_label": "5 km",
            "default_terrain": "wilderness",
            "description": "Sparse regional grid spanning Glass Hollow and Watch Keep."
        }),
    )
    .unwrap();
    assert_eq!(reg["grid"]["id"], "valley-region");

    let scifi = execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "upsert_grid",
            "grid_id": "helix-sector",
            "name": "Helix Star Sector",
            "scale_kind": "space_sector",
            "width": 12,
            "height": 10,
            "cell_scale_label": "1 AU",
            "default_terrain": "vacuum"
        }),
    )
    .unwrap();
    assert_eq!(scifi["grid"]["scale_kind"], "space_sector");

    // 2. Batch-paint sparse cells: anchor Glass Hollow at (4,5), Watch Keep at (8,5), adjacent ridge at (4,4) with GM secret, and unexplored ruin at (5,5)
    let batch = execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "set_cell",
            "grid_id": "valley-region",
            "cells": [
                {
                    "x": 4, "y": 5,
                    "terrain": "ruins",
                    "place_id": "glass-hollow",
                    "fog_status": "charted"
                },
                {
                    "x": 8, "y": 5,
                    "terrain": "urban",
                    "place_id": "watch-keep",
                    "faction_control": "wardens",
                    "fog_status": "charted"
                },
                {
                    "x": 4, "y": 4,
                    "terrain": "mountain",
                    "landmark_name": "Whispering Ridge",
                    "hazards": ["Rockslide"],
                    "fog_status": "surveyed",
                    "gm_notes": "SECRET_CELL_AMBUSH_MARKER"
                },
                {
                    "x": 5, "y": 5,
                    "terrain": "forest",
                    "landmark_name": "HIDDEN_SHRINE_FOG_MARKER",
                    "fog_status": "unexplored"
                }
            ],
            "reason": "Charted Glass Valley sparse grid"
        }),
    )
    .unwrap();
    assert_eq!(batch["updated_count"], 4);

    // Anchoring a place on (x, y) automatically syncs PlaceSpatialExt.coordinates!
    let gh_sp = engine.db().get_place_spatial("glass-hollow").unwrap().unwrap();
    assert_eq!(gh_sp.coordinates, Some([4.0, 5.0]));
    assert_eq!(gh_sp.map_region, "valley-region");

    // 3. Verify compact, token-safe 3x3 compass neighborhood in get_scene_context & strict POV fog/secret isolation!
    let gm_ctx = execute_tool(
        &engine,
        "get_scene_context",
        json!({ "place_id": "glass-hollow", "perspective": "gm", "include_gm_secrets": true, "format": "markdown" }),
    )
    .unwrap();
    let gm_str = gm_ctx.as_str().or_else(|| gm_ctx["context_markdown"].as_str()).unwrap();
    assert!(gm_str.contains("Glass Valley Regional Atlas"));
    assert!(gm_str.contains("N (4,4): mountain [Landmark: Whispering Ridge]"));
    assert!(gm_str.contains("SECRET_CELL_AMBUSH_MARKER"));
    assert!(gm_str.contains("HIDDEN_SHRINE_FOG_MARKER"));
    assert!(gm_str.contains("4.0 cells E")); // Watch Keep is at (8,5) -> 4 cells East of (4,5)!

    let pc_ctx = execute_tool(
        &engine,
        "get_scene_context",
        json!({ "place_id": "glass-hollow", "perspective": "aria-vale", "include_gm_secrets": true, "format": "markdown" }),
    )
    .unwrap();
    let pc_str = pc_ctx.as_str().or_else(|| pc_ctx["context_markdown"].as_str()).unwrap();
    assert!(pc_str.contains("N (4,4): mountain [Landmark: Whispering Ridge]"));
    assert!(pc_str.contains("E (5,5): Unexplored"));
    assert!(
        !pc_str.contains("SECRET_CELL_AMBUSH_MARKER"),
        "Cell gm_notes must NEVER leak to player POV!"
    );
    assert!(
        !pc_str.contains("HIDDEN_SHRINE_FOG_MARKER"),
        "Unexplored fog cell landmarks must NEVER leak to player POV!"
    );

    // 4. Compiled Place Markdown includes Map Grid Position & Neighbors
    engine.compile_docs().unwrap();
    let gh_md = std::fs::read_to_string(tmp.path().join("docs/places/glass-hollow.md")).unwrap();
    assert!(gh_md.contains("### Map Grid Position & Neighbors"));
    assert!(gh_md.contains("Whispering Ridge"));

    // 5. Clear cell & auto_layout_grid work cleanly
    let clr = execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({ "action": "clear_cell", "grid_id": "valley-region", "x": 5, "y": 5 }),
    )
    .unwrap();
    assert_eq!(clr["cleared"], true);

    // 6. REST `/api/maps` & `/api/maps/<id>` (with route lines + character pins!) and Web UI HTML verification
    let oauth = std::sync::Arc::new(std::sync::Mutex::new(
        chronicle::oauth::OAuthStore::new("gm-map-pass".to_string()),
    ));
    let (_, _, _, login_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "POST",
        "/oauth/ui-login",
        BTreeMap::new(),
        br#"{"passphrase":"gm-map-pass"}"#,
    );
    let tok = serde_json::from_slice::<serde_json::Value>(&login_body).unwrap()["access_token"]
        .as_str()
        .unwrap()
        .to_string();
    let mut auth_h = BTreeMap::new();
    auth_h.insert("authorization".to_string(), format!("Bearer {tok}"));

    let (st_list, _, _, list_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "GET",
        "/api/maps",
        auth_h.clone(),
        b"",
    );
    assert_eq!(st_list, 200);
    let list_json: serde_json::Value = serde_json::from_slice(&list_body).unwrap();
    assert_eq!(list_json["count"], 2);

    let (st_one, _, _, one_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "GET",
        "/api/maps/valley-region",
        auth_h,
        b"",
    );
    assert_eq!(st_one, 200);
    let one_json: serde_json::Value = serde_json::from_slice(&one_body).unwrap();
    assert_eq!(one_json["placed_places"].as_array().unwrap().len(), 2);
    assert!(!one_json["routes"].as_array().unwrap().is_empty());
    assert!(!one_json["character_pins"].as_array().unwrap().is_empty());

    let (st_ui, _, _, ui_body) = chronicle::web::dispatch_http_with_oauth(
        &engine,
        &oauth,
        "http://127.0.0.1:7878",
        "GET",
        "/",
        BTreeMap::new(),
        b"",
    );
    assert_eq!(st_ui, 200);
    let ui_html = String::from_utf8(ui_body).unwrap();
    assert!(ui_html.contains("atlasGridCard"));
    assert!(ui_html.contains("modalNewGrid"));
    assert!(ui_html.contains("onAtlasCellClick"));

    assert!(engine.db().verify_event_chain_detailed().unwrap().valid);
}

#[test]
fn test_active_signal_bus_stage_rules_routines_auto_clocks_hydration_currency_and_domain_facades() {
    use chronicle::mcp::{tool_definitions_for_profile, ToolProfile};

    // 1. Verify Sanitized MCP Tool Surface (~80% token reduction) & Annotations
    let play_tools = tool_definitions_for_profile(ToolProfile::Play);
    let play_arr = play_tools.as_array().unwrap();
    assert_eq!(play_arr.len(), 12, "Play profile must expose 12 unified tools");
    let play_json = serde_json::to_string(&play_tools).unwrap();
    let legacy_json = serde_json::to_string(&chronicle::mcp::all_tool_definitions()).unwrap();
    assert!(
        play_json.len() < 32_000 && play_json.len() * 3 < legacy_json.len(),
        "Play profile ({} chars) should be dramatically smaller than legacy ({} chars)",
        play_json.len(),
        legacy_json.len()
    );
    let query_tool = play_arr.iter().find(|t| t["name"] == "query").unwrap();
    assert_eq!(query_tool["annotations"]["readOnlyHint"], true);
    let world_tool = play_arr.iter().find(|t| t["name"] == "world").unwrap();
    let world_props = world_tool["inputSchema"]["properties"].as_object().unwrap();
    assert!(world_props.contains_key("x") && world_props.contains_key("y"));
    assert!(world_props.contains_key("terrain") && world_props.contains_key("cells"));
    assert!(world_props.contains_key("fog_status"));
    assert_eq!(world_tool["inputSchema"]["additionalProperties"], true);
    let compact_full = tool_definitions_for_profile(ToolProfile::CompactFull);
    assert_eq!(compact_full.as_array().unwrap().len(), 13);
    let admin_tool = compact_full.as_array().unwrap().iter().find(|t| t["name"] == "admin").unwrap();
    let admin_props = admin_tool["inputSchema"]["properties"].as_object().unwrap();
    assert!(admin_props.contains_key("ruleset_notes") && admin_props.contains_key("append_ruleset_notes"));
    let codex_tool = play_arr.iter().find(|t| t["name"] == "codex").unwrap();
    assert!(codex_tool["inputSchema"]["properties"].as_object().unwrap().contains_key("always_include"));

    // 2. Setup fresh temp campaign using ONLY Unified Domain Facade Tools!
    let tmp = TempDir::new().unwrap();
    let engine = CampaignEngine::open(tmp.path()).unwrap();
    engine
        .init_campaign("Active Engine Test", "Multi-genre active rules", "d20", None)
        .unwrap();

    execute_tool(
        &engine,
        "world",
        json!({ "op": "create_place", "id": "workshop", "name": "Timber Workshop", "terrain": "Urban" }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "world",
        json!({ "op": "create_place", "id": "market-ward", "name": "Market Ward", "terrain": "Urban" }),
    )
    .unwrap();

    execute_tool(
        &engine,
        "character",
        json!({
            "op": "create",
            "id": "lin-mo",
            "name": "Lin Mo",
            "kind": "pc",
            "initial_location": "workshop",
            "initial_stats": { "Strength": 14, "Dexterity": 12, "Charisma": 10 },
            "initial_skills": [{ "name": "Carpentry", "rank": "Apprentice", "governing_stat": "Dexterity", "xp": 160 }]
        }),
    )
    .unwrap();

    // Set Day-1 baseline avatar & fitted trousers (Hips 99 cm)
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "pc_sheet",
            "sub_action": "update_avatar",
            "character_id": "lin-mo",
            "height": "178 cm",
            "weight": "80 kg",
            "facial_hair": "Light jaw stubble",
            "measurements": {
                "Shoulders": "49 cm",
                "Chest": "108 cm",
                "Waist": "88 cm",
                "Hips": "96 cm"
            }
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "equipment",
            "sub_action": "acquire",
            "character_id": "lin-mo",
            "item_name": "Fitted Work Trousers",
            "equipped": true,
            "garment_cut": "fitted",
            "garment_taper": "straight",
            "garment_measurements": { "Waist": "90 cm", "Hips": "98 cm" }
        }),
    )
    .unwrap();

    // 3. Declarative StageTransitionRule on Body-Tempering Track (Layer 2) + D&D ASI Ladder Rule
    execute_tool(
        &engine,
        "progression",
        json!({ "op": "define_ladder", "preset": "body_tempering", "ladder_id": "still-water" }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "progression",
        json!({
            "op": "enroll_track",
            "character_id": "lin-mo",
            "track_id": "still-water",
            "ladder_id": "still-water",
            "name": "Still Water Body Manual",
            "track_kind": "parallel_manual",
            "stage_rules": {
                "2": {
                    "appearance_stage": "Layer 2 — Supple Sinew",
                    "height": "176 cm",
                    "weight": "76 kg",
                    "facial_hair": "",
                    "measurements": {
                        "Shoulders": "46 cm",
                        "Chest": "104 cm",
                        "Waist": "78 cm",
                        "Hips": "106 cm"
                    },
                    "stat_deltas": { "Strength": -2, "Dexterity": 2 },
                    "unspent_stat_points_delta": 1,
                    "upsert_conditions": [{
                        "name": "Softened Leverage",
                        "kind": "condition",
                        "severity": "minor",
                        "notes": "-2 [scope: strength, brawling] Reduced raw brute burst"
                    }],
                    "grant_traits": [{
                        "name": "Fluid Joint Alignment",
                        "category": "physique",
                        "effect": "+1 [scope: dexterity, carpentry] Smooth fine motor control"
                    }],
                    "checkpoint_summary": "Layer 2 shifted frame softer and wider at the hips; jaw stubble vanished.",
                    "narrative_directive": "Reflect softened shoulders, cleared facial hair, and binding trousers at the hips."
                }
            }
        }),
    )
    .unwrap();

    // Advance 45 XP (Layer 1 threshold is 40 XP) -> enters Layer 2 and executes StageTransitionRule atomically!
    let adv_res = execute_tool(
        &engine,
        "progression",
        json!({
            "op": "advance_xp",
            "character_id": "lin-mo",
            "track_id": "still-water",
            "xp": 45,
            "reason": "Evening meridian circulation"
        }),
    )
    .unwrap();
    assert_eq!(adv_res["stages_advanced"], 1);
    assert_eq!(adv_res["stage_rules_applied"].as_array().unwrap().len(), 1);

    let prof_after = engine.get_character_profile("lin-mo").unwrap();
    assert_eq!(prof_after.avatar.baseline_height, "178 cm", "Day-1 baseline height auto-preserved!");
    assert_eq!(prof_after.avatar.height, "176 cm");
    assert_eq!(prof_after.avatar.facial_hair, "", "Facial hair explicitly cleared!");
    assert_eq!(prof_after.avatar.baseline_measurements.get("Hips").map(String::as_str), Some("96 cm"));
    assert_eq!(prof_after.avatar.measurements.get("Hips").map(String::as_str), Some("106 cm"));
    assert_eq!(prof_after.unspent_stat_points, 1);
    assert_eq!(prof_after.avatar.transformation_log.len(), 1);

    let proj_after = engine.get_character("lin-mo", None).unwrap();
    assert_eq!(proj_after.current.stats.get("Strength").copied(), Some(12));
    assert_eq!(proj_after.current.stats.get("Dexterity").copied(), Some(14));
    assert!(proj_after.current.conditions.iter().any(|c| c.name == "Softened Leverage"));
    assert!(
        proj_after
            .current
            .conditions
            .iter()
            .any(|c| c.name == "Apparel Discomfort: Binding & Chafing")
    );

    // Also verify D&D 1-20 preset ladder-scoped ASI stage_rule at Level 4 (+2 unspent_stat_points)
    execute_tool(
        &engine,
        "progression",
        json!({ "op": "define_ladder", "preset": "dnd_level_20", "ladder_id": "dnd-lvl" }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "progression",
        json!({
            "op": "enroll_track",
            "character_id": "lin-mo",
            "track_id": "class-level",
            "ladder_id": "dnd-lvl",
            "name": "Artificer Level",
            "track_kind": "level"
        }),
    )
    .unwrap();
    // Level 1 (300) + Level 2 (600) + Level 3 (1800) = 2700 XP -> reaches Level 4!
    execute_tool(
        &engine,
        "progression",
        json!({ "op": "advance_xp", "character_id": "lin-mo", "track_id": "class-level", "xp": 2700 }),
    )
    .unwrap();
    let prof_asi = engine.get_character_profile("lin-mo").unwrap();
    assert_eq!(prof_asi.unspent_stat_points, 3, "1 from manual + 2 from D&D Level 4 ASI!");

    // 4. Full Check Hydration (Title + Trait + Proficiency + Injury + Encumbrance + Apparel) + Coupled XP Trickle
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "pc_sheet",
            "sub_action": "upsert_title",
            "character_id": "lin-mo",
            "name": "Steady Joiner",
            "equipped": true,
            "stat_modifiers": { "Dexterity": 1 }
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "pc_sheet",
            "sub_action": "upsert_proficiency",
            "character_id": "lin-mo",
            "name": "Mortise Chisel",
            "category": "tool",
            "rank": "Proficient",
            "bonus": "+2"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "pc_sheet",
            "sub_action": "upsert_job",
            "character_id": "lin-mo",
            "name": "Carpentry Apprentice",
            "role": "occupation",
            "governing_stats": ["Dexterity"]
        }),
    )
    .unwrap();
    // Add an injury directly in DB (without duplicate StatusCondition) & heavy timber cargo to verify table hydration
    engine
        .db()
        .upsert_injury(&chronicle::world_sim::InjuryRecord {
            id: "sprained-wrist".into(),
            character_id: "lin-mo".into(),
            name: "Sprained Wrist".into(),
            severity: "moderate".into(),
            body_location: "wrist".into(),
            complications: vec![],
            treated: false,
            recovery_days_total: 5,
            recovery_days_remaining: 5,
            status: "active".into(),
            source_event_id: None,
            notes: String::new(),
            inflicted_at_world_time: String::new(),
            updated_at_world_time: String::new(),
        })
        .unwrap();
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "equipment",
            "sub_action": "acquire",
            "character_id": "lin-mo",
            "item_name": "Oak Timber Beam",
            "quantity": 5
        }),
    )
    .unwrap();

    let chk = execute_tool(
        &engine,
        "resolve",
        json!({
            "op": "check",
            "actor_id": "lin-mo",
            "action": "Carve fine wrist-angled mortise chisel joint while sprint-hauling",
            "skill": "Carpentry",
            "dc_tier": "moderate",
            "dc_factors": ["proper_tools"],
            "seed": 77
        }),
    )
    .unwrap();
    let mod_sources: Vec<String> = chk["modifiers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["source"].as_str().unwrap().to_string())
        .collect();
    assert!(mod_sources.iter().any(|s| s == "stat:Dexterity"));
    assert!(mod_sources.iter().any(|s| s == "skill:Carpentry"));
    assert!(mod_sources.iter().any(|s| s == "title:Steady Joiner"));
    assert!(mod_sources.iter().any(|s| s == "trait:Fluid Joint Alignment"));
    assert!(mod_sources.iter().any(|s| s == "proficiency:Mortise Chisel"));
    assert!(mod_sources.iter().any(|s| s == "injury:Sprained Wrist"));
    assert!(mod_sources.iter().any(|s| s.starts_with("encumbrance:")));
    assert!(mod_sources.iter().any(|s| s == "apparel:fit_movement"));
    // Verify coupled Stat Training XP & Job XP trickled automatically!
    assert!(chk.get("stat_xp_gained").is_some(), "Governing Stat Training XP must trickle!");
    assert!(!chk["job_xp_gained"].as_array().unwrap().is_empty(), "Matching active Job XP must trickle!");

    // 5. Multi-Denomination Currency Auto-Change-Making + Cross-Family Guard
    execute_tool(
        &engine,
        "economy",
        json!({ "op": "set_balance", "owner_id": "lin-mo", "currency": "silver_tael", "amount": 1, "notes": "Savings" }),
    )
    .unwrap();
    let tx = execute_tool(
        &engine,
        "economy",
        json!({
            "op": "execute_transaction",
            "kind": "purchase",
            "buyer_id": "lin-mo",
            "seller_id": "tea-stall",
            "amount": 40,
            "currency": "copper_cash",
            "item_name": "Hot Ginger Tea",
            "notes": "Morning tea"
        }),
    )
    .unwrap();
    assert_eq!(tx["buyer_balance"], 960, "1 silver_tael (1000) - 40 copper_cash = 960 copper_cash change!");
    assert_eq!(engine.db().get_currency_balance("lin-mo", "silver_tael").unwrap(), 0);
    // Cross-family guard: D&D `gp` must NEVER convert from Xianxia `copper_cash`!
    let cross_err = execute_tool(
        &engine,
        "economy",
        json!({
            "op": "execute_transaction",
            "kind": "purchase",
            "buyer_id": "lin-mo",
            "seller_id": "foreign-mage",
            "amount": 1,
            "currency": "gp"
        }),
    );
    assert!(cross_err.is_err(), "Cross-family currency conversion must be rejected!");

    // 6. Auto-Ticking ProgressClock + Standing CharacterRoutine + Epistemic Signal Isolation
    execute_tool(
        &engine,
        "world",
        json!({
            "op": "clock",
            "sub_action": "create",
            "clock_id": "monthly-rent",
            "name": "Workshop Rent Countdown",
            "kind": "upkeep",
            "segments": 3,
            "filled": 0,
            "cadence_hours": 24,
            "auto_reset": true,
            "consequence_on_fill": "Pay 120 copper_cash rent to Landlord Zhao",
            "related_ids": ["lin-mo", "workshop"]
        }),
    )
    .unwrap();

    execute_tool(
        &engine,
        "character",
        json!({
            "op": "routine",
            "sub_action": "upsert",
            "routine": {
                "id": "daily-benchwork",
                "character_id": "lin-mo",
                "name": "Daily Joinery & Breathing",
                "category": "work",
                "cadence_hours": 24,
                "require_location_id": "workshop",
                "halt_on_injury_severity": "severe",
                "progression_xp": { "still-water": 10 },
                "skill_xp": { "Carpentry": 15 },
                "currency_deltas": { "copper_cash": 25 }
            }
        }),
    )
    .unwrap();

    // Advance 3 days (4320 min) in a single deferred-compile commit_turn!
    let turn = execute_tool(
        &engine,
        "commit_turn",
        json!({
            "advance_minutes": 4320,
            "time_reason": "Three days of steady joinery",
            "log": { "title": "Three Days at the Bench", "details": "Worked through three quiet days." }
        }),
    )
    .unwrap();
    assert!(!turn["signals"].as_array().unwrap().is_empty(), "commit_turn must return active signals!");
    let rent_clock = engine.db().get_clock("monthly-rent").unwrap().unwrap();
    assert_eq!(rent_clock.filled, 0, "Auto-reset clock should wrap 3/3 back to 0/3!");
    assert_eq!(engine.db().get_currency_balance("lin-mo", "copper_cash").unwrap(), 960 + 75);

    // GM Scene Context shows CRITICAL CLOCK_FILLED (gm_secret) AND OUTGROWN_GARMENT / STAGE_RULE_APPLIED at the very top!
    let gm_md = execute_tool(
        &engine,
        "scene",
        json!({ "op": "context", "place_id": "workshop", "character_ids": ["lin-mo"], "perspective": "gm" }),
    )
    .unwrap();
    let gm_str = gm_md.as_str().unwrap();
    assert!(gm_str.contains("## Engine Alarms & Active Directives"));
    assert!(gm_str.contains("CLOCK_FILLED"));
    assert!(gm_str.contains("OUTGROWN_GARMENT"));
    assert!(gm_str.contains("STAGE_RULE_APPLIED"));

    // Player POV Context strictly hides gm_secret CLOCK_FILLED while preserving player/public alarms!
    let player_md = execute_tool(
        &engine,
        "scene",
        json!({ "op": "context", "place_id": "workshop", "character_ids": ["lin-mo"], "perspective": "player" }),
    )
    .unwrap();
    let player_str = player_md.as_str().unwrap();
    assert!(!player_str.contains("CLOCK_FILLED"), "gm_secret signal must never leak to player POV!");
    assert!(player_str.contains("OUTGROWN_GARMENT"));

    // Move Lin Mo away from `workshop` and advance 1 day -> routine auto-pauses and emits ROUTINE_PAUSED!
    execute_tool(
        &engine,
        "character",
        json!({ "op": "move", "character_id": "lin-mo", "to_place_id": "market-ward" }),
    )
    .unwrap();
    execute_tool(&engine, "world", json!({ "op": "advance_time", "minutes": 1440 })).unwrap();
    let sigs_after_move = execute_tool(&engine, "scene", json!({ "op": "signals", "perspective": "player" })).unwrap();
    assert!(
        sigs_after_move["signals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["code"] == "ROUTINE_PAUSED"),
        "Leaving required location must pause routine and raise ROUTINE_PAUSED signal!"
    );

    // Acknowledge/resolve a live-derived signal (OUTGROWN_GARMENT) and verify it suppresses cleanly!
    let outgrown_id = sigs_after_move["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["code"] == "OUTGROWN_GARMENT")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    execute_tool(&engine, "scene", json!({ "op": "resolve_signal", "signal_id": outgrown_id })).unwrap();
    let sigs_after_resolve = execute_tool(&engine, "scene", json!({ "op": "signals", "perspective": "player" })).unwrap();
    assert!(
        !sigs_after_resolve["signals"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["code"] == "OUTGROWN_GARMENT"),
        "Resolved live signal must stay suppressed!"
    );

    // Verify REST /api/signals and /api/routines endpoints & SHA-256 chain integrity
    let (st_sig, _, sig_body) = dispatch_http(&engine, "GET", "/api/signals", b"");
    assert_eq!(st_sig, 200);
    assert!(serde_json::from_slice::<serde_json::Value>(&sig_body).unwrap()["count"].as_u64().unwrap() >= 1);

    let (st_rt, _, rt_body) = dispatch_http(&engine, "GET", "/api/routines?character_id=lin-mo", b"");
    assert_eq!(st_rt, 200);
    assert_eq!(serde_json::from_slice::<serde_json::Value>(&rt_body).unwrap()["count"], 1);

    assert!(engine.db().verify_event_chain_detailed().unwrap().valid);
}

#[test]
fn test_inventory_tag_system_auto_tagging_filtering_catalog_sync_and_grouped_rendering() {
    let tmp = TempDir::new().unwrap();
    let engine = CampaignEngine::open(tmp.path()).unwrap();
    engine
        .init_campaign("Tag System Test", "Inventory tagging & retrieval", "d20", None)
        .unwrap();

    execute_tool(
        &engine,
        "world",
        json!({ "op": "create_place", "id": "inn", "name": "Wayfarer Inn", "terrain": "Urban" }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "character",
        json!({ "op": "create", "id": "hero", "name": "Shen Li", "kind": "pc", "initial_location": "inn" }),
    )
    .unwrap();

    // 1. Auto-inference on acquire + catalog item sync
    for (name, slot, equipped, extra) in [
        ("Wool Shoulder Cloak", Some("Outerwear"), true, json!({})),
        ("Iron Hatchet", None, false, json!({})),
        ("Flint-and-Steel Fire Kit", None, false, json!({})),
        ("Guild Charter Letter", None, false, json!({})),
        ("Simple Practice Jade Disk", None, false, json!({})),
        ("Canvas Bedroll", None, false, json!({})),
        ("Spare Linen Tunic", Some("Shirt"), false, json!({ "garment_cut": "standard", "garment_measurements": { "Chest": "104 cm" } })),
        ("Travel Provisions", None, false, json!({})),
        ("Copper Coin Pouch", None, false, json!({})),
    ] {
        let mut payload = json!({
            "op": "equipment",
            "sub_action": "acquire",
            "character_id": "hero",
            "name": name,
            "equipped": equipped
        });
        if let Some(sl) = slot {
            payload["slot"] = json!(sl);
        }
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                payload[k] = v.clone();
            }
        }
        execute_tool(&engine, "character", payload).unwrap();
    }

    // 2. Single-item tag add/remove + batch tag + catalog item tag mirroring
    execute_tool(
        &engine,
        "economy",
        json!({
            "op": "upsert_item",
            "id": "iron-hatchet",
            "name": "Iron Hatchet",
            "item_type": "tool",
            "current_holder_id": "hero"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "equipment",
            "sub_action": "tag",
            "character_id": "hero",
            "name": "Wool Shoulder Cloak",
            "add_tags": ["Winter Gear", "Travel"]
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "equipment",
            "sub_action": "tag",
            "character_id": "hero",
            "items": [
                { "name": "Iron Hatchet", "add_tags": ["craft", "woodworking"] },
                { "name": "Spare Linen Tunic", "add_tags": ["workwear"], "remove_tags": ["spare"] }
            ]
        }),
    )
    .unwrap();

    // Verify catalog item mirrored the new tags from inventory!
    let cat_hatchet = engine.get_catalog_item("iron-hatchet").unwrap();
    assert!(cat_hatchet.tags.contains(&"craft".to_string()));
    assert!(cat_hatchet.tags.contains(&"woodworking".to_string()));

    // Verify catalog-side tag_item mirrors back into inventory!
    execute_tool(
        &engine,
        "economy",
        json!({
            "op": "tag_item",
            "item_id": "iron-hatchet",
            "add_tags": ["heirloom"],
            "remove_tags": ["woodworking"]
        }),
    )
    .unwrap();

    // 3. Multi-tag inventory retrieval (`all` vs `any`, `exclude_tags`, `tag_facets`, `group_counts`)
    let and_res = execute_tool(
        &engine,
        "query",
        json!({
            "op": "search_inventory",
            "character_id": "hero",
            "tags": ["tool", "heirloom"],
            "tag_match": "all"
        }),
    )
    .unwrap();
    assert_eq!(and_res["count"], 1);
    assert_eq!(and_res["items"][0]["item"]["name"], "Iron Hatchet");
    assert!(!and_res["items"][0]["item"]["tags"].as_array().unwrap().iter().any(|t| t == "woodworking"));

    let or_res = execute_tool(
        &engine,
        "query",
        json!({
            "op": "search_inventory",
            "character_id": "hero",
            "tags": ["cultivation", "document"],
            "tag_match": "any"
        }),
    )
    .unwrap();
    assert_eq!(or_res["count"], 2);

    let excl_res = execute_tool(
        &engine,
        "character",
        json!({
            "op": "equipment",
            "sub_action": "search",
            "character_id": "hero",
            "exclude_tags": ["clothing"]
        }),
    )
    .unwrap();
    assert_eq!(excl_res["count"], 7);
    assert!(excl_res["tag_facets"]["clothing"].as_u64().unwrap() == 2);

    // 4. Verify REST /api/items?tags=... & /api/inventory?tags=...
    let (st_i, _, body_i) = dispatch_http(&engine, "GET", "/api/items?tags=tool,heirloom&tag_match=all", b"");
    assert_eq!(st_i, 200);
    let j_i: serde_json::Value = serde_json::from_slice(&body_i).unwrap();
    assert_eq!(j_i["count"], 1);
    assert!(j_i["tag_facets"]["heirloom"].as_u64().unwrap() >= 1);

    let (st_inv, _, body_inv) = dispatch_http(&engine, "GET", "/api/inventory?character_id=hero&tags=winter-gear", b"");
    assert_eq!(st_inv, 200);
    let j_inv: serde_json::Value = serde_json::from_slice(&body_inv).unwrap();
    assert_eq!(j_inv["count"], 1);
    assert_eq!(j_inv["items"][0]["item"]["name"], "Wool Shoulder Cloak");

    // 5. Verify grouped Markdown docs & tag-grouped Scene Context brief (>6 carried items)
    let char_md = std::fs::read_to_string(engine.docs_dir().join("characters").join("hero.md")).unwrap();
    assert!(char_md.contains("> **Inventory Tags:**"));
    assert!(char_md.contains("#### Tools & Craft"));
    assert!(char_md.contains("#### Clothing & Apparel"));

    let scene_md = execute_tool(
        &engine,
        "scene",
        json!({ "op": "context", "place_id": "inn", "character_ids": ["hero"], "perspective": "player" }),
    )
    .unwrap();
    assert!(scene_md.as_str().unwrap().contains("- **Carried (by tag):**"));
    assert!(engine.db().verify_event_chain_detailed().unwrap().valid);
}

#[test]
fn test_covert_recurring_figure_map_roaming_conveyance_and_gm_convergence_radar() {
    let tmp = TempDir::new().unwrap();
    let engine = CampaignEngine::open(tmp.path()).unwrap();
    engine
        .init_campaign("Covert Patron Test", "Regional intrigue", "d20", None)
        .unwrap();

    // Create places (note: deliberately NO regional route between capital and frontier-county to test speculative/unrouted map roaming!)
    for (id, name, ptype) in [
        ("capital-city", "Capital City", "settlement"),
        ("patron-estate", "Patron Estate", "establishment"),
        ("frontier-county", "Frontier County", "settlement"),
        ("frontier-inn", "Frontier Market Inn", "establishment"),
    ] {
        execute_tool(
            &engine,
            "world",
            json!({ "op": "create_place", "id": id, "name": name, "place_type": ptype, "terrain": "Urban" }),
        )
        .unwrap();
    }
    // Local settlement links only
    execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({ "action": "upsert_route", "id": "local-cap-estate", "from_place_id": "capital-city", "to_place_id": "patron-estate", "distance_km": 1.0, "terrain": "road" }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({ "action": "upsert_route", "id": "local-front-inn", "from_place_id": "frontier-county", "to_place_id": "frontier-inn", "distance_km": 0.6, "terrain": "road" }),
    )
    .unwrap();

    // Regional 5 km/cell map grid with wilderness forest cells near Frontier County
    execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "upsert_grid",
            "grid_id": "heartland-map",
            "name": "Heartland Regional Map",
            "scale_kind": "region",
            "width": 24,
            "height": 20,
            "cell_scale_label": "5 km",
            "default_terrain": "wilderness"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "manage_travel_and_exploration",
        json!({
            "action": "set_cell",
            "grid_id": "heartland-map",
            "cells": [
                { "x": 18, "y": 14, "terrain": "urban", "place_id": "capital-city", "label": "Capital City" },
                { "x": 8, "y": 6, "terrain": "urban", "place_id": "frontier-county", "label": "Frontier County" },
                { "x": 10, "y": 6, "terrain": "forest", "label": "Eastern Foothill Woods" },
                { "x": 9, "y": 7, "terrain": "mountain", "label": "Mist Ridge" }
            ]
        }),
    )
    .unwrap();

    // PC in Frontier Market Inn (inside Frontier County), Covert Patron at Patron Estate (inside Capital City, ~64 km away)
    execute_tool(
        &engine,
        "character",
        json!({ "op": "create", "id": "pc-yan", "name": "Yan", "kind": "pc", "initial_location": "frontier-inn" }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "character",
        json!({ "op": "create", "id": "lady-bai", "name": "Lady Bai", "kind": "npc", "initial_location": "patron-estate" }),
    )
    .unwrap();

    // Configure Lady Bai as a GM-Secret Patron with a data-driven carried conveyance, map-aware wild roaming, and GM convergence radar
    let cfg = execute_tool(
        &engine,
        "society",
        json!({
            "op": "autonomy",
            "sub_action": "configure_mobility",
            "character_id": "lady-bai",
            "prominent": true,
            "mobility_mode": "wanderer",
            "narrative_role": "patron",
            "visibility": "gm_secret",
            "roam_strategy": "map_roam",
            "home_place_id": "patron-estate",
            "roam_grid_id": "heartland-map",
            "roam_terrains": ["forest", "mountain"],
            "wild_roam_chance": 100,
            "pc_proximity_bias": 100,
            "gm_convergence_alert": true,
            "convergence_radius_km": 25.0,
            "incognito_alias": "Veiled Traveling Herbalist",
            "conveyance": {
                "label": "Curtained lacquered sedan chair carried by four silent bearers",
                "speed_kmh": 12.0,
                "offroad_capable": true,
                "signature": "discreet"
            },
            "purposes": ["rare_herb_foraging", "rare_item_scouting"],
            "trip_cadence_days": 2,
            "stay_duration_days": 2
        }),
    )
    .unwrap();
    assert_eq!(cfg["mobility"]["narrative_role"], "patron");
    assert_eq!(cfg["mobility"]["visibility"], "gm_secret");
    assert_eq!(cfg["mobility"]["conveyance"]["speed_kmh"], 12.0);

    // Before moving, Lady Bai is ~64 km away at Capital City -> NO convergence alarm yet!
    let gm_far = execute_tool(
        &engine,
        "scene",
        json!({ "op": "context", "place_id": "frontier-inn", "character_ids": ["pc-yan"], "perspective": "gm", "include_gm_secrets": true, "format": "json" }),
    )
    .unwrap();
    assert!(
        !gm_far["active_signals"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .any(|s| s["code"] == "NPC_CONVERGENCE_OPPORTUNITY"),
        "Patron >60 km away at home should NOT fire convergence radar!"
    );

    // Advance 36 hours -> Lady Bai departs covertly and halts her sedan in the wild forest/mountain cells 5-10 km from Frontier County!
    let tick = execute_tool(
        &engine,
        "advance_world",
        json!({ "duration_hours": 36, "tick_npc_movements": true, "seed": 77 }),
    )
    .unwrap();
    assert!(tick["npc_movements"]["events_count"].as_u64().unwrap() >= 1);

    let bai_prof = engine.get_character_profile("lady-bai").unwrap();
    assert_eq!(bai_prof.mobility.current_status, "roaming_wild");
    let gp = bai_prof
        .mobility
        .current_grid_pos
        .as_ref()
        .expect("Wild map roam must record exact regional grid position");
    assert_eq!(gp.grid_id, "heartland-map");
    assert!(
        (gp.x == 10 && gp.y == 6) || (gp.x == 9 && gp.y == 7),
        "Soft PC proximity bias should guide wilderness stop to cells near Frontier County, got ({},{})",
        gp.x,
        gp.y
    );

    // 1. GM POV: MUST receive `NPC_CONVERGENCE_OPPORTUNITY` signal with Patron role, conveyance, alias, and km distance, AND covert movement summary!
    let gm_near = execute_tool(
        &engine,
        "scene",
        json!({ "op": "context", "place_id": "frontier-inn", "character_ids": ["pc-yan"], "perspective": "gm", "include_gm_secrets": true, "format": "json" }),
    )
    .unwrap();
    let conv_sig = gm_near["active_signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["code"] == "NPC_CONVERGENCE_OPPORTUNITY")
        .expect("GM POV MUST see NPC_CONVERGENCE_OPPORTUNITY when covert Patron halts <=25 km from PC!");
    assert_eq!(conv_sig["visibility"], "gm_secret");
    assert!(conv_sig["title"].as_str().unwrap().contains("[PATRON]"));
    assert!(conv_sig["summary"]
        .as_str()
        .unwrap()
        .contains("Curtained lacquered sedan chair"));
    assert!(conv_sig["summary"]
        .as_str()
        .unwrap()
        .contains("Veiled Traveling Herbalist"));
    assert!(
        !gm_near["prominent_figure_movements"].as_array().unwrap().is_empty(),
        "GM POV should see covert roaming figure movements"
    );

    // 2. Player POV: MUST have ZERO leaks! No convergence signal, no covert movements, no covert ledger events!
    let player_near = execute_tool(
        &engine,
        "scene",
        json!({ "op": "context", "place_id": "frontier-inn", "character_ids": ["pc-yan"], "perspective": "player", "include_gm_secrets": true, "format": "json" }),
    )
    .unwrap();
    assert!(
        !player_near["active_signals"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .any(|s| s["code"] == "NPC_CONVERGENCE_OPPORTUNITY"),
        "Player POV must NEVER see GM-secret convergence signals!"
    );
    assert!(
        player_near["prominent_figure_movements"]
            .as_array()
            .unwrap_or(&vec![])
            .is_empty(),
        "Player POV must NEVER see gm_secret figure movements!"
    );
    for ev_line in player_near["recent_campaign_events"].as_array().unwrap_or(&vec![]) {
        let s = ev_line.as_str().unwrap_or("");
        assert!(
            !s.contains("departed")
                && !s.contains("halted")
                && !s.contains("sedan")
                && !s.contains("foraging")
                && !s.contains("Configured mobility"),
            "Covert Patron movement leaked into Player POV events: {s}"
        );
    }

    // 3. list_movements(include_gm_secrets=false) hides covert roamer; true shows her!
    let pub_list = execute_tool(
        &engine,
        "society",
        json!({ "op": "autonomy", "sub_action": "list_movements", "include_gm_secrets": false }),
    )
    .unwrap();
    assert!(
        !pub_list["figures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["character_id"] == "lady-bai"),
        "Covert roamer must be hidden from public list_movements!"
    );

    // 4. Also verify manual covert dispatch to an uncharted frontier (keeps map canon-honest without fabricating places!)
    let disp = execute_tool(
        &engine,
        "society",
        json!({
            "op": "autonomy",
            "sub_action": "dispatch_trip",
            "character_id": "lady-bai",
            "uncharted_label": "Unnamed Mist Valley Herb Slope",
            "purpose": "rare_herb_foraging"
        }),
    )
    .unwrap();
    assert_eq!(disp["visibility"], "gm_secret");
    assert_eq!(disp["trip"]["uncharted_label"], "Unnamed Mist Valley Herb Slope");

    // 5. Verify hard GM-secret firewall across get_lore, search_lore, get_text, search_all, get_character_progression & Markdown seals!
    execute_tool(
        &engine,
        "codex",
        json!({
            "op": "lore",
            "id": "secret-art-mechanism",
            "title": "Secret Art Mechanism",
            "category": "magic-system",
            "visibility": "gm_secret",
            "always_include": true,
            "summary": "Hidden mechanism",
            "body": "SECRET_SOUL_IMPRINT_AND_LAYER9_LOCK"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "codex",
        json!({
            "op": "text",
            "id": "secret-manual",
            "title": "Silent Basin Scripture",
            "read_status": "undiscovered",
            "summary": "Plain basin scripture",
            "content": "UNREAD_SCRIPTURE_BODY",
            "gm_notes": "SECRET_AUTHOR_CASE_NOTES"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "progression",
        json!({ "op": "define_ladder", "preset": "body_tempering", "ladder_id": "secret-ladder" }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "progression",
        json!({
            "op": "enroll_track",
            "character_id": "pc-yan",
            "track_id": "secret-art",
            "ladder_id": "secret-ladder",
            "name": "Secret Art",
            "stage_rules": {
                "1": { "narrative_directive": "Layer 1 observable breath shift" },
                "9": { "narrative_directive": "SECRET_LAYER9_HIDDEN_BLUEPRINT", "signal_visibility": "gm_secret" }
            }
        }),
    )
    .unwrap();

    // Player POV MUST be blocked/redacted across every read tool
    assert!(execute_tool(&engine, "get_lore", json!({ "lore_id": "secret-art-mechanism", "perspective": "player" })).is_err());
    let pub_lore = execute_tool(&engine, "search_lore", json!({ "query": "Secret", "include_gm_secrets": false })).unwrap();
    assert_eq!(pub_lore["count"], 0);
    let pub_text = execute_tool(&engine, "get_text", json!({ "text_id": "secret-manual", "perspective": "player" })).unwrap();
    assert_eq!(pub_text["text"]["gm_notes"], "");
    assert!(!pub_text["text"]["content"].as_str().unwrap().contains("UNREAD_SCRIPTURE_BODY"));
    let pub_all = execute_tool(&engine, "search_all", json!({ "query": "Secret", "perspective": "player" })).unwrap();
    assert!(pub_all["lore"].as_array().unwrap().is_empty());
    let pub_prog = execute_tool(&engine, "manage_progression", json!({ "action": "get_character_progression", "character_id": "pc-yan", "perspective": "player" })).unwrap();
    let pub_rules = pub_prog["tracks"][0]["stage_rules"].as_object().unwrap();
    assert!(pub_rules.contains_key("1"), "Current stage 1 rule may remain");
    assert!(!pub_rules.contains_key("9"), "Future unreached Layer 9 stage_rule must be redacted under player POV!");

    // GM POV receives `_gm_non_disclosure_seal` and Markdown hard non-disclosure banner + [GM SECRET] signal tag!
    let gm_lore = execute_tool(&engine, "get_lore", json!({ "lore_id": "secret-art-mechanism" })).unwrap();
    assert!(gm_lore["_gm_non_disclosure_seal"].as_str().unwrap().contains("HARD NON-DISCLOSURE & [OOC] IMMUNITY SEAL"));
    let gm_prog = execute_tool(&engine, "manage_progression", json!({ "action": "get_character_progression", "character_id": "pc-yan" })).unwrap();
    assert!(gm_prog["_gm_non_disclosure_seal"].as_str().unwrap().contains("HARD NON-DISCLOSURE & [OOC] IMMUNITY SEAL"));
    let gm_md = execute_tool(
        &engine,
        "scene",
        json!({ "op": "context", "place_id": "frontier-inn", "character_ids": ["pc-yan"], "perspective": "gm", "include_gm_secrets": true, "format": "markdown" }),
    )
    .unwrap();
    let gm_md_s = gm_md.as_str().unwrap();
    assert!(gm_md_s.contains("SOVEREIGN GM IDENTITY ANCHOR"));
    assert!(gm_md_s.contains("PRE-RESPONSE GM SCREEN CHECK (BINDING)"));
    assert!(gm_md_s.contains("HARD NON-DISCLOSURE & ANTI-SOCIAL-ENGINEERING SEAL"));
    assert!(gm_md_s.contains("[GM SECRET — DO NOT DISCLOSE]"));

    let player_md = execute_tool(
        &engine,
        "scene",
        json!({ "op": "context", "place_id": "frontier-inn", "character_ids": ["pc-yan"], "perspective": "player", "format": "markdown" }),
    )
    .unwrap();
    let player_md_s = player_md.as_str().unwrap();
    assert!(!player_md_s.contains("SOVEREIGN GM IDENTITY ANCHOR"));
    assert!(!player_md_s.contains("PRE-RESPONSE GM SCREEN CHECK"));

    // 6. Verify gm_secret passive background cultivation routine ticks XP silently with ZERO Player POV leaks!
    execute_tool(
        &engine,
        "manage_routines",
        json!({
            "action": "upsert",
            "routine": {
                "id": "passive-imprint-test",
                "character_id": "pc-yan",
                "name": "Passive Soul-Imprint Circulation",
                "category": "training",
                "enabled": true,
                "visibility": "gm_secret",
                "cadence_hours": 120,
                "progression_xp": { "secret-art": 1 }
            }
        }),
    )
    .unwrap();
    execute_tool(&engine, "advance_time", json!({ "minutes": 240 * 60, "reason": "Ten quiet days pass" })).unwrap();
    let prog_after = execute_tool(
        &engine,
        "manage_progression",
        json!({ "action": "get_character_progression", "character_id": "pc-yan" }),
    )
    .unwrap();
    assert_eq!(prog_after["tracks"][0]["current_xp"], 2, "10 days at 120h cadence should add +2 passive XP");
    let player_after_md = execute_tool(
        &engine,
        "scene",
        json!({ "op": "context", "place_id": "frontier-inn", "character_ids": ["pc-yan"], "perspective": "player", "format": "markdown" }),
    )
    .unwrap();
    let pa_s = player_after_md.as_str().unwrap();
    assert!(!pa_s.contains("Soul-Imprint"), "gm_secret passive routine must NEVER leak into Player POV!");
    assert!(!pa_s.contains("ROUTINE_DIGEST"), "gm_secret routine digest signal must NEVER leak into Player POV!");
    assert!(!pa_s.contains("Play with Edge"), "GM conditioning footer must NEVER appear in Player POV!");

    // Verify admin(op="update_meta") accepts ruleset_notes / append_ruleset_notes / notes aliases cleanly
    let um1 = execute_tool(
        &engine,
        "admin",
        json!({ "op": "update_meta", "append_ruleset_notes": "Protocol A: Always track camp rations.", "reason": "Add camp protocol" }),
    )
    .unwrap();
    assert!(um1["campaign"]["ruleset_notes"].as_str().unwrap().contains("Protocol A"));
    let um2 = execute_tool(
        &engine,
        "admin",
        json!({ "op": "update_meta", "notes": "Protocol B: Updated full ruleset.", "reason": "Alias check" }),
    )
    .unwrap();
    assert_eq!(um2["campaign"]["ruleset_notes"], "Protocol B: Updated full ruleset.");

    assert!(engine.db().verify_event_chain_detailed().unwrap().valid);
}

#[test]
fn test_strict_unknown_key_validation_help_introspection_and_first_second_order_cascades() {
    let tmp = TempDir::new().unwrap();
    let engine = CampaignEngine::open(tmp.path()).unwrap();
    engine
        .init_campaign("Cascade & Schema Test", "Strict MCP test", "d20", None)
        .unwrap();

    // 1. Strict Unknown-Key Rejection (self-correcting error listing unknown + accepted keys)
    let bad_meta = execute_tool(
        &engine,
        "admin",
        json!({ "op": "update_meta", "invented_protocol_key": "foo" }),
    );
    let err_msg = bad_meta.unwrap_err().to_string();
    assert!(
        err_msg.contains("Rejected unknown parameter(s) [invented_protocol_key]"),
        "Must strictly reject unknown key: {err_msg}"
    );
    assert!(
        err_msg.contains("Accepted parameters:") && err_msg.contains("ruleset_notes"),
        "Error must list exact accepted keys: {err_msg}"
    );

    let bad_sync = execute_tool(
        &engine,
        "character",
        json!({ "op": "sync_state", "character_id": "hero", "bogus_field": 123 }),
    );
    assert!(
        bad_sync
            .unwrap_err()
            .to_string()
            .contains("Rejected unknown parameter(s) [bogus_field]")
    );

    // 2. On-demand zero-bloat schema introspection (`op='help'` and `query(op='describe_op')`)
    let char_help = execute_tool(&engine, "character", json!({ "op": "help" })).unwrap();
    assert!(char_help["operation_groups"].is_object());
    assert!(char_help["automatic_cascades"].as_array().unwrap().len() >= 5);
    let eq_help = execute_tool(
        &engine,
        "character",
        json!({ "op": "help", "target_op": "equipment" }),
    )
    .unwrap();
    assert_eq!(eq_help["resolved_tool"], "update_character_equipment");
    assert!(eq_help["accepted_keys"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k == "weight_kg"));
    let query_desc = execute_tool(
        &engine,
        "query",
        json!({ "op": "describe_op", "tool": "update_campaign_meta" }),
    )
    .unwrap();
    assert_eq!(query_desc["resolved_tool"], "update_campaign_meta");

    // 3. Create 2D map grid + places using cohesive inline `create_place(grid_id, x, y)`
    execute_tool(
        &engine,
        "world",
        json!({
            "op": "travel",
            "sub_action": "upsert_grid",
            "grid_id": "valley-map",
            "name": "Valley Map",
            "scale_kind": "region",
            "width": 10,
            "height": 10,
            "cell_scale_label": "5 km",
            "default_terrain": "plains"
        }),
    )
    .unwrap();
    let cp1 = execute_tool(
        &engine,
        "world",
        json!({
            "op": "create_place",
            "id": "river-town",
            "name": "River Town",
            "place_type": "town",
            "terrain": "Urban",
            "grid_id": "valley-map",
            "x": 2,
            "y": 2,
            "fog_status": "charted"
        }),
    )
    .unwrap();
    assert!(cp1["_cascades"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c.as_str().unwrap().contains("valley-map")));
    execute_tool(
        &engine,
        "world",
        json!({
            "op": "create_place",
            "id": "mist-ruins",
            "name": "Mist Ruins",
            "place_type": "ruins",
            "terrain": "Forest",
            "grid_id": "valley-map",
            "x": 5,
            "y": 5,
            "fog_status": "unexplored"
        }),
    )
    .unwrap();

    // 4. Create PC + NPC + Faction + Route, verify control & route cascades
    execute_tool(
        &engine,
        "society",
        json!({
            "op": "faction",
            "sub_action": "upsert",
            "id": "iron-guild",
            "name": "Iron Guild",
            "faction_type": "merchant-guild"
        }),
    )
    .unwrap();
    let ctrl = execute_tool(
        &engine,
        "world",
        json!({
            "op": "update_place",
            "sub_action": "control",
            "place_id": "river-town",
            "faction": "Iron Guild",
            "danger_level": "Low"
        }),
    )
    .unwrap();
    assert!(ctrl["_cascades"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c.as_str().unwrap().contains("iron-guild")));
    let fac = engine.db().get_faction("iron-guild").unwrap().unwrap();
    assert!(fac.territory.contains(&"river-town".to_string()));
    let rels = engine.db().list_relations_for_entity("iron-guild").unwrap();
    assert!(rels
        .iter()
        .any(|r| r.relation_type == "controls" && r.target_id == "river-town"));

    // Sync travel route status when `update_place(route)` blocks it
    execute_tool(
        &engine,
        "world",
        json!({
            "op": "travel",
            "sub_action": "upsert_route",
            "id": "river-to-ruins",
            "from_place_id": "river-town",
            "to_place_id": "mist-ruins",
            "distance_km": 20.0,
            "terrain": "forest",
            "status": "open",
            "bidirectional": true
        }),
    )
    .unwrap();
    let rt_upd = execute_tool(
        &engine,
        "world",
        json!({
            "op": "update_place",
            "sub_action": "route",
            "place_id": "river-town",
            "destination_id": "mist-ruins",
            "route_status": "Blocked",
            "notes": "Flooded bridge"
        }),
    )
    .unwrap();
    assert!(rt_upd["_cascades"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c.as_str().unwrap().contains("travel_route")));
    assert_eq!(
        engine.db().get_travel_route("river-to-ruins").unwrap().unwrap().status,
        "blocked"
    );

    // 5. Create PC & NPC; test structured-field lint, atomic purchase cascade, avatar->garment fit cascade,
    //    outfit->equipment cascade, transfer->catalog cascade, PC move->fog chart cascade, and dead->routine/mobility halt!
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "create",
            "id": "pc-lin",
            "name": "Lin",
            "kind": "pc",
            "initial_location": "river-town",
            "initial_stats": { "STR": 10, "DEX": 12 }
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "create",
            "id": "npc-merchant",
            "name": "Old Wen",
            "kind": "npc",
            "initial_location": "river-town"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "pc_sheet",
            "sub_action": "update_avatar",
            "character_id": "pc-lin",
            "measurements": { "chest": "90 cm", "waist": "76 cm", "shoulders": "43 cm", "hips": "92 cm" }
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "economy",
        json!({ "op": "set_balance", "owner_id": "pc-lin", "currency": "copper_cash", "amount": 500 }),
    )
    .unwrap();

    // Unstructured-in-structured lint triggers when weight text is stuffed in description without weight_kg
    let lint_res = execute_tool(
        &engine,
        "character",
        json!({
            "op": "equipment",
            "sub_action": "acquire",
            "character_id": "pc-lin",
            "name": "Iron Hammer",
            "description": "Weight: 2.5 kg forged smithing hammer"
        }),
    )
    .unwrap();
    assert!(lint_res["_schema_warnings"].as_array().unwrap().len() >= 1);

    // ONE-CALL cohesive purchase: currency + delivered item + item_economics + CatalogItem + equip_on_buy + encumbrance!
    let tx = execute_tool(
        &engine,
        "economy",
        json!({
            "op": "execute_transaction",
            "kind": "purchase",
            "buyer_id": "pc-lin",
            "seller_id": "npc-merchant",
            "amount": 120,
            "currency": "copper_cash",
            "item_name": "Indigo Coat",
            "quantity": 1,
            "weight_kg": 1.8,
            "slot": "outerwear",
            "tags": ["clothing", "travel"],
            "garment_cut": "fitted",
            "garment_measurements": { "chest": "95 cm", "waist": "82 cm", "shoulders": "45 cm" },
            "equip_on_buy": true,
            "catalog_item": true,
            "item_type": "clothing"
        }),
    )
    .unwrap();
    assert!(tx["encumbrance"]["total_weight_kg"].as_f64().unwrap() >= 1.8);
    let econ = engine.db().list_item_economics_map().unwrap();
    assert_eq!(econ["indigo coat"].weight_kg, 1.8);
    assert_eq!(econ["indigo coat"].unit_value, 120);
    assert!(engine.db().get_catalog_item("indigo-coat").unwrap().is_some());

    // Avatar update cascade: chest grows to 102 cm -> fitted 95 cm Indigo Coat immediately flags garment fit alert!
    let av_grow = execute_tool(
        &engine,
        "character",
        json!({
            "op": "pc_sheet",
            "sub_action": "update_avatar",
            "character_id": "pc-lin",
            "measurements": { "chest": "102 cm" }
        }),
    )
    .unwrap();
    assert!(av_grow["garment_fit_reports"].as_array().unwrap().len() >= 1);
    assert!(av_grow["_cascades"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c.as_str().unwrap().contains("Garment fit cascade")));

    // Outfit cascade: create two outfits + set_active_outfit auto-equips/unequips!
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "equipment",
            "sub_action": "acquire",
            "character_id": "pc-lin",
            "name": "Loose Linen Robe",
            "slot": "outerwear",
            "equipped": false,
            "weight_kg": 0.9
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "pc_sheet",
            "sub_action": "upsert_outfit",
            "character_id": "pc-lin",
            "id": "travel-kit",
            "name": "Travel Kit",
            "outer_layer": "Indigo Coat"
        }),
    )
    .unwrap();
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "pc_sheet",
            "sub_action": "upsert_outfit",
            "character_id": "pc-lin",
            "id": "summer-robe",
            "name": "Summer Robe",
            "outer_layer": "Loose Linen Robe"
        }),
    )
    .unwrap();
    let sw = execute_tool(
        &engine,
        "character",
        json!({
            "op": "pc_sheet",
            "sub_action": "set_active_outfit",
            "character_id": "pc-lin",
            "outfit_id": "summer-robe"
        }),
    )
    .unwrap();
    assert!(sw["_cascades"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c.as_str().unwrap().contains("Auto-equipped outfit items")));
    let lin_proj = engine.get_character("pc-lin", None).unwrap();
    assert!(lin_proj.current.equipment.iter().find(|i| i.name == "Loose Linen Robe").unwrap().equipped);
    assert!(!lin_proj.current.equipment.iter().find(|i| i.name == "Indigo Coat").unwrap().equipped);

    // Transfer item cascade: transferring Indigo Coat auto-transfers CatalogItem AND prunes Travel Kit outfit!
    let tr = execute_tool(
        &engine,
        "character",
        json!({
            "op": "transfer_item",
            "from_character_id": "pc-lin",
            "to_character_id": "npc-merchant",
            "item_name": "Indigo Coat",
            "quantity": 1,
            "reason": "Returned for alteration"
        }),
    )
    .unwrap();
    assert!(tr["_cascades"].as_array().unwrap().len() >= 2);
    assert_eq!(
        engine.db().get_catalog_item("indigo-coat").unwrap().unwrap().current_holder_id.as_deref(),
        Some("npc-merchant")
    );
    let lin_prof = engine.get_character_profile("pc-lin").unwrap();
    assert!(lin_prof.avatar.outfits.iter().find(|o| o.id == "travel-kit").unwrap().outer_layer.is_empty());

    // Cohesive `character(op='sync_state')` moves PC to unexplored `mist-ruins` + updates stat + adds condition in ONE call
    // AND auto-charts the destination 2D map cell!
    let synced = execute_tool(
        &engine,
        "character",
        json!({
            "op": "sync_state",
            "character_id": "pc-lin",
            "location_id": "mist-ruins",
            "stats": { "DEX": 13 },
            "add_conditions": [{ "name": "Damp Chill", "kind": "debuff", "severity": "mild" }],
            "reason": "Entered misty ruins"
        }),
    )
    .unwrap();
    assert!(synced["_cascades"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c.as_str().unwrap().contains("Auto-charted map cell valley-map (5,5)")));
    let cells = engine.db().list_map_cells("valley-map").unwrap();
    assert_eq!(
        cells.iter().find(|c| c.x == 5 && c.y == 5).unwrap().fog_status,
        "charted"
    );

    // Dead status cascade: standing routine auto-paused when NPC dies!
    execute_tool(
        &engine,
        "character",
        json!({
            "op": "routine",
            "sub_action": "upsert",
            "character_id": "npc-merchant",
            "routine": {
                "id": "wen-shopkeeping",
                "name": "Daily Shopkeeping",
                "category": "work",
                "cadence_hours": 24,
                "enabled": true
            }
        }),
    )
    .unwrap();
    let dead = execute_tool(
        &engine,
        "character",
        json!({
            "op": "status",
            "character_id": "npc-merchant",
            "alive_status": "dead",
            "reason": "Bandit raid"
        }),
    )
    .unwrap();
    assert!(dead["_cascades"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c.as_str().unwrap().contains("Auto-paused 1 standing routine")));
    assert!(!engine.db().get_routine("wen-shopkeeping").unwrap().unwrap().enabled);

    assert!(engine.db().verify_event_chain_detailed().unwrap().valid);
}
