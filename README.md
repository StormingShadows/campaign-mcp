# Chronicle — Cross-Platform RPG GM Engine & Studio (Rust)

**Chronicle** is a cross-platform (**Linux, macOS, Windows**), zero-runtime-dependency RPG Game Master state engine written in **Rust**. It ships as a single native binary combining:

- **Built-In Web GM Studio GUI** (`chronicle ui`)
- **Model Context Protocol (MCP) Server** (`chronicle mcp`)
- **REST + OpenAPI 3.1 API** & **OpenAI / Anthropic Tool Schema Exporters**
- **Multi-Dimensional "Search By" Engine** across every RPG entity domain
- **Auto-Compiled, Cross-Linked Referencable Markdown Codex** backed by **SQLite Event Sourcing + Knowledge Graph + FTS5**

> **Clean & Fresh by Design:** Chronicle ships with **zero pre-populated campaign or test data**. Every facility is documented here and in [SKILL.md](.agents/skills/gm-campaign/SKILL.md), leaving your campaign workspace completely empty and ready for your own world.

---

## Architecture Overview

Pure flat-file / Markdown-only campaigns suffer from LLM drift on HP/inventory math, item duplication, forgotten conditions/seasons, and context bloat. Chronicle separates the **deterministic SQLite state engine** from the **auto-compiled Markdown reference documents**:

```text
   Human GM (Browser GUI)  ◄──►  AI Agents (MCP / OpenAI / Anthropic / OpenAPI REST / CLI)
                                           │
                                           ▼
                       ┌────────────────────────────────────────┐
                       │      Chronicle Single Rust Binary      │
                       │  • Built-in Web GM Studio (HTTP + SPA) │
                       │  • MCP stdio + OpenAPI 3.1 REST API    │
                       │  • Multi-Dimensional "Search By" Engine│
                       │  • Deterministic Projection & Dice     │
                       │  • Fuzzy Name/ID Resolution & Batching │
                       └──────────┬──────────────────┬──────────┘
                                  │                  │
                       Canonical Source       Auto-Compiled
                           of Truth          Referencable Docs
                                  │                  │
                                  ▼                  ▼
                    ┌───────────────────────┐  ┌───────────────────────────┐
                    │  campaign.db (SQLite) │  │          docs/            │
                    │  • SHA-256 Event Log  │  │  • INDEX.md / TIMELINE.md │
                    │  • Knowledge Graph    │  │  • characters/<slug>.md   │
                    │  • FTS5 Lorebook      │  │  • places/<slug>.md       │
                    │  • Items & Histories  │  │  • items/<slug>.md        │
                    │  • Books & Scrolls DB │  │  • texts/<slug>.md        │
                    │  • Attached Ref Docs  │  │  • lore/<cat>/<slug>.md   │
                    │  • Clocks & Calendar  │  │  • attachments/<slug>.md  │
                    └───────────────────────┘  └───────────────────────────┘
```

---

## Six Specialized Document Sections

Every mutation automatically compiles and cross-links Markdown documents under `<campaign>/docs/`:

1. **Characters (`docs/characters/<slug>.md`)**
   - **1. Initial / Baseline Profile:** Immutable starting info (origin, archetype, faction, appearance, personality, backstory, starting stats/resources, initial equipment, skills, techniques, goals).
   - **2. Current Status:** Deterministically projected live state (alive status, current location, resource bars, stat deltas vs. baseline, active buffs/debuffs/injuries, equipped gear vs. carried inventory, updated skills & techniques, goals, Knowledge Graph relations).
   - **3. Historical Chronicle & Event Log (at the end):** Chronological history of every event affecting this character.
2. **Places (`docs/places/<slug>.md`)**
   - **1. Baseline Geography & Significance:** Parent region, terrain, climate, seasonal profiles (Spring/Summer/Autumn/Winter), baseline landmarks (places of significance), and routes.
   - **2. Current Status:** Active season + seasonal terrain modifier, live weather, controlling faction, danger level, active local conditions, auto-resolved present characters, live landmark & route statuses.
   - **3. Historical Chronicle & Place Event Log (at the end):** Dedicated historical milestones (`add_entity_history` for places & establishments) plus chronological play events.
