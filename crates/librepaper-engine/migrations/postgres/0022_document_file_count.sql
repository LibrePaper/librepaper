-- How many files the project's directory holds, as the Files panel shows them.
-- The sequencer stamps it on the document row when it flushes and when it
-- builds the document, so the listing reads it without projecting. NULL until
-- a project has been opened or written since this column existed.
ALTER TABLE documents ADD COLUMN file_count integer CHECK (file_count >= 0);
