-- Boring Forum schema baseline. All earlier migrations are squashed into this one file.
-- Databases created before the squash cannot be upgraded: recreate them.

DO $$
BEGIN
    CREATE ROLE bf_explorer NOLOGIN;
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN insufficient_privilege THEN
        RAISE NOTICE 'bf_explorer not created: the Data Explorer stays off';
END
$$;

SET LOCAL check_function_bodies = false;

--
--

--
-- Name: explorer; Type: SCHEMA; Schema: -; Owner: -
--

CREATE SCHEMA explorer;

--
-- Name: pg_trgm; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;

--
-- Name: EXTENSION pg_trgm; Type: COMMENT; Schema: -; Owner: -
--

COMMENT ON EXTENSION pg_trgm IS 'text similarity measurement and index searching based on trigrams';

--
-- Name: unaccent; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS unaccent WITH SCHEMA public;

--
-- Name: EXTENSION unaccent; Type: COMMENT; Schema: -; Owner: -
--

COMMENT ON EXTENSION unaccent IS 'text search dictionary that removes accents';

--
-- Name: audit_clear_old_ips(integer); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.audit_clear_old_ips(days integer) RETURNS bigint
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
DECLARE n bigint;
BEGIN
    PERFORM set_config('bf.audit_maintenance', 'on', true);
    UPDATE audit_log SET ip = NULL WHERE ip IS NOT NULL AND created_at < now() - make_interval(days => days);
    GET DIAGNOSTICS n = ROW_COUNT;
    PERFORM set_config('bf.audit_maintenance', 'off', true);
    RETURN n;
END;
$$;

