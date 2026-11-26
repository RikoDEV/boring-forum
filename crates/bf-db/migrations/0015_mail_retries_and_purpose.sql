-- What a queued mail is for, where something must count them (the contact form's hourly cap).
ALTER TABLE mail_outbox ADD COLUMN purpose text;
-- Mail used to give up after five tries. It now gets nine over two days, and what had already
-- given up stays given up: its links have long expired.
UPDATE mail_outbox SET attempts = 9 WHERE sent_at IS NULL AND attempts >= 5;
