-- The Apps screen's "Launch" buttons used to run full office setup with the
-- app's label as the business type, so an office could believe it is a
-- "Lead Generation & CRM" business. Those labels are not businesses: clear
-- them, and bots work from the office's name instead.
UPDATE workspaces
SET business_type = NULL, updated_at = now()
WHERE business_type IN ('Lead Generation & CRM', 'Video & Media Production', 'Finance & Invoicing');
