# Database Schema

siggy uses SQLite with WAL (Write-Ahead Logging) mode for safe concurrent
reads/writes. The database file is stored alongside the config file.

## Tables

### `schema_version`

Tracks the current migration version.

```sql
CREATE TABLE schema_version (
    version INTEGER NOT NULL
);
```

### `conversations`

One row per conversation (1:1 or group).

```sql
CREATE TABLE conversations (
    id                TEXT PRIMARY KEY,      -- phone number or group ID
    name              TEXT NOT NULL,         -- display name
    is_group          INTEGER NOT NULL DEFAULT 0,
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    muted             INTEGER NOT NULL DEFAULT 0,  -- added in migration v2
    expiration_timer  INTEGER NOT NULL DEFAULT 0,  -- disappearing msg seconds (v7)
    accepted          INTEGER NOT NULL DEFAULT 1,  -- message request state (v8)
    blocked           INTEGER NOT NULL DEFAULT 0,  -- blocked state (v9)
    mute_expires_at   TEXT,                        -- timed mute expiry, RFC 3339 (v14)
    archived          INTEGER NOT NULL DEFAULT 0   -- hidden from sidebar (v15)
);
```

The `id` is a phone number (E.164 format) for 1:1 conversations or a
base64-encoded group ID for groups.

### `messages`

All messages, ordered by insertion rowid.

```sql
CREATE TABLE messages (
    rowid           INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL REFERENCES conversations(id),
    sender          TEXT NOT NULL,       -- sender display name or empty for system
    timestamp       TEXT NOT NULL,       -- RFC 3339 timestamp
    body            TEXT NOT NULL,       -- message text
    is_system       INTEGER NOT NULL DEFAULT 0,
    status          INTEGER NOT NULL DEFAULT 0,    -- MessageStatus enum (v3)
    timestamp_ms    INTEGER NOT NULL DEFAULT 0,    -- server epoch ms (v3)
    is_edited       INTEGER NOT NULL DEFAULT 0,    -- edited flag (v6)
    is_deleted      INTEGER NOT NULL DEFAULT 0,    -- deleted flag (v6)
    quote_author    TEXT,                           -- quoted reply author (v6)
    quote_body      TEXT,                           -- quoted reply body (v6)
    quote_ts_ms     INTEGER,                        -- quoted reply timestamp (v6)
    sender_id            TEXT NOT NULL DEFAULT '',       -- sender phone number (v6)
    expires_in_seconds   INTEGER NOT NULL DEFAULT 0,    -- disappearing timer (v7)
    expiration_start_ms  INTEGER NOT NULL DEFAULT 0,    -- timer start epoch ms (v7)
    pinned               INTEGER NOT NULL DEFAULT 0,    -- pinned flag (v10)
    poll_data            TEXT,                           -- serialized poll JSON (v11)
    link_preview         TEXT,                           -- serialized preview JSON (v12)
    body_raw             TEXT,                           -- body with mention placeholders (v13)
    mentions_json        TEXT,                           -- serialized mention ranges (v13)
    entry_seq            INTEGER NOT NULL DEFAULT 0     -- row number within one message (v16)
);

CREATE INDEX idx_messages_conv_ts ON messages(conversation_id, timestamp);
CREATE INDEX idx_messages_conv_ts_ms ON messages(conversation_id, timestamp_ms);
CREATE UNIQUE INDEX idx_messages_incoming_dedup                    -- replay dedup (v16)
    ON messages(conversation_id, sender_id, timestamp_ms, entry_seq)
    WHERE sender <> 'you' AND sender_id <> '';
```

One incoming message persists as several rows (the body plus one row per
attachment) sharing `(conversation_id, sender_id, timestamp_ms)`;
`entry_seq` numbers them in insertion order. The partial unique index makes
replayed envelopes (reconnect redelivery) conflict-skip instead of
duplicating, while outgoing rows stay outside it because the send-confirm
flow rewrites their `timestamp_ms`.

System messages (`is_system = 1`) are used for join/leave notifications and
are excluded from unread counts.

### `reactions`

Emoji reactions on messages. One reaction per sender per message, with
the latest emoji replacing any previous one.

```sql
CREATE TABLE reactions (
    rowid           INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL,
    target_ts_ms    INTEGER NOT NULL,     -- timestamp of the reacted-to message
    target_author   TEXT NOT NULL,         -- author of the reacted-to message
    emoji           TEXT NOT NULL,
    sender          TEXT NOT NULL,         -- who sent this reaction
    UNIQUE(conversation_id, target_ts_ms, target_author, sender)
);

CREATE INDEX idx_reactions_target ON reactions(conversation_id, target_ts_ms);
```

### `poll_votes`

Votes on poll messages, one row per voter per poll (v11).

```sql
CREATE TABLE poll_votes (
    conv_id        TEXT NOT NULL,
    poll_timestamp INTEGER NOT NULL,
    voter          TEXT NOT NULL,
    voter_name     TEXT,
    option_indexes TEXT NOT NULL,
    vote_count     INTEGER NOT NULL DEFAULT 1,
    UNIQUE(conv_id, poll_timestamp, voter)
);
```

### `read_markers`

Tracks the last-read message per conversation for unread counting.

```sql
CREATE TABLE read_markers (
    conversation_id TEXT PRIMARY KEY REFERENCES conversations(id),
    last_read_rowid INTEGER NOT NULL DEFAULT 0
);
```

Unread count = incoming messages with `rowid > last_read_rowid`, excluding
system messages and your own sent messages. `/unread` moves the marker back
so the newest incoming message counts as unread again.

## Migrations

Migrations are version-based and run sequentially in `Database::migrate()`:

| Version | Changes |
|---|---|
| 1 | Initial schema: `conversations`, `messages`, `read_markers` tables |
| 2 | Add `muted` column to `conversations` |
| 3 | Add `status` and `timestamp_ms` columns to `messages` (delivery status tracking) |
| 4 | Create `reactions` table with unique constraint per sender per message |
| 5 | Add index on `messages(conversation_id, timestamp_ms)` for search performance |
| 6 | Add `is_edited`, `is_deleted`, `quote_author`, `quote_body`, `quote_ts_ms`, `sender_id` columns to `messages` |
| 7 | Add `expiration_timer` to `conversations` and `expires_in_seconds`, `expiration_start_ms` to `messages` |
| 8 | Add `accepted` column to `conversations` (message request tracking) |
| 9 | Add `blocked` column to `conversations` (block/unblock state) |
| 10 | Add `pinned` column to `messages` (pinned messages) |
| 11 | Add `poll_data` column to `messages` and create `poll_votes` table (polls) |
| 12 | Add `link_preview` column to `messages` (link preview persistence) |
| 13 | Add `body_raw` and `mentions_json` columns to `messages` (mention re-resolution) |
| 14 | Add `mute_expires_at` column to `conversations` (timed mutes) |
| 15 | Add `archived` column to `conversations` (archive) |
| 16 | Add `entry_seq` column to `messages` (backfilled per message key) and a partial unique index on incoming rows `(conversation_id, sender_id, timestamp_ms, entry_seq)` for replay deduplication |

Each migration is wrapped in a transaction. The `schema_version` table tracks
the current version.

## WAL mode

WAL mode is enabled on every connection:

```sql
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;
```

WAL allows concurrent readers while a writer is active, preventing database
locks during normal operation.

## In-memory mode

When running with `--incognito`, `Database::open_in_memory()` is used instead
of `Database::open()`. The same schema and migrations apply, but everything
lives in memory and is lost on exit.
