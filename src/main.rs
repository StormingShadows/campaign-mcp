use anyhow::Result;
use chronicle::context::{render_scene_context_markdown, SceneContextRequest};
use chronicle::dice::roll_dice;
use chronicle::engine::{CampaignEngine, ImportAttachmentRequest};
use chronicle::maintenance::{cleanup_campaign, diagnose_campaign};
use chronicle::mcp::{export_tool_schemas, run_mcp_stdio};
use chronicle::search::{
    character_to_hit, place_to_hit, AttachmentSearchFilter, CharacterSearchFilter, EventSearchFilter,
    HistorySearchFilter, ItemSearchFilter, LoreSearchFilter, PlaceSearchFilter,
    RelationSearchFilter, TextSearchFilter,
};
use chronicle::web::{build_openapi_spec, run_web_server, ServerOptions};
use clap::{Parser, Subcommand};
use std::io::Read;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "chronicle",
    version,
    about = "Event-sourced RPG GM Campaign Engine (Web GM Studio GUI + MCP Server + REST/OpenAPI + CLI + SQLite Knowledge Graph + FTS5 Lorebook + Auto-Compiled Markdown Docs)"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize a new empty, fresh campaign directory (SQLite DB + compiled Markdown docs).
    Init {
        /// Campaign directory path (e.g., ./campaign)
        campaign_dir: PathBuf,
        /// Campaign name
        #[arg(long)]
        name: String,
        /// Short campaign description
        #[arg(long, default_value = "")]
        description: String,
        /// Optional ruleset / mechanics notes
        #[arg(long, default_value = "System-agnostic RPG core")]
        ruleset: String,
    },

    /// Launch the built-in Chronicle GM Studio Web GUI + REST / OpenAPI 3.1 + MCP Streamable HTTP (/mcp) server.
    Ui {
        /// Path to the campaign directory containing campaign.db
        #[arg(long, short = 'c')]
        campaign: PathBuf,
        /// Bind host address
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Bind port
        #[arg(long, short = 'p', default_value_t = 7878)]
        port: u16,
        /// Optional Bearer token to protect /mcp and /api/* (also reads CHRONICLE_MCP_TOKEN env var)
        #[arg(long)]
        bearer_token: Option<String>,
        /// Optional public base URL (e.g. https://your-tunnel.example.com) for OAuth metadata & ChatGPT search/fetch citation URLs
        #[arg(long)]
        public_url: Option<String>,
        /// Enable built-in OAuth 2.1 + PKCE + Dynamic Client Registration (for remote/Colab web-hosted MCP)
        #[arg(long)]
        oauth: bool,
        /// Passphrase required on `/oauth/authorize` consent page (also reads CHRONICLE_OAUTH_SECRET; auto-generated if omitted)
        #[arg(long)]
        oauth_secret: Option<String>,
        /// Allow HTTP clients to read local host files via `import_attachment(source_path)` (disabled by default for security)
        #[arg(long)]
        allow_local_file_import: bool,
    },

    /// Run the Model Context Protocol (MCP) server (stdio for Codex CLI/Antigravity/Claude/Cursor, or Streamable HTTP for ChatGPT Web/Desktop/Colab).
    Mcp {
        /// Path to the campaign directory containing campaign.db
        #[arg(long, short = 'c')]
        campaign: PathBuf,
        /// Transport mode: 'stdio' (default) or 'http' (Streamable HTTP on /mcp)
        #[arg(long, short = 't', default_value = "stdio")]
        transport: String,
        /// Bind host address when --transport http is used
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Bind port when --transport http is used
        #[arg(long, short = 'p', default_value_t = 7878)]
        port: u16,
        /// Optional static Bearer token when --transport http is used (also reads CHRONICLE_MCP_TOKEN env var)
        #[arg(long)]
        bearer_token: Option<String>,
        /// Optional public base URL for OAuth metadata & ChatGPT search/fetch citation URLs
        #[arg(long)]
        public_url: Option<String>,
        /// Enable built-in OAuth 2.1 + PKCE + Dynamic Client Registration (for remote/Colab web-hosted MCP)
        #[arg(long)]
        oauth: bool,
        /// Passphrase required on `/oauth/authorize` consent page (also reads CHRONICLE_OAUTH_SECRET; auto-generated if omitted)
        #[arg(long)]
        oauth_secret: Option<String>,
        /// Allow HTTP clients to read local host files via `import_attachment(source_path)` (disabled by default for security)
        #[arg(long)]
        allow_local_file_import: bool,
    },

    /// Multi-dimensional 'Search By' queries across Characters, Places, Lore, Texts, Attachments, Events, Relations, or Everything.
    Search {
        #[arg(long, short = 'c')]
        campaign: PathBuf,
        #[command(subcommand)]
        target: Box<SearchCommands>,
    },

    /// Copy an external document, map, PDF, portrait, or handout into `<campaign>/docs/attachments/files/` with a stable addressable reference.
    Attach {
        #[arg(long, short = 'c')]
        campaign: PathBuf,
        /// Local file path to copy into the campaign folder
        source_path: PathBuf,
        /// Display title for this reference document
        #[arg(long)]
        title: String,
        /// Category: map, handout, portrait, rules, world-doc, notes, audio, other
        #[arg(long)]
        category: Option<String>,
        /// Short summary of what the document contains and why it matters
        #[arg(long, default_value = "")]
        summary: String,
        /// Visibility: public, rumor, gm_secret
        #[arg(long, default_value = "public")]
        visibility: String,
        /// Related entity slugs (repeatable)
        #[arg(long = "related")]
        related_ids: Vec<String>,
        /// Trigger keywords (repeatable)
        #[arg(long = "keyword")]
        keywords: Vec<String>,
    },

    /// Create a new reference document file (or append to an existing one) in `<campaign>/docs/attachments/files/` from pasted-in text or stdin.
    CreateDoc(Box<CreateDocArgs>),

    /// Export Chronicle's complete AI tool definitions in MCP, OpenAI, Anthropic, or OpenAPI 3.1 schema format.
    ExportTools {
        /// Target format: mcp, openai, anthropic, or openapi
        #[arg(long, short = 'f', default_value = "openai")]
        format: String,
    },

    /// Re-compile all referencable Markdown documents from the SQLite event ledger.
    Compile {
        #[arg(long, short = 'c')]
        campaign: PathBuf,
    },

    /// Show campaign status, world clock, characters, places, clocks, and hash chain verification.
    Status {
        #[arg(long, short = 'c')]
        campaign: PathBuf,
    },

    /// Audit campaign health: detect mislabeled registers, broken references, duplicates, sync drift, and stale docs.
    Diagnose {
        #[arg(long, short = 'c')]
        campaign: PathBuf,
        /// Output raw JSON diagnostic report
        #[arg(long)]
        json: bool,
    },

    /// Run safe campaign hygiene (prune stale .md docs, sync carryable books/items, deduplicate tags/links). Dry-run by default unless --apply is passed.
    Cleanup {
        #[arg(long, short = 'c')]
        campaign: PathBuf,
        /// Apply cleanup changes (without --apply, runs as dry-run preview)
        #[arg(long)]
        apply: bool,
    },

    /// Assemble and print a GM Scene Context brief (compact Markdown or JSON).
    Context {
        #[arg(long, short = 'c')]
        campaign: PathBuf,
        #[arg(long)]
        place: Option<String>,
        #[arg(long = "char")]
        characters: Vec<String>,
        #[arg(long, default_value = "")]
        scene: String,
        /// Output format: markdown or json
        #[arg(long, default_value = "markdown")]
        format: String,
    },

    /// Inspect the immutable event sourcing ledger.
    Log {
        #[arg(long, short = 'c')]
        campaign: PathBuf,
        /// Filter events affecting a specific character, place, or lore slug
        #[arg(long)]
        entity: Option<String>,
        /// Maximum number of recent events to show
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },

    /// Quick full-text search across lorebook entries and campaign history via SQLite FTS5.
    Query {
        #[arg(long, short = 'c')]
        campaign: PathBuf,
        /// Search query text
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },

    /// Time-travel rollback: safely rewind campaign state to event sequence #seq (creates backup first).
    Rollback {
        #[arg(long, short = 'c')]
        campaign: PathBuf,
        #[arg(long)]
        to_seq: i64,
    },

    /// Roll RPG dice (e.g. "2d20kh1+5", "4d6kh3", "1d100").
    Roll {
        expression: String,
        #[arg(long)]
        seed: Option<u64>,
    },
}

