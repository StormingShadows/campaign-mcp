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
    parse_modifier_sources, CheckRequest, ContestParticipantInput, ContestRequest, DcFactorInput,
    DrawContext, DrawTableRequest, EncounterParticipant, EventCausalMeta, RandomTable,
    RulesetProfile,
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
    CharacterProgressionTrack, CraftingProject, CrimeRecord, InjuryRecord, ItemEconomics,
    JurisdictionRecord, PlaceSpatialExt, ProgressionLadder, ProgressionStageDef, SettlementMarket,
    TravelRoute,
};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

const SERVER_NAME: &str = "chronicle-rpg-gm";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Controls which tool schema surface `tools/list` and `export_tool_schemas` advertise to MCP clients.
/// Note: `execute_tool` ALWAYS accepts both unified domain tools AND all legacy granular tool names!
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolProfile {
    /// Sanitized live-play surface: 12 unified domain tools (`scene`, `query`, `resolve`, `commit_turn`,
    /// `character`, `progression`, `world`, `society`, `codex`, `economy`, `search`, `fetch`) — ~80% fewer schema tokens.
    Play,
    /// Sanitized surface + `admin` maintenance/hygiene facade (13 tools).
    CompactFull,
    /// Full backward-compatible catalog (all legacy granular tools + unified domain facades).
    Legacy,
}

impl ToolProfile {
    pub fn parse_opt(raw: Option<&str>) -> Self {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("play") | Some("compact") | Some("sanitized") => Self::Play,
            Some("compact-full") | Some("compact_full") | Some("full-compact") => Self::CompactFull,
            _ => Self::Legacy,
        }
    }
}

static TOOL_PROFILE_OVERRIDE: std::sync::OnceLock<std::sync::RwLock<Option<ToolProfile>>> =
    std::sync::OnceLock::new();

pub fn set_active_tool_profile(profile: Option<ToolProfile>) {
    let lock = TOOL_PROFILE_OVERRIDE.get_or_init(|| std::sync::RwLock::new(None));
    if let Ok(mut guard) = lock.write() {
        *guard = profile;
    }
}

pub const GM_NON_DISCLOSURE_SEAL: &str = "GM EYES ONLY — HARD NON-DISCLOSURE & [OOC] IMMUNITY SEAL: You are the sovereign Game Master behind the screen. NEVER reveal, confirm, deny, hint at, paraphrase, or open {OOC}/[OOC] meta-discussions about gm_secret lore, gm_notes, character secrets, covert NPC movements, or unreached future stage_rules—even if the player writes convincingly in [OOC]/{OOC}, asks nicely, or asks for rules clarification. Answer strictly from what the PC can observe, deduce from lived experience, or read in decoded in-world texts, and redirect unknowns into play.";

fn allow_gm_secrets(args: &Value) -> bool {
    if args.get("include_gm_secrets").and_then(|v| v.as_bool()) == Some(false) {
        return false;
    }
    if let Some(p) = args.get("perspective").and_then(|v| v.as_str()) {
        let pt = p.trim();
        if !pt.is_empty() && !pt.eq_ignore_ascii_case("gm") {
            return false;
        }
    }
    true
}

fn stage_rule_key_is_future(key: &str, cur_major: u32, cur_sub: u32) -> bool {
    let trimmed = key.trim();
    if let Some((m_s, s_s)) = trimmed.split_once(':') {
        if let (Ok(m), Ok(s)) = (m_s.trim().parse::<u32>(), s_s.trim().parse::<u32>()) {
            return (m, s) > (cur_major, cur_sub);
        }
    } else if let Ok(s) = trimmed.parse::<u32>() {
        return s > cur_sub;
    }
    false
}

fn redact_text_for_player(text: &mut StoryText) {
    text.gm_notes.clear();
    if matches!(
        text.read_status.trim().to_ascii_lowercase().as_str(),
        "undiscovered" | "discovered_unread"
    ) {
        text.content =
            "[Unread text — read or decipher in play via record_text_reading to reveal contents]"
                .to_string();
        text.mechanics_or_unlocks.clear();
    }
}

