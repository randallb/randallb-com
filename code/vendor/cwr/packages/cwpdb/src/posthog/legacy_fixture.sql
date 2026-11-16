-- Synthetic retained-state fixture for the legacy PostHog startup migration.
-- Executed only against synthetic temporary databases by migration tests.
CREATE TABLE queue(id INTEGER PRIMARY KEY AUTOINCREMENT, destination TEXT NOT NULL, received INTEGER NOT NULL, event TEXT NOT NULL, quarantined INTEGER NOT NULL DEFAULT 0);
CREATE TABLE counters(key TEXT PRIMARY KEY, value INTEGER NOT NULL);
CREATE TABLE identity(value TEXT PRIMARY KEY);
INSERT INTO counters VALUES('dropped',17);
INSERT INTO counters VALUES('synthetic_extra',5);
INSERT INTO identity VALUES('00000000-0000-4000-8000-000000000001');
INSERT INTO queue VALUES(1,'synthetic-destination-a',1000,'{ "uuid": "00000000-0000-4000-8000-000000000011", "event": "cwr_metric", "properties": {} }',0);
INSERT INTO queue VALUES(4,'synthetic-destination-b',2000,'{"uuid":"00000000-0000-4000-8000-000000000014","event":"cwr_metric","properties":{}}',1);
INSERT INTO queue VALUES(9,'synthetic-destination-a',3000,'{malformed retained JSON',0);
-- Preserve the AUTOINCREMENT high-water value even when the newest row was deleted.
INSERT INTO queue VALUES(20,'synthetic-destination-a',4000,'{}',0);
DELETE FROM queue WHERE id=20;
