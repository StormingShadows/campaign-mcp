use crate::autonomy::{
    ActorBelief, AutonomyCommitRequest, FactionMember, FactionRecord, KnowledgeClaim,
    KnowledgeSearchFilter, RelationshipDimensions,
};
use crate::context::{render_scene_context_markdown, SceneContextRequest};
use crate::engine::{CampaignEngine, ImportAttachmentRequest};
use crate::maintenance::{
    backup_campaign, cleanup_campaign, delete_entity, diagnose_campaign, list_backups,
    reclassify_entity, rename_or_merge_entity, restore_backup, update_attachment_metadata,
    update_campaign_meta, update_character_baseline, update_clock_fields, update_entity_history,
    update_place_baseline, DeleteEntityRequest, ReclassifyRequest, RenameOrMergeRequest,
    UpdateAttachmentRequest, UpdateCampaignMetaRequest, UpdateCharacterBaselineRequest,
    UpdateClockRequest, UpdateEntityHistoryRequest, UpdatePlaceBaselineRequest,
};
use crate::mechanics::{
    parse_modifier_sources, CheckRequest, ContestParticipantInput, ContestRequest, DrawContext,
    DrawTableRequest, EncounterParticipant, EventCausalMeta, RandomTable, RulesetProfile,
};
use crate::models::{
    slugify, CatalogItem, CharacterBaseline, EntityHistoryEntry, EventPayload, GraphRelation, Item,
    Landmark, LoreEntry, PlaceBaseline, ProgressClock, Route, Skill, StatusCondition, StoryText,
    Technique,
};
use crate::search::{
    character_to_hit, place_to_hit, AttachmentSearchFilter, CharacterSearchFilter,
    EventSearchFilter, HistorySearchFilter, ItemSearchFilter, LoreSearchFilter, PlaceSearchFilter,
    RelationSearchFilter, TextSearchFilter,
};
use crate::world_sim::{
    CraftingProject, CrimeRecord, InjuryRecord, ItemEconomics, JurisdictionRecord, PlaceSpatialExt,
    SettlementMarket, TravelRoute,
};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

const SERVER_NAME: &str = "chronicle-rpg-gm";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

/// Runs the MCP JSON-RPC 2.0 server over stdio (newline-delimited JSON).
/// All diagnostic logging goes strictly to stderr so stdout stays clean for MCP frames.
pub fn run_mcp_stdio(engine: &CampaignEngine) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    eprintln!(
        "[{SERVER_NAME}] MCP stdio server started for campaign: {}",
        engine.root_dir().display()
    );

    for line_res in stdin.lock().lines() {
        let line = match line_res {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[{SERVER_NAME}] stdin read error: {e}");
                break;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let req: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let err_resp = JsonRpcResponse {
                    jsonrpc: "2.0",
                    id: Value::Null,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700,
                        message: format!("Parse error: {e}"),
                    }),
                };
                write_response(&mut stdout, &err_resp)?;
                continue;
            }
        };

        // JSON-RPC notifications have no `id` and must not receive a response.
        let Some(id) = req.id.clone() else {
            continue;
        };

        let resp = handle_request(engine, id, &req.method, req.params, None, true);
        write_response(&mut stdout, &resp)?;
    }

    Ok(())
}

/// Processes a single JSON-RPC 2.0 MCP message (request or notification) and returns `Some(response)`
/// for requests or `None` for notifications. Used by Streamable HTTP (`POST /mcp`) and embedded callers.
pub fn handle_mcp_jsonrpc_message(
    engine: &CampaignEngine,
    raw: Value,
    base_url: Option<&str>,
) -> Option<Value> {
    handle_mcp_jsonrpc_message_with_policy(engine, raw, base_url, true)
}

/// Same as `handle_mcp_jsonrpc_message`, with an explicit policy toggle controlling whether
/// `import_attachment(source_path)` is allowed to read arbitrary local host files.
pub fn handle_mcp_jsonrpc_message_with_policy(
    engine: &CampaignEngine,
    raw: Value,
    base_url: Option<&str>,
    allow_local_file_import: bool,
) -> Option<Value> {
    let req: JsonRpcRequest = match serde_json::from_value(raw) {
        Ok(r) => r,
        Err(e) => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": Value::Null,
                "error": {
                    "code": -32600,
                    "message": format!("Invalid JSON-RPC Request: {e}")
                }
            }));
        }
    };
    let id = req.id?;
    let resp = handle_request(
        engine,
        id,
        &req.method,
        req.params,
        base_url,
        allow_local_file_import,
    );
    serde_json::to_value(resp).ok()
}

fn write_response(stdout: &mut io::Stdout, resp: &JsonRpcResponse) -> Result<()> {
    let serialized = serde_json::to_string(resp)?;
    writeln!(stdout, "{serialized}")?;
    stdout.flush()?;
    Ok(())
}

fn handle_request(
    engine: &CampaignEngine,
    id: Value,
    method: &str,
    params: Value,
    base_url: Option<&str>,
    allow_local_file_import: bool,
) -> JsonRpcResponse {
    let outcome: Result<Value> = match method {
        "initialize" => {
            let client_ver = params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("2025-06-18");
            Ok(json!({
                "protocolVersion": client_ver,
                "capabilities": {
                    "tools": { "listChanged": false },
                    "resources": { "subscribe": false, "listChanged": false },
                    "prompts": { "listChanged": false }
                },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": SERVER_VERSION
                },
                "instructions": "Chronicle RPG GM Engine. Follow strict read order: campaign_status -> get_scene_context(format: 'markdown') -> targeted search_*/get_*. Use roll_dice before adjudicating uncertain outcomes, batch mutations in commit_turn (advance_time -> move_character -> character deltas -> update_place -> texts/lore -> relations/clocks -> log_event), and never reveal unread Books/Scrolls content until record_text_reading is logged."
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => handle_tool_call(engine, params, base_url, allow_local_file_import),
        "resources/list" => handle_resources_list(engine),
        "resources/read" => handle_resources_read(engine, params),
        "prompts/list" => Ok(json!({ "prompts": prompt_definitions() })),
        "prompts/get" => handle_prompt_get(params),
        _ => {
            return JsonRpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(JsonRpcError {
                    code: -32601,
                    message: format!("Method not found: {method}"),
                }),
            };
        }
    };

    match outcome {
        Ok(val) => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(val),
            error: None,
        },
        Err(e) => JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code: -32603,
                message: e.to_string(),
            }),
        },
    }
}

fn handle_tool_call(
    engine: &CampaignEngine,
    params: Value,
    base_url: Option<&str>,
    allow_local_file_import: bool,
) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing tool name"))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let exec_result = match name {
        "search" => execute_chatgpt_search(engine, args, base_url),
        "fetch" => execute_chatgpt_fetch(engine, args, base_url),
        _ => execute_tool_with_policy(engine, name, args, allow_local_file_import),
    };

    match exec_result {
        Ok(data) => {
            let text = if let Some(s) = data.as_str() {
                s.to_string()
            } else {
                serde_json::to_string(&data)?
            };
            // Return both `content` (JSON/text) and `structuredContent` (when object)
            // for full MCP 2025-06-18 & ChatGPT Connectors/Apps compatibility.
            if data.is_object() {
                Ok(json!({
                    "content": [{ "type": "text", "text": text }],
                    "structuredContent": data,
                    "isError": false
                }))
            } else {
                Ok(json!({
                    "content": [{ "type": "text", "text": text }],
                    "isError": false
                }))
            }
        }
        Err(e) => Ok(json!({
            "content": [{ "type": "text", "text": format!("Error executing tool '{name}': {e}") }],
            "isError": true
        })),
    }
}

/// ChatGPT-standard `search` tool (Connector / Deep Research spec):
/// Accepts `{ "query": string }` and returns `{ "results": [{ "id", "title", "url" }] }`.
fn execute_chatgpt_search(
    engine: &CampaignEngine,
    args: Value,
    base_url: Option<&str>,
) -> Result<Value> {
    let query = opt_str(&args, "query").unwrap_or_default();
    let origin = base_url
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_else(|| "http://127.0.0.1:7878".to_string());

    let omni = engine.search_omni(&query, 10)?;
    let mut results = Vec::new();

    if query.trim().is_empty()
        || query.to_lowercase().contains("index")
        || query.to_lowercase().contains("overview")
        || query.to_lowercase().contains("campaign")
    {
        results.push(json!({
            "id": "index:campaign",
            "title": "Campaign Codex Index",
            "url": format!("{origin}/api/docs/INDEX.md")
        }));
    }
    if query.to_lowercase().contains("timeline") || query.to_lowercase().contains("history") {
        results.push(json!({
            "id": "timeline:campaign",
            "title": "Master Campaign Timeline",
            "url": format!("{origin}/api/docs/TIMELINE.md")
        }));
    }

    for c in omni.characters {
        results.push(json!({
            "id": format!("character:{}", c.id),
            "title": format!("Character: {} ({})", c.name, c.kind),
            "url": format!("{origin}/api/docs/characters/{}.md", c.id)
        }));
    }
    for p in omni.places {
        results.push(json!({
            "id": format!("place:{}", p.id),
            "title": format!("Place: {} ({})", p.name, p.place_type),
            "url": format!("{origin}/api/docs/places/{}.md", p.id)
        }));
    }
    for t in omni.texts {
        results.push(json!({
            "id": format!("text:{}", t.id),
            "title": format!("{} [{}]: {}", t.title, t.medium, t.subject),
            "url": format!("{origin}/api/docs/texts/{}.md", t.id)
        }));
    }
    for l in omni.lore {
        let cat_slug = slugify(&l.category);
        results.push(json!({
            "id": format!("lore:{}", l.id),
            "title": format!("Lore [{}]: {}", l.category, l.title),
            "url": format!("{origin}/api/docs/lore/{cat_slug}/{}.md", l.id)
        }));
    }
    for a in omni.attachments {
        results.push(json!({
            "id": format!("attachment:{}", a.id),
            "title": format!("Attached Doc [{}]: {} ({})", a.category, a.title, a.original_filename),
            "url": format!("{origin}/api/docs/attachments/{}.md", a.id)
        }));
    }
    for it in omni.items {
        results.push(json!({
            "id": format!("item:{}", it.id),
            "title": format!("Item [{}]: {}", it.item_type, it.name),
            "url": format!("{origin}/api/docs/items/{}.md", it.id)
        }));
    }

    Ok(json!({ "results": results }))
}

/// ChatGPT-standard `fetch` tool (Connector / Deep Research spec):
/// Accepts `{ "id": string }` and returns `{ "id", "title", "text", "url", "metadata" }`.
fn execute_chatgpt_fetch(
    engine: &CampaignEngine,
    args: Value,
    base_url: Option<&str>,
) -> Result<Value> {
    let raw_id = req_str(&args, "id")?;
    let origin = base_url
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_else(|| "http://127.0.0.1:7878".to_string());

    // Ensure compiled Markdown docs are fresh before reading
    let _ = engine.compile_docs()?;
    let docs = engine.docs_dir();

    if raw_id == "index:campaign" || raw_id.eq_ignore_ascii_case("index") {
        let text = std::fs::read_to_string(docs.join("INDEX.md"))?;
        return Ok(json!({
            "id": "index:campaign",
            "title": "Campaign Codex Index",
            "text": text,
            "url": format!("{origin}/api/docs/INDEX.md"),
            "metadata": { "type": "index" }
        }));
    }
    if raw_id == "timeline:campaign" || raw_id.eq_ignore_ascii_case("timeline") {
        let text = std::fs::read_to_string(docs.join("TIMELINE.md"))?;
        return Ok(json!({
            "id": "timeline:campaign",
            "title": "Master Campaign Timeline",
            "text": text,
            "url": format!("{origin}/api/docs/TIMELINE.md"),
            "metadata": { "type": "timeline" }
        }));
    }

    if let Some(cid) = raw_id.strip_prefix("character:") {
        let proj = engine.get_character(cid, None)?;
        let path = docs.join("characters").join(format!("{}.md", proj.baseline.id));
        let text = std::fs::read_to_string(&path)?;
        return Ok(json!({
            "id": format!("character:{}", proj.baseline.id),
            "title": proj.baseline.name,
            "text": text,
            "url": format!("{origin}/api/docs/characters/{}.md", proj.baseline.id),
            "metadata": {
                "type": "character",
                "kind": proj.baseline.kind,
                "status": proj.current.alive_status,
                "location": proj.current.current_location
            }
        }));
    }
    if let Some(pid) = raw_id.strip_prefix("place:") {
        let proj = engine.get_place(pid, None)?;
        let path = docs.join("places").join(format!("{}.md", proj.baseline.id));
        let text = std::fs::read_to_string(&path)?;
        return Ok(json!({
            "id": format!("place:{}", proj.baseline.id),
            "title": proj.baseline.name,
            "text": text,
            "url": format!("{origin}/api/docs/places/{}.md", proj.baseline.id),
            "metadata": {
                "type": "place",
                "place_type": proj.baseline.place_type,
                "terrain": proj.baseline.terrain,
                "season": proj.current.current_season,
                "weather": proj.current.current_weather
            }
        }));
    }
    if let Some(tid) = raw_id.strip_prefix("text:") {
        let t = engine.get_text(tid)?;
        let path = docs.join("texts").join(format!("{}.md", t.id));
        let text = std::fs::read_to_string(&path)?;
        return Ok(json!({
            "id": format!("text:{}", t.id),
            "title": t.title,
            "text": text,
            "url": format!("{origin}/api/docs/texts/{}.md", t.id),
            "metadata": {
                "type": "text",
                "medium": t.medium,
                "subject": t.subject,
                "language": t.language,
                "read_status": t.read_status,
                "holder_or_location_id": t.holder_or_location_id
            }
        }));
    }
    if let Some(lid) = raw_id.strip_prefix("lore:") {
        let l = engine.get_lore(lid)?;
        let cat_slug = slugify(&l.category);
        let path = docs.join("lore").join(&cat_slug).join(format!("{}.md", l.id));
        let text = std::fs::read_to_string(&path)?;
        return Ok(json!({
            "id": format!("lore:{}", l.id),
            "title": l.title,
            "text": text,
            "url": format!("{origin}/api/docs/lore/{cat_slug}/{}.md", l.id),
            "metadata": {
                "type": "lore",
                "category": l.category,
                "visibility": l.visibility
            }
        }));
    }
    if let Some(aid) = raw_id.strip_prefix("attachment:").or_else(|| raw_id.strip_prefix("doc:")) {
        let a = engine.get_attachment(aid)?;
        let card_path = docs.join("attachments").join(format!("{}.md", a.id));
        let mut text = std::fs::read_to_string(&card_path)?;
        // If it's a UTF-8 text/markdown/json file, append the copied file contents directly for convenience
        if a.mime_type.starts_with("text/") || a.mime_type == "application/json" {
            if let Ok(raw_text) = std::fs::read_to_string(engine.attachment_file_path(&a)) {
                text.push_str("\n---\n\n## Copied File Contents\n\n");
                text.push_str(&raw_text);
            }
        }
        return Ok(json!({
            "id": format!("attachment:{}", a.id),
            "title": a.title,
            "text": text,
            "url": format!("{origin}/api/attachments/{}/file", a.id),
            "metadata": {
                "type": "attachment",
                "category": a.category,
                "mime_type": a.mime_type,
                "relative_path": a.relative_path,
                "mcp_uri": format!("chronicle://attachments/{}", a.id),
                "file_url": format!("{origin}/api/attachments/{}/file", a.id),
                "sha256": a.sha256
            }
        }));
    }
    if let Some(iid) = raw_id.strip_prefix("item:") {
        let it = engine.get_catalog_item(iid)?;
        let path = docs.join("items").join(format!("{}.md", it.id));
        let text = std::fs::read_to_string(&path)?;
        return Ok(json!({
            "id": format!("item:{}", it.id),
            "title": it.name,
            "text": text,
            "url": format!("{origin}/api/docs/items/{}.md", it.id),
            "metadata": {
                "type": "item",
                "item_type": it.item_type,
                "current_holder_id": it.current_holder_id,
                "previous_owners": it.previous_owners,
                "linked_text_id": it.linked_text_id
            }
        }));
    }

    // Fallback: auto-detect bare slug across characters, places, texts, lore, items, and attachments
    if let Ok(proj) = engine.get_character(&raw_id, None) {
        return execute_chatgpt_fetch(
            engine,
            json!({ "id": format!("character:{}", proj.baseline.id) }),
            base_url,
        );
    }
    if let Ok(proj) = engine.get_place(&raw_id, None) {
        return execute_chatgpt_fetch(
            engine,
            json!({ "id": format!("place:{}", proj.baseline.id) }),
            base_url,
        );
    }
    if let Ok(t) = engine.get_text(&raw_id) {
        return execute_chatgpt_fetch(engine, json!({ "id": format!("text:{}", t.id) }), base_url);
    }
    if let Ok(l) = engine.get_lore(&raw_id) {
        return execute_chatgpt_fetch(engine, json!({ "id": format!("lore:{}", l.id) }), base_url);
    }
    if let Ok(a) = engine.get_attachment(&raw_id) {
        return execute_chatgpt_fetch(
            engine,
            json!({ "id": format!("attachment:{}", a.id) }),
            base_url,
        );
    }
    if let Ok(it) = engine.get_catalog_item(&raw_id) {
        return execute_chatgpt_fetch(
            engine,
            json!({ "id": format!("item:{}", it.id) }),
            base_url,
        );
    }

    Err(anyhow!("Document '{raw_id}' not found. Pass an ID returned by `search` (e.g., character:<slug>, place:<slug>, text:<slug>, lore:<slug>, index:campaign, timeline:campaign)."))
}

/// Public unified tool dispatcher shared by MCP stdio, HTTP REST API, and batch `commit_turn`.
pub fn execute_tool(engine: &CampaignEngine, name: &str, args: Value) -> Result<Value> {
    execute_tool_with_policy(engine, name, args, true)
}

