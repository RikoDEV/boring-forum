-- A cached AI summary or gist is built from the posts that were visible then. When one of them is
-- hidden, deleted, restored, edited or removed, the cached text may still quote it, so it goes.
CREATE FUNCTION public.ai_summaries_invalidate() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        DELETE FROM ai_summaries WHERE item_id = OLD.item_id;
    ELSE
        DELETE FROM ai_summaries WHERE item_id = NEW.item_id;
    END IF;
    RETURN NULL;
END;
$$;

CREATE TRIGGER posts_invalidate_ai_summaries
    AFTER UPDATE OF state, raw ON public.posts
    FOR EACH ROW WHEN (OLD.state IS DISTINCT FROM NEW.state OR OLD.raw IS DISTINCT FROM NEW.raw)
    EXECUTE FUNCTION public.ai_summaries_invalidate();

CREATE TRIGGER posts_delete_invalidate_ai_summaries
    AFTER DELETE ON public.posts
    FOR EACH ROW EXECUTE FUNCTION public.ai_summaries_invalidate();
