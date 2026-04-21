/// Phase 1: pure-SQLite vector store.
/// Embeddings stored as JSON float arrays; cosine similarity computed in Rust.
/// Phase 2: swap for LanceDB + candle/MiniLM-L6 once protoc is available.
use anyhow::Result;
use sqlx::{sqlite::SqlitePool, Row};
use uuid::Uuid;

const EMBEDDING_DIM: usize = 128;

/// Number of independent hash buckets written per token.
/// Reduces collision rate without increasing dimension size.
const PROJECTIONS_PER_TOKEN: usize = 3;

pub struct VectorStore {
    pool: SqlitePool,
}

impl VectorStore {
    pub async fn new(pool: SqlitePool) -> Result<Self> {
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS embeddings ( \
                node_id TEXT PRIMARY KEY, \
                vector  TEXT NOT NULL \
            );",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn upsert(&self, node_id: Uuid, content: &str) -> Result<()> {
        let vec = embed(content);
        let json = serde_json::to_string(&vec)?;
        sqlx::query(
            "INSERT INTO embeddings (node_id, vector) VALUES (?, ?) \
             ON CONFLICT(node_id) DO UPDATE SET vector = excluded.vector",
        )
        .bind(node_id.to_string())
        .bind(&json)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete(&self, node_id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM embeddings WHERE node_id = ?")
            .bind(node_id.to_string())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Cosine similarity search. Returns `(node_id, distance)` ascending by distance.
    /// `distance = 1.0 - cosine_similarity`, so `0.0 = identical`.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<(Uuid, f32)>> {
        let query_vec = embed(query);

        let rows = sqlx::query("SELECT node_id, vector FROM embeddings")
            .fetch_all(&self.pool)
            .await?;

        let mut scored: Vec<(Uuid, f32)> = rows
            .into_iter()
            .filter_map(|r| {
                let id_str: String = r.get("node_id");
                let vec_json: String = r.get("vector");
                let id = Uuid::parse_str(&id_str).ok()?;
                let vec: Vec<f32> = serde_json::from_str(&vec_json).ok()?;
                let dist = cosine_distance(&query_vec, &vec);
                Some((id, dist))
            })
            .collect();

        scored.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        scored.truncate(limit);
        Ok(scored)
    }
}

// ── Embedding ─────────────────────────────────────────────────────────────────

/// Project `content` into a 128-dim L2-normalized f32 vector.
///
/// Algorithm:
///   1. Normalise: lowercase, strip punctuation, filter stop words
///   2. Unigrams: PROJECTIONS_PER_TOKEN independent FNV1a salts per token,
///      position-weighted contribution (earlier words matter more)
///   3. Bigrams: pairs of adjacent tokens contribute at 0.6× weight
///   4. L2-normalise the result
///
/// Deterministic and collision-resistant at Phase 1 scale. Phase 2 swaps this
/// for candle + MiniLM-L6-v2 inference with no API changes.
pub fn embed(content: &str) -> Vec<f32> {
    let mut vec = vec![0.0f32; EMBEDDING_DIM];
    let tokens = tokenize(content);

    for (i, token) in tokens.iter().enumerate() {
        let pos_weight = 1.0 / (1.0 + i as f32).sqrt();
        project_token(&mut vec, token, pos_weight);
    }

    // Bigrams: capture phrase-level similarity at reduced weight.
    for window in tokens.windows(2) {
        let bigram = format!("{}\x01{}", window[0], window[1]);
        project_token(&mut vec, &bigram, 0.6);
    }

    l2_normalize(&mut vec);
    vec
}

/// Write `PROJECTIONS_PER_TOKEN` independent hash projections for `token` into `vec`.
fn project_token(vec: &mut [f32], token: &str, weight: f32) {
    for salt in 0..PROJECTIONS_PER_TOKEN as u64 {
        let hash = fnv1a_salted(token, salt);
        let bucket = (hash % EMBEDDING_DIM as u64) as usize;
        let sign = if (hash >> 32) & 1 == 0 { 1.0f32 } else { -1.0f32 };
        vec[bucket] += sign * weight;
    }
}

/// FNV-1a with a salt prepended so different salt values give independent hashes.
fn fnv1a_salted(s: &str, salt: u64) -> u64 {
    const OFFSET: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;
    // Seed with salt bytes first, then the token bytes.
    let seed = salt
        .to_le_bytes()
        .iter()
        .fold(OFFSET, |h, &b| (h ^ b as u64).wrapping_mul(PRIME));
    s.bytes().fold(seed, |h, b| (h ^ b as u64).wrapping_mul(PRIME))
}

fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-8 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    // Vectors are L2-normalised, so dot product == cosine similarity.
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    1.0 - dot.clamp(-1.0, 1.0)
}