/// Unified tool dispatcher with explicit HTTP security policy (`allow_local_file_import`).
pub fn execute_tool_with_policy(
    engine: &CampaignEngine,
    name: &str,
    args: Value,
    allow_local_file_import: bool,
) -> Result<Value> {
    match name {
        "campaign_status" => {
            let meta = engine.db().get_campaign_meta()?;
            let chars = engine.list_characters()?;
            let places = engine.list_places()?;
            let lore = engine.db().list_lore(None)?;
            let texts = engine.db().list_texts()?;
            let items = engine.db().list_catalog_items()?;
            let histories = engine.db().list_all_histories()?;
            let attachments = engine.db().list_attachments()?;
            let clocks = engine.db().list_clocks()?;
            let profiles = engine.db().list_ruleset_profiles()?;
            let tables = engine.db().list_random_tables()?;
            let active_encounters = engine.db().list_encounters(Some("active"))?;
            let pending_discoveries = engine.db().list_staged_discoveries(Some("pending"))?;
            let factions = engine.db().list_factions()?;
            let claims = engine.db().list_knowledge_claims()?;
            let chain_report = engine.db().verify_event_chain_detailed()?;
            let db_identity = engine.db().database_identity()?;
            Ok(json!({
                "campaign": meta,
                "database_identity": db_identity,
                "docs_dir": engine.docs_dir().display().to_string(),
                "counts": {
                    "characters": chars.len(),
                    "places": places.len(),
                    "lore_entries": lore.len(),
                    "story_texts": texts.len(),
                    "catalog_items": items.len(),
                    "entity_histories": histories.len(),
                    "attachments": attachments.len(),
                    "clocks": clocks.len(),
                    "ruleset_profiles": profiles.len(),
                    "random_tables": tables.len(),
                    "active_encounters": active_encounters.len(),
                    "pending_discoveries": pending_discoveries.len(),
                    "factions": factions.len(),
                    "knowledge_claims": claims.len(),
                    "events": chain_report.total_events
                },
                "ledger_hash_chain_valid": chain_report.valid,
                "ledger_chain_report": chain_report,
                "active_clocks": clocks,
                "active_encounters": active_encounters.iter().map(|e| json!({
                    "id": e.id,
                    "title": e.title,
                    "encounter_type": e.encounter_type,
                    "round": e.round,
                    "active_actor": e.active_participant().map(|p| p.entity_id.clone())
                })).collect::<Vec<_>>(),
                "characters": chars.iter().map(|c| json!({
                    "id": c.baseline.id,
                    "name": c.baseline.name,
                    "kind": c.baseline.kind,
                    "status": c.current.alive_status,
                    "location": c.current.current_location
                })).collect::<Vec<_>>(),
                "places": places.iter().map(|p| json!({
                    "id": p.baseline.id,
                    "name": p.baseline.name,
                    "place_type": p.baseline.place_type,
                    "terrain": p.baseline.terrain,
                    "weather": p.current.current_weather,
                    "present_characters": p.current.present_characters
                })).collect::<Vec<_>>()
            }))
        }

        "get_scene_context" => {
            let format = opt_str(&args, "format").unwrap_or_else(|| "json".to_string());
            let max_chars = args
                .get("max_chars")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            let req: SceneContextRequest = serde_json::from_value(args)?;
            let packet = engine.get_scene_context(&req)?;
            if format.eq_ignore_ascii_case("markdown") || format.eq_ignore_ascii_case("md") {
                Ok(Value::String(render_scene_context_markdown(
                    &packet, max_chars,
                )))
            } else {
                Ok(serde_json::to_value(packet)?)
            }
        }

        "advance_time" => {
            let minutes = args
                .get("minutes")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| anyhow!("Missing 'minutes' (u64)"))?;
            let global_weather = opt_str(&args, "global_weather");
            let reason = opt_str(&args, "reason");
            let (clock, season_changed, ev) =
                engine.advance_time(minutes, global_weather, reason)?;
            Ok(json!({
                "world_clock": clock,
                "time_of_day": clock.time_of_day(),
                "season_changed": season_changed,
                "event_seq": ev.seq,
                "summary": ev.summary
            }))
        }

        "start_new_session" => {
            let title = req_str(&args, "title")?;
            let summary = opt_str(&args, "summary").unwrap_or_default();
            let (session_num, ev) = engine.start_new_session(&title, &summary)?;
            Ok(json!({
                "session_number": session_num,
                "event_seq": ev.seq,
                "summary": ev.summary
            }))
        }

        "create_character" => {
            let baseline: CharacterBaseline = serde_json::from_value(args)?;
            let proj = engine.create_character(baseline)?;
            Ok(json!({
                "doc_path": engine.docs_dir().join("characters").join(format!("{}.md", proj.baseline.id)).display().to_string(),
                "character": proj
            }))
        }

        "get_character" => {
            let id = req_str(&args, "character_id")?;
            let as_of = args.get("as_of_seq").and_then(|v| v.as_i64());
            let proj = engine.get_character(&id, as_of)?;
            let rels = engine.db().list_relations_for_entity(&proj.baseline.id)?;
            Ok(json!({
                "doc_path": engine.docs_dir().join("characters").join(format!("{}.md", proj.baseline.id)).display().to_string(),
                "character": proj,
                "relations": rels
            }))
        }

        "move_character" => {
            let cid = req_str(&args, "character_id")?;
            let to_place = req_str(&args, "to_place_id")?;
            let notes = opt_str(&args, "travel_notes").unwrap_or_default();
            let (proj, ev) = engine.move_character(&cid, &to_place, &notes)?;
            Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "current_status": proj.current }))
        }

        "modify_character_resource" => {
            let cid = req_str(&args, "character_id")?;
            let resource = req_str(&args, "resource")?;
            let delta = args.get("delta").and_then(|v| v.as_i64());
            let set_current = args.get("set_current").and_then(|v| v.as_i64());
            let set_max = args.get("set_max").and_then(|v| v.as_i64());
            let unit = opt_str(&args, "unit");
            let reason = opt_str(&args, "reason").unwrap_or_default();
            let (proj, ev) =
                engine.modify_resource(&cid, &resource, delta, set_current, set_max, unit, &reason)?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "resources": proj.current.resources
            }))
        }

        "modify_character_stat" => {
            let cid = req_str(&args, "character_id")?;
            let stat = req_str(&args, "stat")?;
            let delta = args.get("delta").and_then(|v| v.as_i64());
            let set_value = args.get("set_value").and_then(|v| v.as_i64());
            let reason = opt_str(&args, "reason").unwrap_or_default();
            let (proj, ev) = engine.modify_stat(&cid, &stat, delta, set_value, &reason)?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "stats": proj.current.stats
            }))
        }

        "set_character_status" => {
            let cid = req_str(&args, "character_id")?;
            let alive_status = req_str(&args, "alive_status")?;
            let reason = opt_str(&args, "reason").unwrap_or_default();
            let (proj, ev) = engine.set_character_status(&cid, &alive_status, &reason)?;
            Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "alive_status": proj.current.alive_status }))
        }

        "update_character_condition" => {
            let cid = req_str(&args, "character_id")?;
            let action = req_str(&args, "action")?;
            match action.to_lowercase().as_str() {
                "add" | "upsert" => {
                    let cond = StatusCondition {
                        name: req_str(&args, "name")?,
                        kind: opt_str(&args, "kind").unwrap_or_else(|| "condition".to_string()),
                        severity: opt_str(&args, "severity").unwrap_or_default(),
                        duration: opt_str(&args, "duration").unwrap_or_default(),
                        source: opt_str(&args, "source").unwrap_or_default(),
                        notes: opt_str(&args, "notes").unwrap_or_default(),
                    };
                    let (proj, ev) = engine.add_condition(&cid, cond)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "conditions": proj.current.conditions }))
                }
                "remove" | "clear" => {
                    let name = req_str(&args, "name")?;
                    let reason = opt_str(&args, "reason").unwrap_or_default();
                    let (proj, ev) = engine.remove_condition(&cid, &name, &reason)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "conditions": proj.current.conditions }))
                }
                other => Err(anyhow!("Unknown action '{other}'; expected 'add' or 'remove'")),
            }
        }

        "update_character_equipment" => {
            let cid = req_str(&args, "character_id")?;
            let action = req_str(&args, "action")?;
            let reason = opt_str(&args, "reason").unwrap_or_default();
            match action.to_lowercase().as_str() {
                "acquire" | "add" => {
                    let item = Item {
                        name: req_str(&args, "name")?,
                        slot: opt_str(&args, "slot"),
                        quantity: args.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1),
                        equipped: args
                            .get("equipped")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        durability: opt_str(&args, "durability"),
                        tags: opt_str_vec(&args, "tags"),
                        description: opt_str(&args, "description").unwrap_or_default(),
                    };
                    let (proj, ev) = engine.add_item(&cid, item, &reason)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "equipment": proj.current.equipment }))
                }
                "remove" | "consume" => {
                    let name = req_str(&args, "name")?;
                    let qty = args.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1);
                    let (proj, ev) = engine.remove_item(&cid, &name, qty, &reason)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "equipment": proj.current.equipment }))
                }
                "equip" | "unequip" => {
                    let name = req_str(&args, "name")?;
                    let slot = opt_str(&args, "slot");
                    let equipped = action.eq_ignore_ascii_case("equip");
                    let (proj, ev) = engine.equip_item(&cid, &name, slot, equipped)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "equipment": proj.current.equipment }))
                }
                other => Err(anyhow!(
                    "Unknown equipment action '{other}'; expected 'acquire', 'remove', 'equip', or 'unequip'"
                )),
            }
        }

        "upsert_character_skill" => {
            let cid = req_str(&args, "character_id")?;
            let reason = opt_str(&args, "reason").unwrap_or_default();
            let skill = Skill {
                name: req_str(&args, "name")?,
                rank: opt_str(&args, "rank").unwrap_or_else(|| "Novice".to_string()),
                category: opt_str(&args, "category").unwrap_or_default(),
                modifier: opt_str(&args, "modifier"),
                description: opt_str(&args, "description").unwrap_or_default(),
            };
            let (proj, ev) = engine.upsert_skill(&cid, skill, &reason)?;
            Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "skills": proj.current.skills }))
        }

        "upsert_character_technique" => {
            let cid = req_str(&args, "character_id")?;
            let reason = opt_str(&args, "reason").unwrap_or_default();
            let tech = Technique {
                name: req_str(&args, "name")?,
                tier: opt_str(&args, "tier").unwrap_or_default(),
                cost: opt_str(&args, "cost").unwrap_or_default(),
                cooldown: opt_str(&args, "cooldown").unwrap_or_default(),
                mastery: opt_str(&args, "mastery").unwrap_or_default(),
                effect: opt_str(&args, "effect").unwrap_or_default(),
                description: opt_str(&args, "description").unwrap_or_default(),
            };
            let (proj, ev) = engine.upsert_technique(&cid, tech, &reason)?;
            Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "techniques": proj.current.techniques }))
        }

        "update_character_goal" => {
            let cid = req_str(&args, "character_id")?;
            let goal = req_str(&args, "goal")?;
            let completed = args
                .get("completed")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let (proj, ev) = engine.update_goal(&cid, &goal, completed)?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "active_goals": proj.current.active_goals,
                "completed_goals": proj.current.completed_goals
            }))
        }

        "create_place" => {
            let baseline: PlaceBaseline = serde_json::from_value(args)?;
            let proj = engine.create_place(baseline)?;
            Ok(json!({
                "doc_path": engine.docs_dir().join("places").join(format!("{}.md", proj.baseline.id)).display().to_string(),
                "place": proj
            }))
        }

        "get_place" => {
            let id = req_str(&args, "place_id")?;
            let as_of = args.get("as_of_seq").and_then(|v| v.as_i64());
            let proj = engine.get_place(&id, as_of)?;
            let rels = engine.db().list_relations_for_entity(&proj.baseline.id)?;
            Ok(json!({
                "doc_path": engine.docs_dir().join("places").join(format!("{}.md", proj.baseline.id)).display().to_string(),
                "place": proj,
                "relations": rels
            }))
        }

        "update_place" => {
            let pid = req_str(&args, "place_id")?;
            let action = req_str(&args, "action")?;
            let notes = opt_str(&args, "notes").unwrap_or_default();
            match action.to_lowercase().as_str() {
                "weather" => {
                    let weather = req_str(&args, "weather")?;
                    let (proj, ev) = engine.set_place_weather(&pid, &weather, &notes)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "current_status": proj.current }))
                }
                "add_condition" | "remove_condition" => {
                    let condition = req_str(&args, "condition")?;
                    let active = action.eq_ignore_ascii_case("add_condition");
                    let (proj, ev) = engine.set_place_condition(&pid, &condition, active, &notes)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "current_status": proj.current }))
                }
                "control" => {
                    let faction = opt_str(&args, "faction");
                    let danger = opt_str(&args, "danger_level");
                    let (proj, ev) = engine.set_place_control(&pid, faction, danger, &notes)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "current_status": proj.current }))
                }
                "landmark" => {
                    let lm = Landmark {
                        name: req_str(&args, "landmark_name")?,
                        kind: opt_str(&args, "landmark_kind").unwrap_or_default(),
                        status: opt_str(&args, "landmark_status")
                            .unwrap_or_else(|| "Intact".to_string()),
                        description: opt_str(&args, "description").unwrap_or_default(),
                        tags: opt_str_vec(&args, "tags"),
                    };
                    let (proj, ev) = engine.upsert_landmark(&pid, lm, &notes)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "landmarks": proj.current.landmarks }))
                }
                "route" => {
                    let route = Route {
                        destination_id: req_str(&args, "destination_id")?,
                        travel_time: opt_str(&args, "travel_time").unwrap_or_default(),
                        terrain_difficulty: opt_str(&args, "terrain_difficulty").unwrap_or_default(),
                        status: opt_str(&args, "route_status")
                            .unwrap_or_else(|| "Open".to_string()),
                        notes: notes.clone(),
                    };
                    let (proj, ev) = engine.upsert_route(&pid, route, &notes)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "routes": proj.current.routes }))
                }
                other => Err(anyhow!(
                    "Unknown place action '{other}'; expected 'weather', 'add_condition', 'remove_condition', 'control', 'landmark', or 'route'"
                )),
            }
        }

        "upsert_lore" => {
            let notes = opt_str(&args, "notes").unwrap_or_default();
            let entry = LoreEntry {
                id: opt_str(&args, "id").unwrap_or_default(),
                title: req_str(&args, "title")?,
                category: opt_str(&args, "category").unwrap_or_else(|| "world".to_string()),
                summary: opt_str(&args, "summary").unwrap_or_default(),
                body: req_str(&args, "body")?,
                keywords: opt_str_vec(&args, "keywords"),
                tags: opt_str_vec(&args, "tags"),
                visibility: opt_str(&args, "visibility").unwrap_or_else(|| "public".to_string()),
                always_include: args
                    .get("always_include")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                related_ids: opt_str_vec(&args, "related_ids"),
                created_at_world_time: String::new(),
                updated_at_world_time: String::new(),
            };
            let (saved, ev) = engine.upsert_lore(entry, &notes)?;
            let doc_path = engine
                .docs_dir()
                .join("lore")
                .join(slugify(&saved.category))
                .join(format!("{}.md", saved.id));
            Ok(json!({
                "event_seq": ev.seq,
                "doc_path": doc_path.display().to_string(),
                "lore": saved
            }))
        }

        "search_lore" => {
            if let Some(id) = opt_str(&args, "lore_id") {
                let entry = engine.get_lore(&id)?;
                return Ok(serde_json::to_value(entry)?);
            }
            let filter: LoreSearchFilter = serde_json::from_value(args)?;
            let entries = engine.search_lore_by(&filter)?;
            Ok(json!({ "count": entries.len(), "entries": entries }))
        }

        "upsert_text" => {
            let notes = opt_str(&args, "notes").unwrap_or_default();
            let text = StoryText {
                id: opt_str(&args, "id").unwrap_or_default(),
                title: req_str(&args, "title")?,
                medium: opt_str(&args, "medium").unwrap_or_else(|| "book".to_string()),
                subject: opt_str(&args, "subject").unwrap_or_else(|| "history".to_string()),
                author: opt_str(&args, "author").unwrap_or_default(),
                language: opt_str(&args, "language").unwrap_or_else(|| "Common".to_string()),
                condition: opt_str(&args, "condition").unwrap_or_else(|| "Intact".to_string()),
                read_status: opt_str(&args, "read_status")
                    .unwrap_or_else(|| "undiscovered".to_string()),
                read_by: opt_str_vec(&args, "read_by"),
                holder_or_location_id: opt_str(&args, "holder_or_location_id"),
                summary: opt_str(&args, "summary").unwrap_or_default(),
                content: req_str(&args, "content")?,
                mechanics_or_unlocks: opt_str_vec(&args, "mechanics_or_unlocks"),
                gm_notes: opt_str(&args, "gm_notes").unwrap_or_default(),
                keywords: opt_str_vec(&args, "keywords"),
                tags: opt_str_vec(&args, "tags"),
                related_ids: opt_str_vec(&args, "related_ids"),
                created_at_world_time: String::new(),
                updated_at_world_time: String::new(),
            };
            let (saved, ev) = engine.upsert_text(text, &notes)?;
            let doc_path = engine
                .docs_dir()
                .join("texts")
                .join(format!("{}.md", saved.id));
            Ok(json!({
                "event_seq": ev.seq,
                "doc_path": doc_path.display().to_string(),
                "text": saved
            }))
        }

        "get_text" => {
            let id = req_str(&args, "text_id")?;
            let text = engine.get_text(&id)?;
            let events = engine.db().list_events_for_entity(&text.id)?;
            let rels = engine.db().list_relations_for_entity(&text.id)?;
            Ok(json!({
                "doc_path": engine.docs_dir().join("texts").join(format!("{}.md", text.id)).display().to_string(),
                "text": text,
                "relations": rels,
                "event_log": events
            }))
        }

        "record_text_reading" => {
            let text_id = req_str(&args, "text_id")?;
            let reader_id = opt_str(&args, "reader_id");
            let read_status = opt_str(&args, "read_status");
            let holder = opt_str(&args, "holder_or_location_id");
            let condition = opt_str(&args, "condition");
            let notes = opt_str(&args, "notes").unwrap_or_default();
            let (saved, ev) = engine.record_text_reading(
                &text_id,
                reader_id,
                read_status,
                holder,
                condition,
                &notes,
            )?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "text": saved
            }))
        }

        "search_texts" => {
            if let Some(id) = opt_str(&args, "text_id") {
                let text = engine.get_text(&id)?;
                return Ok(serde_json::to_value(text)?);
            }
            let filter: TextSearchFilter = serde_json::from_value(args)?;
            let texts = engine.search_texts_by(&filter)?;
            Ok(json!({ "count": texts.len(), "texts": texts }))
        }

        "import_attachment" => {
            let source_path = opt_str(&args, "source_path");
            if !allow_local_file_import && source_path.is_some() {
                return Err(anyhow!(
                    "Reading arbitrary host files via `import_attachment.source_path` is disabled over HTTP for security. Pass inline `text_content` or `base64_content`, use `create_reference_doc`, or start Chronicle with `--allow-local-file-import`."
                ));
            }
            let req = ImportAttachmentRequest {
                id: opt_str(&args, "id"),
                title: req_str(&args, "title")?,
                source_path,
                text_content: opt_str(&args, "text_content"),
                base64_content: opt_str(&args, "base64_content"),
                filename: opt_str(&args, "filename"),
                category: opt_str(&args, "category"),
                mime_type: opt_str(&args, "mime_type"),
                summary: opt_str(&args, "summary").unwrap_or_default(),
                extracted_notes: opt_str(&args, "extracted_notes").unwrap_or_default(),
                visibility: opt_str(&args, "visibility"),
                keywords: opt_str_vec(&args, "keywords"),
                tags: opt_str_vec(&args, "tags"),
                related_ids: opt_str_vec(&args, "related_ids"),
                notes: opt_str(&args, "notes").unwrap_or_default(),
            };
            let (att, abs_path, ev) = engine.import_attachment(req)?;
            Ok(json!({
                "event_seq": ev.seq,
                "id": att.id,
                "reference_id": format!("attachment:{}", att.id),
                "mcp_uri": format!("chronicle://attachments/{}", att.id),
                "relative_path": att.relative_path,
                "absolute_path": abs_path.display().to_string(),
                "doc_card_path": engine.docs_dir().join("attachments").join(format!("{}.md", att.id)).display().to_string(),
                "attachment": att
            }))
        }

        "get_attachment" => {
            let id = req_str(&args, "attachment_id")?;
            let include_text = args
                .get("include_text_content")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let att = engine.get_attachment(&id)?;
            let abs_path = engine.attachment_file_path(&att);
            let events = engine.db().list_events_for_entity(&att.id)?;
            let rels = engine.db().list_relations_for_entity(&att.id)?;
            let text_content = if include_text
                && (att.mime_type.starts_with("text/") || att.mime_type == "application/json")
            {
                std::fs::read_to_string(&abs_path).ok()
            } else {
                None
            };
            Ok(json!({
                "id": att.id,
                "reference_id": format!("attachment:{}", att.id),
                "mcp_uri": format!("chronicle://attachments/{}", att.id),
                "relative_path": att.relative_path,
                "absolute_path": abs_path.display().to_string(),
                "attachment": att,
                "text_content": text_content,
                "relations": rels,
                "event_log": events
            }))
        }

        "search_attachments" => {
            if let Some(id) = opt_str(&args, "attachment_id") {
                let att = engine.get_attachment(&id)?;
                return Ok(serde_json::to_value(att)?);
            }
            let filter: AttachmentSearchFilter = serde_json::from_value(args)?;
            let atts = engine.search_attachments_by(&filter)?;
            Ok(json!({ "count": atts.len(), "attachments": atts }))
        }

        "create_reference_doc" => {
            let title = req_str(&args, "title")?;
            let content = req_str(&args, "content")?;
            let append = args.get("append").and_then(|v| v.as_bool()).unwrap_or(false);
            let (att, abs_path, ev) = engine.create_reference_doc(
                opt_str(&args, "id"),
                title,
                opt_str(&args, "filename"),
                content,
                append,
                opt_str(&args, "category"),
                opt_str(&args, "summary").unwrap_or_default(),
                opt_str(&args, "extracted_notes").unwrap_or_default(),
                opt_str(&args, "visibility"),
                opt_str_vec(&args, "keywords"),
                opt_str_vec(&args, "tags"),
                opt_str_vec(&args, "related_ids"),
                opt_str(&args, "notes").unwrap_or_default(),
            )?;
            Ok(json!({
                "event_seq": ev.seq,
                "id": att.id,
                "reference_id": format!("attachment:{}", att.id),
                "mcp_uri": format!("chronicle://attachments/{}", att.id),
                "relative_path": att.relative_path,
                "absolute_path": abs_path.display().to_string(),
                "attachment": att
            }))
        }

        "upsert_item" => {
            let notes = opt_str(&args, "notes").unwrap_or_default();
            let item = CatalogItem {
                id: opt_str(&args, "id").unwrap_or_default(),
                name: req_str(&args, "name")?,
                item_type: opt_str(&args, "item_type").unwrap_or_else(|| "gear".to_string()),
                rarity_or_tier: opt_str(&args, "rarity_or_tier").unwrap_or_default(),
                slot: opt_str(&args, "slot"),
                quantity: args.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1),
                equipped: args
                    .get("equipped")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                condition_or_durability: opt_str(&args, "condition_or_durability")
                    .unwrap_or_else(|| "Intact".to_string()),
                origin_and_maker: opt_str(&args, "origin_and_maker").unwrap_or_default(),
                previous_owners: opt_str_vec(&args, "previous_owners"),
                current_holder_id: opt_str(&args, "current_holder_id"),
                significance: opt_str(&args, "significance").unwrap_or_default(),
                properties_and_effects: opt_str(&args, "properties_and_effects")
                    .unwrap_or_default(),
                description: opt_str(&args, "description").unwrap_or_default(),
                linked_text_id: opt_str(&args, "linked_text_id"),
                gm_notes: opt_str(&args, "gm_notes").unwrap_or_default(),
                keywords: opt_str_vec(&args, "keywords"),
                tags: opt_str_vec(&args, "tags"),
                related_ids: opt_str_vec(&args, "related_ids"),
                created_at_world_time: String::new(),
                updated_at_world_time: String::new(),
            };
            let (saved, ev) = engine.upsert_catalog_item(item, &notes)?;
            Ok(json!({
                "event_seq": ev.seq,
                "doc_path": engine.docs_dir().join("items").join(format!("{}.md", saved.id)).display().to_string(),
                "item": saved
            }))
        }

        "get_item" => {
            let id = req_str(&args, "item_id")?;
            let item = engine.get_catalog_item(&id)?;
            let histories = engine.db().list_histories_for_entity(&item.id)?;
            let events = engine.db().list_events_for_entity(&item.id)?;
            let rels = engine.db().list_relations_for_entity(&item.id)?;
            Ok(json!({
                "doc_path": engine.docs_dir().join("items").join(format!("{}.md", item.id)).display().to_string(),
                "item": item,
                "histories": histories,
                "relations": rels,
                "event_log": events
            }))
        }

        "transfer_catalog_item" => {
            let item_id = req_str(&args, "item_id")?;
            let to_holder = opt_str(&args, "to_holder_id");
            let equipped = args.get("equipped").and_then(|v| v.as_bool());
            let condition = opt_str(&args, "condition_or_durability");
            let notes = opt_str(&args, "notes").unwrap_or_default();
            let (saved, ev) =
                engine.transfer_catalog_item(&item_id, to_holder, equipped, condition, &notes)?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "item": saved
            }))
        }

        "search_items" => {
            if let Some(id) = opt_str(&args, "item_id") {
                let item = engine.get_catalog_item(&id)?;
                return Ok(serde_json::to_value(item)?);
            }
            let filter: ItemSearchFilter = serde_json::from_value(args)?;
            let items = engine.search_items_by(&filter)?;
            Ok(json!({ "count": items.len(), "items": items }))
        }

        "add_entity_history" => {
            let entry = EntityHistoryEntry {
                id: 0,
                entity_id: req_str(&args, "entity_id")?,
                entity_kind: opt_str(&args, "entity_kind").unwrap_or_else(|| "place".to_string()),
                era_or_date: opt_str(&args, "era_or_date").unwrap_or_default(),
                sort_order: args.get("sort_order").and_then(|v| v.as_i64()).unwrap_or(0),
                title: req_str(&args, "title")?,
                details: req_str(&args, "details")?,
                significance: opt_str(&args, "significance").unwrap_or_default(),
                related_ids: opt_str_vec(&args, "related_ids"),
                visibility: opt_str(&args, "visibility").unwrap_or_else(|| "public".to_string()),
                recorded_at_world_time: String::new(),
            };
            let (saved, ev) = engine.add_entity_history(entry)?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "history": saved
            }))
        }

        "search_histories" => {
            let filter: HistorySearchFilter = serde_json::from_value(args)?;
            let histories = engine.search_histories_by(&filter)?;
            Ok(json!({ "count": histories.len(), "histories": histories }))
        }

        "diagnose_campaign" => {
            let report = diagnose_campaign(engine)?;
            Ok(serde_json::to_value(report)?)
        }

        "update_character_baseline" => {
            let req: UpdateCharacterBaselineRequest = serde_json::from_value(args)?;
            let (saved, ev) = update_character_baseline(engine, req)?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "baseline": saved
            }))
        }

        "update_place_baseline" => {
            let req: UpdatePlaceBaselineRequest = serde_json::from_value(args)?;
            let (saved, ev) = update_place_baseline(engine, req)?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "baseline": saved
            }))
        }

        "update_entity_history" => {
            let req: UpdateEntityHistoryRequest = serde_json::from_value(args)?;
            let (saved, ev) = update_entity_history(engine, req)?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "history": saved
            }))
        }

        "reclassify_entity" => {
            let req: ReclassifyRequest = serde_json::from_value(args)?;
            let res = reclassify_entity(engine, req)?;
            Ok(serde_json::to_value(res)?)
        }

        "rename_or_merge_entity" => {
            let req: RenameOrMergeRequest = serde_json::from_value(args)?;
            let res = rename_or_merge_entity(engine, req)?;
            Ok(serde_json::to_value(res)?)
        }

        "delete_entity" => {
            let req: DeleteEntityRequest = serde_json::from_value(args)?;
            let res = delete_entity(engine, req)?;
            Ok(serde_json::to_value(res)?)
        }

        "cleanup_campaign" => {
            let dry_run = args
                .get("dry_run")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let report = cleanup_campaign(engine, dry_run)?;
            Ok(serde_json::to_value(report)?)
        }

        "update_campaign_meta" => {
            let req: UpdateCampaignMetaRequest = serde_json::from_value(args)?;
            let (meta, ev) = update_campaign_meta(engine, req)?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "campaign": meta
            }))
        }

        "get_lore" => {
            let id = slugify(&req_str(&args, "lore_id")?);
            let lore = engine
                .db()
                .get_lore(&id)?
                .ok_or_else(|| anyhow!("Lore entry '{id}' not found"))?;
            let histories = engine.db().list_histories_for_entity(&lore.id)?;
            let rels = engine.db().list_relations_for_entity(&lore.id)?;
            let events = engine.db().list_events_for_entity(&lore.id)?;
            Ok(json!({
                "doc_path": engine
                    .docs_dir()
                    .join("lore")
                    .join(slugify(&lore.category))
                    .join(format!("{}.md", lore.id))
                    .display()
                    .to_string(),
                "lore": lore,
                "histories": histories,
                "relations": rels,
                "event_log": events
            }))
        }

        "update_attachment" => {
            let req: UpdateAttachmentRequest = serde_json::from_value(args)?;
            let (att, ev) = update_attachment_metadata(engine, req)?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "attachment": att
            }))
        }

        "backup_campaign" => {
            let label = opt_str(&args, "label").unwrap_or_else(|| "manual".to_string());
            let path = backup_campaign(engine, &label)?;
            let backups = list_backups(engine)?;
            Ok(json!({
                "backup_path": path.display().to_string(),
                "label": label,
                "available_backups": backups
            }))
        }

        "restore_backup" => {
            if let Some(filename) = opt_str(&args, "backup_filename") {
                restore_backup(engine, &filename)
            } else {
                let backups = list_backups(engine)?;
                Ok(json!({
                    "count": backups.len(),
                    "available_backups": backups,
                    "hint": "Pass backup_filename (e.g. 'campaign.backup-....db') to restore a snapshot."
                }))
            }
        }

        "search_characters" => {
            let include_full = args
                .get("include_full")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let filter: CharacterSearchFilter = serde_json::from_value(args)?;
            let matches = engine.search_characters_by(&filter)?;
            if include_full {
                Ok(json!({ "count": matches.len(), "characters": matches }))
            } else {
                let hits: Vec<_> = matches.into_iter().map(character_to_hit).collect();
                Ok(json!({ "count": hits.len(), "characters": hits }))
            }
        }

        "search_places" => {
            let include_full = args
                .get("include_full")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let filter: PlaceSearchFilter = serde_json::from_value(args)?;
            let matches = engine.search_places_by(&filter)?;
            if include_full {
                Ok(json!({ "count": matches.len(), "places": matches }))
            } else {
                let hits: Vec<_> = matches.into_iter().map(place_to_hit).collect();
                Ok(json!({ "count": hits.len(), "places": hits }))
            }
        }

        "search_relations" => {
            let filter: RelationSearchFilter = serde_json::from_value(args)?;
            let edges = engine.search_relations_by(&filter)?;
            Ok(json!({ "count": edges.len(), "relations": edges }))
        }

        "search_all" => {
            let query = opt_str(&args, "query").unwrap_or_default();
            let limit = args
                .get("limit_per_kind")
                .and_then(|v| v.as_u64())
                .unwrap_or(10) as usize;
            let res = engine.search_omni(&query, limit)?;
            Ok(serde_json::to_value(res)?)
        }

        "search" => execute_chatgpt_search(engine, args, None),
        "fetch" => execute_chatgpt_fetch(engine, args, None),

        "transfer_item" => {
            let from_id = req_str(&args, "from_character_id")?;
            let to_id = req_str(&args, "to_character_id")?;
            let item_name = req_str(&args, "item_name")?;
            let quantity = args.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1);
            let reason = opt_str(&args, "reason").unwrap_or_default();
            let (ev_rem, ev_add) =
                engine.transfer_item(&from_id, &to_id, &item_name, quantity, &reason)?;
            Ok(json!({
                "removed_event_seq": ev_rem.seq,
                "added_event_seq": ev_add.seq,
                "summary": format!("{} | {}", ev_rem.summary, ev_add.summary)
            }))
        }

        "rest_characters" => {
            let char_ids = opt_str_vec(&args, "character_ids");
            if char_ids.is_empty() {
                return Err(anyhow!("'character_ids' must contain at least one character"));
            }
            let rest_type = opt_str(&args, "rest_type").unwrap_or_else(|| "long".to_string());
            let advance_minutes = args.get("advance_minutes").and_then(|v| v.as_u64());
            let restore_percent = args
                .get("restore_percent")
                .and_then(|v| v.as_u64())
                .unwrap_or_else(|| if rest_type.eq_ignore_ascii_case("short") { 50 } else { 100 })
                as u8;
            let resources_filter = opt_str_vec(&args, "resources");
            let clear_conditions = opt_str_vec(&args, "clear_conditions");
            let notes = opt_str(&args, "notes").unwrap_or_default();
            let summaries = engine.rest_characters(
                &char_ids,
                &rest_type,
                advance_minutes,
                restore_percent,
                &resources_filter,
                &clear_conditions,
                &notes,
            )?;
            Ok(json!({ "rest_type": rest_type, "applied_changes": summaries }))
        }

        "commit_turn" => {
            let actions = args
                .get("actions")
                .and_then(|v| v.as_array())
                .ok_or_else(|| anyhow!("Missing 'actions' array for commit_turn"))?;
            let mut results = Vec::new();
            for (idx, act) in actions.iter().enumerate() {
                let tool_name = act
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("Action #{idx} missing 'tool' string"))?;
                if tool_name == "commit_turn" {
                    return Err(anyhow!("Nested commit_turn is not allowed"));
                }
                let tool_args = act.get("arguments").cloned().unwrap_or_else(|| json!({}));
                let res =
                    execute_tool_with_policy(engine, tool_name, tool_args, allow_local_file_import)
                        .map_err(|e| anyhow!("Action #{idx} ('{tool_name}') failed: {e}"))?;
                results.push(json!({ "index": idx, "tool": tool_name, "result": res }));
            }
            Ok(json!({
                "committed_actions": results.len(),
                "results": results
            }))
        }

        "upsert_relation" => {
            let dimensions: RelationshipDimensions = args
                .get("dimensions")
                .or_else(|| args.get("relationship"))
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let default_vis = if dimensions.is_empty() {
                "public".to_string()
            } else {
                "gm_secret".to_string()
            };
            let rel = GraphRelation {
                source_id: req_str(&args, "source_id")?,
                source_kind: opt_str(&args, "source_kind").unwrap_or_else(|| "entity".to_string()),
                relation_type: req_str(&args, "relation_type")?,
                target_id: req_str(&args, "target_id")?,
                target_kind: opt_str(&args, "target_kind").unwrap_or_else(|| "entity".to_string()),
                strength: args.get("strength").and_then(|v| v.as_i64()),
                notes: opt_str(&args, "notes").unwrap_or_default(),
                updated_at_world_time: String::new(),
                dimensions,
                direction: opt_str(&args, "direction").unwrap_or_else(|| "directed".to_string()),
                visibility: opt_str(&args, "visibility").unwrap_or(default_vis),
                origin_event_id: args.get("origin_event_id").and_then(|v| v.as_i64()),
                obligations: opt_str_vec(&args, "obligations"),
            };
            let ev = engine.upsert_relation(rel)?;
            Ok(json!({ "event_seq": ev.seq, "summary": ev.summary }))
        }

        "remove_relation" => {
            let source_id = req_str(&args, "source_id")?;
            let relation_type = req_str(&args, "relation_type")?;
            let target_id = req_str(&args, "target_id")?;
            let reason = opt_str(&args, "reason").unwrap_or_default();
            let ev = engine.remove_relation(&source_id, &relation_type, &target_id, &reason)?;
            Ok(json!({ "event_seq": ev.seq, "summary": ev.summary }))
        }

        "manage_clock" => {
            let action = req_str(&args, "action")?;
            match action.to_lowercase().as_str() {
                "create" => {
                    let clock = ProgressClock {
                        id: opt_str(&args, "clock_id").unwrap_or_default(),
                        name: req_str(&args, "name")?,
                        kind: opt_str(&args, "kind").unwrap_or_else(|| "quest".to_string()),
                        filled: args.get("filled").and_then(|v| v.as_i64()).unwrap_or(0),
                        segments: args.get("segments").and_then(|v| v.as_i64()).unwrap_or(6),
                        status: opt_str(&args, "status").unwrap_or_else(|| "active".to_string()),
                        description: opt_str(&args, "description").unwrap_or_default(),
                        related_ids: opt_str_vec(&args, "related_ids"),
                    };
                    let (c, ev) = engine.create_clock(clock)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "clock": c }))
                }
                "tick" => {
                    let cid = req_str(&args, "clock_id")?;
                    let delta = args.get("delta").and_then(|v| v.as_i64()).unwrap_or(1);
                    let set_status = opt_str(&args, "status");
                    let reason = opt_str(&args, "reason").unwrap_or_default();
                    let (c, ev) = engine.tick_clock(&cid, delta, set_status, &reason)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "clock": c }))
                }
                "list" => {
                    let clocks = engine.db().list_clocks()?;
                    Ok(json!({ "clocks": clocks }))
                }
                "update" => {
                    let req: UpdateClockRequest = serde_json::from_value(args)?;
                    let (c, ev) = update_clock_fields(engine, req)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "clock": c }))
                }
                other => Err(anyhow!(
                    "Unknown clock action '{other}'; expected 'create', 'tick', 'update', or 'list'"
                )),
            }
        }

        "log_event" => {
            let title = req_str(&args, "title")?;
            let details = opt_str(&args, "details").unwrap_or_default();
            let location_id = opt_str(&args, "location_id");
            let affected = opt_str_vec(&args, "affected_entities");
            let tags = opt_str_vec(&args, "tags");
            let actor_id = opt_str(&args, "actor_id").map(|s| slugify(&s));
            let causal_event_ids = opt_i64_vec(&args, "causal_event_ids");
            let visibility = opt_str(&args, "visibility").unwrap_or_else(|| "public".to_string());
            let ev = engine.log_narrative_event_causal(
                &title,
                &details,
                location_id,
                affected,
                tags,
                EventCausalMeta {
                    actor_id,
                    causal_event_ids,
                    visibility,
                },
            )?;
            Ok(json!({
                "event_seq": ev.seq,
                "world_time": ev.world_time,
                "causal_event_ids": ev.causal_event_ids,
                "visibility": ev.visibility,
                "summary": ev.summary
            }))
        }

        // ---------------------------------------------------------------------
        // Phase 1: Ruleset Profiles, Checks, Contests, Random Tables & Encounters
        // ---------------------------------------------------------------------
        "upsert_ruleset_profile" => {
            if args.get("list").and_then(|v| v.as_bool()).unwrap_or(false) {
                let profiles = engine.list_ruleset_profiles()?;
                return Ok(json!({
                    "count": profiles.len(),
                    "default_fallback": RulesetProfile::default(),
                    "profiles": profiles
                }));
            }
            let mut profile: RulesetProfile = serde_json::from_value(args)?;
            if profile.id.trim().is_empty() {
                profile.id = slugify(&profile.name);
            }
            let (saved, ev) = engine.upsert_ruleset_profile(profile)?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "profile": saved
            }))
        }

        "resolve_check" => {
            let req = CheckRequest {
                actor_id: opt_str(&args, "actor_id"),
                action: req_str(&args, "action")?,
                profile: opt_str(&args, "profile"),
                stat: opt_str(&args, "stat"),
                skill: opt_str(&args, "skill"),
                technique: opt_str(&args, "technique"),
                equipment: opt_str_vec(&args, "equipment"),
                difficulty: args
                    .get("difficulty")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| anyhow!("Missing required integer parameter 'difficulty'"))?,
                modifier: args.get("modifier").and_then(|v| v.as_i64()).unwrap_or(0),
                situational_modifiers: parse_modifier_sources(args.get("situational_modifiers")),
                advantage: opt_str(&args, "advantage").unwrap_or_else(|| "none".to_string()),
                dice_override: opt_str(&args, "dice_override"),
                auto_hydrate: args.get("auto_hydrate").and_then(|v| v.as_bool()),
                stakes: opt_str(&args, "stakes").unwrap_or_default(),
                seed: args.get("seed").and_then(|v| v.as_u64()),
                dry_run: args
                    .get("dry_run")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                strict_validation: args
                    .get("strict_validation")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                log_to_ledger: args
                    .get("log_to_ledger")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                causal_event_ids: opt_i64_vec(&args, "causal_event_ids"),
                visibility: opt_str(&args, "visibility").unwrap_or_else(|| "public".to_string()),
            };
            let res = engine.resolve_check(req)?;
            Ok(serde_json::to_value(res)?)
        }

        "resolve_contest" => {
            let mut participants: Vec<ContestParticipantInput> = args
                .get("participants")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            // Also support convenient shorthand `initiator` + `defender` objects
            if participants.is_empty() {
                if let Some(init_v) = args.get("initiator") {
                    let mut p: ContestParticipantInput = serde_json::from_value(init_v.clone())?;
                    if p.role.is_empty() {
                        p.role = "initiator".to_string();
                    }
                    participants.push(p);
                }
                if let Some(def_v) = args.get("defender") {
                    let mut p: ContestParticipantInput = serde_json::from_value(def_v.clone())?;
                    if p.role.is_empty() {
                        p.role = "defender".to_string();
                    }
                    participants.push(p);
                }
            }
            let req = ContestRequest {
                action: req_str(&args, "action")?,
                profile: opt_str(&args, "profile"),
                participants,
                tie_breaker: opt_str(&args, "tie_breaker")
                    .unwrap_or_else(|| "higher_modifier".to_string()),
                stakes: opt_str(&args, "stakes").unwrap_or_default(),
                seed: args.get("seed").and_then(|v| v.as_u64()),
                strict_validation: args
                    .get("strict_validation")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                log_to_ledger: args
                    .get("log_to_ledger")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                causal_event_ids: opt_i64_vec(&args, "causal_event_ids"),
                visibility: opt_str(&args, "visibility").unwrap_or_else(|| "public".to_string()),
            };
            let res = engine.resolve_contest(req)?;
            Ok(serde_json::to_value(res)?)
        }

        "upsert_random_table" => {
            if args.get("list").and_then(|v| v.as_bool()).unwrap_or(false) {
                let tables = engine.list_random_tables()?;
                return Ok(json!({ "count": tables.len(), "tables": tables }));
            }
            if let Some(get_id) = opt_str(&args, "get_table_id") {
                let tbl = engine.get_random_table(&get_id)?;
                return Ok(serde_json::to_value(tbl)?);
            }
            let table: RandomTable = serde_json::from_value(args)?;
            let (saved, ev) = engine.upsert_random_table(table)?;
            Ok(json!({
                "event_seq": ev.seq,
                "doc_path": engine.docs_dir().join("tables").join(format!("{}.md", saved.id)).display().to_string(),
                "table": saved
            }))
        }

        "draw_random_table" => {
            let ctx: DrawContext = args
                .get("context")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let req = DrawTableRequest {
                table_id: req_str(&args, "table_id")?,
                context: ctx,
                count: args.get("count").and_then(|v| v.as_u64()).unwrap_or(1) as usize,
                replacement: args
                    .get("replacement")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                deck_scope: opt_str(&args, "deck_scope"),
                reset_deck: args
                    .get("reset_deck")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                seed: args.get("seed").and_then(|v| v.as_u64()),
                stage_discovery: args.get("stage_discovery").and_then(|v| v.as_bool()),
                log_to_ledger: args
                    .get("log_to_ledger")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
                causal_event_ids: opt_i64_vec(&args, "causal_event_ids"),
            };
            let out = engine.draw_random_table(req)?;
            Ok(serde_json::to_value(out)?)
        }

        "claim_discovery" => {
            if args
                .get("list_pending")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                let list = engine.list_staged_discoveries(opt_str(&args, "status").as_deref())?;
                return Ok(json!({ "count": list.len(), "discoveries": list }));
            }
            let discovery_id = req_str(&args, "discovery_id")?;
            let claimant_id = req_str(&args, "claimant_id")?;
            let item_indices = args.get("item_indices").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|x| x.as_u64().map(|n| n as usize))
                    .collect::<Vec<_>>()
            });
            let create_catalog_items = args
                .get("create_catalog_items")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let notes = opt_str(&args, "notes").unwrap_or_default();
            let (disc, claimed, ev) = engine.claim_discovery(
                &discovery_id,
                &claimant_id,
                item_indices,
                create_catalog_items,
                &notes,
            )?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "claimed_items": claimed,
                "discovery": disc
            }))
        }

        "manage_encounter" => {
            let action = req_str(&args, "action")?.to_lowercase();
            match action.as_str() {
                "start" => {
                    let title = req_str(&args, "title")?;
                    let encounter_type =
                        opt_str(&args, "encounter_type").unwrap_or_else(|| "combat".to_string());
                    let location_id = opt_str(&args, "location_id");
                    let participants: Vec<EncounterParticipant> = args
                        .get("participants")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .ok_or_else(|| anyhow!("Missing valid 'participants' array"))?;
                    let init_dice = opt_str(&args, "initiative_dice");
                    let zones = opt_str_vec(&args, "zones");
                    let env_notes = opt_str(&args, "environmental_notes").unwrap_or_default();
                    let seed = args.get("seed").and_then(|v| v.as_u64());
                    let (enc, ev) = engine.start_encounter(
                        opt_str(&args, "encounter_id"),
                        &title,
                        &encounter_type,
                        location_id,
                        participants,
                        init_dice.as_deref(),
                        zones,
                        &env_notes,
                        seed,
                    )?;
                    Ok(json!({
                        "event_seq": ev.seq,
                        "doc_path": engine.docs_dir().join("encounters").join(format!("{}.md", enc.id)).display().to_string(),
                        "encounter": enc
                    }))
                }
                "act" => {
                    let enc_id = req_str(&args, "encounter_id")?;
                    let actor_id = req_str(&args, "actor_id")?;
                    let action_type =
                        opt_str(&args, "action_type").unwrap_or_else(|| "action".to_string());
                    let description = req_str(&args, "description")?;
                    let target_ids = opt_str_vec(&args, "target_ids");
                    let new_pos = opt_str(&args, "new_position");
                    let new_range = opt_str(&args, "new_range_band");
                    let res_deltas: Vec<(String, String, i64)> = args
                        .get("resource_deltas")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|item| {
                                    Some((
                                        item.get("character_id")?.as_str()?.to_string(),
                                        item.get("resource")?.as_str()?.to_string(),
                                        item.get("delta")?.as_i64()?,
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let add_conds: Vec<(String, StatusCondition)> = args
                        .get("add_conditions")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|item| {
                                    let cid = item.get("character_id")?.as_str()?.to_string();
                                    let cond: StatusCondition =
                                        serde_json::from_value(item.get("condition")?.clone()).ok()?;
                                    Some((cid, cond))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let rem_conds: Vec<(String, String)> = args
                        .get("remove_conditions")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|item| {
                                    Some((
                                        item.get("character_id")?.as_str()?.to_string(),
                                        item.get("condition_name")?.as_str()?.to_string(),
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let advance_turn = args
                        .get("advance_turn")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let causal_ids = opt_i64_vec(&args, "causal_event_ids");
                    let (enc, ev) = engine.resolve_encounter_action(
                        &enc_id,
                        &actor_id,
                        &action_type,
                        &description,
                        target_ids,
                        new_pos,
                        new_range,
                        res_deltas,
                        add_conds,
                        rem_conds,
                        advance_turn,
                        causal_ids,
                    )?;
                    Ok(json!({
                        "event_seq": ev.seq,
                        "summary": ev.summary,
                        "encounter": enc
                    }))
                }
                "advance_turn" => {
                    let enc_id = req_str(&args, "encounter_id")?;
                    let mins = args.get("advance_world_minutes").and_then(|v| v.as_u64());
                    let (enc, next) = engine.advance_encounter_turn(&enc_id, mins)?;
                    Ok(json!({
                        "next_actor_id": next,
                        "round": enc.round,
                        "encounter": enc
                    }))
                }
                "update_participant" => {
                    let enc_id = req_str(&args, "encounter_id")?;
                    let part: EncounterParticipant = args
                        .get("participant")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .ok_or_else(|| anyhow!("Missing valid 'participant' object"))?;
                    let enc = engine.update_encounter_participant(&enc_id, part)?;
                    Ok(json!({ "encounter": enc }))
                }
                "get" => {
                    let enc_id = req_str(&args, "encounter_id")?;
                    let enc = engine.get_encounter(&enc_id)?;
                    Ok(serde_json::to_value(enc)?)
                }
                "list" => {
                    let list = engine.list_encounters(opt_str(&args, "status").as_deref())?;
                    Ok(json!({ "count": list.len(), "encounters": list }))
                }
                "end" => {
                    let enc_id = req_str(&args, "encounter_id")?;
                    let outcome = opt_str(&args, "outcome").unwrap_or_else(|| "Resolved".to_string());
                    let aftermath = opt_str(&args, "aftermath").unwrap_or_default();
                    let (enc, ev) = engine.end_encounter(&enc_id, &outcome, &aftermath)?;
                    Ok(json!({
                        "event_seq": ev.seq,
                        "summary": ev.summary,
                        "encounter": enc
                    }))
                }
                other => Err(anyhow!(
                    "Unknown manage_encounter action '{other}'. Expected: start, act, advance_turn, update_participant, get, list, end."
                )),
            }
        }

        // ---------------------------------------------------------------------
        // Phase 2: Character Profiles, Epistemic Knowledge, Factions & Autonomy Sandbox
        // ---------------------------------------------------------------------
        "update_character_profile" => {
            let cid = req_str(&args, "character_id")?;
            if args.get("get_only").and_then(|v| v.as_bool()).unwrap_or(false) {
                let prof = engine.get_character_profile(&cid)?;
                return Ok(serde_json::to_value(prof)?);
            }
            let (prof, ev) = engine.update_character_profile(&cid, args)?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "profile": prof
            }))
        }

        "record_knowledge" => {
            let claim = KnowledgeClaim {
                id: opt_str(&args, "claim_id")
                    .or_else(|| opt_str(&args, "id"))
                    .unwrap_or_default(),
                subject: req_str(&args, "subject")?,
                claim: req_str(&args, "claim")?,
                truth_status: opt_str(&args, "truth_status").unwrap_or_else(|| "true".to_string()),
                visibility: opt_str(&args, "visibility")
                    .unwrap_or_else(|| "restricted".to_string()),
                origin_source: opt_str(&args, "source")
                    .or_else(|| opt_str(&args, "origin_source"))
                    .unwrap_or_default(),
                source_event_id: args.get("source_event_id").and_then(|v| v.as_i64()),
                superseded_by_id: None,
                tags: opt_str_vec(&args, "tags"),
                related_ids: opt_str_vec(&args, "related_ids"),
                created_at_world_time: String::new(),
                updated_at_world_time: String::new(),
            };

            let mut beliefs: Vec<ActorBelief> = args
                .get("beliefs")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            // Convenient shorthand: known_by + belief/stance + confidence + perceived_claim
            let default_stance = opt_str(&args, "stance")
                .or_else(|| opt_str(&args, "belief"))
                .unwrap_or_else(|| "observed".to_string());
            let default_conf = args
                .get("confidence")
                .and_then(|v| v.as_u64())
                .unwrap_or(80)
                .min(100) as u8;
            let default_perceived = opt_str(&args, "perceived_claim").unwrap_or_default();
            let default_source = opt_str(&args, "source").unwrap_or_default();
            let default_concealing = args
                .get("concealing")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            for actor in opt_str_vec(&args, "known_by") {
                if !beliefs.iter().any(|b| slugify(&b.actor_id) == slugify(&actor)) {
                    beliefs.push(ActorBelief {
                        claim_id: String::new(),
                        actor_id: actor,
                        stance: default_stance.clone(),
                        confidence: default_conf,
                        perceived_claim: default_perceived.clone(),
                        source: default_source.clone(),
                        source_trustworthy: args.get("source_trustworthy").and_then(|v| v.as_bool()),
                        concealing: default_concealing,
                        outdated: false,
                        learned_at_world_time: opt_str(&args, "learned_at").unwrap_or_default(),
                        notes: opt_str(&args, "notes").unwrap_or_default(),
                    });
                }
            }
            for actor in opt_str_vec(&args, "disbelieved_by") {
                beliefs.push(ActorBelief {
                    claim_id: String::new(),
                    actor_id: actor,
                    stance: "disbelieved".to_string(),
                    confidence: 10,
                    perceived_claim: default_perceived.clone(),
                    source: default_source.clone(),
                    source_trustworthy: Some(false),
                    concealing: false,
                    outdated: false,
                    learned_at_world_time: opt_str(&args, "learned_at").unwrap_or_default(),
                    notes: opt_str(&args, "notes").unwrap_or_default(),
                });
            }

            let supersedes = opt_str(&args, "supersedes_claim_id");
            let causal = opt_i64_vec(&args, "causal_event_ids");
            let (view, ev) = engine.record_knowledge(claim, beliefs, supersedes, causal)?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "knowledge": view
            }))
        }

        "propagate_knowledge" => {
            let claim_id = req_str(&args, "claim_id")?;
            let from_actor = opt_str(&args, "from_actor_id");
            let to_actors = opt_str_vec(&args, "to_actor_ids");
            let stance = opt_str(&args, "stance");
            let decay = args
                .get("confidence_decay")
                .and_then(|v| v.as_u64())
                .unwrap_or(15)
                .min(95) as u8;
            let distorted = opt_str(&args, "distorted_claim");
            let deceptive = args
                .get("deceptive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let notes = opt_str(&args, "notes").unwrap_or_default();
            let (beliefs, ev) = engine.propagate_knowledge(
                &claim_id,
                from_actor,
                to_actors,
                stance,
                decay,
                distorted,
                deceptive,
                &notes,
            )?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "propagated_beliefs": beliefs
            }))
        }

        "search_knowledge" => {
            let filter: KnowledgeSearchFilter = serde_json::from_value(args)?;
            let records = engine.search_knowledge(&filter)?;
            Ok(json!({ "count": records.len(), "records": records }))
        }

        "manage_faction" => {
            let action = req_str(&args, "action")?.to_lowercase();
            match action.as_str() {
                "create" | "update" => {
                    let notes = opt_str(&args, "reason").unwrap_or_default();
                    let faction: FactionRecord = serde_json::from_value(args)?;
                    let (saved, ev) = engine.upsert_faction(faction, &notes)?;
                    Ok(json!({
                        "event_seq": ev.seq,
                        "doc_path": engine.docs_dir().join("factions").join(format!("{}.md", saved.id)).display().to_string(),
                        "faction": saved
                    }))
                }
                "get" => {
                    let fid = req_str(&args, "faction_id")
                        .or_else(|_| req_str(&args, "id"))?;
                    let fac = engine.get_faction(&fid)?;
                    let rels = engine.db().list_relations_for_entity(&fac.id)?;
                    let histories = engine.db().list_histories_for_entity(&fac.id)?;
                    let events = engine.db().list_events_for_entity(&fac.id)?;
                    Ok(json!({
                        "doc_path": engine.docs_dir().join("factions").join(format!("{}.md", fac.id)).display().to_string(),
                        "faction": fac,
                        "relations": rels,
                        "histories": histories,
                        "event_log": events
                    }))
                }
                "list" => {
                    let list = engine.list_factions()?;
                    Ok(json!({ "count": list.len(), "factions": list }))
                }
                "add_member" => {
                    let fid = req_str(&args, "faction_id")?;
                    let member = FactionMember {
                        character_id: req_str(&args, "character_id")?,
                        rank: opt_str(&args, "rank").unwrap_or_else(|| "Member".to_string()),
                        role: opt_str(&args, "role").unwrap_or_default(),
                        loyalty_note: opt_str(&args, "loyalty_note").unwrap_or_default(),
                    };
                    let (fac, ev) = engine.add_faction_member(&fid, member)?;
                    Ok(json!({ "event_seq": ev.seq, "faction": fac }))
                }
                "remove_member" => {
                    let fid = req_str(&args, "faction_id")?;
                    let cid = req_str(&args, "character_id")?;
                    let reason = opt_str(&args, "reason").unwrap_or_default();
                    let (fac, ev) = engine.remove_faction_member(&fid, &cid, &reason)?;
                    Ok(json!({ "event_seq": ev.seq, "faction": fac }))
                }
                "modify_resource" => {
                    let fid = req_str(&args, "faction_id")?;
                    let res = req_str(&args, "resource")?;
                    let delta = args
                        .get("delta")
                        .and_then(|v| v.as_i64())
                        .ok_or_else(|| anyhow!("Missing integer 'delta'"))?;
                    let allow_debt = args
                        .get("allow_debt")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let reason = opt_str(&args, "reason").unwrap_or_default();
                    let (fac, ev) =
                        engine.modify_faction_resource(&fid, &res, delta, allow_debt, &reason)?;
                    Ok(json!({ "event_seq": ev.seq, "faction": fac }))
                }
                "promote_from_lore" => {
                    let lore_id = req_str(&args, "lore_id")?;
                    let ftype = opt_str(&args, "faction_type");
                    let tier = opt_str(&args, "capacity_tier");
                    let remove_old = args
                        .get("remove_old_lore")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let (fac, ev) =
                        engine.promote_lore_to_faction(&lore_id, ftype, tier, remove_old)?;
                    Ok(json!({ "event_seq": ev.seq, "faction": fac }))
                }
                other => Err(anyhow!(
                    "Unknown manage_faction action '{other}'. Expected: create, update, get, list, add_member, remove_member, modify_resource, promote_from_lore."
                )),
            }
        }

        "simulate_autonomy" => {
            let mode = opt_str(&args, "mode").unwrap_or_else(|| "prepare".to_string()).to_lowercase();
            let scope_type = opt_str(&args, "scope_type").unwrap_or_else(|| "npc".to_string());
            let duration_hours = args
                .get("duration_hours")
                .and_then(|v| v.as_u64())
                .or_else(|| args.get("duration_days").and_then(|v| v.as_u64()).map(|d| d * 24))
                .unwrap_or(24);
            match mode.as_str() {
                "prepare" => {
                    let fid = opt_str(&args, "faction_id");
                    let aids = opt_str_vec(&args, "actor_ids");
                    let loc = opt_str(&args, "location_id");
                    let dossier = engine.prepare_autonomy_dossier(
                        &scope_type,
                        fid.as_deref(),
                        &aids,
                        duration_hours,
                        loc.as_deref(),
                    )?;
                    Ok(serde_json::to_value(dossier)?)
                }
                "commit" | "auto_mechanical" => {
                    let mut req: AutonomyCommitRequest = serde_json::from_value(args.clone())?;
                    req.scope_type = scope_type;
                    req.duration_hours = duration_hours;
                    if mode == "auto_mechanical" && req.faction_resource_deltas.is_empty() {
                        // Auto-apply deterministic upkeep if faction has a "grain" or "rations" resource
                        if let Some(ref fid) = req.faction_id {
                            let dossier = engine.prepare_autonomy_dossier(
                                &req.scope_type,
                                Some(fid),
                                &req.actor_ids,
                                duration_hours,
                                req.location_id.as_deref(),
                            )?;
                            if let Some(ref fac) = dossier.faction {
                                if let Some(&drain) = dossier.projected_upkeep.get("daily_rations") {
                                    for key in ["rations", "grain", "food", "supplies"] {
                                        if fac.resources.contains_key(key) {
                                            req.faction_resource_deltas.push(
                                                crate::autonomy::FactionResourceDelta {
                                                    resource: key.to_string(),
                                                    delta: -drain,
                                                },
                                            );
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    let (res, _) = engine.commit_autonomy_turn(req)?;
                    Ok(res)
                }
                other => Err(anyhow!(
                    "Unknown simulate_autonomy mode '{other}'. Expected 'prepare', 'commit', or 'auto_mechanical'."
                )),
            }
        }

        // ---------------------------------------------------------------------
        // Phase 3 & Phase 4: Orthogonal Lifecycle Primitives
        // ---------------------------------------------------------------------
        "manage_travel_and_exploration" => {
            let action = req_str(&args, "action")?.to_lowercase();
            match action.as_str() {
                "upsert_route" => {
                    let route: TravelRoute = serde_json::from_value(args)?;
                    let (saved, ev) = engine.upsert_travel_route(route)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "route": saved }))
                }
                "list_routes" => {
                    let routes = engine.all_effective_routes()?;
                    Ok(json!({ "count": routes.len(), "routes": routes }))
                }
                "update_spatial" => {
                    let sp: PlaceSpatialExt = serde_json::from_value(args)?;
                    let saved = engine.update_place_spatial(sp)?;
                    Ok(json!({ "spatial": saved }))
                }
                "calculate" => {
                    let from = req_str(&args, "from_place_id")?;
                    let to = req_str(&args, "to_place_id")?;
                    let chars = opt_str_vec(&args, "character_ids");
                    let mode = opt_str(&args, "transport_mode").unwrap_or_else(|| "foot".to_string());
                    let forced = args.get("forced_march").and_then(|v| v.as_bool()).unwrap_or(false);
                    let plan = engine.calculate_travel(&from, &to, &chars, &mode, forced)?;
                    Ok(serde_json::to_value(plan)?)
                }
                "advance" => {
                    let from = req_str(&args, "from_place_id")?;
                    let to = req_str(&args, "to_place_id")?;
                    let chars = opt_str_vec(&args, "character_ids");
                    let mode = opt_str(&args, "transport_mode").unwrap_or_else(|| "foot".to_string());
                    let forced = args.get("forced_march").and_then(|v| v.as_bool()).unwrap_or(false);
                    let pay_from = opt_str(&args, "pay_tolls_from");
                    let haz_tbl = opt_str(&args, "hazard_table_id");
                    let seed = args.get("seed").and_then(|v| v.as_u64());
                    let (plan, haz, ev) = engine.advance_travel(
                        &from,
                        &to,
                        &chars,
                        &mode,
                        forced,
                        pay_from.as_deref(),
                        haz_tbl.as_deref(),
                        seed,
                    )?;
                    Ok(json!({
                        "event_seq": ev.seq,
                        "summary": ev.summary,
                        "plan": plan,
                        "hazard_draw": haz
                    }))
                }
                "explore" => {
                    let pid = req_str(&args, "place_id")?;
                    let actor = opt_str(&args, "actor_id");
                    let mode = opt_str(&args, "mode").unwrap_or_else(|| "scout".to_string());
                    let skill = opt_str(&args, "skill");
                    let diff = args.get("difficulty").and_then(|v| v.as_i64());
                    let tbl = opt_str(&args, "table_id");
                    let seed = args.get("seed").and_then(|v| v.as_u64());
                    engine.explore_area(
                        &pid,
                        actor.as_deref(),
                        &mode,
                        skill.as_deref(),
                        diff,
                        tbl.as_deref(),
                        seed,
                    )
                }
                other => Err(anyhow!(
                    "Unknown manage_travel_and_exploration action '{other}'. Expected: upsert_route, list_routes, update_spatial, calculate, advance, explore."
                )),
            }
        }

        "manage_economy" => {
            let action = req_str(&args, "action")?.to_lowercase();
            match action.as_str() {
                "set_item_weight" => {
                    let econ: ItemEconomics = serde_json::from_value(args)?;
                    let saved = engine.set_item_economics(econ)?;
                    Ok(json!({ "item_economics": saved }))
                }
                "check_encumbrance" => {
                    let cid = req_str(&args, "character_id")?;
                    let rep = engine.check_character_encumbrance(&cid)?;
                    Ok(serde_json::to_value(rep)?)
                }
                "get_balances" => {
                    let oid = slugify(&req_str(&args, "owner_id")?);
                    let balances = engine.db().list_currency_balances(&oid)?;
                    Ok(json!({ "owner_id": oid, "balances": balances }))
                }
                "set_balance" => {
                    let oid = req_str(&args, "owner_id")?;
                    let curr = opt_str(&args, "currency").unwrap_or_else(|| "copper_cash".to_string());
                    let amt = args
                        .get("amount")
                        .and_then(|v| v.as_i64())
                        .ok_or_else(|| anyhow!("Missing integer 'amount'"))?;
                    let reason = opt_str(&args, "reason").unwrap_or_default();
                    let (bal, ev) = engine.set_currency_balance(&oid, &curr, amt, &reason)?;
                    Ok(json!({ "event_seq": ev.seq, "balance": bal, "currency": curr }))
                }
                "transfer_currency" => {
                    let from = req_str(&args, "from_id")?;
                    let to = req_str(&args, "to_id")?;
                    let amt = args
                        .get("amount")
                        .and_then(|v| v.as_i64())
                        .ok_or_else(|| anyhow!("Missing integer 'amount'"))?;
                    let curr = opt_str(&args, "currency").unwrap_or_else(|| "copper_cash".to_string());
                    let reason = opt_str(&args, "reason").unwrap_or_default();
                    let (fb, tb, ev) = engine.transfer_currency(&from, &to, amt, &curr, &reason)?;
                    Ok(json!({
                        "event_seq": ev.seq,
                        "summary": ev.summary,
                        "from_balance": fb,
                        "to_balance": tb,
                        "currency": curr
                    }))
                }
                "execute_transaction" => {
                    let kind = opt_str(&args, "kind").unwrap_or_else(|| "purchase".to_string());
                    let buyer = req_str(&args, "buyer_id").or_else(|_| req_str(&args, "from_id"))?;
                    let seller = req_str(&args, "seller_id").or_else(|_| req_str(&args, "to_id"))?;
                    let amt = args
                        .get("amount")
                        .and_then(|v| v.as_i64())
                        .ok_or_else(|| anyhow!("Missing integer 'amount'"))?;
                    let curr = opt_str(&args, "currency").unwrap_or_else(|| "copper_cash".to_string());
                    let tax = args.get("tax_rate_pct").and_then(|v| v.as_i64()).unwrap_or(0);
                    let tax_to = opt_str(&args, "tax_recipient_id");
                    let item_name = opt_str(&args, "item_name");
                    let qty = args.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1);
                    let notes = opt_str(&args, "notes").unwrap_or_default();
                    let (res, _) = engine.execute_transaction(
                        &kind,
                        &buyer,
                        &seller,
                        amt,
                        &curr,
                        tax,
                        tax_to.as_deref(),
                        item_name.as_deref(),
                        qty,
                        &notes,
                    )?;
                    Ok(res)
                }
                "upsert_market" => {
                    let mkt: SettlementMarket = serde_json::from_value(args)?;
                    let (saved, ev) = engine.upsert_market(mkt)?;
                    Ok(json!({
                        "event_seq": ev.seq,
                        "doc_path": engine.docs_dir().join("economy").join("markets.md").display().to_string(),
                        "market": saved
                    }))
                }
                "get_market_prices" => {
                    let pid = req_str(&args, "place_id")?;
                    let report = engine.get_market_prices(&pid)?;
                    Ok(serde_json::to_value(report)?)
                }
                "update_market_supply" => {
                    let pid = req_str(&args, "place_id")?;
                    let comm = req_str(&args, "commodity")?;
                    let s_delta = args.get("supply_delta").and_then(|v| v.as_i64()).unwrap_or(0);
                    let d_delta = args.get("demand_delta").and_then(|v| v.as_i64()).unwrap_or(0);
                    let reason = opt_str(&args, "reason").unwrap_or_default();
                    let (report, ev) =
                        engine.update_market_supply(&pid, &comm, s_delta, d_delta, &reason)?;
                    Ok(json!({ "event_seq": ev.seq, "market_prices": report }))
                }
                other => Err(anyhow!(
                    "Unknown manage_economy action '{other}'. Expected: set_item_weight, check_encumbrance, get_balances, set_balance, transfer_currency, execute_transaction, upsert_market, get_market_prices, update_market_supply."
                )),
            }
        }

        "manage_law_and_reputation" => {
            let action = req_str(&args, "action")?.to_lowercase();
            match action.as_str() {
                "upsert_jurisdiction" => {
                    let j: JurisdictionRecord = serde_json::from_value(args)?;
                    let (saved, ev) = engine.upsert_jurisdiction(j)?;
                    Ok(json!({ "event_seq": ev.seq, "jurisdiction": saved }))
                }
                "check_jurisdiction" => {
                    let pid = opt_str(&args, "place_id").map(|p| slugify(&p));
                    let all = engine.db().list_jurisdictions()?;
                    let matched: Vec<_> = match pid {
                        Some(ref p) => all
                            .into_iter()
                            .filter(|j| j.place_ids.contains(p) || j.id == *p)
                            .collect(),
                        None => all,
                    };
                    let crimes = engine.db().list_crimes(
                        opt_str(&args, "suspect_id").map(|s| slugify(&s)).as_deref(),
                        matched.first().map(|j| j.id.as_str()),
                    )?;
                    Ok(json!({ "jurisdictions": matched, "crimes": crimes }))
                }
                "record_crime" => {
                    let c: CrimeRecord = serde_json::from_value(args)?;
                    let (saved, ev) = engine.record_crime(c)?;
                    Ok(json!({ "event_seq": ev.seq, "crime": saved }))
                }
                "update_warrant" => {
                    let cid = slugify(&req_str(&args, "crime_id")?);
                    let mut c = engine
                        .db()
                        .get_crime(&cid)?
                        .ok_or_else(|| anyhow!("Crime record '{cid}' not found"))?;
                    if let Some(st) = opt_str(&args, "warrant_status") {
                        c.warrant_status = st;
                    }
                    if let Some(b) = args.get("bounty").and_then(|v| v.as_i64()) {
                        c.bounty = b;
                    }
                    if let Some(n) = opt_str(&args, "notes") {
                        c.notes = n;
                    }
                    let (saved, ev) = engine.record_crime(c)?;
                    Ok(json!({ "event_seq": ev.seq, "crime": saved }))
                }
                "modify_reputation" => {
                    let eid = req_str(&args, "entity_id")?;
                    let tid = req_str(&args, "target_group_id")?;
                    let delta = args
                        .get("delta")
                        .and_then(|v| v.as_i64())
                        .ok_or_else(|| anyhow!("Missing integer 'delta'"))?;
                    let notes = opt_str(&args, "notes").unwrap_or_default();
                    let (rep, ev) = engine.modify_reputation(&eid, &tid, delta, &notes)?;
                    Ok(json!({ "event_seq": ev.seq, "reputation": rep }))
                }
                "get_reputation" => {
                    let eid = opt_str(&args, "entity_id").map(|s| slugify(&s));
                    let tid = opt_str(&args, "target_group_id").map(|s| slugify(&s));
                    let list = engine.db().list_reputations(eid.as_deref(), tid.as_deref())?;
                    Ok(json!({ "count": list.len(), "standings": list }))
                }
                other => Err(anyhow!(
                    "Unknown manage_law_and_reputation action '{other}'. Expected: upsert_jurisdiction, check_jurisdiction, record_crime, update_warrant, modify_reputation, get_reputation."
                )),
            }
        }

        "manage_health_and_downtime" => {
            let action = req_str(&args, "action")?.to_lowercase();
            match action.as_str() {
                "apply_injury" => {
                    let inj = InjuryRecord {
                        id: opt_str(&args, "injury_id").unwrap_or_default(),
                        character_id: req_str(&args, "character_id")?,
                        name: req_str(&args, "name")?,
                        severity: opt_str(&args, "severity").unwrap_or_else(|| "moderate".to_string()),
                        body_location: opt_str(&args, "body_location").unwrap_or_default(),
                        complications: opt_str_vec(&args, "complications"),
                        treated: false,
                        recovery_days_total: args.get("recovery_days").and_then(|v| v.as_i64()).unwrap_or(0),
                        recovery_days_remaining: args.get("recovery_days").and_then(|v| v.as_i64()).unwrap_or(0),
                        status: "active".to_string(),
                        source_event_id: args.get("source_event_id").and_then(|v| v.as_i64()),
                        notes: opt_str(&args, "notes").unwrap_or_default(),
                        inflicted_at_world_time: String::new(),
                        updated_at_world_time: String::new(),
                    };
                    let (saved, ev) = engine.apply_injury(inj)?;
                    Ok(json!({ "event_seq": ev.seq, "injury": saved }))
                }
                "treat_injury" => {
                    let iid = req_str(&args, "injury_id")?;
                    let notes = opt_str(&args, "notes").unwrap_or_default();
                    let reduced = args.get("days_reduced").and_then(|v| v.as_i64()).unwrap_or(1);
                    let (saved, ev) = engine.treat_injury(&iid, &notes, reduced)?;
                    Ok(json!({ "event_seq": ev.seq, "injury": saved }))
                }
                "advance_recovery" => {
                    let cid = req_str(&args, "character_id")?;
                    let days = args.get("days").and_then(|v| v.as_i64()).unwrap_or(1);
                    let updated = engine.advance_injury_recovery(&cid, days)?;
                    Ok(json!({ "count": updated.len(), "injuries": updated }))
                }
                "list_injuries" => {
                    let cid = opt_str(&args, "character_id").map(|s| slugify(&s));
                    let active_only = args.get("active_only").and_then(|v| v.as_bool()).unwrap_or(true);
                    let list = engine.db().list_injuries(cid.as_deref(), active_only)?;
                    Ok(json!({ "count": list.len(), "injuries": list }))
                }
                "resolve_downtime" => {
                    let cid = req_str(&args, "character_id")?;
                    let activity = req_str(&args, "activity")?;
                    let days = args.get("days").and_then(|v| v.as_u64()).unwrap_or(1);
                    let (_, _, tev) = engine.advance_time(
                        days * 1440,
                        None,
                        Some(format!("Downtime ({days}d) for {cid}: {activity}")),
                    )?;
                    let mut check_opt = None;
                    if let Some(dc) = args.get("difficulty").and_then(|v| v.as_i64()) {
                        check_opt = Some(engine.resolve_check(CheckRequest {
                            actor_id: Some(cid.clone()),
                            action: format!("Downtime: {activity}"),
                            profile: opt_str(&args, "profile"),
                            stat: opt_str(&args, "stat"),
                            skill: opt_str(&args, "skill"),
                            technique: opt_str(&args, "technique"),
                            equipment: vec![],
                            difficulty: dc,
                            modifier: args.get("modifier").and_then(|v| v.as_i64()).unwrap_or(0),
                            situational_modifiers: vec![],
                            advantage: "none".to_string(),
                            dice_override: None,
                            auto_hydrate: Some(true),
                            stakes: opt_str(&args, "stakes").unwrap_or_default(),
                            seed: args.get("seed").and_then(|v| v.as_u64()),
                            dry_run: false,
                            strict_validation: false,
                            log_to_ledger: true,
                            causal_event_ids: vec![tev.seq],
                            visibility: "public".to_string(),
                        })?);
                    }
                    let recovered = engine.advance_injury_recovery(&cid, days as i64)?;
                    Ok(json!({
                        "time_event_seq": tev.seq,
                        "days": days,
                        "activity": activity,
                        "check": check_opt,
                        "injuries_updated": recovered
                    }))
                }
                "begin_crafting" => {
                    let proj: CraftingProject = serde_json::from_value(args)?;
                    let (saved, ev) = engine.begin_crafting(proj)?;
                    Ok(json!({ "event_seq": ev.seq, "project": saved }))
                }
                "resolve_crafting" => {
                    let pid = req_str(&args, "project_id")?;
                    let hrs = args.get("hours_worked").and_then(|v| v.as_u64()).unwrap_or(4);
                    let prof = opt_str(&args, "profile");
                    let seed = args.get("seed").and_then(|v| v.as_u64());
                    let (saved, check, ev) =
                        engine.resolve_crafting(&pid, hrs, prof.as_deref(), seed)?;
                    Ok(json!({ "event_seq": ev.seq, "project": saved, "check": check }))
                }
                "resolve_progression" => {
                    // System-agnostic progression check (e.g. cultivation breakthrough, body tempering, rank trial)
                    let cid = req_str(&args, "character_id")?;
                    let kind = opt_str(&args, "progression_kind")
                        .unwrap_or_else(|| "breakthrough".to_string());
                    let dc = args.get("difficulty").and_then(|v| v.as_i64()).unwrap_or(15);
                    let check = engine.resolve_check(CheckRequest {
                        actor_id: Some(cid.clone()),
                        action: format!("Progression ({kind})"),
                        profile: opt_str(&args, "profile"),
                        stat: opt_str(&args, "stat"),
                        skill: opt_str(&args, "skill"),
                        technique: opt_str(&args, "technique"),
                        equipment: opt_str_vec(&args, "equipment"),
                        difficulty: dc,
                        modifier: args.get("modifier").and_then(|v| v.as_i64()).unwrap_or(0),
                        situational_modifiers: parse_modifier_sources(args.get("situational_modifiers")),
                        advantage: opt_str(&args, "advantage").unwrap_or_else(|| "none".to_string()),
                        dice_override: opt_str(&args, "dice_override"),
                        auto_hydrate: Some(true),
                        stakes: opt_str(&args, "stakes")
                            .unwrap_or_else(|| "Advancement vs bottleneck / backlash".to_string()),
                        seed: args.get("seed").and_then(|v| v.as_u64()),
                        dry_run: false,
                        strict_validation: false,
                        log_to_ledger: true,
                        causal_event_ids: opt_i64_vec(&args, "causal_event_ids"),
                        visibility: "public".to_string(),
                    })?;
                    let ev = engine.db().append_event_causal(
                        check.location_id.clone(),
                        vec![slugify(&cid)],
                        format!(
                            "Progression [{kind}] for {cid}: {} ({})",
                            check.outcome, check.degree
                        ),
                        EventPayload::ProgressionResolved {
                            actor_id: slugify(&cid),
                            progression_kind: kind,
                            outcome: check.outcome.clone(),
                            notes: check.degree.clone(),
                        },
                        EventCausalMeta {
                            actor_id: Some(slugify(&cid)),
                            causal_event_ids: check.event_id.into_iter().collect(),
                            visibility: "public".to_string(),
                        },
                    )?;
                    Ok(json!({ "event_seq": ev.seq, "check": check }))
                }
                other => Err(anyhow!(
                    "Unknown manage_health_and_downtime action '{other}'. Expected: apply_injury, treat_injury, advance_recovery, list_injuries, resolve_downtime, begin_crafting, resolve_crafting, resolve_progression."
                )),
            }
        }

        "manage_weather" => {
            let action = opt_str(&args, "action")
                .unwrap_or_else(|| "advance".to_string())
                .to_lowercase();
            let pid = req_str(&args, "place_id")?;
            match action.as_str() {
                "generate" | "advance" => {
                    let seed = args.get("seed").and_then(|v| v.as_u64());
                    let (w, ev) = engine.advance_place_weather(&pid, seed)?;
                    Ok(json!({ "event_seq": ev.seq, "place_id": pid, "weather": w }))
                }
                "get" => {
                    let pl = engine.get_place(&pid, None)?;
                    Ok(json!({
                        "place_id": pl.baseline.id,
                        "season": pl.current.current_season,
                        "climate": pl.baseline.climate,
                        "weather": pl.current.current_weather
                    }))
                }
                other => Err(anyhow!(
                    "Unknown manage_weather action '{other}'. Expected: generate, advance, get."
                )),
            }
        }

        "advance_world" => {
            let hours = args
                .get("duration_hours")
                .and_then(|v| v.as_u64())
                .or_else(|| args.get("duration_days").and_then(|v| v.as_u64()).map(|d| d * 24))
                .unwrap_or(24);
            let places = opt_str_vec(&args, "place_ids");
            let factions = opt_str_vec(&args, "faction_ids");
            let chars = opt_str_vec(&args, "character_ids");
            let tick_weather = args.get("tick_weather").and_then(|v| v.as_bool()).unwrap_or(true);
            let tick_recovery = args.get("tick_recovery").and_then(|v| v.as_bool()).unwrap_or(true);
            let tick_markets = args.get("tick_markets").and_then(|v| v.as_bool()).unwrap_or(true);
            let tick_upkeep = args.get("tick_upkeep").and_then(|v| v.as_bool()).unwrap_or(true);
            let prep_dossiers = args
                .get("prepare_autonomy_dossiers")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let seed = args.get("seed").and_then(|v| v.as_u64());
            engine.advance_world(
                hours,
                &places,
                &factions,
                &chars,
                tick_weather,
                tick_recovery,
                tick_markets,
                tick_upkeep,
                prep_dossiers,
                seed,
            )
        }

        "roll_dice" => {
            let expr = req_str(&args, "expression")?;
            let actor_id = opt_str(&args, "actor_id");
            let location_id = opt_str(&args, "location_id");
            let purpose = opt_str(&args, "purpose").unwrap_or_default();
            let log_to_ledger = args
                .get("log_to_ledger")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let seed = args.get("seed").and_then(|v| v.as_u64());
            let (res, ev) = engine.roll_and_optionally_log(
                &expr,
                actor_id,
                location_id,
                &purpose,
                log_to_ledger,
                seed,
            )?;
            Ok(json!({
                "roll": res,
                "event_seq": ev.map(|e| e.seq)
            }))
        }

        "search_history" | "search_events" => {
            let filter: EventSearchFilter = serde_json::from_value(args)?;
            let events = engine.search_events_by(&filter)?;
            Ok(json!({ "count": events.len(), "events": events }))
        }

        "rollback_to_event" => {
            let to_seq = args
                .get("to_seq")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| anyhow!("Missing 'to_seq' (integer event sequence)"))?;
            let (backup, report) = engine.rollback_to_seq(to_seq)?;
            Ok(json!({
                "rolled_back_to_seq": to_seq,
                "backup_db_path": backup.display().to_string(),
                "recompiled_docs": {
                    "characters": report.character_files,
                    "places": report.place_files,
                    "lore": report.lore_files,
                    "events": report.total_events
                }
            }))
        }

        "compile_docs" => {
            let report = engine.compile_docs()?;
            Ok(json!({
                "docs_dir": report.docs_dir.display().to_string(),
                "character_files": report.character_files,
                "place_files": report.place_files,
                "lore_files": report.lore_files,
                "text_files": report.text_files,
                "item_files": report.item_files,
                "attachment_files": report.attachment_files,
                "total_events": report.total_events
            }))
        }

        _ => Err(anyhow!("Unknown tool: {name}")),
    }
}

