-- V28__add_comment_anchor.sql
--
-- Anchored comments: a comment may point at one node inside its bookmark.
--
-- * `query`        tree-sitter query relative to the bookmark's deepest
--                  landmark (the innermost function, method, class... around
--                  the bookmark's target). NULL = a comment on the whole
--                  bookmark, which is every comment written before V28.
-- * `content_hash` hash of the anchored node's text at creation; breaks ties
--                  when the query matches more than once inside the landmark.
-- * `anchor_hint`  line range and first line at creation, shown when the
--                  anchor no longer resolves.
--
-- Replies keep all three NULL: they inherit their root comment's anchor.
ALTER TABLE bookmark_comments ADD COLUMN query TEXT;
ALTER TABLE bookmark_comments ADD COLUMN content_hash TEXT;
ALTER TABLE bookmark_comments ADD COLUMN anchor_hint TEXT;