#[derive(clap::Args, Debug)]
struct CreateDocArgs {
    #[arg(long, short = 'c')]
    campaign: PathBuf,
    /// Optional stable slug; auto-generated from title if omitted
    #[arg(long)]
    id: Option<String>,
    /// Display title for this reference document
    #[arg(long)]
    title: String,
    /// Optional filename with extension (defaults to '<id>.md')
    #[arg(long)]
    filename: Option<String>,
    /// Inline pasted text content (or pass --stdin to read from standard input)
    #[arg(long)]
    content: Option<String>,
    /// Read pasted reference content from stdin
    #[arg(long)]
    stdin: bool,
    /// Append content if the reference file already exists
    #[arg(long)]
    append: bool,
    /// Category: world-doc, rules, notes, handout, map, other
    #[arg(long)]
    category: Option<String>,
    /// Short summary of what the document contains
    #[arg(long, default_value = "")]
    summary: String,
    /// Visibility: public, rumor, gm_secret
    #[arg(long, default_value = "public")]
    visibility: String,
    /// Related entity slugs (repeatable)
    #[arg(long = "related")]
    related_ids: Vec<String>,
    /// Trigger keywords (repeatable)
    #[arg(long = "keyword")]
    keywords: Vec<String>,
}

#[derive(clap::Args, Debug)]
struct SearchItemsArgs {
    #[arg(long)]
    query: Option<String>,
    #[arg(long)]
    item_type: Option<String>,
    #[arg(long)]
    rarity: Option<String>,
    #[arg(long)]
    holder: Option<String>,
    #[arg(long)]
    previous_owner: Option<String>,
    #[arg(long)]
    condition: Option<String>,
    #[arg(long)]
    has_linked_text: Option<bool>,
    #[arg(long)]
    keyword: Option<String>,
    #[arg(long)]
    tag: Option<String>,
    #[arg(long)]
    related_to: Option<String>,
    #[arg(long, default_value_t = 25)]
    limit: usize,
}