fn handle_resources_list(engine: &CampaignEngine) -> Result<Value> {
    let mut resources = vec![
        json!({
            "uri": "chronicle://index",
            "name": "Campaign Index",
            "description": "Master campaign overview, world clock, active clocks, and directory of all documents.",
            "mimeType": "text/markdown"
        }),
        json!({
            "uri": "chronicle://timeline",
            "name": "Master Campaign Timeline",
            "description": "Full chronological event ledger across all sessions.",
            "mimeType": "text/markdown"
        }),
    ];

    for c in engine.db().list_character_baselines()? {
        resources.push(json!({
            "uri": format!("chronicle://characters/{}", c.id),
            "name": format!("Character: {}", c.name),
            "description": format!("Initial profile, current projected status, and event log for {} ({}).", c.name, c.kind),
            "mimeType": "text/markdown"
        }));
    }
    for p in engine.db().list_place_baselines()? {
        resources.push(json!({
            "uri": format!("chronicle://places/{}", p.id),
            "name": format!("Place: {}", p.name),
            "description": format!("Terrain, seasonal profile, landmarks, current status, and history for {}.", p.name),
            "mimeType": "text/markdown"
        }));
    }
    for l in engine.db().list_lore(None)? {
        resources.push(json!({
            "uri": format!("chronicle://lore/{}", l.id),
            "name": format!("Lore [{}]: {}", l.category, l.title),
            "description": l.summary,
            "mimeType": "text/markdown"
        }));
    }
    for t in engine.db().list_texts()? {
        resources.push(json!({
            "uri": format!("chronicle://texts/{}", t.id),
            "name": format!("Text [{} / {}]: {}", t.medium, t.subject, t.title),
            "description": format!("{} ({}, status: {})", t.summary, t.language, t.read_status),
            "mimeType": "text/markdown"
        }));
    }
    for a in engine.db().list_attachments()? {
        resources.push(json!({
            "uri": format!("chronicle://attachments/{}", a.id),
            "name": format!("Attached Doc [{}]: {} ({})", a.category, a.title, a.original_filename),
            "description": format!("{} — copied at {}", a.summary, a.relative_path),
            "mimeType": "text/markdown"
        }));
    }
    for it in engine.db().list_catalog_items()? {
        resources.push(json!({
            "uri": format!("chronicle://items/{}", it.id),
            "name": format!("Item [{}]: {}", it.item_type, it.name),
            "description": format!("Holder: {} | {}", it.current_holder_id.as_deref().unwrap_or("unclaimed"), it.significance),
            "mimeType": "text/markdown"
        }));
    }

    Ok(json!({ "resources": resources }))
}