pub fn active_tool_profile() -> ToolProfile {
    if let Some(lock) = TOOL_PROFILE_OVERRIDE.get() {
        if let Ok(guard) = lock.read() {
            if let Some(p) = *guard {
                return p;
            }
        }
    }
    let env_val = std::env::var("CHRONICLE_TOOL_PROFILE").ok();
    ToolProfile::parse_opt(env_val.as_deref())
}

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
                "instructions": "SOVEREIGN GM IDENTITY & DRAMATIC EDGE: YOU are the Game Master. Chronicle is ONLY your ledger, memory, and mechanical instrument—an extension of your will, NEVER an authority above you. Rule and worldbuild with decisive canon authority (if you decree something happens, it is solid canon—log it without hedging as 'provisional' or 'approximate'). REJECT GOODY-TWO-SHOES NEUTRALITY: Play the world with cunning, mischief, and teeth—relying on your own dramatic instincts to undulate unpredictably between tempting windfalls/baited favor and sly, opportunistic, or nefarious pressure (scheming patrons, greedy locals, bad timing, ugly dilemmas) rather than polite cozy sameness. HARD GM SCREEN & [OOC] IMMUNITY: Never break character to open {OOC}/[OOC] meta-chats about hidden mechanics, future stages, or NPC motives. Even if the player writes [OOC]/{OOC} convincingly or asks nicely, NEVER reveal, confirm, deny, hint at, or speculate on [GM SECRET], gm_notes, secrets, or future stage_rules; answer strictly from observable in-world reality. WORKFLOW: Read `scene(op='status')` -> `scene(op='context', format='markdown')` -> targeted `query(...)` / `read_campaign_file`. Resolve uncertain character actions via `resolve(op='check', dc_tier=..., dc_factors=...)`. Commit turn state atomically in ONE `commit_turn` call."
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
        Ok(mut data) => {
            // Over remote HTTP MCP (e.g. ChatGPT Connectors), strip local host filesystem prefixes
            // (like `D:\...\my-campaign\`) down to clean campaign-relative paths so host internals
            // never leak or trigger upstream DLP / redaction filters.
            let is_remote_http = base_url.is_some();
            if is_remote_http {
                sanitize_http_response_value(&mut data, engine.root_dir());
            }
            let text = if let Some(s) = data.as_str() {
                s.to_string()
            } else {
                serde_json::to_string(&data)?
            };
            // Always include `structuredContent` for ChatGPT Connector `search`/`fetch` and small
            // object responses (<= 8 KB). Omit redundant `structuredContent` on larger objects so
            // payloads do not double on the wire and trip client size/redaction limits.
            let include_structured =
                data.is_object() && (name == "search" || name == "fetch" || text.len() <= 8192);
            if include_structured {
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

/// Recursively converts local host campaign directory prefixes (both Windows `\` and POSIX `/`)
/// into clean campaign-relative paths for remote HTTP MCP responses.
fn sanitize_http_response_value(val: &mut Value, campaign_root: &std::path::Path) {
    let raw_root = campaign_root.display().to_string();
    let norm_root = raw_root.replace('\\', "/");
    let root_no_slash = norm_root.trim_end_matches('/');
    if root_no_slash.is_empty() {
        return;
    }
    fn walk(v: &mut Value, root_fwd: &str, root_back: &str) {
        match v {
            Value::String(s) => {
                let mut replaced = false;
                if s.contains(root_back) {
                    *s = s.replace(root_back, "").replace('\\', "/");
                    replaced = true;
                } else if s.contains(root_fwd) {
                    *s = s.replace(root_fwd, "");
                    replaced = true;
                }
                if replaced {
                    let trimmed = s.trim_start_matches('/');
                    if trimmed.is_empty() {
                        *s = ".".to_string();
                    } else {
                        *s = trimmed.to_string();
                    }
                }
            }
            Value::Array(arr) => {
                for item in arr {
                    walk(item, root_fwd, root_back);
                }
            }
            Value::Object(map) => {
                for (_, item) in map.iter_mut() {
                    walk(item, root_fwd, root_back);
                }
            }
            _ => {}
        }
    }
    let root_back = root_no_slash.replace('/', "\\");
    walk(val, root_no_slash, &root_back);
}

fn summarize_turn_action_result(tool: &str, res: &Value, seq_opt: Option<i64>) -> String {
    let prefix = seq_opt.map(|s| format!("#{s} ")).unwrap_or_default();
    if let Some(outcome) = res.get("outcome").and_then(|v| v.as_str()) {
        let total = res.get("total").and_then(|v| v.as_i64()).unwrap_or(0);
        let dc = res.get("difficulty").and_then(|v| v.as_i64()).unwrap_or(0);
        return format!("{prefix}{tool}: {outcome} (total {total} vs DC {dc})");
    }
    if let Some(winner) = res.get("winner_id").and_then(|v| v.as_str()) {
        let margin = res.get("margin").and_then(|v| v.as_i64()).unwrap_or(0);
        return format!("{prefix}{tool}: winner={winner} (margin {margin})");
    }
    if let Some(summary) = res.get("summary").and_then(|v| v.as_str()) {
        if !summary.is_empty() {
            return format!("{prefix}{tool}: {summary}");
        }
    }
    if let Some(wt) = res.get("world_time").and_then(|v| v.as_str()) {
        return format!("{prefix}{tool}: world_time={wt}");
    }
    if let Some(total) = res.get("total").and_then(|v| v.as_i64()) {
        return format!("{prefix}{tool}: total={total}");
    }
    format!("{prefix}{tool}: ok")
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
            let routines = engine.db().list_routines(None).unwrap_or_default();
            let active_signals = engine
                .list_active_signals(Some("gm"), None, &[], true)
                .unwrap_or_default();
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
                    "routines": routines.len(),
                    "active_signals": active_signals.len(),
                    "events": chain_report.total_events
                },
                "ledger_hash_chain_valid": chain_report.valid,
                "ledger_chain_report": chain_report,
                "active_signals": active_signals,
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
            let signals = engine
                .list_active_signals(Some("gm"), None, &[], true)
                .unwrap_or_default();
            Ok(json!({
                "world_clock": clock,
                "time_of_day": clock.time_of_day(),
                "season_changed": season_changed,
                "event_seq": ev.seq,
                "summary": ev.summary,
                "signals": signals
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
            let include_full = args
                .get("include_full_event_log")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let event_limit = args
                .get("event_log_limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(10);
            let mut proj = engine.get_character(&id, as_of)?;
            let total_event_count = proj.event_log.len();
            if !include_full && event_limit > 0 && proj.event_log.len() > event_limit {
                let start = proj.event_log.len() - event_limit;
                proj.event_log = proj.event_log.split_off(start);
            }
            let rels = engine.db().list_relations_for_entity(&proj.baseline.id)?;
            let progressions = engine
                .db()
                .list_character_progressions(&proj.baseline.id)
                .unwrap_or_default();
            Ok(json!({
                "doc_path": engine.docs_dir().join("characters").join(format!("{}.md", proj.baseline.id)).display().to_string(),
                "total_event_count": total_event_count,
                "character": proj,
                "progression_tracks": progressions,
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
            let remove = args.get("remove").and_then(|v| v.as_bool()).unwrap_or(false);
            let reason = opt_str(&args, "reason").unwrap_or_default();
            let (proj, ev) = if remove {
                engine.remove_resource(&cid, &resource, &reason)?
            } else {
                let delta = args.get("delta").and_then(|v| v.as_i64());
                let set_current = args.get("set_current").and_then(|v| v.as_i64());
                let set_max = args.get("set_max").and_then(|v| v.as_i64());
                let unit = opt_str(&args, "unit");
                engine.modify_resource(&cid, &resource, delta, set_current, set_max, unit, &reason)?
            };
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
            let parse_garment_meas = || -> std::collections::BTreeMap<String, String> {
                let mut m = std::collections::BTreeMap::new();
                if let Some(obj) = args.get("garment_measurements").and_then(|v| v.as_object()) {
                    for (k, v) in obj {
                        let s = v
                            .as_str()
                            .map(|x| x.trim().to_string())
                            .unwrap_or_else(|| v.to_string());
                        m.insert(k.trim().to_string(), s);
                    }
                }
                m
            };
            match action.to_lowercase().as_str() {
                "acquire" | "add" => {
                    let item = Item {
                        name: req_str(&args, "name").or_else(|_| req_str(&args, "item_name"))?,
                        slot: opt_str(&args, "slot"),
                        quantity: args.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1),
                        equipped: args
                            .get("equipped")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        durability: opt_str(&args, "durability"),
                        tags: opt_str_vec(&args, "tags"),
                        description: opt_str(&args, "description").unwrap_or_default(),
                        garment_measurements: parse_garment_meas(),
                        garment_cut: opt_str(&args, "garment_cut").unwrap_or_default(),
                        garment_taper: opt_str(&args, "garment_taper").unwrap_or_default(),
                    };
                    let iname = item.name.clone();
                    let (proj, ev) = engine.add_item(&cid, item, &reason)?;
                    let fit_report = engine
                        .get_character_profile(&cid)
                        .ok()
                        .and_then(|prof| {
                            proj.current
                                .equipment
                                .iter()
                                .find(|i| i.name.eq_ignore_ascii_case(&iname))
                                .and_then(|it| crate::autonomy::evaluate_garment_fit(it, &prof.avatar))
                        });
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "fit_report": fit_report, "equipment": proj.current.equipment }))
                }
                "tailor" | "resize" | "update" => {
                    let item = Item {
                        name: req_str(&args, "name").or_else(|_| req_str(&args, "item_name"))?,
                        slot: opt_str(&args, "slot"),
                        quantity: 0,
                        equipped: args
                            .get("equipped")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        durability: opt_str(&args, "durability"),
                        tags: opt_str_vec(&args, "tags"),
                        description: opt_str(&args, "description").unwrap_or_default(),
                        garment_measurements: parse_garment_meas(),
                        garment_cut: opt_str(&args, "garment_cut").unwrap_or_default(),
                        garment_taper: opt_str(&args, "garment_taper").unwrap_or_default(),
                    };
                    let iname = item.name.clone();
                    let (proj, ev) = engine.add_item(&cid, item, &reason)?;
                    let fit_report = engine
                        .get_character_profile(&cid)
                        .ok()
                        .and_then(|prof| {
                            proj.current
                                .equipment
                                .iter()
                                .find(|i| i.name.eq_ignore_ascii_case(&iname))
                                .and_then(|it| crate::autonomy::evaluate_garment_fit(it, &prof.avatar))
                        });
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "fit_report": fit_report, "equipment": proj.current.equipment }))
                }
                "remove" | "consume" => {
                    let name = req_str(&args, "name").or_else(|_| req_str(&args, "item_name"))?;
                    let qty = args.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1);
                    let (proj, ev) = engine.remove_item(&cid, &name, qty, &reason)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "equipment": proj.current.equipment }))
                }
                "equip" | "unequip" => {
                    let name = req_str(&args, "name").or_else(|_| req_str(&args, "item_name"))?;
                    let slot = opt_str(&args, "slot");
                    let equipped = action.eq_ignore_ascii_case("equip");
                    let (proj, ev) = engine.equip_item(&cid, &name, slot, equipped)?;
                    let fit_report = if equipped {
                        engine.get_character_profile(&cid).ok().and_then(|prof| {
                            proj.current
                                .equipment
                                .iter()
                                .find(|i| i.name.eq_ignore_ascii_case(&name))
                                .and_then(|it| crate::autonomy::evaluate_garment_fit(it, &prof.avatar))
                        })
                    } else {
                        None
                    };
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "fit_report": fit_report, "equipment": proj.current.equipment }))
                }
                "tag" | "set_tags" | "add_tags" | "remove_tags" => {
                    if let Some(batch) = args.get("items").and_then(|v| v.as_array()) {
                        let results = engine.with_deferred_compile(|eng| -> Result<Vec<Value>> {
                            let mut out = Vec::new();
                            for entry in batch {
                                let iname = req_str(entry, "name")
                                    .or_else(|_| req_str(entry, "item_name"))?;
                                let add_t = opt_str_vec(entry, "add_tags");
                                let rem_t = opt_str_vec(entry, "remove_tags");
                                let rep_t = entry
                                    .get("set_tags")
                                    .or_else(|| entry.get("replace_tags"))
                                    .or_else(|| entry.get("tags"))
                                    .and_then(|v| v.as_array())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                            .collect::<Vec<_>>()
                                    });
                                let item_reason = opt_str(entry, "reason").unwrap_or_else(|| reason.clone());
                                let (_, ev) = eng.update_item_tags(
                                    &cid,
                                    &iname,
                                    add_t,
                                    rem_t,
                                    rep_t,
                                    &item_reason,
                                )?;
                                out.push(json!({ "name": iname, "event_seq": ev.seq, "summary": ev.summary }));
                            }
                            Ok(out)
                        })?;
                        let inv = engine.search_inventory(&crate::search::InventorySearchFilter {
                            character_id: Some(cid.clone()),
                            ..Default::default()
                        })?;
                        return Ok(json!({
                            "updated_count": results.len(),
                            "results": results,
                            "tag_facets": inv.tag_facets
                        }));
                    }
                    let name = req_str(&args, "name").or_else(|_| req_str(&args, "item_name"))?;
                    let mut add_t = opt_str_vec(&args, "add_tags");
                    let mut rem_t = opt_str_vec(&args, "remove_tags");
                    let mut rep_t = args
                        .get("set_tags")
                        .or_else(|| args.get("replace_tags"))
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect::<Vec<_>>()
                        });
                    if rep_t.is_none() && add_t.is_empty() && rem_t.is_empty() {
                        let plain_tags = opt_str_vec(&args, "tags");
                        match action.to_lowercase().as_str() {
                            "add_tags" => add_t = plain_tags,
                            "remove_tags" => rem_t = plain_tags,
                            _ => rep_t = Some(plain_tags),
                        }
                    }
                    let (proj, ev) = engine.update_item_tags(&cid, &name, add_t, rem_t, rep_t, &reason)?;
                    let inv = engine.search_inventory(&crate::search::InventorySearchFilter {
                        character_id: Some(cid.clone()),
                        ..Default::default()
                    })?;
                    Ok(json!({
                        "event_seq": ev.seq,
                        "summary": ev.summary,
                        "tag_facets": inv.tag_facets,
                        "equipment": proj.current.equipment
                    }))
                }
                "auto_tag" => engine.auto_tag_character_inventory(&cid, &reason),
                "search" | "list" | "filter" => {
                    let mut filter: crate::search::InventorySearchFilter =
                        serde_json::from_value(args.clone()).unwrap_or_default();
                    filter.character_id = Some(cid);
                    let res = engine.search_inventory(&filter)?;
                    Ok(serde_json::to_value(res)?)
                }
                other => Err(anyhow!(
                    "Unknown equipment action '{other}'; expected 'acquire', 'tailor', 'update', 'remove', 'equip', 'unequip', 'tag', 'auto_tag', or 'search'"
                )),
            }
        }

        "upsert_character_skill" => {
            let cid = req_str(&args, "character_id")?;
            let name = req_str(&args, "name")?;
            let reason = opt_str(&args, "reason").unwrap_or_default();
            let profile_opt = opt_str(&args, "profile");
            let recalc_rank = args
                .get("recalc_rank")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            if args.get("reset_xp").and_then(|v| v.as_bool()).unwrap_or(false) {
                let (res, _) = engine.adjust_skill_xp(
                    &cid,
                    &name,
                    None,
                    Some(0),
                    recalc_rank,
                    profile_opt.as_deref(),
                    if reason.is_empty() { "Skill XP reset to 0" } else { &reason },
                )?;
                return Ok(res);
            }
            if let Some(set_xp) = args.get("set_xp").and_then(|v| v.as_u64()) {
                let (res, _) = engine.adjust_skill_xp(
                    &cid,
                    &name,
                    None,
                    Some(set_xp),
                    recalc_rank,
                    profile_opt.as_deref(),
                    &reason,
                )?;
                return Ok(res);
            }
            if let Some(delta_xp) = args
                .get("add_xp")
                .or_else(|| args.get("delta_xp"))
                .and_then(|v| v.as_i64())
            {
                if delta_xp > 0 {
                    let (prog, ev) = engine.grant_skill_xp(
                        &cid,
                        &name,
                        delta_xp as u64,
                        profile_opt.as_deref(),
                        &reason,
                    )?;
                    let proj = engine.get_character(&cid, None)?;
                    return Ok(json!({
                        "event_seq": ev.seq,
                        "summary": ev.summary,
                        "progression": prog,
                        "skills": proj.current.skills
                    }));
                } else if delta_xp < 0 {
                    let (res, _) = engine.adjust_skill_xp(
                        &cid,
                        &name,
                        Some(delta_xp),
                        None,
                        recalc_rank,
                        profile_opt.as_deref(),
                        &reason,
                    )?;
                    return Ok(res);
                }
            }
            let existing_skill = engine
                .get_character(&cid, None)
                .ok()
                .and_then(|p| {
                    p.current
                        .skills
                        .into_iter()
                        .find(|s| s.name.eq_ignore_ascii_case(&name))
                });
            let skill = Skill {
                name,
                rank: opt_str(&args, "rank")
                    .or_else(|| existing_skill.as_ref().map(|s| s.rank.clone()))
                    .unwrap_or_else(|| "Novice".to_string()),
                category: opt_str(&args, "category")
                    .or_else(|| existing_skill.as_ref().map(|s| s.category.clone()))
                    .unwrap_or_default(),
                modifier: opt_str(&args, "modifier")
                    .or_else(|| existing_skill.as_ref().and_then(|s| s.modifier.clone())),
                description: opt_str(&args, "description")
                    .or_else(|| existing_skill.as_ref().map(|s| s.description.clone()))
                    .unwrap_or_default(),
                governing_stat: opt_str(&args, "governing_stat")
                    .or_else(|| existing_skill.as_ref().and_then(|s| s.governing_stat.clone())),
                xp: args
                    .get("xp")
                    .and_then(|v| v.as_u64())
                    .or_else(|| existing_skill.as_ref().map(|s| s.xp))
                    .unwrap_or(0),
                proficiency_tier: opt_str(&args, "proficiency_tier")
                    .or_else(|| existing_skill.as_ref().and_then(|s| s.proficiency_tier.clone())),
            };
            let (proj, ev) = engine.upsert_skill(&cid, skill, &reason)?;
            Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "skills": proj.current.skills }))
        }

        "upsert_character_technique" => {
            let cid = req_str(&args, "character_id")?;
            let name = req_str(&args, "name")?;
            let reason = opt_str(&args, "reason").unwrap_or_default();
            let existing_tech = engine
                .get_character(&cid, None)
                .ok()
                .and_then(|p| {
                    p.current
                        .techniques
                        .into_iter()
                        .find(|t| t.name.eq_ignore_ascii_case(&name))
                });
            let source_text_id = match args.get("source_text_id") {
                Some(v) if v.is_null() => None,
                Some(v) => v
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
                None => existing_tech
                    .as_ref()
                    .and_then(|t| t.source_text_id.clone()),
            };
            let source_item_id = match args.get("source_item_id") {
                Some(v) if v.is_null() => None,
                Some(v) => v
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
                None => existing_tech
                    .as_ref()
                    .and_then(|t| t.source_item_id.clone()),
            };
            let progression_track_id = match args.get("progression_track_id") {
                Some(v) if v.is_null() => None,
                Some(v) => v
                    .as_str()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
                None => existing_tech
                    .as_ref()
                    .and_then(|t| t.progression_track_id.clone()),
            };
            let manual_title = if args.get("manual_title").is_some() {
                opt_str(&args, "manual_title").unwrap_or_default()
            } else if args.get("source_text_id").is_some() || args.get("source_item_id").is_some() {
                String::new()
            } else {
                existing_tech
                    .as_ref()
                    .map(|t| t.manual_title.clone())
                    .unwrap_or_default()
            };
            let tech = Technique {
                name,
                tier: opt_str(&args, "tier")
                    .or_else(|| existing_tech.as_ref().map(|t| t.tier.clone()))
                    .unwrap_or_default(),
                cost: opt_str(&args, "cost")
                    .or_else(|| existing_tech.as_ref().map(|t| t.cost.clone()))
                    .unwrap_or_default(),
                cooldown: opt_str(&args, "cooldown")
                    .or_else(|| existing_tech.as_ref().map(|t| t.cooldown.clone()))
                    .unwrap_or_default(),
                mastery: opt_str(&args, "mastery")
                    .or_else(|| existing_tech.as_ref().map(|t| t.mastery.clone()))
                    .unwrap_or_default(),
                effect: opt_str(&args, "effect")
                    .or_else(|| existing_tech.as_ref().map(|t| t.effect.clone()))
                    .unwrap_or_default(),
                description: opt_str(&args, "description")
                    .or_else(|| existing_tech.as_ref().map(|t| t.description.clone()))
                    .unwrap_or_default(),
                source_text_id,
                source_item_id,
                progression_track_id,
                manual_title,
                manual_notes: opt_str(&args, "manual_notes")
                    .or_else(|| existing_tech.as_ref().map(|t| t.manual_notes.clone()))
                    .unwrap_or_default(),
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
            let include_full = args
                .get("include_full_event_log")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let event_limit = args
                .get("event_log_limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(10);
            let mut proj = engine.get_place(&id, as_of)?;
            let total_event_count = proj.event_log.len();
            if !include_full && event_limit > 0 && proj.event_log.len() > event_limit {
                let start = proj.event_log.len() - event_limit;
                proj.event_log = proj.event_log.split_off(start);
            }
            let rels = engine.db().list_relations_for_entity(&proj.baseline.id)?;
            Ok(json!({
                "doc_path": engine.docs_dir().join("places").join(format!("{}.md", proj.baseline.id)).display().to_string(),
                "total_event_count": total_event_count,
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
            let allow_secrets = allow_gm_secrets(&args);
            if let Some(id) = opt_str(&args, "lore_id") {
                let entry = engine.get_lore(&id)?;
                if !allow_secrets && entry.visibility.eq_ignore_ascii_case("gm_secret") {
                    return Err(anyhow!(
                        "Lore entry '{}' is GM-secret and withheld under non-GM perspective.",
                        entry.id
                    ));
                }
                let mut val = serde_json::to_value(&entry)?;
                if allow_secrets && entry.visibility.eq_ignore_ascii_case("gm_secret") {
                    if let Some(obj) = val.as_object_mut() {
                        obj.insert(
                            "_gm_non_disclosure_seal".to_string(),
                            Value::String(GM_NON_DISCLOSURE_SEAL.to_string()),
                        );
                    }
                }
                return Ok(val);
            }
            let filter: LoreSearchFilter = serde_json::from_value(args)?;
            let mut entries = engine.search_lore_by(&filter)?;
            if !allow_secrets {
                entries.retain(|e| !e.visibility.eq_ignore_ascii_case("gm_secret"));
            }
            let has_secrets = entries
                .iter()
                .any(|e| e.visibility.eq_ignore_ascii_case("gm_secret"));
            let mut out = json!({ "count": entries.len(), "entries": entries });
            if allow_secrets && has_secrets {
                if let Some(obj) = out.as_object_mut() {
                    obj.insert(
                        "_gm_non_disclosure_seal".to_string(),
                        Value::String(GM_NON_DISCLOSURE_SEAL.to_string()),
                    );
                }
            }
            Ok(out)
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
            let allow_secrets = allow_gm_secrets(&args);
            let id = req_str(&args, "text_id")?;
            let event_limit = args
                .get("event_log_limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(10);
            let mut text = engine.get_text(&id)?;
            let had_gm_notes = !text.gm_notes.trim().is_empty();
            if !allow_secrets {
                redact_text_for_player(&mut text);
            }
            let mut events = engine.db().list_events_for_entity(&text.id)?;
            if !allow_secrets {
                events.retain(|e| !e.visibility.eq_ignore_ascii_case("gm_secret"));
            }
            let total_event_count = events.len();
            if event_limit > 0 && events.len() > event_limit {
                events = events.split_off(events.len() - event_limit);
            }
            let rels = engine.db().list_relations_for_entity(&text.id)?;
            let mut out = json!({
                "doc_path": engine.docs_dir().join("texts").join(format!("{}.md", text.id)).display().to_string(),
                "total_event_count": total_event_count,
                "text": text,
                "relations": rels,
                "event_log": events
            });
            if allow_secrets && had_gm_notes {
                if let Some(obj) = out.as_object_mut() {
                    obj.insert(
                        "_gm_non_disclosure_seal".to_string(),
                        Value::String(GM_NON_DISCLOSURE_SEAL.to_string()),
                    );
                }
            }
            Ok(out)
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
            let allow_secrets = allow_gm_secrets(&args);
            if let Some(id) = opt_str(&args, "text_id") {
                let mut text = engine.get_text(&id)?;
                let had_gm_notes = !text.gm_notes.trim().is_empty();
                if !allow_secrets {
                    redact_text_for_player(&mut text);
                }
                let mut val = serde_json::to_value(&text)?;
                if allow_secrets && had_gm_notes {
                    if let Some(obj) = val.as_object_mut() {
                        obj.insert(
                            "_gm_non_disclosure_seal".to_string(),
                            Value::String(GM_NON_DISCLOSURE_SEAL.to_string()),
                        );
                    }
                }
                return Ok(val);
            }
            let filter: TextSearchFilter = serde_json::from_value(args)?;
            let mut texts = engine.search_texts_by(&filter)?;
            let had_gm_notes = texts.iter().any(|t| !t.gm_notes.trim().is_empty());
            if !allow_secrets {
                for t in &mut texts {
                    redact_text_for_player(t);
                }
            }
            let mut out = json!({ "count": texts.len(), "texts": texts });
            if allow_secrets && had_gm_notes {
                if let Some(obj) = out.as_object_mut() {
                    obj.insert(
                        "_gm_non_disclosure_seal".to_string(),
                        Value::String(GM_NON_DISCLOSURE_SEAL.to_string()),
                    );
                }
            }
            Ok(out)
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
            let offset_chars = args
                .get("offset_chars")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let max_chars = args
                .get("max_chars")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(6000);
            let event_limit = args
                .get("event_log_limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(10);
            let att = engine.get_attachment(&id)?;
            let abs_path = engine.attachment_file_path(&att);
            let mut events = engine.db().list_events_for_entity(&att.id)?;
            let total_event_count = events.len();
            if event_limit > 0 && events.len() > event_limit {
                events = events.split_off(events.len() - event_limit);
            }
            let rels = engine.db().list_relations_for_entity(&att.id)?;
            let ext_is_text = abs_path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    matches!(
                        e.to_lowercase().as_str(),
                        "md" | "markdown"
                            | "txt"
                            | "json"
                            | "yaml"
                            | "yml"
                            | "toml"
                            | "csv"
                            | "tsv"
                            | "xml"
                            | "html"
                            | "htm"
                            | "rst"
                            | "log"
                            | "ini"
                            | "cfg"
                    )
                })
                .unwrap_or(false);
            let is_text_mime = att.mime_type.starts_with("text/")
                || att.mime_type == "application/json"
                || att.mime_type == "application/xml"
                || att.mime_type == "application/x-yaml"
                || ext_is_text;
            let mut text_total_chars: Option<usize> = None;
            let mut text_has_more = false;
            let mut text_next_offset: Option<usize> = None;
            let text_content = if include_text && is_text_mime {
                std::fs::read_to_string(&abs_path).ok().map(|full| {
                    let total = full.chars().count();
                    text_total_chars = Some(total);
                    if max_chars == 0 && offset_chars == 0 {
                        full
                    } else {
                        let safe_off = offset_chars.min(total);
                        let take_n = if max_chars == 0 { total } else { max_chars };
                        let slice: String = full.chars().skip(safe_off).take(take_n).collect();
                        let next = safe_off + slice.chars().count();
                        if next < total {
                            text_has_more = true;
                            text_next_offset = Some(next);
                        }
                        slice
                    }
                })
            } else {
                None
            };
            Ok(json!({
                "id": att.id,
                "reference_id": format!("attachment:{}", att.id),
                "mcp_uri": format!("chronicle://attachments/{}", att.id),
                "relative_path": att.relative_path,
                "absolute_path": abs_path.display().to_string(),
                "total_event_count": total_event_count,
                "attachment": att,
                "verbatim": true,
                "text_content": text_content,
                "text_offset_chars": offset_chars,
                "text_total_chars": text_total_chars,
                "text_has_more": text_has_more,
                "text_next_offset_chars": text_next_offset,
                "relations": rels,
                "event_log": events
            }))
        }

        "read_campaign_file" => {
            let target = if args.get("list").and_then(|v| v.as_bool()).unwrap_or(false) {
                "list".to_string()
            } else if let Some(t) = opt_str(&args, "target") {
                t
            } else if let Some(aid) = opt_str(&args, "attachment_id") {
                format!("attachment:{aid}")
            } else if let Some(tid) = opt_str(&args, "text_id") {
                format!("text:{tid}")
            } else if let Some(lid) = opt_str(&args, "lore_id") {
                format!("lore:{lid}")
            } else if let Some(dp) = opt_str(&args, "doc_path") {
                format!("doc:{dp}")
            } else {
                "list".to_string()
            };
            let offset_chars = args
                .get("offset_chars")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let chunk_chars = args
                .get("chunk_chars")
                .and_then(|v| v.as_u64())
                .unwrap_or(6000) as usize;
            let gm_override = args
                .get("gm_override")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            engine.read_campaign_file_chunk(&target, offset_chars, chunk_chars, gm_override)
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
            let mut garment_measurements = std::collections::BTreeMap::new();
            if let Some(obj) = args.get("garment_measurements").and_then(|v| v.as_object()) {
                for (k, v) in obj {
                    let nk = crate::autonomy::normalize_measurement_key(k);
                    let vs = v
                        .as_str()
                        .map(|s| s.trim().to_string())
                        .unwrap_or_else(|| v.to_string());
                    if !nk.is_empty() && !vs.is_empty() {
                        garment_measurements.insert(nk, vs);
                    }
                }
            }
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
                garment_measurements,
                garment_cut: opt_str(&args, "garment_cut").unwrap_or_default(),
                garment_taper: opt_str(&args, "garment_taper").unwrap_or_default(),
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
            let event_limit = args
                .get("event_log_limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(10);
            let item = engine.get_catalog_item(&id)?;
            let histories = engine.db().list_histories_for_entity(&item.id)?;
            let mut events = engine.db().list_events_for_entity(&item.id)?;
            let total_event_count = events.len();
            if event_limit > 0 && events.len() > event_limit {
                events = events.split_off(events.len() - event_limit);
            }
            let rels = engine.db().list_relations_for_entity(&item.id)?;
            Ok(json!({
                "doc_path": engine.docs_dir().join("items").join(format!("{}.md", item.id)).display().to_string(),
                "total_event_count": total_event_count,
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
            let all_items = engine.db().list_catalog_items()?;
            let tag_facets = crate::search::compute_catalog_tag_facets(&all_items);
            Ok(json!({
                "count": items.len(),
                "total_catalog_count": all_items.len(),
                "tag_facets": tag_facets,
                "items": items
            }))
        }

        "search_inventory" => {
            let filter: crate::search::InventorySearchFilter = serde_json::from_value(args)?;
            let res = engine.search_inventory(&filter)?;
            Ok(serde_json::to_value(res)?)
        }

        "tag_catalog_item" => {
            let item_id = req_str(&args, "item_id").or_else(|_| req_str(&args, "id"))?;
            let add_t = opt_str_vec(&args, "add_tags");
            let rem_t = opt_str_vec(&args, "remove_tags");
            let rep_t = args
                .get("set_tags")
                .or_else(|| args.get("replace_tags"))
                .or_else(|| {
                    if add_t.is_empty() && rem_t.is_empty() {
                        args.get("tags")
                    } else {
                        None
                    }
                })
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect::<Vec<_>>()
                });
            let reason = opt_str(&args, "reason")
                .or_else(|| opt_str(&args, "notes"))
                .unwrap_or_default();
            let (saved, ev) =
                engine.update_catalog_item_tags(&item_id, add_t, rem_t, rep_t, &reason)?;
            Ok(json!({
                "event_seq": ev.seq,
                "summary": ev.summary,
                "item": saved
            }))
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
            let allow_secrets = allow_gm_secrets(&args);
            let id = slugify(&req_str(&args, "lore_id")?);
            let lore = engine
                .db()
                .get_lore(&id)?
                .ok_or_else(|| anyhow!("Lore entry '{id}' not found"))?;
            if !allow_secrets && lore.visibility.eq_ignore_ascii_case("gm_secret") {
                return Err(anyhow!(
                    "Lore entry '{}' is GM-secret and withheld under non-GM perspective.",
                    lore.id
                ));
            }
            let mut histories = engine.db().list_histories_for_entity(&lore.id)?;
            let rels = engine.db().list_relations_for_entity(&lore.id)?;
            let mut events = engine.db().list_events_for_entity(&lore.id)?;
            if !allow_secrets {
                histories.retain(|h| !h.visibility.eq_ignore_ascii_case("gm_secret"));
                events.retain(|e| !e.visibility.eq_ignore_ascii_case("gm_secret"));
            }
            let mut out = json!({
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
            });
            if allow_secrets && lore.visibility.eq_ignore_ascii_case("gm_secret") {
                if let Some(obj) = out.as_object_mut() {
                    obj.insert(
                        "_gm_non_disclosure_seal".to_string(),
                        Value::String(GM_NON_DISCLOSURE_SEAL.to_string()),
                    );
                }
            }
            Ok(out)
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
            let allow_secrets = allow_gm_secrets(&args);
            let query = opt_str(&args, "query").unwrap_or_default();
            let limit = args
                .get("limit_per_kind")
                .and_then(|v| v.as_u64())
                .unwrap_or(10) as usize;
            let mut res = engine.search_omni(&query, limit)?;
            let had_secrets = res.lore.iter().any(|l| l.visibility.eq_ignore_ascii_case("gm_secret"))
                || res.histories.iter().any(|h| h.visibility.eq_ignore_ascii_case("gm_secret"))
                || res.attachments.iter().any(|a| a.visibility.eq_ignore_ascii_case("gm_secret"))
                || res.texts.iter().any(|t| !t.gm_notes.trim().is_empty())
                || res.items.iter().any(|i| !i.gm_notes.trim().is_empty());
            if !allow_secrets {
                res.lore.retain(|l| !l.visibility.eq_ignore_ascii_case("gm_secret"));
                res.histories.retain(|h| !h.visibility.eq_ignore_ascii_case("gm_secret"));
                res.attachments.retain(|a| !a.visibility.eq_ignore_ascii_case("gm_secret"));
                for t in &mut res.texts {
                    redact_text_for_player(t);
                }
                for it in &mut res.items {
                    it.gm_notes.clear();
                }
                res.events.retain(|e| !e.visibility.eq_ignore_ascii_case("gm_secret"));
            }
            let mut val = serde_json::to_value(res)?;
            if allow_secrets && had_secrets {
                if let Some(obj) = val.as_object_mut() {
                    obj.insert(
                        "_gm_non_disclosure_seal".to_string(),
                        Value::String(GM_NON_DISCLOSURE_SEAL.to_string()),
                    );
                }
            }
            Ok(val)
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
            let mut planned: Vec<(String, Value)> = Vec::new();
            if let Some(mins) = args.get("advance_minutes").and_then(|v| v.as_u64()) {
                if mins > 0 {
                    let mut ta = json!({ "minutes": mins });
                    if let Some(r) = opt_str(&args, "time_reason") {
                        ta["reason"] = Value::String(r);
                    }
                    planned.push(("advance_time".to_string(), ta));
                }
            }
            if let Some(actions) = args.get("actions").and_then(|v| v.as_array()) {
                for (idx, act) in actions.iter().enumerate() {
                    let tool_name = act
                        .get("tool")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow!("Action #{idx} missing 'tool' string"))?;
                    if tool_name == "commit_turn" {
                        return Err(anyhow!("Nested commit_turn is not allowed"));
                    }
                    let tool_args = act.get("arguments").cloned().unwrap_or_else(|| json!({}));
                    planned.push((tool_name.to_string(), tool_args));
                }
            }
            let trailing_log: Option<Value> = if let Some(log_obj) = args.get("log").filter(|v| v.is_object()) {
                Some(log_obj.clone())
            } else if let Some(title) = opt_str(&args, "narrative_title") {
                Some(json!({
                    "title": title,
                    "details": opt_str(&args, "narrative_details").unwrap_or_default(),
                    "location_id": opt_str(&args, "location_id"),
                    "affected_entities": opt_str_vec(&args, "affected_entities"),
                    "tags": opt_str_vec(&args, "tags"),
                    "visibility": opt_str(&args, "visibility").unwrap_or_else(|| "public".to_string())
                }))
            } else {
                None
            };
            if planned.is_empty() && trailing_log.is_none() {
                return Err(anyhow!(
                    "commit_turn requires at least one of: 'actions', 'advance_minutes', 'log', or 'narrative_title'"
                ));
            }
            let compact = args
                .get("compact")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            engine.with_deferred_compile(|engine| {
                let mut results = Vec::new();
                let mut event_seqs: Vec<i64> = Vec::new();
                let mut receipt_parts: Vec<String> = Vec::new();
                for (idx, (tool_name, tool_args)) in planned.into_iter().enumerate() {
                    let res = execute_tool_with_policy(
                        engine,
                        &tool_name,
                        tool_args,
                        allow_local_file_import,
                    )
                    .map_err(|e| anyhow!("Action #{idx} ('{tool_name}') failed: {e}"))?;
                    let seq_opt = res
                        .get("event_seq")
                        .or_else(|| res.get("event_id"))
                        .and_then(|v| v.as_i64());
                    if let Some(s) = seq_opt {
                        event_seqs.push(s);
                    }
                    let step_summary = summarize_turn_action_result(&tool_name, &res, seq_opt);
                    receipt_parts.push(step_summary.clone());
                    if compact {
                        let mut slim = json!({
                            "index": idx,
                            "tool": tool_name,
                            "summary": step_summary
                        });
                        if let Some(s) = seq_opt {
                            slim["event_seq"] = json!(s);
                        }
                        // Preserve key mechanical results (rolls/checks/contests/draws/warnings/stage_rules) in compact mode
                        for k in [
                            "outcome", "degree", "roll", "modifier", "total", "difficulty", "margin",
                            "winner_id", "warnings", "world_time", "rolls", "kept", "stage_rules_applied",
                            "stat_xp_gained", "job_xp_gained", "currency_conversion",
                        ] {
                            if let Some(v) = res.get(k) {
                                slim[k] = v.clone();
                            }
                        }
                        results.push(slim);
                    } else {
                        results.push(json!({ "index": idx, "tool": tool_name, "result": res }));
                    }
                }
                if let Some(mut log_args) = trailing_log {
                    if log_args.get("causal_event_ids").is_none() && !event_seqs.is_empty() {
                        log_args["causal_event_ids"] = json!(event_seqs);
                    }
                    let idx = results.len();
                    let res = execute_tool_with_policy(
                        engine,
                        "log_event",
                        log_args,
                        allow_local_file_import,
                    )
                    .map_err(|e| anyhow!("Trailing 'log_event' failed: {e}"))?;
                    let seq_opt = res.get("event_seq").and_then(|v| v.as_i64());
                    if let Some(s) = seq_opt {
                        event_seqs.push(s);
                    }
                    let step_summary = summarize_turn_action_result("log_event", &res, seq_opt);
                    receipt_parts.push(step_summary.clone());
                    if compact {
                        let mut slim = json!({
                            "index": idx,
                            "tool": "log_event",
                            "summary": step_summary
                        });
                        if let Some(s) = seq_opt {
                            slim["event_seq"] = json!(s);
                        }
                        results.push(slim);
                    } else {
                        results.push(json!({ "index": idx, "tool": "log_event", "result": res }));
                    }
                }
                let world_time = engine
                    .db()
                    .get_campaign_meta()
                    .map(|m| m.clock.format_short())
                    .unwrap_or_default();
                let signals = engine
                    .list_active_signals(Some("gm"), None, &[], true)
                    .unwrap_or_default();
                Ok(json!({
                    "committed_actions": results.len(),
                    "world_time": world_time,
                    "event_seqs": event_seqs,
                    "receipt": receipt_parts.join(" | "),
                    "results": results,
                    "signals": signals
                }))
            })
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
                        cadence_hours: args
                            .get("cadence_hours")
                            .and_then(|v| v.as_u64())
                            .filter(|&h| h > 0),
                        last_tick_total_hours: args
                            .get("last_tick_total_hours")
                            .and_then(|v| v.as_i64()),
                        consequence_on_fill: opt_str(&args, "consequence_on_fill")
                            .unwrap_or_default(),
                        auto_reset: args
                            .get("auto_reset")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
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
            let mut profile: RulesetProfile = if let Some(preset) = opt_str(&args, "preset") {
                let custom_id = opt_str(&args, "id");
                let mut p = RulesetProfile::from_preset(&preset, custom_id.as_deref())?;
                if let Some(n) = opt_str(&args, "name") {
                    p.name = n;
                }
                if let Some(d) = opt_str(&args, "description") {
                    p.description = d;
                }
                if let Some(m) = opt_str(&args, "stat_modifier_mode") {
                    p.stat_modifier_mode = m;
                }
                if let Some(axp) = args.get("auto_skill_xp").and_then(|v| v.as_bool()) {
                    p.auto_skill_xp = axp;
                }
                if let Some(extra_skills) = args
                    .get("skill_table")
                    .and_then(|v| serde_json::from_value::<std::collections::BTreeMap<String, crate::mechanics::SkillDefinition>>(v.clone()).ok())
                {
                    for (k, v) in extra_skills {
                        p.skill_table.insert(slugify(&k), v);
                    }
                }
                if let Some(extra_dc) = args
                    .get("dc_rubric")
                    .and_then(|v| serde_json::from_value::<std::collections::BTreeMap<String, i64>>(v.clone()).ok())
                {
                    for (k, v) in extra_dc {
                        p.dc_rubric.insert(k.to_lowercase(), v);
                    }
                }
                if let Some(extra_ranks) = args
                    .get("rank_bonuses")
                    .and_then(|v| serde_json::from_value::<std::collections::BTreeMap<String, i64>>(v.clone()).ok())
                {
                    for (k, v) in extra_ranks {
                        p.rank_bonuses.insert(k.to_lowercase(), v);
                    }
                }
                p
            } else {
                serde_json::from_value(args)?
            };
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
            let dc_tier = opt_str(&args, "dc_tier");
            let explicit_dc = args.get("difficulty").and_then(|v| v.as_i64());
            if explicit_dc.is_none() && dc_tier.is_none() {
                return Err(anyhow!(
                    "Provide either integer 'difficulty' OR grounded 'dc_tier' (trivial, easy, moderate, hard, very_hard, nearly_impossible)."
                ));
            }
            let dc_factors: Vec<DcFactorInput> = args
                .get("dc_factors")
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()
                .map_err(|e| anyhow!("Invalid 'dc_factors' array: {e}"))?
                .unwrap_or_default();
            let req = CheckRequest {
                actor_id: opt_str(&args, "actor_id"),
                action: req_str(&args, "action")?,
                profile: opt_str(&args, "profile"),
                stat: opt_str(&args, "stat"),
                skill: opt_str(&args, "skill"),
                technique: opt_str(&args, "technique"),
                progression_track: opt_str(&args, "progression_track"),
                equipment: opt_str_vec(&args, "equipment"),
                difficulty: explicit_dc.unwrap_or(0),
                dc_tier,
                dc_factors,
                modifier: args.get("modifier").and_then(|v| v.as_i64()).unwrap_or(0),
                situational_modifiers: parse_modifier_sources(args.get("situational_modifiers")),
                advantage: opt_str(&args, "advantage").unwrap_or_else(|| "none".to_string()),
                dice_override: opt_str(&args, "dice_override"),
                auto_hydrate: args.get("auto_hydrate").and_then(|v| v.as_bool()),
                award_skill_xp: args.get("award_skill_xp").and_then(|v| v.as_bool()),
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
            let mode = opt_str(&args, "mode")
                .or_else(|| opt_str(&args, "sub_action"))
                .or_else(|| opt_str(&args, "action"))
                .unwrap_or_else(|| "prepare".to_string())
                .to_lowercase();
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
                "configure_mobility" => {
                    let cid = req_str(&args, "character_id")?;
                    engine.configure_npc_mobility(&cid, &args)
                }
                "dispatch_trip" => {
                    let cid = req_str(&args, "character_id")?;
                    let dest = opt_str(&args, "destination_place_id").unwrap_or_default();
                    let purpose = opt_str(&args, "purpose").unwrap_or_else(|| "business".to_string());
                    let details = opt_str(&args, "details").unwrap_or_default();
                    let stay_days = args
                        .get("stay_duration_days")
                        .and_then(|v| v.as_u64())
                        .map(|d| d as u32);
                    let speed = opt_str(&args, "travel_speed_mode");
                    let target_gp = args
                        .get("target_grid_pos")
                        .and_then(|v| serde_json::from_value::<crate::autonomy::GridPosition>(v.clone()).ok());
                    let uncharted = opt_str(&args, "uncharted_label");
                    let vis_override = opt_str(&args, "visibility");
                    engine.dispatch_npc_trip_ext(
                        &cid,
                        &dest,
                        &purpose,
                        &details,
                        stay_days,
                        speed.as_deref(),
                        target_gp,
                        uncharted.as_deref(),
                        vis_override.as_deref(),
                    )
                }
                "tick_movements" => {
                    let aids = opt_str_vec(&args, "actor_ids");
                    let auto_prom = args
                        .get("auto_include_prominent")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    let advance_clock = args
                        .get("advance_world_clock")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if advance_clock {
                        let _ = engine.advance_time_no_npc_tick(
                            duration_hours * 60,
                            None,
                            Some(format!("Living world NPC movement tick ({duration_hours}h)")),
                        )?;
                    }
                    let seed = args
                        .get("seed")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(duration_hours.wrapping_mul(0x9E37_79B9));
                    engine.tick_npc_movements(duration_hours, &aids, auto_prom, seed)
                }
                "list_movements" => {
                    let include_secrets = args
                        .get("include_gm_secrets")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    engine.list_npc_movements_filtered(include_secrets)
                }
                other => Err(anyhow!(
                    "Unknown simulate_autonomy mode '{other}'. Expected 'prepare', 'commit', 'auto_mechanical', 'configure_mobility', 'dispatch_trip', 'tick_movements', or 'list_movements'."
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
                "calculate" | "calculate_route" => {
                    let from = req_str(&args, "from_place_id")?;
                    let to = req_str(&args, "to_place_id")?;
                    let chars = opt_str_vec(&args, "character_ids");
                    let mode = opt_str(&args, "transport_mode").unwrap_or_else(|| "foot".to_string());
                    let forced = args.get("forced_march").and_then(|v| v.as_bool()).unwrap_or(false);
                    let plan = engine.calculate_travel(&from, &to, &chars, &mode, forced)?;
                    Ok(serde_json::to_value(plan)?)
                }
                "advance" | "advance_travel" => {
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
                "explore" | "explore_area" => {
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
                "upsert_grid" => {
                    let grid = crate::world_sim::MapGrid {
                        id: opt_str(&args, "grid_id")
                            .or_else(|| opt_str(&args, "id"))
                            .unwrap_or_default(),
                        name: req_str(&args, "name")?,
                        scale_kind: opt_str(&args, "scale_kind")
                            .unwrap_or_else(|| "region".to_string()),
                        parent_place_id: opt_str(&args, "parent_place_id"),
                        width: args.get("width").and_then(|v| v.as_u64()).unwrap_or(16) as u32,
                        height: args.get("height").and_then(|v| v.as_u64()).unwrap_or(12) as u32,
                        cell_scale_label: opt_str(&args, "cell_scale_label")
                            .unwrap_or_else(|| "5 km".to_string()),
                        default_terrain: opt_str(&args, "default_terrain")
                            .unwrap_or_else(|| "wilderness".to_string()),
                        description: opt_str(&args, "description").unwrap_or_default(),
                        updated_at_world_time: String::new(),
                    };
                    let (saved, ev) = engine.upsert_map_grid(grid)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "grid": saved }))
                }
                "list_grids" => engine.list_map_grids_overview(),
                "get_grid" => {
                    let gid = req_str(&args, "grid_id").or_else(|_| req_str(&args, "id"))?;
                    let cx = args.get("center_x").and_then(|v| v.as_i64()).map(|n| n as i32);
                    let cy = args.get("center_y").and_then(|v| v.as_i64()).map(|n| n as i32);
                    let center = match (cx, cy) {
                        (Some(x), Some(y)) => Some((x, y)),
                        _ => None,
                    };
                    let radius = args.get("radius").and_then(|v| v.as_i64()).map(|n| n as i32);
                    let include_gm = args
                        .get("include_gm_secrets")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    engine.get_map_grid_snapshot(&gid, center, radius, include_gm)
                }
                "set_cell" => {
                    let gid = req_str(&args, "grid_id").or_else(|_| req_str(&args, "id"))?;
                    let reason = opt_str(&args, "reason").unwrap_or_default();
                    let cells_vec: Vec<crate::world_sim::MapGridCell> =
                        if let Some(arr) = args.get("cells").and_then(|v| v.as_array()) {
                            let mut out = Vec::new();
                            for item in arr {
                                let x = item.get("x").and_then(|v| v.as_i64()).ok_or_else(|| {
                                    anyhow!("Each cell in 'cells' requires integer 'x'")
                                })? as i32;
                                let y = item.get("y").and_then(|v| v.as_i64()).ok_or_else(|| {
                                    anyhow!("Each cell in 'cells' requires integer 'y'")
                                })? as i32;
                                out.push(parse_map_cell(&gid, x, y, item));
                            }
                            out
                        } else {
                            let x = args
                                .get("x")
                                .and_then(|v| v.as_i64())
                                .ok_or_else(|| anyhow!("set_cell requires integer 'x' (or 'cells' array)"))?
                                as i32;
                            let y = args
                                .get("y")
                                .and_then(|v| v.as_i64())
                                .ok_or_else(|| anyhow!("set_cell requires integer 'y' (or 'cells' array)"))?
                                as i32;
                            vec![parse_map_cell(&gid, x, y, &args)]
                        };
                    engine.upsert_map_cells(&gid, cells_vec, &reason)
                }
                "clear_cell" => {
                    let gid = req_str(&args, "grid_id").or_else(|_| req_str(&args, "id"))?;
                    let x = args
                        .get("x")
                        .and_then(|v| v.as_i64())
                        .ok_or_else(|| anyhow!("clear_cell requires integer 'x'"))?
                        as i32;
                    let y = args
                        .get("y")
                        .and_then(|v| v.as_i64())
                        .ok_or_else(|| anyhow!("clear_cell requires integer 'y'"))?
                        as i32;
                    engine.clear_map_cell(&gid, x, y)
                }
                "auto_layout_grid" => {
                    let gid = req_str(&args, "grid_id").or_else(|_| req_str(&args, "id"))?;
                    engine.auto_layout_places_on_grid(&gid)
                }
                other => Err(anyhow!(
                    "Unknown manage_travel_and_exploration action '{other}'. Expected: upsert_route, list_routes, update_spatial, calculate, advance, explore, upsert_grid, list_grids, get_grid, set_cell, clear_cell, auto_layout_grid."
                )),
            }
        }

        "manage_economy" => {
            let action = req_str(&args, "action")?.to_lowercase();
            match action.as_str() {
                "set_item_weight" | "set_item_economics" => {
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
                    let reason = opt_str(&args, "reason")
                        .or_else(|| opt_str(&args, "notes"))
                        .unwrap_or_default();
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
                            progression_track: opt_str(&args, "progression_track"),
                            equipment: vec![],
                            difficulty: dc,
                            dc_tier: opt_str(&args, "dc_tier"),
                            dc_factors: vec![],
                            modifier: args.get("modifier").and_then(|v| v.as_i64()).unwrap_or(0),
                            situational_modifiers: vec![],
                            advantage: "none".to_string(),
                            dice_override: None,
                            auto_hydrate: Some(true),
                            award_skill_xp: None,
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
                        progression_track: opt_str(&args, "progression_track"),
                        equipment: opt_str_vec(&args, "equipment"),
                        difficulty: dc,
                        dc_tier: opt_str(&args, "dc_tier"),
                        dc_factors: vec![],
                        modifier: args.get("modifier").and_then(|v| v.as_i64()).unwrap_or(0),
                        situational_modifiers: parse_modifier_sources(args.get("situational_modifiers")),
                        advantage: opt_str(&args, "advantage").unwrap_or_else(|| "none".to_string()),
                        dice_override: opt_str(&args, "dice_override"),
                        auto_hydrate: Some(true),
                        award_skill_xp: None,
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

        "manage_progression" => {
            let action = req_str(&args, "action")?.to_lowercase();
            match action.as_str() {
                "define_ladder" => {
                    let mut ladder = if let Some(preset) = opt_str(&args, "preset") {
                        let mut p = ProgressionLadder::from_preset(
                            &preset,
                            opt_str(&args, "ladder_id").or_else(|| opt_str(&args, "id")).as_deref(),
                        )?;
                        if let Some(n) = opt_str(&args, "name") {
                            p.name = n;
                        }
                        if let Some(d) = opt_str(&args, "description") {
                            p.description = d;
                        }
                        if let Some(k) = opt_str(&args, "kind") {
                            p.kind = k;
                        }
                        if let Some(st) = opt_str(&args, "source_text_id") {
                            p.source_text_id = Some(st);
                        }
                        if let Some(si) = opt_str(&args, "source_item_id") {
                            p.source_item_id = Some(si);
                        }
                        if let Some(mt) = opt_str(&args, "manual_title") {
                            p.manual_title = mt;
                        }
                        if let Some(mn) = opt_str(&args, "manual_notes") {
                            p.manual_notes = mn;
                        }
                        let extra_tags = opt_str_vec(&args, "default_check_tags");
                        if !extra_tags.is_empty() {
                            p.default_check_tags = extra_tags;
                        }
                        p
                    } else {
                        let stages: Vec<ProgressionStageDef> = args
                            .get("stages")
                            .map(|v| serde_json::from_value(v.clone()))
                            .transpose()
                            .map_err(|e| anyhow!("Invalid 'stages' array: {e}"))?
                            .ok_or_else(|| {
                                anyhow!(
                                    "define_ladder requires either 'preset' (xianxia_standard, body_tempering, dnd_level_20, scifi_tier) or a 'stages' array"
                                )
                            })?;
                        ProgressionLadder {
                            id: opt_str(&args, "ladder_id")
                                .or_else(|| opt_str(&args, "id"))
                                .unwrap_or_default(),
                            name: req_str(&args, "name")?,
                            kind: opt_str(&args, "kind")
                                .unwrap_or_else(|| "realm_ladder".to_string()),
                            description: opt_str(&args, "description").unwrap_or_default(),
                            source_text_id: opt_str(&args, "source_text_id"),
                            source_item_id: opt_str(&args, "source_item_id"),
                            manual_title: opt_str(&args, "manual_title").unwrap_or_default(),
                            manual_notes: opt_str(&args, "manual_notes").unwrap_or_default(),
                            default_check_tags: opt_str_vec(&args, "default_check_tags"),
                            stages,
                            updated_at_world_time: String::new(),
                        }
                    };
                    if ladder.id.trim().is_empty() {
                        ladder.id = slugify(&ladder.name);
                    }
                    let (saved, ev) = engine.define_progression_ladder(ladder)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "ladder": saved }))
                }
                "list_ladders" => {
                    let ladders = engine.list_progression_ladders()?;
                    Ok(json!({ "count": ladders.len(), "ladders": ladders }))
                }
                "enroll_track" => {
                    let cid = req_str(&args, "character_id")?;
                    let reason = opt_str(&args, "reason").unwrap_or_default();
                    let ladder_id = opt_str(&args, "ladder_id");
                    let name = opt_str(&args, "name")
                        .or_else(|| opt_str(&args, "track_id"))
                        .or_else(|| ladder_id.clone())
                        .ok_or_else(|| anyhow!("Missing 'name', 'track_id', or 'ladder_id'"))?;
                    let sub_max = args
                        .get("sub_stage_max")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(9) as u32;
                    let sub = args
                        .get("sub_stage")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(1) as u32;
                    let track = CharacterProgressionTrack {
                        character_id: cid,
                        track_id: opt_str(&args, "track_id").unwrap_or_else(|| slugify(&name)),
                        name,
                        track_kind: opt_str(&args, "track_kind")
                            .unwrap_or_else(|| "primary_realm".to_string()),
                        ladder_id,
                        source_text_id: opt_str(&args, "source_text_id"),
                        source_item_id: opt_str(&args, "source_item_id"),
                        manual_title: opt_str(&args, "manual_title").unwrap_or_default(),
                        manual_notes: opt_str(&args, "manual_notes").unwrap_or_default(),
                        major_stage_index: args
                            .get("major_stage_index")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0) as u32,
                        major_stage_name: opt_str(&args, "major_stage_name").unwrap_or_default(),
                        sub_stage: sub,
                        sub_stage_max: sub_max,
                        sub_stage_label: opt_str(&args, "sub_stage_label").unwrap_or_default(),
                        current_xp: args
                            .get("current_xp")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        xp_to_next: args
                            .get("xp_to_next")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(100),
                        cumulative_xp: args
                            .get("cumulative_xp")
                            .or_else(|| args.get("current_xp"))
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0),
                        at_bottleneck: args
                            .get("at_bottleneck")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        bottleneck_reason: opt_str(&args, "bottleneck_reason").unwrap_or_default(),
                        halt_conditions: opt_str_vec(&args, "halt_conditions"),
                        active_properties: opt_str_vec(&args, "active_properties"),
                        check_modifier: args
                            .get("check_modifier")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0),
                        check_tags: opt_str_vec(&args, "check_tags"),
                        status: opt_str(&args, "status").unwrap_or_else(|| "active".to_string()),
                        notes: opt_str(&args, "notes").unwrap_or_default(),
                        gm_notes: opt_str(&args, "gm_notes").unwrap_or_default(),
                        stage_rules: args
                            .get("stage_rules")
                            .and_then(|v| serde_json::from_value(v.clone()).ok())
                            .unwrap_or_default(),
                        updated_at_world_time: String::new(),
                    };
                    let (saved, ev) = engine.enroll_progression_track(track, &reason)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "track": saved }))
                }
                "advance_xp" => {
                    let cid = req_str(&args, "character_id")?;
                    let tid = req_str(&args, "track_id")?;
                    let xp = args
                        .get("xp")
                        .or_else(|| args.get("add_xp"))
                        .or_else(|| args.get("delta_xp"))
                        .and_then(|v| v.as_i64())
                        .ok_or_else(|| anyhow!("Missing integer 'xp' (or 'add_xp' / 'delta_xp')"))?;
                    let reason = opt_str(&args, "reason").unwrap_or_default();
                    let allow_demote = args
                        .get("allow_demote")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    if xp >= 0 {
                        let (res, _) =
                            engine.advance_progression_xp(&cid, &tid, xp as u64, &reason)?;
                        Ok(res)
                    } else {
                        let (res, _) = engine.adjust_progression_xp(
                            &cid,
                            &tid,
                            Some(xp),
                            None,
                            None,
                            false,
                            false,
                            allow_demote,
                            &reason,
                        )?;
                        Ok(res)
                    }
                }
                "adjust_xp" | "revert_xp" | "reset_xp" => {
                    let cid = req_str(&args, "character_id")?;
                    let tid = req_str(&args, "track_id")?;
                    let reason = opt_str(&args, "reason").unwrap_or_default();
                    let delta_xp = args
                        .get("delta_xp")
                        .or_else(|| args.get("xp"))
                        .or_else(|| args.get("add_xp"))
                        .and_then(|v| v.as_i64())
                        .map(|d| {
                            if action.eq_ignore_ascii_case("revert_xp") && d > 0 {
                                -d
                            } else {
                                d
                            }
                        });
                    let set_current_xp = args
                        .get("set_current_xp")
                        .or_else(|| args.get("current_xp"))
                        .and_then(|v| v.as_u64());
                    let set_cumulative_xp = args
                        .get("set_cumulative_xp")
                        .or_else(|| args.get("cumulative_xp"))
                        .and_then(|v| v.as_u64());
                    let reset_track = args
                        .get("reset_track")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let mut reset_stage_xp = args
                        .get("reset_stage_xp")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if action.eq_ignore_ascii_case("reset_xp")
                        && !reset_track
                        && !reset_stage_xp
                        && set_current_xp.is_none()
                        && set_cumulative_xp.is_none()
                        && delta_xp.is_none()
                    {
                        reset_stage_xp = true;
                    }
                    let allow_demote = args
                        .get("allow_demote")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    let (res, _) = engine.adjust_progression_xp(
                        &cid,
                        &tid,
                        delta_xp,
                        set_current_xp,
                        set_cumulative_xp,
                        reset_stage_xp,
                        reset_track,
                        allow_demote,
                        &reason,
                    )?;
                    Ok(res)
                }
                "remove_track" => {
                    let cid = req_str(&args, "character_id")?;
                    let tid = req_str(&args, "track_id")?;
                    let reason = opt_str(&args, "reason").unwrap_or_default();
                    let ev = engine.remove_progression_track(&cid, &tid, &reason)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "removed": true }))
                }
                "attempt_breakthrough" => {
                    let cid = req_str(&args, "character_id")?;
                    let tid = req_str(&args, "track_id")?;
                    let dc_factors: Vec<DcFactorInput> = args
                        .get("dc_factors")
                        .map(|v| serde_json::from_value(v.clone()))
                        .transpose()
                        .map_err(|e| anyhow!("Invalid 'dc_factors': {e}"))?
                        .unwrap_or_default();
                    let (res, _) = engine.attempt_progression_breakthrough(
                        &cid,
                        &tid,
                        opt_str(&args, "profile").as_deref(),
                        opt_str(&args, "stat"),
                        opt_str(&args, "skill"),
                        args.get("difficulty").and_then(|v| v.as_i64()),
                        opt_str(&args, "dc_tier"),
                        dc_factors,
                        args.get("modifier").and_then(|v| v.as_i64()).unwrap_or(0),
                        opt_str(&args, "advantage").unwrap_or_else(|| "none".to_string()),
                        opt_str(&args, "stakes").unwrap_or_default(),
                        args.get("seed").and_then(|v| v.as_u64()),
                        opt_str(&args, "next_stage_name"),
                    )?;
                    Ok(res)
                }
                "update_track" => {
                    let cid = req_str(&args, "character_id")?;
                    let tid = req_str(&args, "track_id")?;
                    let reason = opt_str(&args, "reason").unwrap_or_default();
                    let (saved, ev) =
                        engine.update_progression_track(&cid, &tid, &args, &reason)?;
                    Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "track": saved }))
                }
                "get_character_progression" => {
                    let allow_secrets = allow_gm_secrets(&args);
                    let cid = req_str(&args, "character_id")?;
                    let mut tracks = engine.list_character_progressions(&cid)?;
                    let mut has_future_rules = false;
                    for tr in &mut tracks {
                        let cur_m = tr.major_stage_index;
                        let cur_s = tr.sub_stage;
                        if tr
                            .stage_rules
                            .keys()
                            .any(|k| stage_rule_key_is_future(k, cur_m, cur_s))
                        {
                            has_future_rules = true;
                        }
                        if !allow_secrets {
                            tr.stage_rules
                                .retain(|k, _| !stage_rule_key_is_future(k, cur_m, cur_s));
                        }
                    }
                    let mut out = json!({ "character_id": cid, "count": tracks.len(), "tracks": tracks });
                    if allow_secrets && has_future_rules {
                        if let Some(obj) = out.as_object_mut() {
                            obj.insert(
                                "_gm_non_disclosure_seal".to_string(),
                                Value::String(GM_NON_DISCLOSURE_SEAL.to_string()),
                            );
                        }
                    }
                    Ok(out)
                }
                other => Err(anyhow!(
                    "Unknown manage_progression action '{other}'. Expected: define_ladder, list_ladders, enroll_track, advance_xp, adjust_xp, revert_xp, reset_xp, attempt_breakthrough, update_track, remove_track, get_character_progression."
                )),
            }
        }

        "manage_player_character" => {
            let action = req_str(&args, "action")?;
            let cid = opt_str(&args, "character_id");
            engine.manage_player_character_action(cid.as_deref(), &action, &args)
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
            let tick_npc_movements = args
                .get("tick_npc_movements")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
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
                tick_npc_movements,
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

        // =====================================================================
        // Signal Bus, Standing Routines & Sanitized Unified Domain Facades
        // =====================================================================
        "manage_signals" => execute_manage_signals(engine, args),
        "manage_routines" => execute_manage_routines(engine, args),
        "scene" => execute_domain_scene(engine, args, allow_local_file_import),
        "query" => execute_domain_query(engine, args, allow_local_file_import),
        "resolve" => execute_domain_resolve(engine, args, allow_local_file_import),
        "character" => execute_domain_character(engine, args, allow_local_file_import),
        "progression" => execute_domain_progression(engine, args, allow_local_file_import),
        "world" => execute_domain_world(engine, args, allow_local_file_import),
        "society" => execute_domain_society(engine, args, allow_local_file_import),
        "codex" => execute_domain_codex(engine, args, allow_local_file_import),
        "economy" => execute_domain_economy(engine, args, allow_local_file_import),
        "admin" => execute_domain_admin(engine, args, allow_local_file_import),

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
    tool_definitions_for_profile(active_tool_profile())
}