3. **Carryable Items, Artifacts, Tools, Weapons & Keepsakes (`docs/items/<slug>.md`)**
   - Tracks all carryable story items (`weapon`, `armor`, `tool`, `artifact`, `keepsake`, `book`, `manual`, `scroll`, `record`, `key`, `consumable`, `gear`) with `origin_and_maker`, chronological `previous_owners`, live `current_holder_id` (character or place), `condition_or_durability`, `significance`, `properties_and_effects`, `## Historical Chronicle`, and live provenance log.
   - **Automatic Inventory & Book Sync:** Physical books, manuals, scrolls, and records created via `upsert_text` auto-sync as carryable catalog items; transferring an item (`transfer_catalog_item`) or moving a book (`record_text_reading`) automatically appends the prior holder to `previous_owners` and syncs both characters' inventories.
4. **Books, Scrolls & Story Texts (`docs/texts/<slug>.md`)**
   - Dedicated library for **in-world readable texts** (`book`, `scroll`, `grimoire`, `ritual-manual`, `tablet`, `journal`, `letter`, `inscription`, `codex`) relevant to world history, spell rituals, prophecies, or technique manuals.
   - Tracks `medium`, `subject`, `author`, `language`, `condition`, `read_status` (`undiscovered` → `discovered_unread` → `partially_read` → `read` → `deciphered`), `read_by`, `holder_or_location_id`, outward `summary`, readable `content`, `mechanics_or_unlocks`, hidden `gm_notes`, `## Historical Chronicle`, and a **Provenance & Reading Log**.
5. **World-Building, Lore & Entity Histories (`docs/lore/<category>/<slug>.md` + `entity_histories`)**
   - Separate world reference (`magic-system`, `factions`, `history`, `cosmology`, `bestiary`, `culture`, `artifacts`, `rules`, `secrets`) with `public` / `rumor` / `gm_secret` visibility, `always_include` world laws, FTS5 search, and keyword auto-triggers.
   - First-class **Historical Chronicles (`add_entity_history` / `search_histories`)** attachable to any **Place**, **Establishment** (tavern, guild, academy, temple), **Sect / Faction**, **Carryable Item**, **Book/Manual**, **Lore entry**, or **Character**.
6. **Attached & Pasted Reference Docs (`docs/attachments/`)**
   - Copy player-attached files relevant to the campaign (maps, PDF handouts, portraits, rules sheets, Markdown notes, audio) into `<campaign>/docs/attachments/files/<id>.<ext>` via `import_attachment`, **or create/append new files directly from pasted-in reference text** via `create_reference_doc` (`chronicle create-doc`).
   - Returns permanent addressable references (`attachment:<id>`, `chronicle://attachments/<id>`, `docs/attachments/files/<id>.<ext>`, and `/api/attachments/<id>/file`), compiles a companion reference card `docs/attachments/<id>.md`, and indexes `summary` + `extracted_notes` in SQLite FTS5.
7. **Master Campaign Files (`docs/INDEX.md`, `docs/TIMELINE.md`)**
   - World clock/season overview, active Progress Clocks, full directory tables (including Carryable Items & Attached Docs), and SHA-256 verified master event timeline.

---

## Cross-Platform Build (Linux, macOS, Windows)

Chronicle uses only pure cross-platform Rust crates and `rusqlite` with `bundled` SQLite (compiles SQLite + FTS5 + JSON1 directly into the binary—**no external `libsqlite3` or runtime libraries required**).

