-- Derived-state and data-integrity fixes: markers written by "mark forum read", topics a moderator
-- reopened, files that lost their last use, and the AI summary cache when posts change topic.

-- "Mark forum read" writes a marker for every topic active in 90 days. Those rows are not reading,
-- so they must not count as "topics read" for trust levels and promotion. A row flips to a real
-- one when a member's reading moves it on; writes that carry the `bf.bulk_read` setting (mark
-- forum read, merging topics) keep the flag as it is.
ALTER TABLE public.read_markers ADD COLUMN bulk boolean DEFAULT false NOT NULL;

CREATE FUNCTION public.read_markers_real_read() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF OLD.bulk AND NEW.bulk AND NEW.last_read_post_number > OLD.last_read_post_number
       AND coalesce(current_setting('bf.bulk_read', true), '') <> 'on' THEN
        NEW.bulk := false;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER read_markers_real_read
    BEFORE UPDATE OF last_read_post_number ON public.read_markers
    FOR EACH ROW EXECUTE FUNCTION public.read_markers_real_read();

-- A topic a moderator (or a timer) reopened is not closed again by the inactivity job.
ALTER TABLE public.items ADD COLUMN auto_close_exempt boolean DEFAULT false NOT NULL;

CREATE FUNCTION public.items_unlock_exempts_auto_close() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    NEW.auto_close_exempt := true;
    RETURN NEW;
END;
$$;

CREATE TRIGGER items_unlock_exempts_auto_close
    BEFORE UPDATE OF locked ON public.items
    FOR EACH ROW WHEN (OLD.locked AND NOT NEW.locked)
    EXECUTE FUNCTION public.items_unlock_exempts_auto_close();

-- A file that a post, message or chat used once and none uses now (its post was deleted, its author
-- erased) is no longer "used nowhere, open to everyone" until the orphan sweep removes it.
ALTER TABLE public.uploads ADD COLUMN had_use boolean DEFAULT false NOT NULL;

CREATE FUNCTION public.upload_uses_mark_had_use() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    UPDATE uploads SET had_use = true WHERE id = NEW.upload_id AND NOT had_use;
    RETURN NULL;
END;
$$;

CREATE TRIGGER upload_uses_mark_had_use
    AFTER INSERT ON public.upload_uses
    FOR EACH ROW EXECUTE FUNCTION public.upload_uses_mark_had_use();

UPDATE public.uploads u SET had_use = true WHERE EXISTS (SELECT 1 FROM public.upload_uses x WHERE x.upload_id = u.id);

-- The cached AI summary also goes when a post moves to or from a topic (merge, split).
CREATE OR REPLACE FUNCTION public.ai_summaries_invalidate() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM ai_summaries WHERE item_id = OLD.item_id;
    ELSE
        DELETE FROM ai_summaries WHERE item_id = NEW.item_id OR item_id = OLD.item_id;
    END IF;
    RETURN NULL;
END;
$$;

DROP TRIGGER posts_invalidate_ai_summaries ON public.posts;
CREATE TRIGGER posts_invalidate_ai_summaries
    AFTER UPDATE OF state, raw, item_id ON public.posts
    FOR EACH ROW
    WHEN (OLD.state IS DISTINCT FROM NEW.state OR OLD.raw IS DISTINCT FROM NEW.raw OR OLD.item_id IS DISTINCT FROM NEW.item_id)
    EXECUTE FUNCTION public.ai_summaries_invalidate();
