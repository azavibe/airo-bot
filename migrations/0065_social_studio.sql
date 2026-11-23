-- The marketing app is called "Social Studio" in the interface. Offices that
-- installed it under its old name get the new one; a name the owner chose
-- (anything else) is left alone.
UPDATE office_apps
SET title = 'Social Studio', updated_at = now()
WHERE slug = 'social-video-studio' AND title = 'AI Video & Social Studio';