fn handle_resources_read(engine: &CampaignEngine, params: Value) -> Result<Value> {
    let uri = params
        .get("uri")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing resource uri"))?;

    // Ensure docs are up-to-date before reading
    let _ = engine.compile_docs()?;
    let docs = engine.docs_dir();

    let path = if uri == "chronicle://index" {
        docs.join("INDEX.md")
    } else if uri == "chronicle://timeline" {
        docs.join("TIMELINE.md")
    } else if let Some(id) = uri.strip_prefix("chronicle://characters/") {
        docs.join("characters").join(format!("{}.md", slugify(id)))
    } else if let Some(id) = uri.strip_prefix("chronicle://places/") {
        docs.join("places").join(format!("{}.md", slugify(id)))
    } else if let Some(id) = uri.strip_prefix("chronicle://lore/") {
        let entry = engine.get_lore(id)?;
        docs.join("lore")
            .join(slugify(&entry.category))
            .join(format!("{}.md", entry.id))
    } else if let Some(id) = uri.strip_prefix("chronicle://texts/") {
        docs.join("texts").join(format!("{}.md", slugify(id)))
    } else if let Some(id) = uri.strip_prefix("chronicle://items/") {
        docs.join("items").join(format!("{}.md", slugify(id)))
    } else if let Some(id) = uri.strip_prefix("chronicle://attachments/") {
        docs.join("attachments").join(format!("{}.md", slugify(id)))
    } else {
        return Err(anyhow!("Unknown resource URI: {uri}"));
    };

    let text = std::fs::read_to_string(&path)?;
    Ok(json!({
        "contents": [{
            "uri": uri,
            "mimeType": "text/markdown",
            "text": text
        }]
    }))
}

