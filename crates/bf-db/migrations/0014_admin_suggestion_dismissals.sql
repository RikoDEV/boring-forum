-- Suggestions an administrator chose not to follow. The suggestions themselves are computed.
CREATE TABLE admin_suggestion_dismissals (
    id text PRIMARY KEY,
    dismissed_at timestamp with time zone NOT NULL DEFAULT now()
);
