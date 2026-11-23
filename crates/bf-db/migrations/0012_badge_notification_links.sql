-- Badge notifications linked to /u/{id}/badges, a page that does not exist. Point them at the member's
-- own badges tab, which is addressed by name.
UPDATE notifications n
   SET data = jsonb_set(n.data, '{url}', to_jsonb('/u/' || lower(u.username) || '?tab=badges'))
  FROM users u
 WHERE n.kind = 'badge' AND n.user_id = u.id AND n.data->>'url' ~ '^/u/[0-9]+/badges$';
