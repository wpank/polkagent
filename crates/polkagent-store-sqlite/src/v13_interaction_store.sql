-- Migration V13: durable interaction turns, run links, and replayable events.
--
-- Conversations and conversation_messages remain the transcript source of
-- truth. These tables add the execution correlation and event checkpoint
-- model required by terminal, TUI, API, and ACP surfaces.

CREATE TABLE IF NOT EXISTS interaction_turns (
    id                   TEXT    PRIMARY KEY, -- UUID v7
    conversation_id      TEXT    NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    ordinal               INTEGER NOT NULL CHECK (ordinal > 0),
    state                 TEXT    NOT NULL CHECK (
        state IN (
            'pending', 'running', 'awaiting_approval', 'completed',
            'failed', 'cancelled', 'timed_out'
        )
    ),
    target_json           TEXT    NOT NULL,
    config_json           TEXT    NOT NULL,
    user_message_id       TEXT    NOT NULL REFERENCES conversation_messages(id),
    assistant_message_id  TEXT    REFERENCES conversation_messages(id),
    started_at             TEXT    NOT NULL,
    completed_at           TEXT,
    error_json             TEXT,
    UNIQUE (conversation_id, ordinal)
);

CREATE INDEX IF NOT EXISTS idx_interaction_turns_conversation
    ON interaction_turns(conversation_id, ordinal);

CREATE INDEX IF NOT EXISTS idx_interaction_turns_active
    ON interaction_turns(conversation_id, state)
    WHERE state IN ('pending', 'running', 'awaiting_approval');

CREATE TABLE IF NOT EXISTS interaction_turn_runs (
    turn_id       TEXT    NOT NULL REFERENCES interaction_turns(id) ON DELETE CASCADE,
    run_id        TEXT    NOT NULL REFERENCES runs(id),
    role_json     TEXT    NOT NULL,
    ordinal       INTEGER NOT NULL CHECK (ordinal > 0),
    PRIMARY KEY (turn_id, run_id),
    UNIQUE (turn_id, ordinal)
);

CREATE INDEX IF NOT EXISTS idx_interaction_turn_runs_run
    ON interaction_turn_runs(run_id);

CREATE TABLE IF NOT EXISTS interaction_events (
    id               TEXT    PRIMARY KEY, -- UUID v7
    conversation_id  TEXT    NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    turn_id          TEXT    NOT NULL REFERENCES interaction_turns(id) ON DELETE CASCADE,
    sequence         INTEGER NOT NULL CHECK (sequence > 0),
    kind             TEXT    NOT NULL,
    payload_json     TEXT    NOT NULL,
    is_terminal      INTEGER NOT NULL DEFAULT 0 CHECK (is_terminal IN (0, 1)),
    created_at       TEXT    NOT NULL,
    UNIQUE (conversation_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_interaction_events_turn_sequence
    ON interaction_events(turn_id, sequence);

-- A turn may durably publish only one successful/failed/cancelled/timed-out
-- terminal envelope, even if a worker retries after losing its acknowledgement.
CREATE UNIQUE INDEX IF NOT EXISTS idx_interaction_events_one_terminal
    ON interaction_events(turn_id)
    WHERE is_terminal = 1;
