-- A public channel every member is put into: those already here when it is switched on, and each
-- account made after. Only an open channel can do this (everyone may read it anyway).
ALTER TABLE chat_channels ADD COLUMN auto_join boolean NOT NULL DEFAULT false;
ALTER TABLE chat_channels ADD CONSTRAINT chat_channels_auto_join_open
    CHECK (NOT auto_join OR (kind = 'public' AND access = 'open'));