#[derive(clap::Args, Debug)]
struct SearchHistoriesArgs {
    #[arg(long)]
    query: Option<String>,
    #[arg(long)]
    entity: Option<String>,
    #[arg(long)]
    kind: Option<String>,
    #[arg(long)]
    era: Option<String>,
    #[arg(long)]
    visibility: Option<String>,
    #[arg(long)]
    related_to: Option<String>,
    #[arg(long, default_value_t = 40)]
    limit: usize,
}

#[derive(clap::Args, Debug)]
struct SearchCharsArgs {
    #[arg(long)]
    query: Option<String>,
    #[arg(long)]
    kind: Option<String>,
    #[arg(long)]
    status: Option<String>,
    #[arg(long)]
    location: Option<String>,
    #[arg(long)]
    faction: Option<String>,
    #[arg(long)]
    has_item: Option<String>,
    #[arg(long)]
    equipped_only: bool,
    #[arg(long)]
    has_skill: Option<String>,
    #[arg(long)]
    has_technique: Option<String>,
    #[arg(long)]
    has_condition: Option<String>,
    #[arg(long)]
    tag: Option<String>,
    #[arg(long)]
    stat_name: Option<String>,
    #[arg(long)]
    stat_min: Option<i64>,
    #[arg(long)]
    resource_name: Option<String>,
    #[arg(long)]
    resource_below_pct: Option<u8>,
}

#[derive(clap::Args, Debug)]
struct SearchPlacesArgs {
    #[arg(long)]
    query: Option<String>,
    #[arg(long)]
    place_type: Option<String>,
    #[arg(long)]
    terrain: Option<String>,
    #[arg(long)]
    climate: Option<String>,
    #[arg(long)]
    season_effect: Option<String>,
    #[arg(long)]
    weather: Option<String>,
    #[arg(long)]
    faction: Option<String>,
    #[arg(long)]
    danger: Option<String>,
    #[arg(long)]
    has_landmark: Option<String>,
    #[arg(long)]
    landmark_status: Option<String>,
    #[arg(long)]
    has_condition: Option<String>,
    #[arg(long)]
    has_character: Option<String>,
    #[arg(long)]
    connected_to: Option<String>,
    #[arg(long)]
    tag: Option<String>,
}

