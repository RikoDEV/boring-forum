-- A member may ask for the summary once a month. The column and its claim clock
-- (`last_digest_at`) are the ones the daily and weekly digests already use.
ALTER TABLE users DROP CONSTRAINT IF EXISTS users_digest_check;
ALTER TABLE users ADD CONSTRAINT users_digest_check
    CHECK (digest = ANY (ARRAY['none'::text, 'daily'::text, 'weekly'::text, 'monthly'::text]));
