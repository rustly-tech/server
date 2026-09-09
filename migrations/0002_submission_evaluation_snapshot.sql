-- A submission must keep the exact evaluation package it was created for.
-- Joining the mutable Trial catalogue while leasing could otherwise judge an
-- already queued submission against a newer package.
ALTER TABLE submissions ADD COLUMN trial_package_cid TEXT;

UPDATE submissions AS s
SET trial_package_cid = t.content_cid
FROM trials AS t
WHERE s.trial_id = t.id;

ALTER TABLE submissions ALTER COLUMN trial_package_cid SET NOT NULL;