#[derive(clap::Args, Debug)]
struct SearchLoreArgs {
    #[arg(long)]
    query: Option<String>,
    #[arg(long)]
    category: Option<String>,
    #[arg(long)]
    visibility: Option<String>,
    #[arg(long)]
    keyword: Option<String>,
    #[arg(long)]
    tag: Option<String>,
    #[arg(long)]
    related_to: Option<String>,
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

#[derive(clap::Args, Debug)]
struct SearchTextsArgs {
    #[arg(long)]
    query: Option<String>,
    #[arg(long)]
    medium: Option<String>,
    #[arg(long)]
    subject: Option<String>,
    #[arg(long)]
    author: Option<String>,
    #[arg(long)]
    language: Option<String>,
    #[arg(long)]
    condition: Option<String>,
    #[arg(long)]
    read_status: Option<String>,
    #[arg(long)]
    read_by: Option<String>,
    #[arg(long)]
    holder: Option<String>,
    #[arg(long)]
    keyword: Option<String>,
    #[arg(long)]
    tag: Option<String>,
    #[arg(long)]
    related_to: Option<String>,
    #[arg(long, default_value_t = 25)]
    limit: usize,
}

#[derive(clap::Args, Debug)]
struct SearchAttachmentsArgs {
    #[arg(long)]
    query: Option<String>,
    #[arg(long)]
    category: Option<String>,
    #[arg(long)]
    mime_type: Option<String>,
    #[arg(long)]
    visibility: Option<String>,
    #[arg(long)]
    keyword: Option<String>,
    #[arg(long)]
    tag: Option<String>,
    #[arg(long)]
    related_to: Option<String>,
    #[arg(long, default_value_t = 25)]
    limit: usize,
}

#[derive(clap::Args, Debug)]
struct SearchEventsArgs {
    #[arg(long)]
    query: Option<String>,
    #[arg(long)]
    entity: Option<String>,
    #[arg(long)]
    location: Option<String>,
    #[arg(long)]
    event_type: Option<String>,
    #[arg(long)]
    session: Option<u32>,
    #[arg(long)]
    world_time: Option<String>,
    #[arg(long)]
    seq_from: Option<i64>,
    #[arg(long)]
    seq_to: Option<i64>,
    #[arg(long, default_value_t = 30)]
    limit: usize,
}

#[derive(clap::Args, Debug)]
struct SearchRelationsArgs {
    #[arg(long)]
    entity: Option<String>,
    #[arg(long)]
    relation_type: Option<String>,
    #[arg(long)]
    min_strength: Option<i64>,
    #[arg(long)]
    max_strength: Option<i64>,
    #[arg(long)]
    query: Option<String>,
    #[arg(long, default_value_t = 1)]
    hops: u8,
}

#[derive(Subcommand, Debug)]
enum SearchCommands {
    /// Omni-search across Characters, Places, Lore, Texts, Attachments, Relations, Clocks, and Events.
    All {
        query: String,
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Search Characters by kind, status, location, faction, item, skill, technique, condition, stat, or resource %.
    Characters(Box<SearchCharsArgs>),
    /// Search Places by type, terrain, climate, seasonal effect, weather, faction, danger, landmark, condition, occupant, or route.
    Places(Box<SearchPlacesArgs>),
    /// Search World-Building & Lore by text, category, visibility, keyword, tag, or linked entity.
    Lore(Box<SearchLoreArgs>),
    /// Search Books, Scrolls, Grimoires & Ritual Texts by text, medium, subject, author, language, status, reader, or holder/location.
    Texts(Box<SearchTextsArgs>),
    /// Search Carryable Items, Artifacts, Weapons, Tools, Keepsakes & Books by text, type, holder, previous owner, condition, or linked text.
    Items(Box<SearchItemsArgs>),
    /// Search Recorded Historical Chronicles for places, establishments, sects, factions, items, books/manuals, and characters.
    Histories(Box<SearchHistoriesArgs>),
    /// Search Attached Reference Docs & Media copied into `docs/attachments/`.
    Attachments(Box<SearchAttachmentsArgs>),
    /// Search Historical Events by text, entity, location, event type, session, world time/season, or sequence range.
    Events(Box<SearchEventsArgs>),
    /// Search Knowledge Graph Relations by entity, relation type, disposition strength, or multi-hop depth (1-3).
    Relations(Box<SearchRelationsArgs>),
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init {
            campaign_dir,
            name,
            description,
            ruleset,
        } => {
            let engine = CampaignEngine::open(&campaign_dir)?;
            let meta = engine.init_campaign(&name, &description, &ruleset, None)?;
            println!(
                "Initialized campaign '{}' at {}",
                meta.name,
                campaign_dir.display()
            );
            println!("  SQLite DB : {}", engine.db().db_path().display());
            println!("  Docs Index: {}", engine.docs_dir().join("INDEX.md").display());
        }

        Commands::Ui {
            campaign,
            host,
            port,
            bearer_token,
            public_url,
            oauth,
            oauth_secret,
            allow_local_file_import,
        } => {
            let token = bearer_token.or_else(|| std::env::var("CHRONICLE_MCP_TOKEN").ok());
            let oa_secret = oauth_secret.or_else(|| std::env::var("CHRONICLE_OAUTH_SECRET").ok());
            let oauth_enabled = oauth || oa_secret.is_some();
            run_web_server(
                &campaign,
                ServerOptions {
                    host,
                    port,
                    bearer_token: token,
                    public_url,
                    oauth_enabled,
                    oauth_secret: oa_secret,
                    allow_local_file_import,
                },
            )?;
        }

        Commands::Mcp {
            campaign,
            transport,
            host,
            port,
            bearer_token,
            public_url,
            oauth,
            oauth_secret,
            allow_local_file_import,
        } => {
            if transport.eq_ignore_ascii_case("http")
                || transport.eq_ignore_ascii_case("streamable-http")
                || transport.eq_ignore_ascii_case("streamable_http")
            {
                let token = bearer_token.or_else(|| std::env::var("CHRONICLE_MCP_TOKEN").ok());
                let oa_secret =
                    oauth_secret.or_else(|| std::env::var("CHRONICLE_OAUTH_SECRET").ok());
                let oauth_enabled = oauth || oa_secret.is_some();
                run_web_server(
                    &campaign,
                    ServerOptions {
                        host,
                        port,
                        bearer_token: token,
                        public_url,
                        oauth_enabled,
                        oauth_secret: oa_secret,
                        allow_local_file_import,
                    },
                )?;
            } else {
                let engine = CampaignEngine::open(&campaign)?;
                run_mcp_stdio(&engine)?;
            }
        }

        Commands::Search { campaign, target } => handle_search(campaign, *target)?,

        Commands::Attach {
            campaign,
            source_path,
            title,
            category,
            summary,
            visibility,
            related_ids,
            keywords,
        } => {
            let engine = CampaignEngine::open(&campaign)?;
            let (att, abs, ev) = engine.import_attachment(ImportAttachmentRequest {
                title,
                source_path: Some(source_path.display().to_string()),
                category,
                summary,
                visibility: Some(visibility),
                related_ids,
                keywords,
                ..Default::default()
            })?;
            println!("Attached '{}' (event #{})", att.title, ev.seq);
            println!("  Reference ID : attachment:{}", att.id);
            println!("  MCP URI      : chronicle://attachments/{}", att.id);
            println!("  Relative Path: {}", att.relative_path);
            println!("  Copied File  : {}", abs.display());
            println!("  SHA-256      : {}", att.sha256);
        }

        Commands::CreateDoc(args) => {
            let a = *args;
            let content = if a.stdin {
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf)?;
                buf
            } else {
                a.content.unwrap_or_default()
            };
            let engine = CampaignEngine::open(&a.campaign)?;
            let (att, abs, ev) = engine.create_reference_doc(
                a.id,
                a.title,
                a.filename,
                content,
                a.append,
                a.category,
                a.summary,
                String::new(),
                Some(a.visibility),
                a.keywords,
                Vec::new(),
                a.related_ids,
                String::new(),
            )?;
            println!(
                "{} reference doc '{}' (event #{})",
                if a.append { "Appended" } else { "Created" },
                att.title,
                ev.seq
            );
            println!("  Reference ID : attachment:{}", att.id);
            println!("  MCP URI      : chronicle://attachments/{}", att.id);
            println!("  Relative Path: {}", att.relative_path);
            println!("  File Path    : {}", abs.display());
        }

