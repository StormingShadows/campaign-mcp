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
            }],
            initial_skills: vec![Skill {
                name: "Tracking".into(),
                rank: "Adept".into(),
                category: "Survival".into(),
                modifier: Some("+3".into()),
                description: "Follow faint trails.".into(),
            }],
            initial_techniques: vec![Technique {
                name: "Ash Step".into(),
                tier: "Tier I".into(),
                cost: "6 Qi".into(),
                cooldown: "1 turn".into(),
                mastery: "Practiced".into(),
                effect: "Quick dash.".into(),
                description: String::new(),
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
}