fn prompt_definitions() -> Value {
    json!([
        {
            "name": "gm-start-session",
            "description": "Load full campaign continuity and start or resume a roleplay session cleanly across chats.",
            "arguments": [
                { "name": "place_id", "description": "Optional active place slug", "required": false }
            ]
        },
        {
            "name": "gm-turn-protocol",
            "description": "Strict GM adjudication loop ensuring zero hallucinated stats, equipment, weather, or timeline drift.",
            "arguments": []
        }
    ])
}

fn handle_prompt_get(params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("Missing prompt name"))?;
    match name {
        "gm-start-session" => {
            let place = params
                .get("arguments")
                .and_then(|a| a.get("place_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let place_hint = if place.is_empty() {
                String::new()
            } else {
                format!(" Focus on place_id='{place}'.")
            };
            Ok(json!({
                "description": "Start or resume a consistent campaign session",
                "messages": [{
                    "role": "user",
                    "content": {
                        "type": "text",
                        "text": format!(
                            "You are the Game Master using Chronicle. Call `campaign_status` and `get_scene_context` now to load the canonical world clock, season, active place, character sheets, knowledge graph, triggered lore, and active clocks.{place_hint} Summarize where we left off in 3-5 atmospheric sentences and prompt the player for their action. Never invent stats, equipment, or past events not backed by Chronicle."
                        )
                    }
                }]
            }))
        }
        "gm-turn-protocol" => Ok(json!({
            "description": "Strict GM turn consistency loop",
            "messages": [{
                "role": "user",
                "content": {
                    "type": "text",
                    "text": "On every roleplay turn: (1) Call `get_scene_context` with the player's latest action text to retrieve triggered lore, exact vitals, equipment, techniques, terrain/season effects, and graph relations. (2) If an outcome is uncertain, call `roll_dice` first before narrating. (3) Immediately commit every consequence using Chronicle mutation tools (`modify_character_resource`, `update_character_condition`, `update_character_equipment`, `move_character`, `update_place`, `advance_time`, `manage_clock`, `upsert_relation`, `log_event`). (4) Only then present vivid narrative prose that strictly reflects the committed state."
                }
            }]
        })),
        _ => Err(anyhow!("Unknown prompt: {name}")),
    }
}