        Commands::ExportTools { format } => {
            let val = if format.eq_ignore_ascii_case("openapi") {
                build_openapi_spec()
            } else {
                export_tool_schemas(&format)?
            };
            println!("{}", serde_json::to_string_pretty(&val)?);
        }

        Commands::Compile { campaign } => {
            let engine = CampaignEngine::open(&campaign)?;
            let report = engine.compile_docs()?;
            println!(
                "Compiled docs to {}: {} characters, {} places, {} items, {} lore entries, {} books/scrolls, {} attached docs, {} events.",
                report.docs_dir.display(),
                report.character_files,
                report.place_files,
                report.item_files,
                report.lore_files,
                report.text_files,
                report.attachment_files,
                report.total_events
            );
        }

        Commands::Status { campaign } => {
            let engine = CampaignEngine::open(&campaign)?;
            let meta = engine.db().get_campaign_meta()?;
            let chars = engine.list_characters()?;
            let places = engine.list_places()?;
            let items = engine.db().list_catalog_items()?;
            let histories = engine.db().list_all_histories()?;
            let lore = engine.db().list_lore(None)?;
            let texts = engine.db().list_texts()?;
            let attachments = engine.db().list_attachments()?;
            let clocks = engine.db().list_clocks()?;
            let (valid, count) = engine.db().verify_event_chain()?;

            println!("=== {} (Session {}) ===", meta.name, meta.current_session);
            println!(
                "Clock   : {} ({}) | Global Weather: {}",
                meta.clock.format_short(),
                meta.clock.time_of_day(),
                meta.clock.global_weather
            );
            println!(
                "Ledger  : {} events | SHA-256 Chain: {}",
                count,
                if valid { "VERIFIED" } else { "CORRUPTED" }
            );
            println!(
                "Entities: {} characters, {} places, {} items, {} histories, {} lore entries, {} books/scrolls, {} attached docs, {} clocks",
                chars.len(),
                places.len(),
                items.len(),
                histories.len(),
                lore.len(),
                texts.len(),
                attachments.len(),
                clocks.len()
            );
            if !chars.is_empty() {
                println!("\nCharacters:");
                for c in chars {
                    println!(
                        "  - {} [{}] ({}) @ {}",
                        c.baseline.name,
                        c.baseline.id,
                        c.current.alive_status,
                        c.current.current_location.as_deref().unwrap_or("unknown")
                    );
                }
            }
            if !places.is_empty() {
                println!("\nPlaces:");
                for p in places {
                    println!(
                        "  - {} [{}] ({}) | Weather: {} | Present: {:?}",
                        p.baseline.name,
                        p.baseline.id,
                        p.baseline.terrain,
                        p.current.current_weather,
                        p.current.present_characters
                    );
                }
            }
            if !clocks.is_empty() {
                println!("\nClocks:");
                for cl in clocks {
                    println!(
                        "  - {} [{}] {}/{} ({})",
                        cl.name, cl.id, cl.filled, cl.segments, cl.status
                    );
                }
            }
        }

