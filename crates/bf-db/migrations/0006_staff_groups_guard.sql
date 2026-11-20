-- Moderators and administrators are made through group membership, so a membership in a staff
-- group is a privilege. Promotion rules, referral rewards, club joins, plugin calls and bulk tools
-- all add members to groups; none of them may add one to a group that carries staff rights
-- (`is_staff`, `is_moderator` or `is_admin`, the built-in ids 3 and 4 included). The one path that
-- may is an administrator editing a member, which sets `bf.grant_staff` for its own transaction
-- (see `groups::add_members` with `Grant::Staff`). A backup restore disables user triggers while
-- it loads, so restoring is unaffected.
CREATE OR REPLACE FUNCTION user_groups_guard_staff() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    IF current_setting('bf.grant_staff', true) IS DISTINCT FROM 'on'
       AND EXISTS (SELECT 1 FROM groups g
                   WHERE g.id = NEW.group_id AND (g.is_staff OR g.is_moderator OR g.is_admin)) THEN
        RAISE EXCEPTION 'a staff group is granted only by an administrator editing the member'
            USING ERRCODE = 'insufficient_privilege';
    END IF;
    RETURN NEW;
END
$$;

DROP TRIGGER IF EXISTS user_groups_guard_staff ON user_secondary_groups;
CREATE TRIGGER user_groups_guard_staff BEFORE INSERT ON user_secondary_groups
    FOR EACH ROW EXECUTE FUNCTION user_groups_guard_staff();