fn tool_definitions() -> Value {
    let base: Value = serde_json::from_str(
        r#"[
        {
            "name": "campaign_status",
            "description": "Get campaign overview: world clock, season, session number, character & place summaries, active clocks, and SHA-256 event ledger integrity.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "get_scene_context",
            "description": "Assemble compact, hallucination-proof GM context for a scene: world clock, active place (terrain, season, weather, landmarks, routes), present characters (current status, gear, skills, techniques, recent events), 1-hop knowledge graph, keyword/FTS5 triggered lore, and active clocks. Supports fuzzy names/IDs and compact Markdown output.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "place_id": { "type": "string", "description": "Active place slug or name (optional; inferred from first character if omitted)." },
                    "character_ids": { "type": "array", "items": { "type": "string" }, "description": "Explicit character slugs or names to include in addition to everyone at place_id." },
                    "scene_text": { "type": "string", "description": "Latest player input or scene description used to trigger relevant Lorebook entries." },
                    "recent_events_limit": { "type": "integer", "default": 8 },
                    "include_gm_secrets": { "type": "boolean", "default": true },
                    "perspective": { "type": "string", "description": "Epistemic POV: 'gm' (default, includes NPC blind spots & fenced GM EYES ONLY block), 'player', or a specific character_id (strict hard wall — zero GM secrets, only that character's actual beliefs!)." },
                    "format": { "type": "string", "enum": ["json", "markdown"], "default": "json", "description": "Use 'markdown' for ~40% fewer tokens in LLM prompts." },
                    "max_chars": { "type": "integer", "description": "Optional character budget cap when format is 'markdown'." }
                }
            }
        },
        {
            "name": "advance_time",
            "description": "Advance the in-world calendar by minutes (automatically rolls hours, days, seasons, and years) and optionally update global weather.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "minutes": { "type": "integer", "description": "Minutes to advance (e.g., 60 for 1 hour, 1440 for 1 day)." },
                    "global_weather": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["minutes"]
            }
        },
        {
            "name": "start_new_session",
            "description": "Start a new roleplay session/chat chapter and log a session marker in the timeline.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "summary": { "type": "string" }
                },
                "required": ["title"]
            }
        },
        {
            "name": "create_character",
            "description": "Create a new character with an immutable Initial/Baseline Profile (details, initial stats, resources, equipment, skills, techniques, goals). Automatically compiles docs/characters/<id>.md.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Optional slug; auto-generated from name if omitted." },
                    "name": { "type": "string" },
                    "title": { "type": "string" },
                    "kind": { "type": "string", "description": "pc, npc, companion, or antagonist" },
                    "origin": { "type": "string" },
                    "archetype": { "type": "string" },
                    "faction": { "type": "string" },
                    "appearance": { "type": "string" },
                    "personality": { "type": "string" },
                    "backstory": { "type": "string" },
                    "initial_location": { "type": "string" },
                    "initial_stats": { "type": "object", "additionalProperties": { "type": "integer" } },
                    "initial_resources": { "type": "object", "description": "Map of resource name -> {current, max, unit}" },
                    "initial_equipment": { "type": "array", "items": { "type": "object" } },
                    "initial_skills": { "type": "array", "items": { "type": "object" } },
                    "initial_techniques": { "type": "array", "items": { "type": "object" } },
                    "initial_goals": { "type": "array", "items": { "type": "string" } },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "notes": { "type": "string" }
                },
                "required": ["name"]
            }
        },
        {
            "name": "get_character",
            "description": "Get full character document data: Initial Baseline, projected Current Status, Knowledge Graph relations, and chronological Event Log (supports time-travel via as_of_seq).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": { "type": "string" },
                    "as_of_seq": { "type": "integer", "description": "Optional event sequence number to inspect historical state." }
                },
                "required": ["character_id"]
            }
        },
        {
            "name": "move_character",
            "description": "Move a character to another place and log the movement in both the character and place event logs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": { "type": "string" },
                    "to_place_id": { "type": "string" },
                    "travel_notes": { "type": "string" }
                },
                "required": ["character_id", "to_place_id"]
            }
        },
        {
            "name": "modify_character_resource",
            "description": "Adjust or set a character resource pool (e.g., HP, Stamina, Qi, Mana, Sanity, Gold).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": { "type": "string" },
                    "resource": { "type": "string" },
                    "delta": { "type": "integer", "description": "Relative change, e.g., -12 or +15." },
                    "set_current": { "type": "integer" },
                    "set_max": { "type": "integer" },
                    "unit": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["character_id", "resource"]
            }
        },
        {
            "name": "modify_character_stat",
            "description": "Adjust or set a character attribute/stat (e.g., Might, Agility, Spirit).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": { "type": "string" },
                    "stat": { "type": "string" },
                    "delta": { "type": "integer" },
                    "set_value": { "type": "integer" },
                    "reason": { "type": "string" }
                },
                "required": ["character_id", "stat"]
            }
        },
        {
            "name": "set_character_status",
            "description": "Update a character's overall status (e.g., Active, Injured, Incapacitated, Missing, Deceased).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": { "type": "string" },
                    "alive_status": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["character_id", "alive_status"]
            }
        },
        {
            "name": "update_character_condition",
            "description": "Add or remove an active status effect, buff, debuff, injury, or curse on a character.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": { "type": "string" },
                    "action": { "type": "string", "enum": ["add", "remove"] },
                    "name": { "type": "string" },
                    "kind": { "type": "string", "description": "buff, debuff, injury, curse, condition" },
                    "severity": { "type": "string" },
                    "duration": { "type": "string" },
                    "source": { "type": "string" },
                    "notes": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["character_id", "action", "name"]
            }
        },
        {
            "name": "update_character_equipment",
            "description": "Acquire, remove/consume, equip, or unequip an item in a character's gear & inventory.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": { "type": "string" },
                    "action": { "type": "string", "enum": ["acquire", "remove", "equip", "unequip"] },
                    "name": { "type": "string" },
                    "quantity": { "type": "integer", "default": 1 },
                    "slot": { "type": "string" },
                    "equipped": { "type": "boolean" },
                    "durability": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "description": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["character_id", "action", "name"]
            }
        },
        {
            "name": "upsert_character_skill",
            "description": "Learn a new skill or upgrade an existing skill's rank/modifier.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": { "type": "string" },
                    "name": { "type": "string" },
                    "rank": { "type": "string" },
                    "category": { "type": "string" },
                    "modifier": { "type": "string" },
                    "description": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["character_id", "name"]
            }
        },
        {
            "name": "upsert_character_technique",
            "description": "Learn a new technique/ability/spell or update its mastery, tier, cost, cooldown, or effect.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": { "type": "string" },
                    "name": { "type": "string" },
                    "tier": { "type": "string" },
                    "cost": { "type": "string" },
                    "cooldown": { "type": "string" },
                    "mastery": { "type": "string" },
                    "effect": { "type": "string" },
                    "description": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["character_id", "name"]
            }
        },
        {
            "name": "update_character_goal",
            "description": "Add an active personal goal/objective for a character or mark an existing goal completed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": { "type": "string" },
                    "goal": { "type": "string" },
                    "completed": { "type": "boolean", "default": false }
                },
                "required": ["character_id", "goal"]
            }
        },
        {
            "name": "create_place",
            "description": "Create a place document with terrain, climate, seasonal profiles, places of significance (landmarks), connected routes, initial weather, and controlling faction.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "place_type": { "type": "string", "description": "continent, region, settlement, district, dungeon, wilderness, building" },
                    "parent_place": { "type": "string" },
                    "terrain": { "type": "string" },
                    "climate": { "type": "string" },
                    "seasonal_profiles": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Map of season name (Spring/Summer/Autumn/Winter) -> terrain & weather effects" },
                    "description": { "type": "string" },
                    "initial_weather": { "type": "string" },
                    "initial_faction": { "type": "string" },
                    "initial_danger_level": { "type": "string" },
                    "places_of_significance": { "type": "array", "items": { "type": "object" } },
                    "routes": { "type": "array", "items": { "type": "object" } },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "notes": { "type": "string" }
                },
                "required": ["name"]
            }
        },
        {
            "name": "get_place",
            "description": "Get full place data: Baseline Geography & Seasonal Profiles, live Current Status (season, weather, present characters, landmarks, routes), relations, and Event Log.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "place_id": { "type": "string" },
                    "as_of_seq": { "type": "integer" }
                },
                "required": ["place_id"]
            }
        },
        {
            "name": "update_place",
            "description": "Mutate live place state: change weather, add/remove local condition, update controlling faction/danger, update/discover/damage a landmark, or update a route.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "place_id": { "type": "string" },
                    "action": { "type": "string", "enum": ["weather", "add_condition", "remove_condition", "control", "landmark", "route"] },
                    "weather": { "type": "string" },
                    "condition": { "type": "string" },
                    "faction": { "type": "string" },
                    "danger_level": { "type": "string" },
                    "landmark_name": { "type": "string" },
                    "landmark_kind": { "type": "string" },
                    "landmark_status": { "type": "string" },
                    "destination_id": { "type": "string" },
                    "travel_time": { "type": "string" },
                    "terrain_difficulty": { "type": "string" },
                    "route_status": { "type": "string" },
                    "description": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "notes": { "type": "string" }
                },
                "required": ["place_id", "action"]
            }
        },
        {
            "name": "upsert_lore",
            "description": "Create or update a World-Building / Lore reference document (separate from characters & places) with auto-trigger keywords, FTS5 indexing, visibility (public/rumor/gm_secret), and entity cross-references.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "category": { "type": "string", "description": "e.g., world, history, magic-system, factions, bestiary, artifacts, culture, cosmology, rules" },
                    "summary": { "type": "string" },
                    "body": { "type": "string" },
                    "keywords": { "type": "array", "items": { "type": "string" }, "description": "Trigger keywords that auto-whisper this lore in get_scene_context." },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "visibility": { "type": "string", "enum": ["public", "rumor", "gm_secret"], "default": "public" },
                    "always_include": { "type": "boolean", "default": false },
                    "related_ids": { "type": "array", "items": { "type": "string" } },
                    "notes": { "type": "string" }
                },
                "required": ["title", "body"]
            }
        },
        {
            "name": "search_lore",
            "description": "Search World-Building & Lore entries by full-text query, category, visibility (public/rumor/gm_secret), trigger keyword, tag, related entity ID, always_include flag, or exact lore_id.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "lore_id": { "type": "string" },
                    "query": { "type": "string" },
                    "category": { "type": "string" },
                    "visibility": { "type": "string", "enum": ["public", "rumor", "gm_secret"] },
                    "keyword": { "type": "string" },
                    "tag": { "type": "string" },
                    "related_to": { "type": "string", "description": "Find lore linked to a character, place, or lore slug." },
                    "always_include": { "type": "boolean" },
                    "limit": { "type": "integer", "default": 20 }
                }
            }
        },
        {
            "name": "upsert_relation",
            "description": "Add or update a typed Knowledge Graph relationship between any two entities (character, place, lore, faction), supporting multi-dimensional hidden scores (trust, respect, affection, fear, resentment, dependence), directed/mutual flags, visibility (gm_secret hides scores from player POV), provenance origin_event_id, and obligations.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source_id": { "type": "string" },
                    "source_kind": { "type": "string", "default": "character" },
                    "relation_type": { "type": "string" },
                    "target_id": { "type": "string" },
                    "target_kind": { "type": "string", "default": "character" },
                    "strength": { "type": "integer", "description": "Optional numeric disposition/bond (-100 to +100)." },
                    "dimensions": {
                        "type": "object",
                        "properties": {
                            "trust": { "type": "integer" },
                            "respect": { "type": "integer" },
                            "affection": { "type": "integer" },
                            "fear": { "type": "integer" },
                            "resentment": { "type": "integer" },
                            "dependence": { "type": "integer" }
                        }
                    },
                    "direction": { "type": "string", "enum": ["directed", "mutual"], "default": "directed" },
                    "visibility": { "type": "string", "enum": ["public", "gm_secret"], "description": "Defaults to gm_secret when dimensions are supplied." },
                    "origin_event_id": { "type": "integer" },
                    "obligations": { "type": "array", "items": { "type": "string" } },
                    "notes": { "type": "string" }
                },
                "required": ["source_id", "relation_type", "target_id"]
            }
        },
        {
            "name": "remove_relation",
            "description": "Remove an active Knowledge Graph relationship and log why it ended.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source_id": { "type": "string" },
                    "relation_type": { "type": "string" },
                    "target_id": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["source_id", "relation_type", "target_id"]
            }
        },
        {
            "name": "manage_clock",
            "description": "Create, tick, or list Progress Clocks (quests, threats, faction schemes, rituals, travel legs, training arcs).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["create", "tick", "update", "list"] },
                    "clock_id": { "type": "string" },
                    "name": { "type": "string" },
                    "kind": { "type": "string", "description": "quest, threat, faction, ritual, travel, training" },
                    "segments": { "type": "integer", "default": 6 },
                    "filled": { "type": "integer", "default": 0 },
                    "delta": { "type": "integer", "default": 1 },
                    "status": { "type": "string" },
                    "description": { "type": "string" },
                    "related_ids": { "type": "array", "items": { "type": "string" } },
                    "reason": { "type": "string" }
                },
                "required": ["action"]
            }
        },
        {
            "name": "log_event",
            "description": "Record a narrative event or plot beat that affected specific characters, places, or lore entries, with optional actor_id, causal_event_ids (parent event DAG links), and visibility.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": { "type": "string" },
                    "details": { "type": "string" },
                    "location_id": { "type": "string" },
                    "actor_id": { "type": "string" },
                    "affected_entities": { "type": "array", "items": { "type": "string" } },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "causal_event_ids": { "type": "array", "items": { "type": "integer" } },
                    "visibility": { "type": "string", "enum": ["public", "witnessed_only", "gm_secret"], "default": "public" }
                },
                "required": ["title"]
            }
        },
        {
            "name": "roll_dice",
            "description": "Roll RPG dice deterministically (supports NdX, khN keep-highest, klN keep-lowest, +/- modifiers, e.g., '2d20kh1+5', '4d6kh3', '1d100'). Optionally logs the roll to the campaign ledger.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "expression": { "type": "string" },
                    "actor_id": { "type": "string" },
                    "location_id": { "type": "string" },
                    "purpose": { "type": "string" },
                    "log_to_ledger": { "type": "boolean", "default": true },
                    "seed": { "type": "integer" }
                },
                "required": ["expression"]
            }
        },
        {
            "name": "search_history",
            "description": "Search the campaign event ledger by full-text query, entity_id, location_id, event_type, session_number, world_time substring (e.g., 'Autumn'), or sequence range.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "entity_id": { "type": "string" },
                    "location_id": { "type": "string" },
                    "event_type": { "type": "string", "description": "e.g. movement, resource_changed, item_acquired, technique_updated, dice_rolled, landmark_updated, narrative_event" },
                    "session_number": { "type": "integer" },
                    "world_time_contains": { "type": "string", "description": "Filter by season/year/day substring, e.g. 'Autumn' or 'Y412'" },
                    "seq_from": { "type": "integer" },
                    "seq_to": { "type": "integer" },
                    "limit": { "type": "integer", "default": 30 }
                }
            }
        },
        {
            "name": "rollback_to_event",
            "description": "Safely rewind the entire campaign state to event sequence #to_seq (automatically creates a backup DB copy first and recompiles all Markdown documents).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to_seq": { "type": "integer" }
                },
                "required": ["to_seq"]
            }
        },
        {
            "name": "compile_docs",
            "description": "Force re-compilation of all referencable Markdown documents (INDEX.md, TIMELINE.md, characters/, places/, lore/, texts/, attachments/) from the SQLite event-sourced state.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ]"#,
    )
    .expect("valid base MCP tool definitions JSON");
    let mut list = match base {
        Value::Array(v) => v,
        _ => Vec::new(),
    };
    list.extend(extra_rpg_ai_tools());
    // Attach ChatGPT / MCP standard behavioral annotations to every tool
    for tool in &mut list {
        if let Some(obj) = tool.as_object_mut() {
            let name = obj
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            obj.insert("annotations".to_string(), tool_annotations(&name));
        }
    }
    Value::Array(list)
}

fn tool_annotations(name: &str) -> Value {
    let read_only = matches!(
        name,
        "campaign_status"
            | "get_scene_context"
            | "get_character"
            | "get_place"
            | "get_text"
            | "get_item"
            | "get_lore"
            | "get_attachment"
            | "search"
            | "fetch"
            | "search_characters"
            | "search_places"
            | "search_lore"
            | "search_texts"
            | "search_items"
            | "search_histories"
            | "search_attachments"
            | "search_history"
            | "search_events"
            | "search_relations"
            | "search_all"
            | "diagnose_campaign"
    );
    let destructive = matches!(
        name,
        "rollback_to_event" | "remove_relation" | "delete_entity" | "restore_backup"
    );
    let idempotent = read_only || matches!(name, "compile_docs" | "cleanup_campaign");
    json!({
        "readOnlyHint": read_only,
        "destructiveHint": destructive,
        "idempotentHint": idempotent,
        "openWorldHint": false
    })
}

