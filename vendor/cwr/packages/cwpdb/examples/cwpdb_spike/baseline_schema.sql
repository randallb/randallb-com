CREATE TABLE metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE workspace_binding (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                workspace_id TEXT NOT NULL,
                workspace_path BLOB NOT NULL
            );
            CREATE TABLE conversations (
                conversation_ref TEXT PRIMARY KEY,
                position INTEGER NOT NULL,
                creation_order INTEGER NOT NULL,
                title TEXT,
                archived INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1)),
                parent_conversation_ref TEXT,
                participant_actor_refs_json BLOB NOT NULL,
                last_message_ref TEXT,
                last_message_preview TEXT,
                revision INTEGER NOT NULL DEFAULT 0
            );
            CREATE UNIQUE INDEX conversations_by_creation
                ON conversations(creation_order, conversation_ref);
            CREATE INDEX root_conversations_by_creation
                ON conversations(creation_order, conversation_ref)
                WHERE parent_conversation_ref IS NULL;
            CREATE TABLE messages (
                message_ref TEXT PRIMARY KEY,
                conversation_ref TEXT NOT NULL REFERENCES conversations(conversation_ref) ON DELETE CASCADE,
                position INTEGER NOT NULL,
                payload_json BLOB NOT NULL
            );
            CREATE INDEX messages_by_conversation ON messages(conversation_ref, position);
            CREATE TABLE activities (
                activity_ref TEXT PRIMARY KEY,
                conversation_ref TEXT NOT NULL REFERENCES conversations(conversation_ref) ON DELETE CASCADE,
                position INTEGER NOT NULL,
                payload_json BLOB NOT NULL,
                activity_kind TEXT GENERATED ALWAYS AS (json_extract(CAST(payload_json AS TEXT), '$.event.kind')) VIRTUAL,
                message_ref TEXT GENERATED ALWAYS AS (json_extract(CAST(payload_json AS TEXT), '$.event.message_ref')) VIRTUAL,
                invocation_ref TEXT GENERATED ALWAYS AS (json_extract(CAST(payload_json AS TEXT), '$.event.invocation_ref')) VIRTUAL,
                source_message_ref TEXT GENERATED ALWAYS AS (json_extract(CAST(payload_json AS TEXT), '$.event.source_message_ref')) VIRTUAL
            );
            CREATE INDEX activities_by_conversation ON activities(conversation_ref, position);
            CREATE INDEX timeline_activities_by_conversation
                ON activities(conversation_ref, position)
                WHERE activity_kind IN ('message_recorded', 'turn_started');
            CREATE INDEX work_activities_by_invocation
                ON activities(conversation_ref, invocation_ref, position)
                WHERE activity_kind = 'work_update';
            CREATE TABLE timeline_items (
                conversation_ref TEXT NOT NULL REFERENCES conversations(conversation_ref) ON DELETE CASCADE,
                position INTEGER NOT NULL,
                item_kind TEXT NOT NULL,
                item_ref TEXT NOT NULL,
                PRIMARY KEY (conversation_ref, position),
                UNIQUE (conversation_ref, item_kind, item_ref)
            );
            CREATE TABLE timeline_messages (
                conversation_ref TEXT NOT NULL REFERENCES conversations(conversation_ref) ON DELETE CASCADE,
                position INTEGER NOT NULL,
                message_ref TEXT NOT NULL,
                payload_json BLOB NOT NULL,
                PRIMARY KEY (conversation_ref, message_ref)
            );
            CREATE TABLE timeline_activities (
                conversation_ref TEXT NOT NULL REFERENCES conversations(conversation_ref) ON DELETE CASCADE,
                position INTEGER NOT NULL,
                activity_ref TEXT NOT NULL,
                payload_json BLOB NOT NULL,
                activity_kind TEXT GENERATED ALWAYS AS (json_extract(CAST(payload_json AS TEXT), '$.event.kind')) VIRTUAL,
                invocation_ref TEXT GENERATED ALWAYS AS (json_extract(CAST(payload_json AS TEXT), '$.event.invocation_ref')) VIRTUAL,
                source_message_ref TEXT GENERATED ALWAYS AS (json_extract(CAST(payload_json AS TEXT), '$.event.source_message_ref')) VIRTUAL,
                PRIMARY KEY (conversation_ref, activity_ref)
            );
            CREATE INDEX timeline_public_activity_by_position
                ON timeline_activities(conversation_ref, position);
            CREATE INDEX timeline_public_work_by_invocation
                ON timeline_activities(conversation_ref, invocation_ref, position)
                WHERE activity_kind IN ('work_update', 'work_lifecycle');
            CREATE INDEX timeline_public_work_by_source_message
                ON timeline_activities(conversation_ref, source_message_ref, position)
                WHERE activity_kind IN ('work_update', 'work_lifecycle');
            CREATE TABLE admissions (
                integration_ref TEXT NOT NULL,
                provider_event_ref TEXT NOT NULL,
                position INTEGER NOT NULL,
                payload_json BLOB NOT NULL,
                PRIMARY KEY (integration_ref, provider_event_ref)
            );
            CREATE TABLE invocations (
                invocation_ref TEXT PRIMARY KEY,
                payload_json BLOB NOT NULL
            );
            CREATE TABLE invocation_waits (
                waiter_invocation_ref TEXT NOT NULL,
                call_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                payload_json BLOB NOT NULL,
                PRIMARY KEY (waiter_invocation_ref, call_id)
            );
            CREATE TABLE execution_conversation_refs (
                invocation_ref TEXT PRIMARY KEY,
                conversation_ref TEXT NOT NULL
            );