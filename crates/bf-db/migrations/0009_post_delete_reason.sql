-- Why a post was deleted, so a restore can tell an ordinary deletion from spam removal, a review
-- rejection or an erased account. NULL (every existing row) is an ordinary deletion.
ALTER TABLE public.posts ADD COLUMN deleted_reason text;
ALTER TABLE public.posts ADD CONSTRAINT posts_deleted_reason_check
    CHECK (deleted_reason IS NULL OR deleted_reason IN ('spam', 'account_erased', 'rejected'));