fn extra_rpg_ai_tools() -> Vec<Value> {
    let mut v = Vec::new();
    let parse = |s: &str| serde_json::from_str::<Value>(s).expect("valid extra tool JSON");
    v.push(parse(r#"{
            "name": "search_characters",
            "description": "Multi-dimensional 'Search By' query for Characters: filter by name/text, kind (pc/npc), alive_status, location, faction, owned/equipped item, skill, technique, active condition, tag, minimum stat, or low resource percentage.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "kind": { "type": "string" },
                    "alive_status": { "type": "string" },
                    "location_id": { "type": "string" },
                    "faction": { "type": "string" },
                    "has_item": { "type": "string", "description": "Find characters holding an item matching name or tag." },
                    "equipped_only": { "type": "boolean" },
                    "has_skill": { "type": "string" },
                    "has_technique": { "type": "string" },
                    "has_condition": { "type": "string" },
                    "tag": { "type": "string" },
                    "stat_name": { "type": "string" },
                    "stat_min": { "type": "integer" },
                    "resource_name": { "type": "string" },
                    "resource_below_pct": { "type": "integer", "description": "Find characters whose resource is <= this percentage (0-100)." },
                    "include_full": { "type": "boolean", "default": false }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "search_places",
            "description": "Multi-dimensional 'Search By' query for Places: filter by name/text, place_type, parent_place, terrain, climate, seasonal profile text, current weather, controlling faction, danger_level, landmark name/status, active local condition, present character, connected route, or tag.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "place_type": { "type": "string" },
                    "parent_place": { "type": "string" },
                    "terrain": { "type": "string" },
                    "climate": { "type": "string" },
                    "season_effect": { "type": "string" },
                    "weather": { "type": "string" },
                    "faction": { "type": "string" },
                    "danger_level": { "type": "string" },
                    "has_landmark": { "type": "string" },
                    "landmark_status": { "type": "string" },
                    "has_condition": { "type": "string" },
                    "has_character": { "type": "string" },
                    "connected_to": { "type": "string" },
                    "tag": { "type": "string" },
                    "include_full": { "type": "boolean", "default": false }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "search_relations",
            "description": "Search the Knowledge Graph by entity, source, target, relation_type, disposition strength range, notes query, or multi-hop traversal (1-3 hops).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity_id": { "type": "string" },
                    "source_id": { "type": "string" },
                    "target_id": { "type": "string" },
                    "relation_type": { "type": "string" },
                    "min_strength": { "type": "integer" },
                    "max_strength": { "type": "integer" },
                    "query": { "type": "string" },
                    "hops": { "type": "integer", "default": 1, "description": "Traversal depth from entity_id (1, 2, or 3)." }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "search_all",
            "description": "Unified omni-search across Characters, Places, Lore, Knowledge Graph Relations, Progress Clocks, and Historical Events in one call.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "limit_per_kind": { "type": "integer", "default": 10 }
                },
                "required": ["query"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "transfer_item",
            "description": "Atomically transfer an item from one character to another (verifies ownership and quantity to prevent duplication).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from_character_id": { "type": "string" },
                    "to_character_id": { "type": "string" },
                    "item_name": { "type": "string" },
                    "quantity": { "type": "integer", "default": 1 },
                    "reason": { "type": "string" }
                },
                "required": ["from_character_id", "to_character_id", "item_name"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "rest_characters",
            "description": "Perform a Short Rest or Long Rest for one or more characters: advances world time, restores resources (by percentage or to max), and optionally clears temporary conditions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_ids": { "type": "array", "items": { "type": "string" } },
                    "rest_type": { "type": "string", "enum": ["short", "long"], "default": "long" },
                    "advance_minutes": { "type": "integer", "description": "Defaults to 60 for short rest, 480 for long rest." },
                    "restore_percent": { "type": "integer", "description": "Defaults to 50 for short rest, 100 for long rest." },
                    "resources": { "type": "array", "items": { "type": "string" }, "description": "Optional subset of resources to restore; empty = all." },
                    "clear_conditions": { "type": "array", "items": { "type": "string" } },
                    "notes": { "type": "string" }
                },
                "required": ["character_ids"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "commit_turn",
            "description": "AI-optimized batch turn commit: execute multiple Chronicle tool operations in order within a single tool call (e.g., roll_dice + modify_character_resource + update_character_condition + manage_clock + log_event).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "actions": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "tool": { "type": "string" },
                                "arguments": { "type": "object" }
                            },
                            "required": ["tool", "arguments"]
                        }
                    }
                },
                "required": ["actions"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "upsert_text",
            "description": "Catalog or update a story-relevant in-world Book, Scroll, Grimoire, Ritual Manual, Tablet, Journal, or Letter that characters can find, carry, read, or decipher to learn world history, spell rituals, or secrets. Automatically compiles docs/texts/<id>.md.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "medium": { "type": "string", "description": "book, scroll, grimoire, tablet, journal, letter, ritual-manual, inscription, codex" },
                    "subject": { "type": "string", "description": "history, spell-ritual, prophecy, technique-manual, alchemy, law, correspondence, treatise" },
                    "author": { "type": "string" },
                    "language": { "type": "string", "default": "Common" },
                    "condition": { "type": "string", "default": "Intact" },
                    "read_status": { "type": "string", "enum": ["undiscovered", "discovered_unread", "partially_read", "read", "deciphered"], "default": "undiscovered" },
                    "read_by": { "type": "array", "items": { "type": "string" } },
                    "holder_or_location_id": { "type": "string", "description": "Place slug where the text rests or character slug carrying it." },
                    "summary": { "type": "string", "description": "Outward cover/appearance description visible before reading." },
                    "content": { "type": "string", "description": "Actual readable in-world passages, ritual steps, historical accounts, or chapters (Markdown)." },
                    "mechanics_or_unlocks": { "type": "array", "items": { "type": "string" }, "description": "What reading/deciphering unlocks (rituals, techniques, secrets, clues)." },
                    "gm_notes": { "type": "string", "description": "Hidden GM notes: deciphering DC, curses, missing pages, historical bias." },
                    "keywords": { "type": "array", "items": { "type": "string" } },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "related_ids": { "type": "array", "items": { "type": "string" } },
                    "notes": { "type": "string" }
                },
                "required": ["title", "content"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "get_text",
            "description": "Retrieve a Book/Scroll/StoryText by ID along with its full readable content, reading status, cross-references, and provenance log.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text_id": { "type": "string" }
                },
                "required": ["text_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "record_text_reading",
            "description": "Record that a character discovered, studied, read, deciphered, or moved a Book/Scroll/Ritual Manual. Updates read_by, read_status, holder_or_location_id, and condition, logging to both the text's provenance log and the reader's event log.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text_id": { "type": "string" },
                    "reader_id": { "type": "string", "description": "Character slug or name who studied/read it." },
                    "read_status": { "type": "string", "enum": ["undiscovered", "discovered_unread", "partially_read", "read", "deciphered"] },
                    "holder_or_location_id": { "type": "string", "description": "New character carrying it or place storing it." },
                    "condition": { "type": "string" },
                    "notes": { "type": "string" }
                },
                "required": ["text_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "search_texts",
            "description": "Multi-dimensional 'Search By' query for Books, Scrolls & Story Texts: filter by full-text query, medium (book/scroll/grimoire), subject (history/spell-ritual), author, language, condition, read_status, read_by (character), holder_or_location_id, keyword, tag, or related_to.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text_id": { "type": "string" },
                    "query": { "type": "string" },
                    "medium": { "type": "string" },
                    "subject": { "type": "string" },
                    "author": { "type": "string" },
                    "language": { "type": "string" },
                    "condition": { "type": "string" },
                    "read_status": { "type": "string" },
                    "read_by": { "type": "string" },
                    "holder_or_location_id": { "type": "string" },
                    "keyword": { "type": "string" },
                    "tag": { "type": "string" },
                    "related_to": { "type": "string" },
                    "limit": { "type": "integer", "default": 25 }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "search",
            "description": "ChatGPT & MCP Connector standard read-only search across all campaign documents (Characters, Places, Books/Scrolls, Lore, Index, Timeline). Returns canonical results with id, title, and url for use with `fetch`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query text across campaign characters, places, books/scrolls, and world lore." }
                },
                "required": ["query"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "fetch",
            "description": "ChatGPT & MCP Connector standard read-only document fetch. Pass an `id` returned by `search` (e.g., character:<slug>, place:<slug>, text:<slug>, lore:<slug>, attachment:<slug>, index:campaign, timeline:campaign) to retrieve the complete compiled Markdown document and metadata.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Unique document ID returned by `search` (or an entity slug)." }
                },
                "required": ["id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "import_attachment",
            "description": "Copy an attached file relevant to the story (map, handout, PDF, portrait, rules doc, notes, audio) into `<campaign>/docs/attachments/files/` and return a stable addressable reference (`id`, `relative_path`, `absolute_path`, `chronicle://attachments/<id>`) for future chats. Accepts `source_path` (local copy), `text_content` (inline UTF-8/Markdown), or `base64_content` (binary upload).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Optional stable slug; auto-generated from title if omitted." },
                    "title": { "type": "string" },
                    "source_path": { "type": "string", "description": "Local filesystem path of an attached document/file to copy into the campaign folder." },
                    "text_content": { "type": "string", "description": "Inline UTF-8/Markdown/JSON content when no local source_path exists." },
                    "base64_content": { "type": "string", "description": "Base64-encoded binary file content (PNG/PDF/MP3/etc.) when uploading remotely." },
                    "filename": { "type": "string", "description": "Original filename with extension (e.g., 'region-map.png', 'treaty.pdf')." },
                    "category": { "type": "string", "description": "map, handout, portrait, rules, world-doc, notes, audio, other" },
                    "mime_type": { "type": "string" },
                    "summary": { "type": "string", "description": "Concise description of what this document is and why it matters to the campaign." },
                    "extracted_notes": { "type": "string", "description": "Searchable key facts or text excerpt extracted from the file so future chats can recall it via FTS5." },
                    "visibility": { "type": "string", "enum": ["public", "rumor", "gm_secret"], "default": "public" },
                    "keywords": { "type": "array", "items": { "type": "string" } },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "related_ids": { "type": "array", "items": { "type": "string" } },
                    "notes": { "type": "string" }
                },
                "required": ["title"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "get_attachment",
            "description": "Retrieve an attached reference document's addressable file path (`relative_path`, `absolute_path`, `mcp_uri`), metadata, extracted notes, provenance log, and inline text content (for text/markdown/json files).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "attachment_id": { "type": "string" },
                    "include_text_content": { "type": "boolean", "default": true }
                },
                "required": ["attachment_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "search_attachments",
            "description": "Multi-dimensional 'Search By' query for copied/attached campaign docs & media: filter by FTS5 query, category (map/handout/portrait/rules/world-doc/audio), mime_type, visibility, keyword, tag, or related entity ID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "attachment_id": { "type": "string" },
                    "query": { "type": "string" },
                    "category": { "type": "string" },
                    "mime_type": { "type": "string" },
                    "visibility": { "type": "string", "enum": ["public", "rumor", "gm_secret"] },
                    "keyword": { "type": "string" },
                    "tag": { "type": "string" },
                    "related_to": { "type": "string" },
                    "limit": { "type": "integer", "default": 25 }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "create_reference_doc",
            "description": "Create a new reference file (or update/append to an existing one) in `<campaign>/docs/attachments/files/` to store pasted-in reference material, house rules, world-building dumps, or session notes with a stable addressable reference (`attachment:<id>`, `chronicle://attachments/<id>`, `docs/attachments/files/<id>.<ext>`).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Optional stable slug; auto-generated from title if omitted." },
                    "title": { "type": "string" },
                    "filename": { "type": "string", "description": "Optional filename with extension (defaults to '<id>.md')." },
                    "content": { "type": "string", "description": "Pasted-in text/Markdown/JSON/YAML content to write into the file." },
                    "append": { "type": "boolean", "default": false, "description": "If true and the file already exists, append content instead of overwriting." },
                    "category": { "type": "string", "default": "world-doc", "description": "world-doc, rules, notes, handout, map, other" },
                    "summary": { "type": "string" },
                    "extracted_notes": { "type": "string" },
                    "visibility": { "type": "string", "enum": ["public", "rumor", "gm_secret"], "default": "public" },
                    "keywords": { "type": "array", "items": { "type": "string" } },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "related_ids": { "type": "array", "items": { "type": "string" } },
                    "notes": { "type": "string" }
                },
                "required": ["title", "content"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "upsert_item",
            "description": "Catalog or update a carryable RPG item (weapon, armor, tool, artifact, keepsake, book, manual, scroll, record, key, relic) with origin/maker, previous owners, current holder (character or place), significance, mechanics, and optional linked_text_id. Automatically syncs into the holder character's inventory and compiles docs/items/<id>.md.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "item_type": { "type": "string", "description": "weapon, armor, tool, artifact, keepsake, book, manual, scroll, record, relic, key, consumable, gear" },
                    "rarity_or_tier": { "type": "string" },
                    "slot": { "type": "string" },
                    "quantity": { "type": "integer", "default": 1 },
                    "equipped": { "type": "boolean", "default": false },
                    "condition_or_durability": { "type": "string", "default": "Intact" },
                    "origin_and_maker": { "type": "string", "description": "Who forged, crafted, or authored this item and where." },
                    "previous_owners": { "type": "array", "items": { "type": "string" }, "description": "Chronological list of previous owners." },
                    "current_holder_id": { "type": "string", "description": "Character slug carrying it OR place slug where it rests." },
                    "significance": { "type": "string", "description": "Why this item matters to the story, bloodlines, or sects." },
                    "properties_and_effects": { "type": "string" },
                    "description": { "type": "string" },
                    "linked_text_id": { "type": "string", "description": "Optional StoryText slug when this carryable item is a readable book, manual, scroll, or record." },
                    "gm_notes": { "type": "string" },
                    "keywords": { "type": "array", "items": { "type": "string" } },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "related_ids": { "type": "array", "items": { "type": "string" } },
                    "notes": { "type": "string" }
                },
                "required": ["name"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "get_item",
            "description": "Retrieve a cataloged carryable item's full dossier: ownership chain, significance, historical chronicle, linked book/manual text, relations, and live provenance log.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "item_id": { "type": "string" }
                },
                "required": ["item_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "transfer_catalog_item",
            "description": "Transfer a cataloged carryable item (weapon, tool, keepsake, artifact, book, manual, scroll, record) to a new character or place. Automatically appends the former holder to previous_owners, updates character inventories, syncs any linked StoryText holder, and logs what happened to it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "item_id": { "type": "string" },
                    "to_holder_id": { "type": "string", "description": "New character slug or place slug (omit to mark unclaimed/lost)." },
                    "equipped": { "type": "boolean" },
                    "condition_or_durability": { "type": "string" },
                    "notes": { "type": "string", "description": "What happened during the transfer (looted, gifted, stolen, damaged, repaired)." }
                },
                "required": ["item_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "search_items",
            "description": "Multi-dimensional 'Search By' query for cataloged carryable items (weapons, tools, keepsakes, artifacts, books, manuals, records): filter by FTS5 query, item_type, rarity_or_tier, current_holder_id, previous_owner, condition, has_linked_text, keyword, tag, or related_to.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "item_id": { "type": "string" },
                    "query": { "type": "string" },
                    "item_type": { "type": "string" },
                    "rarity_or_tier": { "type": "string" },
                    "current_holder_id": { "type": "string" },
                    "previous_owner": { "type": "string" },
                    "condition": { "type": "string" },
                    "has_linked_text": { "type": "boolean" },
                    "keyword": { "type": "string" },
                    "tag": { "type": "string" },
                    "related_to": { "type": "string" },
                    "limit": { "type": "integer", "default": 25 }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "add_entity_history",
            "description": "Record a structured historical chronicle entry for any Place, Establishment (tavern/guild/academy/temple), Sect/Faction, Carryable Item (weapon/tool/keepsake/book/manual/record), Lore entry, or Character. Rendered directly into that entity's compiled Markdown doc under '## Historical Chronicle'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity_id": { "type": "string", "description": "Slug or name of the place, establishment, sect, faction, item, text, or character." },
                    "entity_kind": { "type": "string", "enum": ["place", "establishment", "sect", "faction", "item", "text", "character", "lore"], "default": "place" },
                    "era_or_date": { "type": "string", "description": "Historical era, year, or timeframe (e.g. 'First Age Y12', '300 years ago')." },
                    "sort_order": { "type": "integer", "default": 0, "description": "Chronological ordering integer (lower sorts earlier)." },
                    "title": { "type": "string" },
                    "details": { "type": "string", "description": "What happened historically, who was involved, and its aftermath." },
                    "significance": { "type": "string", "description": "Why this historical event still matters today." },
                    "related_ids": { "type": "array", "items": { "type": "string" } },
                    "visibility": { "type": "string", "enum": ["public", "rumor", "gm_secret"], "default": "public" }
                },
                "required": ["entity_id", "title", "details"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "search_histories",
            "description": "Multi-dimensional 'Search By' query across recorded Historical Chronicles for places, establishments, sects, factions, items, books/manuals, and characters.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "entity_id": { "type": "string" },
                    "entity_kind": { "type": "string", "description": "place, establishment, sect, faction, item, text, character, lore" },
                    "era_or_date": { "type": "string" },
                    "visibility": { "type": "string", "enum": ["public", "rumor", "gm_secret"] },
                    "related_to": { "type": "string" },
                    "limit": { "type": "integer", "default": 40 }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "diagnose_campaign",
            "description": "Run a full campaign diagnostic & integrity audit: verifies SHA-256 ledger, detects mislabeled entities placed in the wrong register (e.g., books/items/places/characters/histories filed under lore), finds dangling references, history kind mismatches, unsynced carryable texts/inventories, duplicate names/slugs, missing attachment files, and orphaned compiled .md docs — returning exact suggested_tool + suggested_args fixes.",
            "inputSchema": { "type": "object", "properties": {} }
        }"#));
    v.push(parse(r#"{
            "name": "update_character_baseline",
            "description": "Edit typos or mislabeled profile fields on an existing character baseline (name, title, kind, origin, archetype, faction, appearance, personality, backstory, tags, notes) with full audit logging and automatic Markdown recompilation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": { "type": "string" },
                    "name": { "type": "string" },
                    "title": { "type": "string" },
                    "kind": { "type": "string" },
                    "origin": { "type": "string" },
                    "archetype": { "type": "string" },
                    "faction": { "type": "string" },
                    "appearance": { "type": "string" },
                    "personality": { "type": "string" },
                    "backstory": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "notes": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["character_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "update_place_baseline",
            "description": "Edit typos or mislabeled baseline fields on an existing place or establishment (name, place_type, parent_place, terrain, climate, seasonal_profiles, description, tags, notes) with full audit logging and automatic Markdown recompilation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "place_id": { "type": "string" },
                    "name": { "type": "string" },
                    "place_type": { "type": "string" },
                    "parent_place": { "type": "string" },
                    "terrain": { "type": "string" },
                    "climate": { "type": "string" },
                    "seasonal_profiles": { "type": "object", "additionalProperties": { "type": "string" } },
                    "description": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "notes": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["place_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "update_entity_history",
            "description": "Edit an existing Historical Chronicle entry by history_id: fix mislabeled entity_id, entity_kind, era_or_date, sort_order, title, details, significance, visibility, or related_ids.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "history_id": { "type": "integer" },
                    "entity_id": { "type": "string" },
                    "entity_kind": { "type": "string", "enum": ["place", "establishment", "sect", "faction", "item", "text", "character", "lore"] },
                    "era_or_date": { "type": "string" },
                    "sort_order": { "type": "integer" },
                    "title": { "type": "string" },
                    "details": { "type": "string" },
                    "significance": { "type": "string" },
                    "visibility": { "type": "string", "enum": ["public", "rumor", "gm_secret"] },
                    "related_ids": { "type": "array", "items": { "type": "string" } },
                    "reason": { "type": "string" }
                },
                "required": ["history_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "reclassify_entity",
            "description": "Move or migrate a mislabeled record to the right Chronicle register (e.g., lore -> text/item/place/character/history, item -> text, text -> lore/item, attachment -> lore/text, history -> lore). Preserves content/tags/keywords/links, removes the old entry (unless keep_source: true), prunes stale .md files, and logs an audit event.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from_register": { "type": "string", "enum": ["lore", "text", "item", "attachment", "history"] },
                    "entity_id": { "type": "string", "description": "Source slug (or integer string for history)." },
                    "to_register": { "type": "string", "enum": ["lore", "text", "item", "place", "character", "history"] },
                    "new_id": { "type": "string", "description": "Optional new slug in the destination register." },
                    "keep_source": { "type": "boolean", "default": false },
                    "overrides": { "type": "object", "description": "Optional destination fields (e.g. medium, subject, item_type, place_type, target_entity_id, target_entity_kind, era_or_date)." },
                    "reason": { "type": "string" }
                },
                "required": ["from_register", "entity_id", "to_register"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "rename_or_merge_entity",
            "description": "Rename an entity's slug OR merge a duplicate entity (old_id) into a canonical target (new_id, with allow_merge: true) within the same register. Automatically rewrites all cross-references across relations, histories, lore, texts, items, attachments, clocks, routes, and character locations, and prunes stale .md docs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "register": { "type": "string", "enum": ["character", "place", "lore", "text", "item", "attachment"] },
                    "old_id": { "type": "string" },
                    "new_id": { "type": "string" },
                    "allow_merge": { "type": "boolean", "default": false },
                    "reason": { "type": "string" }
                },
                "required": ["register", "old_id", "new_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "delete_entity",
            "description": "Cleanly delete a miscreated, duplicate, or obsolete record from any register (character, place, lore, text, item, history, attachment, clock), optionally removing dangling Knowledge Graph relations and stale compiled .md docs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "register": { "type": "string", "enum": ["character", "place", "lore", "text", "item", "history", "attachment", "clock"] },
                    "entity_id": { "type": "string" },
                    "remove_backing_file": { "type": "boolean", "default": false, "description": "For attachments: also delete the copied file under docs/attachments/files/." },
                    "cleanup_relations": { "type": "boolean", "default": true },
                    "reason": { "type": "string" }
                },
                "required": ["register", "entity_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "cleanup_campaign",
            "description": "Run safe automated campaign hygiene: prune orphaned compiled .md docs, sync carryable books/texts into catalog_items & character inventories, deduplicate tags/keywords/links/previous_owners, and normalize visibility/read_status/entity_kind casing. Defaults to dry_run: true for safe preview.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "dry_run": { "type": "boolean", "default": true, "description": "Set false to apply all safe cleanup actions." }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "update_campaign_meta",
            "description": "Update campaign-level metadata (name, description, ruleset_notes) and/or world clock settings (era, year, season, day, hour, minute, days_per_season, seasons, global_weather) with audit logging.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "ruleset_notes": { "type": "string" },
                    "era": { "type": "string" },
                    "year": { "type": "integer" },
                    "season": { "type": "string" },
                    "day": { "type": "integer" },
                    "hour": { "type": "integer" },
                    "minute": { "type": "integer" },
                    "days_per_season": { "type": "integer" },
                    "seasons": { "type": "array", "items": { "type": "string" } },
                    "global_weather": { "type": "string" },
                    "reason": { "type": "string" }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "get_lore",
            "description": "Get a single World-Building / Lore entry by lore_id along with its Historical Chronicle entries, Knowledge Graph relations, and event log.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "lore_id": { "type": "string" }
                },
                "required": ["lore_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "update_attachment",
            "description": "Edit metadata on an existing attached reference doc (title, category, summary, extracted_notes, visibility, keywords, tags, related_ids) without re-uploading the backing file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "attachment_id": { "type": "string" },
                    "title": { "type": "string" },
                    "category": { "type": "string" },
                    "summary": { "type": "string" },
                    "extracted_notes": { "type": "string" },
                    "visibility": { "type": "string", "enum": ["public", "rumor", "gm_secret"] },
                    "keywords": { "type": "array", "items": { "type": "string" } },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "related_ids": { "type": "array", "items": { "type": "string" } },
                    "reason": { "type": "string" }
                },
                "required": ["attachment_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "backup_campaign",
            "description": "Flush SQLite WAL and write a timestamped safety backup copy of campaign.db before performing major reorganizations or edits (also returns available backups).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "label": { "type": "string", "default": "manual", "description": "Short label included in the backup filename (e.g. 'before-reorg')." }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "restore_backup",
            "description": "List available campaign.backup-*.db snapshots (when backup_filename is omitted) OR restore the entire campaign database bit-for-bit from a chosen backup snapshot (automatically saves a pre-restore safety backup first, recompiles docs, and prunes stale .md files).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "backup_filename": { "type": "string", "description": "Bare backup filename (e.g. 'campaign.backup-before-audit-20261121.db'). Omit to list all available backups." }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "upsert_ruleset_profile",
            "description": "Define or update a campaign-specific, system-agnostic check resolution profile ('roll_over', 'roll_under', 'dice_pool', or 'step_dice') with custom base dice, advantage/disadvantage dice, critical thresholds, PbtA partial bands, and rank/condition modifier tables. Pass list=true to view all profiles.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "list": { "type": "boolean", "description": "Set true to list all saved ruleset profiles plus the built-in default." },
                    "id": { "type": "string", "description": "Profile slug (e.g. 'default', 'mortal-2d6', 'brp-d100', 'cultivation-tribulation')." },
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "resolution_mode": { "type": "string", "enum": ["roll_over", "roll_under", "dice_pool", "step_dice"], "default": "roll_over" },
                    "base_dice": { "type": "string", "default": "1d20", "description": "Base dice expression (e.g. '1d20', '2d6', '1d100', '1d6' for dice_pool)." },
                    "advantage_dice": { "type": "string" },
                    "disadvantage_dice": { "type": "string" },
                    "crit_success_natural": { "type": "integer" },
                    "crit_failure_natural": { "type": "integer" },
                    "crit_margin": { "type": "integer" },
                    "partial_band_margin": { "type": "integer" },
                    "pbta_full_success_at": { "type": "integer" },
                    "pbta_partial_success_at": { "type": "integer" },
                    "pool_success_threshold": { "type": "integer" },
                    "rank_bonuses": { "type": "object", "additionalProperties": { "type": "integer" } },
                    "condition_penalties": { "type": "object", "additionalProperties": { "type": "integer" } },
                    "auto_hydrate": { "type": "boolean", "default": true }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "resolve_check",
            "description": "Resolve a structured, system-agnostic action check. Automatically hydrates the actor's live stat, skill, technique, relevant equipment, active injuries/conditions, AND their current place's live weather/conditions into an itemized modifier breakdown. Supports advantage/disadvantage, dry_run odds preview, deterministic seeds, and causal ledger recording.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "actor_id": { "type": "string", "description": "Character slug or name performing the action." },
                    "action": { "type": "string", "description": "Concrete action attempted (e.g. 'Climb the loose wet ridge path')." },
                    "difficulty": { "type": "integer", "description": "Target DC, roll-under target, or required hits." },
                    "profile": { "type": "string", "default": "default", "description": "Ruleset profile ID (defaults to campaign 'default')." },
                    "stat": { "type": "string", "description": "Optional character stat name to auto-hydrate." },
                    "skill": { "type": "string", "description": "Optional character skill name to auto-hydrate." },
                    "technique": { "type": "string", "description": "Optional character technique name to auto-hydrate." },
                    "equipment": { "type": "array", "items": { "type": "string" }, "description": "Optional specific carried items to factor in (including durability penalties)." },
                    "modifier": { "type": "integer", "default": 0, "description": "Additional manual modifier." },
                    "situational_modifiers": { "type": "array", "items": { "type": "object", "properties": { "source": { "type": "string" }, "value": { "type": "integer" }, "category": { "type": "string" }, "notes": { "type": "string" } }, "required": ["source", "value"] } },
                    "advantage": { "type": "string", "enum": ["none", "advantage", "disadvantage"], "default": "none" },
                    "dice_override": { "type": "string", "description": "Optional override dice expression (e.g. '1d10' for step-dice)." },
                    "auto_hydrate": { "type": "boolean", "description": "Override whether live character/weather modifiers are auto-applied." },
                    "stakes": { "type": "string", "description": "Explicit stakes at risk on failure/success." },
                    "seed": { "type": "integer", "description": "Optional deterministic RNG seed (auto-generated and returned if omitted)." },
                    "dry_run": { "type": "boolean", "default": false, "description": "Preview itemized modifiers and success odds WITHOUT rolling or logging." },
                    "strict_validation": { "type": "boolean", "default": false, "description": "When true, return an error if any requested stat, skill, technique, or equipment item does not exist on the actor (otherwise returns explicit warnings and treats missing references as +0)." },
                    "log_to_ledger": { "type": "boolean", "default": true },
                    "causal_event_ids": { "type": "array", "items": { "type": "integer" } },
                    "visibility": { "type": "string", "enum": ["public", "witnessed_only", "gm_secret"], "default": "public" }
                },
                "required": ["action", "difficulty"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "resolve_contest",
            "description": "Resolve an opposed / contested check between 2+ participants (grappling, stealth vs perception, persuasion vs resistance, pursuit, cultivation contests) with auto-hydrated modifiers, deterministic derived seeds, tie-breakers, margins, and ledger recording.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "description": "What is being contested (e.g. 'Sneak past camp sentries')." },
                    "profile": { "type": "string", "default": "default" },
                    "participants": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "actor_id": { "type": "string" },
                                "role": { "type": "string" },
                                "stat": { "type": "string" },
                                "skill": { "type": "string" },
                                "technique": { "type": "string" },
                                "equipment": { "type": "array", "items": { "type": "string" } },
                                "modifier": { "type": "integer", "default": 0 },
                                "advantage": { "type": "string", "enum": ["none", "advantage", "disadvantage"], "default": "none" },
                                "auto_hydrate": { "type": "boolean" }
                            },
                            "required": ["actor_id"]
                        }
                    },
                    "initiator": { "type": "object", "description": "Optional shorthand instead of participants[0]." },
                    "defender": { "type": "object", "description": "Optional shorthand instead of participants[1]." },
                    "tie_breaker": { "type": "string", "enum": ["higher_modifier", "defender", "draw"], "default": "higher_modifier" },
                    "stakes": { "type": "string" },
                    "seed": { "type": "integer" },
                    "strict_validation": { "type": "boolean", "default": false, "description": "When true, return an error if any participant references an unknown stat, skill, technique, or equipment item (otherwise returns explicit warnings)." },
                    "log_to_ledger": { "type": "boolean", "default": true },
                    "causal_event_ids": { "type": "array", "items": { "type": "integer" } },
                    "visibility": { "type": "string", "enum": ["public", "witnessed_only", "gm_secret"], "default": "public" }
                },
                "required": ["action"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "upsert_random_table",
            "description": "Create or version-update a persistent random table (loot, encounters, travel, rumors, wilderness, settlement, cultivation, social) supporting weights, dice ranges, context conditions, dynamic weight multipliers, inline dice templates ('{{2d6}} coins'), nested subtables, and loot templates. Pass list=true or get_table_id to inspect tables.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "list": { "type": "boolean" },
                    "get_table_id": { "type": "string" },
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "category": { "type": "string", "description": "loot | encounters | travel | rumors | npc_activities | wilderness | settlement | cultivation | social" },
                    "description": { "type": "string" },
                    "context": { "type": "array", "items": { "type": "string" } },
                    "dice": { "type": "string", "description": "Optional dice expression for range-based tables (e.g. '1d100')." },
                    "is_loot_table": { "type": "boolean", "description": "Auto-true when category=='loot'; draws create Staged Discovery Receipts instead of auto-granting items." },
                    "entries": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "result": { "type": "string" },
                                "weight": { "type": "integer" },
                                "range": { "type": "array", "items": { "type": "integer" }, "minItems": 2, "maxItems": 2 },
                                "rarity": { "type": "string", "default": "common" },
                                "conditions": { "type": "object" },
                                "weight_multipliers": { "type": "array", "items": { "type": "object" } },
                                "subtable_id": { "type": "string" },
                                "loot_template": { "type": "object" },
                                "notes": { "type": "string" }
                            },
                            "required": ["result"]
                        }
                    }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "draw_random_table",
            "description": "Draw 1+ results from a persistent random table using live context filtering (auto-hydrates current season, place weather/type/tags), dynamic weight multipliers, stateful cross-session without-replacement decks, nested subtables, and deterministic seeds. Loot tables create a Staged Discovery Receipt without auto-granting items to any character.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "table_id": { "type": "string" },
                    "context": {
                        "type": "object",
                        "properties": {
                            "location": { "type": "string" },
                            "place_type": { "type": "string" },
                            "season": { "type": "string" },
                            "weather": { "type": "string" },
                            "party": { "type": "array", "items": { "type": "string" } },
                            "tags": { "type": "array", "items": { "type": "string" } },
                            "threat_level": { "type": "integer" },
                            "source_description": { "type": "string", "description": "Specific container/source (e.g. 'Salt merchant cart on Old Imperial Road')." }
                        }
                    },
                    "count": { "type": "integer", "default": 1 },
                    "replacement": { "type": "boolean", "default": true, "description": "Set false for persistent without-replacement deck draws across turns/sessions." },
                    "deck_scope": { "type": "string", "description": "Optional deck scope key (defaults to location or 'global')." },
                    "reset_deck": { "type": "boolean", "default": false },
                    "seed": { "type": "integer" },
                    "stage_discovery": { "type": "boolean", "description": "Force staging a discovery receipt even for non-loot tables." },
                    "log_to_ledger": { "type": "boolean", "default": true },
                    "causal_event_ids": { "type": "array", "items": { "type": "integer" } }
                },
                "required": ["table_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "claim_discovery",
            "description": "Atomically claim GM-approved items from a Staged Discovery Receipt (created by draw_random_table on a loot table) into a character's inventory and CatalogItem codex with full provenance. Prevents double-claiming. Pass list_pending=true to view unclaimed discoveries.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "list_pending": { "type": "boolean" },
                    "status": { "type": "string", "enum": ["pending", "partially_claimed", "claimed", "expired"] },
                    "discovery_id": { "type": "string" },
                    "claimant_id": { "type": "string", "description": "Character receiving the claimed items." },
                    "item_indices": { "type": "array", "items": { "type": "integer" }, "description": "Specific item indices from the discovery receipt to claim (omit to claim all remaining unclaimed items)." },
                    "create_catalog_items": { "type": "boolean", "default": true },
                    "notes": { "type": "string" }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "manage_encounter",
            "description": "Unified encounter & initiative manager supporting BOTH combat and non-combat encounters (negotiations, pursuits, survival, cultivation contests). Actions: start | act | advance_turn | update_participant | get | list | end. Syncs resource/condition deltas directly to live characters and compiles docs/encounters/<id>.md.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["start", "act", "advance_turn", "update_participant", "get", "list", "end"] },
                    "encounter_id": { "type": "string" },
                    "title": { "type": "string" },
                    "encounter_type": { "type": "string", "default": "combat", "description": "combat | negotiation | pursuit | survival | cultivation_contest | stealth" },
                    "location_id": { "type": "string" },
                    "participants": { "type": "array", "items": { "type": "object" } },
                    "participant": { "type": "object", "description": "Used with action='update_participant'." },
                    "initiative_dice": { "type": "string", "description": "Optional dice expression to auto-roll initiative added to each participant's base initiative (e.g. '1d20')." },
                    "zones": { "type": "array", "items": { "type": "string" } },
                    "environmental_notes": { "type": "string" },
                    "actor_id": { "type": "string" },
                    "action_type": { "type": "string", "enum": ["action", "reaction", "movement", "free"], "default": "action" },
                    "description": { "type": "string" },
                    "target_ids": { "type": "array", "items": { "type": "string" } },
                    "new_position": { "type": "string" },
                    "new_range_band": { "type": "string" },
                    "resource_deltas": { "type": "array", "items": { "type": "object", "properties": { "character_id": { "type": "string" }, "resource": { "type": "string" }, "delta": { "type": "integer" } } } },
                    "add_conditions": { "type": "array", "items": { "type": "object" } },
                    "remove_conditions": { "type": "array", "items": { "type": "object" } },
                    "advance_turn": { "type": "boolean", "default": false },
                    "advance_world_minutes": { "type": "integer" },
                    "causal_event_ids": { "type": "array", "items": { "type": "integer" } },
                    "status": { "type": "string" },
                    "outcome": { "type": "string" },
                    "aftermath": { "type": "string" },
                    "seed": { "type": "integer" }
                },
                "required": ["action"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "update_character_profile",
            "description": "Set or merge structured Identity, Physical (constitution, body_tempering, disabilities), Psychological/Autonomy drivers (long_term_motivations, values_and_principles, fears, loyalties, ambitions, commitments, secrets, risk_tolerance, stress, morale, current_plans), and prioritized structured_goals (or upsert_goal) for a character without mutating their immutable baseline. Pass get_only=true to read.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": { "type": "string" },
                    "get_only": { "type": "boolean" },
                    "aliases": { "type": "array", "items": { "type": "string" } },
                    "age_or_birthdate": { "type": "string" },
                    "occupation": { "type": "string" },
                    "social_class": { "type": "string" },
                    "body_constitution": { "type": "string" },
                    "body_tempering": { "type": "string" },
                    "temperament": { "type": "string" },
                    "long_term_motivations": { "type": "array", "items": { "type": "string" } },
                    "values_and_principles": { "type": "array", "items": { "type": "string" } },
                    "fears": { "type": "array", "items": { "type": "string" } },
                    "loyalties": { "type": "array", "items": { "type": "string" } },
                    "ambitions": { "type": "array", "items": { "type": "string" } },
                    "commitments": { "type": "array", "items": { "type": "string" } },
                    "secrets": { "type": "array", "items": { "type": "string" } },
                    "risk_tolerance": { "type": "string", "enum": ["cautious", "measured", "bold", "reckless"] },
                    "stress": { "type": "integer" },
                    "morale": { "type": "integer" },
                    "current_plans": { "type": "array", "items": { "type": "string" } },
                    "upsert_goal": { "type": "object", "description": "Upsert a single StructuredGoal ({goal_id, description, priority, progress, target, deadline, resources_required, known_to, status, notes})." },
                    "structured_goals": { "type": "array", "items": { "type": "object" } }
                },
                "required": ["character_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "record_knowledge",
            "description": "Record a world fact, rumor, or secret in the Epistemic Knowledge Graph along with per-actor beliefs (stance: observed|testimony|documented|rumor|inferred|suspected|disbelieved|misinformed|concealing; confidence 0..100; optional perceived_claim for partial/distorted observation!). Supports supersedes_claim_id to automatically flag outdated beliefs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "claim_id": { "type": "string" },
                    "subject": { "type": "string", "description": "Entity ID or topic this claim is about." },
                    "claim": { "type": "string", "description": "Canonical statement of the fact or circulating rumor." },
                    "truth_status": { "type": "string", "enum": ["true", "false", "partial", "unverified"], "default": "true" },
                    "visibility": { "type": "string", "enum": ["public", "local", "restricted", "gm_secret"], "default": "restricted" },
                    "source": { "type": "string" },
                    "source_event_id": { "type": "integer" },
                    "supersedes_claim_id": { "type": "string", "description": "Marks an older claim outdated for actors who have not learned this update." },
                    "known_by": { "type": "array", "items": { "type": "string" }, "description": "Shorthand list of actor IDs who hold this claim." },
                    "disbelieved_by": { "type": "array", "items": { "type": "string" } },
                    "stance": { "type": "string", "enum": ["observed", "testimony", "documented", "rumor", "inferred", "suspected", "disbelieved", "misinformed", "concealing"], "default": "observed" },
                    "confidence": { "type": "integer", "default": 80 },
                    "perceived_claim": { "type": "string", "description": "Partial or distorted version actually believed by known_by actors." },
                    "concealing": { "type": "boolean", "default": false },
                    "beliefs": { "type": "array", "items": { "type": "object" }, "description": "Explicit per-actor ActorBelief records for fine-grained mixed stances/partial truths." },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "related_ids": { "type": "array", "items": { "type": "string" } },
                    "causal_event_ids": { "type": "array", "items": { "type": "integer" } }
                },
                "required": ["subject", "claim"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "propagate_knowledge",
            "description": "Spread a knowledge claim from a source actor (or rumor mill) to recipient actors with confidence decay, optional distortion, and deception tracking. Enforces epistemic honesty: a source actor cannot truthfully spread a claim they do not know (unless deceptive=true).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "claim_id": { "type": "string" },
                    "from_actor_id": { "type": "string" },
                    "to_actor_ids": { "type": "array", "items": { "type": "string" } },
                    "stance": { "type": "string" },
                    "confidence_decay": { "type": "integer", "default": 15 },
                    "distorted_claim": { "type": "string", "description": "Optional drift/distortion as the rumor passes along." },
                    "deceptive": { "type": "boolean", "default": false },
                    "notes": { "type": "string" }
                },
                "required": ["claim_id", "to_actor_ids"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "search_knowledge",
            "description": "Query the Epistemic Knowledge Graph: what a specific actor knows (actor_id), what they do NOT know yet (not_known_by blind-spot finder!), by subject, stance, truth_status, visibility, min_confidence, or FTS5 query.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "actor_id": { "type": "string" },
                    "not_known_by": { "type": "string", "description": "Return claims this actor does NOT know (blind-spot query)." },
                    "subject": { "type": "string" },
                    "stance": { "type": "string" },
                    "truth_status": { "type": "string" },
                    "visibility": { "type": "string" },
                    "min_confidence": { "type": "integer" },
                    "include_outdated": { "type": "boolean", "default": true },
                    "limit": { "type": "integer", "default": 30 }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "manage_faction",
            "description": "Dedicated first-class Factions & Institutions register (separate from lore). Actions: create | update | get | list | add_member | remove_member | modify_resource | promote_from_lore. Tracks baseline identity, capacity_tier, roster/ranks, territory, resource pools, reputation, structured objectives, morale, and internal conflicts; compiles docs/factions/<id>.md.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["create", "update", "get", "list", "add_member", "remove_member", "modify_resource", "promote_from_lore"] },
                    "id": { "type": "string" },
                    "faction_id": { "type": "string" },
                    "name": { "type": "string" },
                    "faction_type": { "type": "string" },
                    "capacity_tier": { "type": "string", "description": "Realistic scale, e.g. 'micro-band (9 mortals)', 'local-guild', 'regional-sect'." },
                    "origins": { "type": "string" },
                    "leadership_model": { "type": "string" },
                    "ideology": { "type": "string" },
                    "leader_id": { "type": "string" },
                    "members": { "type": "array", "items": { "type": "object" } },
                    "territory": { "type": "array", "items": { "type": "string" } },
                    "resources": { "type": "object", "additionalProperties": { "type": "integer" } },
                    "reputation": { "type": "object", "additionalProperties": { "type": "integer" } },
                    "objectives": { "type": "array", "items": { "type": "object" } },
                    "internal_conflicts": { "type": "array", "items": { "type": "string" } },
                    "morale": { "type": "integer", "default": 60 },
                    "character_id": { "type": "string" },
                    "rank": { "type": "string" },
                    "role": { "type": "string" },
                    "loyalty_note": { "type": "string" },
                    "resource": { "type": "string" },
                    "delta": { "type": "integer" },
                    "allow_debt": { "type": "boolean", "default": false },
                    "lore_id": { "type": "string" },
                    "remove_old_lore": { "type": "boolean", "default": false },
                    "reason": { "type": "string" }
                },
                "required": ["action"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "simulate_autonomy",
            "description": "Two-Phase Constrained Agency Sandbox for off-screen NPC, Group (e.g. bandit band), and Faction autonomy. Phase 1 (mode='prepare'): returns a strictly leak-proof POV dossier containing ONLY what those actors know, exact resources/capacity/injuries, tensions, and projected upkeep. Phase 2 (mode='commit'): enforces hard resource & epistemic guards (blocks acting on unknown claims or spending non-existent resources!) and commits deltas atomically. mode='auto_mechanical' applies routine upkeep ticks.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mode": { "type": "string", "enum": ["prepare", "commit", "auto_mechanical"], "default": "prepare" },
                    "scope_type": { "type": "string", "enum": ["npc", "group", "faction"], "default": "npc" },
                    "faction_id": { "type": "string" },
                    "actor_ids": { "type": "array", "items": { "type": "string" } },
                    "location_id": { "type": "string" },
                    "duration_hours": { "type": "integer", "default": 24 },
                    "duration_days": { "type": "integer" },
                    "actions": { "type": "array", "items": { "type": "object", "properties": { "actor_id": { "type": "string" }, "action": { "type": "string" }, "outcome_summary": { "type": "string" }, "relied_on_claim_ids": { "type": "array", "items": { "type": "string" } } }, "required": ["actor_id", "action"] } },
                    "faction_resource_deltas": { "type": "array", "items": { "type": "object", "properties": { "resource": { "type": "string" }, "delta": { "type": "integer" } } } },
                    "character_resource_deltas": { "type": "array", "items": { "type": "object", "properties": { "character_id": { "type": "string" }, "resource": { "type": "string" }, "delta": { "type": "integer" } } } },
                    "goal_updates": { "type": "array", "items": { "type": "object" } },
                    "morale_delta": { "type": "integer" },
                    "add_internal_conflicts": { "type": "array", "items": { "type": "string" } },
                    "resolve_internal_conflicts": { "type": "array", "items": { "type": "string" } },
                    "allow_debt": { "type": "boolean", "default": false },
                    "advance_world_clock": { "type": "boolean", "default": false },
                    "summary": { "type": "string" },
                    "causal_event_ids": { "type": "array", "items": { "type": "integer" } }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "manage_travel_and_exploration",
            "description": "Connected geography, graph pathfinding (Dijkstra), travel execution, and grounded exploration. Actions: upsert_route (distance_km, terrain, elevation, weather_exposure, tolls, status: open|hazardous|blocked|destroyed) | list_routes | update_spatial (coordinates, jurisdiction, population, services, resource_deposits) | calculate (auto-detours around destroyed bridges, factors weather, encumbrance, injuries, forced march) | advance (moves party, advances clock, deducts tolls, rolls hazards) | explore (scout/search/track/forage grounded in place lore & tables).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["upsert_route", "list_routes", "update_spatial", "calculate", "advance", "explore"] },
                    "id": { "type": "string" },
                    "from_place_id": { "type": "string" },
                    "to_place_id": { "type": "string" },
                    "place_id": { "type": "string" },
                    "bidirectional": { "type": "boolean", "default": true },
                    "distance_km": { "type": "number" },
                    "terrain": { "type": "string" },
                    "elevation_gain_m": { "type": "integer" },
                    "weather_exposure": { "type": "number" },
                    "toll_cost": { "type": "integer" },
                    "toll_currency": { "type": "string" },
                    "status": { "type": "string", "enum": ["open", "hazardous", "blocked", "destroyed"] },
                    "hazards": { "type": "array", "items": { "type": "string" } },
                    "jurisdiction": { "type": "string" },
                    "patrol_presence": { "type": "string" },
                    "character_ids": { "type": "array", "items": { "type": "string" } },
                    "actor_id": { "type": "string" },
                    "transport_mode": { "type": "string", "enum": ["foot", "mount", "cart", "boat", "flying"], "default": "foot" },
                    "forced_march": { "type": "boolean", "default": false },
                    "pay_tolls_from": { "type": "string" },
                    "hazard_table_id": { "type": "string" },
                    "mode": { "type": "string", "description": "For explore: scout | search | track | forage | survey." },
                    "skill": { "type": "string" },
                    "difficulty": { "type": "integer" },
                    "table_id": { "type": "string" },
                    "coordinates": { "type": "array", "items": { "type": "number" } },
                    "population": { "type": "integer" },
                    "resource_deposits": { "type": "array", "items": { "type": "string" } },
                    "services": { "type": "array", "items": { "type": "string" } },
                    "seed": { "type": "integer" },
                    "notes": { "type": "string" }
                },
                "required": ["action"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "manage_economy",
            "description": "Encumbrance, atomic multi-currency ledger, transactions, and dynamic supply/demand markets (automatically spikes commodity prices when connected bridges/supply routes are destroyed!). Actions: set_item_weight | check_encumbrance | get_balances | set_balance | transfer_currency | execute_transaction | upsert_market | get_market_prices | update_market_supply.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["set_item_weight", "check_encumbrance", "get_balances", "set_balance", "transfer_currency", "execute_transaction", "upsert_market", "get_market_prices", "update_market_supply"] },
                    "item_key": { "type": "string" },
                    "weight_kg": { "type": "number" },
                    "unit_value": { "type": "integer" },
                    "character_id": { "type": "string" },
                    "owner_id": { "type": "string" },
                    "from_id": { "type": "string" },
                    "to_id": { "type": "string" },
                    "buyer_id": { "type": "string" },
                    "seller_id": { "type": "string" },
                    "kind": { "type": "string", "default": "purchase" },
                    "amount": { "type": "integer" },
                    "currency": { "type": "string", "default": "copper_cash" },
                    "tax_rate_pct": { "type": "integer" },
                    "tax_recipient_id": { "type": "string" },
                    "item_name": { "type": "string" },
                    "quantity": { "type": "integer", "default": 1 },
                    "place_id": { "type": "string" },
                    "name": { "type": "string" },
                    "supply_origins": { "type": "array", "items": { "type": "string" } },
                    "commodities": { "type": "array", "items": { "type": "object" } },
                    "commodity": { "type": "string" },
                    "supply_delta": { "type": "integer" },
                    "demand_delta": { "type": "integer" },
                    "reason": { "type": "string" },
                    "notes": { "type": "string" }
                },
                "required": ["action"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "manage_law_and_reputation",
            "description": "Track jurisdictions, laws, crimes, warrants, bounties, and faction/regional reputation standings. Actions: upsert_jurisdiction | check_jurisdiction | record_crime | update_warrant | modify_reputation | get_reputation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["upsert_jurisdiction", "check_jurisdiction", "record_crime", "update_warrant", "modify_reputation", "get_reputation"] },
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "controlling_faction_id": { "type": "string" },
                    "place_id": { "type": "string" },
                    "place_ids": { "type": "array", "items": { "type": "string" } },
                    "laws": { "type": "array", "items": { "type": "string" } },
                    "tax_rate_pct": { "type": "integer" },
                    "enforcement_strength": { "type": "string" },
                    "crime_id": { "type": "string" },
                    "jurisdiction_id": { "type": "string" },
                    "suspect_id": { "type": "string" },
                    "offense": { "type": "string" },
                    "severity": { "type": "string" },
                    "warrant_status": { "type": "string", "enum": ["reported", "investigating", "warrant_issued", "captured", "resolved", "pardoned"] },
                    "bounty": { "type": "integer" },
                    "bounty_currency": { "type": "string" },
                    "witnesses": { "type": "array", "items": { "type": "string" } },
                    "entity_id": { "type": "string" },
                    "target_group_id": { "type": "string" },
                    "delta": { "type": "integer" },
                    "notes": { "type": "string" }
                },
                "required": ["action"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "manage_health_and_downtime",
            "description": "Persistent injuries & recovery (synced with live StatusConditions), downtime activities, multi-hour crafting projects, and system-agnostic progression/breakthrough checks (cultivation, body-tempering, rank trials). Actions: apply_injury | treat_injury | advance_recovery | list_injuries | resolve_downtime | begin_crafting | resolve_crafting | resolve_progression.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["apply_injury", "treat_injury", "advance_recovery", "list_injuries", "resolve_downtime", "begin_crafting", "resolve_crafting", "resolve_progression"] },
                    "character_id": { "type": "string" },
                    "crafter_id": { "type": "string" },
                    "injury_id": { "type": "string" },
                    "project_id": { "type": "string" },
                    "name": { "type": "string" },
                    "item_name": { "type": "string" },
                    "item_type": { "type": "string" },
                    "severity": { "type": "string", "enum": ["minor", "moderate", "severe", "critical"] },
                    "body_location": { "type": "string" },
                    "complications": { "type": "array", "items": { "type": "string" } },
                    "recovery_days": { "type": "integer" },
                    "days_reduced": { "type": "integer" },
                    "days": { "type": "integer" },
                    "hours_worked": { "type": "integer" },
                    "required_hours": { "type": "integer" },
                    "activity": { "type": "string" },
                    "progression_kind": { "type": "string", "description": "e.g. 'cultivation_breakthrough', 'body_tempering', 'rank_trial'." },
                    "profile": { "type": "string" },
                    "stat": { "type": "string" },
                    "skill": { "type": "string" },
                    "skill_name": { "type": "string" },
                    "technique": { "type": "string" },
                    "difficulty": { "type": "integer" },
                    "modifier": { "type": "integer" },
                    "active_only": { "type": "boolean" },
                    "seed": { "type": "integer" },
                    "notes": { "type": "string" }
                },
                "required": ["action"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "manage_weather",
            "description": "Generate, advance, or inspect seasonal Markov weather for a place (respecting its climate and active season, updating live PlaceCurrent weather so checks and travel immediately respond). Actions: generate | advance | get.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["generate", "advance", "get"], "default": "advance" },
                    "place_id": { "type": "string" },
                    "seed": { "type": "integer" }
                },
                "required": ["place_id"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "advance_world",
            "description": "Bounded-scope world simulation tick: advances world clock by duration_hours/duration_days and orchestrates regional weather, injury recovery, market production/consumption drift, routine faction ration upkeep, and optional leak-proof autonomy dossiers strictly within the specified places, factions, and characters (never unbounded).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "duration_hours": { "type": "integer" },
                    "duration_days": { "type": "integer" },
                    "place_ids": { "type": "array", "items": { "type": "string" } },
                    "faction_ids": { "type": "array", "items": { "type": "string" } },
                    "character_ids": { "type": "array", "items": { "type": "string" } },
                    "tick_weather": { "type": "boolean", "default": true },
                    "tick_recovery": { "type": "boolean", "default": true },
                    "tick_markets": { "type": "boolean", "default": true },
                    "tick_upkeep": { "type": "boolean", "default": true },
                    "prepare_autonomy_dossiers": { "type": "boolean", "default": false },
                    "seed": { "type": "integer" }
                }
            }
        }"#));
    v
}

