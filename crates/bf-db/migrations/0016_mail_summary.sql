-- What the HTML version of an activity summary shows (counts and topic cards), kept with the queued
-- mail: it cannot be read back from the text. Other mail has none.
ALTER TABLE mail_outbox ADD COLUMN summary jsonb;
