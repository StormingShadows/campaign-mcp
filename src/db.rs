use crate::autonomy::{
    ActorBelief, CharacterProfileExt, FactionRecord, KnowledgeClaim, RelationshipDimensions,
};
use crate::mechanics::{
    EncounterState, EventCausalMeta, RandomTable, RulesetProfile, StagedDiscovery,
};
use crate::models::{
    CampaignMeta, CatalogItem, CharacterBaseline, EntityHistoryEntry, EventPayload, GraphRelation,
    LoreEntry, PlaceBaseline, ProgressClock, StoryAttachment, StoryText, WorldClock, WorldEvent,
};
use crate::routines::CharacterRoutine;
use crate::signals::EngineSignal;
use crate::world_sim::{
    CharacterProgressionTrack, CraftingProject, CrimeRecord, InjuryRecord, ItemEconomics,
    JurisdictionRecord, MapGrid, MapGridCell, PlaceSpatialExt, ProgressionLadder,
    ReputationStanding, SettlementMarket, TravelRoute,
};
use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChainVerificationReport {
    pub valid: bool,
    pub total_events: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_failed_seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored_hash: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DatabaseIdentity {
    pub db_path: String,
    pub db_fingerprint: String,
    pub total_events: usize,
    pub latest_event_seq: i64,
    pub latest_event_hash: String,
    pub wal_present: bool,
    pub wal_bytes: u64,
    pub backup_count: usize,
    pub backups: Vec<String>,
}

pub struct CampaignDb {
    conn: Connection,
    db_path: PathBuf,
}

impl CampaignDb {
    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory {}", parent.display()))?;
        }
        let conn = Connection::open(db_path)
            .with_context(|| format!("Failed to open SQLite database {}", db_path.display()))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;",
        )?;
        let db = Self {
            conn,
            db_path: db_path.to_path_buf(),
        };
        db.init_schema()?;
        Ok(db)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn init_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS campaign_meta (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT NOT NULL,
                ruleset_notes TEXT NOT NULL,
                clock_json TEXT NOT NULL,
                current_session INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS character_baselines (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                baseline_json TEXT NOT NULL,
                created_seq INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS place_baselines (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                place_type TEXT NOT NULL,
                parent_place TEXT,
                baseline_json TEXT NOT NULL,
                created_seq INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS lore_entries (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                category TEXT NOT NULL,
                summary TEXT NOT NULL,
                body TEXT NOT NULL,
                keywords_json TEXT NOT NULL,
                tags_json TEXT NOT NULL,
                visibility TEXT NOT NULL,
                always_include INTEGER NOT NULL DEFAULT 0,
                related_ids_json TEXT NOT NULL,
                created_at_world_time TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS lore_fts USING fts5(
                id UNINDEXED,
                title,
                category,
                summary,
                body,
                keywords,
                tags
            );

            CREATE TABLE IF NOT EXISTS story_texts (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                medium TEXT NOT NULL,
                subject TEXT NOT NULL,
                author TEXT NOT NULL,
                language TEXT NOT NULL,
                condition TEXT NOT NULL,
                read_status TEXT NOT NULL,
                read_by_json TEXT NOT NULL,
                holder_or_location_id TEXT,
                summary TEXT NOT NULL,
                content TEXT NOT NULL,
                mechanics_json TEXT NOT NULL,
                gm_notes TEXT NOT NULL,
                keywords_json TEXT NOT NULL,
                tags_json TEXT NOT NULL,
                related_ids_json TEXT NOT NULL,
                created_at_world_time TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS story_texts_fts USING fts5(
                id UNINDEXED,
                title,
                medium,
                subject,
                author,
                language,
                summary,
                content,
                mechanics,
                keywords,
                tags
            );

            CREATE TABLE IF NOT EXISTS attachments (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                original_filename TEXT NOT NULL,
                stored_filename TEXT NOT NULL,
                relative_path TEXT NOT NULL,
                category TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                sha256 TEXT NOT NULL,
                summary TEXT NOT NULL,
                extracted_notes TEXT NOT NULL,
                visibility TEXT NOT NULL,
                keywords_json TEXT NOT NULL,
                tags_json TEXT NOT NULL,
                related_ids_json TEXT NOT NULL,
                source_origin TEXT NOT NULL,
                created_at_world_time TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS attachments_fts USING fts5(
                id UNINDEXED,
                title,
                original_filename,
                category,
                summary,
                extracted_notes,
                keywords,
                tags
            );

            CREATE TABLE IF NOT EXISTS catalog_items (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                item_type TEXT NOT NULL,
                rarity_or_tier TEXT NOT NULL,
                slot TEXT,
                quantity INTEGER NOT NULL DEFAULT 1,
                equipped INTEGER NOT NULL DEFAULT 0,
                condition_or_durability TEXT NOT NULL,
                origin_and_maker TEXT NOT NULL,
                previous_owners_json TEXT NOT NULL,
                current_holder_id TEXT,
                significance TEXT NOT NULL,
                properties_and_effects TEXT NOT NULL,
                description TEXT NOT NULL,
                linked_text_id TEXT,
                gm_notes TEXT NOT NULL,
                keywords_json TEXT NOT NULL,
                tags_json TEXT NOT NULL,
                related_ids_json TEXT NOT NULL,
                garment_measurements_json TEXT NOT NULL DEFAULT '{}',
                garment_cut TEXT NOT NULL DEFAULT '',
                garment_taper TEXT NOT NULL DEFAULT '',
                created_at_world_time TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS catalog_items_fts USING fts5(
                id UNINDEXED,
                name,
                item_type,
                origin_and_maker,
                previous_owners,
                significance,
                properties_and_effects,
                description,
                keywords,
                tags
            );

            CREATE TABLE IF NOT EXISTS entity_histories (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entity_id TEXT NOT NULL,
                entity_kind TEXT NOT NULL,
                era_or_date TEXT NOT NULL,
                sort_order INTEGER NOT NULL DEFAULT 0,
                title TEXT NOT NULL,
                details TEXT NOT NULL,
                significance TEXT NOT NULL,
                related_ids_json TEXT NOT NULL,
                visibility TEXT NOT NULL,
                recorded_at_world_time TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_entity_histories_entity ON entity_histories(entity_id, sort_order, id);

            CREATE VIRTUAL TABLE IF NOT EXISTS entity_histories_fts USING fts5(
                id UNINDEXED,
                entity_id,
                entity_kind,
                era_or_date,
                title,
                details,
                significance
            );

            CREATE TABLE IF NOT EXISTS events (
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

            CREATE TABLE IF NOT EXISTS event_entities (
                event_seq INTEGER NOT NULL,
                entity_id TEXT NOT NULL,
                PRIMARY KEY (event_seq, entity_id),
                FOREIGN KEY (event_seq) REFERENCES events(seq) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_event_entities_entity ON event_entities(entity_id, event_seq);
            CREATE INDEX IF NOT EXISTS idx_events_location ON events(location_id, seq);

            CREATE VIRTUAL TABLE IF NOT EXISTS events_fts USING fts5(
                seq UNINDEXED,
                world_time,
                summary,
                details
            );

            CREATE TABLE IF NOT EXISTS relations (
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

            CREATE INDEX IF NOT EXISTS idx_relations_target ON relations(target_id);

            CREATE TABLE IF NOT EXISTS clocks (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                filled INTEGER NOT NULL,
                segments INTEGER NOT NULL,
                status TEXT NOT NULL,
                description TEXT NOT NULL,
                related_ids_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS event_causality (
                event_seq INTEGER NOT NULL,
                parent_event_seq INTEGER NOT NULL,
                PRIMARY KEY (event_seq, parent_event_seq),
                FOREIGN KEY (event_seq) REFERENCES events(seq) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_event_causality_parent ON event_causality(parent_event_seq);

            CREATE TABLE IF NOT EXISTS ruleset_profiles (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                resolution_mode TEXT NOT NULL,
                profile_json TEXT NOT NULL,
                version INTEGER NOT NULL DEFAULT 1,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS random_tables (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                category TEXT NOT NULL,
                is_loot_table INTEGER NOT NULL DEFAULT 0,
                version INTEGER NOT NULL DEFAULT 1,
                table_json TEXT NOT NULL,
                created_at_world_time TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS table_draw_state (
                table_id TEXT NOT NULL,
                deck_scope TEXT NOT NULL,
                drawn_ids_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL,
                PRIMARY KEY (table_id, deck_scope)
            );

            CREATE TABLE IF NOT EXISTS staged_discoveries (
                id TEXT PRIMARY KEY,
                source_table_id TEXT NOT NULL,
                location_id TEXT,
                status TEXT NOT NULL,
                discovery_json TEXT NOT NULL,
                created_at_world_time TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS encounters (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                encounter_type TEXT NOT NULL,
                location_id TEXT,
                status TEXT NOT NULL,
                encounter_json TEXT NOT NULL,
                started_at_world_time TEXT NOT NULL,
                ended_at_world_time TEXT
            );

            CREATE TABLE IF NOT EXISTS character_profiles (
                character_id TEXT PRIMARY KEY,
                profile_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS knowledge_claims (
                id TEXT PRIMARY KEY,
                subject TEXT NOT NULL,
                claim TEXT NOT NULL,
                truth_status TEXT NOT NULL,
                visibility TEXT NOT NULL,
                claim_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_knowledge_claims_subject ON knowledge_claims(subject);

            CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_claims_fts USING fts5(
                id UNINDEXED,
                subject,
                claim,
                tags
            );

            CREATE TABLE IF NOT EXISTS actor_beliefs (
                claim_id TEXT NOT NULL,
                actor_id TEXT NOT NULL,
                stance TEXT NOT NULL,
                confidence INTEGER NOT NULL,
                belief_json TEXT NOT NULL,
                PRIMARY KEY (claim_id, actor_id),
                FOREIGN KEY (claim_id) REFERENCES knowledge_claims(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_actor_beliefs_actor ON actor_beliefs(actor_id);

            CREATE TABLE IF NOT EXISTS factions (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                faction_type TEXT NOT NULL,
                faction_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE VIRTUAL TABLE IF NOT EXISTS factions_fts USING fts5(
                id UNINDEXED,
                name,
                faction_type,
                ideology,
                origins,
                notes,
                tags
            );

            CREATE TABLE IF NOT EXISTS place_spatial (
                place_id TEXT PRIMARY KEY,
                spatial_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS travel_routes (
                id TEXT PRIMARY KEY,
                from_place_id TEXT NOT NULL,
                to_place_id TEXT NOT NULL,
                status TEXT NOT NULL,
                route_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS item_economics (
                item_key TEXT PRIMARY KEY,
                econ_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS currency_balances (
                owner_id TEXT NOT NULL,
                currency TEXT NOT NULL,
                amount INTEGER NOT NULL DEFAULT 0,
                updated_at_world_time TEXT NOT NULL,
                PRIMARY KEY (owner_id, currency)
            );

            CREATE TABLE IF NOT EXISTS markets (
                place_id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                market_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS jurisdictions (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                jurisdiction_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS crime_records (
                id TEXT PRIMARY KEY,
                jurisdiction_id TEXT NOT NULL,
                suspect_id TEXT NOT NULL,
                warrant_status TEXT NOT NULL,
                crime_json TEXT NOT NULL,
                recorded_at_world_time TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS reputation_scores (
                entity_id TEXT NOT NULL,
                target_group_id TEXT NOT NULL,
                score INTEGER NOT NULL,
                rep_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL,
                PRIMARY KEY (entity_id, target_group_id)
            );

            CREATE TABLE IF NOT EXISTS injuries (
                id TEXT PRIMARY KEY,
                character_id TEXT NOT NULL,
                status TEXT NOT NULL,
                injury_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS crafting_projects (
                id TEXT PRIMARY KEY,
                crafter_id TEXT NOT NULL,
                status TEXT NOT NULL,
                project_json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS progression_ladders (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                ladder_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS character_progression (
                character_id TEXT NOT NULL,
                track_id TEXT NOT NULL,
                track_kind TEXT NOT NULL,
                status TEXT NOT NULL,
                track_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL,
                PRIMARY KEY (character_id, track_id)
            );

            CREATE TABLE IF NOT EXISTS map_grids (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                scale_kind TEXT NOT NULL,
                grid_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS map_grid_cells (
                grid_id TEXT NOT NULL,
                x INTEGER NOT NULL,
                y INTEGER NOT NULL,
                place_id TEXT,
                cell_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL,
                PRIMARY KEY (grid_id, x, y)
            );
            CREATE INDEX IF NOT EXISTS idx_map_grid_cells_place ON map_grid_cells(place_id);

            CREATE INDEX IF NOT EXISTS idx_char_prog_char ON character_progression(character_id);

            CREATE TABLE IF NOT EXISTS engine_signals (
                id TEXT PRIMARY KEY,
                dedup_key TEXT NOT NULL,
                code TEXT NOT NULL,
                severity TEXT NOT NULL,
                category TEXT NOT NULL,
                status TEXT NOT NULL,
                visibility TEXT NOT NULL,
                signal_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_engine_signals_status ON engine_signals(status, severity);
            CREATE INDEX IF NOT EXISTS idx_engine_signals_dedup ON engine_signals(dedup_key);

            CREATE TABLE IF NOT EXISTS character_routines (
                id TEXT PRIMARY KEY,
                character_id TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                routine_json TEXT NOT NULL,
                updated_at_world_time TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_character_routines_char ON character_routines(character_id);
            ",
        )?;
        self.ensure_event_causal_columns()?;
        self.ensure_relation_extended_columns()?;
        self.ensure_catalog_item_garment_columns()?;
        self.ensure_clock_extended_columns()?;
        Ok(())
    }

    fn ensure_clock_extended_columns(&self) -> Result<()> {
        let mut stmt = self.conn.prepare("PRAGMA table_info(clocks)")?;
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .collect();
        if !cols.iter().any(|c| c == "cadence_hours") {
            self.conn
                .execute("ALTER TABLE clocks ADD COLUMN cadence_hours INTEGER", [])?;
        }
        if !cols.iter().any(|c| c == "last_tick_total_hours") {
            self.conn.execute(
                "ALTER TABLE clocks ADD COLUMN last_tick_total_hours INTEGER",
                [],
            )?;
        }
        if !cols.iter().any(|c| c == "consequence_on_fill") {
            self.conn.execute(
                "ALTER TABLE clocks ADD COLUMN consequence_on_fill TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        if !cols.iter().any(|c| c == "auto_reset") {
            self.conn.execute(
                "ALTER TABLE clocks ADD COLUMN auto_reset INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_catalog_item_garment_columns(&self) -> Result<()> {
        let mut stmt = self.conn.prepare("PRAGMA table_info(catalog_items)")?;
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .collect();
        if !cols.iter().any(|c| c == "garment_measurements_json") {
            self.conn.execute(
                "ALTER TABLE catalog_items ADD COLUMN garment_measurements_json TEXT NOT NULL DEFAULT '{}'",
                [],
            )?;
        }
        if !cols.iter().any(|c| c == "garment_cut") {
            self.conn.execute(
                "ALTER TABLE catalog_items ADD COLUMN garment_cut TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        if !cols.iter().any(|c| c == "garment_taper") {
            self.conn.execute(
                "ALTER TABLE catalog_items ADD COLUMN garment_taper TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_relation_extended_columns(&self) -> Result<()> {
        let mut stmt = self.conn.prepare("PRAGMA table_info(relations)")?;
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .collect();
        if !cols.iter().any(|c| c == "dimensions_json") {
            self.conn.execute(
                "ALTER TABLE relations ADD COLUMN dimensions_json TEXT NOT NULL DEFAULT '{}'",
                [],
            )?;
        }
        if !cols.iter().any(|c| c == "direction") {
            self.conn.execute(
                "ALTER TABLE relations ADD COLUMN direction TEXT NOT NULL DEFAULT 'directed'",
                [],
            )?;
        }
        if !cols.iter().any(|c| c == "visibility") {
            self.conn.execute(
                "ALTER TABLE relations ADD COLUMN visibility TEXT NOT NULL DEFAULT 'public'",
                [],
            )?;
        }
        if !cols.iter().any(|c| c == "origin_event_id") {
            self.conn
                .execute("ALTER TABLE relations ADD COLUMN origin_event_id INTEGER", [])?;
        }
        if !cols.iter().any(|c| c == "obligations_json") {
            self.conn.execute(
                "ALTER TABLE relations ADD COLUMN obligations_json TEXT NOT NULL DEFAULT '[]'",
                [],
            )?;
        }
        Ok(())
    }

    /// Safe idempotent schema migration adding causal DAG & visibility columns to existing `events` tables
    /// without altering the existing SHA-256 hash chain inputs.
    fn ensure_event_causal_columns(&self) -> Result<()> {
        let mut stmt = self.conn.prepare("PRAGMA table_info(events)")?;
        let cols: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .collect();
        if !cols.iter().any(|c| c == "actor_id") {
            self.conn
                .execute("ALTER TABLE events ADD COLUMN actor_id TEXT", [])?;
        }
        if !cols.iter().any(|c| c == "causal_event_ids_json") {
            self.conn.execute(
                "ALTER TABLE events ADD COLUMN causal_event_ids_json TEXT NOT NULL DEFAULT '[]'",
                [],
            )?;
        }
        if !cols.iter().any(|c| c == "visibility") {
            self.conn.execute(
                "ALTER TABLE events ADD COLUMN visibility TEXT NOT NULL DEFAULT 'public'",
                [],
            )?;
        }
        Ok(())
    }

    pub fn is_initialized(&self) -> Result<bool> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM campaign_meta", [], |r| r.get(0))?;
        Ok(count > 0)
    }

    pub fn init_campaign(&self, meta: &CampaignMeta) -> Result<()> {
        if self.is_initialized()? {
            bail!("Campaign is already initialized in {}", self.db_path.display());
        }
        let clock_json = serde_json::to_string(&meta.clock)?;
        self.conn.execute(
            "INSERT INTO campaign_meta (id, name, description, ruleset_notes, clock_json, current_session, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                meta.id,
                meta.name,
                meta.description,
                meta.ruleset_notes,
                clock_json,
                meta.current_session,
                meta.created_at,
            ],
        )?;
        self.append_event(
            None,
            vec![],
            format!("Campaign '{}' initialized.", meta.name),
            EventPayload::CampaignStarted {
                name: meta.name.clone(),
            },
        )?;
        Ok(())
    }

    pub fn get_campaign_meta(&self) -> Result<CampaignMeta> {
        self.conn
            .query_row(
                "SELECT id, name, description, ruleset_notes, clock_json, current_session, created_at
                 FROM campaign_meta LIMIT 1",
                [],
                |row| {
                    let clock_json: String = row.get(4)?;
                    let clock: WorldClock = serde_json::from_str(&clock_json).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?;
                    Ok(CampaignMeta {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        description: row.get(2)?,
                        ruleset_notes: row.get(3)?,
                        clock,
                        current_session: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                },
            )
            .map_err(|_| {
                anyhow!(
                    "Campaign not initialized in {}. Run `chronicle init` first.",
                    self.db_path.display()
                )
            })
    }

    pub fn update_campaign_meta_row(&self, meta: &CampaignMeta) -> Result<()> {
        let clock_json = serde_json::to_string(&meta.clock)?;
        self.conn.execute(
            "UPDATE campaign_meta
             SET name = ?1, description = ?2, ruleset_notes = ?3, clock_json = ?4, current_session = ?5",
            params![
                meta.name,
                meta.description,
                meta.ruleset_notes,
                clock_json,
                meta.current_session,
            ],
        )?;
        Ok(())
    }

    pub fn create_backup(&self, label: &str) -> Result<PathBuf> {
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        let safe_label =
            crate::models::slugify(if label.trim().is_empty() { "manual" } else { label });
        let backup_name = format!(
            "campaign.backup-{}-{}.db",
            safe_label,
            Utc::now().format("%Y%m%dT%H%M%SZ")
        );
        let backup_path = self
            .db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(backup_name);
        std::fs::copy(&self.db_path, &backup_path)?;
        Ok(backup_path)
    }

    pub fn update_clock_and_session(&self, clock: &WorldClock, session: u32) -> Result<()> {
        let clock_json = serde_json::to_string(clock)?;
        self.conn.execute(
            "UPDATE campaign_meta SET clock_json = ?1, current_session = ?2",
            params![clock_json, session],
        )?;
        Ok(())
    }

    /// Appends an immutable event to the SHA-256 hash-chained ledger.
    pub fn append_event(
        &self,
        location_id: Option<String>,
        affected_entities: Vec<String>,
        summary: String,
        payload: EventPayload,
    ) -> Result<WorldEvent> {
        self.append_event_causal(
            location_id,
            affected_entities,
            summary,
            payload,
            EventCausalMeta::default(),
        )
    }

    /// Appends an immutable event with optional causal DAG parent links, actor ID, and visibility.
    /// Keeps the SHA-256 hash input format 100% backward-compatible with pre-Phase-1 databases.
    pub fn append_event_causal(
        &self,
        location_id: Option<String>,
        mut affected_entities: Vec<String>,
        summary: String,
        payload: EventPayload,
        causal: EventCausalMeta,
    ) -> Result<WorldEvent> {
        let meta = self.get_campaign_meta()?;
        let world_time = meta.clock.format_short();
        let real_timestamp = Utc::now().to_rfc3339();

        // Deduplicate affected entities cleanly while preserving order
        let mut dedup = Vec::new();
        for e in affected_entities.drain(..) {
            if !e.is_empty() && !dedup.contains(&e) {
                dedup.push(e);
            }
        }
        if let Some(ref actor) = causal.actor_id {
            if !actor.is_empty() && !dedup.contains(actor) {
                dedup.push(actor.clone());
            }
        }
        if let Some(ref loc) = location_id {
            if !loc.is_empty() && !dedup.contains(loc) {
                dedup.push(loc.clone());
            }
        }

        let mut dedup_parents = Vec::new();
        for p in causal.causal_event_ids {
            if p > 0 && !dedup_parents.contains(&p) {
                dedup_parents.push(p);
            }
        }
        let visibility = if causal.visibility.trim().is_empty() {
            "public".to_string()
        } else {
            causal.visibility.trim().to_string()
        };

        let prev_hash: String = self
            .conn
            .query_row(
                "SELECT hash FROM events ORDER BY seq DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| "GENESIS".to_string());

        let affected_json = serde_json::to_string(&dedup)?;
        let payload_json = serde_json::to_string(&payload)?;
        let causal_json = serde_json::to_string(&dedup_parents)?;
        let hash = compute_event_hash(
            &prev_hash,
            meta.current_session,
            &world_time,
            &real_timestamp,
            location_id.as_deref().unwrap_or(""),
            &summary,
            &payload_json,
        );

        self.conn.execute(
            "INSERT INTO events
             (session_number, world_time, real_timestamp, location_id, affected_entities_json,
              summary, payload_json, prev_hash, hash, actor_id, causal_event_ids_json, visibility)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                meta.current_session,
                world_time,
                real_timestamp,
                location_id,
                affected_json,
                summary,
                payload_json,
                prev_hash,
                hash,
                causal.actor_id,
                causal_json,
                visibility,
            ],
        )?;
        let seq = self.conn.last_insert_rowid();

        for ent in &dedup {
            self.conn.execute(
                "INSERT OR IGNORE INTO event_entities (event_seq, entity_id) VALUES (?1, ?2)",
                params![seq, ent],
            )?;
        }

        for parent_seq in &dedup_parents {
            self.conn.execute(
                "INSERT OR IGNORE INTO event_causality (event_seq, parent_event_seq) VALUES (?1, ?2)",
                params![seq, parent_seq],
            )?;
        }

        self.conn.execute(
            "INSERT INTO events_fts (seq, world_time, summary, details) VALUES (?1, ?2, ?3, ?4)",
            params![seq, world_time, summary, payload_json],
        )?;

        Ok(WorldEvent {
            seq,
            session_number: meta.current_session,
            world_time,
            real_timestamp,
            location_id,
            affected_entities: dedup,
            summary,
            payload,
            prev_hash,
            hash,
            actor_id: causal.actor_id,
            causal_event_ids: dedup_parents,
            visibility,
        })
    }

    /// Verifies the cryptographic hash chain of the entire event ledger directly against the
    /// canonical stored `payload_json` column in SQLite (so backward-compatible schema additions
    /// with `#[serde(default)]` never invalidate historical digests, while any actual DB tampering
    /// is still caught immediately) and returns exact diagnostic details on the first failure.
    pub fn verify_event_chain_detailed(&self) -> Result<ChainVerificationReport> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, session_number, world_time, real_timestamp, location_id,
                    summary, payload_json, prev_hash, hash
             FROM events ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
            ))
        })?;

        let mut total_events = 0usize;
        let mut expected_prev = "GENESIS".to_string();
        let mut first_failure: Option<(i64, String, String, String)> = None;

        for row_res in rows {
            let (seq, session_number, world_time, real_timestamp, location_id, summary, payload_json, prev_hash, hash) =
                row_res?;
            total_events += 1;

            if first_failure.is_none() {
                if prev_hash != expected_prev {
                    first_failure = Some((
                        seq,
                        "prev_hash_mismatch".to_string(),
                        expected_prev.clone(),
                        prev_hash.clone(),
                    ));
                } else {
                    let computed = compute_event_hash(
                        &prev_hash,
                        session_number,
                        &world_time,
                        &real_timestamp,
                        location_id.as_deref().unwrap_or(""),
                        &summary,
                        &payload_json,
                    );
                    if computed != hash {
                        first_failure = Some((
                            seq,
                            "digest_mismatch".to_string(),
                            computed,
                            hash.clone(),
                        ));
                    }
                }
            }
            expected_prev = hash;
        }

        if let Some((seq, kind, exp, stored)) = first_failure {
            Ok(ChainVerificationReport {
                valid: false,
                total_events,
                first_failed_seq: Some(seq),
                failure_kind: Some(kind),
                expected_hash: Some(exp),
                stored_hash: Some(stored),
            })
        } else {
            Ok(ChainVerificationReport {
                valid: true,
                total_events,
                first_failed_seq: None,
                failure_kind: None,
                expected_hash: None,
                stored_hash: None,
            })
        }
    }

    /// Verifies the cryptographic hash chain of the entire event ledger.
    pub fn verify_event_chain(&self) -> Result<(bool, usize)> {
        let rep = self.verify_event_chain_detailed()?;
        Ok((rep.valid, rep.total_events))
    }

    /// Returns a stable database/environment identity fingerprint plus backup visibility
    /// so agents and users can immediately detect environment or snapshot discrepancies.
    pub fn database_identity(&self) -> Result<DatabaseIdentity> {
        let total_events: usize = self
            .conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get::<_, i64>(0))?
            .max(0) as usize;
        let genesis_hash: String = self
            .conn
            .query_row("SELECT hash FROM events ORDER BY seq ASC LIMIT 1", [], |r| {
                r.get(0)
            })
            .optional()?
            .unwrap_or_else(|| "EMPTY".to_string());
        let (latest_seq, latest_hash): (i64, String) = self
            .conn
            .query_row(
                "SELECT seq, hash FROM events ORDER BY seq DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .unwrap_or((0, "EMPTY".to_string()));

        let mut fp_hasher = Sha256::new();
        fp_hasher.update(genesis_hash.as_bytes());
        fp_hasher.update(b":");
        fp_hasher.update(latest_hash.as_bytes());
        fp_hasher.update(b":");
        fp_hasher.update(total_events.to_string().as_bytes());
        let fp_hex: String = fp_hasher
            .finalize()
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();

        let wal_path = PathBuf::from(format!("{}-wal", self.db_path.display()));
        let (wal_present, wal_bytes) = match std::fs::metadata(&wal_path) {
            Ok(m) => (true, m.len()),
            Err(_) => (false, 0),
        };

        let mut backups = Vec::new();
        if let Some(dir) = self.db_path.parent() {
            if let Ok(rd) = std::fs::read_dir(dir) {
                for ent in rd.flatten() {
                    if let Some(name) = ent.file_name().to_str() {
                        if name.starts_with("campaign.backup-") && name.ends_with(".db") {
                            backups.push(name.to_string());
                        }
                    }
                }
            }
        }
        backups.sort();
        backups.reverse();

        Ok(DatabaseIdentity {
            db_path: self.db_path.display().to_string(),
            db_fingerprint: fp_hex,
            total_events,
            latest_event_seq: latest_seq,
            latest_event_hash: latest_hash,
            wal_present,
            wal_bytes,
            backup_count: backups.len(),
            backups,
        })
    }

    pub fn insert_character_baseline(
        &self,
        baseline: &CharacterBaseline,
        created_seq: i64,
    ) -> Result<()> {
        let json = serde_json::to_string(baseline)?;
        self.conn.execute(
            "INSERT INTO character_baselines (id, name, kind, baseline_json, created_seq)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![baseline.id, baseline.name, baseline.kind, json, created_seq],
        )?;
        Ok(())
    }

    pub fn get_character_baseline(&self, id: &str) -> Result<Option<CharacterBaseline>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT baseline_json FROM character_baselines WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match json {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_character_baselines(&self) -> Result<Vec<CharacterBaseline>> {
        let mut stmt = self
            .conn
            .prepare("SELECT baseline_json FROM character_baselines ORDER BY kind, name")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    pub fn insert_place_baseline(&self, baseline: &PlaceBaseline, created_seq: i64) -> Result<()> {
        let json = serde_json::to_string(baseline)?;
        self.conn.execute(
            "INSERT INTO place_baselines (id, name, place_type, parent_place, baseline_json, created_seq)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                baseline.id,
                baseline.name,
                baseline.place_type,
                baseline.parent_place,
                json,
                created_seq,
            ],
        )?;
        Ok(())
    }

    pub fn get_place_baseline(&self, id: &str) -> Result<Option<PlaceBaseline>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT baseline_json FROM place_baselines WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match json {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_place_baselines(&self) -> Result<Vec<PlaceBaseline>> {
        let mut stmt = self
            .conn
            .prepare("SELECT baseline_json FROM place_baselines ORDER BY place_type, name")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(serde_json::from_str(&row?)?);
        }
        Ok(out)
    }

    pub fn upsert_lore(&self, entry: &LoreEntry) -> Result<()> {
        let keywords_json = serde_json::to_string(&entry.keywords)?;
        let tags_json = serde_json::to_string(&entry.tags)?;
        let related_json = serde_json::to_string(&entry.related_ids)?;
        self.conn.execute(
            "INSERT INTO lore_entries
             (id, title, category, summary, body, keywords_json, tags_json, visibility, always_include, related_ids_json, created_at_world_time, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET
               title = excluded.title,
               category = excluded.category,
               summary = excluded.summary,
               body = excluded.body,
               keywords_json = excluded.keywords_json,
               tags_json = excluded.tags_json,
               visibility = excluded.visibility,
               always_include = excluded.always_include,
               related_ids_json = excluded.related_ids_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                entry.id,
                entry.title,
                entry.category,
                entry.summary,
                entry.body,
                keywords_json,
                tags_json,
                entry.visibility,
                if entry.always_include { 1 } else { 0 },
                related_json,
                entry.created_at_world_time,
                entry.updated_at_world_time,
            ],
        )?;

        self.conn
            .execute("DELETE FROM lore_fts WHERE id = ?1", params![entry.id])?;
        self.conn.execute(
            "INSERT INTO lore_fts (id, title, category, summary, body, keywords, tags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                entry.id,
                entry.title,
                entry.category,
                entry.summary,
                entry.body,
                entry.keywords.join(" "),
                entry.tags.join(" "),
            ],
        )?;
        Ok(())
    }

    pub fn get_lore(&self, id: &str) -> Result<Option<LoreEntry>> {
        self.conn
            .query_row(
                "SELECT id, title, category, summary, body, keywords_json, tags_json, visibility, always_include, related_ids_json, created_at_world_time, updated_at_world_time
                 FROM lore_entries WHERE id = ?1",
                params![id],
                row_to_lore,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_lore(&self, category: Option<&str>) -> Result<Vec<LoreEntry>> {
        let mut out = Vec::new();
        if let Some(cat) = category {
            let mut stmt = self.conn.prepare(
                "SELECT id, title, category, summary, body, keywords_json, tags_json, visibility, always_include, related_ids_json, created_at_world_time, updated_at_world_time
                 FROM lore_entries WHERE LOWER(category) = LOWER(?1) ORDER BY title",
            )?;
            let rows = stmt.query_map(params![cat], row_to_lore)?;
            for r in rows {
                out.push(r?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT id, title, category, summary, body, keywords_json, tags_json, visibility, always_include, related_ids_json, created_at_world_time, updated_at_world_time
                 FROM lore_entries ORDER BY category, title",
            )?;
            let rows = stmt.query_map([], row_to_lore)?;
            for r in rows {
                out.push(r?);
            }
        }
        Ok(out)
    }

    pub fn search_lore_fts(&self, query: &str, limit: usize) -> Result<Vec<LoreEntry>> {
        let fts_query = sanitize_fts_query(query);
        if fts_query.is_empty() {
            return Ok(vec![]);
        }
        let mut stmt = self.conn.prepare(
            "SELECT l.id, l.title, l.category, l.summary, l.body, l.keywords_json, l.tags_json,
                    l.visibility, l.always_include, l.related_ids_json, l.created_at_world_time, l.updated_at_world_time
             FROM lore_fts f
             JOIN lore_entries l ON l.id = f.id
             WHERE lore_fts MATCH ?1
             ORDER BY bm25(lore_fts)
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts_query, limit as i64], row_to_lore)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn upsert_text(&self, text: &StoryText) -> Result<()> {
        let read_by_json = serde_json::to_string(&text.read_by)?;
        let mechanics_json = serde_json::to_string(&text.mechanics_or_unlocks)?;
        let keywords_json = serde_json::to_string(&text.keywords)?;
        let tags_json = serde_json::to_string(&text.tags)?;
        let related_json = serde_json::to_string(&text.related_ids)?;

        self.conn.execute(
            "INSERT INTO story_texts
             (id, title, medium, subject, author, language, condition, read_status, read_by_json,
              holder_or_location_id, summary, content, mechanics_json, gm_notes, keywords_json,
              tags_json, related_ids_json, created_at_world_time, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
             ON CONFLICT(id) DO UPDATE SET
               title = excluded.title,
               medium = excluded.medium,
               subject = excluded.subject,
               author = excluded.author,
               language = excluded.language,
               condition = excluded.condition,
               read_status = excluded.read_status,
               read_by_json = excluded.read_by_json,
               holder_or_location_id = excluded.holder_or_location_id,
               summary = excluded.summary,
               content = excluded.content,
               mechanics_json = excluded.mechanics_json,
               gm_notes = excluded.gm_notes,
               keywords_json = excluded.keywords_json,
               tags_json = excluded.tags_json,
               related_ids_json = excluded.related_ids_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                text.id,
                text.title,
                text.medium,
                text.subject,
                text.author,
                text.language,
                text.condition,
                text.read_status,
                read_by_json,
                text.holder_or_location_id,
                text.summary,
                text.content,
                mechanics_json,
                text.gm_notes,
                keywords_json,
                tags_json,
                related_json,
                text.created_at_world_time,
                text.updated_at_world_time,
            ],
        )?;

        self.conn
            .execute("DELETE FROM story_texts_fts WHERE id = ?1", params![text.id])?;
        self.conn.execute(
            "INSERT INTO story_texts_fts
             (id, title, medium, subject, author, language, summary, content, mechanics, keywords, tags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                text.id,
                text.title,
                text.medium,
                text.subject,
                text.author,
                text.language,
                text.summary,
                text.content,
                text.mechanics_or_unlocks.join(" "),
                text.keywords.join(" "),
                text.tags.join(" "),
            ],
        )?;
        Ok(())
    }

    pub fn get_text(&self, id: &str) -> Result<Option<StoryText>> {
        self.conn
            .query_row(
                "SELECT id, title, medium, subject, author, language, condition, read_status, read_by_json,
                        holder_or_location_id, summary, content, mechanics_json, gm_notes, keywords_json,
                        tags_json, related_ids_json, created_at_world_time, updated_at_world_time
                 FROM story_texts WHERE id = ?1",
                params![id],
                row_to_text,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_texts(&self) -> Result<Vec<StoryText>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, medium, subject, author, language, condition, read_status, read_by_json,
                    holder_or_location_id, summary, content, mechanics_json, gm_notes, keywords_json,
                    tags_json, related_ids_json, created_at_world_time, updated_at_world_time
             FROM story_texts ORDER BY subject, medium, title",
        )?;
        let rows = stmt.query_map([], row_to_text)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn search_texts_fts(&self, query: &str, limit: usize) -> Result<Vec<StoryText>> {
        let fts_query = sanitize_fts_query(query);
        if fts_query.is_empty() {
            return Ok(vec![]);
        }
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.title, t.medium, t.subject, t.author, t.language, t.condition, t.read_status,
                    t.read_by_json, t.holder_or_location_id, t.summary, t.content, t.mechanics_json,
                    t.gm_notes, t.keywords_json, t.tags_json, t.related_ids_json,
                    t.created_at_world_time, t.updated_at_world_time
             FROM story_texts_fts f
             JOIN story_texts t ON t.id = f.id
             WHERE story_texts_fts MATCH ?1
             ORDER BY bm25(story_texts_fts)
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts_query, limit as i64], row_to_text)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn upsert_attachment(&self, att: &StoryAttachment) -> Result<()> {
        let keywords_json = serde_json::to_string(&att.keywords)?;
        let tags_json = serde_json::to_string(&att.tags)?;
        let related_json = serde_json::to_string(&att.related_ids)?;

        self.conn.execute(
            "INSERT INTO attachments
             (id, title, original_filename, stored_filename, relative_path, category, mime_type,
              size_bytes, sha256, summary, extracted_notes, visibility, keywords_json, tags_json,
              related_ids_json, source_origin, created_at_world_time, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
             ON CONFLICT(id) DO UPDATE SET
               title = excluded.title,
               original_filename = excluded.original_filename,
               stored_filename = excluded.stored_filename,
               relative_path = excluded.relative_path,
               category = excluded.category,
               mime_type = excluded.mime_type,
               size_bytes = excluded.size_bytes,
               sha256 = excluded.sha256,
               summary = excluded.summary,
               extracted_notes = excluded.extracted_notes,
               visibility = excluded.visibility,
               keywords_json = excluded.keywords_json,
               tags_json = excluded.tags_json,
               related_ids_json = excluded.related_ids_json,
               source_origin = excluded.source_origin,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                att.id,
                att.title,
                att.original_filename,
                att.stored_filename,
                att.relative_path,
                att.category,
                att.mime_type,
                att.size_bytes as i64,
                att.sha256,
                att.summary,
                att.extracted_notes,
                att.visibility,
                keywords_json,
                tags_json,
                related_json,
                att.source_origin,
                att.created_at_world_time,
                att.updated_at_world_time,
            ],
        )?;

        self.conn
            .execute("DELETE FROM attachments_fts WHERE id = ?1", params![att.id])?;
        self.conn.execute(
            "INSERT INTO attachments_fts
             (id, title, original_filename, category, summary, extracted_notes, keywords, tags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                att.id,
                att.title,
                att.original_filename,
                att.category,
                att.summary,
                att.extracted_notes,
                att.keywords.join(" "),
                att.tags.join(" "),
            ],
        )?;
        Ok(())
    }

    pub fn get_attachment(&self, id: &str) -> Result<Option<StoryAttachment>> {
        self.conn
            .query_row(
                "SELECT id, title, original_filename, stored_filename, relative_path, category, mime_type,
                        size_bytes, sha256, summary, extracted_notes, visibility, keywords_json, tags_json,
                        related_ids_json, source_origin, created_at_world_time, updated_at_world_time
                 FROM attachments WHERE id = ?1",
                params![id],
                row_to_attachment,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_attachments(&self) -> Result<Vec<StoryAttachment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, original_filename, stored_filename, relative_path, category, mime_type,
                    size_bytes, sha256, summary, extracted_notes, visibility, keywords_json, tags_json,
                    related_ids_json, source_origin, created_at_world_time, updated_at_world_time
             FROM attachments ORDER BY category, title",
        )?;
        let rows = stmt.query_map([], row_to_attachment)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn search_attachments_fts(&self, query: &str, limit: usize) -> Result<Vec<StoryAttachment>> {
        let fts_query = sanitize_fts_query(query);
        if fts_query.is_empty() {
            return Ok(vec![]);
        }
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.title, a.original_filename, a.stored_filename, a.relative_path, a.category,
                    a.mime_type, a.size_bytes, a.sha256, a.summary, a.extracted_notes, a.visibility,
                    a.keywords_json, a.tags_json, a.related_ids_json, a.source_origin,
                    a.created_at_world_time, a.updated_at_world_time
             FROM attachments_fts f
             JOIN attachments a ON a.id = f.id
             WHERE attachments_fts MATCH ?1
             ORDER BY bm25(attachments_fts)
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts_query, limit as i64], row_to_attachment)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // -------------------------------------------------------------------------
    // Cataloged Carryable Items (Weapons, Tools, Keepsakes, Books, Manuals, Records)
    // -------------------------------------------------------------------------

    pub fn upsert_catalog_item(&self, item: &CatalogItem) -> Result<()> {
        let prev_json = serde_json::to_string(&item.previous_owners)?;
        let kw_json = serde_json::to_string(&item.keywords)?;
        let tags_json = serde_json::to_string(&item.tags)?;
        let rel_json = serde_json::to_string(&item.related_ids)?;
        let gm_meas_json = serde_json::to_string(&item.garment_measurements)?;

        self.conn.execute(
            "INSERT INTO catalog_items
             (id, name, item_type, rarity_or_tier, slot, quantity, equipped, condition_or_durability,
              origin_and_maker, previous_owners_json, current_holder_id, significance,
              properties_and_effects, description, linked_text_id, gm_notes, keywords_json,
              tags_json, related_ids_json, created_at_world_time, updated_at_world_time,
              garment_measurements_json, garment_cut, garment_taper)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               item_type = excluded.item_type,
               rarity_or_tier = excluded.rarity_or_tier,
               slot = excluded.slot,
               quantity = excluded.quantity,
               equipped = excluded.equipped,
               condition_or_durability = excluded.condition_or_durability,
               origin_and_maker = excluded.origin_and_maker,
               previous_owners_json = excluded.previous_owners_json,
               current_holder_id = excluded.current_holder_id,
               significance = excluded.significance,
               properties_and_effects = excluded.properties_and_effects,
               description = excluded.description,
               linked_text_id = excluded.linked_text_id,
               gm_notes = excluded.gm_notes,
               keywords_json = excluded.keywords_json,
               tags_json = excluded.tags_json,
               related_ids_json = excluded.related_ids_json,
               updated_at_world_time = excluded.updated_at_world_time,
               garment_measurements_json = excluded.garment_measurements_json,
               garment_cut = excluded.garment_cut,
               garment_taper = excluded.garment_taper",
            params![
                item.id,
                item.name,
                item.item_type,
                item.rarity_or_tier,
                item.slot,
                item.quantity,
                if item.equipped { 1 } else { 0 },
                item.condition_or_durability,
                item.origin_and_maker,
                prev_json,
                item.current_holder_id,
                item.significance,
                item.properties_and_effects,
                item.description,
                item.linked_text_id,
                item.gm_notes,
                kw_json,
                tags_json,
                rel_json,
                item.created_at_world_time,
                item.updated_at_world_time,
                gm_meas_json,
                item.garment_cut,
                item.garment_taper,
            ],
        )?;

        self.conn
            .execute("DELETE FROM catalog_items_fts WHERE id = ?1", params![item.id])?;
        self.conn.execute(
            "INSERT INTO catalog_items_fts
             (id, name, item_type, origin_and_maker, previous_owners, significance, properties_and_effects, description, keywords, tags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                item.id,
                item.name,
                item.item_type,
                item.origin_and_maker,
                item.previous_owners.join(" "),
                item.significance,
                item.properties_and_effects,
                item.description,
                item.keywords.join(" "),
                item.tags.join(" "),
            ],
        )?;
        Ok(())
    }

    pub fn get_catalog_item(&self, id: &str) -> Result<Option<CatalogItem>> {
        self.conn
            .query_row(
                "SELECT id, name, item_type, rarity_or_tier, slot, quantity, equipped, condition_or_durability,
                        origin_and_maker, previous_owners_json, current_holder_id, significance,
                        properties_and_effects, description, linked_text_id, gm_notes, keywords_json,
                        tags_json, related_ids_json, created_at_world_time, updated_at_world_time,
                        garment_measurements_json, garment_cut, garment_taper
                 FROM catalog_items WHERE id = ?1",
                params![id],
                row_to_catalog_item,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_catalog_items(&self) -> Result<Vec<CatalogItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, item_type, rarity_or_tier, slot, quantity, equipped, condition_or_durability,
                    origin_and_maker, previous_owners_json, current_holder_id, significance,
                    properties_and_effects, description, linked_text_id, gm_notes, keywords_json,
                    tags_json, related_ids_json, created_at_world_time, updated_at_world_time,
                    garment_measurements_json, garment_cut, garment_taper
             FROM catalog_items ORDER BY item_type, name",
        )?;
        let rows = stmt.query_map([], row_to_catalog_item)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn search_catalog_items_fts(&self, query: &str, limit: usize) -> Result<Vec<CatalogItem>> {
        let fts_query = sanitize_fts_query(query);
        if fts_query.is_empty() {
            return Ok(vec![]);
        }
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.name, c.item_type, c.rarity_or_tier, c.slot, c.quantity, c.equipped,
                    c.condition_or_durability, c.origin_and_maker, c.previous_owners_json,
                    c.current_holder_id, c.significance, c.properties_and_effects, c.description,
                    c.linked_text_id, c.gm_notes, c.keywords_json, c.tags_json, c.related_ids_json,
                    c.created_at_world_time, c.updated_at_world_time,
                    c.garment_measurements_json, c.garment_cut, c.garment_taper
             FROM catalog_items_fts f
             JOIN catalog_items c ON c.id = f.id
             WHERE catalog_items_fts MATCH ?1
             ORDER BY bm25(catalog_items_fts)
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts_query, limit as i64], row_to_catalog_item)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // -------------------------------------------------------------------------
    // Entity Histories (Places, Establishments, Sects/Factions, Items, Texts, Chars)
    // -------------------------------------------------------------------------

    pub fn insert_entity_history(&self, entry: &EntityHistoryEntry) -> Result<i64> {
        let rel_json = serde_json::to_string(&entry.related_ids)?;
        self.conn.execute(
            "INSERT INTO entity_histories
             (entity_id, entity_kind, era_or_date, sort_order, title, details, significance,
              related_ids_json, visibility, recorded_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                entry.entity_id,
                entry.entity_kind,
                entry.era_or_date,
                entry.sort_order,
                entry.title,
                entry.details,
                entry.significance,
                rel_json,
                entry.visibility,
                entry.recorded_at_world_time,
            ],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "INSERT INTO entity_histories_fts
             (id, entity_id, entity_kind, era_or_date, title, details, significance)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                entry.entity_id,
                entry.entity_kind,
                entry.era_or_date,
                entry.title,
                entry.details,
                entry.significance,
            ],
        )?;
        Ok(id)
    }

    pub fn list_histories_for_entity(&self, entity_id: &str) -> Result<Vec<EntityHistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, entity_id, entity_kind, era_or_date, sort_order, title, details, significance,
                    related_ids_json, visibility, recorded_at_world_time
             FROM entity_histories
             WHERE entity_id = ?1
             ORDER BY sort_order ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![entity_id], row_to_entity_history)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_all_histories(&self) -> Result<Vec<EntityHistoryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, entity_id, entity_kind, era_or_date, sort_order, title, details, significance,
                    related_ids_json, visibility, recorded_at_world_time
             FROM entity_histories
             ORDER BY entity_kind, entity_id, sort_order ASC, id ASC",
        )?;
        let rows = stmt.query_map([], row_to_entity_history)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn search_histories_fts(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<EntityHistoryEntry>> {
        let fts_query = sanitize_fts_query(query);
        if fts_query.is_empty() {
            return Ok(vec![]);
        }
        let mut stmt = self.conn.prepare(
            "SELECT h.id, h.entity_id, h.entity_kind, h.era_or_date, h.sort_order, h.title,
                    h.details, h.significance, h.related_ids_json, h.visibility, h.recorded_at_world_time
             FROM entity_histories_fts f
             JOIN entity_histories h ON h.id = f.id
             WHERE entity_histories_fts MATCH ?1
             ORDER BY bm25(entity_histories_fts)
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts_query, limit as i64], row_to_entity_history)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn search_events_fts(&self, query: &str, limit: usize) -> Result<Vec<WorldEvent>> {
        let fts_query = sanitize_fts_query(query);
        if fts_query.is_empty() {
            return Ok(vec![]);
        }
        let mut stmt = self.conn.prepare(
            "SELECT e.seq, e.session_number, e.world_time, e.real_timestamp, e.location_id,
                    e.affected_entities_json, e.summary, e.payload_json, e.prev_hash, e.hash,
                    e.actor_id, e.causal_event_ids_json, e.visibility
             FROM events_fts f
             JOIN events e ON e.seq = f.seq
             WHERE events_fts MATCH ?1
             ORDER BY bm25(events_fts)
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts_query, limit as i64], row_to_event)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn get_event(&self, seq: i64) -> Result<Option<WorldEvent>> {
        self.conn
            .query_row(
                "SELECT seq, session_number, world_time, real_timestamp, location_id,
                        affected_entities_json, summary, payload_json, prev_hash, hash,
                        actor_id, causal_event_ids_json, visibility
                 FROM events WHERE seq = ?1",
                params![seq],
                row_to_event,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_causal_children(&self, parent_seq: i64) -> Result<Vec<WorldEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.seq, e.session_number, e.world_time, e.real_timestamp, e.location_id,
                    e.affected_entities_json, e.summary, e.payload_json, e.prev_hash, e.hash,
                    e.actor_id, e.causal_event_ids_json, e.visibility
             FROM events e
             JOIN event_causality ec ON ec.event_seq = e.seq
             WHERE ec.parent_event_seq = ?1
             ORDER BY e.seq ASC",
        )?;
        let rows = stmt.query_map(params![parent_seq], row_to_event)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_all_events(&self) -> Result<Vec<WorldEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, session_number, world_time, real_timestamp, location_id,
                    affected_entities_json, summary, payload_json, prev_hash, hash,
                    actor_id, causal_event_ids_json, visibility
             FROM events ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map([], row_to_event)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_events_up_to(&self, max_seq: i64) -> Result<Vec<WorldEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, session_number, world_time, real_timestamp, location_id,
                    affected_entities_json, summary, payload_json, prev_hash, hash,
                    actor_id, causal_event_ids_json, visibility
             FROM events WHERE seq <= ?1 ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(params![max_seq], row_to_event)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_events_for_entity(&self, entity_id: &str) -> Result<Vec<WorldEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.seq, e.session_number, e.world_time, e.real_timestamp, e.location_id,
                    e.affected_entities_json, e.summary, e.payload_json, e.prev_hash, e.hash,
                    e.actor_id, e.causal_event_ids_json, e.visibility
             FROM events e
             JOIN event_entities ee ON ee.event_seq = e.seq
             WHERE ee.entity_id = ?1
             ORDER BY e.seq ASC",
        )?;
        let rows = stmt.query_map(params![entity_id], row_to_event)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_events_for_place_or_entity(&self, place_id: &str) -> Result<Vec<WorldEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.seq, e.session_number, e.world_time, e.real_timestamp, e.location_id,
                    e.affected_entities_json, e.summary, e.payload_json, e.prev_hash, e.hash,
                    e.actor_id, e.causal_event_ids_json, e.visibility
             FROM events e
             WHERE e.location_id = ?1
                OR e.seq IN (SELECT ee.event_seq FROM event_entities ee WHERE ee.entity_id = ?1)
             ORDER BY e.seq ASC",
        )?;
        let rows = stmt.query_map(params![place_id], row_to_event)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_recent_events(&self, limit: usize) -> Result<Vec<WorldEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, session_number, world_time, real_timestamp, location_id,
                    affected_entities_json, summary, payload_json, prev_hash, hash,
                    actor_id, causal_event_ids_json, visibility
             FROM events ORDER BY seq DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], row_to_event)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        out.reverse();
        Ok(out)
    }

    pub fn upsert_relation(&self, rel: &GraphRelation) -> Result<()> {
        let dims_json = serde_json::to_string(&rel.dimensions)?;
        let ob_json = serde_json::to_string(&rel.obligations)?;
        let direction = if rel.direction.trim().is_empty() {
            "directed"
        } else {
            rel.direction.as_str()
        };
        let visibility = if rel.visibility.trim().is_empty() {
            "public"
        } else {
            rel.visibility.as_str()
        };
        self.conn.execute(
            "INSERT INTO relations
             (source_id, source_kind, relation_type, target_id, target_kind, strength, notes, updated_at_world_time,
              dimensions_json, direction, visibility, origin_event_id, obligations_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(source_id, relation_type, target_id) DO UPDATE SET
               source_kind = excluded.source_kind,
               target_kind = excluded.target_kind,
               strength = excluded.strength,
               notes = excluded.notes,
               updated_at_world_time = excluded.updated_at_world_time,
               dimensions_json = excluded.dimensions_json,
               direction = excluded.direction,
               visibility = excluded.visibility,
               origin_event_id = COALESCE(excluded.origin_event_id, relations.origin_event_id),
               obligations_json = excluded.obligations_json",
            params![
                rel.source_id,
                rel.source_kind,
                rel.relation_type,
                rel.target_id,
                rel.target_kind,
                rel.strength,
                rel.notes,
                rel.updated_at_world_time,
                dims_json,
                direction,
                visibility,
                rel.origin_event_id,
                ob_json,
            ],
        )?;
        Ok(())
    }

    pub fn delete_relation(
        &self,
        source_id: &str,
        relation_type: &str,
        target_id: &str,
    ) -> Result<bool> {
        let changed = self.conn.execute(
            "DELETE FROM relations WHERE source_id = ?1 AND relation_type = ?2 AND target_id = ?3",
            params![source_id, relation_type, target_id],
        )?;
        Ok(changed > 0)
    }

    pub fn list_relations_for_entity(&self, entity_id: &str) -> Result<Vec<GraphRelation>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_id, source_kind, relation_type, target_id, target_kind, strength, notes, updated_at_world_time,
                    dimensions_json, direction, visibility, origin_event_id, obligations_json
             FROM relations
             WHERE source_id = ?1 OR target_id = ?1
             ORDER BY relation_type, source_id, target_id",
        )?;
        let rows = stmt.query_map(params![entity_id], row_to_relation)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn list_all_relations(&self) -> Result<Vec<GraphRelation>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_id, source_kind, relation_type, target_id, target_kind, strength, notes, updated_at_world_time,
                    dimensions_json, direction, visibility, origin_event_id, obligations_json
             FROM relations
             ORDER BY source_id, relation_type, target_id",
        )?;
        let rows = stmt.query_map([], row_to_relation)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn upsert_clock(&self, clock: &ProgressClock) -> Result<()> {
        let related_json = serde_json::to_string(&clock.related_ids)?;
        let cadence_i64 = clock.cadence_hours.map(|h| h as i64);
        let auto_reset_int: i64 = if clock.auto_reset { 1 } else { 0 };
        self.conn.execute(
            "INSERT INTO clocks (id, name, kind, filled, segments, status, description, related_ids_json,
                                 cadence_hours, last_tick_total_hours, consequence_on_fill, auto_reset)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               kind = excluded.kind,
               filled = excluded.filled,
               segments = excluded.segments,
               status = excluded.status,
               description = excluded.description,
               related_ids_json = excluded.related_ids_json,
               cadence_hours = excluded.cadence_hours,
               last_tick_total_hours = excluded.last_tick_total_hours,
               consequence_on_fill = excluded.consequence_on_fill,
               auto_reset = excluded.auto_reset",
            params![
                clock.id,
                clock.name,
                clock.kind,
                clock.filled,
                clock.segments,
                clock.status,
                clock.description,
                related_json,
                cadence_i64,
                clock.last_tick_total_hours,
                clock.consequence_on_fill,
                auto_reset_int,
            ],
        )?;
        Ok(())
    }

    pub fn get_clock(&self, id: &str) -> Result<Option<ProgressClock>> {
        self.conn
            .query_row(
                "SELECT id, name, kind, filled, segments, status, description, related_ids_json,
                        cadence_hours, last_tick_total_hours, consequence_on_fill, auto_reset
                 FROM clocks WHERE id = ?1",
                params![id],
                row_to_clock,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_clocks(&self) -> Result<Vec<ProgressClock>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, kind, filled, segments, status, description, related_ids_json,
                    cadence_hours, last_tick_total_hours, consequence_on_fill, auto_reset
             FROM clocks ORDER BY status, kind, name",
        )?;
        let rows = stmt.query_map([], row_to_clock)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // -------------------------------------------------------------------------
    // Active Engine Signal Bus & Standing Character Routines
    // -------------------------------------------------------------------------

    pub fn upsert_signal(&self, sig: &EngineSignal) -> Result<()> {
        let json = serde_json::to_string(sig)?;
        // Deduplicate by dedup_key if an existing row already has this dedup_key
        if !sig.dedup_key.is_empty() {
            self.conn.execute(
                "DELETE FROM engine_signals WHERE dedup_key = ?1 AND id != ?2",
                params![sig.dedup_key, sig.id],
            )?;
        }
        self.conn.execute(
            "INSERT INTO engine_signals (id, dedup_key, code, severity, category, status, visibility, signal_json, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
               dedup_key = excluded.dedup_key,
               code = excluded.code,
               severity = excluded.severity,
               category = excluded.category,
               status = excluded.status,
               visibility = excluded.visibility,
               signal_json = excluded.signal_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                sig.id,
                sig.dedup_key,
                sig.code,
                sig.severity,
                sig.category,
                sig.status,
                sig.visibility,
                json,
                sig.updated_at_world_time,
            ],
        )?;
        Ok(())
    }

    pub fn get_signal(&self, id_or_dedup: &str) -> Result<Option<EngineSignal>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT signal_json FROM engine_signals WHERE id = ?1 OR dedup_key = ?1 LIMIT 1",
                params![id_or_dedup],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_signals(&self, status_filter: Option<&str>) -> Result<Vec<EngineSignal>> {
        let mut out = Vec::new();
        if let Some(st) = status_filter.filter(|s| !s.trim().is_empty()) {
            let mut stmt = self.conn.prepare(
                "SELECT signal_json FROM engine_signals WHERE status = ?1 ORDER BY updated_at_world_time DESC, id ASC",
            )?;
            let rows = stmt.query_map(params![st], |r| r.get::<_, String>(0))?;
            for r in rows {
                out.push(serde_json::from_str(&r?)?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT signal_json FROM engine_signals ORDER BY updated_at_world_time DESC, id ASC",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for r in rows {
                out.push(serde_json::from_str(&r?)?);
            }
        }
        Ok(out)
    }

    pub fn update_signal_status(
        &self,
        id_or_dedup: &str,
        new_status: &str,
        world_time: &str,
    ) -> Result<Option<EngineSignal>> {
        if let Some(mut sig) = self.get_signal(id_or_dedup)? {
            sig.status = new_status.to_string();
            sig.updated_at_world_time = world_time.to_string();
            self.upsert_signal(&sig)?;
            Ok(Some(sig))
        } else {
            Ok(None)
        }
    }

    pub fn delete_signal(&self, id_or_dedup: &str) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM engine_signals WHERE id = ?1 OR dedup_key = ?1",
            params![id_or_dedup],
        )?;
        Ok(n > 0)
    }

    pub fn upsert_routine(&self, routine: &CharacterRoutine) -> Result<()> {
        let json = serde_json::to_string(routine)?;
        self.conn.execute(
            "INSERT INTO character_routines (id, character_id, enabled, routine_json, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               character_id = excluded.character_id,
               enabled = excluded.enabled,
               routine_json = excluded.routine_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                routine.id,
                routine.character_id,
                if routine.enabled { 1 } else { 0 },
                json,
                routine.updated_at_world_time,
            ],
        )?;
        Ok(())
    }

    pub fn get_routine(&self, id: &str) -> Result<Option<CharacterRoutine>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT routine_json FROM character_routines WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_routines(&self, character_id: Option<&str>) -> Result<Vec<CharacterRoutine>> {
        let mut out = Vec::new();
        if let Some(cid) = character_id.filter(|s| !s.trim().is_empty()) {
            let mut stmt = self.conn.prepare(
                "SELECT routine_json FROM character_routines WHERE character_id = ?1 ORDER BY id ASC",
            )?;
            let rows = stmt.query_map(params![cid], |r| r.get::<_, String>(0))?;
            for r in rows {
                out.push(serde_json::from_str(&r?)?);
            }
        } else {
            let mut stmt = self
                .conn
                .prepare("SELECT routine_json FROM character_routines ORDER BY character_id ASC, id ASC")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for r in rows {
                out.push(serde_json::from_str(&r?)?);
            }
        }
        Ok(out)
    }

    pub fn delete_routine(&self, id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM character_routines WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    // -------------------------------------------------------------------------
    // Phase 1: Ruleset Profiles, Random Tables, Decks, Staged Discoveries & Encounters
    // -------------------------------------------------------------------------

    pub fn upsert_ruleset_profile(&self, profile: &RulesetProfile, world_time: &str) -> Result<()> {
        let json = serde_json::to_string(profile)?;
        self.conn.execute(
            "INSERT INTO ruleset_profiles (id, name, resolution_mode, profile_json, version, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               resolution_mode = excluded.resolution_mode,
               profile_json = excluded.profile_json,
               version = excluded.version,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                profile.id,
                profile.name,
                profile.resolution_mode,
                json,
                profile.version,
                world_time,
            ],
        )?;
        Ok(())
    }

    pub fn get_ruleset_profile(&self, id: &str) -> Result<Option<RulesetProfile>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT profile_json FROM ruleset_profiles WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_ruleset_profiles(&self) -> Result<Vec<RulesetProfile>> {
        let mut stmt = self
            .conn
            .prepare("SELECT profile_json FROM ruleset_profiles ORDER BY id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn delete_ruleset_profile(&self, id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM ruleset_profiles WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn upsert_random_table(&self, table: &RandomTable) -> Result<()> {
        let json = serde_json::to_string(table)?;
        self.conn.execute(
            "INSERT INTO random_tables
             (id, name, category, is_loot_table, version, table_json, created_at_world_time, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               category = excluded.category,
               is_loot_table = excluded.is_loot_table,
               version = excluded.version,
               table_json = excluded.table_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                table.id,
                table.name,
                table.category,
                if table.is_loot_table { 1 } else { 0 },
                table.version,
                json,
                table.created_at_world_time,
                table.updated_at_world_time,
            ],
        )?;
        Ok(())
    }

    pub fn get_random_table(&self, id: &str) -> Result<Option<RandomTable>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT table_json FROM random_tables WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_random_tables(&self) -> Result<Vec<RandomTable>> {
        let mut stmt = self
            .conn
            .prepare("SELECT table_json FROM random_tables ORDER BY category, name")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn delete_random_table(&self, id: &str) -> Result<bool> {
        let _ = self
            .conn
            .execute("DELETE FROM table_draw_state WHERE table_id = ?1", params![id])?;
        let n = self
            .conn
            .execute("DELETE FROM random_tables WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn get_table_drawn_ids(
        &self,
        table_id: &str,
        deck_scope: &str,
    ) -> Result<std::collections::BTreeSet<String>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT drawn_ids_json FROM table_draw_state WHERE table_id = ?1 AND deck_scope = ?2",
                params![table_id, deck_scope],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(serde_json::from_str(&s).unwrap_or_default()),
            None => Ok(std::collections::BTreeSet::new()),
        }
    }

    pub fn set_table_drawn_ids(
        &self,
        table_id: &str,
        deck_scope: &str,
        drawn: &std::collections::BTreeSet<String>,
        world_time: &str,
    ) -> Result<()> {
        let json = serde_json::to_string(drawn)?;
        self.conn.execute(
            "INSERT INTO table_draw_state (table_id, deck_scope, drawn_ids_json, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(table_id, deck_scope) DO UPDATE SET
               drawn_ids_json = excluded.drawn_ids_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![table_id, deck_scope, json, world_time],
        )?;
        Ok(())
    }

    pub fn upsert_staged_discovery(&self, disc: &StagedDiscovery) -> Result<()> {
        let json = serde_json::to_string(disc)?;
        self.conn.execute(
            "INSERT INTO staged_discoveries (id, source_table_id, location_id, status, discovery_json, created_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
               status = excluded.status,
               discovery_json = excluded.discovery_json",
            params![
                disc.id,
                disc.source_table_id,
                disc.location_id,
                disc.status,
                json,
                disc.created_at_world_time,
            ],
        )?;
        Ok(())
    }

    pub fn get_staged_discovery(&self, id: &str) -> Result<Option<StagedDiscovery>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT discovery_json FROM staged_discoveries WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_staged_discoveries(&self, status_filter: Option<&str>) -> Result<Vec<StagedDiscovery>> {
        let mut out = Vec::new();
        if let Some(st) = status_filter {
            let mut stmt = self.conn.prepare(
                "SELECT discovery_json FROM staged_discoveries WHERE status = ?1 ORDER BY rowid DESC",
            )?;
            let rows = stmt.query_map(params![st], |r| r.get::<_, String>(0))?;
            for r in rows {
                out.push(serde_json::from_str(&r?)?);
            }
        } else {
            let mut stmt = self
                .conn
                .prepare("SELECT discovery_json FROM staged_discoveries ORDER BY rowid DESC")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for r in rows {
                out.push(serde_json::from_str(&r?)?);
            }
        }
        Ok(out)
    }

    pub fn delete_staged_discovery(&self, id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM staged_discoveries WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn upsert_encounter(&self, enc: &EncounterState) -> Result<()> {
        let json = serde_json::to_string(enc)?;
        self.conn.execute(
            "INSERT INTO encounters (id, title, encounter_type, location_id, status, encounter_json, started_at_world_time, ended_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
               title = excluded.title,
               encounter_type = excluded.encounter_type,
               location_id = excluded.location_id,
               status = excluded.status,
               encounter_json = excluded.encounter_json,
               ended_at_world_time = excluded.ended_at_world_time",
            params![
                enc.id,
                enc.title,
                enc.encounter_type,
                enc.location_id,
                enc.status,
                json,
                enc.started_at_world_time,
                enc.ended_at_world_time,
            ],
        )?;
        Ok(())
    }

    pub fn get_encounter(&self, id: &str) -> Result<Option<EncounterState>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT encounter_json FROM encounters WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_encounters(&self, status_filter: Option<&str>) -> Result<Vec<EncounterState>> {
        let mut out = Vec::new();
        if let Some(st) = status_filter {
            let mut stmt = self.conn.prepare(
                "SELECT encounter_json FROM encounters WHERE status = ?1 ORDER BY rowid DESC",
            )?;
            let rows = stmt.query_map(params![st], |r| r.get::<_, String>(0))?;
            for r in rows {
                out.push(serde_json::from_str(&r?)?);
            }
        } else {
            let mut stmt = self
                .conn
                .prepare("SELECT encounter_json FROM encounters ORDER BY rowid DESC")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for r in rows {
                out.push(serde_json::from_str(&r?)?);
            }
        }
        Ok(out)
    }

    pub fn delete_encounter(&self, id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM encounters WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    // -------------------------------------------------------------------------
    // Phase 2: Extended Character Profiles, Epistemic Knowledge & Factions
    // -------------------------------------------------------------------------

    pub fn upsert_character_profile(&self, profile: &CharacterProfileExt) -> Result<()> {
        let json = serde_json::to_string(profile)?;
        self.conn.execute(
            "INSERT INTO character_profiles (character_id, profile_json, updated_at_world_time)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(character_id) DO UPDATE SET
               profile_json = excluded.profile_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![profile.character_id, json, profile.updated_at_world_time],
        )?;
        Ok(())
    }

    pub fn get_character_profile(&self, character_id: &str) -> Result<Option<CharacterProfileExt>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT profile_json FROM character_profiles WHERE character_id = ?1",
                params![character_id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn upsert_knowledge_claim(&self, claim: &KnowledgeClaim) -> Result<()> {
        let json = serde_json::to_string(claim)?;
        self.conn.execute(
            "INSERT INTO knowledge_claims
             (id, subject, claim, truth_status, visibility, claim_json, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
               subject = excluded.subject,
               claim = excluded.claim,
               truth_status = excluded.truth_status,
               visibility = excluded.visibility,
               claim_json = excluded.claim_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                claim.id,
                claim.subject,
                claim.claim,
                claim.truth_status,
                claim.visibility,
                json,
                claim.updated_at_world_time,
            ],
        )?;
        self.conn
            .execute("DELETE FROM knowledge_claims_fts WHERE id = ?1", params![claim.id])?;
        self.conn.execute(
            "INSERT INTO knowledge_claims_fts (id, subject, claim, tags) VALUES (?1, ?2, ?3, ?4)",
            params![claim.id, claim.subject, claim.claim, claim.tags.join(" ")],
        )?;
        Ok(())
    }

    pub fn get_knowledge_claim(&self, id: &str) -> Result<Option<KnowledgeClaim>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT claim_json FROM knowledge_claims WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_knowledge_claims(&self) -> Result<Vec<KnowledgeClaim>> {
        let mut stmt = self
            .conn
            .prepare("SELECT claim_json FROM knowledge_claims ORDER BY rowid DESC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn search_knowledge_claims_fts(&self, query: &str, limit: usize) -> Result<Vec<KnowledgeClaim>> {
        let fts_q = sanitize_fts_query(query);
        if fts_q.is_empty() {
            return Ok(vec![]);
        }
        let mut stmt = self.conn.prepare(
            "SELECT k.claim_json
             FROM knowledge_claims_fts f
             JOIN knowledge_claims k ON k.id = f.id
             WHERE knowledge_claims_fts MATCH ?1
             ORDER BY bm25(knowledge_claims_fts)
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![fts_q, limit as i64], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn delete_knowledge_claim(&self, id: &str) -> Result<bool> {
        self.conn
            .execute("DELETE FROM knowledge_claims_fts WHERE id = ?1", params![id])?;
        let n = self
            .conn
            .execute("DELETE FROM knowledge_claims WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn upsert_actor_belief(&self, belief: &ActorBelief) -> Result<()> {
        let json = serde_json::to_string(belief)?;
        self.conn.execute(
            "INSERT INTO actor_beliefs (claim_id, actor_id, stance, confidence, belief_json)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(claim_id, actor_id) DO UPDATE SET
               stance = excluded.stance,
               confidence = excluded.confidence,
               belief_json = excluded.belief_json",
            params![
                belief.claim_id,
                belief.actor_id,
                belief.stance,
                belief.confidence as i64,
                json,
            ],
        )?;
        Ok(())
    }

    pub fn list_beliefs_for_claim(&self, claim_id: &str) -> Result<Vec<ActorBelief>> {
        let mut stmt = self.conn.prepare(
            "SELECT belief_json FROM actor_beliefs WHERE claim_id = ?1 ORDER BY actor_id",
        )?;
        let rows = stmt.query_map(params![claim_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn list_beliefs_for_actor(&self, actor_id: &str) -> Result<Vec<ActorBelief>> {
        let mut stmt = self.conn.prepare(
            "SELECT belief_json FROM actor_beliefs WHERE actor_id = ?1 ORDER BY rowid DESC",
        )?;
        let rows = stmt.query_map(params![actor_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn get_actor_belief(&self, claim_id: &str, actor_id: &str) -> Result<Option<ActorBelief>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT belief_json FROM actor_beliefs WHERE claim_id = ?1 AND actor_id = ?2",
                params![claim_id, actor_id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn upsert_faction(&self, faction: &FactionRecord) -> Result<()> {
        let json = serde_json::to_string(faction)?;
        self.conn.execute(
            "INSERT INTO factions (id, name, faction_type, faction_json, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               faction_type = excluded.faction_type,
               faction_json = excluded.faction_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                faction.id,
                faction.name,
                faction.faction_type,
                json,
                faction.updated_at_world_time,
            ],
        )?;
        self.conn
            .execute("DELETE FROM factions_fts WHERE id = ?1", params![faction.id])?;
        self.conn.execute(
            "INSERT INTO factions_fts (id, name, faction_type, ideology, origins, notes, tags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                faction.id,
                faction.name,
                faction.faction_type,
                faction.ideology,
                faction.origins,
                faction.notes,
                faction.tags.join(" "),
            ],
        )?;
        Ok(())
    }

    pub fn get_faction(&self, id: &str) -> Result<Option<FactionRecord>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT faction_json FROM factions WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_factions(&self) -> Result<Vec<FactionRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT faction_json FROM factions ORDER BY faction_type, name")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn delete_faction(&self, id: &str) -> Result<bool> {
        self.conn
            .execute("DELETE FROM factions_fts WHERE id = ?1", params![id])?;
        let n = self
            .conn
            .execute("DELETE FROM factions WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    // -------------------------------------------------------------------------
    // Phase 3 & Phase 4: Geography, Economy, Law/Reputation, Health & Crafting
    // -------------------------------------------------------------------------

    pub fn upsert_place_spatial(&self, sp: &PlaceSpatialExt) -> Result<()> {
        let json = serde_json::to_string(sp)?;
        self.conn.execute(
            "INSERT INTO place_spatial (place_id, spatial_json, updated_at_world_time)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(place_id) DO UPDATE SET
               spatial_json = excluded.spatial_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![sp.place_id, json, sp.updated_at_world_time],
        )?;
        Ok(())
    }

    pub fn get_place_spatial(&self, place_id: &str) -> Result<Option<PlaceSpatialExt>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT spatial_json FROM place_spatial WHERE place_id = ?1",
                params![place_id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn upsert_travel_route(&self, route: &TravelRoute) -> Result<()> {
        let json = serde_json::to_string(route)?;
        self.conn.execute(
            "INSERT INTO travel_routes (id, from_place_id, to_place_id, status, route_json, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
               from_place_id = excluded.from_place_id,
               to_place_id = excluded.to_place_id,
               status = excluded.status,
               route_json = excluded.route_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                route.id,
                route.from_place_id,
                route.to_place_id,
                route.status,
                json,
                route.updated_at_world_time,
            ],
        )?;
        Ok(())
    }

    pub fn get_travel_route(&self, id: &str) -> Result<Option<TravelRoute>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT route_json FROM travel_routes WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_travel_routes(&self) -> Result<Vec<TravelRoute>> {
        let mut stmt = self
            .conn
            .prepare("SELECT route_json FROM travel_routes ORDER BY id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn upsert_item_economics(&self, econ: &ItemEconomics) -> Result<()> {
        let json = serde_json::to_string(econ)?;
        self.conn.execute(
            "INSERT INTO item_economics (item_key, econ_json) VALUES (?1, ?2)
             ON CONFLICT(item_key) DO UPDATE SET econ_json = excluded.econ_json",
            params![econ.item_key, json],
        )?;
        Ok(())
    }

    pub fn list_item_economics_map(&self) -> Result<std::collections::BTreeMap<String, ItemEconomics>> {
        let mut stmt = self
            .conn
            .prepare("SELECT item_key, econ_json FROM item_economics")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut map = std::collections::BTreeMap::new();
        for r in rows {
            let (k, raw) = r?;
            map.insert(k, serde_json::from_str(&raw)?);
        }
        Ok(map)
    }

    pub fn get_currency_balance(&self, owner_id: &str, currency: &str) -> Result<i64> {
        let val: Option<i64> = self
            .conn
            .query_row(
                "SELECT amount FROM currency_balances WHERE owner_id = ?1 AND currency = ?2",
                params![owner_id, currency],
                |r| r.get(0),
            )
            .optional()?;
        Ok(val.unwrap_or(0))
    }

    pub fn set_currency_balance(
        &self,
        owner_id: &str,
        currency: &str,
        amount: i64,
        world_time: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO currency_balances (owner_id, currency, amount, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(owner_id, currency) DO UPDATE SET
               amount = excluded.amount,
               updated_at_world_time = excluded.updated_at_world_time",
            params![owner_id, currency, amount, world_time],
        )?;
        Ok(())
    }

    pub fn list_currency_balances(
        &self,
        owner_id: &str,
    ) -> Result<std::collections::BTreeMap<String, i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT currency, amount FROM currency_balances WHERE owner_id = ?1 ORDER BY currency",
        )?;
        let rows = stmt.query_map(params![owner_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })?;
        let mut map = std::collections::BTreeMap::new();
        for r in rows {
            let (k, v) = r?;
            map.insert(k, v);
        }
        Ok(map)
    }

    pub fn upsert_market(&self, market: &SettlementMarket) -> Result<()> {
        let json = serde_json::to_string(market)?;
        self.conn.execute(
            "INSERT INTO markets (place_id, name, market_json, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(place_id) DO UPDATE SET
               name = excluded.name,
               market_json = excluded.market_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![market.place_id, market.name, json, market.updated_at_world_time],
        )?;
        Ok(())
    }

    pub fn get_market(&self, place_id: &str) -> Result<Option<SettlementMarket>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT market_json FROM markets WHERE place_id = ?1",
                params![place_id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_markets(&self) -> Result<Vec<SettlementMarket>> {
        let mut stmt = self
            .conn
            .prepare("SELECT market_json FROM markets ORDER BY name")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn upsert_jurisdiction(&self, j: &JurisdictionRecord) -> Result<()> {
        let json = serde_json::to_string(j)?;
        self.conn.execute(
            "INSERT INTO jurisdictions (id, name, jurisdiction_json, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               jurisdiction_json = excluded.jurisdiction_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![j.id, j.name, json, j.updated_at_world_time],
        )?;
        Ok(())
    }

    pub fn list_jurisdictions(&self) -> Result<Vec<JurisdictionRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT jurisdiction_json FROM jurisdictions ORDER BY name")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn upsert_crime(&self, c: &CrimeRecord) -> Result<()> {
        let json = serde_json::to_string(c)?;
        self.conn.execute(
            "INSERT INTO crime_records (id, jurisdiction_id, suspect_id, warrant_status, crime_json, recorded_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
               warrant_status = excluded.warrant_status,
               crime_json = excluded.crime_json",
            params![
                c.id,
                c.jurisdiction_id,
                c.suspect_id,
                c.warrant_status,
                json,
                c.recorded_at_world_time,
            ],
        )?;
        Ok(())
    }

    pub fn get_crime(&self, id: &str) -> Result<Option<CrimeRecord>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT crime_json FROM crime_records WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_crimes(&self, suspect_id: Option<&str>, jurisdiction_id: Option<&str>) -> Result<Vec<CrimeRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT crime_json FROM crime_records ORDER BY rowid DESC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            let c: CrimeRecord = serde_json::from_str(&r?)?;
            if let Some(s) = suspect_id {
                if c.suspect_id != s {
                    continue;
                }
            }
            if let Some(j) = jurisdiction_id {
                if c.jurisdiction_id != j {
                    continue;
                }
            }
            out.push(c);
        }
        Ok(out)
    }

    pub fn upsert_reputation(&self, rep: &ReputationStanding) -> Result<()> {
        let json = serde_json::to_string(rep)?;
        self.conn.execute(
            "INSERT INTO reputation_scores (entity_id, target_group_id, score, rep_json, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(entity_id, target_group_id) DO UPDATE SET
               score = excluded.score,
               rep_json = excluded.rep_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                rep.entity_id,
                rep.target_group_id,
                rep.score,
                json,
                rep.updated_at_world_time,
            ],
        )?;
        Ok(())
    }

    pub fn list_reputations(&self, entity_id: Option<&str>, target_group_id: Option<&str>) -> Result<Vec<ReputationStanding>> {
        let mut stmt = self
            .conn
            .prepare("SELECT rep_json FROM reputation_scores ORDER BY entity_id, target_group_id")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            let rep: ReputationStanding = serde_json::from_str(&r?)?;
            if let Some(e) = entity_id {
                if rep.entity_id != e {
                    continue;
                }
            }
            if let Some(t) = target_group_id {
                if rep.target_group_id != t {
                    continue;
                }
            }
            out.push(rep);
        }
        Ok(out)
    }

    pub fn upsert_injury(&self, inj: &InjuryRecord) -> Result<()> {
        let json = serde_json::to_string(inj)?;
        self.conn.execute(
            "INSERT INTO injuries (id, character_id, status, injury_json, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               status = excluded.status,
               injury_json = excluded.injury_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![inj.id, inj.character_id, inj.status, json, inj.updated_at_world_time],
        )?;
        Ok(())
    }

    pub fn get_injury(&self, id: &str) -> Result<Option<InjuryRecord>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT injury_json FROM injuries WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_injuries(&self, character_id: Option<&str>, active_only: bool) -> Result<Vec<InjuryRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT injury_json FROM injuries ORDER BY rowid DESC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            let inj: InjuryRecord = serde_json::from_str(&r?)?;
            if let Some(cid) = character_id {
                if inj.character_id != cid {
                    continue;
                }
            }
            if active_only && inj.status == "healed" {
                continue;
            }
            out.push(inj);
        }
        Ok(out)
    }

    pub fn upsert_crafting_project(&self, proj: &CraftingProject) -> Result<()> {
        let json = serde_json::to_string(proj)?;
        self.conn.execute(
            "INSERT INTO crafting_projects (id, crafter_id, status, project_json)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET
               status = excluded.status,
               project_json = excluded.project_json",
            params![proj.id, proj.crafter_id, proj.status, json],
        )?;
        Ok(())
    }

    pub fn get_crafting_project(&self, id: &str) -> Result<Option<CraftingProject>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT project_json FROM crafting_projects WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    // -------------------------------------------------------------------------
    // Direct Edit, Delete & Rename Helpers (Diagnostics / Maintenance Layer)
    // -------------------------------------------------------------------------

    pub fn update_character_baseline(&self, baseline: &CharacterBaseline) -> Result<()> {
        let json = serde_json::to_string(baseline)?;
        let changed = self.conn.execute(
            "UPDATE character_baselines
             SET name = ?2, kind = ?3, baseline_json = ?4
             WHERE id = ?1",
            params![baseline.id, baseline.name, baseline.kind, json],
        )?;
        if changed == 0 {
            bail!("Character '{}' not found", baseline.id);
        }
        Ok(())
    }

    pub fn rename_character_baseline_id(
        &self,
        old_id: &str,
        new_id: &str,
        updated: &CharacterBaseline,
    ) -> Result<()> {
        let json = serde_json::to_string(updated)?;
        let changed = self.conn.execute(
            "UPDATE character_baselines
             SET id = ?2, name = ?3, kind = ?4, baseline_json = ?5
             WHERE id = ?1",
            params![old_id, new_id, updated.name, updated.kind, json],
        )?;
        if changed == 0 {
            bail!("Character '{}' not found", old_id);
        }
        Ok(())
    }

    pub fn delete_character_baseline(&self, id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM character_baselines WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn update_place_baseline(&self, baseline: &PlaceBaseline) -> Result<()> {
        let json = serde_json::to_string(baseline)?;
        let changed = self.conn.execute(
            "UPDATE place_baselines
             SET name = ?2, place_type = ?3, parent_place = ?4, baseline_json = ?5
             WHERE id = ?1",
            params![
                baseline.id,
                baseline.name,
                baseline.place_type,
                baseline.parent_place,
                json
            ],
        )?;
        if changed == 0 {
            bail!("Place '{}' not found", baseline.id);
        }
        Ok(())
    }

    pub fn rename_place_baseline_id(
        &self,
        old_id: &str,
        new_id: &str,
        updated: &PlaceBaseline,
    ) -> Result<()> {
        let json = serde_json::to_string(updated)?;
        let changed = self.conn.execute(
            "UPDATE place_baselines
             SET id = ?2, name = ?3, place_type = ?4, parent_place = ?5, baseline_json = ?6
             WHERE id = ?1",
            params![
                old_id,
                new_id,
                updated.name,
                updated.place_type,
                updated.parent_place,
                json
            ],
        )?;
        if changed == 0 {
            bail!("Place '{}' not found", old_id);
        }
        Ok(())
    }

    pub fn delete_place_baseline(&self, id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM place_baselines WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn delete_lore(&self, id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM lore_entries WHERE id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM lore_fts WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    // -------------------------------------------------------------------------
    // Stateful Progression Ladders & Character Parallel Progression Tracks
    // -------------------------------------------------------------------------

    pub fn upsert_progression_ladder(&self, ladder: &ProgressionLadder) -> Result<()> {
        let json = serde_json::to_string(ladder)?;
        self.conn.execute(
            "INSERT INTO progression_ladders (id, name, kind, ladder_json, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               kind = excluded.kind,
               ladder_json = excluded.ladder_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                ladder.id,
                ladder.name,
                ladder.kind,
                json,
                ladder.updated_at_world_time
            ],
        )?;
        Ok(())
    }

    pub fn get_progression_ladder(&self, id: &str) -> Result<Option<ProgressionLadder>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT ladder_json FROM progression_ladders WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_progression_ladders(&self) -> Result<Vec<ProgressionLadder>> {
        let mut stmt = self
            .conn
            .prepare("SELECT ladder_json FROM progression_ladders ORDER BY id ASC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn upsert_character_progression(&self, track: &CharacterProgressionTrack) -> Result<()> {
        let json = serde_json::to_string(track)?;
        self.conn.execute(
            "INSERT INTO character_progression (character_id, track_id, track_kind, status, track_json, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(character_id, track_id) DO UPDATE SET
               track_kind = excluded.track_kind,
               status = excluded.status,
               track_json = excluded.track_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                track.character_id,
                track.track_id,
                track.track_kind,
                track.status,
                json,
                track.updated_at_world_time
            ],
        )?;
        Ok(())
    }

    pub fn get_character_progression(
        &self,
        character_id: &str,
        track_id: &str,
    ) -> Result<Option<CharacterProgressionTrack>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT track_json FROM character_progression WHERE character_id = ?1 AND track_id = ?2",
                params![character_id, track_id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_character_progressions(
        &self,
        character_id: &str,
    ) -> Result<Vec<CharacterProgressionTrack>> {
        let mut stmt = self.conn.prepare(
            "SELECT track_json FROM character_progression WHERE character_id = ?1 ORDER BY track_kind ASC, track_id ASC",
        )?;
        let rows = stmt.query_map(params![character_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn list_all_progressions(&self) -> Result<Vec<CharacterProgressionTrack>> {
        let mut stmt = self.conn.prepare(
            "SELECT track_json FROM character_progression ORDER BY character_id ASC, track_kind ASC, track_id ASC",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn delete_character_progression(&self, character_id: &str, track_id: &str) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM character_progression WHERE character_id = ?1 AND track_id = ?2",
            params![character_id, track_id],
        )?;
        Ok(n > 0)
    }

    // -------------------------------------------------------------------------
    // Sparse Multi-Scale 2D (x, y) Map Grids & Cells
    // -------------------------------------------------------------------------

    pub fn upsert_map_grid(&self, grid: &MapGrid) -> Result<()> {
        let json = serde_json::to_string(grid)?;
        self.conn.execute(
            "INSERT INTO map_grids (id, name, scale_kind, grid_json, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               name = excluded.name,
               scale_kind = excluded.scale_kind,
               grid_json = excluded.grid_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                grid.id,
                grid.name,
                grid.scale_kind,
                json,
                grid.updated_at_world_time
            ],
        )?;
        Ok(())
    }

    pub fn get_map_grid(&self, id: &str) -> Result<Option<MapGrid>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT grid_json FROM map_grids WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_map_grids(&self) -> Result<Vec<MapGrid>> {
        let mut stmt = self
            .conn
            .prepare("SELECT grid_json FROM map_grids ORDER BY scale_kind ASC, id ASC")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn delete_map_grid(&self, id: &str) -> Result<bool> {
        self.conn
            .execute("DELETE FROM map_grid_cells WHERE grid_id = ?1", params![id])?;
        let n = self
            .conn
            .execute("DELETE FROM map_grids WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn upsert_map_cell(&self, cell: &MapGridCell) -> Result<()> {
        let json = serde_json::to_string(cell)?;
        self.conn.execute(
            "INSERT INTO map_grid_cells (grid_id, x, y, place_id, cell_json, updated_at_world_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(grid_id, x, y) DO UPDATE SET
               place_id = excluded.place_id,
               cell_json = excluded.cell_json,
               updated_at_world_time = excluded.updated_at_world_time",
            params![
                cell.grid_id,
                cell.x,
                cell.y,
                cell.place_id,
                json,
                cell.updated_at_world_time
            ],
        )?;
        Ok(())
    }

    pub fn get_map_cell(&self, grid_id: &str, x: i32, y: i32) -> Result<Option<MapGridCell>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT cell_json FROM map_grid_cells WHERE grid_id = ?1 AND x = ?2 AND y = ?3",
                params![grid_id, x, y],
                |r| r.get(0),
            )
            .optional()?;
        match raw {
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
            None => Ok(None),
        }
    }

    pub fn list_map_cells(&self, grid_id: &str) -> Result<Vec<MapGridCell>> {
        let mut stmt = self.conn.prepare(
            "SELECT cell_json FROM map_grid_cells WHERE grid_id = ?1 ORDER BY y ASC, x ASC",
        )?;
        let rows = stmt.query_map(params![grid_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn find_map_cells_for_place(&self, place_id: &str) -> Result<Vec<MapGridCell>> {
        let mut stmt = self.conn.prepare(
            "SELECT cell_json FROM map_grid_cells WHERE place_id = ?1 ORDER BY grid_id ASC",
        )?;
        let rows = stmt.query_map(params![place_id], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(serde_json::from_str(&r?)?);
        }
        Ok(out)
    }

    pub fn delete_map_cell(&self, grid_id: &str, x: i32, y: i32) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM map_grid_cells WHERE grid_id = ?1 AND x = ?2 AND y = ?3",
            params![grid_id, x, y],
        )?;
        Ok(n > 0)
    }

    pub fn delete_text(&self, id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM story_texts WHERE id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM story_texts_fts WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn delete_catalog_item(&self, id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM catalog_items WHERE id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM catalog_items_fts WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn get_entity_history_by_id(&self, id: i64) -> Result<Option<EntityHistoryEntry>> {
        self.conn
            .query_row(
                "SELECT id, entity_id, entity_kind, era_or_date, sort_order, title, details,
                        significance, related_ids_json, visibility, recorded_at_world_time
                 FROM entity_histories WHERE id = ?1",
                params![id],
                row_to_entity_history,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn update_entity_history(&self, entry: &EntityHistoryEntry) -> Result<()> {
        let rel_json = serde_json::to_string(&entry.related_ids)?;
        let changed = self.conn.execute(
            "UPDATE entity_histories
             SET entity_id = ?2, entity_kind = ?3, era_or_date = ?4, sort_order = ?5,
                 title = ?6, details = ?7, significance = ?8, related_ids_json = ?9, visibility = ?10
             WHERE id = ?1",
            params![
                entry.id,
                entry.entity_id,
                entry.entity_kind,
                entry.era_or_date,
                entry.sort_order,
                entry.title,
                entry.details,
                entry.significance,
                rel_json,
                entry.visibility,
            ],
        )?;
        if changed == 0 {
            bail!("Entity history #{} not found", entry.id);
        }
        self.conn.execute(
            "DELETE FROM entity_histories_fts WHERE id = ?1",
            params![entry.id],
        )?;
        self.conn.execute(
            "INSERT INTO entity_histories_fts (id, entity_id, entity_kind, era_or_date, title, details, significance)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                entry.id,
                entry.entity_id,
                entry.entity_kind,
                entry.era_or_date,
                entry.title,
                entry.details,
                entry.significance,
            ],
        )?;
        Ok(())
    }

    pub fn delete_entity_history(&self, id: i64) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM entity_histories WHERE id = ?1", params![id])?;
        self.conn.execute(
            "DELETE FROM entity_histories_fts WHERE id = ?1",
            params![id],
        )?;
        Ok(n > 0)
    }

    pub fn delete_attachment(&self, id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM attachments WHERE id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM attachments_fts WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    pub fn delete_clock(&self, id: &str) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM clocks WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// Rewinds the campaign database to event sequence `to_seq`, creating a timestamped backup first
    /// and reconstructing projected tables (relations, clocks, world clock, baselines) from events 1..=to_seq.
    pub fn rollback_to_seq(&self, to_seq: i64) -> Result<PathBuf> {
        let max_seq: i64 = self
            .conn
            .query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))?;
        if to_seq < 1 || to_seq > max_seq {
            bail!("Invalid rollback sequence {to_seq}. Valid range is 1..={max_seq}.");
        }

        // Flush WAL and save backup copy first
        self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        let backup_name = format!(
            "campaign.backup-before-rollback-seq-{}-{}.db",
            to_seq,
            Utc::now().format("%Y%m%dT%H%M%SZ")
        );
        let backup_path = self
            .db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(backup_name);
        std::fs::copy(&self.db_path, &backup_path)?;

        // Inspect all events before and after `to_seq` so we can cleanly unwind
        // lore, texts, carryable items, entity histories, and attachments created after `to_seq`.
        let all_events = self.list_all_events()?;
        let mut kept_lore = std::collections::BTreeSet::new();
        let mut kept_texts = std::collections::BTreeSet::new();
        let mut kept_items = std::collections::BTreeSet::new();
        let mut kept_attachments = std::collections::BTreeSet::new();
        let mut kept_profiles = std::collections::BTreeSet::new();
        let mut kept_tables = std::collections::BTreeSet::new();
        let mut kept_discoveries = std::collections::BTreeSet::new();
        let mut kept_encounters = std::collections::BTreeSet::new();
        let mut kept_knowledge = std::collections::BTreeSet::new();
        let mut kept_factions = std::collections::BTreeSet::new();
        for ev in all_events.iter().filter(|e| e.seq <= to_seq) {
            match &ev.payload {
                EventPayload::LoreRecorded { lore_id, .. } => {
                    kept_lore.insert(lore_id.clone());
                }
                EventPayload::TextRecorded { text_id, .. } => {
                    kept_texts.insert(text_id.clone());
                    kept_items.insert(text_id.clone());
                }
                EventPayload::ItemCataloged { item_id, .. } => {
                    kept_items.insert(item_id.clone());
                }
                EventPayload::AttachmentRecorded { attachment_id, .. } => {
                    kept_attachments.insert(attachment_id.clone());
                }
                EventPayload::RulesetProfileUpserted { profile_id, .. } => {
                    kept_profiles.insert(profile_id.clone());
                }
                EventPayload::RandomTableUpserted { table_id, .. } => {
                    kept_tables.insert(table_id.clone());
                }
                EventPayload::RandomTableDrawn { staged_discovery_id: Some(did), .. } => {
                    kept_discoveries.insert(did.clone());
                }
                EventPayload::EncounterStarted { encounter_id, .. } => {
                    kept_encounters.insert(encounter_id.clone());
                }
                EventPayload::KnowledgeRecorded { claim_id, .. } => {
                    kept_knowledge.insert(claim_id.clone());
                }
                EventPayload::FactionRecorded { faction_id, .. } => {
                    kept_factions.insert(faction_id.clone());
                }
                _ => {}
            }
        }
        for ev in all_events.iter().filter(|e| e.seq > to_seq) {
            match &ev.payload {
                EventPayload::LoreRecorded { lore_id, .. } => {
                    if !kept_lore.contains(lore_id) {
                        let _ = self.delete_lore(lore_id)?;
                    }
                }
                EventPayload::TextRecorded { text_id, .. } => {
                    if !kept_texts.contains(text_id) {
                        let _ = self.delete_text(text_id)?;
                    }
                    if !kept_items.contains(text_id) {
                        let _ = self.delete_catalog_item(text_id)?;
                    }
                }
                EventPayload::ItemCataloged { item_id, .. } => {
                    if !kept_items.contains(item_id) {
                        let _ = self.delete_catalog_item(item_id)?;
                    }
                }
                EventPayload::EntityHistoryAdded { history_id, .. } => {
                    let _ = self.delete_entity_history(*history_id)?;
                }
                EventPayload::AttachmentRecorded { attachment_id, .. } => {
                    if !kept_attachments.contains(attachment_id) {
                        let _ = self.delete_attachment(attachment_id)?;
                    }
                }
                EventPayload::RulesetProfileUpserted { profile_id, .. } => {
                    if !kept_profiles.contains(profile_id) {
                        let _ = self.delete_ruleset_profile(profile_id)?;
                    }
                }
                EventPayload::RandomTableUpserted { table_id, .. } => {
                    if !kept_tables.contains(table_id) {
                        let _ = self.delete_random_table(table_id)?;
                    }
                }
                EventPayload::RandomTableDrawn { staged_discovery_id: Some(did), .. } => {
                    if !kept_discoveries.contains(did) {
                        let _ = self.delete_staged_discovery(did)?;
                    }
                }
                EventPayload::EncounterStarted { encounter_id, .. } => {
                    if !kept_encounters.contains(encounter_id) {
                        let _ = self.delete_encounter(encounter_id)?;
                    }
                }
                EventPayload::KnowledgeRecorded { claim_id, .. } => {
                    if !kept_knowledge.contains(claim_id) {
                        let _ = self.delete_knowledge_claim(claim_id)?;
                    }
                }
                EventPayload::FactionRecorded { faction_id, .. } => {
                    if !kept_factions.contains(faction_id) {
                        let _ = self.delete_faction(faction_id)?;
                    }
                }
                _ => {}
            }
        }

        // Delete future events and entities created after to_seq
        self.conn.execute("DELETE FROM events WHERE seq > ?1", params![to_seq])?;
        self.conn
            .execute("DELETE FROM events_fts WHERE seq > ?1", params![to_seq])?;
        self.conn.execute(
            "DELETE FROM character_baselines WHERE created_seq > ?1",
            params![to_seq],
        )?;
        self.conn.execute(
            "DELETE FROM place_baselines WHERE created_seq > ?1",
            params![to_seq],
        )?;

        // Rebuild materialized tables (relations, clocks, world clock, session) cleanly from remaining events
        self.conn.execute("DELETE FROM relations", [])?;
        self.conn.execute("DELETE FROM clocks", [])?;

        let remaining = self.list_all_events()?;
        let mut meta = self.get_campaign_meta()?;
        let mut session = 1u32;
        for ev in &remaining {
            match &ev.payload {
                EventPayload::SessionMarker { session_number, .. } => {
                    session = *session_number;
                }
                EventPayload::TimeAdvanced { new_clock, .. } => {
                    meta.clock = new_clock.clone();
                }
                EventPayload::RelationUpserted { relation } => {
                    self.upsert_relation(relation)?;
                }
                EventPayload::RelationRemoved {
                    source_id,
                    relation_type,
                    target_id,
                    ..
                } => {
                    let _ = self.delete_relation(source_id, relation_type, target_id)?;
                }
                EventPayload::EntityDeleted { entity_id, .. } => {
                    let _ = self.conn.execute(
                        "DELETE FROM relations WHERE source_id = ?1 OR target_id = ?1",
                        params![entity_id],
                    )?;
                }
                EventPayload::ClockCreated { clock } => {
                    self.upsert_clock(clock)?;
                }
                EventPayload::ClockTicked {
                    clock_id,
                    delta,
                    set_status,
                    ..
                } => {
                    if let Some(mut c) = self.get_clock(clock_id)? {
                        c.filled = (c.filled + *delta).clamp(0, c.segments);
                        if let Some(st) = set_status {
                            c.status = st.clone();
                        } else if c.filled >= c.segments {
                            c.status = "completed".to_string();
                        }
                        self.upsert_clock(&c)?;
                    }
                }
                _ => {}
            }
        }
        self.update_clock_and_session(&meta.clock, session)?;

        Ok(backup_path)
    }

    /// Restores the live database from an existing `campaign.backup-*.db` snapshot inside the
    /// same campaign directory (creates a pre-restore safety backup first and copies all tables
    /// inside an atomic SQLite transaction so live open connections on Windows/Linux stay valid).
    pub fn restore_from_backup(&self, backup_filename: &str) -> Result<(PathBuf, PathBuf)> {
        if backup_filename.contains('/')
            || backup_filename.contains('\\')
            || backup_filename.contains("..")
            || !backup_filename.ends_with(".db")
        {
            bail!("Invalid backup filename '{backup_filename}'. Pass a bare 'campaign.backup-*.db' filename.");
        }
        let dir = self.db_path.parent().unwrap_or_else(|| Path::new("."));
        let src_path = dir.join(backup_filename);
        if !src_path.exists() {
            bail!("Backup file not found: {}", src_path.display());
        }

        let safety_backup = self.create_backup("before-restore")?;

        let src_str = src_path.to_string_lossy().to_string();
        self.conn
            .execute("ATTACH DATABASE ?1 AS bak", params![src_str])?;

        // Every canonical base table across all Chronicle phases (FTS5 virtual tables are rebuilt below).
        let tables = [
            "campaign_meta",
            "character_baselines",
            "place_baselines",
            "lore_entries",
            "story_texts",
            "attachments",
            "catalog_items",
            "entity_histories",
            "events",
            "event_entities",
            "relations",
            "clocks",
            "event_causality",
            "ruleset_profiles",
            "random_tables",
            "table_draw_state",
            "staged_discoveries",
            "encounters",
            "character_profiles",
            "knowledge_claims",
            "actor_beliefs",
            "factions",
            "place_spatial",
            "travel_routes",
            "item_economics",
            "currency_balances",
            "markets",
            "jurisdictions",
            "crime_records",
            "reputation_scores",
            "injuries",
            "crafting_projects",
            "progression_ladders",
            "character_progression",
            "map_grids",
            "map_grid_cells",
            "engine_signals",
            "character_routines",
        ];

        let restore_result: Result<()> = (|| {
            self.conn
                .execute_batch("PRAGMA foreign_keys = OFF; BEGIN IMMEDIATE;")?;
            for t in &tables {
                // Only restore tables that exist in the backup snapshot (forward/backward compatible).
                let exists: i64 = self.conn.query_row(
                    "SELECT COUNT(*) FROM bak.sqlite_master WHERE type = 'table' AND name = ?1",
                    params![t],
                    |r| r.get(0),
                )?;
                self.conn.execute(&format!("DELETE FROM main.\"{t}\""), [])?;
                if exists > 0 {
                    // Compute exact column intersection between main.{t} and bak.{t} so pre-Phase-1/2
                    // backups (e.g. 10-column `events` or 5-column `relations`) restore seamlessly,
                    // letting SQLite populate any newly-added columns with their schema defaults!
                    let main_cols: Vec<String> = {
                        let mut stmt = self.conn.prepare(&format!("PRAGMA main.table_info(\"{t}\")"))?;
                        let cols = stmt
                            .query_map([], |r| r.get::<_, String>(1))?
                            .filter_map(|r| r.ok())
                            .collect();
                        cols
                    };
                    let bak_cols: std::collections::BTreeSet<String> = {
                        let mut stmt = self.conn.prepare(&format!("PRAGMA bak.table_info(\"{t}\")"))?;
                        let cols = stmt
                            .query_map([], |r| r.get::<_, String>(1))?
                            .filter_map(|r| r.ok())
                            .collect();
                        cols
                    };
                    let common: Vec<String> = main_cols
                        .into_iter()
                        .filter(|c| bak_cols.contains(c))
                        .map(|c| format!("\"{c}\""))
                        .collect();
                    if !common.is_empty() {
                        let col_list = common.join(", ");
                        self.conn.execute(
                            &format!(
                                "INSERT INTO main.\"{t}\" ({col_list}) SELECT {col_list} FROM bak.\"{t}\""
                            ),
                            [],
                        )?;
                    }
                }
            }
            // Rebuild all FTS5 virtual tables cleanly from restored base tables
            self.conn.execute_batch(
                "DELETE FROM lore_fts;
                 INSERT INTO lore_fts (id, title, category, summary, body, keywords, tags)
                   SELECT id, title, category, summary, body, keywords_json, tags_json FROM lore_entries;
                 DELETE FROM story_texts_fts;
                 INSERT INTO story_texts_fts (id, title, medium, subject, author, language, summary, content, mechanics, keywords, tags)
                   SELECT id, title, medium, subject, author, language, summary, content, mechanics_json, keywords_json, tags_json FROM story_texts;
                 DELETE FROM attachments_fts;
                 INSERT INTO attachments_fts (id, title, original_filename, category, summary, extracted_notes, keywords, tags)
                   SELECT id, title, original_filename, category, summary, extracted_notes, keywords_json, tags_json FROM attachments;
                 DELETE FROM catalog_items_fts;
                 INSERT INTO catalog_items_fts (id, name, item_type, origin_and_maker, previous_owners, significance, properties_and_effects, description, keywords, tags)
                   SELECT id, name, item_type, origin_and_maker, previous_owners_json, significance, properties_and_effects, description, keywords_json, tags_json FROM catalog_items;
                 DELETE FROM entity_histories_fts;
                 INSERT INTO entity_histories_fts (id, entity_id, entity_kind, era_or_date, title, details, significance)
                   SELECT id, entity_id, entity_kind, era_or_date, title, details, significance FROM entity_histories;
                 DELETE FROM events_fts;
                 INSERT INTO events_fts (seq, world_time, summary, details)
                   SELECT seq, world_time, summary, payload_json FROM events;
                 DELETE FROM knowledge_claims_fts;
                 DELETE FROM factions_fts;",
            )?;
            for claim in self.list_knowledge_claims()? {
                self.conn.execute(
                    "INSERT INTO knowledge_claims_fts (id, subject, claim, tags) VALUES (?1, ?2, ?3, ?4)",
                    params![claim.id, claim.subject, claim.claim, claim.tags.join(" ")],
                )?;
            }
            for faction in self.list_factions()? {
                self.conn.execute(
                    "INSERT INTO factions_fts (id, name, faction_type, ideology, origins, notes, tags)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        faction.id,
                        faction.name,
                        faction.faction_type,
                        faction.ideology,
                        faction.origins,
                        faction.notes,
                        faction.tags.join(" ")
                    ],
                )?;
            }
            self.conn
                .execute_batch("COMMIT; PRAGMA foreign_keys = ON;")?;
            Ok(())
        })();

        if restore_result.is_err() {
            let _ = self
                .conn
                .execute_batch("ROLLBACK; PRAGMA foreign_keys = ON;");
        }
        let _ = self.conn.execute("DETACH DATABASE bak", []);
        restore_result?;

        Ok((src_path, safety_backup))
    }
}

fn compute_event_hash(
    prev_hash: &str,
    session: u32,
    world_time: &str,
    real_ts: &str,
    location: &str,
    summary: &str,
    payload_json: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prev_hash.as_bytes());
    hasher.update(b"|");
    hasher.update(session.to_string().as_bytes());
    hasher.update(b"|");
    hasher.update(world_time.as_bytes());
    hasher.update(b"|");
    hasher.update(real_ts.as_bytes());
    hasher.update(b"|");
    hasher.update(location.as_bytes());
    hasher.update(b"|");
    hasher.update(summary.as_bytes());
    hasher.update(b"|");
    hasher.update(payload_json.as_bytes());
    let result = hasher.finalize();
    result.iter().map(|b| format!("{b:02x}")).collect()
}

fn row_to_lore(row: &rusqlite::Row<'_>) -> rusqlite::Result<LoreEntry> {
    let kw_json: String = row.get(5)?;
    let tags_json: String = row.get(6)?;
    let always: i64 = row.get(8)?;
    let rel_json: String = row.get(9)?;
    Ok(LoreEntry {
        id: row.get(0)?,
        title: row.get(1)?,
        category: row.get(2)?,
        summary: row.get(3)?,
        body: row.get(4)?,
        keywords: serde_json::from_str(&kw_json).unwrap_or_default(),
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        visibility: row.get(7)?,
        always_include: always != 0,
        related_ids: serde_json::from_str(&rel_json).unwrap_or_default(),
        created_at_world_time: row.get(10)?,
        updated_at_world_time: row.get(11)?,
    })
}

fn row_to_text(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoryText> {
    let read_by_json: String = row.get(8)?;
    let mech_json: String = row.get(12)?;
    let kw_json: String = row.get(14)?;
    let tags_json: String = row.get(15)?;
    let rel_json: String = row.get(16)?;
    Ok(StoryText {
        id: row.get(0)?,
        title: row.get(1)?,
        medium: row.get(2)?,
        subject: row.get(3)?,
        author: row.get(4)?,
        language: row.get(5)?,
        condition: row.get(6)?,
        read_status: row.get(7)?,
        read_by: serde_json::from_str(&read_by_json).unwrap_or_default(),
        holder_or_location_id: row.get(9)?,
        summary: row.get(10)?,
        content: row.get(11)?,
        mechanics_or_unlocks: serde_json::from_str(&mech_json).unwrap_or_default(),
        gm_notes: row.get(13)?,
        keywords: serde_json::from_str(&kw_json).unwrap_or_default(),
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        related_ids: serde_json::from_str(&rel_json).unwrap_or_default(),
        created_at_world_time: row.get(17)?,
        updated_at_world_time: row.get(18)?,
    })
}

fn row_to_attachment(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoryAttachment> {
    let size: i64 = row.get(7)?;
    let kw_json: String = row.get(12)?;
    let tags_json: String = row.get(13)?;
    let rel_json: String = row.get(14)?;
    Ok(StoryAttachment {
        id: row.get(0)?,
        title: row.get(1)?,
        original_filename: row.get(2)?,
        stored_filename: row.get(3)?,
        relative_path: row.get(4)?,
        category: row.get(5)?,
        mime_type: row.get(6)?,
        size_bytes: size.max(0) as u64,
        sha256: row.get(8)?,
        summary: row.get(9)?,
        extracted_notes: row.get(10)?,
        visibility: row.get(11)?,
        keywords: serde_json::from_str(&kw_json).unwrap_or_default(),
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        related_ids: serde_json::from_str(&rel_json).unwrap_or_default(),
        source_origin: row.get(15)?,
        created_at_world_time: row.get(16)?,
        updated_at_world_time: row.get(17)?,
    })
}

fn row_to_catalog_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogItem> {
    let equipped: i64 = row.get(6)?;
    let prev_json: String = row.get(9)?;
    let kw_json: String = row.get(16)?;
    let tags_json: String = row.get(17)?;
    let rel_json: String = row.get(18)?;
    let gm_meas_json: String = row.get(21).unwrap_or_else(|_| "{}".to_string());
    let garment_cut: String = row.get(22).unwrap_or_default();
    let garment_taper: String = row.get(23).unwrap_or_default();
    Ok(CatalogItem {
        id: row.get(0)?,
        name: row.get(1)?,
        item_type: row.get(2)?,
        rarity_or_tier: row.get(3)?,
        slot: row.get(4)?,
        quantity: row.get(5)?,
        equipped: equipped != 0,
        condition_or_durability: row.get(7)?,
        origin_and_maker: row.get(8)?,
        previous_owners: serde_json::from_str(&prev_json).unwrap_or_default(),
        current_holder_id: row.get(10)?,
        significance: row.get(11)?,
        properties_and_effects: row.get(12)?,
        description: row.get(13)?,
        linked_text_id: row.get(14)?,
        gm_notes: row.get(15)?,
        keywords: serde_json::from_str(&kw_json).unwrap_or_default(),
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        related_ids: serde_json::from_str(&rel_json).unwrap_or_default(),
        garment_measurements: serde_json::from_str(&gm_meas_json).unwrap_or_default(),
        garment_cut,
        garment_taper,
        created_at_world_time: row.get(19)?,
        updated_at_world_time: row.get(20)?,
    })
}

fn row_to_entity_history(row: &rusqlite::Row<'_>) -> rusqlite::Result<EntityHistoryEntry> {
    let rel_json: String = row.get(8)?;
    Ok(EntityHistoryEntry {
        id: row.get(0)?,
        entity_id: row.get(1)?,
        entity_kind: row.get(2)?,
        era_or_date: row.get(3)?,
        sort_order: row.get(4)?,
        title: row.get(5)?,
        details: row.get(6)?,
        significance: row.get(7)?,
        related_ids: serde_json::from_str(&rel_json).unwrap_or_default(),
        visibility: row.get(9)?,
        recorded_at_world_time: row.get(10)?,
    })
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorldEvent> {
    let aff_json: String = row.get(5)?;
    let payload_json: String = row.get(7)?;
    let payload: EventPayload = serde_json::from_str(&payload_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let actor_id: Option<String> = row.get(10).unwrap_or(None);
    let causal_json: String = row.get(11).unwrap_or_else(|_| "[]".to_string());
    let visibility: String = row.get(12).unwrap_or_else(|_| "public".to_string());
    Ok(WorldEvent {
        seq: row.get(0)?,
        session_number: row.get(1)?,
        world_time: row.get(2)?,
        real_timestamp: row.get(3)?,
        location_id: row.get(4)?,
        affected_entities: serde_json::from_str(&aff_json).unwrap_or_default(),
        summary: row.get(6)?,
        payload,
        prev_hash: row.get(8)?,
        hash: row.get(9)?,
        actor_id,
        causal_event_ids: serde_json::from_str(&causal_json).unwrap_or_default(),
        visibility,
    })
}

fn row_to_relation(row: &rusqlite::Row<'_>) -> rusqlite::Result<GraphRelation> {
    let dims_json: String = row.get(8).unwrap_or_else(|_| "{}".to_string());
    let direction: String = row.get(9).unwrap_or_else(|_| "directed".to_string());
    let visibility: String = row.get(10).unwrap_or_else(|_| "public".to_string());
    let origin_event_id: Option<i64> = row.get(11).unwrap_or(None);
    let ob_json: String = row.get(12).unwrap_or_else(|_| "[]".to_string());
    Ok(GraphRelation {
        source_id: row.get(0)?,
        source_kind: row.get(1)?,
        relation_type: row.get(2)?,
        target_id: row.get(3)?,
        target_kind: row.get(4)?,
        strength: row.get(5)?,
        notes: row.get(6)?,
        updated_at_world_time: row.get(7)?,
        dimensions: serde_json::from_str::<RelationshipDimensions>(&dims_json).unwrap_or_default(),
        direction,
        visibility,
        origin_event_id,
        obligations: serde_json::from_str(&ob_json).unwrap_or_default(),
    })
}

fn row_to_clock(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProgressClock> {
    let rel_json: String = row.get(7)?;
    let cadence_raw: Option<i64> = row.get(8).unwrap_or(None);
    let last_tick: Option<i64> = row.get(9).unwrap_or(None);
    let consequence: String = row.get(10).unwrap_or_default();
    let auto_reset_int: i64 = row.get(11).unwrap_or(0);
    Ok(ProgressClock {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        filled: row.get(3)?,
        segments: row.get(4)?,
        status: row.get(5)?,
        description: row.get(6)?,
        related_ids: serde_json::from_str(&rel_json).unwrap_or_default(),
        cadence_hours: cadence_raw.filter(|&h| h > 0).map(|h| h as u64),
        last_tick_total_hours: last_tick,
        consequence_on_fill: consequence,
        auto_reset: auto_reset_int != 0,
    })
}

/// Converts arbitrary user text into a safe FTS5 OR query matching alphanumeric tokens.
pub fn sanitize_fts_query(raw: &str) -> String {
    let tokens: Vec<String> = raw
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 2)
        .map(|t| format!("\"{}\"*", t.to_lowercase()))
        .collect();
    tokens.join(" OR ")
}
