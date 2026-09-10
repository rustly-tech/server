-- Publish the first hosted Trial. The evaluation package is private; only its
-- content identifier belongs in PostgreSQL.
INSERT INTO trials (
    id, slug, title, difficulty, topics, lifecycle, version, content_cid, published_at
) VALUES (
    '01992b53-2238-7000-8000-000000000001',
    'ownership-move-or-borrow',
    'Move or Borrow?',
    'easy',
    ARRAY['ownership', 'borrowing'],
    'official',
    2,
    'b3:25d53043ef9403604ed11f508820065f875446d4eafdba694c2d00826025621c',
    now()
)
ON CONFLICT (slug) DO UPDATE SET
    title = EXCLUDED.title,
    difficulty = EXCLUDED.difficulty,
    topics = EXCLUDED.topics,
    lifecycle = EXCLUDED.lifecycle,
    version = EXCLUDED.version,
    content_cid = EXCLUDED.content_cid;
