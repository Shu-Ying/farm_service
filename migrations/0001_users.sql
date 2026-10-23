-- Platform separates identities; the API currently supports ordinary QQ numbers.
CREATE TABLE users (
    platform TEXT NOT NULL,
    platform_user_id TEXT NOT NULL,
    nickname TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (platform, platform_user_id),
    CHECK (platform IN ('qq', 'telegram', 'discord')),
    CHECK (length(platform_user_id) BETWEEN 1 AND 32),
    CHECK (length(nickname) BETWEEN 1 AND 64)
);
