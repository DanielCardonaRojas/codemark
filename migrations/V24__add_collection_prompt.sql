-- V24__add_collection_prompt.sql
--
-- Records the (agent-polished) prompt that motivated a collection, so a
-- collection whose bookmarks have all drifted beyond repair can be recreated
-- from the original intent rather than reverse-engineered from its steps.
ALTER TABLE collections ADD COLUMN prompt TEXT;