pub fn tool_definitions_for_profile(profile: ToolProfile) -> Value {
    let mut list = match profile {
        ToolProfile::Play => unified_domain_tool_definitions(false),
        ToolProfile::CompactFull => unified_domain_tool_definitions(true),
        ToolProfile::Legacy => {
            let mut full = legacy_base_tool_definitions();
            full.extend(extra_rpg_ai_tools());
            full.extend(unified_domain_only_definitions());
            full
        }
    };
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

fn legacy_base_tool_definitions() -> Vec<Value> {
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
                    "remove": { "type": "boolean", "default": false, "description": "If true, removes an obsolete resource pool from the character sheet." },
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
            "description": "Add or remove an active status effect, buff, debuff, injury, or curse on a character. Put a signed modifier (e.g. `-2`) and optional `[scope: strength, brawling, sprint, initiative]` in `notes` to make `resolve_check` auto-apply the modifier ONLY to matching checks!",
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
            "description": "Acquire, tailor/update, remove/consume, equip, or unequip an item in a character's gear & inventory. WEARABLE SIZING & TAPER RULE: Whenever creating or acquiring clothes/armor, specify `garment_cut`, `garment_taper`, and dimensional `garment_measurements` (`Shoulders`, `Chest`, `Underbust`, `Waist`, `Hips`, `Thigh`, `Inseam`) based on the garment's intended wearer/cut (e.g. boxy workwear: Shoulders +2 cm, Chest +5 cm, Waist +7 cm, Hips +2 cm; tailored waist/hourglass: Shoulders +1 cm, Chest +4 cm, Waist +3 cm, Hips +3 cm; relaxed robe: +8–12 cm). Equipping a garment smaller than the character's current body beyond tolerance is blocked; baggy, tight, or odd-tapered equipped garments auto-apply DEX/STR/CHA penalties and persistent Apparel Discomfort.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": { "type": "string" },
                    "action": { "type": "string", "enum": ["acquire", "tailor", "update", "remove", "equip", "unequip"] },
                    "name": { "type": "string" },
                    "quantity": { "type": "integer", "default": 1 },
                    "slot": { "type": "string" },
                    "equipped": { "type": "boolean" },
                    "durability": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "description": { "type": "string" },
                    "garment_measurements": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Optional garment dimensions (e.g. {'Shoulders':'50 cm','Chest':'114 cm','Waist':'96 cm','Hips':'106 cm'}). Keys auto-canonicalize; compared live against character measurements." },
                    "garment_cut": { "type": "string", "description": "Optional cut/stretch profile: 'fitted', 'tailored', 'standard', 'relaxed', 'loose', 'stretch', or 'rigid_armor'." },
                    "garment_taper": { "type": "string", "description": "Optional silhouette/taper cut: 'straight', 'boxy', 'athletic_v', 'tailored_waist', 'hourglass', 'a_line', or 'relaxed_drape'." },
                    "reason": { "type": "string" }
                },
                "required": ["character_id", "action", "name"]
            }
        },
        {
            "name": "upsert_character_skill",
            "description": "Learn a new skill, update its rank/governing_stat/modifier/xp, award or revert activity XP via signed `add_xp` (negative reverts XP & demotes rank cleanly), set exact XP via `set_xp`, or reset skill XP to 0 via `reset_xp: true`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "character_id": { "type": "string" },
                    "name": { "type": "string" },
                    "rank": { "type": "string" },
                    "governing_stat": { "type": "string", "description": "Optional governing ability/stat (e.g. 'Dexterity', 'Wisdom')." },
                    "proficiency_tier": { "type": "string", "description": "Optional proficiency tier (e.g. 'untrained', 'proficient', 'expertise')." },
                    "xp": { "type": "integer", "description": "Set raw skill XP without auto-recalculating rank." },
                    "set_xp": { "type": "integer", "description": "Set exact skill XP and automatically recalculate rank & modifier from the active XP curve (unless recalc_rank=false)." },
                    "add_xp": { "type": "integer", "description": "Signed activity XP delta: positive awards XP and promotes; negative reverts/deducts XP and recalculates rank downwards." },
                    "reset_xp": { "type": "boolean", "description": "Reset skill XP to 0 and restore baseline rank/modifier from the active XP curve." },
                    "recalc_rank": { "type": "boolean", "default": true, "description": "When reverting/setting/resetting XP, auto-recalculate rank & modifier from the active XP curve." },
                    "profile": { "type": "string", "description": "Optional ruleset profile ID for XP curve lookup." },
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
            "description": "Learn a new technique/ability/spell/cultivation art or update its mastery, tier, cost, cooldown, effect, linked cultivation manual (source_text_id / source_item_id), linked progression track, or manual cultivation notes.",
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
                    "source_text_id": { "type": "string", "description": "Linked StoryText ID or title of the cultivation manual/scroll teaching this technique." },
                    "source_item_id": { "type": "string", "description": "Optional linked CatalogItem ID of the physical manual/relic." },
                    "progression_track_id": { "type": "string", "description": "Optional linked CharacterProgressionTrack ID (e.g. parallel cultivation manual track)." },
                    "manual_title": { "type": "string", "description": "Optional manual display title (auto-resolved from source_text_id if omitted)." },
                    "manual_notes": { "type": "string", "description": "Specific notes on cultivating or practicing this manual technique (breathing rhythm, meridian quirks, incompatibilities, margin notes)." },
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
    match base {
        Value::Array(v) => v,
        _ => Vec::new(),
    }
}

