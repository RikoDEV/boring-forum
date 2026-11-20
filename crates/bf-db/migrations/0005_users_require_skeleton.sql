-- A local account is made only through `users::insert_user`, which stores the username skeleton
-- with the name. This trigger turns a path that forgets it into an error at insert time. It
-- guards inserts only: existing rows without a skeleton (the background job still fills those)
-- stay updatable, and a backup restore disables user triggers while it loads.
CREATE OR REPLACE FUNCTION users_require_skeleton() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    IF NOT NEW.remote AND NEW.username_skeleton IS NULL THEN
        RAISE EXCEPTION 'a local account needs its username skeleton (make it with users::insert_user)'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END
$$;

DROP TRIGGER IF EXISTS users_require_skeleton ON users;
CREATE TRIGGER users_require_skeleton BEFORE INSERT ON users
    FOR EACH ROW EXECUTE FUNCTION users_require_skeleton();
