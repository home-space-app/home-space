-- Names inside a protected folder are sealed: base64url text of their padded AES-SIV sealing,
-- up to 1387 characters for a name of 255 (protocol::rules::MAX_SEALED_NODE_NAME_CHARS). The
-- columns allow that much; the file services keep names outside protected folders to 255
-- characters, which a check here cannot, not knowing where a node is. The other rules stay.
ALTER TABLE nodes DROP CONSTRAINT nodes_name_check;
ALTER TABLE nodes ADD CONSTRAINT nodes_name_check CHECK (
    length(name) BETWEEN 1 AND 1387
    AND position('/' IN name) = 0
    AND position(E'\\' IN name) = 0
    AND name NOT IN ('.', '..')
);

-- A resumable upload holds the name of the file it makes, sealed too inside a protected folder.
ALTER TABLE uploads DROP CONSTRAINT uploads_name_check;
ALTER TABLE uploads ADD CONSTRAINT uploads_name_check CHECK (
    length(name) BETWEEN 1 AND 1387
    AND position('/' IN name) = 0
    AND position(E'\\' IN name) = 0
    AND name NOT IN ('.', '..')
);