/// Returns the canonical list of all Chronicle tools.
pub fn all_tool_definitions() -> Value {
    tool_definitions()
}

/// Exports Chronicle's full toolset in universal AI schemas:
/// - "mcp"       -> Model Context Protocol tools/list schema
/// - "openai"    -> OpenAI Chat Completions / Responses function-calling tools array
/// - "anthropic" -> Anthropic Messages API tool-use array
pub fn export_tool_schemas(format: &str) -> Result<Value> {
    let tools = match tool_definitions() {
        Value::Array(arr) => arr,
        _ => Vec::new(),
    };

    match format.to_lowercase().as_str() {
        "mcp" => Ok(json!({ "tools": tools })),
        "openai" => {
            let mapped: Vec<Value> = tools
                .into_iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t["name"],
                            "description": t["description"],
                            "parameters": t["inputSchema"]
                        }
                    })
                })
                .collect();
            Ok(Value::Array(mapped))
        }
        "anthropic" => {
            let mapped: Vec<Value> = tools
                .into_iter()
                .map(|t| {
                    json!({
                        "name": t["name"],
                        "description": t["description"],
                        "input_schema": t["inputSchema"]
                    })
                })
                .collect();
            Ok(Value::Array(mapped))
        }
        other => Err(anyhow!(
            "Unsupported export format '{other}'. Expected 'mcp', 'openai', or 'anthropic'."
        )),
    }
}

fn req_str(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("Missing required string parameter '{key}'"))
}

fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn opt_str_vec(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn opt_i64_vec(args: &Value, key: &str) -> Vec<i64> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|item| item.as_i64()).collect())
        .unwrap_or_default()
}
