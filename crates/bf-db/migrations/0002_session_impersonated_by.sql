-- Who opened an impersonated session, so what happens in it can be traced back to the admin.
ALTER TABLE sessions
    ADD COLUMN IF NOT EXISTS impersonated_by bigint REFERENCES users(id) ON DELETE SET NULL;