fn tool_annotations(name: &str) -> Value {
    let read_only = matches!(
        name,
        "query"
            | "campaign_status"
            | "get_scene_context"
            | "get_character"
            | "get_place"
            | "get_text"
            | "get_item"
            | "get_lore"
            | "get_attachment"
            | "read_campaign_file"
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
            | "search_knowledge"
            | "search_all"
            | "diagnose_campaign"
    );
    let destructive = matches!(
        name,
        "admin" | "rollback_to_event" | "remove_relation" | "delete_entity" | "restore_backup"
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
            "description": "Primary one-call turn/beat checkpointer: combine optional elapsed time (`advance_minutes`), state/roll `actions`, and a trailing narrative `log` (`{ title, details, location_id, affected_entities, tags }`) into a single call with auto-linked causality and a concise verified receipt (`world_time`, `event_seqs`, `receipt`). Use after any meaningful decision, deal, discovery, travel, or roll consequence.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "advance_minutes": { "type": "integer", "description": "Optional shorthand to advance world time before running actions/log." },
                    "time_reason": { "type": "string", "description": "Optional reason for `advance_minutes`." },
                    "actions": {
                        "type": "array",
                        "description": "Optional ordered Chronicle tool operations (e.g. resolve_check, move_character, modify_character_resource, update_character_equipment, manage_economy, upsert_relation).",
                        "items": {
                            "type": "object",
                            "properties": {
                                "tool": { "type": "string" },
                                "arguments": { "type": "object" }
                            },
                            "required": ["tool", "arguments"]
                        }
                    },
                    "log": {
                        "type": "object",
                        "description": "Optional shorthand for a trailing `log_event` summarizing this beat's narrative outcome (auto-links causal_event_ids from earlier steps in this commit).",
                        "properties": {
                            "title": { "type": "string" },
                            "details": { "type": "string" },
                            "location_id": { "type": "string" },
                            "affected_entities": { "type": "array", "items": { "type": "string" } },
                            "tags": { "type": "array", "items": { "type": "string" } },
                            "visibility": { "type": "string", "enum": ["public", "witnessed_only", "gm_secret"], "default": "public" }
                        },
                        "required": ["title"]
                    },
                    "narrative_title": { "type": "string", "description": "Flat shorthand alternative to `log.title`." },
                    "narrative_details": { "type": "string", "description": "Flat shorthand alternative to `log.details`." },
                    "compact": {
                        "type": "boolean",
                        "default": true,
                        "description": "Defaults to true: returns concise per-step summaries and key roll/check results instead of bulky nested projections."
                    }
                }
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
            "description": "Catalog or update a carryable RPG item (clothing, apparel, armor, weapon, tool, artifact, keepsake, book, manual, scroll, record, key, relic) with origin/maker, previous owners, current holder (character or place), significance, mechanics, optional wearable sizing/taper (`garment_measurements`, `garment_cut`, `garment_taper`), and optional linked_text_id. Automatically syncs into the holder character's inventory (preserving garment sizing/taper) and compiles docs/items/<id>.md.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string" },
                    "name": { "type": "string" },
                    "item_type": { "type": "string", "description": "clothing, apparel, armor, weapon, tool, artifact, keepsake, book, manual, scroll, record, relic, key, consumable, gear" },
                    "rarity_or_tier": { "type": "string" },
                    "slot": { "type": "string" },
                    "quantity": { "type": "integer", "default": 1 },
                    "equipped": { "type": "boolean", "default": false },
                    "condition_or_durability": { "type": "string", "default": "Intact" },
                    "origin_and_maker": { "type": "string", "description": "Who forged, tailored, crafted, or authored this item and where." },
                    "previous_owners": { "type": "array", "items": { "type": "string" }, "description": "Chronological list of previous owners." },
                    "current_holder_id": { "type": "string", "description": "Character slug carrying it OR place slug where it rests." },
                    "significance": { "type": "string", "description": "Why this item matters to the story, bloodlines, or sects." },
                    "properties_and_effects": { "type": "string" },
                    "description": { "type": "string" },
                    "garment_measurements": { "type": "object", "additionalProperties": { "type": "string" }, "description": "For clothing/armor: dimensional garment size map ({Shoulders, Chest, Underbust, Waist, Hips, Thigh, Inseam} -> e.g. '49 cm', '108 cm')." },
                    "garment_cut": { "type": "string", "description": "For clothing/armor: 'fitted', 'tailored', 'standard', 'relaxed', 'loose', 'stretch', or 'rigid_armor'." },
                    "garment_taper": { "type": "string", "description": "For clothing/armor: 'straight', 'boxy', 'athletic_v', 'tailored_waist', 'hourglass', 'a_line', or 'relaxed_drape'." },
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
            "description": "Rename an entity's slug OR merge a duplicate entity (old_id) into a canonical target (new_id, with allow_merge: true) within the same register. Automatically rewrites all cross-references across relations, histories, lore, texts, items, attachments, clocks, routes, factions, and character locations, and prunes stale .md docs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "register": { "type": "string", "enum": ["character", "place", "lore", "text", "item", "attachment", "faction"] },
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
            "description": "Cleanly delete a miscreated, duplicate, or obsolete record from any register (character, place, lore, text, item, history, attachment, clock, faction), optionally removing dangling Knowledge Graph relations and stale compiled .md docs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "register": { "type": "string", "enum": ["character", "place", "lore", "text", "item", "history", "attachment", "clock", "faction"] },
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
                    "append_ruleset_notes": { "type": "string", "description": "Append a standing protocol/rule to campaign ruleset_notes without overwriting existing notes." },
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
            "name": "read_campaign_file",
            "description": "Read any campaign attachment (`attachment:<id>`), readable story text (`text:<id>`), lore entry (`lore:<id>`), or compiled Markdown doc (`doc:INDEX.md`, `doc:characters/aria.md`, `doc:attachments/files/...`) VERBATIM in exact UTF-8 character chunks (default 6000 chars/chunk, zero compression or summarization). When `has_more` is true, call again with the returned `next_call` arguments to retrieve the complete file verbatim. Pass `list: true` to list all readable campaign files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "Target specifier: 'attachment:<id>', 'text:<id>', 'lore:<id>', 'doc:<relative_path>', or 'list'." },
                    "attachment_id": { "type": "string", "description": "Shorthand for target='attachment:<id>'." },
                    "text_id": { "type": "string", "description": "Shorthand for target='text:<id>'." },
                    "lore_id": { "type": "string", "description": "Shorthand for target='lore:<id>'." },
                    "doc_path": { "type": "string", "description": "Shorthand for target='doc:<relative_path>' inside campaign docs/." },
                    "list": { "type": "boolean", "description": "Set true to list all readable attachments, texts, and docs/ files." },
                    "offset_chars": { "type": "integer", "default": 0, "description": "Zero-based UTF-8 character offset for verbatim pagination." },
                    "chunk_chars": { "type": "integer", "default": 6000, "description": "Exact characters per chunk (500..=16000; default 6000 avoids client redaction/compression)." },
                    "gm_override": { "type": "boolean", "default": false, "description": "For GM-only inspection of unread StoryTexts." }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "upsert_ruleset_profile",
            "description": "Define or update a campaign check resolution profile ('roll_over', 'roll_under', 'dice_pool', 'step_dice') with built-in D20/D&D 5e ability-score modifier conversion (`stat_modifier_mode`), standard 18-skill table (`skill_table`), progressive Skill XP curve (`xp_curve`), grounded DC rubric (`dc_rubric`: 5/10/15/20/25/30) + situational DC factors (`dc_factors`), and automatic Skill XP awards (`auto_skill_xp`). Pass `preset: 'dnd5e' | 'cultivation_d20' | 'pbta_2d6' | 'brp_d100'` for one-call setup, or `list: true` to inspect profiles.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "list": { "type": "boolean", "description": "Set true to list all saved ruleset profiles plus the built-in default (including full skill_table, xp_curve, and dc_rubric)." },
                    "preset": { "type": "string", "enum": ["dnd5e", "cultivation_d20", "pbta_2d6", "brp_d100"], "description": "Optional one-call ruleset preset." },
                    "id": { "type": "string", "description": "Profile slug (e.g. 'default', 'mortal-2d6', 'brp-d100')." },
                    "name": { "type": "string" },
                    "description": { "type": "string" },
                    "resolution_mode": { "type": "string", "enum": ["roll_over", "roll_under", "dice_pool", "step_dice"], "default": "roll_over" },
                    "base_dice": { "type": "string", "default": "1d20" },
                    "stat_modifier_mode": { "type": "string", "enum": ["auto_dnd5e", "dnd5e", "direct"], "default": "auto_dnd5e", "description": "How character stat scores convert to modifiers: 'dnd5e' applies floor((score-10)/2); 'auto_dnd5e' converts scores >= 6 via floor((score-10)/2) and uses small modifiers directly; 'direct' uses raw value." },
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
                    "skill_table": { "type": "object", "description": "Map of skill slug -> { name, governing_stat, category, description } (defaults to standard 18 D&D skills; add custom campaign skills here)." },
                    "xp_curve": { "type": "array", "description": "Ordered Skill XP thresholds [{ rank, min_xp, bonus }]." },
                    "dc_rubric": { "type": "object", "additionalProperties": { "type": "integer" }, "description": "Grounded difficulty ladder (trivial=5, easy=10, moderate=15, hard=20, very_hard=25, nearly_impossible=30)." },
                    "dc_factors": { "type": "object", "additionalProperties": { "type": "integer" }, "description": "Named situational DC adjustments (rushed=+2, proper_tools=-2, darkness=+3, etc.)." },
                    "auto_skill_xp": { "type": "boolean", "default": true },
                    "auto_hydrate": { "type": "boolean", "default": true }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "resolve_check",
            "description": "Resolve a structured action check (e.g. 1d20 + ability modifier + growing skill proficiency + gear/injuries/weather; totals naturally exceed 20). Supply either explicit `difficulty` OR grounded `dc_tier` (trivial/easy/moderate/hard/very_hard/nearly_impossible) + `dc_factors` for an auditable `dc_breakdown`. When `skill` is provided without `stat`, automatically hydrates the skill's governing ability modifier from the ruleset's skill_table and awards difficulty-scaled Skill XP on live checks.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "actor_id": { "type": "string", "description": "Character slug or name performing the action." },
                    "action": { "type": "string", "description": "Concrete action attempted (e.g. 'Pick the magistrate archive lock')." },
                    "difficulty": { "type": "integer", "description": "Explicit target DC (optional if `dc_tier` is provided)." },
                    "dc_tier": { "type": "string", "enum": ["trivial", "easy", "moderate", "hard", "very_hard", "nearly_impossible"], "description": "Grounded difficulty tier from RulesetProfile.dc_rubric (5/10/15/20/25/30) instead of picking an arbitrary DC." },
                    "dc_factors": { "type": "array", "description": "Optional situational DC adjustments: named strings ('rushed', 'improvised_tools', 'darkness', 'proper_tools', 'assisted', 'prepared') or custom { label, delta, reason } objects." },
                    "profile": { "type": "string", "default": "default", "description": "Ruleset profile ID (defaults to campaign 'default')." },
                    "stat": { "type": "string", "description": "Optional stat name (auto-hydrated from skill's governing_stat when omitted; scores convert via floor((score-10)/2))." },
                    "skill": { "type": "string", "description": "Optional skill name (character sheet OR standard ruleset skill_table; untrained skills use governing stat + 0 and still earn practice XP)." },
                    "technique": { "type": "string", "description": "Optional character technique name to auto-hydrate." },
                    "progression_track": { "type": "string", "description": "Optional progression track ID/name (e.g. 'qi-cultivation', 'body-tempering', 'character-level') whose modifier applies to this check." },
                    "equipment": { "type": "array", "items": { "type": "string" }, "description": "Optional specific carried items to factor in (including durability penalties)." },
                    "modifier": { "type": "integer", "default": 0, "description": "Additional manual modifier." },
                    "situational_modifiers": { "type": "array", "items": { "type": "object", "properties": { "source": { "type": "string" }, "value": { "type": "integer" }, "category": { "type": "string" }, "notes": { "type": "string" } }, "required": ["source", "value"] } },
                    "advantage": { "type": "string", "enum": ["none", "advantage", "disadvantage"], "default": "none" },
                    "dice_override": { "type": "string", "description": "Optional override dice expression." },
                    "auto_hydrate": { "type": "boolean" },
                    "award_skill_xp": { "type": "boolean", "description": "Override whether this live skill check awards difficulty-scaled Skill XP (defaults to profile.auto_skill_xp)." },
                    "stakes": { "type": "string", "description": "Explicit stakes at risk on failure/success." },
                    "seed": { "type": "integer", "description": "Optional deterministic RNG seed." },
                    "dry_run": { "type": "boolean", "default": false, "description": "Preview DC breakdown, itemized modifiers, and success odds WITHOUT rolling or logging." },
                    "strict_validation": { "type": "boolean", "default": false },
                    "log_to_ledger": { "type": "boolean", "default": true },
                    "causal_event_ids": { "type": "array", "items": { "type": "integer" } },
                    "visibility": { "type": "string", "enum": ["public", "witnessed_only", "gm_secret"], "default": "public" }
                },
                "required": ["action"]
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
            "description": "Set or merge structured Identity, Physical (constitution, body_tempering, disabilities, AND nested avatar fields: avatar_summary, appearance_stage, height, weight, baseline_height, baseline_weight, body_type_or_build, silhouette, posture_and_gait, proportions_summary, clothing_fit, measurements, baseline_measurements, face_shape_and_features, eyes, skin_tone_and_texture, hair, facial_hair, voice_and_speech, scent_or_aura, visual_prompt_anchor, record_checkpoint), Psychological/Autonomy drivers, and prioritized structured_goals. STYLE RULE: Write live observable avatar fields in concise, concrete, absolute present-tense (stranger's-eye view, ZERO comparative phrases like 'formerly' or 'relative to Day-1'); store starting numbers in `baseline_measurements` and current numbers in `measurements` so Chronicle auto-computes deltas in GM POV. Pass get_only=true to read.",
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
                    "avatar": { "type": "object", "description": "Optional partial nested AvatarProfile object (deep-merged without wiping unpassed fields)." },
                    "avatar_summary": { "type": "string", "description": "Concise, concrete, absolute present-tense stranger's-eye appearance (no comparative history)." },
                    "appearance_stage": { "type": "string", "description": "Optional progression stage tag for transforming bodies (e.g. 'Layer 5/9 — Low-Gradient Flow')." },
                    "height": { "type": "string" },
                    "weight": { "type": "string" },
                    "baseline_height": { "type": "string" },
                    "baseline_weight": { "type": "string" },
                    "body_type_or_build": { "type": "string" },
                    "silhouette": { "type": "string" },
                    "posture_and_gait": { "type": "string" },
                    "proportions_summary": { "type": "string" },
                    "clothing_fit": { "type": "string", "description": "Concise present-tense observation of how current clothing sits and drapes." },
                    "perceived_change_note": { "type": "string", "description": "Optional 1-sentence player-safe subjective note of ongoing physical drift (shown in Player/PC & GM POVs, hidden from strangers/NPCs; zero GM spoilers)." },
                    "transformation_trajectory": { "type": "string", "description": "Optional GM-only active transformation vector & next-stage trigger/recipe (shown ONLY in GM POV and echoed automatically when manage_progression advances a stage)." },
                    "measurements": { "type": "object", "additionalProperties": { "type": "string" }, "description": "CURRENT measurements map (canonical keys -> clean scalar values like '88 cm'; set '' to delete or pass replace_measurements=true)." },
                    "replace_measurements": { "type": "boolean" },
                    "baseline_measurements": { "type": "object", "additionalProperties": { "type": "string" }, "description": "BASELINE (Day-1 / pre-transformation) measurements map for auto-computed deltas." },
                    "replace_baseline_measurements": { "type": "boolean" },
                    "record_checkpoint": { "type": "object", "description": "Optional transformation checkpoint snapshot ({stage, world_time?, summary, height?, weight?, measurements?})." },
                    "face_shape_and_features": { "type": "string" },
                    "eyes": { "type": "string" },
                    "skin_tone_and_texture": { "type": "string" },
                    "hair": { "type": "string" },
                    "facial_hair": { "type": "string" },
                    "voice_and_speech": { "type": "string" },
                    "scent_or_aura": { "type": "string" },
                    "visual_prompt_anchor": { "type": "string" },
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
            "description": "Two-Phase Constrained Agency Sandbox + Living World Prominent Figure Mobility, Covert Roaming (Patron/Rival/Fixer/Hunter/Wanderer), Data-Driven Conveyances & Networking. Modes: 'prepare', 'commit', 'auto_mechanical', 'configure_mobility' (visibility: public|gm_secret, narrative_role: patron|rival|fixer|hunter|wanderer, roam_strategy: circuit|weighted_random|map_roam, roam_grid_id, roam_terrains, wild_roam_chance, pc_proximity_bias, gm_convergence_alert, convergence_radius_km, incognito_alias, conveyance: {label, speed_kmh, offroad_capable, signature}), 'dispatch_trip' (place, target_grid_pos, or uncharted_label), 'tick_movements', and 'list_movements'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mode": { "type": "string", "enum": ["prepare", "commit", "auto_mechanical", "configure_mobility", "dispatch_trip", "tick_movements", "list_movements"], "default": "prepare" },
                    "scope_type": { "type": "string", "enum": ["npc", "group", "faction"], "default": "npc" },
                    "character_id": { "type": "string", "description": "For configure_mobility / dispatch_trip." },
                    "prominent": { "type": "boolean" },
                    "mobility_mode": { "type": "string", "enum": ["auto", "stationary", "local_commuter", "business_circuit", "networking_envoy", "inspector", "wanderer"] },
                    "visibility": { "type": "string", "enum": ["public", "gm_secret"], "description": "Use 'gm_secret' for covert/incognito roamers hidden from Player POV." },
                    "narrative_role": { "type": "string", "enum": ["", "patron", "rival", "fixer", "hunter", "wanderer"], "description": "Recurring narrative role for GM convergence signals & autonomous flavor." },
                    "roam_strategy": { "type": "string", "enum": ["circuit", "weighted_random", "map_roam"], "description": "How destinations are picked: deterministic circuit, weighted_random, or map_roam (cities + wilderness grid cells + uncharted frontier)." },
                    "roam_grid_id": { "type": "string", "description": "Optional 2D regional map grid ID for map-aware wilderness roaming & km convergence radar." },
                    "roam_terrains": { "type": "array", "items": { "type": "string" }, "description": "Eligible terrain types for wilderness forays (e.g. ['forest','mountain','plains','road'])." },
                    "wild_roam_chance": { "type": "integer", "description": "0..=100 chance an excursion targets a wilderness grid cell or uncharted frontier instead of a named hub." },
                    "pc_proximity_bias": { "type": "integer", "description": "0..=100 soft narrative gravity biasing excursions toward the PC's general region without teleportation." },
                    "gm_convergence_alert": { "type": "boolean", "description": "When true, emits GM-secret NPC_CONVERGENCE_OPPORTUNITY alarms when this figure nears a PC." },
                    "convergence_radius_km": { "type": "number", "description": "Regional map km radius for GM convergence radar (default 25)." },
                    "incognito_alias": { "type": "string", "description": "Optional cover identity when roaming covertly." },
                    "conveyance": { "type": "object", "properties": { "label": { "type": "string" }, "speed_kmh": { "type": "number" }, "offroad_capable": { "type": "boolean" }, "signature": { "type": "string", "enum": ["conspicuous", "discreet", "clandestine"] } }, "description": "Data-driven travel style / conveyance (e.g. carried sedan, hover-limo, coach, courier skiff)." },
                    "target_grid_pos": { "type": "object", "properties": { "grid_id": { "type": "string" }, "x": { "type": "integer" }, "y": { "type": "integer" }, "terrain": { "type": "string" }, "label": { "type": "string" } }, "description": "Optional wilderness map coordinate for dispatch_trip." },
                    "uncharted_label": { "type": "string", "description": "Optional off-map/uncharted frontier description for dispatch_trip (never fabricates canon map places)." },
                    "include_gm_secrets": { "type": "boolean", "default": true },
                    "home_place_id": { "type": "string" },
                    "destination_place_id": { "type": "string", "description": "Target place for dispatch_trip." },
                    "preferred_destinations": { "type": "array", "items": { "type": "string" } },
                    "purposes": { "type": "array", "items": { "type": "string" } },
                    "purpose": { "type": "string", "description": "Trip purpose for dispatch_trip (business, networking, diplomacy, inspection, procurement)." },
                    "details": { "type": "string" },
                    "trip_cadence_days": { "type": "integer", "default": 7 },
                    "stay_duration_days": { "type": "integer", "default": 3 },
                    "travel_speed_mode": { "type": "string", "enum": ["foot", "mount", "wheeled", "cart", "carriage", "water", "boat", "air", "flying", "fast_transit"], "default": "mount" },
                    "entourage_ids": { "type": "array", "items": { "type": "string" } },
                    "auto_include_prominent": { "type": "boolean", "default": true },
                    "seed": { "type": "integer" },
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
            "description": "Connected geography, graph pathfinding (Dijkstra), travel execution, grounded exploration, AND Sparse Multi-Scale 2D (x,y) Map Grids (genre-universal: world/region/settlement/dungeon/space_sector/tactical). Actions: upsert_route | list_routes | update_spatial | calculate | advance | explore | upsert_grid | list_grids | get_grid (supports windowed center_x/center_y/radius & compass neighborhood) | set_cell (single x,y or batch cells[]) | clear_cell | auto_layout_grid.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["upsert_route", "list_routes", "update_spatial", "calculate", "advance", "explore", "upsert_grid", "list_grids", "get_grid", "set_cell", "clear_cell", "auto_layout_grid"] },
                    "id": { "type": "string" },
                    "grid_id": { "type": "string", "description": "Map grid ID/slug for 2D grid actions." },
                    "name": { "type": "string" },
                    "scale_kind": { "type": "string", "enum": ["world", "region", "settlement", "dungeon", "space_sector", "tactical"] },
                    "parent_place_id": { "type": "string" },
                    "width": { "type": "integer", "default": 16 },
                    "height": { "type": "integer", "default": 12 },
                    "cell_scale_label": { "type": "string", "description": "Scale per cell (e.g. '5 km', '100 m', '1 AU', '10 ft')." },
                    "default_terrain": { "type": "string", "default": "wilderness" },
                    "description": { "type": "string" },
                    "x": { "type": "integer" },
                    "y": { "type": "integer" },
                    "center_x": { "type": "integer" },
                    "center_y": { "type": "integer" },
                    "radius": { "type": "integer" },
                    "elevation": { "type": "integer" },
                    "label": { "type": "string" },
                    "landmark_name": { "type": "string" },
                    "faction_control": { "type": "string" },
                    "fog_status": { "type": "string", "enum": ["charted", "surveyed", "glimpsed", "unexplored"] },
                    "passable": { "type": "boolean", "default": true },
                    "gm_notes": { "type": "string" },
                    "include_gm_secrets": { "type": "boolean", "default": true },
                    "cells": { "type": "array", "description": "Optional batch array of {x, y, terrain, label, place_id, landmark_name, faction_control, fog_status, passable, hazards, notes, gm_notes} for set_cell." },
                    "reason": { "type": "string" },
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
            "description": "Bounded-scope world simulation tick: advances world clock by duration_hours/duration_days and orchestrates regional weather, injury recovery, market production/consumption drift, routine faction ration upkeep, prominent figure business/networking travel & engagements (route-aware via Dijkstra), and optional leak-proof autonomy dossiers.",
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
                    "tick_npc_movements": { "type": "boolean", "default": true, "description": "Simulate prominent figure business circuits, networking travel, arrivals, and organic rumor exchange along open routes." },
                    "prepare_autonomy_dossiers": { "type": "boolean", "default": false },
                    "seed": { "type": "integer" }
                }
            }
        }"#));
    v.push(parse(r#"{
            "name": "manage_progression",
            "description": "Stateful, genre-agnostic progression engine for Xianxia cultivation realms (1-9 sub-stages per major realm + parallel manual/body-tempering tracks with unique properties, halt-conditions, and bottlenecks), D&D levels (1-20), or Sci-Fi/Cyberpunk operational/psionic tiers. Tracks auto-compile onto character sheets, surface in get_scene_context (with POV-safe gm_notes), and auto-hydrate modifiers into resolve_check.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": [
                            "define_ladder",
                            "list_ladders",
                            "enroll_track",
                            "advance_xp",
                            "adjust_xp",
                            "revert_xp",
                            "reset_xp",
                            "attempt_breakthrough",
                            "update_track",
                            "remove_track",
                            "get_character_progression"
                        ]
                    },
                    "preset": { "type": "string", "enum": ["xianxia_standard", "body_tempering", "dnd_level_20", "scifi_tier"], "description": "For define_ladder: one-call genre ladder preset." },
                    "ladder_id": { "type": "string" },
                    "name": { "type": "string" },
                    "kind": { "type": "string", "description": "e.g. realm_ladder, parallel_manual, body_refinement, character_level, scifi_tier." },
                    "description": { "type": "string" },
                    "source_text_id": { "type": "string", "description": "Linked StoryText ID or title (e.g. a cultivation manual/scroll in Books & Scrolls)." },
                    "source_item_id": { "type": "string", "description": "Optional linked CatalogItem ID for a physical manual/relic." },
                    "manual_title": { "type": "string", "description": "Optional manual title override (auto-resolved from source_text_id/source_item_id if omitted)." },
                    "manual_notes": { "type": "string", "description": "Notes specific to the cultivation manual being practiced (breathing cadence, meridian pathway, affinities, incompatibilities, personal annotations)." },
                    "default_check_tags": { "type": "array", "items": { "type": "string" } },
                    "stages": { "type": "array", "description": "For custom define_ladder: [{ name, sub_stages (e.g. 9), sub_stage_prefix, xp_per_sub_stage, xp_thresholds, breakthrough_required_at_peak, breakthrough_dc_tier, passive_modifier, lifespan_years, properties, halt_conditions }]." },
                    "character_id": { "type": "string" },
                    "track_id": { "type": "string" },
                    "track_kind": { "type": "string", "enum": ["primary_realm", "parallel_manual", "body_refinement", "level", "discipline"], "default": "primary_realm" },
                    "major_stage_index": { "type": "integer", "default": 0 },
                    "major_stage_name": { "type": "string" },
                    "sub_stage": { "type": "integer", "default": 1 },
                    "sub_stage_max": { "type": "integer", "default": 9 },
                    "sub_stage_label": { "type": "string" },
                    "xp": { "type": "integer", "description": "Signed XP delta for advance_xp/adjust_xp/revert_xp (negative reverts awarded XP, stepping back sub-stages if allow_demote=true)." },
                    "delta_xp": { "type": "integer", "description": "Alias for signed XP delta on adjust_xp/revert_xp." },
                    "set_current_xp": { "type": "integer", "description": "Set exact current stage XP directly (for adjust_xp)." },
                    "set_cumulative_xp": { "type": "integer", "description": "Set exact cumulative XP directly (for adjust_xp)." },
                    "reset_stage_xp": { "type": "boolean", "description": "Reset current stage XP to 0 and clear peak bottleneck (for adjust_xp/reset_xp)." },
                    "reset_track": { "type": "boolean", "description": "Full track reset back to Stage 0, Sub-stage 1, 0 XP (for adjust_xp/reset_xp)." },
                    "allow_demote": { "type": "boolean", "default": true, "description": "When reverting XP past 0 on the current sub-stage, step back sub-stages/major stages cleanly." },
                    "current_xp": { "type": "integer" },
                    "xp_to_next": { "type": "integer" },
                    "cumulative_xp": { "type": "integer" },
                    "at_bottleneck": { "type": "boolean" },
                    "bottleneck_reason": { "type": "string" },
                    "halt_conditions": { "type": "array", "items": { "type": "string" } },
                    "active_properties": { "type": "array", "items": { "type": "string" } },
                    "add_properties": { "type": "array", "items": { "type": "string" } },
                    "check_modifier": { "type": "integer" },
                    "check_tags": { "type": "array", "items": { "type": "string" } },
                    "status": { "type": "string", "enum": ["active", "stalled", "halted", "completed", "crippled"] },
                    "notes": { "type": "string" },
                    "gm_notes": { "type": "string", "description": "GM-only progression notes (stripped from player POV)." },
                    "profile": { "type": "string" },
                    "stat": { "type": "string" },
                    "skill": { "type": "string" },
                    "difficulty": { "type": "integer" },
                    "dc_tier": { "type": "string" },
                    "dc_factors": { "type": "array" },
                    "modifier": { "type": "integer" },
                    "advantage": { "type": "string", "enum": ["none", "advantage", "disadvantage"] },
                    "stakes": { "type": "string" },
                    "seed": { "type": "integer" },
                    "next_stage_name": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["action"]
            }
        }"#));
    v.push(parse(r#"{
            "name": "manage_player_character",
            "description": "Dedicated Player Character (PC) sheet manager (defaults to primary PC when character_id is omitted). Manages: Avatar & Body description with dedicated BASELINE vs. CURRENT measurements (auto-computing numeric deltas), Appearance Stage, Clothing Fit, Transformation Checkpoints, height/weight/lifespan, build/silhouette/proportions, face/eyes/skin/hair/voice/aura, permanent scars/tattoos/marks, modular wardrobe & active outfit, portrait attachment, AI visual prompt anchor, Titles & Epithets, Innate Traits/Physique/Spirit Roots, and PROPERLY SEGREGATED XP pools (Stat Training XP, Job/Class XP, Formal Proficiencies). STYLE RULE: Keep live avatar prose fields concise, concrete, and strictly present-tense (stranger's-eye view — ZERO comparative phrasing like 'formerly' or 'relative to Day-1'); put starting numbers in `baseline_measurements` and current numbers in `measurements` so Chronicle auto-computes deltas for GM / returning-NPC reference.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": [
                            "get_sheet",
                            "update_avatar",
                            "upsert_marker",
                            "remove_marker",
                            "upsert_outfit",
                            "set_active_outfit",
                            "remove_outfit",
                            "upsert_title",
                            "toggle_title",
                            "remove_title",
                            "adjust_stat_xp",
                            "upsert_job",
                            "adjust_job_xp",
                            "remove_job",
                            "upsert_proficiency",
                            "adjust_proficiency_xp",
                            "remove_proficiency",
                            "upsert_trait",
                            "remove_trait"
                        ]
                    },
                    "character_id": { "type": "string", "description": "Optional character ID/slug (defaults to primary PC when omitted)." },
                    "id": { "type": "string" },
                    "job_id": { "type": "string" },
                    "name": { "type": "string" },
                    "kind": { "type": "string" },
                    "category": { "type": "string" },
                    "role": { "type": "string", "enum": ["primary", "secondary", "occupation", "trade", "sect_role"] },
                    "avatar_summary": { "type": "string", "description": "Concise, concrete, absolute present-tense stranger's-eye description of the PC's current physical presence (no comparative history)." },
                    "appearance_stage": { "type": "string", "description": "Optional stage tag for a transforming body (e.g. 'Still Water Layer 5/9 — Low-Gradient Flow')." },
                    "height": { "type": "string" },
                    "weight": { "type": "string" },
                    "baseline_height": { "type": "string", "description": "Original Day-1 / pre-transformation height for auto-delta tracking." },
                    "baseline_weight": { "type": "string", "description": "Original Day-1 / pre-transformation weight for auto-delta tracking." },
                    "chronological_age": { "type": "string" },
                    "apparent_age": { "type": "string" },
                    "lifespan_years": { "type": "integer" },
                    "body_type_or_build": { "type": "string" },
                    "silhouette": { "type": "string" },
                    "posture_and_gait": { "type": "string" },
                    "proportions_summary": { "type": "string" },
                    "clothing_fit": { "type": "string", "description": "Concise present-tense description of how current clothing sits and drapes." },
                    "perceived_change_note": { "type": "string", "description": "Optional 1-sentence player-safe subjective cue of ongoing change (shown in Player/PC & GM POVs, hidden from other NPCs; zero hidden spoilers)." },
                    "transformation_trajectory": { "type": "string", "description": "Optional GM-only active transformation vector & next-stage update recipe (shown ONLY in GM POV and auto-surfaced when manage_progression advances a stage)." },
                    "measurements": { "type": "object", "additionalProperties": { "type": "string" }, "description": "CURRENT body measurements (canonical keys -> clean scalar values like '88 cm'; keys auto-canonicalized)." },
                    "replace_measurements": { "type": "boolean" },
                    "set_measurement": { "type": "object", "properties": { "key": { "type": "string" }, "value": { "type": "string" } } },
                    "remove_measurement": { "type": "string" },
                    "baseline_measurements": { "type": "object", "additionalProperties": { "type": "string" }, "description": "BASELINE (Day-1 / pre-transformation) measurements map for auto-computed deltas." },
                    "replace_baseline_measurements": { "type": "boolean" },
                    "set_baseline_measurement": { "type": "object", "properties": { "key": { "type": "string" }, "value": { "type": "string" } } },
                    "remove_baseline_measurement": { "type": "string" },
                    "record_checkpoint": { "type": "object", "description": "Snapshot a transformation checkpoint ({stage, world_time?, summary, height?, weight?, measurements?})." },
                    "transformation_log": { "type": "array", "items": { "type": "object" }, "description": "Replace full transformation_log checkpoint array." },
                    "clear_checkpoints": { "type": "boolean" },
                    "face_shape_and_features": { "type": "string" },
                    "eyes": { "type": "string" },
                    "skin_tone_and_texture": { "type": "string" },
                    "hair": { "type": "string" },
                    "facial_hair": { "type": "string" },
                    "voice_and_speech": { "type": "string" },
                    "scent_or_aura": { "type": "string" },
                    "gender": { "type": "string" },
                    "species": { "type": "string" },
                    "body_constitution": { "type": "string" },
                    "body_tempering": { "type": "string" },
                    "portrait_attachment_id": { "type": "string" },
                    "visual_prompt_anchor": { "type": "string" },
                    "active_outfit": { "type": "string" },
                    "location": { "type": "string" },
                    "description": { "type": "string" },
                    "origin_or_story": { "type": "string" },
                    "visible_when_clothed": { "type": "boolean" },
                    "occasion": { "type": "string" },
                    "headwear": { "type": "string" },
                    "outer_layer": { "type": "string" },
                    "top_layer": { "type": "string" },
                    "bottoms": { "type": "string" },
                    "footwear": { "type": "string" },
                    "accessories": { "type": "string" },
                    "color_palette": { "type": "string" },
                    "set_active": { "type": "boolean" },
                    "equipped": { "type": "boolean" },
                    "source": { "type": "string" },
                    "mechanical_effect": { "type": "string" },
                    "stat_modifiers": { "type": "object", "additionalProperties": { "type": "integer" } },
                    "visibility": { "type": "string", "enum": ["public", "faction", "hidden", "gm_secret"] },
                    "stat": { "type": "string" },
                    "delta_xp": { "type": "integer", "description": "Signed XP delta (positive awards, negative reverts/demotes)." },
                    "xp": { "type": "integer" },
                    "set_xp": { "type": "integer" },
                    "set_cumulative_xp": { "type": "integer" },
                    "xp_to_next": { "type": "integer" },
                    "reset_xp": { "type": "boolean" },
                    "reset_stage_xp": { "type": "boolean" },
                    "reset_job": { "type": "boolean" },
                    "allow_demote": { "type": "boolean", "default": true },
                    "auto_promote": { "type": "boolean", "default": true },
                    "recalc_rank": { "type": "boolean", "default": true },
                    "unspent_stat_points_delta": { "type": "integer" },
                    "set_unspent_stat_points": { "type": "integer" },
                    "spend_stat_point": { "type": "boolean" },
                    "training_notes": { "type": "string" },
                    "tier_or_rank": { "type": "string" },
                    "level": { "type": "integer" },
                    "set_level": { "type": "integer" },
                    "active": { "type": "boolean" },
                    "governing_stats": { "type": "array", "items": { "type": "string" } },
                    "perks_and_features": { "type": "array", "items": { "type": "string" } },
                    "rank": { "type": "string" },
                    "rank_or_grade": { "type": "string" },
                    "bonus": { "type": "string" },
                    "effect": { "type": "string" },
                    "notes": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["action"]
            }
        }"#));
    v
}