        Commands::Diagnose { campaign, json } => {
            let engine = CampaignEngine::open(&campaign)?;
            let report = diagnose_campaign(&engine)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "=== Campaign Diagnostics: {} ===",
                    if report.healthy { "HEALTHY" } else { "ISSUES FOUND" }
                );
                println!(
                    "Errors: {} | Warnings: {} | Info: {} | SHA-256 Chain: {}",
                    report.error_count,
                    report.warning_count,
                    report.info_count,
                    if report.ledger_hash_chain_valid {
                        "VERIFIED"
                    } else {
                        "CORRUPTED"
                    }
                );
                if report.findings.is_empty() {
                    println!("No issues detected.");
                } else {
                    for f in &report.findings {
                        println!(
                            "  [{}] {} ({}:{}) — {}",
                            f.severity.to_uppercase(),
                            f.code,
                            f.register,
                            f.entity_id,
                            f.message
                        );
                        if let Some(ref tool) = f.suggested_tool {
                            println!("      Suggested fix: {tool}");
                        }
                    }
                }
            }
        }

        Commands::Cleanup { campaign, apply } => {
            let engine = CampaignEngine::open(&campaign)?;
            let report = cleanup_campaign(&engine, !apply)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }

        Commands::Context {
            campaign,
            place,
            characters,
            scene,
            format,
        } => {
            let engine = CampaignEngine::open(&campaign)?;
            let req = SceneContextRequest {
                place_id: place,
                character_ids: characters,
                scene_text: scene,
                recent_events_limit: 8,
                include_gm_secrets: true,
                perspective: None,
            };
            let packet = engine.get_scene_context(&req)?;
            if format.eq_ignore_ascii_case("markdown") || format.eq_ignore_ascii_case("md") {
                println!("{}", render_scene_context_markdown(&packet, None));
            } else {
                println!("{}", serde_json::to_string_pretty(&packet)?);
            }
        }

        Commands::Log {
            campaign,
            entity,
            limit,
        } => {
            let engine = CampaignEngine::open(&campaign)?;
            let events = if let Some(eid) = entity {
                let mut evs = engine.db().list_events_for_entity(&eid)?;
                if evs.len() > limit {
                    evs = evs.split_off(evs.len() - limit);
                }
                evs
            } else {
                engine.db().list_recent_events(limit)?
            };
            for ev in events {
                let loc = ev.location_id.as_deref().unwrap_or("-");
                println!(
                    "[#{:03} | S{} | {} | @{}] {}",
                    ev.seq, ev.session_number, ev.world_time, loc, ev.summary
                );
            }
        }

        Commands::Query {
            campaign,
            query,
            limit,
        } => {
            let engine = CampaignEngine::open(&campaign)?;
            let lore_hits = engine.search_lore(&query, limit)?;
            let event_hits = engine.db().search_events_fts(&query, limit)?;

            println!("=== Lore Matches ({}) ===", lore_hits.len());
            for l in lore_hits {
                println!(
                    "  - [{}] {} ({}) — {}",
                    l.category, l.title, l.id, l.summary
                );
            }

            println!("\n=== Historical Event Matches ({}) ===", event_hits.len());
            for ev in event_hits {
                println!("  - [#{} | {}] {}", ev.seq, ev.world_time, ev.summary);
            }
        }

        Commands::Rollback { campaign, to_seq } => {
            let engine = CampaignEngine::open(&campaign)?;
            let (backup, report) = engine.rollback_to_seq(to_seq)?;
            println!("Rolled back campaign to event #{to_seq}.");
            println!("  Safety backup saved to: {}", backup.display());
            println!(
                "  Recompiled {} character docs, {} place docs, {} lore docs ({} remaining events).",
                report.character_files,
                report.place_files,
                report.lore_files,
                report.total_events
            );
        }

        Commands::Roll { expression, seed } => {
            let res = roll_dice(&expression, seed)?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
    }

    Ok(())
}