### Prerequisites
- Rust toolchain (`rustc` / `cargo` via [rustup](https://rustup.rs))
- C compiler for bundled SQLite (`gcc` / `clang` on Linux/macOS; MSVC Build Tools on Windows)

### Build on Linux / macOS
```bash
cargo build --release
# Binary: ./target/release/chronicle
```

### Build on Windows (PowerShell)
```powershell
cargo build --release
# Binary: .\target\release\chronicle.exe
```

### Run Test Suite
```bash
cargo test
```
*(All test fixtures live strictly inside `tests/integration_tests.rs` using temporary directories and are never bundled into the production binary or workspace.)*

---

## Getting Started With a Fresh Campaign

### 1. Initialize an Empty Campaign
```bash
# Linux / macOS
./target/release/chronicle init ./my-campaign \
  --name "My Campaign" \
  --description "Your world premise" \
  --ruleset "System-agnostic RPG core"
```
```powershell
# Windows
.\target\release\chronicle.exe init .\my-campaign `
  --name "My Campaign" `
  --description "Your world premise" `
  --ruleset "System-agnostic RPG core"
```

### 2. Launch the Built-In Web GM Studio GUI + REST API
```bash
# Linux / macOS
./target/release/chronicle ui --campaign ./my-campaign --port 7878

# Windows
.\target\release\chronicle.exe ui --campaign .\my-campaign --port 7878
```
Open **`http://127.0.0.1:7878`** in your browser for:
- **Dashboard:** World clock & season advancement, weather, interactive Progress Clocks, Dice Tray, Quick Event Logger, Party Rest.
- **Characters / Places / World & Lore / Books & Scrolls:** Dedicated tabs with multi-filter **"Search By"** bars, 3-section inspectors, time-travel `as_of_seq` slider, manuscript reader & study tracker, and creation modals.
- **Knowledge Graph:** Interactive SVG relationship map + multi-hop (1–3 hops) filterable explorer.
- **Timeline & Rollback:** Full filterable event ledger with safe one-click rollback (auto-creates a backup DB first).
- **AI Context & Schemas:** Live `get_scene_context` simulator (Markdown vs. JSON token counts + Lore/Text triggers) and one-click MCP / OpenAI / Anthropic / OpenAPI 3.1 schema viewer.

### 3. Connect an AI Agent via MCP (ChatGPT, Codex CLI, Antigravity, Claude, Cursor)

Chronicle implements full **MCP `2025-06-18` / `2025-03-26`** compatibility across both **Stdio** and **Streamable HTTP (`/mcp`)** transports, complete with ChatGPT behavioral `annotations` (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`), `structuredContent`, and ChatGPT's standard `search` & `fetch` connector tools.

#### A. OpenAI Codex CLI (`~/.codex/config.toml`)
Register Chronicle directly via the Codex CLI (or copy [codex_config.example.toml](codex_config.example.toml)):

```bash
# Local Stdio (Linux / macOS)
codex mcp add chronicle-gm -- /absolute/path/to/chronicle/target/release/chronicle mcp --campaign /absolute/path/to/my-campaign

# Verify live discovery inside Codex CLI
codex mcp list
```
*(Inside a Codex session, run `/mcp` to verify all 60 tools are loaded.)*

#### B. ChatGPT Web / Desktop / Developer Mode & Remote OAuth 2.1 (`--transport http --oauth`)
ChatGPT's web and desktop MCP connectors require **Streamable HTTP** (and support **OAuth 2.1 + PKCE + Dynamic Client Registration** over HTTPS tunnels). Launch Chronicle's built-in Streamable HTTP server (also serves the Web GM Studio GUI on the same port):

```bash
# Linux / macOS — Built-in OAuth 2.1 + PKCE + RFC 7591 Dynamic Client Registration
export CHRONICLE_OAUTH_SECRET="your-gm-passphrase"
./target/release/chronicle mcp --campaign ./my-campaign --transport http --port 7878 \
  --oauth --public-url https://your-https-tunnel.example.com
```
```powershell
# Windows (PowerShell)
$env:CHRONICLE_OAUTH_SECRET = "your-gm-passphrase"
.\target\release\chronicle.exe mcp --campaign .\my-campaign --transport http --port 7878 `
  --oauth --public-url https://your-https-tunnel.example.com
```
*(You can also enable a static Bearer token alongside or instead of OAuth via `--bearer-token` / `CHRONICLE_MCP_TOKEN`.)*

Chronicle serves all MCP OAuth 2.1 discovery & flow endpoints natively (`/.well-known/oauth-protected-resource`, `/.well-known/oauth-authorization-server`, `/oauth/register`, `/oauth/authorize`, `/oauth/token`, and `/health`). Connect ChatGPT (Settings → Apps / Connectors → Developer Mode) to **`https://your-https-tunnel.example.com/mcp`** and enter your GM passphrase on the OAuth consent prompt.

#### B2. One-Command Local Serving over Tailscale Funnel (Free Permanent HTTPS URL)
Recommended when keeping Chronicle source code in Git (outside Google Drive so `target/` build artifacts never sync) while retaining your campaign folder (`campaign.db` + `docs/`) in Google Drive:

1. **One-time Tailscale setup:** Install Tailscale (`winget install Tailscale.Tailscale`), sign in, and enable **MagicDNS + HTTPS Certificates** in the Tailscale Admin Console (`https://login.tailscale.com/admin/dns`).
2. **Start serving (Windows PowerShell):**
   ```powershell
   # First time (saves CampaignDir & a persistent GM OAuth passphrase to gitignored .chronicle-serve.env)
   .\scripts\serve-tailscale.ps1 -CampaignDir "D:\Tgvt\rpg\my-campaign" -Build

   # Subsequent sessions (one command!)
   .\scripts\serve-tailscale.ps1

   # Useful flags:
   .\scripts\serve-tailscale.ps1 -Check          # Preflight check only
   .\scripts\serve-tailscale.ps1 -RotateSecret   # Generate a fresh GM OAuth passphrase
   .\scripts\serve-tailscale.ps1 -Stop           # Turn off Tailscale Funnel immediately
   ```
   *(Linux/macOS equivalent: `./scripts/serve-tailscale.sh /path/to/my-campaign`)*
3. The script exposes **only** `127.0.0.1:7878` on your permanent `https://<your-pc>.<your-tailnet>.ts.net/mcp` URL (with OAuth 2.1 + HTTP local-file-import guard enabled), verifies `/health` and OAuth discovery end-to-end, and on `Ctrl+C` automatically **disables Tailscale Funnel** and checkpoints SQLite WAL cleanly for Google Drive.

#### C. Running Web-Hosted on Google Colab with Google Drive Persistent Storage
A ready-to-run notebook is included at [colab/Chronicle_Colab_MCP_Server.ipynb](colab/Chronicle_Colab_MCP_Server.ipynb):
1. Mounts Google Drive (`/content/drive/MyDrive/ChronicleRPG/`).
2. Builds Chronicle once on Linux and caches the compiled binary on Google Drive for 3-second cold starts.
3. Copies `campaign.db` to Colab's local SSD (`/content/campaign_live/`) and runs a background **SQLite online-backup sync daemon** mirroring `campaign.db` and `docs/` back to Google Drive every 20s + on shutdown (avoiding Google Drive FUSE WAL corruption).
4. Launches a free **Cloudflare Quick Tunnel** (`https://<random>.trycloudflare.com`) + Chronicle `--oauth`, printing your ready-to-paste MCP URL and GM OAuth Passphrase.

#### D. Antigravity, Claude Code / Desktop, Cursor (JSON `mcp_config.json`)
Copy [mcp_config.example.json](mcp_config.example.json) into your MCP client config (Antigravity `~/.gemini/config/mcp_config.json`, Claude Desktop/Code, or Cursor) and set the binary path (`chronicle` on Linux/macOS, `chronicle.exe` on Windows) and your campaign directory path.

A complete GM operating manual (routing matrix, exact read order, exact write order, two-phase Book/Scroll disclosure) is included at [.agents/skills/gm-campaign/SKILL.md](.agents/skills/gm-campaign/SKILL.md).

---

## Multi-Dimensional "Search By" & AI Compatibility

All **60 tools** are identically accessible via **MCP (Stdio & Streamable HTTP)**, **Web GUI**, **HTTP REST (`POST /api/tool/<name>`)**, and **CLI**:

- **Diagnostics, Editing, Rollback & Cleanup Layer:** `diagnose_campaign` (linter for mislabeled registers, broken links, duplicates, sync drift, placeholder metadata, one-way routes, stale docs), `reclassify_entity` (move records between `lore`/`text`/`item`/`place`/`character`/`history`/`attachment`), `rename_or_merge_entity` (rename or merge duplicates with automatic cross-reference rewriting), `update_campaign_meta`, `update_character_baseline`, `update_place_baseline`, `update_entity_history`, `update_attachment`, `manage_clock(update)`, `get_lore`, `delete_entity`, `rollback_to_event` (full-register time-travel rollback + stale doc pruning), `backup_campaign`, `restore_backup`, and `cleanup_campaign` (`dry_run` preview or apply).
- **Search By Engine:** `search_characters`, `search_places`, `search_items` (Carryable Items/Relics/Books), `search_histories` (Places/Establishments/Sects/Items Historical Chronicles), `search_lore`, `search_texts` (Books & Scrolls), `search_attachments` (Attached & Pasted Docs), `search_history` (Events), `search_relations` (1–3 hop Knowledge Graph), `search_all` (Omni-Search), plus ChatGPT standard `search` / `fetch`.
- **AI Ergonomics:**
  - Fuzzy name/slug resolution (e.g., partial character or place names resolve automatically).
  - Compact Markdown scene briefs (`get_scene_context(..., format: "markdown", max_chars: ...)`) reducing prompt tokens by ~40%.
  - Atomic batch turn commits (`commit_turn`), ownership-checked `transfer_item`, and `rest_characters`.
  - Universal schema export:
    ```bash
    ./target/release/chronicle export-tools --format mcp
    ./target/release/chronicle export-tools --format openai
    ./target/release/chronicle export-tools --format anthropic
    ./target/release/chronicle export-tools --format openapi
    ```
  - Live OpenAPI 3.1 endpoint while `chronicle ui` is running: `http://127.0.0.1:7878/api/openapi.json`.