/// Normalise text to lowercase tokens, strip punctuation, remove stop words.
fn tokenize(content: &str) -> Vec<String> {
    content
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty() && t.len() > 1 && !is_stop_word(t))
        .map(str::to_owned)
        .collect()
}

fn is_stop_word(word: &str) -> bool {
    matches!(
        word,
        "a" | "an" | "the" | "and" | "or" | "but" | "in" | "on" | "at" | "to"
            | "for" | "of" | "is" | "it" | "its" | "be" | "as" | "by" | "we"
            | "with" | "from" | "that" | "this" | "are" | "was" | "has" | "have"
            | "had" | "not" | "do" | "does" | "did" | "will" | "can" | "use"
            | "used" | "into" | "if" | "so" | "when" | "which" | "all" | "also"
            | "more" | "than" | "then" | "he" | "she" | "they" | "their" | "our"
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embed_produces_unit_vector() {
        let v = embed("SQLite vector store Rust cosine similarity");
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "not unit: norm={norm}");
    }

    #[test]
    fn identical_content_zero_distance() {
        let a = embed("use sqlite for vector storage in phase one");
        let b = embed("use sqlite for vector storage in phase one");
        let d = cosine_distance(&a, &b);
        assert!(d < 1e-5, "identical content should have distance ~0, got {d}");
    }

    #[test]
    fn case_insensitive_similarity() {
        // "SQLite" and "sqlite" should produce identical embeddings after normalisation.
        let a = embed("SQLite WAL mode");
        let b = embed("sqlite WAL mode");
        let d = cosine_distance(&a, &b);
        assert!(d < 1e-5, "case variants should be identical, distance={d}");
    }

    #[test]
    fn similar_content_closer_than_unrelated() {
        let reference = embed("lancedb vector database rust");
        let similar = embed("lancedb vector store rust cosine");
        let unrelated = embed("authentication bearer token expiry");

        let d_similar = cosine_distance(&reference, &similar);
        let d_unrelated = cosine_distance(&reference, &unrelated);

        assert!(
            d_similar < d_unrelated,
            "similar content ({d_similar:.3}) should be closer than unrelated ({d_unrelated:.3})"
        );
    }

    #[test]
    fn stop_words_filtered() {
        // With stop words filtered, these two should be identical.
        let a = embed("the use of sqlite for the storage");
        let b = embed("sqlite storage");
        let d = cosine_distance(&a, &b);
        // Not identical (one has more signal tokens) but very close.
        assert!(d < 0.15, "stop-word-heavy variant distance={d:.3} should be close to clean");
    }

    #[test]
    fn empty_content_is_zero_vector() {
        let v = embed("");
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(norm < 1e-8, "empty content should produce zero vector");
    }

    #[test]
    fn bigram_boost_phrase_similarity() {
        let a = embed("commit session firewall integrity check");
        let b = embed("session firewall integrity");
        let c = embed("arbitrary unrelated content here");

        let d_ab = cosine_distance(&a, &b);
        let d_ac = cosine_distance(&a, &c);
        assert!(
            d_ab < d_ac,
            "phrase overlap ({d_ab:.3}) should beat unrelated ({d_ac:.3})"
        );
    }
}