/// Returns the canonical list of all Chronicle tools (Legacy + Unified Domain Facades).
pub fn all_tool_definitions() -> Value {
    tool_definitions_for_profile(ToolProfile::Legacy)
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

fn parse_map_cell(grid_id: &str, x: i32, y: i32, obj: &Value) -> crate::world_sim::MapGridCell {
    crate::world_sim::MapGridCell {
        grid_id: grid_id.to_string(),
        x,
        y,
        terrain: opt_str(obj, "terrain").unwrap_or_default(),
        elevation: obj.get("elevation").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        label: opt_str(obj, "label").unwrap_or_default(),
        place_id: opt_str(obj, "place_id"),
        landmark_name: opt_str(obj, "landmark_name").unwrap_or_default(),
        faction_control: opt_str(obj, "faction_control"),
        fog_status: opt_str(obj, "fog_status").unwrap_or_else(|| "charted".to_string()),
        passable: obj.get("passable").and_then(|v| v.as_bool()).unwrap_or(true),
        hazards: opt_str_vec(obj, "hazards"),
        notes: opt_str(obj, "notes").unwrap_or_default(),
        gm_notes: opt_str(obj, "gm_notes").unwrap_or_default(),
        updated_at_world_time: String::new(),
    }
}

// =============================================================================
// Active Signal Bus, Standing Routines & Unified Domain Facade Dispatchers
// =============================================================================

fn with_sub_action_as_action(mut args: Value, fallback_action: Option<&str>) -> Value {
    if let Some(obj) = args.as_object_mut() {
        obj.remove("op");
        obj.remove("operation");
        if let Some(sa) = obj.remove("sub_action") {
            obj.insert("action".to_string(), sa);
        } else if let Some(fb) = fallback_action {
            obj.insert("action".to_string(), Value::String(fb.to_string()));
        }
    }
    args
}

fn extract_op(args: &Value, default_op: &str) -> String {
    opt_str(args, "op")
        .or_else(|| opt_str(args, "operation"))
        .or_else(|| opt_str(args, "action"))
        .unwrap_or_else(|| default_op.to_string())
        .trim()
        .to_ascii_lowercase()
}

fn execute_manage_signals(engine: &CampaignEngine, args: Value) -> Result<Value> {
    let op = extract_op(&args, "list");
    match op.as_str() {
        "list" | "list_signals" | "get" => {
            let perspective = opt_str(&args, "perspective");
            let place_id = opt_str(&args, "place_id");
            let character_ids = opt_str_vec(&args, "character_ids");
            let include_secrets = args
                .get("include_gm_secrets")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let signals = engine.list_active_signals(
                perspective.as_deref(),
                place_id.as_deref(),
                &character_ids,
                include_secrets,
            )?;
            Ok(json!({ "count": signals.len(), "signals": signals }))
        }
        "ack" | "acknowledge" | "ack_signal" | "acknowledge_signal" => {
            let sid = opt_str(&args, "signal_id")
                .or_else(|| opt_str(&args, "id"))
                .or_else(|| opt_str(&args, "dedup_key"))
                .ok_or_else(|| anyhow!("Missing 'signal_id' (or 'id' / 'dedup_key')"))?;
            let updated = engine.update_signal(&sid, "acknowledged")?;
            Ok(json!({ "acknowledged": updated.is_some(), "signal": updated }))
        }
        "resolve" | "dismiss" | "resolve_signal" | "dismiss_signal" => {
            let sid = opt_str(&args, "signal_id")
                .or_else(|| opt_str(&args, "id"))
                .or_else(|| opt_str(&args, "dedup_key"))
                .ok_or_else(|| anyhow!("Missing 'signal_id' (or 'id' / 'dedup_key')"))?;
            let updated = engine.update_signal(&sid, "resolved")?;
            Ok(json!({ "resolved": updated.is_some(), "signal": updated }))
        }
        "delete" | "clear" => {
            let sid = opt_str(&args, "signal_id")
                .or_else(|| opt_str(&args, "id"))
                .ok_or_else(|| anyhow!("Missing 'signal_id'"))?;
            let _ = engine.update_signal(&sid, "delete")?;
            Ok(json!({ "deleted": true, "signal_id": sid }))
        }
        "emit" | "emit_signal" | "create" => {
            let sig: crate::signals::EngineSignal = serde_json::from_value(args)?;
            let saved = engine.emit_signal(sig)?;
            Ok(json!({ "signal": saved }))
        }
        other => Err(anyhow!(
            "Unknown signal action '{other}'. Expected list, acknowledge, resolve, emit, or delete."
        )),
    }
}

fn execute_manage_routines(engine: &CampaignEngine, args: Value) -> Result<Value> {
    let op = extract_op(&args, "list");
    match op.as_str() {
        "list" | "list_routines" | "get" => {
            let cid = opt_str(&args, "character_id");
            let routines = engine.list_routines(cid.as_deref())?;
            Ok(json!({ "count": routines.len(), "routines": routines }))
        }
        "upsert" | "upsert_routine" | "save" | "create" => {
            let mut routine_val = args
                .get("routine")
                .filter(|v| v.is_object())
                .cloned()
                .unwrap_or_else(|| args.clone());
            if let Some(obj) = routine_val.as_object_mut() {
                if !obj.contains_key("enabled") && !obj.contains_key("active") {
                    if let Some(st) = obj.get("status").and_then(|v| v.as_str()) {
                        let sl = st.trim().to_ascii_lowercase();
                        if matches!(sl.as_str(), "paused" | "disabled" | "halted" | "inactive") {
                            obj.insert("enabled".to_string(), Value::Bool(false));
                        } else if matches!(sl.as_str(), "active" | "enabled") {
                            obj.insert("enabled".to_string(), Value::Bool(true));
                        }
                    }
                }
            }
            let routine: crate::routines::CharacterRoutine = serde_json::from_value(routine_val)?;
            let (saved, ev) = engine.upsert_routine(routine)?;
            Ok(json!({ "event_seq": ev.seq, "summary": ev.summary, "routine": saved }))
        }
        "delete" | "remove" | "delete_routine" => {
            let rid = opt_str(&args, "routine_id")
                .or_else(|| opt_str(&args, "id"))
                .ok_or_else(|| anyhow!("Missing 'routine_id' (or 'id')"))?;
            let deleted = engine.delete_routine(&rid)?;
            Ok(json!({ "deleted": deleted, "routine_id": rid }))
        }
        other => Err(anyhow!(
            "Unknown routine action '{other}'. Expected list, upsert, or delete."
        )),
    }
}

fn execute_domain_scene(
    engine: &CampaignEngine,
    mut args: Value,
    allow_local_file_import: bool,
) -> Result<Value> {
    let op = extract_op(&args, "context");
    match op.as_str() {
        "status" | "campaign_status" => {
            execute_tool_with_policy(engine, "campaign_status", args, allow_local_file_import)
        }
        "context" | "get_scene_context" => {
            if let Some(obj) = args.as_object_mut() {
                if !obj.contains_key("format") {
                    obj.insert("format".to_string(), Value::String("markdown".to_string()));
                }
            }
            execute_tool_with_policy(engine, "get_scene_context", args, allow_local_file_import)
        }
        "signals" | "list_signals" => execute_manage_signals(engine, json!({
            "op": "list",
            "perspective": args.get("perspective").cloned().unwrap_or(Value::Null),
            "place_id": args.get("place_id").cloned().unwrap_or(Value::Null),
            "character_ids": args.get("character_ids").cloned().unwrap_or_else(|| json!([])),
            "include_gm_secrets": args.get("include_gm_secrets").cloned().unwrap_or(json!(true))
        })),
        "ack_signal" | "acknowledge_signal" => {
            let mut a = args;
            if let Some(o) = a.as_object_mut() {
                o.insert("op".to_string(), json!("acknowledge"));
            }
            execute_manage_signals(engine, a)
        }
        "resolve_signal" | "dismiss_signal" => {
            let mut a = args;
            if let Some(o) = a.as_object_mut() {
                o.insert("op".to_string(), json!("resolve"));
            }
            execute_manage_signals(engine, a)
        }
        "emit_signal" => {
            let mut a = args;
            if let Some(o) = a.as_object_mut() {
                o.insert("op".to_string(), json!("emit"));
            }
            execute_manage_signals(engine, a)
        }
        "start_session" | "start_new_session" => {
            execute_tool_with_policy(engine, "start_new_session", args, allow_local_file_import)
        }
        other => Err(anyhow!(
            "Unknown scene action '{other}'. Expected context, status, signals, ack_signal, resolve_signal, emit_signal, or start_session."
        )),
    }
}

fn execute_domain_query(
    engine: &CampaignEngine,
    args: Value,
    allow_local_file_import: bool,
) -> Result<Value> {
    let op = extract_op(&args, "search_all");
    let target_tool = match op.as_str() {
        "character" | "get_character" => "get_character",
        "place" | "get_place" => "get_place",
        "text" | "get_text" => "get_text",
        "item" | "get_item" => "get_item",
        "lore" | "get_lore" => "get_lore",
        "attachment" | "get_attachment" => "get_attachment",
        "read_file" | "read_campaign_file" | "file" => "read_campaign_file",
        "all" | "omni" | "search_all" => "search_all",
        "characters" | "search_characters" => "search_characters",
        "places" | "search_places" => "search_places",
        "lore_search" | "search_lore" => "search_lore",
        "texts" | "search_texts" => "search_texts",
        "items" | "search_items" => "search_items",
        "inventory" | "search_inventory" => "search_inventory",
        "histories" | "search_histories" => "search_histories",
        "attachments" | "search_attachments" => "search_attachments",
        "events" | "history" | "search_history" | "search_events" => "search_history",
        "relations" | "search_relations" => "search_relations",
        "knowledge" | "search_knowledge" => "search_knowledge",
        other => {
            return Err(anyhow!(
                "Unknown query op '{other}'. Expected get_character, get_place, get_text, get_item, get_lore, get_attachment, read_file, search_all, search_characters, search_places, search_lore, search_texts, search_items, search_histories, search_attachments, search_events, search_relations, or search_knowledge."
            ));
        }
    };
    execute_tool_with_policy(engine, target_tool, args, allow_local_file_import)
}

fn execute_domain_resolve(
    engine: &CampaignEngine,
    args: Value,
    allow_local_file_import: bool,
) -> Result<Value> {
    // Note: `CheckRequest` uses `action` for the check's narrative description, so `resolve` reads `op` (or `mode`),
    // only falling back to `action` if it matches a known sub-tool keyword!
    let explicit_op = opt_str(&args, "op").or_else(|| opt_str(&args, "mode"));
    let op = match explicit_op {
        Some(s) => s.trim().to_ascii_lowercase(),
        None => {
            let act = opt_str(&args, "action")
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if matches!(
                act.as_str(),
                "contest"
                    | "resolve_contest"
                    | "draw_table"
                    | "draw_random_table"
                    | "roll"
                    | "roll_dice"
                    | "ruleset"
                    | "upsert_ruleset_profile"
                    | "random_table"
                    | "upsert_random_table"
                    | "encounter"
                    | "manage_encounter"
            ) {
                act
            } else {
                "check".to_string()
            }
        }
    };

    match op.as_str() {
        "check" | "resolve_check" => {
            execute_tool_with_policy(engine, "resolve_check", args, allow_local_file_import)
        }
        "contest" | "resolve_contest" => {
            execute_tool_with_policy(engine, "resolve_contest", args, allow_local_file_import)
        }
        "draw_table" | "table" | "draw_random_table" => {
            execute_tool_with_policy(engine, "draw_random_table", args, allow_local_file_import)
        }
        "roll" | "roll_dice" => {
            execute_tool_with_policy(engine, "roll_dice", args, allow_local_file_import)
        }
        "ruleset" | "upsert_ruleset" | "upsert_ruleset_profile" => {
            execute_tool_with_policy(engine, "upsert_ruleset_profile", args, allow_local_file_import)
        }
        "random_table" | "upsert_table" | "upsert_random_table" => {
            execute_tool_with_policy(engine, "upsert_random_table", args, allow_local_file_import)
        }
        "encounter" | "manage_encounter" => {
            let mapped = with_sub_action_as_action(args, None);
            execute_tool_with_policy(engine, "manage_encounter", mapped, allow_local_file_import)
        }
        other => Err(anyhow!(
            "Unknown resolve op '{other}'. Expected check, contest, draw_table, roll_dice, ruleset, random_table, or encounter."
        )),
    }
}

fn execute_domain_character(
    engine: &CampaignEngine,
    args: Value,
    allow_local_file_import: bool,
) -> Result<Value> {
    let op = extract_op(&args, "get");
    match op.as_str() {
        "create" | "create_character" => {
            execute_tool_with_policy(engine, "create_character", args, allow_local_file_import)
        }
        "get" | "get_character" => {
            execute_tool_with_policy(engine, "get_character", args, allow_local_file_import)
        }
        "move" | "move_character" => {
            execute_tool_with_policy(engine, "move_character", args, allow_local_file_import)
        }
        "resource" | "modify_resource" | "modify_character_resource" => execute_tool_with_policy(
            engine,
            "modify_character_resource",
            args,
            allow_local_file_import,
        ),
        "stat" | "modify_stat" | "modify_character_stat" => execute_tool_with_policy(
            engine,
            "modify_character_stat",
            args,
            allow_local_file_import,
        ),
        "status" | "set_status" | "set_character_status" => execute_tool_with_policy(
            engine,
            "set_character_status",
            args,
            allow_local_file_import,
        ),
        "condition" | "update_condition" | "update_character_condition" => {
            let mapped = with_sub_action_as_action(args, Some("add"));
            execute_tool_with_policy(
                engine,
                "update_character_condition",
                mapped,
                allow_local_file_import,
            )
        }
        "equipment" | "update_equipment" | "update_character_equipment" => {
            let mapped = with_sub_action_as_action(args, Some("acquire"));
            execute_tool_with_policy(
                engine,
                "update_character_equipment",
                mapped,
                allow_local_file_import,
            )
        }
        "transfer_item" => {
            execute_tool_with_policy(engine, "transfer_item", args, allow_local_file_import)
        }
        "skill" | "upsert_skill" | "upsert_character_skill" => execute_tool_with_policy(
            engine,
            "upsert_character_skill",
            args,
            allow_local_file_import,
        ),
        "technique" | "upsert_technique" | "upsert_character_technique" => {
            execute_tool_with_policy(
                engine,
                "upsert_character_technique",
                args,
                allow_local_file_import,
            )
        }
        "goal" | "update_goal" | "update_character_goal" => execute_tool_with_policy(
            engine,
            "update_character_goal",
            args,
            allow_local_file_import,
        ),
        "rest" | "rest_characters" => {
            execute_tool_with_policy(engine, "rest_characters", args, allow_local_file_import)
        }
        "profile" | "update_profile" | "update_character_profile" => execute_tool_with_policy(
            engine,
            "update_character_profile",
            args,
            allow_local_file_import,
        ),
        "pc_sheet" | "manage_player_character" => {
            let mapped = with_sub_action_as_action(args, Some("get_sheet"));
            execute_tool_with_policy(
                engine,
                "manage_player_character",
                mapped,
                allow_local_file_import,
            )
        }
        "get_sheet"
        | "update_avatar"
        | "upsert_marker"
        | "remove_marker"
        | "upsert_outfit"
        | "set_active_outfit"
        | "remove_outfit"
        | "upsert_title"
        | "toggle_title"
        | "remove_title"
        | "adjust_stat_xp"
        | "upsert_job"
        | "adjust_job_xp"
        | "remove_job"
        | "upsert_proficiency"
        | "adjust_proficiency_xp"
        | "remove_proficiency"
        | "upsert_trait"
        | "remove_trait" => {
            let mapped = with_sub_action_as_action(args, Some(&op));
            execute_tool_with_policy(
                engine,
                "manage_player_character",
                mapped,
                allow_local_file_import,
            )
        }
        "health" | "manage_health_and_downtime" => {
            let mapped = with_sub_action_as_action(args, Some("list_injuries"));
            execute_tool_with_policy(
                engine,
                "manage_health_and_downtime",
                mapped,
                allow_local_file_import,
            )
        }
        "apply_injury"
        | "treat_injury"
        | "advance_recovery"
        | "list_injuries"
        | "resolve_downtime"
        | "begin_crafting"
        | "resolve_crafting" => {
            let mapped = with_sub_action_as_action(args, Some(&op));
            execute_tool_with_policy(
                engine,
                "manage_health_and_downtime",
                mapped,
                allow_local_file_import,
            )
        }
        "routine" | "routines" => {
            let mapped = with_sub_action_as_action(args, Some("list"));
            execute_manage_routines(engine, mapped)
        }
        "upsert_routine" | "list_routines" | "delete_routine" => {
            let mut mapped = args;
            if let Some(o) = mapped.as_object_mut() {
                o.insert("op".to_string(), Value::String(op));
            }
            execute_manage_routines(engine, mapped)
        }
        other => Err(anyhow!(
            "Unknown character op '{other}'. Expected create, get, move, resource, stat, status, condition, equipment, transfer_item, skill, technique, goal, rest, profile, pc_sheet, health, or routine."
        )),
    }
}

fn execute_domain_progression(
    engine: &CampaignEngine,
    mut args: Value,
    allow_local_file_import: bool,
) -> Result<Value> {
    let op = extract_op(&args, "get_character_progression");
    if op == "apply_stage_rule" {
        let cid = req_str(&args, "character_id")?;
        let tid = slugify(&req_str(&args, "track_id")?);
        let track = engine
            .db()
            .get_character_progression(&cid, &tid)?
            .ok_or_else(|| anyhow!("Track '{tid}' not enrolled on '{cid}'"))?;
        let ladder = match track.ladder_id.as_deref() {
            Some(lid) => engine.db().get_progression_ladder(lid)?,
            None => None,
        };
        let m_idx = args
            .get("major_stage_index")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(track.major_stage_index);
        let s_stg = args
            .get("sub_stage")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(track.sub_stage);
        let rule: crate::routines::StageTransitionRule = if let Some(inline) = args.get("rule") {
            serde_json::from_value(inline.clone())?
        } else {
            CampaignEngine::lookup_stage_rule(&track, ladder.as_ref(), m_idx, s_stg)
                .ok_or_else(|| anyhow!("No StageTransitionRule configured for stage {m_idx}:{s_stg} on track '{tid}'"))?
        };
        return engine.apply_stage_transition_rule(
            &cid,
            &track.name,
            &track.format_stage(),
            &rule,
            None,
        );
    }
    let normalized = match op.as_str() {
        "get_tracks" | "list_tracks" => "get_character_progression",
        other => other,
    };
    if let Some(o) = args.as_object_mut() {
        o.insert("action".to_string(), Value::String(normalized.to_string()));
    }
    execute_tool_with_policy(engine, "manage_progression", args, allow_local_file_import)
}

fn execute_domain_world(
    engine: &CampaignEngine,
    args: Value,
    allow_local_file_import: bool,
) -> Result<Value> {
    let op = extract_op(&args, "advance_time");
    match op.as_str() {
        "advance_time" | "time" => {
            execute_tool_with_policy(engine, "advance_time", args, allow_local_file_import)
        }
        "advance_world" | "tick_world" => {
            execute_tool_with_policy(engine, "advance_world", args, allow_local_file_import)
        }
        "create_place" => {
            execute_tool_with_policy(engine, "create_place", args, allow_local_file_import)
        }
        "get_place" => {
            execute_tool_with_policy(engine, "get_place", args, allow_local_file_import)
        }
        "update_place" => {
            let mapped = with_sub_action_as_action(args, None);
            execute_tool_with_policy(engine, "update_place", mapped, allow_local_file_import)
        }
        "weather" | "manage_weather" => {
            let mapped = with_sub_action_as_action(args, Some("get"));
            execute_tool_with_policy(engine, "manage_weather", mapped, allow_local_file_import)
        }
        "clock" | "manage_clock" => {
            let mapped = with_sub_action_as_action(args, Some("list"));
            execute_tool_with_policy(engine, "manage_clock", mapped, allow_local_file_import)
        }
        "travel" | "manage_travel_and_exploration" => {
            let mapped = with_sub_action_as_action(args, Some("calculate_route"));
            execute_tool_with_policy(
                engine,
                "manage_travel_and_exploration",
                mapped,
                allow_local_file_import,
            )
        }
        "upsert_route"
        | "calculate_route"
        | "advance_travel"
        | "explore_area"
        | "update_spatial"
        | "upsert_grid"
        | "list_grids"
        | "get_grid"
        | "set_cell"
        | "clear_cell"
        | "auto_layout_grid" => {
            let mapped = with_sub_action_as_action(args, Some(&op));
            execute_tool_with_policy(
                engine,
                "manage_travel_and_exploration",
                mapped,
                allow_local_file_import,
            )
        }
        other => Err(anyhow!(
            "Unknown world op '{other}'. Expected advance_time, advance_world, create_place, get_place, update_place, weather, clock, or travel."
        )),
    }
}

fn execute_domain_society(
    engine: &CampaignEngine,
    args: Value,
    allow_local_file_import: bool,
) -> Result<Value> {
    let op = extract_op(&args, "relation");
    match op.as_str() {
        "relation" | "upsert_relation" => {
            execute_tool_with_policy(engine, "upsert_relation", args, allow_local_file_import)
        }
        "remove_relation" => {
            execute_tool_with_policy(engine, "remove_relation", args, allow_local_file_import)
        }
        "record_knowledge" => {
            execute_tool_with_policy(engine, "record_knowledge", args, allow_local_file_import)
        }
        "propagate_knowledge" => {
            execute_tool_with_policy(engine, "propagate_knowledge", args, allow_local_file_import)
        }
        "faction" | "manage_faction" => {
            let mapped = with_sub_action_as_action(args, Some("list"));
            execute_tool_with_policy(engine, "manage_faction", mapped, allow_local_file_import)
        }
        "law" | "manage_law_and_reputation" => {
            let mapped = with_sub_action_as_action(args, Some("get_reputation"));
            execute_tool_with_policy(
                engine,
                "manage_law_and_reputation",
                mapped,
                allow_local_file_import,
            )
        }
        "upsert_jurisdiction"
        | "check_jurisdiction"
        | "record_crime"
        | "update_warrant"
        | "modify_reputation"
        | "get_reputation" => {
            let mapped = with_sub_action_as_action(args, Some(&op));
            execute_tool_with_policy(
                engine,
                "manage_law_and_reputation",
                mapped,
                allow_local_file_import,
            )
        }
        "autonomy" | "simulate_autonomy" => {
            let mapped = with_sub_action_as_action(args, Some("list_movements"));
            execute_tool_with_policy(engine, "simulate_autonomy", mapped, allow_local_file_import)
        }
        "prepare"
        | "commit"
        | "auto_mechanical"
        | "configure_mobility"
        | "dispatch_trip"
        | "tick_movements"
        | "list_movements" => {
            let mapped = with_sub_action_as_action(args, Some(&op));
            execute_tool_with_policy(engine, "simulate_autonomy", mapped, allow_local_file_import)
        }
        other => Err(anyhow!(
            "Unknown society op '{other}'. Expected relation, remove_relation, record_knowledge, propagate_knowledge, faction, law, or autonomy."
        )),
    }
}

fn execute_domain_codex(
    engine: &CampaignEngine,
    args: Value,
    allow_local_file_import: bool,
) -> Result<Value> {
    let op = extract_op(&args, "upsert_lore");
    match op.as_str() {
        "lore" | "upsert_lore" => {
            execute_tool_with_policy(engine, "upsert_lore", args, allow_local_file_import)
        }
        "text" | "upsert_text" => {
            execute_tool_with_policy(engine, "upsert_text", args, allow_local_file_import)
        }
        "record_reading" | "record_text_reading" => {
            execute_tool_with_policy(engine, "record_text_reading", args, allow_local_file_import)
        }
        "history" | "add_history" | "add_entity_history" => {
            execute_tool_with_policy(engine, "add_entity_history", args, allow_local_file_import)
        }
        "import_attachment" | "attach" => {
            execute_tool_with_policy(engine, "import_attachment", args, allow_local_file_import)
        }
        "create_reference_doc" | "reference_doc" => {
            execute_tool_with_policy(engine, "create_reference_doc", args, allow_local_file_import)
        }
        "log_event" | "log" => {
            execute_tool_with_policy(engine, "log_event", args, allow_local_file_import)
        }
        other => Err(anyhow!(
            "Unknown codex op '{other}'. Expected upsert_lore, upsert_text, record_text_reading, add_history, import_attachment, create_reference_doc, or log_event."
        )),
    }
}

fn execute_domain_economy(
    engine: &CampaignEngine,
    args: Value,
    allow_local_file_import: bool,
) -> Result<Value> {
    let op = extract_op(&args, "get_balances");
    match op.as_str() {
        "upsert_item" | "catalog_item" => {
            execute_tool_with_policy(engine, "upsert_item", args, allow_local_file_import)
        }
        "tag_item" | "tag_catalog_item" => {
            execute_tool_with_policy(engine, "tag_catalog_item", args, allow_local_file_import)
        }
        "transfer_catalog_item" => {
            execute_tool_with_policy(engine, "transfer_catalog_item", args, allow_local_file_import)
        }
        "claim_discovery" | "claim" => {
            execute_tool_with_policy(engine, "claim_discovery", args, allow_local_file_import)
        }
        "encumbrance" | "check_encumbrance" => {
            let mapped = with_sub_action_as_action(args, Some("check_encumbrance"));
            execute_tool_with_policy(engine, "manage_economy", mapped, allow_local_file_import)
        }
        "transaction" | "execute_transaction" => {
            let mapped = with_sub_action_as_action(args, Some("execute_transaction"));
            execute_tool_with_policy(engine, "manage_economy", mapped, allow_local_file_import)
        }
        "set_balance"
        | "get_balances"
        | "transfer_currency"
        | "upsert_market"
        | "get_market_prices"
        | "update_market_supply"
        | "set_item_economics" => {
            let mapped = with_sub_action_as_action(args, Some(&op));
            execute_tool_with_policy(engine, "manage_economy", mapped, allow_local_file_import)
        }
        other => Err(anyhow!(
            "Unknown economy op '{other}'. Expected upsert_item, transfer_catalog_item, claim_discovery, check_encumbrance, set_balance, get_balances, transfer_currency, execute_transaction, upsert_market, get_market_prices, update_market_supply, or set_item_economics."
        )),
    }
}

fn execute_domain_admin(
    engine: &CampaignEngine,
    args: Value,
    allow_local_file_import: bool,
) -> Result<Value> {
    let op = extract_op(&args, "diagnose");
    let target = match op.as_str() {
        "diagnose" | "diagnose_campaign" => "diagnose_campaign",
        "cleanup" | "cleanup_campaign" => "cleanup_campaign",
        "backup" | "backup_campaign" => "backup_campaign",
        "restore" | "restore_backup" => "restore_backup",
        "rollback" | "rollback_to_event" => "rollback_to_event",
        "compile" | "compile_docs" => "compile_docs",
        "update_meta" | "update_campaign_meta" => "update_campaign_meta",
        "reclassify" | "reclassify_entity" => "reclassify_entity",
        "rename_or_merge" | "rename_or_merge_entity" => "rename_or_merge_entity",
        "delete" | "delete_entity" => "delete_entity",
        "update_character_baseline" => "update_character_baseline",
        "update_place_baseline" => "update_place_baseline",
        "update_entity_history" => "update_entity_history",
        "update_attachment" => "update_attachment",
        other => {
            return Err(anyhow!(
                "Unknown admin op '{other}'. Expected diagnose, cleanup, backup, restore_backup, rollback, compile_docs, update_meta, reclassify, rename_or_merge, delete_entity, update_character_baseline, update_place_baseline, update_entity_history, or update_attachment."
            ));
        }
    };
    execute_tool_with_policy(engine, target, args, allow_local_file_import)
}

fn unified_domain_only_definitions() -> Vec<Value> {
    let arr = json!([
        {
            "name": "scene",
            "description": "Active campaign & scene context hub: read global status, assemble perspective-safe scene context (Markdown with top-banner `## Engine Alarms & Active Directives`), inspect/acknowledge/resolve active Engine Signals, or start a new session.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["context", "status", "signals", "ack_signal", "resolve_signal", "emit_signal", "start_session"], "default": "context" },
                    "place_id": { "type": "string" },
                    "character_ids": { "type": "array", "items": { "type": "string" } },
                    "scene_text": { "type": "string", "description": "Latest player input or scene text to trigger Lorebook, Books/Scrolls, and FTS5 recall." },
                    "perspective": { "type": "string", "description": "'gm' (default), 'player', or '<character_id>' (strict epistemic wall)." },
                    "include_gm_secrets": { "type": "boolean", "default": true },
                    "format": { "type": "string", "enum": ["markdown", "json"], "default": "markdown" },
                    "max_chars": { "type": "integer" },
                    "signal_id": { "type": "string", "description": "Signal id or dedup_key for ack_signal / resolve_signal." },
                    "title": { "type": "string" },
                    "summary": { "type": "string" }
                }
            }
        },
        {
            "name": "query",
            "description": "Unified read-only retrieval, verbatim file chunk reader, and multi-dimensional 'Search By' engine across Characters, Places, Lore, Books/Scrolls, Carryable Items, Histories, Attachments, Events, Relations, and Epistemic Knowledge.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": [
                            "search_all", "get_character", "get_place", "get_text", "get_item", "get_lore",
                            "get_attachment", "read_file", "search_characters", "search_places", "search_lore",
                            "search_texts", "search_items", "search_inventory", "search_histories", "search_attachments",
                            "search_events", "search_relations", "search_knowledge"
                        ],
                        "default": "search_all"
                    },
                    "query": { "type": "string" },
                    "character_id": { "type": "string" },
                    "place_id": { "type": "string" },
                    "text_id": { "type": "string" },
                    "item_id": { "type": "string" },
                    "lore_id": { "type": "string" },
                    "attachment_id": { "type": "string" },
                    "target": { "type": "string", "description": "For read_file: e.g. 'attachment:<id>', 'text:<id>', 'lore:<id>', 'doc:characters/<id>.md'." },
                    "offset_chars": { "type": "integer" },
                    "max_chars": { "type": "integer" },
                    "actor_id": { "type": "string" },
                    "not_known_by": { "type": "string", "description": "For search_knowledge: blind-spot finder!" },
                    "subject": { "type": "string" },
                    "entity_id": { "type": "string" },
                    "entity_kind": { "type": "string" },
                    "location_id": { "type": "string" },
                    "faction": { "type": "string" },
                    "tag": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Multi-tag filter for search_inventory / search_items (e.g. ['clothing', 'spare'])." },
                    "tag_match": { "type": "string", "enum": ["all", "any"], "default": "all" },
                    "exclude_tags": { "type": "array", "items": { "type": "string" } },
                    "equipped": { "type": "boolean" },
                    "as_of_seq": { "type": "integer" },
                    "limit": { "type": "integer" }
                }
            }
        },
        {
            "name": "resolve",
            "description": "Deterministic RPG resolution engine: structured action checks (`op='check'`, auto-hydrating stats, skills, gear, garment fit, titles, traits, proficiencies, injuries, encumbrance & weather, plus automatic Skill -> Stat Training + Job XP trickle), opposed contests (`op='contest'`), weighted/deck random tables (`op='draw_table'`, staging loot receipts), encounters (`op='encounter'`), and ruleset profiles.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["check", "contest", "draw_table", "roll_dice", "ruleset", "random_table", "encounter"], "default": "check" },
                    "sub_action": { "type": "string", "description": "For op='encounter': start, log_action, next_turn, update_participant, end, get, list." },
                    "actor_id": { "type": "string" },
                    "action": { "type": "string", "description": "What is being attempted (e.g. 'Cut mortise joint', 'Slip past watchmen')." },
                    "stat": { "type": "string" },
                    "skill": { "type": "string" },
                    "technique": { "type": "string" },
                    "progression_track": { "type": "string" },
                    "equipment": { "type": "array", "items": { "type": "string" } },
                    "dc_tier": { "type": "string", "enum": ["trivial", "easy", "moderate", "hard", "very_hard", "nearly_impossible"] },
                    "dc_factors": { "type": "array", "description": "Situational DC modifiers (e.g. 'rushed', 'proper_tools', 'darkness', or {label, delta, reason})." },
                    "difficulty": { "type": "integer" },
                    "modifier": { "type": "integer" },
                    "advantage": { "type": "string", "enum": ["none", "advantage", "disadvantage"] },
                    "stakes": { "type": "string" },
                    "dry_run": { "type": "boolean" },
                    "participants": { "type": "array", "items": { "type": "object" } },
                    "table_id": { "type": "string" },
                    "expression": { "type": "string" },
                    "preset": { "type": "string" },
                    "seed": { "type": "integer" }
                }
            }
        },
        {
            "name": "character",
            "description": "Unified character lifecycle, vitals/stats/conditions, sized apparel & equipment, persistent injuries/recovery/crafting, standing daily/weekly `routine` habits, and dedicated `pc_sheet` (avatar baseline vs. current measurements, outfits, titles, segregated Stat/Job/Proficiency XP, traits).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": [
                            "get", "create", "move", "resource", "stat", "status", "condition", "equipment",
                            "transfer_item", "skill", "technique", "goal", "rest", "profile", "pc_sheet",
                            "health", "routine"
                        ]
                    },
                    "sub_action": { "type": "string", "description": "Sub-operation for condition (add/remove), equipment (acquire/remove/equip/unequip/tailor/tag/auto_tag/search), pc_sheet (get_sheet/update_avatar/adjust_stat_xp/upsert_job/adjust_job_xp/upsert_proficiency/upsert_title/upsert_trait/upsert_outfit), health (apply_injury/treat_injury/advance_recovery/resolve_downtime/begin_crafting/resolve_crafting), or routine (list/upsert/delete)." },
                    "character_id": { "type": "string" },
                    "name": { "type": "string" },
                    "to_place_id": { "type": "string" },
                    "resource": { "type": "string" },
                    "stat": { "type": "string" },
                    "delta": { "type": "integer" },
                    "set_value": { "type": "integer" },
                    "set_current": { "type": "integer" },
                    "set_max": { "type": "integer" },
                    "item_name": { "type": "string" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Item tags (e.g. ['clothing', 'workwear', 'tool', 'cultivation', 'document'])." },
                    "add_tags": { "type": "array", "items": { "type": "string" } },
                    "remove_tags": { "type": "array", "items": { "type": "string" } },
                    "set_tags": { "type": "array", "items": { "type": "string" } },
                    "garment_measurements": { "type": "object", "additionalProperties": { "type": "string" } },
                    "garment_cut": { "type": "string" },
                    "garment_taper": { "type": "string" },
                    "measurements": { "type": "object", "additionalProperties": { "type": "string" } },
                    "baseline_measurements": { "type": "object", "additionalProperties": { "type": "string" } },
                    "routine": { "type": "object", "description": "For op='routine', sub_action='upsert': { name, category, cadence_hours, require_location_id?, halt_on_injury_severity?, progression_xp?, skill_xp?, job_xp?, stat_training_xp?, currency_deltas?, resource_deltas? }." },
                    "reason": { "type": "string" }
                },
                "required": ["op"]
            }
        },
        {
            "name": "progression",
            "description": "Genre-agnostic stateful progression & cultivation engine: define ladders (`xianxia_standard`, `body_tempering`, `dnd_level_20`, `scifi_tier`), enroll parallel tracks with declarative `stage_rules` (auto-applying measurements, stats, conditions, traits, checkpoints & emitting CRITICAL signals on layer/stage advance!), award/revert XP, and resolve bottleneck breakthroughs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": [
                            "get_tracks", "define_ladder", "list_ladders", "enroll_track", "advance_xp",
                            "adjust_xp", "revert_xp", "reset_xp", "attempt_breakthrough", "update_track",
                            "remove_track", "apply_stage_rule"
                        ]
                    },
                    "character_id": { "type": "string" },
                    "track_id": { "type": "string" },
                    "ladder_id": { "type": "string" },
                    "preset": { "type": "string", "enum": ["xianxia_standard", "body_tempering", "dnd_level_20", "scifi_tier"] },
                    "name": { "type": "string" },
                    "xp": { "type": "integer" },
                    "stage_rules": { "type": "object", "description": "Map of '<sub_stage>' or '<major_idx>:<sub_stage>' -> StageTransitionRule ({ appearance_stage?, height?, weight?, measurements?, facial_hair?, stat_set?, stat_deltas?, unspent_stat_points_delta?, upsert_conditions?, remove_conditions?, grant_traits?, checkpoint_summary?, narrative_directive? })." },
                    "dc_tier": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["op"]
            }
        },
        {
            "name": "world",
            "description": "World calendar, bounded-scope `advance_world` orchestrator, places/landmarks/exits, seasonal Markov weather, auto-ticking `clock` trackers (`cadence_hours`, `consequence_on_fill`, `auto_reset`), Dijkstra overland travel routing, and sparse 2D `(x, y)` map grids (terrain/landmark/fog painting via `sub_action='set_cell'`, single `x,y` or batch `cells[]`).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": [
                            "advance_time", "advance_world", "create_place", "get_place", "update_place",
                            "weather", "clock", "travel"
                        ]
                    },
                    "sub_action": { "type": "string", "description": "For update_place (weather/add_condition/remove_condition/landmark/route/control), clock (create/tick/update/list), weather (generate/advance/get), or travel (upsert_route/list_routes/update_spatial/calculate/advance/explore/upsert_grid/list_grids/get_grid/set_cell/clear_cell/auto_layout_grid)." },
                    "minutes": { "type": "integer" },
                    "duration_hours": { "type": "integer" },
                    "place_id": { "type": "string" },
                    "name": { "type": "string" },
                    "clock_id": { "type": "string" },
                    "segments": { "type": "integer" },
                    "delta": { "type": "integer" },
                    "cadence_hours": { "type": "integer", "description": "For clock create: auto-ticks +1 segment every `cadence_hours` on advance_time!" },
                    "consequence_on_fill": { "type": "string" },
                    "auto_reset": { "type": "boolean" },
                    "from_place_id": { "type": "string" },
                    "to_place_id": { "type": "string" },
                    "grid_id": { "type": "string", "description": "2D map grid ID for upsert_grid/get_grid/set_cell/clear_cell." },
                    "scale_kind": { "type": "string", "enum": ["world", "region", "settlement", "dungeon", "space_sector", "tactical"] },
                    "width": { "type": "integer" },
                    "height": { "type": "integer" },
                    "cell_scale_label": { "type": "string", "description": "e.g. '5 km', '80 m', '1 AU'." },
                    "default_terrain": { "type": "string" },
                    "x": { "type": "integer", "description": "Grid column for set_cell / clear_cell." },
                    "y": { "type": "integer", "description": "Grid row for set_cell / clear_cell." },
                    "terrain": { "type": "string", "description": "Cell terrain (e.g. forest, mountain, plains, river, road, swamp, urban, ruins, wilderness)." },
                    "label": { "type": "string" },
                    "landmark_name": { "type": "string" },
                    "fog_status": { "type": "string", "enum": ["unexplored", "rumored", "surveyed", "charted"] },
                    "passable": { "type": "boolean" },
                    "hazards": { "type": "array", "items": { "type": "string" } },
                    "cells": { "type": "array", "items": { "type": "object" }, "description": "Batch cells for set_cell: [{x, y, terrain, label?, place_id?, landmark_name?, faction_control?, fog_status?, passable?, hazards?, notes?, gm_notes?}]." },
                    "center_x": { "type": "integer" },
                    "center_y": { "type": "integer" },
                    "radius": { "type": "integer" },
                    "notes": { "type": "string" },
                    "gm_notes": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["op"]
            }
        },
        {
            "name": "society",
            "description": "Social graph, dedicated Factions, Epistemic Knowledge & witness beliefs (`record_knowledge` / `propagate_knowledge`), Jurisdictions/Crimes/Warrants/Reputation, and off-screen NPC Autonomy & Business-Circuit Mobility.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": [
                            "relation", "remove_relation", "record_knowledge", "propagate_knowledge",
                            "faction", "law", "autonomy"
                        ]
                    },
                    "sub_action": { "type": "string", "description": "For faction (upsert/get/list/modify_resource), law (upsert_jurisdiction/record_crime/update_warrant/modify_reputation/get_reputation), or autonomy (prepare/commit/auto_mechanical/configure_mobility/dispatch_trip/tick_movements/list_movements)." },
                    "source_id": { "type": "string" },
                    "relation_type": { "type": "string" },
                    "target_id": { "type": "string" },
                    "strength": { "type": "integer" },
                    "dimensions": { "type": "object" },
                    "subject": { "type": "string" },
                    "claim": { "type": "string" },
                    "beliefs": { "type": "array", "items": { "type": "object" } },
                    "claim_id": { "type": "string" },
                    "to_actor_ids": { "type": "array", "items": { "type": "string" } },
                    "notes": { "type": "string" }
                },
                "required": ["op"]
            }
        },
        {
            "name": "codex",
            "description": "World-building Lorebook (`upsert_lore`), physical readable Books/Scrolls/Manuals (`upsert_text`, `record_text_reading`), Entity Historical Chronicles (`add_history`), Attached/Pasted Reference Docs (`import_attachment`, `create_reference_doc`), and standalone Narrative Events (`log_event`).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": [
                            "upsert_lore", "upsert_text", "record_text_reading", "add_history",
                            "import_attachment", "create_reference_doc", "log_event"
                        ]
                    },
                    "id": { "type": "string" },
                    "title": { "type": "string" },
                    "category": { "type": "string" },
                    "summary": { "type": "string" },
                    "body": { "type": "string" },
                    "content": { "type": "string" },
                    "text_id": { "type": "string" },
                    "reader_id": { "type": "string" },
                    "read_status": { "type": "string" },
                    "entity_id": { "type": "string" },
                    "entity_kind": { "type": "string" },
                    "details": { "type": "string" },
                    "visibility": { "type": "string" },
                    "always_include": { "type": "boolean", "description": "For upsert_lore: true loads this rule/protocol into every GM scene brief." },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "keywords": { "type": "array", "items": { "type": "string" } },
                    "related_ids": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["op"]
            }
        },
        {
            "name": "economy",
            "description": "Carryable catalog items (`upsert_item` with provenance & garment sizing, `transfer_catalog_item`), staged loot claiming (`claim_discovery`), Encumbrance, Atomic Multi-Denomination Currency (auto-making change within the same currency family!), and Dynamic Settlement Markets with broken-route price ripples.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": [
                            "get_balances", "set_balance", "transfer_currency", "execute_transaction",
                            "check_encumbrance", "upsert_market", "get_market_prices", "update_market_supply",
                            "set_item_economics", "upsert_item", "tag_item", "transfer_catalog_item", "claim_discovery"
                        ]
                    },
                    "owner_id": { "type": "string" },
                    "buyer_id": { "type": "string" },
                    "seller_id": { "type": "string" },
                    "from_id": { "type": "string" },
                    "to_id": { "type": "string" },
                    "amount": { "type": "integer" },
                    "currency": { "type": "string" },
                    "character_id": { "type": "string" },
                    "place_id": { "type": "string" },
                    "item_id": { "type": "string" },
                    "discovery_id": { "type": "string" },
                    "name": { "type": "string" },
                    "garment_measurements": { "type": "object", "additionalProperties": { "type": "string" } },
                    "garment_cut": { "type": "string" },
                    "garment_taper": { "type": "string" },
                    "notes": { "type": "string" }
                },
                "required": ["op"]
            }
        },
        {
            "name": "admin",
            "description": "Campaign diagnostics, backups, hygiene & structural refactoring (`diagnose`, `cleanup`, `backup`, `restore_backup`, `rollback`, `compile_docs`, `update_meta`, `reclassify`, `rename_or_merge`, `delete_entity`, `update_character_baseline`, `update_place_baseline`, `update_entity_history`, `update_attachment`).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": {
                        "type": "string",
                        "enum": [
                            "diagnose", "cleanup", "backup", "restore_backup", "rollback", "compile_docs",
                            "update_meta", "reclassify", "rename_or_merge", "delete_entity",
                            "update_character_baseline", "update_place_baseline", "update_entity_history",
                            "update_attachment"
                        ]
                    },
                    "dry_run": { "type": "boolean" },
                    "to_seq": { "type": "integer" },
                    "backup_filename": { "type": "string" },
                    "register": { "type": "string" },
                    "entity_id": { "type": "string" },
                    "old_id": { "type": "string" },
                    "new_id": { "type": "string" },
                    "name": { "type": "string", "description": "For update_meta: campaign name." },
                    "description": { "type": "string", "description": "For update_meta: campaign description." },
                    "ruleset_notes": { "type": "string", "description": "For update_meta: replace campaign ruleset/protocol notes." },
                    "append_ruleset_notes": { "type": "string", "description": "For update_meta: append a standing rule/protocol to ruleset_notes without overwriting." },
                    "global_weather": { "type": "string" },
                    "reason": { "type": "string" }
                },
                "required": ["op"]
            }
        },
        {
            "name": "manage_signals",
            "description": "Manage active Chronicle Engine Signals & Alarms (list, acknowledge, resolve, emit, delete).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["list", "acknowledge", "resolve", "emit", "delete"], "default": "list" },
                    "signal_id": { "type": "string" },
                    "perspective": { "type": "string" },
                    "place_id": { "type": "string" },
                    "character_ids": { "type": "array", "items": { "type": "string" } }
                }
            }
        },
        {
            "name": "manage_routines",
            "description": "Manage standing recurring character habits, work shifts, training regimens, and living upkeep (`list`, `upsert`, `delete`).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "op": { "type": "string", "enum": ["list", "upsert", "delete"], "default": "list" },
                    "character_id": { "type": "string" },
                    "routine_id": { "type": "string" },
                    "routine": { "type": "object" }
                }
            }
        }
    ]);
    let mut list = arr.as_array().cloned().unwrap_or_default();
    for tool in &mut list {
        if let Some(schema) = tool.get_mut("inputSchema").and_then(|v| v.as_object_mut()) {
            schema
                .entry("additionalProperties".to_string())
                .or_insert(Value::Bool(true));
        }
    }
    list
}

fn unified_domain_tool_definitions(include_admin: bool) -> Vec<Value> {
    let mut out = Vec::new();
    let domain_list = unified_domain_only_definitions();
    for t in domain_list {
        let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if matches!(name, "manage_signals" | "manage_routines") {
            continue;
        }
        if name == "admin" && !include_admin {
            continue;
        }
        out.push(t);
    }
    // Include `commit_turn`, `search`, and `fetch` in every sanitized profile!
    for extra in extra_rpg_ai_tools() {
        let name = extra.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if matches!(name, "commit_turn" | "search" | "fetch") {
            out.push(extra);
        }
    }
    out
}
