-- V15: Preserve the complete artifact projection shared by the domain and
-- public storage ports. Existing rows were written by the domain store, whose
-- only supported digest algorithm is BLAKE3 and whose historical projection
-- silently defaulted classification to public on read.

ALTER TABLE artifacts
    ADD COLUMN algorithm TEXT NOT NULL DEFAULT 'blake3';

ALTER TABLE artifacts
    ADD COLUMN classification TEXT NOT NULL DEFAULT 'public';
