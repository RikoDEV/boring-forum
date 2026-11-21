-- The picture URL a provider last gave for a login, so an avatar is fetched again only when it changes.
ALTER TABLE user_identities ADD COLUMN avatar_url text;