--
-- Name: audit_log_append_only(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.audit_log_append_only() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF current_setting('bf.audit_maintenance', true) = 'on' THEN
        RETURN NULL;
    END IF;
    RAISE EXCEPTION 'audit_log is append-only; only the retention job may change it';
END;
$$;

--
-- Name: audit_prune(integer); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.audit_prune(days integer) RETURNS bigint
    LANGUAGE plpgsql SECURITY DEFINER
    SET search_path TO 'public', 'pg_temp'
    AS $$
DECLARE n bigint;
BEGIN
    PERFORM set_config('bf.audit_maintenance', 'on', true);
    DELETE FROM audit_log WHERE created_at < now() - make_interval(days => days);
    GET DIAGNOSTICS n = ROW_COUNT;
    PERFORM set_config('bf.audit_maintenance', 'off', true);
    RETURN n;
END;
$$;

--
-- Name: bf_strip_hidden(text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.bf_strip_hidden(t text) RETURNS text
    LANGUAGE sql IMMUTABLE PARALLEL SAFE
    AS $$
    SELECT regexp_replace(regexp_replace(t, '\[hide\].*?\[/hide\]', '', 'g'), '```hide\M.*?(```|\Z)', '', 'g')
$$;

--
-- Name: bf_track_uploads(text, bigint, text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.bf_track_uploads(k text, rid bigint, body text) RETURNS void
    LANGUAGE plpgsql
    AS $$
BEGIN
    DELETE FROM upload_uses WHERE kind = k AND ref_id = rid;
    IF position('/uploads/' IN body) > 0 THEN
        INSERT INTO upload_uses (upload_id, kind, ref_id)
        SELECT DISTINCT u.id, k, rid
          FROM (SELECT (regexp_matches(body, '/uploads/([0-9a-f]{64}\.[a-z0-9]+)', 'g'))[1] AS key) x
          JOIN uploads u ON u.storage_key = x.key
        ON CONFLICT DO NOTHING;
    END IF;
END $$;

--
-- Name: bf_track_uploads_trg(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.bf_track_uploads_trg() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM upload_uses WHERE kind = TG_ARGV[0] AND ref_id = OLD.id;
        RETURN OLD;
    END IF;
    PERFORM bf_track_uploads(TG_ARGV[0], NEW.id, NEW.raw);
    RETURN NEW;
END $$;

--
-- Name: bf_unaccent(text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.bf_unaccent(t text) RETURNS text
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    AS $$
    SELECT public.unaccent('public.unaccent'::regdictionary, t)
$$;

--
-- Name: clear_stale_solved(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.clear_stale_solved() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF OLD.state = 'visible' AND (NEW.state <> 'visible' OR NEW.item_id <> OLD.item_id OR NEW.post_number <> OLD.post_number) THEN
        UPDATE items SET solved_post_number = NULL WHERE id = OLD.item_id AND solved_post_number = OLD.post_number;
    END IF;
    RETURN NEW;
END $$;

--
-- Name: in_quiet_hours(smallint, smallint, text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.in_quiet_hours(s smallint, e smallint, tz text) RETURNS boolean
    LANGUAGE plpgsql STABLE
    AS $$
DECLARE h int;
BEGIN
  IF s IS NULL OR e IS NULL OR s = e THEN
    RETURN false;
  END IF;
  BEGIN
    h := extract(hour FROM now() AT TIME ZONE coalesce(tz, 'UTC'));
  EXCEPTION WHEN others THEN
    h := extract(hour FROM now() AT TIME ZONE 'UTC'); -- an unknown zone name
  END;
  RETURN CASE WHEN s < e THEN h >= s AND h < e ELSE h >= s OR h < e END;
END;
$$;

--
-- Name: quiet_hours_end(smallint, smallint, text); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.quiet_hours_end(s smallint, e smallint, tz text) RETURNS timestamp with time zone
    LANGUAGE plpgsql STABLE
    AS $$
DECLARE local_now timestamp; ends timestamp; zone text := coalesce(tz, 'UTC');
BEGIN
  IF NOT in_quiet_hours(s, e, tz) THEN
    RETURN NULL;
  END IF;
  BEGIN
    local_now := now() AT TIME ZONE zone;
  EXCEPTION WHEN others THEN
    zone := 'UTC'; -- an unknown zone name
    local_now := now() AT TIME ZONE zone;
  END;
  ends := date_trunc('day', local_now) + e * interval '1 hour';
  IF ends <= local_now THEN
    ends := ends + interval '1 day';
  END IF;
  RETURN ends AT TIME ZONE zone;
END;
$$;

--
-- Name: users_email_changed(); Type: FUNCTION; Schema: public; Owner: -
--

CREATE FUNCTION public.users_email_changed() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    NEW.email_bounces := 0;
    NEW.email_bad := false;
    RETURN NEW;
END $$;

--
-- Name: bf_search; Type: TEXT SEARCH CONFIGURATION; Schema: public; Owner: -
--

CREATE TEXT SEARCH CONFIGURATION public.bf_search (
    PARSER = pg_catalog."default" );

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR asciiword WITH public.unaccent, english_stem, simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR word WITH public.unaccent, english_stem, simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR numword WITH public.unaccent, english_stem, simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR email WITH simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR url WITH simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR host WITH simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR sfloat WITH simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR version WITH simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR hword_numpart WITH public.unaccent, english_stem, simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR hword_part WITH public.unaccent, english_stem, simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR hword_asciipart WITH public.unaccent, english_stem, simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR numhword WITH public.unaccent, english_stem, simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR asciihword WITH public.unaccent, english_stem, simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR hword WITH public.unaccent, english_stem, simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR url_path WITH simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR file WITH simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR "float" WITH simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR "int" WITH simple;

ALTER TEXT SEARCH CONFIGURATION public.bf_search
    ADD MAPPING FOR uint WITH simple;

--
-- Name: topic_events; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.topic_events (
    item_id bigint NOT NULL,
    on_date date NOT NULL,
    reminded_at timestamp with time zone,
    starts_at timestamp with time zone NOT NULL,
    ends_at timestamp with time zone,
    timezone text DEFAULT 'UTC'::text NOT NULL,
    all_day boolean DEFAULT true NOT NULL,
    location text,
    recurrence text DEFAULT 'none'::text NOT NULL,
    repeat_until date,
    capacity integer,
    CONSTRAINT topic_events_capacity_check CHECK ((capacity > 0)),
    CONSTRAINT topic_events_ends_after CHECK (((ends_at IS NULL) OR (ends_at >= starts_at))),
    CONSTRAINT topic_events_recurrence_check CHECK ((recurrence = ANY (ARRAY['none'::text, 'daily'::text, 'weekly'::text, 'monthly'::text, 'yearly'::text])))
);

--
-- Name: events; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.events AS
 SELECT item_id AS topic_id,
    starts_at,
    ends_at,
    timezone,
    all_day,
    location,
    recurrence,
    capacity
   FROM public.topic_events;

--
-- Name: nodes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.nodes (
    id bigint NOT NULL,
    parent_id bigint,
    kind text NOT NULL,
    slug text NOT NULL,
    title text NOT NULL,
    description_md text DEFAULT ''::text NOT NULL,
    description_html text DEFAULT ''::text NOT NULL,
    redirect_url text,
    "position" integer DEFAULT 0 NOT NULL,
    settings jsonb DEFAULT '{}'::jsonb NOT NULL,
    topic_count integer DEFAULT 0 NOT NULL,
    post_count integer DEFAULT 0 NOT NULL,
    last_item_id bigint,
    last_activity_at timestamp with time zone,
    default_sort text,
    seo_title text,
    seo_description text,
    list_style text DEFAULT 'table'::text NOT NULL,
    approve_mode text DEFAULT 'none'::text NOT NULL,
    counts_posts boolean DEFAULT true NOT NULL,
    min_trust_level smallint DEFAULT 0 NOT NULL,
    require_tag boolean DEFAULT false NOT NULL,
    clicks integer DEFAULT 0 NOT NULL,
    theme_id bigint,
    mode text DEFAULT 'discussion'::text NOT NULL,
    auto_close_days integer,
    plugin text,
    plugin_key text,
    inherit_perms boolean DEFAULT false NOT NULL,
    CONSTRAINT nodes_approve_mode_check CHECK ((approve_mode = ANY (ARRAY['none'::text, 'all'::text, 'links'::text]))),
    CONSTRAINT nodes_auto_close_days_check CHECK ((auto_close_days > 0)),
    CONSTRAINT nodes_kind_check CHECK ((kind = ANY (ARRAY['category'::text, 'forum'::text, 'link'::text, 'archive'::text]))),
    CONSTRAINT nodes_list_style_check CHECK ((list_style = ANY (ARRAY['table'::text, 'list'::text]))),
    CONSTRAINT nodes_mode_check CHECK ((mode = ANY (ARRAY['discussion'::text, 'qa'::text, 'ideas'::text])))
);

--
-- Name: forums; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.forums AS
 SELECT id,
    parent_id,
    kind,
    slug,
    title,
    "position",
    topic_count,
    post_count,
    mode
   FROM public.nodes;

--
-- Name: groups; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.groups (
    id bigint NOT NULL,
    name text NOT NULL,
    slug text NOT NULL,
    is_staff boolean DEFAULT false NOT NULL,
    is_moderator boolean DEFAULT false NOT NULL,
    is_admin boolean DEFAULT false NOT NULL,
    colour text,
    limits jsonb DEFAULT '{}'::jsonb NOT NULL,
    require_2fa boolean DEFAULT false NOT NULL,
    icon text,
    image text,
    plugin text,
    plugin_key text,
    CONSTRAINT groups_colour_hex CHECK (((colour IS NULL) OR (colour ~ '^#[0-9a-f]{6}$'::text))),
    CONSTRAINT groups_icon_check CHECK ((icon ~ '^[a-z0-9-]{1,40}$'::text)),
    CONSTRAINT groups_image_check CHECK (((image ~ '^/uploads/[A-Za-z0-9._/-]{1,190}$'::text) AND (image !~ '\.\.'::text)))
);

--
-- Name: groups; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.groups AS
 SELECT id,
    name,
    slug
   FROM public.groups;

--
-- Name: post_likes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.post_likes (
    post_id bigint NOT NULL,
    user_id bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    reaction_id bigint DEFAULT 1 NOT NULL
);

--
-- Name: likes; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.likes AS
 SELECT post_id,
    user_id,
    reaction_id
   FROM public.post_likes;

--
-- Name: poll_votes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.poll_votes (
    item_id bigint NOT NULL,
    user_id bigint NOT NULL,
    option_id bigint NOT NULL,
    poll_id bigint NOT NULL,
    rank smallint
);

--
-- Name: poll_votes; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.poll_votes AS
 SELECT poll_id,
    user_id,
    option_id
   FROM public.poll_votes;

--
-- Name: polls; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.polls (
    id bigint NOT NULL,
    item_id bigint NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    question text NOT NULL,
    max_choices integer DEFAULT 1 NOT NULL,
    closes_at timestamp with time zone,
    results text DEFAULT 'always'::text NOT NULL,
    closed_notified_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    kind text DEFAULT 'choice'::text NOT NULL,
    is_public boolean DEFAULT false NOT NULL,
    vote_change boolean DEFAULT true NOT NULL,
    min_trust_level smallint DEFAULT 0 NOT NULL,
    post_number integer,
    CONSTRAINT polls_kind_check CHECK ((kind = ANY (ARRAY['choice'::text, 'ranked'::text, 'rating'::text]))),
    CONSTRAINT polls_max_choices_check CHECK ((max_choices >= 1)),
    CONSTRAINT polls_min_trust_level_check CHECK ((min_trust_level >= 0)),
    CONSTRAINT polls_results_check CHECK ((results = ANY (ARRAY['always'::text, 'voted'::text, 'closed'::text, 'staff'::text])))
);

--
-- Name: polls; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.polls AS
 SELECT id,
    item_id AS topic_id,
    question,
    max_choices,
    closes_at,
    results,
    created_at
   FROM public.polls;

--
-- Name: posts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.posts (
    id bigint NOT NULL,
    item_id bigint NOT NULL,
    author_id bigint,
    post_number integer NOT NULL,
    reply_to_post_id bigint,
    raw text NOT NULL,
    cooked text NOT NULL,
    cooked_version integer NOT NULL,
    action text,
    state text DEFAULT 'visible'::text NOT NULL,
    edit_count integer DEFAULT 0 NOT NULL,
    last_editor_id bigint,
    like_count integer DEFAULT 0 NOT NULL,
    ip inet,
    hidden_reason text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    deleted_by bigint,
    edit_locked boolean DEFAULT false NOT NULL,
    wiki boolean DEFAULT false NOT NULL,
    vote_score integer DEFAULT 0 NOT NULL,
    real_author_id bigint,
    search_vector tsvector GENERATED ALWAYS AS (to_tsvector('public.bf_search'::regconfig, public.bf_strip_hidden(raw))) STORED,
    CONSTRAINT posts_state_check CHECK ((state = ANY (ARRAY['visible'::text, 'hidden'::text, 'pending'::text, 'deleted'::text])))
);

--
-- Name: posts; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.posts AS
 SELECT id,
    item_id AS topic_id,
    author_id,
    post_number,
    reply_to_post_id,
    state,
    like_count,
    vote_score,
    edit_count,
    action,
    created_at,
    length(raw) AS length,
    raw
   FROM public.posts;

--
-- Name: reports; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.reports (
    id bigint NOT NULL,
    target_type text NOT NULL,
    target_id bigint NOT NULL,
    reporter_id bigint,
    reason text NOT NULL,
    note text,
    status text DEFAULT 'open'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT reports_status_check CHECK ((status = ANY (ARRAY['open'::text, 'resolved'::text, 'dismissed'::text])))
);

--
-- Name: reports; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.reports AS
 SELECT id,
    target_type,
    target_id,
    reporter_id,
    reason,
    status,
    created_at
   FROM public.reports;

--
-- Name: event_rsvps; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.event_rsvps (
    item_id bigint NOT NULL,
    user_id bigint NOT NULL,
    status text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT event_rsvps_status_check CHECK ((status = ANY (ARRAY['going'::text, 'maybe'::text, 'not_going'::text])))
);

--
-- Name: rsvps; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.rsvps AS
 SELECT item_id AS topic_id,
    user_id,
    status,
    created_at
   FROM public.event_rsvps;

--
-- Name: tags; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tags (
    id bigint NOT NULL,
    name text NOT NULL,
    slug text NOT NULL,
    plugin text,
    plugin_key text
);

--
-- Name: tags; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.tags AS
 SELECT id,
    name,
    slug
   FROM public.tags;

--
-- Name: item_tags; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.item_tags (
    item_id bigint NOT NULL,
    tag_id bigint NOT NULL
);

--
-- Name: topic_tags; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.topic_tags AS
 SELECT item_id AS topic_id,
    tag_id
   FROM public.item_tags;

--
-- Name: items; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.items (
    id bigint NOT NULL,
    node_id bigint NOT NULL,
    app text DEFAULT 'forums'::text NOT NULL,
    kind text DEFAULT 'topic'::text NOT NULL,
    author_id bigint,
    title text NOT NULL,
    slug text NOT NULL,
    state text DEFAULT 'visible'::text NOT NULL,
    listed boolean DEFAULT true NOT NULL,
    pinned boolean DEFAULT false NOT NULL,
    pinned_until timestamp with time zone,
    featured boolean DEFAULT false NOT NULL,
    locked boolean DEFAULT false NOT NULL,
    archived boolean DEFAULT false NOT NULL,
    reply_count integer DEFAULT 0 NOT NULL,
    view_count integer DEFAULT 0 NOT NULL,
    like_count integer DEFAULT 0 NOT NULL,
    last_post_id bigint,
    last_posted_at timestamp with time zone DEFAULT now() NOT NULL,
    last_poster_id bigint,
    meta jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    deleted_by bigint,
    highest_post_number integer DEFAULT 0 NOT NULL,
    prefix text,
    pinned_global boolean DEFAULT false NOT NULL,
    solved_post_number integer,
    slow_mode_seconds integer DEFAULT 0 NOT NULL,
    poll_question text,
    assigned_to bigint,
    poll_closed_at timestamp with time zone,
    vote_count integer DEFAULT 0 NOT NULL,
    search_vector tsvector GENERATED ALWAYS AS (to_tsvector('public.bf_search'::regconfig, title)) STORED,
    CONSTRAINT items_state_check CHECK ((state = ANY (ARRAY['visible'::text, 'hidden'::text, 'pending'::text, 'deleted'::text])))
);

--
-- Name: topics; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.topics AS
 SELECT id,
    node_id AS forum_id,
    author_id,
    title,
    slug,
    state,
    pinned,
    locked,
    archived,
    listed,
    reply_count,
    view_count,
    like_count,
    vote_count,
    created_at,
    last_posted_at,
    solved_post_number
   FROM public.items;

--
-- Name: user_badges; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_badges (
    user_id bigint NOT NULL,
    badge_id bigint NOT NULL,
    earned_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: user_badges; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.user_badges AS
 SELECT user_id,
    badge_id,
    earned_at
   FROM public.user_badges;

--
-- Name: user_secondary_groups; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_secondary_groups (
    user_id bigint NOT NULL,
    group_id bigint NOT NULL
);

--
-- Name: user_groups; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.user_groups AS
 SELECT user_id,
    group_id
   FROM public.user_secondary_groups;

--
-- Name: users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.users (
    id bigint NOT NULL,
    username text NOT NULL,
    username_lower text NOT NULL,
    display_name text,
    email text NOT NULL,
    email_lower text NOT NULL,
    email_verified_at timestamp with time zone,
    password_hash text,
    primary_group_id bigint DEFAULT 2 NOT NULL,
    trust_level smallint DEFAULT 0 NOT NULL,
    post_count integer DEFAULT 0 NOT NULL,
    approved boolean DEFAULT true NOT NULL,
    title text,
    about_md text,
    about_html text,
    timezone text,
    locale text,
    suspended_until timestamp with time zone,
    silenced_until timestamp with time zone,
    joined_at timestamp with time zone DEFAULT now() NOT NULL,
    last_seen_at timestamp with time zone,
    flags jsonb DEFAULT '{}'::jsonb NOT NULL,
    totp_secret text,
    totp_last_step bigint,
    location text,
    website text,
    avatar_upload_id bigint,
    digest text DEFAULT 'none'::text NOT NULL,
    last_digest_at timestamp with time zone,
    locked_until timestamp with time zone,
    text_size smallint DEFAULT 100 NOT NULL,
    density text DEFAULT 'comfortable'::text NOT NULL,
    cover_upload_id bigint,
    hide_last_seen boolean DEFAULT false NOT NULL,
    email_bounces integer DEFAULT 0 NOT NULL,
    email_bad boolean DEFAULT false NOT NULL,
    infinite_scroll boolean DEFAULT false NOT NULL,
    date_format text DEFAULT 'relative'::text NOT NULL,
    skin_tone smallint DEFAULT 0 NOT NULL,
    profile_visibility text DEFAULT 'everyone'::text NOT NULL,
    theme_id bigint,
    color_scheme text,
    reputation bigint DEFAULT 0 NOT NULL,
    delete_after timestamp with time zone,
    signature_md text,
    signature_html text,
    dnd_until timestamp with time zone,
    show_visitors boolean DEFAULT false NOT NULL,
    mail_post_key text,
    profile_views bigint DEFAULT 0 NOT NULL,
    hide_followers boolean DEFAULT false NOT NULL,
    cover_y smallint DEFAULT 50 NOT NULL,
    quiet_start smallint,
    quiet_end smallint,
    mfa_failures integer DEFAULT 0 NOT NULL,
    mfa_locked_until timestamp with time zone,
    login_lockouts integer DEFAULT 0 NOT NULL,
    username_skeleton text,
    remote boolean DEFAULT false NOT NULL,
    trust_level_locked boolean DEFAULT false NOT NULL,
    referred_by bigint,
    newsletter boolean DEFAULT false NOT NULL,
    newsletter_changed_at timestamp with time zone,
    newsletter_synced_at timestamp with time zone,
    ap_also_known_as text[] DEFAULT '{}'::text[] NOT NULL,
    ap_moved_to text,
    login_cleared_at timestamp with time zone,
    trust_nudged_level smallint DEFAULT 0 NOT NULL,
    allow_pm_from text DEFAULT 'everyone'::text NOT NULL,
    delete_content boolean DEFAULT false NOT NULL,
    welcome_dismissed_at timestamp with time zone,
    visit_credit integer DEFAULT 0 NOT NULL,
    read_credit integer DEFAULT 0 NOT NULL,
    CONSTRAINT users_allow_pm_from_check CHECK ((allow_pm_from = ANY (ARRAY['everyone'::text, 'following'::text, 'nobody'::text]))),
    CONSTRAINT users_cover_y_check CHECK (((cover_y >= 0) AND (cover_y <= 100))),
    CONSTRAINT users_date_format_check CHECK ((date_format = ANY (ARRAY['relative'::text, 'iso'::text, 'locale'::text]))),
    CONSTRAINT users_density_check CHECK ((density = ANY (ARRAY['comfortable'::text, 'compact'::text]))),
    CONSTRAINT users_digest_check CHECK ((digest = ANY (ARRAY['none'::text, 'daily'::text, 'weekly'::text]))),
    CONSTRAINT users_profile_visibility_check CHECK ((profile_visibility = ANY (ARRAY['everyone'::text, 'members'::text, 'nobody'::text]))),
    CONSTRAINT users_quiet_end_check CHECK (((quiet_end >= 0) AND (quiet_end <= 23))),
    CONSTRAINT users_quiet_start_check CHECK (((quiet_start >= 0) AND (quiet_start <= 23))),
    CONSTRAINT users_read_credit_check CHECK ((read_credit >= 0)),
    CONSTRAINT users_skin_tone_check CHECK (((skin_tone >= 0) AND (skin_tone <= 5))),
    CONSTRAINT users_text_size_check CHECK (((text_size >= 90) AND (text_size <= 125))),
    CONSTRAINT users_visit_credit_check CHECK ((visit_credit >= 0))
);

--
-- Name: users; Type: VIEW; Schema: explorer; Owner: -
--

CREATE VIEW explorer.users AS
 SELECT id,
    username,
    display_name,
    primary_group_id,
    trust_level,
    post_count,
    reputation,
    joined_at,
    last_seen_at,
    approved,
    suspended_until,
    silenced_until,
    (email_verified_at IS NOT NULL) AS email_verified,
    locale,
    remote
   FROM public.users;

--
-- Name: admin_notices; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.admin_notices (
    id bigint NOT NULL,
    kind text NOT NULL,
    message text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    dismissed_at timestamp with time zone
);

--
-- Name: admin_notices_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.admin_notices ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.admin_notices_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: ads; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ads (
    id bigint NOT NULL,
    name text NOT NULL,
    placement text NOT NULL,
    html text NOT NULL,
    url text,
    show_groups bigint[] DEFAULT '{}'::bigint[] NOT NULL,
    hide_groups bigint[] DEFAULT '{}'::bigint[] NOT NULL,
    every integer DEFAULT 5 NOT NULL,
    weight integer DEFAULT 1 NOT NULL,
    starts_at timestamp with time zone,
    ends_at timestamp with time zone,
    active boolean DEFAULT true NOT NULL,
    impressions bigint DEFAULT 0 NOT NULL,
    clicks bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ads_every_check CHECK (((every >= 1) AND (every <= 100))),
    CONSTRAINT ads_name_check CHECK (((length(name) >= 1) AND (length(name) <= 100))),
    CONSTRAINT ads_placement_check CHECK ((placement = ANY (ARRAY['header'::text, 'between_posts'::text, 'topic_list'::text, 'sidebar'::text, 'footer'::text]))),
    CONSTRAINT ads_weight_check CHECK (((weight >= 1) AND (weight <= 100)))
);

--
-- Name: ads_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.ads ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.ads_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: ai_embeddings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_embeddings (
    item_id bigint NOT NULL,
    node_id bigint NOT NULL,
    model text NOT NULL,
    dims integer NOT NULL,
    vec bytea NOT NULL,
    text_hash text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: ai_personas; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_personas (
    id bigint NOT NULL,
    slug text NOT NULL,
    name text NOT NULL,
    system_prompt text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    model text DEFAULT ''::text NOT NULL,
    use_search boolean DEFAULT true NOT NULL,
    max_tokens integer DEFAULT 800 NOT NULL,
    group_ids bigint[] DEFAULT '{}'::bigint[] NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ai_personas_max_tokens_check CHECK (((max_tokens >= 50) AND (max_tokens <= 4000))),
    CONSTRAINT ai_personas_slug_check CHECK ((slug ~ '^[a-z0-9-]{2,20}$'::text))
);

--
-- Name: ai_personas_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.ai_personas ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.ai_personas_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: ai_post_labels; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_post_labels (
    post_id bigint NOT NULL,
    spam smallint DEFAULT 0 NOT NULL,
    toxicity smallint DEFAULT 0 NOT NULL,
    sentiment text DEFAULT 'neutral'::text NOT NULL,
    emotion text DEFAULT ''::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ai_post_labels_sentiment_check CHECK ((sentiment = ANY (ARRAY['positive'::text, 'neutral'::text, 'negative'::text])))
);

--
-- Name: ai_summaries; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_summaries (
    item_id bigint NOT NULL,
    kind text NOT NULL,
    locale text NOT NULL,
    up_to_post integer NOT NULL,
    body text NOT NULL,
    model text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ai_summaries_kind_check CHECK ((kind = ANY (ARRAY['summary'::text, 'gist'::text])))
);

--
-- Name: ai_usage; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ai_usage (
    id bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    feature text NOT NULL,
    model text NOT NULL,
    user_id bigint,
    prompt_tokens integer DEFAULT 0 NOT NULL,
    completion_tokens integer DEFAULT 0 NOT NULL,
    ok boolean DEFAULT true NOT NULL,
    error text
);

--
-- Name: ai_usage_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.ai_usage ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.ai_usage_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: announcements; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.announcements (
    id bigint NOT NULL,
    title text NOT NULL,
    body text NOT NULL,
    body_html text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    starts_at timestamp with time zone,
    ends_at timestamp with time zone
);

--
-- Name: announcements_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.announcements ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.announcements_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: ap_actors; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ap_actors (
    id bigint NOT NULL,
    uri text NOT NULL,
    domain text NOT NULL,
    kind text NOT NULL,
    username text NOT NULL,
    display_name text,
    url text,
    inbox text NOT NULL,
    shared_inbox text,
    key_id text NOT NULL,
    public_pem text NOT NULL,
    user_id bigint,
    fetched_at timestamp with time zone DEFAULT now() NOT NULL,
    moved_to text
);

--
-- Name: ap_actors_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.ap_actors ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.ap_actors_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: ap_deliveries; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ap_deliveries (
    id bigint NOT NULL,
    inbox text NOT NULL,
    signer_type text NOT NULL,
    signer_id bigint NOT NULL,
    activity jsonb NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    next_at timestamp with time zone DEFAULT now() NOT NULL,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: ap_deliveries_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.ap_deliveries ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.ap_deliveries_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: ap_dm_objects; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ap_dm_objects (
    uri text NOT NULL,
    message_id bigint NOT NULL,
    conversation_id bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: ap_followers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ap_followers (
    local_type text NOT NULL,
    local_id bigint NOT NULL,
    actor_id bigint NOT NULL,
    follow_id text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ap_followers_local_type_check CHECK ((local_type = ANY (ARRAY['user'::text, 'forum'::text])))
);

--
-- Name: ap_keys; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ap_keys (
    owner_type text NOT NULL,
    owner_id bigint NOT NULL,
    public_pem text NOT NULL,
    private_pem text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ap_keys_owner_type_check CHECK ((owner_type = ANY (ARRAY['instance'::text, 'user'::text, 'forum'::text])))
);

--
-- Name: ap_mirrors; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ap_mirrors (
    node_id bigint NOT NULL,
    actor_id bigint NOT NULL,
    follow_id text NOT NULL,
    accepted boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: ap_objects; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ap_objects (
    uri text NOT NULL,
    post_id bigint NOT NULL,
    actor_id bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: ap_seen; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ap_seen (
    activity_id text NOT NULL,
    seen_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: api_idempotency; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.api_idempotency (
    key_digest text NOT NULL,
    fingerprint text NOT NULL,
    completed boolean DEFAULT false NOT NULL,
    status smallint,
    content_type text,
    body bytea,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: api_keys; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.api_keys (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    name text NOT NULL,
    key_digest text NOT NULL,
    scope text NOT NULL,
    last_used_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone,
    oauth_client text,
    plugin_scopes text[] DEFAULT '{}'::text[] NOT NULL,
    CONSTRAINT api_keys_scope_check CHECK ((scope = ANY (ARRAY['read'::text, 'write'::text, 'admin'::text])))
);

--
-- Name: api_keys_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.api_keys ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.api_keys_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: audit_forward_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.audit_forward_state (
    id smallint DEFAULT 1 NOT NULL,
    last_id bigint DEFAULT 0 NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT audit_forward_state_id_check CHECK ((id = 1))
);

--
-- Name: audit_log; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.audit_log (
    id bigint NOT NULL,
    actor_id bigint,
    action text NOT NULL,
    target text,
    details jsonb DEFAULT '{}'::jsonb NOT NULL,
    ip inet,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: audit_log_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.audit_log ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.audit_log_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: automations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.automations (
    id bigint NOT NULL,
    name text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    trigger text NOT NULL,
    conditions jsonb DEFAULT '{}'::jsonb NOT NULL,
    actions jsonb DEFAULT '[]'::jsonb NOT NULL,
    last_run_at timestamp with time zone,
    run_count bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT automations_trigger_check CHECK ((trigger = ANY (ARRAY['topic_created'::text, 'post_created'::text, 'daily'::text, 'weekly'::text, 'monthly'::text])))
);

--
-- Name: automations_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.automations ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.automations_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: backup_codes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.backup_codes (
    user_id bigint NOT NULL,
    code_digest text NOT NULL,
    used_at timestamp with time zone
);

--
-- Name: badges; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.badges (
    id bigint NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    min_posts integer DEFAULT 0 NOT NULL,
    min_reputation bigint DEFAULT 0 NOT NULL,
    min_solved integer DEFAULT 0 NOT NULL,
    points integer DEFAULT 0 NOT NULL,
    slug text,
    enabled boolean DEFAULT true NOT NULL,
    tier text DEFAULT 'bronze'::text NOT NULL,
    icon text DEFAULT 'trophy'::text NOT NULL,
    category text DEFAULT 'Custom'::text NOT NULL,
    backfilled boolean DEFAULT false NOT NULL,
    color smallint DEFAULT 1 NOT NULL,
    level integer DEFAULT 0 NOT NULL,
    CONSTRAINT badges_tier_check CHECK ((tier = ANY (ARRAY['bronze'::text, 'silver'::text, 'gold'::text])))
);

--
-- Name: badges_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.badges ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.badges_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: bans; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.bans (
    id bigint NOT NULL,
    kind text NOT NULL,
    value text NOT NULL,
    reason text,
    created_by bigint,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone,
    CONSTRAINT bans_kind_check CHECK ((kind = ANY (ARRAY['ip'::text, 'email'::text, 'asn'::text, 'ip_allow'::text])))
);

--
-- Name: bans_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.bans ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.bans_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: bookmarks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.bookmarks (
    user_id bigint NOT NULL,
    post_id bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    remind_at timestamp with time zone
);

--
-- Name: captcha_spent; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.captcha_spent (
    signature text NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

--
-- Name: chat_channel_groups; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.chat_channel_groups (
    channel_id bigint NOT NULL,
    group_id bigint NOT NULL
);

--
-- Name: chat_channels; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.chat_channels (
    id bigint NOT NULL,
    kind text NOT NULL,
    name text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    archived boolean DEFAULT false NOT NULL,
    dm_key text,
    created_by bigint,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    access text DEFAULT 'open'::text NOT NULL,
    CONSTRAINT chat_channels_access_check CHECK ((access = ANY (ARRAY['open'::text, 'groups'::text, 'invite'::text]))),
    CONSTRAINT chat_channels_kind_check CHECK ((kind = ANY (ARRAY['public'::text, 'dm'::text])))
);

--
-- Name: chat_channels_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.chat_channels_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: chat_channels_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.chat_channels_id_seq OWNED BY public.chat_channels.id;

--
-- Name: chat_flags; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.chat_flags (
    message_id bigint NOT NULL,
    user_id bigint NOT NULL,
    reason text DEFAULT ''::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: chat_integration_rules; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.chat_integration_rules (
    id bigint NOT NULL,
    provider text NOT NULL,
    url text DEFAULT ''::text NOT NULL,
    token text DEFAULT ''::text NOT NULL,
    channel text DEFAULT ''::text NOT NULL,
    filter text DEFAULT 'watch'::text NOT NULL,
    node_id bigint,
    tag text DEFAULT ''::text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    CONSTRAINT chat_integration_rules_filter_check CHECK ((filter = ANY (ARRAY['follow'::text, 'watch'::text, 'mute'::text])))
);

--
-- Name: chat_integration_rules_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.chat_integration_rules_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: chat_integration_rules_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.chat_integration_rules_id_seq OWNED BY public.chat_integration_rules.id;

--
-- Name: chat_members; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.chat_members (
    channel_id bigint NOT NULL,
    user_id bigint NOT NULL,
    last_read_id bigint DEFAULT 0 NOT NULL,
    muted boolean DEFAULT false NOT NULL,
    joined_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: chat_messages; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.chat_messages (
    id bigint NOT NULL,
    channel_id bigint NOT NULL,
    user_id bigint,
    raw text NOT NULL,
    cooked text NOT NULL,
    reply_to_id bigint,
    pinned boolean DEFAULT false NOT NULL,
    edited_at timestamp with time zone,
    deleted_at timestamp with time zone,
    deleted_by bigint,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    auto_hidden boolean DEFAULT false NOT NULL
);

--
-- Name: chat_messages_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.chat_messages_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: chat_messages_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.chat_messages_id_seq OWNED BY public.chat_messages.id;

--
-- Name: chat_reactions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.chat_reactions (
    message_id bigint NOT NULL,
    user_id bigint NOT NULL,
    emoji text NOT NULL
);

--
-- Name: club_bans; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.club_bans (
    club_id bigint NOT NULL,
    user_id bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: club_invites; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.club_invites (
    club_id bigint NOT NULL,
    user_id bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: clubs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.clubs (
    id bigint NOT NULL,
    name text NOT NULL,
    node_id bigint NOT NULL,
    group_id bigint NOT NULL,
    owner_id bigint,
    open boolean DEFAULT true NOT NULL,
    archived boolean DEFAULT false NOT NULL
);

--
-- Name: clubs_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.clubs ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.clubs_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: color_schemes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.color_schemes (
    id bigint NOT NULL,
    slug text NOT NULL,
    name text NOT NULL,
    light jsonb DEFAULT '{}'::jsonb NOT NULL,
    dark jsonb DEFAULT '{}'::jsonb NOT NULL,
    selectable boolean DEFAULT true NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    CONSTRAINT color_schemes_slug_check CHECK ((slug ~ '^[a-z0-9-]{1,40}$'::text))
);

--
-- Name: color_schemes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.color_schemes ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.color_schemes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: content_translations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.content_translations (
    kind text NOT NULL,
    target_id bigint NOT NULL,
    locale text NOT NULL,
    title text DEFAULT ''::text NOT NULL,
    raw text DEFAULT ''::text NOT NULL,
    cooked text DEFAULT ''::text NOT NULL,
    translator_id bigint,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    machine boolean DEFAULT false NOT NULL,
    CONSTRAINT content_translations_kind_check CHECK ((kind = ANY (ARRAY['post'::text, 'node'::text]))),
    CONSTRAINT content_translations_locale_check CHECK ((locale ~ '^[a-z]{2}$'::text))
);

--
-- Name: conversation_messages; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.conversation_messages (
    id bigint NOT NULL,
    conversation_id bigint NOT NULL,
    author_id bigint,
    raw text NOT NULL,
    cooked text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: conversation_messages_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.conversation_messages_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: conversation_messages_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.conversation_messages_id_seq OWNED BY public.conversation_messages.id;

--
-- Name: conversation_participants; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.conversation_participants (
    conversation_id bigint NOT NULL,
    user_id bigint NOT NULL,
    last_read_id bigint DEFAULT 0 NOT NULL,
    archived boolean DEFAULT false NOT NULL,
    left_at timestamp with time zone,
    folder text
);

--
-- Name: conversation_reports; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.conversation_reports (
    id bigint NOT NULL,
    conversation_id bigint NOT NULL,
    reporter_id bigint,
    reason text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    resolved_at timestamp with time zone,
    resolved_by bigint
);

--
-- Name: conversation_reports_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.conversation_reports ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.conversation_reports_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: conversations; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.conversations (
    id bigint NOT NULL,
    subject text NOT NULL,
    starter_id bigint,
    last_message_id bigint DEFAULT 0 NOT NULL,
    last_message_at timestamp with time zone DEFAULT now() NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: conversations_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.conversations_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: conversations_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.conversations_id_seq OWNED BY public.conversations.id;

--
-- Name: custom_emoji; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.custom_emoji (
    name text NOT NULL,
    url text NOT NULL
);

--
-- Name: custom_pages; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.custom_pages (
    id bigint NOT NULL,
    slug text NOT NULL,
    title text NOT NULL,
    visibility text DEFAULT 'public'::text NOT NULL,
    published boolean DEFAULT false NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    path text,
    template text DEFAULT 'single'::text NOT NULL,
    CONSTRAINT custom_pages_path_check CHECK ((path ~ '^/[a-z0-9][a-z0-9-]{0,59}(/[a-z0-9][a-z0-9-]{0,59}){0,2}$'::text)),
    CONSTRAINT custom_pages_template_check CHECK ((template = ANY (ARRAY['single'::text, 'sidebar'::text, 'columns2'::text, 'columns3'::text]))),
    CONSTRAINT custom_pages_visibility_check CHECK ((visibility = ANY (ARRAY['public'::text, 'members'::text, 'staff'::text])))
);

--
-- Name: custom_pages_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.custom_pages ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.custom_pages_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: drafts; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.drafts (
    user_id bigint NOT NULL,
    context text NOT NULL,
    data jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: email_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.email_tokens (
    token_digest text NOT NULL,
    user_id bigint NOT NULL,
    purpose text NOT NULL,
    payload text,
    expires_at timestamp with time zone NOT NULL,
    used_at timestamp with time zone,
    CONSTRAINT email_tokens_purpose_check CHECK ((purpose = ANY (ARRAY['verify'::text, 'reset'::text, 'email_change'::text, 'magic'::text, 'backup'::text])))
);

--
-- Name: embed_topics; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.embed_topics (
    url text NOT NULL,
    item_id bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: error_log; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.error_log (
    id bigint NOT NULL,
    method text NOT NULL,
    path text NOT NULL,
    status integer NOT NULL,
    request_id text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: error_log_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.error_log ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.error_log_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: explorer_queries; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.explorer_queries (
    id bigint NOT NULL,
    name text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    sql text NOT NULL,
    created_by bigint,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: explorer_queries_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.explorer_queries ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.explorer_queries_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: failed_logins; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.failed_logins (
    id bigint NOT NULL,
    login text NOT NULL,
    user_id bigint,
    ip inet,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: failed_logins_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.failed_logins ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.failed_logins_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: follows; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.follows (
    user_id bigint NOT NULL,
    target_type text NOT NULL,
    target_id bigint NOT NULL,
    level text NOT NULL,
    email_freq text DEFAULT 'none'::text NOT NULL,
    CONSTRAINT follows_email_freq_check CHECK ((email_freq = ANY (ARRAY['none'::text, 'immediate'::text, 'daily'::text, 'weekly'::text]))),
    CONSTRAINT follows_level_check CHECK ((level = ANY (ARRAY['watch'::text, 'track'::text, 'normal'::text, 'mute'::text]))),
    CONSTRAINT follows_target_type_check CHECK ((target_type = ANY (ARRAY['item'::text, 'node'::text])))
);

--
-- Name: group_requests; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.group_requests (
    group_id bigint NOT NULL,
    user_id bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: groups_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.groups ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.groups_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: held_content; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.held_content (
    id bigint NOT NULL,
    kind text NOT NULL,
    user_id bigint NOT NULL,
    node_id bigint,
    payload jsonb NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    resolved_by bigint,
    resolved_at timestamp with time zone,
    CONSTRAINT held_content_kind_check CHECK ((kind = ANY (ARRAY['conversation'::text, 'message'::text, 'chat'::text, 'chat_edit'::text, 'status'::text, 'status_reply'::text, 'profile'::text, 'signature'::text, 'title'::text, 'club'::text, 'chat_channel'::text]))),
    CONSTRAINT held_content_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'approved'::text, 'rejected'::text])))
);

--
-- Name: held_content_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.held_content ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.held_content_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: ignored_users; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.ignored_users (
    user_id bigint NOT NULL,
    ignored_user_id bigint NOT NULL
);

--
-- Name: import_map; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.import_map (
    kind text NOT NULL,
    legacy_id bigint NOT NULL,
    new_id bigint NOT NULL
);

--
-- Name: invites; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.invites (
    id bigint NOT NULL,
    code_digest text NOT NULL,
    created_by bigint,
    note text,
    used_by bigint,
    used_at timestamp with time zone,
    expires_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: invites_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.invites ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.invites_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: invoice_numbers; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.invoice_numbers
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: item_timers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.item_timers (
    item_id bigint NOT NULL,
    action text NOT NULL,
    run_at timestamp with time zone NOT NULL,
    created_by bigint,
    CONSTRAINT item_timers_action_check CHECK ((action = ANY (ARRAY['close'::text, 'open'::text, 'bump'::text, 'delete'::text, 'publish'::text])))
);

--
-- Name: items_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.items ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.items_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);


--
-- Name: legal_documents; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.legal_documents (
    id bigint NOT NULL,
    kind text NOT NULL,
    version integer NOT NULL,
    body_md text NOT NULL,
    body_html text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT legal_documents_kind_check CHECK ((kind = ANY (ARRAY['terms'::text, 'privacy'::text, 'guidelines'::text])))
);

--
-- Name: legal_documents_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.legal_documents ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.legal_documents_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: legal_notices; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.legal_notices (
    id bigint NOT NULL,
    url text NOT NULL,
    post_id bigint,
    category text NOT NULL,
    explanation text NOT NULL,
    legal_ground text DEFAULT ''::text NOT NULL,
    notifier_name text DEFAULT ''::text NOT NULL,
    notifier_email text DEFAULT ''::text NOT NULL,
    reporter_id bigint,
    good_faith boolean NOT NULL,
    status text DEFAULT 'open'::text NOT NULL,
    decision text,
    decided_by bigint,
    decided_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT legal_notices_status_check CHECK ((status = ANY (ARRAY['open'::text, 'actioned'::text, 'rejected'::text])))
);

--
-- Name: legal_notices_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.legal_notices ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.legal_notices_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: link_previews; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.link_previews (
    url text NOT NULL,
    ok boolean NOT NULL,
    title text DEFAULT ''::text NOT NULL,
    description text DEFAULT ''::text NOT NULL,
    image text DEFAULT ''::text NOT NULL,
    site text DEFAULT ''::text NOT NULL,
    fetched_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: live_question_votes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.live_question_votes (
    question_id bigint NOT NULL,
    user_id bigint NOT NULL
);

--
-- Name: live_questions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.live_questions (
    id bigint NOT NULL,
    item_id bigint NOT NULL,
    author_id bigint,
    body text NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    votes integer DEFAULT 0 NOT NULL,
    answer_post integer,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT live_questions_body_check CHECK (((length(body) >= 1) AND (length(body) <= 500))),
    CONSTRAINT live_questions_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'approved'::text, 'answered'::text, 'rejected'::text])))
);

--
-- Name: live_questions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.live_questions ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.live_questions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: live_sessions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.live_sessions (
    item_id bigint NOT NULL,
    starts_at timestamp with time zone NOT NULL,
    ends_at timestamp with time zone NOT NULL,
    hosts bigint[] DEFAULT '{}'::bigint[] NOT NULL,
    moderated boolean DEFAULT true NOT NULL,
    created_by bigint,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT live_sessions_check CHECK ((ends_at > starts_at))
);

--
-- Name: mail_outbox; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.mail_outbox (
    id bigint NOT NULL,
    to_addr text NOT NULL,
    subject text NOT NULL,
    body text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    sent_at timestamp with time zone,
    attempts smallint DEFAULT 0 NOT NULL,
    last_error text,
    unsubscribe_url text,
    not_before timestamp with time zone,
    unless_read_user bigint,
    unless_read_url text,
    reply_key text
);

--
-- Name: mail_outbox_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.mail_outbox ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.mail_outbox_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);


--
-- Name: mass_messages; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.mass_messages (
    id bigint NOT NULL,
    sender_id bigint,
    subject text NOT NULL,
    body text NOT NULL,
    audience jsonb NOT NULL,
    notice boolean NOT NULL,
    conversation boolean NOT NULL,
    recipients integer NOT NULL,
    delivered integer DEFAULT 0 NOT NULL,
    status text DEFAULT 'running'::text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    finished_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT mass_messages_status_check CHECK ((status = ANY (ARRAY['running'::text, 'done'::text, 'failed'::text])))
);

--
-- Name: mass_messages_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.mass_messages_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: mass_messages_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.mass_messages_id_seq OWNED BY public.mass_messages.id;

--
-- Name: member_reports; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.member_reports (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    reporter_id bigint,
    reason text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    resolved_at timestamp with time zone,
    resolved_by bigint,
    status_id bigint
);

--
-- Name: member_reports_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.member_reports ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.member_reports_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: member_status_replies; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.member_status_replies (
    id bigint NOT NULL,
    status_id bigint NOT NULL,
    author_id bigint NOT NULL,
    body text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: member_status_replies_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.member_status_replies ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.member_status_replies_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: member_statuses; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.member_statuses (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    body text NOT NULL,
    locked boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: member_statuses_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.member_statuses ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.member_statuses_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: mod_actions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.mod_actions (
    id bigint NOT NULL,
    name text NOT NULL,
    steps jsonb NOT NULL
);

--
-- Name: mod_actions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.mod_actions ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.mod_actions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: moderation_statements; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.moderation_statements (
    id bigint NOT NULL,
    user_id bigint,
    target_type text NOT NULL,
    target_id bigint NOT NULL,
    action text NOT NULL,
    ground text NOT NULL,
    category text NOT NULL,
    facts text NOT NULL,
    automated_detection boolean DEFAULT false NOT NULL,
    automated_decision boolean DEFAULT false NOT NULL,
    notice_id bigint,
    moderator_id bigint,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    appeal_text text,
    appealed_at timestamp with time zone,
    appeal_outcome text,
    appeal_decided_by bigint,
    appeal_decided_at timestamp with time zone,
    CONSTRAINT moderation_statements_action_check CHECK ((action = ANY (ARRAY['hidden'::text, 'deleted'::text, 'suspended'::text, 'silenced'::text, 'removed_from_federation'::text]))),
    CONSTRAINT moderation_statements_appeal_outcome_check CHECK ((appeal_outcome = ANY (ARRAY['upheld'::text, 'reversed'::text]))),
    CONSTRAINT moderation_statements_ground_check CHECK ((ground = ANY (ARRAY['terms'::text, 'illegal'::text]))),
    CONSTRAINT moderation_statements_target_type_check CHECK ((target_type = ANY (ARRAY['post'::text, 'topic'::text, 'account'::text])))
);

--
-- Name: moderation_statements_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.moderation_statements ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.moderation_statements_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: muted_tags; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.muted_tags (
    user_id bigint NOT NULL,
    tag_id bigint NOT NULL
);

--
-- Name: node_moderators; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.node_moderators (
    node_id bigint NOT NULL,
    user_id bigint NOT NULL
);

--
-- Name: node_permissions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.node_permissions (
    node_id bigint NOT NULL,
    group_id bigint NOT NULL,
    perms integer NOT NULL
);

--
-- Name: node_read_markers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.node_read_markers (
    user_id bigint NOT NULL,
    node_id bigint NOT NULL,
    marked_at timestamp with time zone NOT NULL
);

--
-- Name: nodes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.nodes ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.nodes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: notification_prefs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.notification_prefs (
    user_id bigint NOT NULL,
    kind text NOT NULL,
    channels smallint NOT NULL
);

--
-- Name: notifications; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.notifications (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    kind text NOT NULL,
    data jsonb NOT NULL,
    read boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: notifications_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.notifications ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.notifications_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: oauth_clients; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.oauth_clients (
    id bigint NOT NULL,
    client_id text NOT NULL,
    secret_digest text NOT NULL,
    name text NOT NULL,
    redirect_uris text[] NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: oauth_clients_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.oauth_clients ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.oauth_clients_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: oauth_codes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.oauth_codes (
    code_digest text NOT NULL,
    client_id text NOT NULL,
    user_id bigint NOT NULL,
    scope text NOT NULL,
    redirect_uri text NOT NULL,
    challenge text NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

--
-- Name: page_views; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.page_views (
    id bigint NOT NULL,
    at timestamp with time zone DEFAULT now() NOT NULL,
    path text NOT NULL,
    referrer text,
    country text,
    kind text NOT NULL,
    visitor bigint NOT NULL,
    CONSTRAINT page_views_kind_check CHECK ((kind = ANY (ARRAY['member'::text, 'anonymous'::text, 'crawler'::text])))
);

--
-- Name: page_views_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.page_views_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: page_views_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.page_views_id_seq OWNED BY public.page_views.id;

--
-- Name: passkeys; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.passkeys (
    id bytea NOT NULL,
    user_id bigint NOT NULL,
    public_key bytea NOT NULL,
    sign_count bigint DEFAULT 0 NOT NULL,
    name text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used_at timestamp with time zone
);

--
-- Name: pending_logins; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.pending_logins (
    token_digest text NOT NULL,
    user_id bigint NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

--
-- Name: permalinks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.permalinks (
    id bigint NOT NULL,
    url text NOT NULL,
    kind text NOT NULL,
    target_id bigint,
    target text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT permalinks_check CHECK ((((kind = ANY (ARRAY['forum'::text, 'topic'::text, 'post'::text])) AND (target_id IS NOT NULL)) OR ((kind = ANY (ARRAY['tag'::text, 'member'::text, 'url'::text])) AND (target IS NOT NULL)))),
    CONSTRAINT permalinks_kind_check CHECK ((kind = ANY (ARRAY['forum'::text, 'topic'::text, 'post'::text, 'tag'::text, 'member'::text, 'url'::text]))),
    CONSTRAINT permalinks_url_check CHECK (((length(url) >= 1) AND (length(url) <= 500)))
);

--
-- Name: permalinks_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.permalinks ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.permalinks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: plugin_api_scopes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plugin_api_scopes (
    plugin text NOT NULL,
    key text NOT NULL,
    label text NOT NULL
);

--
-- Name: plugin_cook_state; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plugin_cook_state (
    id integer DEFAULT 1 NOT NULL,
    digest text DEFAULT ''::text NOT NULL,
    CONSTRAINT plugin_cook_state_id_check CHECK ((id = 1))
);

--
-- Name: plugin_files; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plugin_files (
    plugin text NOT NULL,
    path text NOT NULL,
    content_type text NOT NULL,
    data bytea NOT NULL,
    size integer NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: plugin_jobs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plugin_jobs (
    id bigint NOT NULL,
    plugin text NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    run_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: plugin_jobs_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.plugin_jobs_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: plugin_jobs_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.plugin_jobs_id_seq OWNED BY public.plugin_jobs.id;

--
-- Name: plugin_kv; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plugin_kv (
    plugin text NOT NULL,
    key text NOT NULL,
    value text NOT NULL
);

--
-- Name: plugin_meta; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plugin_meta (
    plugin text NOT NULL,
    key text NOT NULL,
    target text NOT NULL,
    target_id bigint NOT NULL,
    value jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT plugin_meta_target_check CHECK ((target = ANY (ARRAY['topic'::text, 'post'::text, 'user'::text])))
);

--
-- Name: plugin_permission_grants; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plugin_permission_grants (
    plugin text NOT NULL,
    perm text NOT NULL,
    group_id bigint NOT NULL
);

--
-- Name: plugin_permissions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plugin_permissions (
    plugin text NOT NULL,
    key text NOT NULL,
    label text NOT NULL
);

--
-- Name: plugin_review_items; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plugin_review_items (
    id bigint NOT NULL,
    plugin text NOT NULL,
    type text NOT NULL,
    ref text NOT NULL,
    title text NOT NULL,
    excerpt text DEFAULT ''::text NOT NULL,
    url text NOT NULL,
    submitted_by bigint,
    status text DEFAULT 'pending'::text NOT NULL,
    decided_by bigint,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    decided_at timestamp with time zone,
    CONSTRAINT plugin_review_items_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'approved'::text, 'rejected'::text])))
);

--
-- Name: plugin_review_items_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.plugin_review_items_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: plugin_review_items_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.plugin_review_items_id_seq OWNED BY public.plugin_review_items.id;

--
-- Name: plugin_schedules; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plugin_schedules (
    plugin text NOT NULL,
    name text NOT NULL,
    every_minutes integer NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    next_run timestamp with time zone NOT NULL,
    CONSTRAINT plugin_schedules_every_minutes_check CHECK (((every_minutes >= 5) AND (every_minutes <= 43200)))
);

--
-- Name: plugin_search_docs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plugin_search_docs (
    plugin text NOT NULL,
    type text NOT NULL,
    ref text NOT NULL,
    title text NOT NULL,
    body text NOT NULL,
    url text NOT NULL,
    author text,
    perm text,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    search_vector tsvector GENERATED ALWAYS AS ((setweight(to_tsvector('simple'::regconfig, title), 'A'::"char") || to_tsvector('simple'::regconfig, body))) STORED
);

--
-- Name: plugins; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.plugins (
    id text NOT NULL,
    name text NOT NULL,
    version text NOT NULL,
    capabilities text[] DEFAULT '{}'::text[] NOT NULL,
    approved boolean DEFAULT false NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    hooks text[] DEFAULT '{}'::text[] NOT NULL,
    module bytea,
    slots jsonb DEFAULT '{}'::jsonb NOT NULL,
    client_js text,
    schema jsonb,
    settings jsonb DEFAULT '{}'::jsonb NOT NULL,
    checksum text NOT NULL,
    failures integer DEFAULT 0 NOT NULL,
    installed_at timestamp with time zone DEFAULT now() NOT NULL,
    bot_user_id bigint,
    last_failure_at timestamp with time zone,
    approved_capabilities text[] DEFAULT '{}'::text[] NOT NULL,
    package bytea,
    prev_version text,
    prev_package bytea
);

--
-- Name: poll_groups; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.poll_groups (
    poll_id bigint NOT NULL,
    group_id bigint NOT NULL
);

--
-- Name: poll_options; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.poll_options (
    id bigint NOT NULL,
    item_id bigint NOT NULL,
    "position" integer NOT NULL,
    label text NOT NULL,
    poll_id bigint NOT NULL
);

--
-- Name: poll_options_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.poll_options ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.poll_options_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: polls_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.polls ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.polls_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: post_links; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.post_links (
    post_id bigint NOT NULL,
    url text NOT NULL,
    clicks integer DEFAULT 0 NOT NULL
);

--
-- Name: post_notes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.post_notes (
    id bigint NOT NULL,
    post_id bigint NOT NULL,
    author_id bigint,
    body text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT post_notes_body_check CHECK (((length(body) >= 1) AND (length(body) <= 2000)))
);

--
-- Name: post_notes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.post_notes ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.post_notes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: post_revisions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.post_revisions (
    post_id bigint NOT NULL,
    n integer NOT NULL,
    raw text NOT NULL,
    title text,
    editor_id bigint,
    reason text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    hidden boolean DEFAULT false NOT NULL
);

--
-- Name: post_votes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.post_votes (
    post_id bigint NOT NULL,
    user_id bigint NOT NULL,
    value smallint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT post_votes_value_check CHECK ((value = ANY (ARRAY['-1'::integer, 1])))
);

--
-- Name: posts_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.posts ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.posts_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: profile_fields; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.profile_fields (
    id bigint NOT NULL,
    label text NOT NULL,
    kind text NOT NULL,
    options text[] DEFAULT '{}'::text[] NOT NULL,
    required boolean DEFAULT false NOT NULL,
    show_on_register boolean DEFAULT false NOT NULL,
    public boolean DEFAULT true NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    icon text,
    url_prefixes text[] DEFAULT '{}'::text[] NOT NULL,
    url_shorthand boolean DEFAULT false NOT NULL,
    https_only boolean DEFAULT false NOT NULL,
    plugin text,
    plugin_key text,
    editable boolean DEFAULT true NOT NULL,
    active boolean DEFAULT true NOT NULL,
    CONSTRAINT profile_fields_icon_check CHECK ((icon ~ '^[a-z0-9-]{1,40}$'::text)),
    CONSTRAINT profile_fields_kind_check CHECK ((kind = ANY (ARRAY['text'::text, 'textarea'::text, 'url'::text, 'select'::text])))
);

--
-- Name: profile_fields_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.profile_fields ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.profile_fields_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: profile_visits; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.profile_visits (
    profile_id bigint NOT NULL,
    visitor_id bigint NOT NULL,
    visited_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: promotion_rules; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.promotion_rules (
    id bigint NOT NULL,
    name text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    min_posts integer DEFAULT 0 NOT NULL,
    min_days integer DEFAULT 0 NOT NULL,
    min_reputation bigint DEFAULT 0 NOT NULL,
    min_topics_read integer DEFAULT 0 NOT NULL,
    set_trust_level smallint,
    add_group bigint
);

--
-- Name: promotion_rules_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.promotion_rules ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.promotion_rules_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: push_cursor; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.push_cursor (
    id smallint NOT NULL,
    last_id bigint NOT NULL,
    CONSTRAINT push_cursor_id_check CHECK ((id = 1))
);

--
-- Name: push_queue; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.push_queue (
    id bigint NOT NULL,
    notification_id bigint NOT NULL,
    endpoint text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    next_at timestamp with time zone DEFAULT now() NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: push_queue_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.push_queue ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.push_queue_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: push_subscriptions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.push_subscriptions (
    endpoint text NOT NULL,
    user_id bigint NOT NULL,
    p256dh text NOT NULL,
    auth text NOT NULL
);

--
-- Name: reactions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.reactions (
    id bigint NOT NULL,
    name text NOT NULL,
    icon text NOT NULL,
    score smallint DEFAULT 1 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    is_like boolean DEFAULT false NOT NULL,
    plugin text,
    plugin_key text,
    CONSTRAINT reactions_score_check CHECK (((score >= '-1'::integer) AND (score <= 1)))
);

--
-- Name: reactions_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.reactions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: reactions_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.reactions_id_seq OWNED BY public.reactions.id;

--
-- Name: read_markers; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.read_markers (
    user_id bigint NOT NULL,
    item_id bigint NOT NULL,
    last_read_post_number integer NOT NULL
);

--
-- Name: referral_rewards; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.referral_rewards (
    user_id bigint NOT NULL,
    group_id bigint NOT NULL,
    granted_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: report_reasons; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.report_reasons (
    key text NOT NULL,
    label text NOT NULL,
    hint text DEFAULT ''::text NOT NULL,
    weight real DEFAULT 1 NOT NULL,
    needs_note boolean DEFAULT false NOT NULL,
    illegal boolean DEFAULT false NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    plugin text,
    plugin_key text,
    message_user boolean DEFAULT false NOT NULL,
    CONSTRAINT report_reasons_key_check CHECK ((key ~ '^[a-z0-9_]{1,40}$'::text)),
    CONSTRAINT report_reasons_weight_check CHECK (((weight >= (0)::double precision) AND (weight <= (10)::double precision)))
);

--
-- Name: reports_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.reports ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.reports_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: reviewables; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.reviewables (
    id bigint NOT NULL,
    kind text NOT NULL,
    target_type text NOT NULL,
    target_id bigint NOT NULL,
    node_id bigint,
    score real DEFAULT 0 NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    resolved_by bigint,
    resolved_at timestamp with time zone,
    claimed_by bigint,
    CONSTRAINT reviewables_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'approved'::text, 'rejected'::text, 'ignored'::text])))
);

--
-- Name: reviewables_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.reviewables ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.reviewables_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: saved_replies; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.saved_replies (
    id bigint NOT NULL,
    title text NOT NULL,
    body text NOT NULL
);

--
-- Name: saved_replies_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.saved_replies ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.saved_replies_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: search_best_bets; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.search_best_bets (
    id bigint NOT NULL,
    query text NOT NULL,
    item_id bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: search_best_bets_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.search_best_bets_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: search_best_bets_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.search_best_bets_id_seq OWNED BY public.search_best_bets.id;

--
-- Name: search_logs; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.search_logs (
    id bigint NOT NULL,
    term text NOT NULL,
    results integer NOT NULL,
    user_id bigint,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: search_logs_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.search_logs ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.search_logs_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: search_synonyms; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.search_synonyms (
    id bigint NOT NULL,
    term text NOT NULL,
    synonyms text[] NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: search_synonyms_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

CREATE SEQUENCE public.search_synonyms_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

--
-- Name: search_synonyms_id_seq; Type: SEQUENCE OWNED BY; Schema: public; Owner: -
--

ALTER SEQUENCE public.search_synonyms_id_seq OWNED BY public.search_synonyms.id;

--
-- Name: sessions; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sessions (
    token_digest text NOT NULL,
    user_id bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    ip inet,
    user_agent text,
    country text,
    sudo_until timestamp with time zone,
    impersonated boolean DEFAULT false NOT NULL
);

--
-- Name: site_settings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.site_settings (
    key text NOT NULL,
    value jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: sso_nonces; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.sso_nonces (
    digest text NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

--
-- Name: tag_group_members; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tag_group_members (
    group_id bigint NOT NULL,
    tag_id bigint NOT NULL
);

--
-- Name: tag_group_nodes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tag_group_nodes (
    group_id bigint NOT NULL,
    node_id bigint NOT NULL
);

--
-- Name: tag_groups; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.tag_groups (
    id bigint NOT NULL,
    name text NOT NULL,
    one_per_topic boolean DEFAULT false NOT NULL
);

--
-- Name: tag_groups_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.tag_groups ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.tag_groups_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: tags_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.tags ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.tags_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: task_log; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.task_log (
    id bigint NOT NULL,
    task text NOT NULL,
    detail jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: task_log_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.task_log ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.task_log_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: themes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.themes (
    id bigint NOT NULL,
    slug text NOT NULL,
    name text NOT NULL,
    version text DEFAULT '1.0.0'::text NOT NULL,
    kind text DEFAULT 'theme'::text NOT NULL,
    tokens jsonb DEFAULT '{}'::jsonb NOT NULL,
    css text DEFAULT ''::text NOT NULL,
    slots jsonb DEFAULT '{}'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    selectable boolean DEFAULT true NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    light_scheme text,
    dark_scheme text,
    group_ids bigint[] DEFAULT '{}'::bigint[] NOT NULL,
    CONSTRAINT themes_kind_check CHECK ((kind = ANY (ARRAY['theme'::text, 'component'::text])))
);

--
-- Name: themes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.themes ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.themes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: topic_redirects; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.topic_redirects (
    id bigint NOT NULL,
    node_id bigint NOT NULL,
    item_id bigint NOT NULL,
    expires_at timestamp with time zone
);

--
-- Name: topic_redirects_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.topic_redirects ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.topic_redirects_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: topic_votes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.topic_votes (
    item_id bigint NOT NULL,
    user_id bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: translation_overrides; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.translation_overrides (
    locale text NOT NULL,
    key text NOT NULL,
    value text NOT NULL
);

--
-- Name: trusted_devices; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.trusted_devices (
    token_digest text NOT NULL,
    user_id bigint NOT NULL,
    user_agent text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

--
-- Name: unsubscribe_tokens; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.unsubscribe_tokens (
    token text NOT NULL,
    user_id bigint NOT NULL,
    kind text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    item_id bigint,
    reply_key text DEFAULT (gen_random_uuid())::text,
    reply_key_expires timestamp with time zone DEFAULT (now() + '90 days'::interval),
    conversation_id bigint
);

--
-- Name: upload_quota; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.upload_quota (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    size bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT upload_quota_size_check CHECK ((size >= 0))
);

--
-- Name: upload_quota_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.upload_quota ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.upload_quota_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: upload_uses; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.upload_uses (
    upload_id bigint NOT NULL,
    kind text NOT NULL,
    ref_id bigint NOT NULL,
    CONSTRAINT upload_uses_kind_check CHECK ((kind = ANY (ARRAY['post'::text, 'message'::text, 'chat'::text])))
);

--
-- Name: uploads; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.uploads (
    id bigint NOT NULL,
    sha256 text NOT NULL,
    user_id bigint,
    storage_key text NOT NULL,
    filename text NOT NULL,
    mime text NOT NULL,
    size bigint NOT NULL,
    width integer,
    height integer,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    download_count integer DEFAULT 0 NOT NULL
);

--
-- Name: uploads_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.uploads ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.uploads_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: user_field_values; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_field_values (
    user_id bigint NOT NULL,
    field_id bigint NOT NULL,
    value text NOT NULL
);

--
-- Name: user_follows; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_follows (
    user_id bigint NOT NULL,
    target_id bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: user_identities; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_identities (
    provider text NOT NULL,
    subject text NOT NULL,
    user_id bigint NOT NULL,
    email text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: user_legal_acceptances; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_legal_acceptances (
    user_id bigint NOT NULL,
    document_id bigint NOT NULL,
    accepted_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: user_notes; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_notes (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    author_id bigint,
    body text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: user_notes_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.user_notes ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.user_notes_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: user_visits; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.user_visits (
    user_id bigint NOT NULL,
    day date NOT NULL
);

--
-- Name: username_history; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.username_history (
    user_id bigint NOT NULL,
    old_username text NOT NULL,
    old_lower text NOT NULL,
    changed_at timestamp with time zone DEFAULT now() NOT NULL
);

--
-- Name: users_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.users ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.users_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: warnings; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.warnings (
    id bigint NOT NULL,
    user_id bigint NOT NULL,
    moderator_id bigint,
    points integer NOT NULL,
    reason text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

--
-- Name: warnings_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.warnings ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.warnings_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: watched_words; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.watched_words (
    id bigint NOT NULL,
    action text NOT NULL,
    pattern text NOT NULL,
    replacement text,
    CONSTRAINT watched_words_action_check CHECK ((action = ANY (ARRAY['block'::text, 'censor'::text, 'require_approval'::text, 'replace'::text, 'link'::text, 'tag'::text, 'flag'::text, 'silence'::text])))
);

--
-- Name: watched_words_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.watched_words ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.watched_words_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: webauthn_challenges; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.webauthn_challenges (
    token_digest text NOT NULL,
    challenge bytea NOT NULL,
    user_id bigint,
    expires_at timestamp with time zone NOT NULL
);

--
-- Name: webhook_deliveries; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.webhook_deliveries (
    id bigint NOT NULL,
    webhook_id bigint NOT NULL,
    event text NOT NULL,
    payload jsonb NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    next_at timestamp with time zone DEFAULT now() NOT NULL,
    last_status integer,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT webhook_deliveries_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'delivered'::text, 'failed'::text])))
);

--
-- Name: webhook_deliveries_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.webhook_deliveries ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.webhook_deliveries_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: webhooks; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.webhooks (
    id bigint NOT NULL,
    url text NOT NULL,
    secret text NOT NULL,
    events text[] NOT NULL,
    active boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    node_ids bigint[] DEFAULT '{}'::bigint[] NOT NULL
);

--
-- Name: webhooks_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.webhooks ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.webhooks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: widgets; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE public.widgets (
    id bigint NOT NULL,
    area text NOT NULL,
    kind text NOT NULL,
    title text DEFAULT ''::text NOT NULL,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    "position" integer DEFAULT 0 NOT NULL,
    cache_seconds integer DEFAULT 60 NOT NULL,
    CONSTRAINT widgets_area_check CHECK (((area = ANY (ARRAY['home_top'::text, 'home_main'::text, 'home_bottom'::text])) OR (area ~ '^(forum|topic|latest|top|search|tag|members|groups|badges|leaderboard|about|clubs|live)_(top|side|bottom)$'::text) OR (area ~ '^page[0-9]{1,18}_(top|main|side|bottom|col1|col2|col3)$'::text)))
);

--
-- Name: widgets_id_seq; Type: SEQUENCE; Schema: public; Owner: -
--

ALTER TABLE public.widgets ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME public.widgets_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);

--
-- Name: chat_channels id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_channels ALTER COLUMN id SET DEFAULT nextval('public.chat_channels_id_seq'::regclass);

--
-- Name: chat_integration_rules id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_integration_rules ALTER COLUMN id SET DEFAULT nextval('public.chat_integration_rules_id_seq'::regclass);

--
-- Name: chat_messages id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_messages ALTER COLUMN id SET DEFAULT nextval('public.chat_messages_id_seq'::regclass);

--
-- Name: conversation_messages id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_messages ALTER COLUMN id SET DEFAULT nextval('public.conversation_messages_id_seq'::regclass);

--
-- Name: conversations id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversations ALTER COLUMN id SET DEFAULT nextval('public.conversations_id_seq'::regclass);

--
-- Name: mass_messages id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mass_messages ALTER COLUMN id SET DEFAULT nextval('public.mass_messages_id_seq'::regclass);

--
-- Name: page_views id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.page_views ALTER COLUMN id SET DEFAULT nextval('public.page_views_id_seq'::regclass);

--
-- Name: plugin_jobs id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_jobs ALTER COLUMN id SET DEFAULT nextval('public.plugin_jobs_id_seq'::regclass);

--
-- Name: plugin_review_items id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_review_items ALTER COLUMN id SET DEFAULT nextval('public.plugin_review_items_id_seq'::regclass);

--
-- Name: reactions id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reactions ALTER COLUMN id SET DEFAULT nextval('public.reactions_id_seq'::regclass);

--
-- Name: search_best_bets id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.search_best_bets ALTER COLUMN id SET DEFAULT nextval('public.search_best_bets_id_seq'::regclass);

--
-- Name: search_synonyms id; Type: DEFAULT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.search_synonyms ALTER COLUMN id SET DEFAULT nextval('public.search_synonyms_id_seq'::regclass);

--
-- Data for Name: admin_notices; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ads; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ai_embeddings; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ai_personas; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ai_post_labels; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ai_summaries; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ai_usage; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: announcements; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ap_actors; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ap_deliveries; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ap_dm_objects; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ap_followers; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ap_keys; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ap_mirrors; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ap_objects; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ap_seen; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: api_idempotency; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: api_keys; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: audit_forward_state; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.audit_forward_state VALUES
	(1, 0, '2026-11-16 16:28:13.200382+00');

--
-- Data for Name: audit_log; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: automations; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: backup_codes; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: badges; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.badges OVERRIDING SYSTEM VALUE VALUES
	(1, 'Basic User', 'Reached trust level 1', 0, 0, 0, 1, 'basic', true, 'bronze', 'shield', 'Trust', false, 9, 0),
	(2, 'Member', 'Reached trust level 2', 0, 0, 0, 5, 'member', true, 'bronze', 'shield', 'Trust', false, 3, 0),
	(3, 'Regular', 'Reached trust level 3', 0, 0, 0, 10, 'regular', true, 'silver', 'shield', 'Trust', false, 6, 0),
	(4, 'Leader', 'Reached trust level 4', 0, 0, 0, 25, 'leader', true, 'gold', 'shield', 'Trust', false, 8, 0),
	(5, 'Autobiographer', 'Filled in a profile with a picture and an about section', 0, 0, 0, 1, 'autobiographer', true, 'bronze', 'user', 'Getting started', false, 1, 0),
	(6, 'First Like', 'Liked a post', 0, 0, 0, 1, 'first_like', true, 'bronze', 'heart', 'Getting started', false, 7, 0),
	(7, 'First Link', 'Linked to something in a post', 0, 0, 0, 1, 'first_link', true, 'bronze', 'link', 'Getting started', false, 5, 0),
	(8, 'First Quote', 'Quoted another post', 0, 0, 0, 1, 'first_quote', true, 'bronze', 'quote', 'Getting started', false, 2, 0),
	(9, 'First Mention', 'Mentioned someone in a post', 0, 0, 0, 1, 'first_mention', true, 'bronze', 'user', 'Getting started', false, 9, 0),
	(10, 'First Flag', 'Reported a post that staff acted on', 0, 0, 0, 1, 'first_flag', true, 'bronze', 'flag', 'Getting started', false, 6, 0),
	(11, 'Editor', 'Edited their own post', 0, 0, 0, 1, 'editor', true, 'bronze', 'pencil', 'Getting started', false, 4, 0),
	(12, 'Wiki Editor', 'Edited someone else''s wiki post', 0, 0, 0, 2, 'wiki_editor', true, 'bronze', 'file-text', 'Getting started', false, 3, 0),
	(13, 'Welcome', 'Received a first like', 0, 0, 0, 1, 'welcome', true, 'bronze', 'heart', 'Community', false, 8, 0),
	(14, 'Nice Reply', 'Received 10 likes on a reply', 0, 0, 0, 2, 'nice_post', true, 'bronze', 'heart', 'Community', false, 1, 10),
	(15, 'Good Reply', 'Received 25 likes on a reply', 0, 0, 0, 5, 'good_post', true, 'silver', 'heart', 'Community', false, 1, 25),
	(16, 'Great Reply', 'Received 50 likes on a reply', 0, 0, 0, 15, 'great_post', true, 'gold', 'heart', 'Community', false, 1, 50),
	(17, 'Nice Topic', 'Received 10 likes on a topic', 0, 0, 0, 2, 'nice_topic', true, 'bronze', 'star', 'Community', false, 2, 10),
	(18, 'Good Topic', 'Received 25 likes on a topic', 0, 0, 0, 5, 'good_topic', true, 'silver', 'star', 'Community', false, 2, 25),
	(19, 'Great Topic', 'Received 50 likes on a topic', 0, 0, 0, 15, 'great_topic', true, 'gold', 'star', 'Community', false, 2, 50),
	(20, 'Appreciated', 'Received a like on 20 posts', 0, 0, 0, 5, 'appreciated', true, 'bronze', 'circle-check', 'Community', false, 3, 20),
	(21, 'Respected', 'Received 2 likes on 100 posts', 0, 0, 0, 10, 'respected', true, 'silver', 'circle-check', 'Community', false, 3, 100),
	(22, 'Admired', 'Received 5 likes on 300 posts', 0, 0, 0, 25, 'admired', true, 'gold', 'circle-check', 'Community', false, 3, 300),
	(23, 'Thank You', 'Gave 20 likes', 0, 0, 0, 2, 'thank_you', true, 'bronze', 'heart', 'Community', false, 7, 20),
	(24, 'Gives Back', 'Gave 100 likes', 0, 0, 0, 5, 'gives_back', true, 'silver', 'heart', 'Community', false, 7, 100),
	(25, 'Empathetic', 'Gave 500 likes', 0, 0, 0, 15, 'empathetic', true, 'gold', 'heart', 'Community', false, 7, 500),
	(26, 'Anniversary', 'A member for a year, and posted in that time', 0, 0, 0, 5, 'anniversary', true, 'silver', 'calendar', 'Community', false, 8, 0),
	(27, 'Solved', 'Gave an answer marked as the solution', 0, 0, 0, 2, 'solved', true, 'bronze', 'circle-check', 'Help', false, 9, 1),
	(28, 'Tech Support', 'Gave 10 accepted answers', 0, 0, 0, 10, 'tech_support', true, 'silver', 'circle-check', 'Help', false, 9, 10),
	(29, 'Tech Guru', 'Gave 50 accepted answers', 0, 0, 0, 25, 'tech_guru', true, 'gold', 'circle-check', 'Help', false, 9, 50),
	(30, 'Promoter', 'Invited someone who joined', 0, 0, 0, 2, 'promoter', true, 'bronze', 'mail', 'Invitations', false, 0, 1),
	(31, 'Campaigner', 'Invited 3 people who reached trust level 1', 0, 0, 0, 5, 'campaigner', true, 'silver', 'mail', 'Invitations', false, 0, 3),
	(32, 'Champion', 'Invited 5 people who reached trust level 2', 0, 0, 0, 15, 'champion', true, 'gold', 'mail', 'Invitations', false, 0, 5),
	(33, 'Chatty', 'Wrote 10 posts', 0, 0, 0, 2, 'posts_10', true, 'bronze', 'message-square', 'Posting', false, 1, 10),
	(34, 'Talkative', 'Wrote 50 posts', 0, 0, 0, 3, 'posts_50', true, 'bronze', 'message-square', 'Posting', false, 1, 50),
	(35, 'Conversationalist', 'Wrote 100 posts', 0, 0, 0, 5, 'posts_100', true, 'silver', 'message-square', 'Posting', false, 1, 100),
	(36, 'Storyteller', 'Wrote 500 posts', 0, 0, 0, 10, 'posts_500', true, 'silver', 'message-square', 'Posting', false, 1, 500),
	(37, 'Veteran Poster', 'Wrote 1000 posts', 0, 0, 0, 20, 'posts_1000', true, 'gold', 'message-square', 'Posting', false, 1, 1000),
	(38, 'Living Legend', 'Wrote 5000 posts', 0, 0, 0, 50, 'posts_5000', true, 'gold', 'message-square', 'Posting', false, 1, 5000),
	(39, 'Conversation Starter', 'Started a topic', 0, 0, 0, 1, 'topics_1', true, 'bronze', 'file-text', 'Posting', false, 2, 1),
	(40, 'Topic Maker', 'Started 10 topics', 0, 0, 0, 3, 'topics_10', true, 'bronze', 'file-text', 'Posting', false, 2, 10),
	(41, 'Discussion Leader', 'Started 50 topics', 0, 0, 0, 8, 'topics_50', true, 'silver', 'file-text', 'Posting', false, 2, 50),
	(42, 'Forum Pillar', 'Started 100 topics', 0, 0, 0, 15, 'topics_100', true, 'silver', 'file-text', 'Posting', false, 2, 100),
	(43, 'Topic Titan', 'Started 500 topics', 0, 0, 0, 40, 'topics_500', true, 'gold', 'file-text', 'Posting', false, 2, 500),
	(44, 'Lively Topic', 'Started a topic with 25 replies', 0, 0, 0, 3, 'hot_25', true, 'bronze', 'rocket', 'Posting', false, 3, 25),
	(45, 'Hot Topic', 'Started a topic with 50 replies', 0, 0, 0, 8, 'hot_50', true, 'silver', 'rocket', 'Posting', false, 3, 50),
	(46, 'Blazing Topic', 'Started a topic with 100 replies', 0, 0, 0, 20, 'hot_100', true, 'gold', 'rocket', 'Posting', false, 3, 100),
	(47, 'Popular Topic', 'Started a topic with 1000 views', 0, 0, 0, 3, 'viewed_1000', true, 'bronze', 'eye', 'Posting', false, 4, 1000),
	(48, 'Notable Topic', 'Started a topic with 5000 views', 0, 0, 0, 8, 'viewed_5000', true, 'silver', 'eye', 'Posting', false, 4, 5000),
	(49, 'Famous Topic', 'Started a topic with 25000 views', 0, 0, 0, 20, 'viewed_25000', true, 'gold', 'eye', 'Posting', false, 4, 25000),
	(50, 'Link Sharer', 'Shared 25 links in posts', 0, 0, 0, 2, 'links_25', true, 'bronze', 'link', 'Posting', false, 5, 25);
INSERT INTO public.badges OVERRIDING SYSTEM VALUE VALUES
	(51, 'Curator', 'Shared 100 links in posts', 0, 0, 0, 6, 'links_100', true, 'silver', 'link', 'Posting', false, 5, 100),
	(52, 'Wiki Starter', 'Started a wiki post', 0, 0, 0, 2, 'wiki_1', true, 'bronze', 'book-open', 'Posting', false, 6, 1),
	(53, 'Librarian', 'Started 10 wiki posts', 0, 0, 0, 8, 'wiki_10', true, 'silver', 'book-open', 'Posting', false, 6, 10),
	(54, 'Liked', 'Received 10 likes in total', 0, 0, 0, 2, 'received_10', true, 'bronze', 'heart', 'Reactions', false, 7, 10),
	(55, 'Well Liked', 'Received 50 likes in total', 0, 0, 0, 4, 'received_50', true, 'bronze', 'heart', 'Reactions', false, 7, 50),
	(56, 'Popular', 'Received 100 likes in total', 0, 0, 0, 8, 'received_100', true, 'silver', 'heart', 'Reactions', false, 7, 100),
	(57, 'Beloved', 'Received 500 likes in total', 0, 0, 0, 15, 'received_500', true, 'silver', 'heart', 'Reactions', false, 7, 500),
	(58, 'Adored', 'Received 1000 likes in total', 0, 0, 0, 30, 'received_1000', true, 'gold', 'heart', 'Reactions', false, 7, 1000),
	(59, 'Superstar', 'Received 5000 likes in total', 0, 0, 0, 75, 'received_5000', true, 'gold', 'heart', 'Reactions', false, 7, 5000),
	(60, 'Generous', 'Gave 1000 likes', 0, 0, 0, 25, 'gave_1000', true, 'gold', 'smile', 'Reactions', false, 8, 1000),
	(61, 'Big Hearted', 'Gave 2500 likes', 0, 0, 0, 40, 'gave_2500', true, 'gold', 'smile', 'Reactions', false, 8, 2500),
	(62, 'Noticed', 'Gained a follower', 0, 0, 0, 1, 'followers_1', true, 'bronze', 'users', 'Community', false, 9, 1),
	(63, 'Followed', 'Gained 10 followers', 0, 0, 0, 4, 'followers_10', true, 'bronze', 'users', 'Community', false, 9, 10),
	(64, 'Influencer', 'Gained 50 followers', 0, 0, 0, 12, 'followers_50', true, 'silver', 'users', 'Community', false, 9, 50),
	(65, 'Trendsetter', 'Gained 100 followers', 0, 0, 0, 25, 'followers_100', true, 'gold', 'users', 'Community', false, 9, 100),
	(66, 'Icon', 'Gained 500 followers', 0, 0, 0, 60, 'followers_500', true, 'gold', 'users', 'Community', false, 9, 500),
	(67, 'Networker', 'Follows 10 members', 0, 0, 0, 2, 'following_10', true, 'bronze', 'user', 'Community', false, 0, 10),
	(68, 'Social Butterfly', 'Follows 50 members', 0, 0, 0, 5, 'following_50', true, 'silver', 'user', 'Community', false, 0, 50),
	(69, 'First Status', 'Shared a status update', 0, 0, 0, 1, 'statuses_1', true, 'bronze', 'megaphone', 'Community', false, 1, 1),
	(70, 'Status Updater', 'Shared 50 status updates', 0, 0, 0, 5, 'statuses_50', true, 'silver', 'megaphone', 'Community', false, 1, 50),
	(71, 'Broadcaster', 'Shared 500 status updates', 0, 0, 0, 20, 'statuses_500', true, 'gold', 'megaphone', 'Community', false, 1, 500),
	(72, 'First Vote', 'Voted in a poll', 0, 0, 0, 1, 'votes_1', true, 'bronze', 'list', 'Community', false, 2, 1),
	(73, 'Voter', 'Voted in 10 polls', 0, 0, 0, 3, 'votes_10', true, 'bronze', 'list', 'Community', false, 2, 10),
	(74, 'Pollster', 'Voted in 100 polls', 0, 0, 0, 10, 'votes_100', true, 'silver', 'list', 'Community', false, 2, 100),
	(75, 'Poll Starter', 'Started a poll', 0, 0, 0, 1, 'polls_1', true, 'bronze', 'table', 'Community', false, 3, 1),
	(76, 'Opinion Seeker', 'Started 10 polls', 0, 0, 0, 6, 'polls_10', true, 'silver', 'table', 'Community', false, 3, 10),
	(77, 'Event Organizer', 'Put an event on the calendar', 0, 0, 0, 2, 'events_1', true, 'bronze', 'calendar', 'Community', false, 4, 1),
	(78, 'Host', 'Put 10 events on the calendar', 0, 0, 0, 8, 'events_10', true, 'silver', 'calendar', 'Community', false, 4, 10),
	(79, 'Bookmarker', 'Bookmarked a post', 0, 0, 0, 1, 'bookmarks_1', true, 'bronze', 'bookmark', 'Community', false, 5, 1),
	(80, 'Collector', 'Bookmarked 50 posts', 0, 0, 0, 5, 'bookmarks_50', true, 'silver', 'bookmark', 'Community', false, 5, 50),
	(81, 'Watchful', 'Reported 5 posts that staff acted on', 0, 0, 0, 3, 'guardian_5', true, 'bronze', 'flag', 'Community', false, 6, 5),
	(82, 'Guardian', 'Reported 25 posts that staff acted on', 0, 0, 0, 10, 'guardian_25', true, 'silver', 'flag', 'Community', false, 6, 25),
	(83, 'Sentinel', 'Reported 100 posts that staff acted on', 0, 0, 0, 30, 'guardian_100', true, 'gold', 'flag', 'Community', false, 6, 100),
	(84, 'First Chat', 'Sent a chat message', 0, 0, 0, 1, 'chat_1', true, 'bronze', 'message-square', 'Chat', false, 7, 1),
	(85, 'Chatterbox', 'Sent 100 chat messages', 0, 0, 0, 3, 'chat_100', true, 'bronze', 'message-square', 'Chat', false, 7, 100),
	(86, 'Regular Chatter', 'Sent 1000 chat messages', 0, 0, 0, 8, 'chat_1000', true, 'silver', 'message-square', 'Chat', false, 7, 1000),
	(87, 'Chat Legend', 'Sent 10000 chat messages', 0, 0, 0, 30, 'chat_10000', true, 'gold', 'message-square', 'Chat', false, 7, 10000),
	(88, 'Settling In', 'A member for 7 days', 0, 0, 0, 1, 'tenure_7', true, 'bronze', 'clock', 'Time', false, 8, 7),
	(89, 'One Month In', 'A member for 30 days', 0, 0, 0, 2, 'tenure_30', true, 'bronze', 'clock', 'Time', false, 8, 30),
	(90, 'Three Months In', 'A member for 90 days', 0, 0, 0, 3, 'tenure_90', true, 'bronze', 'clock', 'Time', false, 8, 90),
	(91, 'Half a Year', 'A member for 180 days', 0, 0, 0, 5, 'tenure_180', true, 'silver', 'clock', 'Time', false, 8, 180),
	(92, 'Two Years', 'A member for 2 years', 0, 0, 0, 10, 'tenure_730', true, 'silver', 'clock', 'Time', false, 8, 730),
	(93, 'Three Years', 'A member for 3 years', 0, 0, 0, 15, 'tenure_1095', true, 'silver', 'clock', 'Time', false, 8, 1095),
	(94, 'Five Years', 'A member for 5 years', 0, 0, 0, 30, 'tenure_1825', true, 'gold', 'clock', 'Time', false, 8, 1825),
	(95, 'Ten Years', 'A member for 10 years', 0, 0, 0, 60, 'tenure_3650', true, 'gold', 'clock', 'Time', false, 8, 3650),
	(96, 'Problem Solver', 'Gave 100 accepted answers', 0, 0, 0, 40, 'answers_100', true, 'gold', 'lightbulb', 'Help', false, 9, 100),
	(97, 'Oracle', 'Gave 250 accepted answers', 0, 0, 0, 80, 'answers_250', true, 'gold', 'lightbulb', 'Help', false, 9, 250),
	(98, 'Ambassador', 'Invited 10 people who joined', 0, 0, 0, 10, 'ambassador_10', true, 'silver', 'globe', 'Invitations', false, 0, 10),
	(99, 'Recruiter', 'Invited 25 people who joined', 0, 0, 0, 25, 'ambassador_25', true, 'gold', 'globe', 'Invitations', false, 0, 25),
	(100, 'Decorator', 'Added a cover picture to their profile', 0, 0, 0, 1, 'decorator', true, 'bronze', 'image', 'Getting started', false, 1, 0);
INSERT INTO public.badges OVERRIDING SYSTEM VALUE VALUES
	(101, 'Signed', 'Added a signature', 0, 0, 0, 1, 'signed', true, 'bronze', 'pencil', 'Getting started', false, 2, 0);

--
-- Data for Name: bans; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: bookmarks; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: captcha_spent; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: chat_channel_groups; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: chat_channels; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: chat_flags; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: chat_integration_rules; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: chat_members; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: chat_messages; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: chat_reactions; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: club_bans; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: club_invites; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: clubs; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: color_schemes; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.color_schemes OVERRIDING SYSTEM VALUE VALUES
	(1, 'standard', 'Standard', '{"text": "#1a1f27", "muted": "#596371", "accent": "#0b64c0", "border": "#e1e4e9", "danger": "#b42318", "success": "#17693c", "surface": "#ffffff", "warning": "#8a5300", "background": "#f5f6f8", "header_text": "#1a1f27", "header_background": "#ffffff"}', '{"text": "#e6e9ee", "muted": "#9fa9b6", "accent": "#6fb1f5", "border": "#2c323b", "danger": "#f28b82", "success": "#6fcf97", "surface": "#191d23", "warning": "#e4b567", "background": "#111419", "header_text": "#e6e9ee", "header_background": "#191d23"}', true, 0),
	(2, 'forest', 'Forest', '{"text": "#1a1f27", "muted": "#596371", "accent": "#17693c", "border": "#e1e4e9", "danger": "#b42318", "success": "#17693c", "surface": "#ffffff", "warning": "#8a5300", "background": "#f5f6f8", "header_text": "#1a1f27", "header_background": "#ffffff"}', '{"text": "#e6e9ee", "muted": "#9fa9b6", "accent": "#6fcf97", "border": "#2c323b", "danger": "#f28b82", "success": "#6fcf97", "surface": "#191d23", "warning": "#e4b567", "background": "#111419", "header_text": "#e6e9ee", "header_background": "#191d23"}', true, 1),
	(3, 'plum', 'Plum', '{"text": "#1a1f27", "muted": "#596371", "accent": "#7a3e9d", "border": "#e1e4e9", "danger": "#b42318", "success": "#17693c", "surface": "#ffffff", "warning": "#8a5300", "background": "#f5f6f8", "header_text": "#1a1f27", "header_background": "#ffffff"}', '{"text": "#e6e9ee", "muted": "#9fa9b6", "accent": "#cfa1ea", "border": "#2c323b", "danger": "#f28b82", "success": "#6fcf97", "surface": "#191d23", "warning": "#e4b567", "background": "#111419", "header_text": "#e6e9ee", "header_background": "#191d23"}', true, 2);

--
-- Data for Name: content_translations; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: conversation_messages; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: conversation_participants; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: conversation_reports; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: conversations; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: custom_emoji; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: custom_pages; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: drafts; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: email_tokens; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: embed_topics; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: error_log; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: event_rsvps; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: explorer_queries; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.explorer_queries OVERRIDING SYSTEM VALUE VALUES
	(1, 'Most active members, last 30 days', 'Posts per member in the last 30 days.', 'SELECT u.username, count(*) AS posts
FROM posts p JOIN users u ON u.id = p.author_id
WHERE p.created_at > now() - interval ''30 days'' AND p.state = ''visible'' AND p.action IS NULL
GROUP BY u.username ORDER BY posts DESC LIMIT 50', NULL, '2026-11-16 16:28:19.60724+00', '2026-11-16 16:28:19.60724+00'),
	(2, 'Topics without replies', 'Visible topics nobody has answered, oldest first.', 'SELECT id, title, created_at FROM topics
WHERE state = ''visible'' AND reply_count = 0 ORDER BY created_at LIMIT 100', NULL, '2026-11-16 16:28:19.60724+00', '2026-11-16 16:28:19.60724+00'),
	(3, 'Signups per week', 'New accounts by week.', 'SELECT date_trunc(''week'', joined_at)::date AS week, count(*) AS members
FROM users GROUP BY 1 ORDER BY 1 DESC LIMIT 52', NULL, '2026-11-16 16:28:19.60724+00', '2026-11-16 16:28:19.60724+00');

--
-- Data for Name: failed_logins; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: follows; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: group_requests; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: groups; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.groups OVERRIDING SYSTEM VALUE VALUES
	(1, 'Guests', 'guests', false, false, false, NULL, '{}', false, NULL, NULL, NULL, NULL),
	(2, 'Members', 'members', false, false, false, NULL, '{}', false, NULL, NULL, NULL, NULL),
	(3, 'Moderators', 'moderators', true, true, false, NULL, '{}', false, NULL, NULL, NULL, NULL),
	(4, 'Administrators', 'administrators', true, true, true, NULL, '{}', false, NULL, NULL, NULL, NULL);

--
-- Data for Name: held_content; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: ignored_users; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: import_map; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: invites; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: item_tags; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: item_timers; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: items; Type: TABLE DATA; Schema: public; Owner: -
--


--
-- Data for Name: legal_documents; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: legal_notices; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: link_previews; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: live_question_votes; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: live_questions; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: live_sessions; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: mail_outbox; Type: TABLE DATA; Schema: public; Owner: -
--


--
-- Data for Name: mass_messages; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: member_reports; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: member_status_replies; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: member_statuses; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: mod_actions; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: moderation_statements; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: muted_tags; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: node_moderators; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: node_permissions; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: node_read_markers; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: nodes; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: notification_prefs; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: notifications; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: oauth_clients; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: oauth_codes; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: page_views; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: passkeys; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: pending_logins; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: permalinks; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: plugin_api_scopes; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: plugin_cook_state; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.plugin_cook_state VALUES
	(1, '');

--
-- Data for Name: plugin_files; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: plugin_jobs; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: plugin_kv; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: plugin_meta; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: plugin_permission_grants; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: plugin_permissions; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: plugin_review_items; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: plugin_schedules; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: plugin_search_docs; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: plugins; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: poll_groups; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: poll_options; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: poll_votes; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: polls; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: post_likes; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: post_links; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: post_notes; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: post_revisions; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: post_votes; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: posts; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: profile_fields; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: profile_visits; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: promotion_rules; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: push_cursor; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.push_cursor VALUES
	(1, 0);

--
-- Data for Name: push_queue; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: push_subscriptions; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: reactions; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.reactions VALUES
	(2, 'Thanks', '🙏', 1, true, 2, false, NULL, NULL),
	(3, 'Insightful', '💡', 1, true, 3, false, NULL, NULL),
	(4, 'Funny', '😄', 0, true, 4, false, NULL, NULL),
	(5, 'Disagree', '👎', -1, true, 5, false, NULL, NULL),
	(1, 'Like', '❤️', 1, true, 1, true, NULL, NULL);

--
-- Data for Name: read_markers; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: referral_rewards; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: report_reasons; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.report_reasons VALUES
	('off_topic', 'Off topic', '', 0.5, false, false, true, 10, NULL, NULL, false),
	('inappropriate', 'Inappropriate', '', 1, false, false, true, 20, NULL, NULL, false),
	('spam', 'Spam', '', 1.5, false, false, true, 30, NULL, NULL, false),
	('illegal', 'Illegal', '', 2, true, true, true, 40, NULL, NULL, false),
	('other', 'Something else', '', 1, true, false, true, 50, NULL, NULL, false);

--
-- Data for Name: reports; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: reviewables; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: saved_replies; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: search_best_bets; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: search_logs; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: search_synonyms; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: sessions; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: site_settings; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: sso_nonces; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: tag_group_members; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: tag_group_nodes; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: tag_groups; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: tags; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: task_log; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: themes; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: topic_events; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: topic_redirects; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: topic_votes; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: translation_overrides; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: trusted_devices; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: unsubscribe_tokens; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: upload_quota; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: upload_uses; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: uploads; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: user_badges; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: user_field_values; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: user_follows; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: user_identities; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: user_legal_acceptances; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: user_notes; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: user_secondary_groups; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: user_visits; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: username_history; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: users; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: warnings; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: watched_words; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: webauthn_challenges; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: webhook_deliveries; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: webhooks; Type: TABLE DATA; Schema: public; Owner: -
--

--
-- Data for Name: widgets; Type: TABLE DATA; Schema: public; Owner: -
--

INSERT INTO public.widgets OVERRIDING SYSTEM VALUE VALUES
	(1, 'home_main', 'forum_list', '', '{}', -30, 0),
	(2, 'home_bottom', 'home_latest', '', '{}', -20, 0),
	(3, 'home_bottom', 'home_stats', '', '{}', 100000, 0),
	(4, 'topic_bottom', 'suggested_topics', '', '{}', 0, 0),
	(5, 'home_bottom', 'browsing', '', '{}', 100001, 0),
	(6, 'forum_bottom', 'browsing', '', '{}', 0, 0),
	(7, 'topic_bottom', 'browsing', '', '{}', 1, 0);

--
-- Name: admin_notices_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.admin_notices_id_seq', 1, false);

--
-- Name: ads_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.ads_id_seq', 1, false);

--
-- Name: ai_personas_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.ai_personas_id_seq', 1, false);

--
-- Name: ai_usage_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.ai_usage_id_seq', 1, false);

--
-- Name: announcements_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.announcements_id_seq', 1, false);

--
-- Name: ap_actors_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.ap_actors_id_seq', 1, false);

--
-- Name: ap_deliveries_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.ap_deliveries_id_seq', 1, false);

--
-- Name: api_keys_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.api_keys_id_seq', 1, false);

--
-- Name: audit_log_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.audit_log_id_seq', 1, false);

--
-- Name: automations_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.automations_id_seq', 1, false);

--
-- Name: badges_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.badges_id_seq', 108, true);

--
-- Name: bans_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.bans_id_seq', 1, false);

--
-- Name: chat_channels_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.chat_channels_id_seq', 1, false);

--
-- Name: chat_integration_rules_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.chat_integration_rules_id_seq', 1, false);

--
-- Name: chat_messages_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.chat_messages_id_seq', 1, false);

--
-- Name: clubs_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.clubs_id_seq', 1, false);

--
-- Name: color_schemes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.color_schemes_id_seq', 3, true);

--
-- Name: conversation_messages_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.conversation_messages_id_seq', 1, false);

--
-- Name: conversation_reports_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.conversation_reports_id_seq', 1, false);

--
-- Name: conversations_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.conversations_id_seq', 1, false);

--
-- Name: custom_pages_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.custom_pages_id_seq', 1, false);

--
-- Name: error_log_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.error_log_id_seq', 1, false);

--
-- Name: explorer_queries_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.explorer_queries_id_seq', 3, true);

--
-- Name: failed_logins_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.failed_logins_id_seq', 1, false);

--
-- Name: groups_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.groups_id_seq', 100, false);

--
-- Name: held_content_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.held_content_id_seq', 1, false);

--
-- Name: invites_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.invites_id_seq', 1, false);

--
-- Name: invoice_numbers; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.invoice_numbers', 1, false);

--
-- Name: items_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.items_id_seq', 1, false);

--
-- Name: legal_documents_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.legal_documents_id_seq', 1, false);

--
-- Name: legal_notices_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.legal_notices_id_seq', 1, false);

--
-- Name: live_questions_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.live_questions_id_seq', 1, false);

--
-- Name: mail_outbox_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.mail_outbox_id_seq', 1, false);

--
-- Name: mass_messages_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.mass_messages_id_seq', 1, false);

--
-- Name: member_reports_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.member_reports_id_seq', 1, false);

--
-- Name: member_status_replies_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.member_status_replies_id_seq', 1, false);

--
-- Name: member_statuses_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.member_statuses_id_seq', 1, false);

--
-- Name: mod_actions_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.mod_actions_id_seq', 1, false);

--
-- Name: moderation_statements_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.moderation_statements_id_seq', 1, false);

--
-- Name: nodes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.nodes_id_seq', 1, false);

--
-- Name: notifications_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.notifications_id_seq', 1, false);

--
-- Name: oauth_clients_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.oauth_clients_id_seq', 1, false);

--
-- Name: page_views_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.page_views_id_seq', 1, false);

--
-- Name: permalinks_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.permalinks_id_seq', 1, false);

--
-- Name: plugin_jobs_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.plugin_jobs_id_seq', 1, false);

--
-- Name: plugin_review_items_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.plugin_review_items_id_seq', 1, false);

--
-- Name: poll_options_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.poll_options_id_seq', 1, false);

--
-- Name: polls_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.polls_id_seq', 1, false);

--
-- Name: post_notes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.post_notes_id_seq', 1, false);

--
-- Name: posts_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.posts_id_seq', 1, false);

--
-- Name: profile_fields_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.profile_fields_id_seq', 1, false);

--
-- Name: promotion_rules_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.promotion_rules_id_seq', 1, false);

--
-- Name: push_queue_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.push_queue_id_seq', 1, false);

--
-- Name: reactions_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.reactions_id_seq', 5, true);

--
-- Name: reports_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.reports_id_seq', 1, false);

--
-- Name: reviewables_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.reviewables_id_seq', 1, false);

--
-- Name: saved_replies_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.saved_replies_id_seq', 1, false);

--
-- Name: search_best_bets_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.search_best_bets_id_seq', 1, false);

--
-- Name: search_logs_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.search_logs_id_seq', 1, false);

--
-- Name: search_synonyms_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.search_synonyms_id_seq', 1, false);

--
-- Name: tag_groups_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.tag_groups_id_seq', 1, false);

--
-- Name: tags_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.tags_id_seq', 1, false);

--
-- Name: task_log_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.task_log_id_seq', 1, false);

--
-- Name: themes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.themes_id_seq', 1, false);

--
-- Name: topic_redirects_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.topic_redirects_id_seq', 1, false);

--
-- Name: upload_quota_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.upload_quota_id_seq', 1, false);

--
-- Name: uploads_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.uploads_id_seq', 1, false);

--
-- Name: user_notes_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.user_notes_id_seq', 1, false);

--
-- Name: users_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.users_id_seq', 1, false);

--
-- Name: warnings_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.warnings_id_seq', 1, false);

--
-- Name: watched_words_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.watched_words_id_seq', 1, false);

--
-- Name: webhook_deliveries_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.webhook_deliveries_id_seq', 1, false);

--
-- Name: webhooks_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.webhooks_id_seq', 1, false);

--
-- Name: widgets_id_seq; Type: SEQUENCE SET; Schema: public; Owner: -
--

SELECT pg_catalog.setval('public.widgets_id_seq', 7, true);

--
-- Name: admin_notices admin_notices_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.admin_notices
    ADD CONSTRAINT admin_notices_pkey PRIMARY KEY (id);

--
-- Name: ads ads_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ads
    ADD CONSTRAINT ads_pkey PRIMARY KEY (id);

--
-- Name: ai_embeddings ai_embeddings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_embeddings
    ADD CONSTRAINT ai_embeddings_pkey PRIMARY KEY (item_id);

--
-- Name: ai_personas ai_personas_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_personas
    ADD CONSTRAINT ai_personas_pkey PRIMARY KEY (id);

--
-- Name: ai_personas ai_personas_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_personas
    ADD CONSTRAINT ai_personas_slug_key UNIQUE (slug);

--
-- Name: ai_post_labels ai_post_labels_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_post_labels
    ADD CONSTRAINT ai_post_labels_pkey PRIMARY KEY (post_id);

--
-- Name: ai_summaries ai_summaries_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_summaries
    ADD CONSTRAINT ai_summaries_pkey PRIMARY KEY (item_id, kind, locale);

--
-- Name: ai_usage ai_usage_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage
    ADD CONSTRAINT ai_usage_pkey PRIMARY KEY (id);

--
-- Name: announcements announcements_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.announcements
    ADD CONSTRAINT announcements_pkey PRIMARY KEY (id);

--
-- Name: ap_actors ap_actors_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_actors
    ADD CONSTRAINT ap_actors_pkey PRIMARY KEY (id);

--
-- Name: ap_actors ap_actors_uri_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_actors
    ADD CONSTRAINT ap_actors_uri_key UNIQUE (uri);

--
-- Name: ap_actors ap_actors_user_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_actors
    ADD CONSTRAINT ap_actors_user_id_key UNIQUE (user_id);

--
-- Name: ap_deliveries ap_deliveries_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_deliveries
    ADD CONSTRAINT ap_deliveries_pkey PRIMARY KEY (id);

--
-- Name: ap_dm_objects ap_dm_objects_message_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_dm_objects
    ADD CONSTRAINT ap_dm_objects_message_id_key UNIQUE (message_id);

--
-- Name: ap_dm_objects ap_dm_objects_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_dm_objects
    ADD CONSTRAINT ap_dm_objects_pkey PRIMARY KEY (uri);

--
-- Name: ap_followers ap_followers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_followers
    ADD CONSTRAINT ap_followers_pkey PRIMARY KEY (local_type, local_id, actor_id);

--
-- Name: ap_keys ap_keys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_keys
    ADD CONSTRAINT ap_keys_pkey PRIMARY KEY (owner_type, owner_id);

--
-- Name: ap_mirrors ap_mirrors_follow_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_mirrors
    ADD CONSTRAINT ap_mirrors_follow_id_key UNIQUE (follow_id);

--
-- Name: ap_mirrors ap_mirrors_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_mirrors
    ADD CONSTRAINT ap_mirrors_pkey PRIMARY KEY (node_id, actor_id);

--
-- Name: ap_objects ap_objects_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_objects
    ADD CONSTRAINT ap_objects_pkey PRIMARY KEY (uri);

--
-- Name: ap_objects ap_objects_post_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_objects
    ADD CONSTRAINT ap_objects_post_id_key UNIQUE (post_id);

--
-- Name: ap_seen ap_seen_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_seen
    ADD CONSTRAINT ap_seen_pkey PRIMARY KEY (activity_id);

--
-- Name: api_idempotency api_idempotency_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.api_idempotency
    ADD CONSTRAINT api_idempotency_pkey PRIMARY KEY (key_digest);

--
-- Name: api_keys api_keys_key_digest_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_key_digest_key UNIQUE (key_digest);

--
-- Name: api_keys api_keys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_pkey PRIMARY KEY (id);

--
-- Name: audit_forward_state audit_forward_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.audit_forward_state
    ADD CONSTRAINT audit_forward_state_pkey PRIMARY KEY (id);

--
-- Name: audit_log audit_log_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.audit_log
    ADD CONSTRAINT audit_log_pkey PRIMARY KEY (id);

--
-- Name: automations automations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.automations
    ADD CONSTRAINT automations_pkey PRIMARY KEY (id);

--
-- Name: backup_codes backup_codes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.backup_codes
    ADD CONSTRAINT backup_codes_pkey PRIMARY KEY (user_id, code_digest);

--
-- Name: badges badges_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.badges
    ADD CONSTRAINT badges_pkey PRIMARY KEY (id);

--
-- Name: badges badges_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.badges
    ADD CONSTRAINT badges_slug_key UNIQUE (slug);

--
-- Name: bans bans_kind_value_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bans
    ADD CONSTRAINT bans_kind_value_key UNIQUE (kind, value);

--
-- Name: bans bans_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bans
    ADD CONSTRAINT bans_pkey PRIMARY KEY (id);

--
-- Name: bookmarks bookmarks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bookmarks
    ADD CONSTRAINT bookmarks_pkey PRIMARY KEY (user_id, post_id);

--
-- Name: captcha_spent captcha_spent_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.captcha_spent
    ADD CONSTRAINT captcha_spent_pkey PRIMARY KEY (signature);

--
-- Name: chat_channel_groups chat_channel_groups_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_channel_groups
    ADD CONSTRAINT chat_channel_groups_pkey PRIMARY KEY (channel_id, group_id);

--
-- Name: chat_channels chat_channels_dm_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_channels
    ADD CONSTRAINT chat_channels_dm_key_key UNIQUE (dm_key);

--
-- Name: chat_channels chat_channels_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_channels
    ADD CONSTRAINT chat_channels_pkey PRIMARY KEY (id);

--
-- Name: chat_flags chat_flags_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_flags
    ADD CONSTRAINT chat_flags_pkey PRIMARY KEY (message_id, user_id);

--
-- Name: chat_integration_rules chat_integration_rules_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_integration_rules
    ADD CONSTRAINT chat_integration_rules_pkey PRIMARY KEY (id);

--
-- Name: chat_members chat_members_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_members
    ADD CONSTRAINT chat_members_pkey PRIMARY KEY (channel_id, user_id);

--
-- Name: chat_messages chat_messages_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_messages
    ADD CONSTRAINT chat_messages_pkey PRIMARY KEY (id);

--
-- Name: chat_reactions chat_reactions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_reactions
    ADD CONSTRAINT chat_reactions_pkey PRIMARY KEY (message_id, user_id, emoji);

--
-- Name: club_bans club_bans_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.club_bans
    ADD CONSTRAINT club_bans_pkey PRIMARY KEY (club_id, user_id);

--
-- Name: club_invites club_invites_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.club_invites
    ADD CONSTRAINT club_invites_pkey PRIMARY KEY (club_id, user_id);

--
-- Name: clubs clubs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.clubs
    ADD CONSTRAINT clubs_pkey PRIMARY KEY (id);

--
-- Name: color_schemes color_schemes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.color_schemes
    ADD CONSTRAINT color_schemes_pkey PRIMARY KEY (id);

--
-- Name: color_schemes color_schemes_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.color_schemes
    ADD CONSTRAINT color_schemes_slug_key UNIQUE (slug);

--
-- Name: content_translations content_translations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.content_translations
    ADD CONSTRAINT content_translations_pkey PRIMARY KEY (kind, target_id, locale);

--
-- Name: conversation_messages conversation_messages_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_messages
    ADD CONSTRAINT conversation_messages_pkey PRIMARY KEY (id);

--
-- Name: conversation_participants conversation_participants_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_participants
    ADD CONSTRAINT conversation_participants_pkey PRIMARY KEY (conversation_id, user_id);

--
-- Name: conversation_reports conversation_reports_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_reports
    ADD CONSTRAINT conversation_reports_pkey PRIMARY KEY (id);

--
-- Name: conversations conversations_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_pkey PRIMARY KEY (id);

--
-- Name: custom_emoji custom_emoji_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_emoji
    ADD CONSTRAINT custom_emoji_pkey PRIMARY KEY (name);

--
-- Name: custom_pages custom_pages_path_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_pages
    ADD CONSTRAINT custom_pages_path_key UNIQUE (path);

--
-- Name: custom_pages custom_pages_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_pages
    ADD CONSTRAINT custom_pages_pkey PRIMARY KEY (id);

--
-- Name: custom_pages custom_pages_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.custom_pages
    ADD CONSTRAINT custom_pages_slug_key UNIQUE (slug);

--
-- Name: drafts drafts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.drafts
    ADD CONSTRAINT drafts_pkey PRIMARY KEY (user_id, context);

--
-- Name: email_tokens email_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.email_tokens
    ADD CONSTRAINT email_tokens_pkey PRIMARY KEY (token_digest);

--
-- Name: embed_topics embed_topics_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.embed_topics
    ADD CONSTRAINT embed_topics_pkey PRIMARY KEY (url);

--
-- Name: error_log error_log_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.error_log
    ADD CONSTRAINT error_log_pkey PRIMARY KEY (id);

--
-- Name: event_rsvps event_rsvps_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.event_rsvps
    ADD CONSTRAINT event_rsvps_pkey PRIMARY KEY (item_id, user_id);

--
-- Name: explorer_queries explorer_queries_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.explorer_queries
    ADD CONSTRAINT explorer_queries_pkey PRIMARY KEY (id);

--
-- Name: failed_logins failed_logins_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.failed_logins
    ADD CONSTRAINT failed_logins_pkey PRIMARY KEY (id);

--
-- Name: follows follows_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follows
    ADD CONSTRAINT follows_pkey PRIMARY KEY (user_id, target_type, target_id);

--
-- Name: group_requests group_requests_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.group_requests
    ADD CONSTRAINT group_requests_pkey PRIMARY KEY (group_id, user_id);

--
-- Name: groups groups_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.groups
    ADD CONSTRAINT groups_name_key UNIQUE (name);

--
-- Name: groups groups_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.groups
    ADD CONSTRAINT groups_pkey PRIMARY KEY (id);

--
-- Name: groups groups_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.groups
    ADD CONSTRAINT groups_slug_key UNIQUE (slug);

--
-- Name: held_content held_content_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.held_content
    ADD CONSTRAINT held_content_pkey PRIMARY KEY (id);

--
-- Name: ignored_users ignored_users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ignored_users
    ADD CONSTRAINT ignored_users_pkey PRIMARY KEY (user_id, ignored_user_id);

--
-- Name: import_map import_map_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.import_map
    ADD CONSTRAINT import_map_pkey PRIMARY KEY (kind, legacy_id);

--
-- Name: invites invites_code_digest_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invites
    ADD CONSTRAINT invites_code_digest_key UNIQUE (code_digest);

--
-- Name: invites invites_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invites
    ADD CONSTRAINT invites_pkey PRIMARY KEY (id);

--
-- Name: item_tags item_tags_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.item_tags
    ADD CONSTRAINT item_tags_pkey PRIMARY KEY (item_id, tag_id);

--
-- Name: item_timers item_timers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.item_timers
    ADD CONSTRAINT item_timers_pkey PRIMARY KEY (item_id);

--
-- Name: items items_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.items
    ADD CONSTRAINT items_pkey PRIMARY KEY (id);


--
-- Name: legal_documents legal_documents_kind_version_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.legal_documents
    ADD CONSTRAINT legal_documents_kind_version_key UNIQUE (kind, version);

--
-- Name: legal_documents legal_documents_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.legal_documents
    ADD CONSTRAINT legal_documents_pkey PRIMARY KEY (id);

--
-- Name: legal_notices legal_notices_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.legal_notices
    ADD CONSTRAINT legal_notices_pkey PRIMARY KEY (id);

--
-- Name: link_previews link_previews_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.link_previews
    ADD CONSTRAINT link_previews_pkey PRIMARY KEY (url);

--
-- Name: live_question_votes live_question_votes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.live_question_votes
    ADD CONSTRAINT live_question_votes_pkey PRIMARY KEY (question_id, user_id);

--
-- Name: live_questions live_questions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.live_questions
    ADD CONSTRAINT live_questions_pkey PRIMARY KEY (id);

--
-- Name: live_sessions live_sessions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.live_sessions
    ADD CONSTRAINT live_sessions_pkey PRIMARY KEY (item_id);

--
-- Name: mail_outbox mail_outbox_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mail_outbox
    ADD CONSTRAINT mail_outbox_pkey PRIMARY KEY (id);


--
-- Name: mass_messages mass_messages_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mass_messages
    ADD CONSTRAINT mass_messages_pkey PRIMARY KEY (id);

--
-- Name: member_reports member_reports_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.member_reports
    ADD CONSTRAINT member_reports_pkey PRIMARY KEY (id);

--
-- Name: member_status_replies member_status_replies_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.member_status_replies
    ADD CONSTRAINT member_status_replies_pkey PRIMARY KEY (id);

--
-- Name: member_statuses member_statuses_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.member_statuses
    ADD CONSTRAINT member_statuses_pkey PRIMARY KEY (id);

--
-- Name: mod_actions mod_actions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mod_actions
    ADD CONSTRAINT mod_actions_pkey PRIMARY KEY (id);

--
-- Name: moderation_statements moderation_statements_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.moderation_statements
    ADD CONSTRAINT moderation_statements_pkey PRIMARY KEY (id);

--
-- Name: muted_tags muted_tags_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.muted_tags
    ADD CONSTRAINT muted_tags_pkey PRIMARY KEY (user_id, tag_id);

--
-- Name: node_moderators node_moderators_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.node_moderators
    ADD CONSTRAINT node_moderators_pkey PRIMARY KEY (node_id, user_id);

--
-- Name: node_permissions node_permissions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.node_permissions
    ADD CONSTRAINT node_permissions_pkey PRIMARY KEY (node_id, group_id);

--
-- Name: node_read_markers node_read_markers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.node_read_markers
    ADD CONSTRAINT node_read_markers_pkey PRIMARY KEY (user_id, node_id);

--
-- Name: nodes nodes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.nodes
    ADD CONSTRAINT nodes_pkey PRIMARY KEY (id);

--
-- Name: notification_prefs notification_prefs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notification_prefs
    ADD CONSTRAINT notification_prefs_pkey PRIMARY KEY (user_id, kind);

--
-- Name: notifications notifications_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_pkey PRIMARY KEY (id);

--
-- Name: oauth_clients oauth_clients_client_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_clients
    ADD CONSTRAINT oauth_clients_client_id_key UNIQUE (client_id);

--
-- Name: oauth_clients oauth_clients_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_clients
    ADD CONSTRAINT oauth_clients_pkey PRIMARY KEY (id);

--
-- Name: oauth_codes oauth_codes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_codes
    ADD CONSTRAINT oauth_codes_pkey PRIMARY KEY (code_digest);

--
-- Name: page_views page_views_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.page_views
    ADD CONSTRAINT page_views_pkey PRIMARY KEY (id);

--
-- Name: passkeys passkeys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.passkeys
    ADD CONSTRAINT passkeys_pkey PRIMARY KEY (id);

--
-- Name: pending_logins pending_logins_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.pending_logins
    ADD CONSTRAINT pending_logins_pkey PRIMARY KEY (token_digest);

--
-- Name: permalinks permalinks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.permalinks
    ADD CONSTRAINT permalinks_pkey PRIMARY KEY (id);

--
-- Name: plugin_api_scopes plugin_api_scopes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_api_scopes
    ADD CONSTRAINT plugin_api_scopes_pkey PRIMARY KEY (plugin, key);

--
-- Name: plugin_cook_state plugin_cook_state_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_cook_state
    ADD CONSTRAINT plugin_cook_state_pkey PRIMARY KEY (id);

--
-- Name: plugin_files plugin_files_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_files
    ADD CONSTRAINT plugin_files_pkey PRIMARY KEY (plugin, path);

--
-- Name: plugin_jobs plugin_jobs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_jobs
    ADD CONSTRAINT plugin_jobs_pkey PRIMARY KEY (id);

--
-- Name: plugin_kv plugin_kv_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_kv
    ADD CONSTRAINT plugin_kv_pkey PRIMARY KEY (plugin, key);

--
-- Name: plugin_meta plugin_meta_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_meta
    ADD CONSTRAINT plugin_meta_pkey PRIMARY KEY (plugin, key, target, target_id);

--
-- Name: plugin_permission_grants plugin_permission_grants_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_permission_grants
    ADD CONSTRAINT plugin_permission_grants_pkey PRIMARY KEY (plugin, perm, group_id);

--
-- Name: plugin_permissions plugin_permissions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_permissions
    ADD CONSTRAINT plugin_permissions_pkey PRIMARY KEY (plugin, key);

--
-- Name: plugin_review_items plugin_review_items_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_review_items
    ADD CONSTRAINT plugin_review_items_pkey PRIMARY KEY (id);

--
-- Name: plugin_schedules plugin_schedules_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_schedules
    ADD CONSTRAINT plugin_schedules_pkey PRIMARY KEY (plugin, name);

--
-- Name: plugin_search_docs plugin_search_docs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_search_docs
    ADD CONSTRAINT plugin_search_docs_pkey PRIMARY KEY (plugin, type, ref);

--
-- Name: plugins plugins_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugins
    ADD CONSTRAINT plugins_pkey PRIMARY KEY (id);

--
-- Name: poll_groups poll_groups_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_groups
    ADD CONSTRAINT poll_groups_pkey PRIMARY KEY (poll_id, group_id);

--
-- Name: poll_options poll_options_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_options
    ADD CONSTRAINT poll_options_pkey PRIMARY KEY (id);

--
-- Name: poll_votes poll_votes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_votes
    ADD CONSTRAINT poll_votes_pkey PRIMARY KEY (poll_id, user_id, option_id);

--
-- Name: polls polls_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.polls
    ADD CONSTRAINT polls_pkey PRIMARY KEY (id);

--
-- Name: post_likes post_likes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_likes
    ADD CONSTRAINT post_likes_pkey PRIMARY KEY (post_id, user_id);

--
-- Name: post_links post_links_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_links
    ADD CONSTRAINT post_links_pkey PRIMARY KEY (post_id, url);

--
-- Name: post_notes post_notes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_notes
    ADD CONSTRAINT post_notes_pkey PRIMARY KEY (id);

--
-- Name: post_revisions post_revisions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_revisions
    ADD CONSTRAINT post_revisions_pkey PRIMARY KEY (post_id, n);

--
-- Name: post_votes post_votes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_votes
    ADD CONSTRAINT post_votes_pkey PRIMARY KEY (post_id, user_id);

--
-- Name: posts posts_item_id_post_number_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.posts
    ADD CONSTRAINT posts_item_id_post_number_key UNIQUE (item_id, post_number);

--
-- Name: posts posts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.posts
    ADD CONSTRAINT posts_pkey PRIMARY KEY (id);

--
-- Name: profile_fields profile_fields_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.profile_fields
    ADD CONSTRAINT profile_fields_pkey PRIMARY KEY (id);

--
-- Name: profile_visits profile_visits_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.profile_visits
    ADD CONSTRAINT profile_visits_pkey PRIMARY KEY (profile_id, visitor_id);

--
-- Name: promotion_rules promotion_rules_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.promotion_rules
    ADD CONSTRAINT promotion_rules_pkey PRIMARY KEY (id);

--
-- Name: push_cursor push_cursor_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.push_cursor
    ADD CONSTRAINT push_cursor_pkey PRIMARY KEY (id);

--
-- Name: push_queue push_queue_notification_id_endpoint_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.push_queue
    ADD CONSTRAINT push_queue_notification_id_endpoint_key UNIQUE (notification_id, endpoint);

--
-- Name: push_queue push_queue_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.push_queue
    ADD CONSTRAINT push_queue_pkey PRIMARY KEY (id);

--
-- Name: push_subscriptions push_subscriptions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.push_subscriptions
    ADD CONSTRAINT push_subscriptions_pkey PRIMARY KEY (endpoint);

--
-- Name: reactions reactions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reactions
    ADD CONSTRAINT reactions_pkey PRIMARY KEY (id);

--
-- Name: read_markers read_markers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.read_markers
    ADD CONSTRAINT read_markers_pkey PRIMARY KEY (user_id, item_id);

--
-- Name: referral_rewards referral_rewards_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.referral_rewards
    ADD CONSTRAINT referral_rewards_pkey PRIMARY KEY (user_id, group_id);

--
-- Name: report_reasons report_reasons_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_reasons
    ADD CONSTRAINT report_reasons_pkey PRIMARY KEY (key);

--
-- Name: reports reports_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_pkey PRIMARY KEY (id);

--
-- Name: reviewables reviewables_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reviewables
    ADD CONSTRAINT reviewables_pkey PRIMARY KEY (id);

--
-- Name: reviewables reviewables_target_type_target_id_kind_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reviewables
    ADD CONSTRAINT reviewables_target_type_target_id_kind_key UNIQUE (target_type, target_id, kind);

--
-- Name: saved_replies saved_replies_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.saved_replies
    ADD CONSTRAINT saved_replies_pkey PRIMARY KEY (id);

--
-- Name: search_best_bets search_best_bets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.search_best_bets
    ADD CONSTRAINT search_best_bets_pkey PRIMARY KEY (id);

--
-- Name: search_logs search_logs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.search_logs
    ADD CONSTRAINT search_logs_pkey PRIMARY KEY (id);

--
-- Name: search_synonyms search_synonyms_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.search_synonyms
    ADD CONSTRAINT search_synonyms_pkey PRIMARY KEY (id);

--
-- Name: sessions sessions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_pkey PRIMARY KEY (token_digest);

--
-- Name: site_settings site_settings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.site_settings
    ADD CONSTRAINT site_settings_pkey PRIMARY KEY (key);

--
-- Name: sso_nonces sso_nonces_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sso_nonces
    ADD CONSTRAINT sso_nonces_pkey PRIMARY KEY (digest);

--
-- Name: tag_group_members tag_group_members_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tag_group_members
    ADD CONSTRAINT tag_group_members_pkey PRIMARY KEY (group_id, tag_id);

--
-- Name: tag_group_nodes tag_group_nodes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tag_group_nodes
    ADD CONSTRAINT tag_group_nodes_pkey PRIMARY KEY (group_id, node_id);

--
-- Name: tag_groups tag_groups_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tag_groups
    ADD CONSTRAINT tag_groups_pkey PRIMARY KEY (id);

--
-- Name: tags tags_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tags
    ADD CONSTRAINT tags_pkey PRIMARY KEY (id);

--
-- Name: tags tags_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tags
    ADD CONSTRAINT tags_slug_key UNIQUE (slug);

--
-- Name: task_log task_log_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.task_log
    ADD CONSTRAINT task_log_pkey PRIMARY KEY (id);

--
-- Name: themes themes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.themes
    ADD CONSTRAINT themes_pkey PRIMARY KEY (id);

--
-- Name: themes themes_slug_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.themes
    ADD CONSTRAINT themes_slug_key UNIQUE (slug);

--
-- Name: topic_events topic_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.topic_events
    ADD CONSTRAINT topic_events_pkey PRIMARY KEY (item_id);

--
-- Name: topic_redirects topic_redirects_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.topic_redirects
    ADD CONSTRAINT topic_redirects_pkey PRIMARY KEY (id);

--
-- Name: topic_votes topic_votes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.topic_votes
    ADD CONSTRAINT topic_votes_pkey PRIMARY KEY (item_id, user_id);

--
-- Name: translation_overrides translation_overrides_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.translation_overrides
    ADD CONSTRAINT translation_overrides_pkey PRIMARY KEY (locale, key);

--
-- Name: trusted_devices trusted_devices_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.trusted_devices
    ADD CONSTRAINT trusted_devices_pkey PRIMARY KEY (token_digest);

--
-- Name: unsubscribe_tokens unsubscribe_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.unsubscribe_tokens
    ADD CONSTRAINT unsubscribe_tokens_pkey PRIMARY KEY (token);

--
-- Name: unsubscribe_tokens unsubscribe_tokens_reply_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.unsubscribe_tokens
    ADD CONSTRAINT unsubscribe_tokens_reply_key_key UNIQUE (reply_key);

--
-- Name: upload_quota upload_quota_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.upload_quota
    ADD CONSTRAINT upload_quota_pkey PRIMARY KEY (id);

--
-- Name: upload_uses upload_uses_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.upload_uses
    ADD CONSTRAINT upload_uses_pkey PRIMARY KEY (kind, ref_id, upload_id);

--
-- Name: uploads uploads_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.uploads
    ADD CONSTRAINT uploads_pkey PRIMARY KEY (id);

--
-- Name: uploads uploads_storage_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.uploads
    ADD CONSTRAINT uploads_storage_key_key UNIQUE (storage_key);

--
-- Name: user_badges user_badges_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_badges
    ADD CONSTRAINT user_badges_pkey PRIMARY KEY (user_id, badge_id);

--
-- Name: user_field_values user_field_values_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_field_values
    ADD CONSTRAINT user_field_values_pkey PRIMARY KEY (user_id, field_id);

--
-- Name: user_follows user_follows_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_follows
    ADD CONSTRAINT user_follows_pkey PRIMARY KEY (user_id, target_id);

--
-- Name: user_identities user_identities_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_identities
    ADD CONSTRAINT user_identities_pkey PRIMARY KEY (provider, subject);

--
-- Name: user_legal_acceptances user_legal_acceptances_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_legal_acceptances
    ADD CONSTRAINT user_legal_acceptances_pkey PRIMARY KEY (user_id, document_id);

--
-- Name: user_notes user_notes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_notes
    ADD CONSTRAINT user_notes_pkey PRIMARY KEY (id);

--
-- Name: user_secondary_groups user_secondary_groups_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_secondary_groups
    ADD CONSTRAINT user_secondary_groups_pkey PRIMARY KEY (user_id, group_id);

--
-- Name: user_visits user_visits_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_visits
    ADD CONSTRAINT user_visits_pkey PRIMARY KEY (user_id, day);

--
-- Name: users users_email_lower_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_email_lower_key UNIQUE (email_lower);

--
-- Name: users users_mail_post_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_mail_post_key_key UNIQUE (mail_post_key);

--
-- Name: users users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);

--
-- Name: users users_username_lower_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_username_lower_key UNIQUE (username_lower);

--
-- Name: warnings warnings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.warnings
    ADD CONSTRAINT warnings_pkey PRIMARY KEY (id);

--
-- Name: watched_words watched_words_action_pattern_key; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.watched_words
    ADD CONSTRAINT watched_words_action_pattern_key UNIQUE (action, pattern);

--
-- Name: watched_words watched_words_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.watched_words
    ADD CONSTRAINT watched_words_pkey PRIMARY KEY (id);

--
-- Name: webauthn_challenges webauthn_challenges_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webauthn_challenges
    ADD CONSTRAINT webauthn_challenges_pkey PRIMARY KEY (token_digest);

--
-- Name: webhook_deliveries webhook_deliveries_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webhook_deliveries
    ADD CONSTRAINT webhook_deliveries_pkey PRIMARY KEY (id);

--
-- Name: webhooks webhooks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webhooks
    ADD CONSTRAINT webhooks_pkey PRIMARY KEY (id);

--
-- Name: widgets widgets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.widgets
    ADD CONSTRAINT widgets_pkey PRIMARY KEY (id);

--
-- Name: ai_embeddings_node; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_embeddings_node ON public.ai_embeddings USING btree (node_id);

--
-- Name: ai_post_labels_day; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_post_labels_day ON public.ai_post_labels USING btree (created_at);

--
-- Name: ai_usage_day; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_usage_day ON public.ai_usage USING btree (created_at);

--
-- Name: ai_usage_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ai_usage_user ON public.ai_usage USING btree (user_id, created_at) WHERE (user_id IS NOT NULL);

--
-- Name: ap_actors_domain; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ap_actors_domain ON public.ap_actors USING btree (domain);

--
-- Name: ap_actors_key; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ap_actors_key ON public.ap_actors USING btree (key_id);

--
-- Name: ap_deliveries_due; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ap_deliveries_due ON public.ap_deliveries USING btree (next_at);

--
-- Name: ap_dm_objects_conversation; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ap_dm_objects_conversation ON public.ap_dm_objects USING btree (conversation_id);

--
-- Name: ap_mirrors_actor; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ap_mirrors_actor ON public.ap_mirrors USING btree (actor_id);

--
-- Name: ap_objects_actor; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ap_objects_actor ON public.ap_objects USING btree (actor_id, created_at);

--
-- Name: api_idempotency_age; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX api_idempotency_age ON public.api_idempotency USING btree (created_at);

--
-- Name: api_keys_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX api_keys_user ON public.api_keys USING btree (user_id);

--
-- Name: audit_log_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX audit_log_created_at ON public.audit_log USING btree (created_at DESC);

--
-- Name: audit_log_ip_age; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX audit_log_ip_age ON public.audit_log USING btree (created_at) WHERE (ip IS NOT NULL);

--
-- Name: bookmarks_due; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX bookmarks_due ON public.bookmarks USING btree (remind_at) WHERE (remind_at IS NOT NULL);

--
-- Name: bookmarks_post; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX bookmarks_post ON public.bookmarks USING btree (post_id);

--
-- Name: chat_flags_message; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX chat_flags_message ON public.chat_flags USING btree (message_id);

--
-- Name: chat_members_by_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX chat_members_by_user ON public.chat_members USING btree (user_id);

--
-- Name: chat_messages_by_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX chat_messages_by_user ON public.chat_messages USING btree (user_id, created_at DESC);

--
-- Name: chat_messages_page; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX chat_messages_page ON public.chat_messages USING btree (channel_id, id DESC);

--
-- Name: chat_reactions_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX chat_reactions_user ON public.chat_reactions USING btree (user_id);

--
-- Name: conversation_messages_author; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX conversation_messages_author ON public.conversation_messages USING btree (author_id, created_at DESC);

--
-- Name: conversation_messages_by_conversation; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX conversation_messages_by_conversation ON public.conversation_messages USING btree (conversation_id, id);

--
-- Name: conversation_participants_by_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX conversation_participants_by_user ON public.conversation_participants USING btree (user_id) WHERE (left_at IS NULL);

--
-- Name: conversation_reports_open; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX conversation_reports_open ON public.conversation_reports USING btree (id) WHERE (resolved_at IS NULL);

--
-- Name: conversations_starter; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX conversations_starter ON public.conversations USING btree (starter_id, created_at DESC);

--
-- Name: email_tokens_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX email_tokens_user ON public.email_tokens USING btree (user_id);

--
-- Name: event_rsvps_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX event_rsvps_user ON public.event_rsvps USING btree (user_id);

--
-- Name: failed_logins_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX failed_logins_created ON public.failed_logins USING btree (created_at);

--
-- Name: failed_logins_ip; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX failed_logins_ip ON public.failed_logins USING btree (ip, created_at DESC);

--
-- Name: failed_logins_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX failed_logins_user ON public.failed_logins USING btree (user_id, created_at DESC);

--
-- Name: follows_target; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX follows_target ON public.follows USING btree (target_type, target_id) WHERE (level = 'watch'::text);

--
-- Name: groups_plugin_key; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX groups_plugin_key ON public.groups USING btree (plugin, plugin_key) WHERE (plugin IS NOT NULL);

--
-- Name: held_content_pending; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX held_content_pending ON public.held_content USING btree (created_at) WHERE (status = 'pending'::text);

--
-- Name: held_content_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX held_content_user ON public.held_content USING btree (user_id);

--
-- Name: ignored_users_target; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX ignored_users_target ON public.ignored_users USING btree (ignored_user_id);

--
-- Name: item_tags_tag; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX item_tags_tag ON public.item_tags USING btree (tag_id, item_id);

--
-- Name: item_timers_due; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX item_timers_due ON public.item_timers USING btree (run_at);

--
-- Name: items_assigned; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX items_assigned ON public.items USING btree (assigned_to) WHERE (assigned_to IS NOT NULL);

--
-- Name: items_author; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX items_author ON public.items USING btree (author_id, created_at DESC);

--
-- Name: items_deleted_by; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX items_deleted_by ON public.items USING btree (deleted_by) WHERE (deleted_by IS NOT NULL);

--
-- Name: items_last_poster; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX items_last_poster ON public.items USING btree (last_poster_id);

--
-- Name: items_latest; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX items_latest ON public.items USING btree (last_posted_at DESC, id DESC) WHERE ((state = 'visible'::text) AND listed);

--
-- Name: items_node_activity; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX items_node_activity ON public.items USING btree (node_id, pinned DESC, last_posted_at DESC, id DESC) WHERE (state = 'visible'::text);

--
-- Name: items_node_votes; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX items_node_votes ON public.items USING btree (node_id, vote_count DESC, id DESC) WHERE (state = 'visible'::text);

--
-- Name: items_search; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX items_search ON public.items USING gin (search_vector);

--
-- Name: items_title_trgm; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX items_title_trgm ON public.items USING gin (public.bf_unaccent(lower(title)) public.gin_trgm_ops);

--
-- Name: legal_notices_open; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX legal_notices_open ON public.legal_notices USING btree (created_at) WHERE (status = 'open'::text);

--
-- Name: live_questions_item; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX live_questions_item ON public.live_questions USING btree (item_id, status);

--
-- Name: live_sessions_when; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX live_sessions_when ON public.live_sessions USING btree (ends_at);

--
-- Name: mail_outbox_sent; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX mail_outbox_sent ON public.mail_outbox USING btree (sent_at) WHERE (sent_at IS NOT NULL);

--
-- Name: mail_outbox_unsent; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX mail_outbox_unsent ON public.mail_outbox USING btree (id) WHERE (sent_at IS NULL);

--
-- Name: mass_messages_recent; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX mass_messages_recent ON public.mass_messages USING btree (id DESC);

--
-- Name: member_reports_open; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX member_reports_open ON public.member_reports USING btree (user_id, reporter_id) WHERE (resolved_at IS NULL);

--
-- Name: member_reports_open_all; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX member_reports_open_all ON public.member_reports USING btree (id) WHERE (resolved_at IS NULL);

--
-- Name: member_status_replies_by_author; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX member_status_replies_by_author ON public.member_status_replies USING btree (author_id);

--
-- Name: member_status_replies_by_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX member_status_replies_by_status ON public.member_status_replies USING btree (status_id, id);

--
-- Name: member_statuses_by_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX member_statuses_by_user ON public.member_statuses USING btree (user_id, id DESC);

--
-- Name: moderation_statements_appeals; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX moderation_statements_appeals ON public.moderation_statements USING btree (appealed_at) WHERE ((appealed_at IS NOT NULL) AND (appeal_outcome IS NULL));

--
-- Name: moderation_statements_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX moderation_statements_user ON public.moderation_statements USING btree (user_id, created_at DESC);

--
-- Name: node_moderators_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX node_moderators_user ON public.node_moderators USING btree (user_id);

--
-- Name: nodes_parent; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX nodes_parent ON public.nodes USING btree (parent_id, "position");

--
-- Name: nodes_plugin_key; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX nodes_plugin_key ON public.nodes USING btree (plugin, plugin_key) WHERE (plugin IS NOT NULL);

--
-- Name: notifications_folding; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX notifications_folding ON public.notifications USING btree (user_id, kind, ((data ->> 'group'::text))) WHERE ((NOT read) AND (data ? 'group'::text));

--
-- Name: notifications_group; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX notifications_group ON public.notifications USING btree (user_id, kind, ((data ->> 'post_id'::text))) WHERE (NOT read);

--
-- Name: notifications_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX notifications_user ON public.notifications USING btree (user_id, read, created_at DESC);

--
-- Name: page_views_by_time; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX page_views_by_time ON public.page_views USING btree (at);

--
-- Name: passkeys_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX passkeys_user ON public.passkeys USING btree (user_id);

--
-- Name: permalinks_by_url; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX permalinks_by_url ON public.permalinks USING btree (lower(url));

--
-- Name: plugin_jobs_due; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX plugin_jobs_due ON public.plugin_jobs USING btree (run_at);

--
-- Name: plugin_meta_target; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX plugin_meta_target ON public.plugin_meta USING btree (target, target_id);

--
-- Name: plugin_review_items_pending; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX plugin_review_items_pending ON public.plugin_review_items USING btree (plugin, type, ref) WHERE (status = 'pending'::text);

--
-- Name: plugin_review_items_queue; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX plugin_review_items_queue ON public.plugin_review_items USING btree (status, created_at);

--
-- Name: plugin_search_docs_vector; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX plugin_search_docs_vector ON public.plugin_search_docs USING gin (search_vector);

--
-- Name: poll_options_by_item; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX poll_options_by_item ON public.poll_options USING btree (item_id);

--
-- Name: poll_options_by_poll; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX poll_options_by_poll ON public.poll_options USING btree (poll_id, "position");

--
-- Name: poll_votes_by_option; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX poll_votes_by_option ON public.poll_votes USING btree (option_id);

--
-- Name: poll_votes_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX poll_votes_user ON public.poll_votes USING btree (user_id);

--
-- Name: polls_closing; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX polls_closing ON public.polls USING btree (closes_at) WHERE (closed_notified_at IS NULL);

--
-- Name: polls_item; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX polls_item ON public.polls USING btree (item_id, "position");

--
-- Name: post_likes_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX post_likes_user ON public.post_likes USING btree (user_id);

--
-- Name: post_links_url; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX post_links_url ON public.post_links USING btree (url);

--
-- Name: post_notes_by_post; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX post_notes_by_post ON public.post_notes USING btree (post_id, created_at);

--
-- Name: post_revisions_editor; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX post_revisions_editor ON public.post_revisions USING btree (editor_id);

--
-- Name: post_votes_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX post_votes_user ON public.post_votes USING btree (user_id);

--
-- Name: posts_author; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX posts_author ON public.posts USING btree (author_id, created_at DESC);

--
-- Name: posts_deleted_by; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX posts_deleted_by ON public.posts USING btree (deleted_by) WHERE (deleted_by IS NOT NULL);

--
-- Name: posts_ip; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX posts_ip ON public.posts USING gist (ip inet_ops) WHERE (ip IS NOT NULL);

--
-- Name: posts_ip_old; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX posts_ip_old ON public.posts USING btree (created_at) WHERE (ip IS NOT NULL);

--
-- Name: posts_last_editor; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX posts_last_editor ON public.posts USING btree (last_editor_id) WHERE (last_editor_id IS NOT NULL);

--
-- Name: posts_real_author; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX posts_real_author ON public.posts USING btree (real_author_id, created_at) WHERE (real_author_id IS NOT NULL);

--
-- Name: posts_reply_to; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX posts_reply_to ON public.posts USING btree (reply_to_post_id) WHERE (reply_to_post_id IS NOT NULL);

--
-- Name: posts_search; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX posts_search ON public.posts USING gin (search_vector);

--
-- Name: profile_fields_plugin_key; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX profile_fields_plugin_key ON public.profile_fields USING btree (plugin, plugin_key) WHERE (plugin IS NOT NULL);

--
-- Name: profile_visits_visitor; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX profile_visits_visitor ON public.profile_visits USING btree (visitor_id);

--
-- Name: push_queue_by_endpoint; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX push_queue_by_endpoint ON public.push_queue USING btree (endpoint);

--
-- Name: push_queue_due; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX push_queue_due ON public.push_queue USING btree (next_at);

--
-- Name: push_subscriptions_by_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX push_subscriptions_by_user ON public.push_subscriptions USING btree (user_id);

--
-- Name: reactions_one_like; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX reactions_one_like ON public.reactions USING btree (is_like) WHERE is_like;

--
-- Name: reactions_plugin_key; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX reactions_plugin_key ON public.reactions USING btree (plugin, plugin_key) WHERE (plugin IS NOT NULL);

--
-- Name: read_markers_item; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX read_markers_item ON public.read_markers USING btree (item_id);

--
-- Name: report_reasons_plugin_key; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX report_reasons_plugin_key ON public.report_reasons USING btree (plugin, plugin_key) WHERE (plugin IS NOT NULL);

--
-- Name: reports_one_per_member; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX reports_one_per_member ON public.reports USING btree (target_type, target_id, reporter_id) WHERE (status = 'open'::text);

--
-- Name: reports_reporter; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX reports_reporter ON public.reports USING btree (reporter_id);

--
-- Name: reviewables_pending; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX reviewables_pending ON public.reviewables USING btree (status, created_at) WHERE (status = 'pending'::text);

--
-- Name: reviewables_target; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX reviewables_target ON public.reviewables USING btree (target_type, target_id);

--
-- Name: search_best_bets_item; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX search_best_bets_item ON public.search_best_bets USING btree (item_id);

--
-- Name: search_best_bets_query; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX search_best_bets_query ON public.search_best_bets USING btree (lower(query), item_id);

--
-- Name: search_logs_time; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX search_logs_time ON public.search_logs USING btree (created_at DESC);

--
-- Name: search_logs_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX search_logs_user ON public.search_logs USING btree (user_id) WHERE (user_id IS NOT NULL);

--
-- Name: search_synonyms_term; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX search_synonyms_term ON public.search_synonyms USING btree (lower(term));

--
-- Name: sessions_expires; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX sessions_expires ON public.sessions USING btree (expires_at);

--
-- Name: sessions_last_used; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX sessions_last_used ON public.sessions USING btree (last_used_at);

--
-- Name: sessions_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX sessions_user ON public.sessions USING btree (user_id);

--
-- Name: tags_name_trgm; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX tags_name_trgm ON public.tags USING gin (public.bf_unaccent(lower(name)) public.gin_trgm_ops);

--
-- Name: tags_plugin_key; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX tags_plugin_key ON public.tags USING btree (plugin, plugin_key) WHERE (plugin IS NOT NULL);

--
-- Name: topic_events_by_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX topic_events_by_date ON public.topic_events USING btree (on_date);

--
-- Name: topic_events_repeating; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX topic_events_repeating ON public.topic_events USING btree (on_date) WHERE (recurrence <> 'none'::text);

--
-- Name: topic_redirects_node; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX topic_redirects_node ON public.topic_redirects USING btree (node_id);

--
-- Name: topic_votes_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX topic_votes_user ON public.topic_votes USING btree (user_id);

--
-- Name: trusted_devices_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX trusted_devices_user ON public.trusted_devices USING btree (user_id);

--
-- Name: upload_quota_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX upload_quota_user ON public.upload_quota USING btree (user_id, created_at DESC);

--
-- Name: upload_uses_by_upload; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX upload_uses_by_upload ON public.upload_uses USING btree (upload_id);

--
-- Name: uploads_sha; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX uploads_sha ON public.uploads USING btree (sha256);

--
-- Name: uploads_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX uploads_user ON public.uploads USING btree (user_id, created_at DESC);

--
-- Name: user_follows_target; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX user_follows_target ON public.user_follows USING btree (target_id);

--
-- Name: user_identities_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX user_identities_user ON public.user_identities USING btree (user_id);

--
-- Name: user_notes_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX user_notes_user ON public.user_notes USING btree (user_id, created_at DESC);

--
-- Name: username_history_lower; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX username_history_lower ON public.username_history USING btree (old_lower);

--
-- Name: username_history_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX username_history_user ON public.username_history USING btree (user_id);

--
-- Name: users_delete_after; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX users_delete_after ON public.users USING btree (delete_after) WHERE (delete_after IS NOT NULL);

--
-- Name: users_newsletter_pending; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX users_newsletter_pending ON public.users USING btree (newsletter_changed_at) WHERE ((newsletter_changed_at IS NOT NULL) AND ((newsletter_synced_at IS NULL) OR (newsletter_synced_at < newsletter_changed_at)));

--
-- Name: users_pending; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX users_pending ON public.users USING btree (id) WHERE (NOT approved);

--
-- Name: users_referred_by; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX users_referred_by ON public.users USING btree (referred_by) WHERE (referred_by IS NOT NULL);

--
-- Name: users_remote; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX users_remote ON public.users USING btree (id) WHERE remote;

--
-- Name: users_username_skeleton; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX users_username_skeleton ON public.users USING btree (username_skeleton);

--
-- Name: warnings_by_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX warnings_by_user ON public.warnings USING btree (user_id, expires_at);

--
-- Name: webhook_deliveries_due; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX webhook_deliveries_due ON public.webhook_deliveries USING btree (next_at) WHERE (status = 'pending'::text);

--
-- Name: audit_log audit_log_no_delete; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER audit_log_no_delete BEFORE DELETE ON public.audit_log FOR EACH STATEMENT EXECUTE FUNCTION public.audit_log_append_only();

--
-- Name: audit_log audit_log_no_truncate; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER audit_log_no_truncate BEFORE TRUNCATE ON public.audit_log FOR EACH STATEMENT EXECUTE FUNCTION public.audit_log_append_only();

--
-- Name: audit_log audit_log_no_update; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER audit_log_no_update BEFORE UPDATE ON public.audit_log FOR EACH STATEMENT EXECUTE FUNCTION public.audit_log_append_only();

--
-- Name: chat_messages chat_messages_track_uploads; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER chat_messages_track_uploads AFTER INSERT OR DELETE OR UPDATE OF raw ON public.chat_messages FOR EACH ROW EXECUTE FUNCTION public.bf_track_uploads_trg('chat');

--
-- Name: conversation_messages conversation_messages_track_uploads; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER conversation_messages_track_uploads AFTER INSERT OR DELETE OR UPDATE OF raw ON public.conversation_messages FOR EACH ROW EXECUTE FUNCTION public.bf_track_uploads_trg('message');

--
-- Name: posts posts_clear_solved; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER posts_clear_solved AFTER UPDATE OF state, item_id, post_number ON public.posts FOR EACH ROW EXECUTE FUNCTION public.clear_stale_solved();

--
-- Name: posts posts_track_uploads; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER posts_track_uploads AFTER INSERT OR DELETE OR UPDATE OF raw ON public.posts FOR EACH ROW EXECUTE FUNCTION public.bf_track_uploads_trg('post');

--
-- Name: users users_email_changed; Type: TRIGGER; Schema: public; Owner: -
--

CREATE TRIGGER users_email_changed BEFORE UPDATE OF email ON public.users FOR EACH ROW WHEN ((old.email IS DISTINCT FROM new.email)) EXECUTE FUNCTION public.users_email_changed();

--
-- Name: ai_embeddings ai_embeddings_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_embeddings
    ADD CONSTRAINT ai_embeddings_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: ai_post_labels ai_post_labels_post_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_post_labels
    ADD CONSTRAINT ai_post_labels_post_id_fkey FOREIGN KEY (post_id) REFERENCES public.posts(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: ai_summaries ai_summaries_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_summaries
    ADD CONSTRAINT ai_summaries_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: ai_usage ai_usage_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ai_usage
    ADD CONSTRAINT ai_usage_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: ap_actors ap_actors_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_actors
    ADD CONSTRAINT ap_actors_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: ap_dm_objects ap_dm_objects_conversation_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_dm_objects
    ADD CONSTRAINT ap_dm_objects_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: ap_dm_objects ap_dm_objects_message_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_dm_objects
    ADD CONSTRAINT ap_dm_objects_message_id_fkey FOREIGN KEY (message_id) REFERENCES public.conversation_messages(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: ap_followers ap_followers_actor_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_followers
    ADD CONSTRAINT ap_followers_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.ap_actors(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: ap_mirrors ap_mirrors_actor_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_mirrors
    ADD CONSTRAINT ap_mirrors_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.ap_actors(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: ap_mirrors ap_mirrors_node_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_mirrors
    ADD CONSTRAINT ap_mirrors_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: ap_objects ap_objects_actor_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_objects
    ADD CONSTRAINT ap_objects_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.ap_actors(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: ap_objects ap_objects_post_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ap_objects
    ADD CONSTRAINT ap_objects_post_id_fkey FOREIGN KEY (post_id) REFERENCES public.posts(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: api_keys api_keys_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: backup_codes backup_codes_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.backup_codes
    ADD CONSTRAINT backup_codes_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: bans bans_created_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bans
    ADD CONSTRAINT bans_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: bookmarks bookmarks_post_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bookmarks
    ADD CONSTRAINT bookmarks_post_id_fkey FOREIGN KEY (post_id) REFERENCES public.posts(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: bookmarks bookmarks_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.bookmarks
    ADD CONSTRAINT bookmarks_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: chat_channel_groups chat_channel_groups_channel_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_channel_groups
    ADD CONSTRAINT chat_channel_groups_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.chat_channels(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: chat_channel_groups chat_channel_groups_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_channel_groups
    ADD CONSTRAINT chat_channel_groups_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.groups(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: chat_channels chat_channels_created_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_channels
    ADD CONSTRAINT chat_channels_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: chat_flags chat_flags_message_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_flags
    ADD CONSTRAINT chat_flags_message_id_fkey FOREIGN KEY (message_id) REFERENCES public.chat_messages(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: chat_flags chat_flags_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_flags
    ADD CONSTRAINT chat_flags_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: chat_integration_rules chat_integration_rules_node_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_integration_rules
    ADD CONSTRAINT chat_integration_rules_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: chat_members chat_members_channel_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_members
    ADD CONSTRAINT chat_members_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.chat_channels(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: chat_members chat_members_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_members
    ADD CONSTRAINT chat_members_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: chat_messages chat_messages_channel_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_messages
    ADD CONSTRAINT chat_messages_channel_id_fkey FOREIGN KEY (channel_id) REFERENCES public.chat_channels(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: chat_messages chat_messages_deleted_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_messages
    ADD CONSTRAINT chat_messages_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: chat_messages chat_messages_reply_to_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_messages
    ADD CONSTRAINT chat_messages_reply_to_id_fkey FOREIGN KEY (reply_to_id) REFERENCES public.chat_messages(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: chat_messages chat_messages_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_messages
    ADD CONSTRAINT chat_messages_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: chat_reactions chat_reactions_message_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_reactions
    ADD CONSTRAINT chat_reactions_message_id_fkey FOREIGN KEY (message_id) REFERENCES public.chat_messages(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: chat_reactions chat_reactions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.chat_reactions
    ADD CONSTRAINT chat_reactions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: club_bans club_bans_club_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.club_bans
    ADD CONSTRAINT club_bans_club_id_fkey FOREIGN KEY (club_id) REFERENCES public.clubs(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: club_bans club_bans_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.club_bans
    ADD CONSTRAINT club_bans_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: club_invites club_invites_club_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.club_invites
    ADD CONSTRAINT club_invites_club_id_fkey FOREIGN KEY (club_id) REFERENCES public.clubs(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: club_invites club_invites_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.club_invites
    ADD CONSTRAINT club_invites_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: clubs clubs_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.clubs
    ADD CONSTRAINT clubs_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.groups(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: clubs clubs_node_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.clubs
    ADD CONSTRAINT clubs_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: clubs clubs_owner_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.clubs
    ADD CONSTRAINT clubs_owner_id_fkey FOREIGN KEY (owner_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: content_translations content_translations_translator_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.content_translations
    ADD CONSTRAINT content_translations_translator_id_fkey FOREIGN KEY (translator_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: conversation_messages conversation_messages_author_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_messages
    ADD CONSTRAINT conversation_messages_author_id_fkey FOREIGN KEY (author_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: conversation_messages conversation_messages_conversation_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_messages
    ADD CONSTRAINT conversation_messages_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: conversation_participants conversation_participants_conversation_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_participants
    ADD CONSTRAINT conversation_participants_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: conversation_participants conversation_participants_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_participants
    ADD CONSTRAINT conversation_participants_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: conversation_reports conversation_reports_conversation_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_reports
    ADD CONSTRAINT conversation_reports_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: conversation_reports conversation_reports_reporter_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_reports
    ADD CONSTRAINT conversation_reports_reporter_id_fkey FOREIGN KEY (reporter_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: conversation_reports conversation_reports_resolved_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversation_reports
    ADD CONSTRAINT conversation_reports_resolved_by_fkey FOREIGN KEY (resolved_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: conversations conversations_starter_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.conversations
    ADD CONSTRAINT conversations_starter_id_fkey FOREIGN KEY (starter_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: drafts drafts_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.drafts
    ADD CONSTRAINT drafts_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: email_tokens email_tokens_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.email_tokens
    ADD CONSTRAINT email_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: embed_topics embed_topics_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.embed_topics
    ADD CONSTRAINT embed_topics_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: event_rsvps event_rsvps_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.event_rsvps
    ADD CONSTRAINT event_rsvps_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.topic_events(item_id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: event_rsvps event_rsvps_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.event_rsvps
    ADD CONSTRAINT event_rsvps_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: explorer_queries explorer_queries_created_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.explorer_queries
    ADD CONSTRAINT explorer_queries_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: failed_logins failed_logins_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.failed_logins
    ADD CONSTRAINT failed_logins_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: follows follows_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.follows
    ADD CONSTRAINT follows_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: group_requests group_requests_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.group_requests
    ADD CONSTRAINT group_requests_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.groups(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: group_requests group_requests_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.group_requests
    ADD CONSTRAINT group_requests_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: groups groups_plugin_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.groups
    ADD CONSTRAINT groups_plugin_fkey FOREIGN KEY (plugin) REFERENCES public.plugins(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: held_content held_content_node_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.held_content
    ADD CONSTRAINT held_content_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: held_content held_content_resolved_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.held_content
    ADD CONSTRAINT held_content_resolved_by_fkey FOREIGN KEY (resolved_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: held_content held_content_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.held_content
    ADD CONSTRAINT held_content_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: ignored_users ignored_users_ignored_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ignored_users
    ADD CONSTRAINT ignored_users_ignored_user_id_fkey FOREIGN KEY (ignored_user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: ignored_users ignored_users_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.ignored_users
    ADD CONSTRAINT ignored_users_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: invites invites_created_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invites
    ADD CONSTRAINT invites_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: invites invites_used_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.invites
    ADD CONSTRAINT invites_used_by_fkey FOREIGN KEY (used_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: item_tags item_tags_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.item_tags
    ADD CONSTRAINT item_tags_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: item_tags item_tags_tag_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.item_tags
    ADD CONSTRAINT item_tags_tag_id_fkey FOREIGN KEY (tag_id) REFERENCES public.tags(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: item_timers item_timers_created_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.item_timers
    ADD CONSTRAINT item_timers_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: item_timers item_timers_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.item_timers
    ADD CONSTRAINT item_timers_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: items items_assigned_to_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.items
    ADD CONSTRAINT items_assigned_to_fkey FOREIGN KEY (assigned_to) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: items items_author_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.items
    ADD CONSTRAINT items_author_id_fkey FOREIGN KEY (author_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: items items_deleted_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.items
    ADD CONSTRAINT items_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: items items_last_poster_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.items
    ADD CONSTRAINT items_last_poster_id_fkey FOREIGN KEY (last_poster_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: items items_node_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.items
    ADD CONSTRAINT items_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id) DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: legal_notices legal_notices_decided_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.legal_notices
    ADD CONSTRAINT legal_notices_decided_by_fkey FOREIGN KEY (decided_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: legal_notices legal_notices_post_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.legal_notices
    ADD CONSTRAINT legal_notices_post_id_fkey FOREIGN KEY (post_id) REFERENCES public.posts(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: legal_notices legal_notices_reporter_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.legal_notices
    ADD CONSTRAINT legal_notices_reporter_id_fkey FOREIGN KEY (reporter_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: live_question_votes live_question_votes_question_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.live_question_votes
    ADD CONSTRAINT live_question_votes_question_id_fkey FOREIGN KEY (question_id) REFERENCES public.live_questions(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: live_question_votes live_question_votes_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.live_question_votes
    ADD CONSTRAINT live_question_votes_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: live_questions live_questions_author_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.live_questions
    ADD CONSTRAINT live_questions_author_id_fkey FOREIGN KEY (author_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: live_questions live_questions_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.live_questions
    ADD CONSTRAINT live_questions_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.live_sessions(item_id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: live_sessions live_sessions_created_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.live_sessions
    ADD CONSTRAINT live_sessions_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: live_sessions live_sessions_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.live_sessions
    ADD CONSTRAINT live_sessions_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: mass_messages mass_messages_sender_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.mass_messages
    ADD CONSTRAINT mass_messages_sender_id_fkey FOREIGN KEY (sender_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: member_reports member_reports_reporter_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.member_reports
    ADD CONSTRAINT member_reports_reporter_id_fkey FOREIGN KEY (reporter_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: member_reports member_reports_resolved_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.member_reports
    ADD CONSTRAINT member_reports_resolved_by_fkey FOREIGN KEY (resolved_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: member_reports member_reports_status_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.member_reports
    ADD CONSTRAINT member_reports_status_id_fkey FOREIGN KEY (status_id) REFERENCES public.member_statuses(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: member_reports member_reports_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.member_reports
    ADD CONSTRAINT member_reports_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: member_status_replies member_status_replies_author_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.member_status_replies
    ADD CONSTRAINT member_status_replies_author_id_fkey FOREIGN KEY (author_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: member_status_replies member_status_replies_status_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.member_status_replies
    ADD CONSTRAINT member_status_replies_status_id_fkey FOREIGN KEY (status_id) REFERENCES public.member_statuses(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: member_statuses member_statuses_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.member_statuses
    ADD CONSTRAINT member_statuses_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: moderation_statements moderation_statements_appeal_decided_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.moderation_statements
    ADD CONSTRAINT moderation_statements_appeal_decided_by_fkey FOREIGN KEY (appeal_decided_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: moderation_statements moderation_statements_moderator_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.moderation_statements
    ADD CONSTRAINT moderation_statements_moderator_id_fkey FOREIGN KEY (moderator_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: moderation_statements moderation_statements_notice_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.moderation_statements
    ADD CONSTRAINT moderation_statements_notice_id_fkey FOREIGN KEY (notice_id) REFERENCES public.legal_notices(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: moderation_statements moderation_statements_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.moderation_statements
    ADD CONSTRAINT moderation_statements_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: muted_tags muted_tags_tag_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.muted_tags
    ADD CONSTRAINT muted_tags_tag_id_fkey FOREIGN KEY (tag_id) REFERENCES public.tags(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: muted_tags muted_tags_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.muted_tags
    ADD CONSTRAINT muted_tags_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: node_moderators node_moderators_node_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.node_moderators
    ADD CONSTRAINT node_moderators_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: node_moderators node_moderators_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.node_moderators
    ADD CONSTRAINT node_moderators_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: node_permissions node_permissions_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.node_permissions
    ADD CONSTRAINT node_permissions_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.groups(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: node_permissions node_permissions_node_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.node_permissions
    ADD CONSTRAINT node_permissions_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: node_read_markers node_read_markers_node_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.node_read_markers
    ADD CONSTRAINT node_read_markers_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: node_read_markers node_read_markers_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.node_read_markers
    ADD CONSTRAINT node_read_markers_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: nodes nodes_parent_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.nodes
    ADD CONSTRAINT nodes_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES public.nodes(id) ON DELETE RESTRICT DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: nodes nodes_theme_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.nodes
    ADD CONSTRAINT nodes_theme_id_fkey FOREIGN KEY (theme_id) REFERENCES public.themes(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: notification_prefs notification_prefs_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notification_prefs
    ADD CONSTRAINT notification_prefs_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: notifications notifications_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.notifications
    ADD CONSTRAINT notifications_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: oauth_codes oauth_codes_client_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_codes
    ADD CONSTRAINT oauth_codes_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.oauth_clients(client_id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: oauth_codes oauth_codes_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.oauth_codes
    ADD CONSTRAINT oauth_codes_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: passkeys passkeys_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.passkeys
    ADD CONSTRAINT passkeys_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: pending_logins pending_logins_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.pending_logins
    ADD CONSTRAINT pending_logins_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: plugin_api_scopes plugin_api_scopes_plugin_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_api_scopes
    ADD CONSTRAINT plugin_api_scopes_plugin_fkey FOREIGN KEY (plugin) REFERENCES public.plugins(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: plugin_files plugin_files_plugin_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_files
    ADD CONSTRAINT plugin_files_plugin_fkey FOREIGN KEY (plugin) REFERENCES public.plugins(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: plugin_jobs plugin_jobs_plugin_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_jobs
    ADD CONSTRAINT plugin_jobs_plugin_fkey FOREIGN KEY (plugin) REFERENCES public.plugins(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: plugin_kv plugin_kv_plugin_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_kv
    ADD CONSTRAINT plugin_kv_plugin_fkey FOREIGN KEY (plugin) REFERENCES public.plugins(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: plugin_meta plugin_meta_plugin_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_meta
    ADD CONSTRAINT plugin_meta_plugin_fkey FOREIGN KEY (plugin) REFERENCES public.plugins(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: plugin_permission_grants plugin_permission_grants_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_permission_grants
    ADD CONSTRAINT plugin_permission_grants_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.groups(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: plugin_permission_grants plugin_permission_grants_plugin_perm_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_permission_grants
    ADD CONSTRAINT plugin_permission_grants_plugin_perm_fkey FOREIGN KEY (plugin, perm) REFERENCES public.plugin_permissions(plugin, key) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: plugin_permissions plugin_permissions_plugin_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_permissions
    ADD CONSTRAINT plugin_permissions_plugin_fkey FOREIGN KEY (plugin) REFERENCES public.plugins(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: plugin_review_items plugin_review_items_decided_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_review_items
    ADD CONSTRAINT plugin_review_items_decided_by_fkey FOREIGN KEY (decided_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: plugin_review_items plugin_review_items_plugin_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_review_items
    ADD CONSTRAINT plugin_review_items_plugin_fkey FOREIGN KEY (plugin) REFERENCES public.plugins(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: plugin_review_items plugin_review_items_submitted_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_review_items
    ADD CONSTRAINT plugin_review_items_submitted_by_fkey FOREIGN KEY (submitted_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: plugin_schedules plugin_schedules_plugin_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_schedules
    ADD CONSTRAINT plugin_schedules_plugin_fkey FOREIGN KEY (plugin) REFERENCES public.plugins(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: plugin_search_docs plugin_search_docs_plugin_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugin_search_docs
    ADD CONSTRAINT plugin_search_docs_plugin_fkey FOREIGN KEY (plugin) REFERENCES public.plugins(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: plugins plugins_bot_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.plugins
    ADD CONSTRAINT plugins_bot_user_id_fkey FOREIGN KEY (bot_user_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: poll_groups poll_groups_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_groups
    ADD CONSTRAINT poll_groups_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.groups(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: poll_groups poll_groups_poll_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_groups
    ADD CONSTRAINT poll_groups_poll_id_fkey FOREIGN KEY (poll_id) REFERENCES public.polls(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: poll_options poll_options_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_options
    ADD CONSTRAINT poll_options_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: poll_options poll_options_poll_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_options
    ADD CONSTRAINT poll_options_poll_id_fkey FOREIGN KEY (poll_id) REFERENCES public.polls(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: poll_votes poll_votes_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_votes
    ADD CONSTRAINT poll_votes_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: poll_votes poll_votes_option_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_votes
    ADD CONSTRAINT poll_votes_option_id_fkey FOREIGN KEY (option_id) REFERENCES public.poll_options(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: poll_votes poll_votes_poll_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_votes
    ADD CONSTRAINT poll_votes_poll_id_fkey FOREIGN KEY (poll_id) REFERENCES public.polls(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: poll_votes poll_votes_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.poll_votes
    ADD CONSTRAINT poll_votes_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: polls polls_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.polls
    ADD CONSTRAINT polls_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: post_likes post_likes_post_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_likes
    ADD CONSTRAINT post_likes_post_id_fkey FOREIGN KEY (post_id) REFERENCES public.posts(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: post_likes post_likes_reaction_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_likes
    ADD CONSTRAINT post_likes_reaction_id_fkey FOREIGN KEY (reaction_id) REFERENCES public.reactions(id) DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: post_likes post_likes_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_likes
    ADD CONSTRAINT post_likes_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: post_links post_links_post_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_links
    ADD CONSTRAINT post_links_post_id_fkey FOREIGN KEY (post_id) REFERENCES public.posts(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: post_notes post_notes_author_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_notes
    ADD CONSTRAINT post_notes_author_id_fkey FOREIGN KEY (author_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: post_notes post_notes_post_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_notes
    ADD CONSTRAINT post_notes_post_id_fkey FOREIGN KEY (post_id) REFERENCES public.posts(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: post_revisions post_revisions_editor_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_revisions
    ADD CONSTRAINT post_revisions_editor_id_fkey FOREIGN KEY (editor_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: post_revisions post_revisions_post_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_revisions
    ADD CONSTRAINT post_revisions_post_id_fkey FOREIGN KEY (post_id) REFERENCES public.posts(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: post_votes post_votes_post_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_votes
    ADD CONSTRAINT post_votes_post_id_fkey FOREIGN KEY (post_id) REFERENCES public.posts(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: post_votes post_votes_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.post_votes
    ADD CONSTRAINT post_votes_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: posts posts_author_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.posts
    ADD CONSTRAINT posts_author_id_fkey FOREIGN KEY (author_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: posts posts_deleted_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.posts
    ADD CONSTRAINT posts_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: posts posts_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.posts
    ADD CONSTRAINT posts_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: posts posts_last_editor_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.posts
    ADD CONSTRAINT posts_last_editor_id_fkey FOREIGN KEY (last_editor_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: posts posts_real_author_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.posts
    ADD CONSTRAINT posts_real_author_id_fkey FOREIGN KEY (real_author_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: posts posts_reply_to_post_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.posts
    ADD CONSTRAINT posts_reply_to_post_id_fkey FOREIGN KEY (reply_to_post_id) REFERENCES public.posts(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: profile_fields profile_fields_plugin_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.profile_fields
    ADD CONSTRAINT profile_fields_plugin_fkey FOREIGN KEY (plugin) REFERENCES public.plugins(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: profile_visits profile_visits_profile_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.profile_visits
    ADD CONSTRAINT profile_visits_profile_id_fkey FOREIGN KEY (profile_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: profile_visits profile_visits_visitor_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.profile_visits
    ADD CONSTRAINT profile_visits_visitor_id_fkey FOREIGN KEY (visitor_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: promotion_rules promotion_rules_add_group_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.promotion_rules
    ADD CONSTRAINT promotion_rules_add_group_fkey FOREIGN KEY (add_group) REFERENCES public.groups(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: push_queue push_queue_endpoint_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.push_queue
    ADD CONSTRAINT push_queue_endpoint_fkey FOREIGN KEY (endpoint) REFERENCES public.push_subscriptions(endpoint) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: push_queue push_queue_notification_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.push_queue
    ADD CONSTRAINT push_queue_notification_id_fkey FOREIGN KEY (notification_id) REFERENCES public.notifications(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: push_subscriptions push_subscriptions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.push_subscriptions
    ADD CONSTRAINT push_subscriptions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: reactions reactions_plugin_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reactions
    ADD CONSTRAINT reactions_plugin_fkey FOREIGN KEY (plugin) REFERENCES public.plugins(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: read_markers read_markers_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.read_markers
    ADD CONSTRAINT read_markers_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: read_markers read_markers_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.read_markers
    ADD CONSTRAINT read_markers_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: referral_rewards referral_rewards_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.referral_rewards
    ADD CONSTRAINT referral_rewards_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.groups(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: referral_rewards referral_rewards_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.referral_rewards
    ADD CONSTRAINT referral_rewards_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: report_reasons report_reasons_plugin_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.report_reasons
    ADD CONSTRAINT report_reasons_plugin_fkey FOREIGN KEY (plugin) REFERENCES public.plugins(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: reports reports_reporter_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reports
    ADD CONSTRAINT reports_reporter_id_fkey FOREIGN KEY (reporter_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: reviewables reviewables_claimed_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reviewables
    ADD CONSTRAINT reviewables_claimed_by_fkey FOREIGN KEY (claimed_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: reviewables reviewables_node_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reviewables
    ADD CONSTRAINT reviewables_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: reviewables reviewables_resolved_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.reviewables
    ADD CONSTRAINT reviewables_resolved_by_fkey FOREIGN KEY (resolved_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: search_best_bets search_best_bets_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.search_best_bets
    ADD CONSTRAINT search_best_bets_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: search_logs search_logs_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.search_logs
    ADD CONSTRAINT search_logs_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: sessions sessions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: tag_group_members tag_group_members_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tag_group_members
    ADD CONSTRAINT tag_group_members_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.tag_groups(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: tag_group_members tag_group_members_tag_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tag_group_members
    ADD CONSTRAINT tag_group_members_tag_id_fkey FOREIGN KEY (tag_id) REFERENCES public.tags(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: tag_group_nodes tag_group_nodes_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tag_group_nodes
    ADD CONSTRAINT tag_group_nodes_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.tag_groups(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: tag_group_nodes tag_group_nodes_node_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tag_group_nodes
    ADD CONSTRAINT tag_group_nodes_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: tags tags_plugin_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.tags
    ADD CONSTRAINT tags_plugin_fkey FOREIGN KEY (plugin) REFERENCES public.plugins(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: topic_events topic_events_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.topic_events
    ADD CONSTRAINT topic_events_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: topic_redirects topic_redirects_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.topic_redirects
    ADD CONSTRAINT topic_redirects_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: topic_redirects topic_redirects_node_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.topic_redirects
    ADD CONSTRAINT topic_redirects_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.nodes(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: topic_votes topic_votes_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.topic_votes
    ADD CONSTRAINT topic_votes_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: topic_votes topic_votes_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.topic_votes
    ADD CONSTRAINT topic_votes_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: trusted_devices trusted_devices_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.trusted_devices
    ADD CONSTRAINT trusted_devices_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: unsubscribe_tokens unsubscribe_tokens_conversation_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.unsubscribe_tokens
    ADD CONSTRAINT unsubscribe_tokens_conversation_id_fkey FOREIGN KEY (conversation_id) REFERENCES public.conversations(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: unsubscribe_tokens unsubscribe_tokens_item_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.unsubscribe_tokens
    ADD CONSTRAINT unsubscribe_tokens_item_id_fkey FOREIGN KEY (item_id) REFERENCES public.items(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: unsubscribe_tokens unsubscribe_tokens_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.unsubscribe_tokens
    ADD CONSTRAINT unsubscribe_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: upload_quota upload_quota_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.upload_quota
    ADD CONSTRAINT upload_quota_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: upload_uses upload_uses_upload_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.upload_uses
    ADD CONSTRAINT upload_uses_upload_id_fkey FOREIGN KEY (upload_id) REFERENCES public.uploads(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: uploads uploads_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.uploads
    ADD CONSTRAINT uploads_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: user_badges user_badges_badge_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_badges
    ADD CONSTRAINT user_badges_badge_id_fkey FOREIGN KEY (badge_id) REFERENCES public.badges(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: user_badges user_badges_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_badges
    ADD CONSTRAINT user_badges_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: user_field_values user_field_values_field_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_field_values
    ADD CONSTRAINT user_field_values_field_id_fkey FOREIGN KEY (field_id) REFERENCES public.profile_fields(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: user_field_values user_field_values_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_field_values
    ADD CONSTRAINT user_field_values_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: user_follows user_follows_target_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_follows
    ADD CONSTRAINT user_follows_target_id_fkey FOREIGN KEY (target_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: user_follows user_follows_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_follows
    ADD CONSTRAINT user_follows_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: user_identities user_identities_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_identities
    ADD CONSTRAINT user_identities_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: user_legal_acceptances user_legal_acceptances_document_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_legal_acceptances
    ADD CONSTRAINT user_legal_acceptances_document_id_fkey FOREIGN KEY (document_id) REFERENCES public.legal_documents(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: user_legal_acceptances user_legal_acceptances_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_legal_acceptances
    ADD CONSTRAINT user_legal_acceptances_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: user_notes user_notes_author_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_notes
    ADD CONSTRAINT user_notes_author_id_fkey FOREIGN KEY (author_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: user_notes user_notes_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_notes
    ADD CONSTRAINT user_notes_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: user_secondary_groups user_secondary_groups_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_secondary_groups
    ADD CONSTRAINT user_secondary_groups_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.groups(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: user_secondary_groups user_secondary_groups_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_secondary_groups
    ADD CONSTRAINT user_secondary_groups_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: user_visits user_visits_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.user_visits
    ADD CONSTRAINT user_visits_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: username_history username_history_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.username_history
    ADD CONSTRAINT username_history_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: users users_avatar_upload_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_avatar_upload_id_fkey FOREIGN KEY (avatar_upload_id) REFERENCES public.uploads(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: users users_cover_upload_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_cover_upload_id_fkey FOREIGN KEY (cover_upload_id) REFERENCES public.uploads(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: users users_primary_group_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_primary_group_id_fkey FOREIGN KEY (primary_group_id) REFERENCES public.groups(id) DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: users users_referred_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_referred_by_fkey FOREIGN KEY (referred_by) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: users users_theme_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_theme_id_fkey FOREIGN KEY (theme_id) REFERENCES public.themes(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: warnings warnings_moderator_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.warnings
    ADD CONSTRAINT warnings_moderator_id_fkey FOREIGN KEY (moderator_id) REFERENCES public.users(id) ON DELETE SET NULL DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: warnings warnings_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.warnings
    ADD CONSTRAINT warnings_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: webauthn_challenges webauthn_challenges_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webauthn_challenges
    ADD CONSTRAINT webauthn_challenges_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: webhook_deliveries webhook_deliveries_webhook_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY public.webhook_deliveries
    ADD CONSTRAINT webhook_deliveries_webhook_id_fkey FOREIGN KEY (webhook_id) REFERENCES public.webhooks(id) ON DELETE CASCADE DEFERRABLE INITIALLY IMMEDIATE;

--
-- Name: SCHEMA explorer; Type: ACL; Schema: -; Owner: -
--

--
-- Name: TABLE events; Type: ACL; Schema: explorer; Owner: -
--

--
-- Name: TABLE forums; Type: ACL; Schema: explorer; Owner: -
--

--
-- Name: TABLE groups; Type: ACL; Schema: explorer; Owner: -
--

--
-- Name: TABLE likes; Type: ACL; Schema: explorer; Owner: -
--

--
-- Name: TABLE poll_votes; Type: ACL; Schema: explorer; Owner: -
--

--
-- Name: TABLE polls; Type: ACL; Schema: explorer; Owner: -
--

--
-- Name: TABLE posts; Type: ACL; Schema: explorer; Owner: -
--

--
-- Name: TABLE reports; Type: ACL; Schema: explorer; Owner: -
--

--
-- Name: TABLE rsvps; Type: ACL; Schema: explorer; Owner: -
--

--
-- Name: TABLE tags; Type: ACL; Schema: explorer; Owner: -
--

--
-- Name: TABLE topic_tags; Type: ACL; Schema: explorer; Owner: -
--

--
-- Name: TABLE topics; Type: ACL; Schema: explorer; Owner: -
--

--
-- Name: TABLE user_badges; Type: ACL; Schema: explorer; Owner: -
--

--
-- Name: TABLE user_groups; Type: ACL; Schema: explorer; Owner: -
--

--
-- Name: TABLE users; Type: ACL; Schema: explorer; Owner: -
--

--
--


DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bf_explorer') THEN
        EXECUTE 'GRANT USAGE ON SCHEMA explorer TO bf_explorer';
        EXECUTE 'GRANT SELECT ON ALL TABLES IN SCHEMA explorer TO bf_explorer';
        -- The application switches to the role for each query.
        EXECUTE format('GRANT bf_explorer TO %I', current_user);
    END IF;
EXCEPTION
    WHEN insufficient_privilege THEN
        RAISE NOTICE 'bf_explorer not granted: the Data Explorer stays off';
END
$$;

-- Strip bf_explorer of the built-ins that run a SQL string or read the host filesystem.
DO $$
DECLARE i record;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bf_explorer') THEN
        RAISE NOTICE 'bf_explorer absent; explorer confinement skipped';
        RETURN;
    END IF;

    -- Built-ins that execute a SQL string argument (the query_to_xml / cursor_to_xml family) or
    -- read from the host filesystem. bf_explorer has no need for any of them.
    FOR i IN (
        SELECT format('%I.%I(%s)', n.nspname, p.proname, pg_get_function_identity_arguments(p.oid)) AS sig
        FROM pg_proc p
        JOIN pg_namespace n ON n.oid = p.pronamespace
        WHERE n.nspname = 'pg_catalog'
          AND p.proname IN (
              'query_to_xml', 'query_to_xmlschema', 'query_to_xml_and_xmlschema',
              'cursor_to_xml', 'cursor_to_xmlschema',
              'pg_read_file', 'pg_read_binary_file', 'pg_ls_dir', 'pg_stat_file',
              'lo_import', 'lo_export'
          )
    ) LOOP
        BEGIN
            EXECUTE format('REVOKE ALL ON FUNCTION %s FROM bf_explorer', i.sig);
        EXCEPTION WHEN OTHERS THEN
            -- Not granted, or not present in this version: nothing to revoke.
            NULL;
        END;
    END LOOP;
END
$$;
