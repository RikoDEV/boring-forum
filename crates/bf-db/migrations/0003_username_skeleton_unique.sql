-- One local account per confusable name skeleton. The application now fills `username_skeleton`
-- in the same transaction as the insert or rename (under an advisory lock) and refuses look-alikes
-- itself. This index is the last wall, created only when the existing data allows it: this
-- migration never changes or deletes rows, and an install that already holds look-alike names
-- keeps the plain index from the baseline and relies on the check in code.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM users WHERE username_skeleton IS NOT NULL AND NOT remote
        GROUP BY username_skeleton HAVING count(*) > 1
    ) THEN
        RAISE NOTICE 'users_username_skeleton_key not created: look-alike usernames already exist';
    ELSE
        CREATE UNIQUE INDEX IF NOT EXISTS users_username_skeleton_key
            ON users (username_skeleton) WHERE username_skeleton IS NOT NULL AND NOT remote;
    END IF;
END
$$;