#[inline(never)]
fn handle_search(campaign: PathBuf, target: SearchCommands) -> Result<()> {
    let engine = CampaignEngine::open(&campaign)?;
    match target {
        SearchCommands::All { query, limit } => {
            let res = engine.search_omni(&query, limit)?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        SearchCommands::Characters(a) => {
            let a = *a;
            let hits: Vec<_> = engine
                .search_characters_by(&CharacterSearchFilter {
                    query: a.query,
                    kind: a.kind,
                    alive_status: a.status,
                    location_id: a.location,
                    faction: a.faction,
                    has_item: a.has_item,
                    equipped_only: if a.equipped_only { Some(true) } else { None },
                    has_skill: a.has_skill,
                    has_technique: a.has_technique,
                    has_condition: a.has_condition,
                    tag: a.tag,
                    stat_name: a.stat_name,
                    stat_min: a.stat_min,
                    resource_name: a.resource_name,
                    resource_below_pct: a.resource_below_pct,
                })?
                .into_iter()
                .map(character_to_hit)
                .collect();
            println!("{}", serde_json::to_string_pretty(&hits)?);
        }
        SearchCommands::Places(a) => {
            let a = *a;
            let hits: Vec<_> = engine
                .search_places_by(&PlaceSearchFilter {
                    query: a.query,
                    place_type: a.place_type,
                    parent_place: None,
                    terrain: a.terrain,
                    climate: a.climate,
                    season_effect: a.season_effect,
                    weather: a.weather,
                    faction: a.faction,
                    danger_level: a.danger,
                    has_landmark: a.has_landmark,
                    landmark_status: a.landmark_status,
                    has_condition: a.has_condition,
                    has_character: a.has_character,
                    connected_to: a.connected_to,
                    tag: a.tag,
                })?
                .into_iter()
                .map(place_to_hit)
                .collect();
            println!("{}", serde_json::to_string_pretty(&hits)?);
        }
        SearchCommands::Lore(a) => {
            let a = *a;
            let res = engine.search_lore_by(&LoreSearchFilter {
                query: a.query,
                category: a.category,
                visibility: a.visibility,
                keyword: a.keyword,
                tag: a.tag,
                related_to: a.related_to,
                always_include: None,
                limit: Some(a.limit),
            })?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        SearchCommands::Texts(a) => {
            let a = *a;
            let res = engine.search_texts_by(&TextSearchFilter {
                query: a.query,
                medium: a.medium,
                subject: a.subject,
                author: a.author,
                language: a.language,
                condition: a.condition,
                read_status: a.read_status,
                read_by: a.read_by,
                holder_or_location_id: a.holder,
                keyword: a.keyword,
                tag: a.tag,
                related_to: a.related_to,
                limit: Some(a.limit),
            })?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        SearchCommands::Items(a) => {
            let a = *a;
            let res = engine.search_items_by(&ItemSearchFilter {
                query: a.query,
                item_type: a.item_type,
                rarity_or_tier: a.rarity,
                current_holder_id: a.holder,
                previous_owner: a.previous_owner,
                condition: a.condition,
                has_linked_text: a.has_linked_text,
                keyword: a.keyword,
                tag: a.tag,
                related_to: a.related_to,
                limit: Some(a.limit),
            })?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        SearchCommands::Histories(a) => {
            let a = *a;
            let res = engine.search_histories_by(&HistorySearchFilter {
                query: a.query,
                entity_id: a.entity,
                entity_kind: a.kind,
                era_or_date: a.era,
                visibility: a.visibility,
                related_to: a.related_to,
                limit: Some(a.limit),
            })?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        SearchCommands::Attachments(a) => {
            let a = *a;
            let res = engine.search_attachments_by(&AttachmentSearchFilter {
                query: a.query,
                category: a.category,
                mime_type: a.mime_type,
                visibility: a.visibility,
                keyword: a.keyword,
                tag: a.tag,
                related_to: a.related_to,
                limit: Some(a.limit),
            })?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        SearchCommands::Events(a) => {
            let a = *a;
            let res = engine.search_events_by(&EventSearchFilter {
                query: a.query,
                entity_id: a.entity,
                location_id: a.location,
                event_type: a.event_type,
                session_number: a.session,
                world_time_contains: a.world_time,
                seq_from: a.seq_from,
                seq_to: a.seq_to,
                limit: Some(a.limit),
            })?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
        SearchCommands::Relations(a) => {
            let a = *a;
            let res = engine.search_relations_by(&RelationSearchFilter {
                entity_id: a.entity,
                source_id: None,
                target_id: None,
                relation_type: a.relation_type,
                min_strength: a.min_strength,
                max_strength: a.max_strength,
                query: a.query,
                hops: Some(a.hops),
            })?;
            println!("{}", serde_json::to_string_pretty(&res)?);
        }
    }
    Ok(())
}
