//! Offline lexical similarity for Smart Organization and Smart Duplicate
//! Detection, plus the "Surprise Me" message composer.
//!
//! This module is deliberately **dependency-free and fully offline** (Decision 2
//! in `FEATURE_BACKLOG.md`): no embeddings service, no LLM, no network. It scores
//! notes against each other with classic term-frequency / inverse-document-
//! frequency (TF-IDF) cosine similarity. That's weaker than embeddings but it
//! ships today, adds zero dependencies, and keeps the "never leaves your machine"
//! promise intact. If the team later relaxes that promise, `most_similar` and
//! `cluster` are the two seams where an embedding backend would slot in.
//!
//! **Policy (Decisions 4 & 6):** only *analyzable* notes take part — non-protected
//! and non-empty. Every public entry point filters through [`is_analyzable`], so
//! protected/blank notes are never surfaced, clustered, or used as a match.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::Note;

/// Common English words that carry no topical signal, dropped during tokenizing.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "if", "then", "than", "so", "to", "of", "in", "on",
    "for", "with", "at", "by", "from", "up", "out", "is", "am", "are", "was", "were", "be",
    "been", "being", "do", "does", "did", "have", "has", "had", "this", "that", "these", "those",
    "it", "its", "as", "i", "you", "he", "she", "we", "they", "me", "him", "her", "them", "my",
    "your", "our", "their", "his", "not", "no", "yes", "can", "will", "just", "get", "got",
];

/// A single similarity match: which note, how close (0..=1), and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Match {
    /// Id of the matched note.
    pub id: String,
    /// Cosine similarity in `0.0..=1.0`.
    pub score: f64,
    /// The distinctive terms the two notes share, best first — the "reason".
    pub shared_terms: Vec<String>,
}

/// A proposed group of related notes for Smart Organization.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cluster {
    /// Ids of the notes in this cluster (size >= 2).
    pub note_ids: Vec<String>,
    /// A short human-readable label derived from the cluster's shared terms.
    pub label: String,
}

/// Whether a note may take part in any library analysis (Decisions 4 & 6):
/// it must be neither protected nor effectively empty.
pub fn is_analyzable(note: &Note) -> bool {
    !note.protected && !note.content.trim().is_empty()
}

/// Split text into lowercased, meaningful tokens (drops punctuation, short words
/// and stopwords).
pub fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(|w| w.to_lowercase())
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
        .collect()
}

/// Term-frequency map for a token list.
fn term_freq(tokens: &[String]) -> HashMap<String, f64> {
    let mut tf: HashMap<String, f64> = HashMap::new();
    for t in tokens {
        *tf.entry(t.clone()).or_insert(0.0) += 1.0;
    }
    tf
}

/// Inverse-document-frequency across a corpus of tokenized documents.
fn idf(docs: &[Vec<String>]) -> HashMap<String, f64> {
    let n = docs.len().max(1) as f64;
    let mut df: HashMap<String, f64> = HashMap::new();
    for doc in docs {
        let unique: HashSet<&String> = doc.iter().collect();
        for t in unique {
            *df.entry(t.clone()).or_insert(0.0) += 1.0;
        }
    }
    // Smoothed idf; always positive so a term never zeroes out a vector.
    df.into_iter()
        .map(|(t, d)| (t, ((n + 1.0) / (d + 1.0)).ln() + 1.0))
        .collect()
}

/// Weight a term-frequency map by idf to get a TF-IDF vector.
fn tfidf(tf: &HashMap<String, f64>, idf: &HashMap<String, f64>) -> HashMap<String, f64> {
    tf.iter()
        .map(|(t, f)| (t.clone(), f * idf.get(t).copied().unwrap_or(1.0)))
        .collect()
}

/// Cosine similarity between two sparse vectors, in `0.0..=1.0`.
fn cosine(a: &HashMap<String, f64>, b: &HashMap<String, f64>) -> f64 {
    // Iterate the smaller map for the dot product.
    let (small, big) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    let mut dot = 0.0;
    for (t, av) in small {
        if let Some(bv) = big.get(t) {
            dot += av * bv;
        }
    }
    let na: f64 = a.values().map(|v| v * v).sum::<f64>().sqrt();
    let nb: f64 = b.values().map(|v| v * v).sum::<f64>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        (dot / (na * nb)).clamp(0.0, 1.0)
    }
}

/// A precomputed TF-IDF view over the analyzable notes in a store.
struct Analyzed {
    ids: Vec<String>,
    vectors: Vec<HashMap<String, f64>>,
}

impl Analyzed {
    fn build(notes: &[Note]) -> Self {
        let analyzable: Vec<&Note> = notes.iter().filter(|n| is_analyzable(n)).collect();
        let docs: Vec<Vec<String>> = analyzable.iter().map(|n| tokenize(&n.content)).collect();
        let idf = idf(&docs);
        let vectors: Vec<HashMap<String, f64>> =
            docs.iter().map(|d| tfidf(&term_freq(d), &idf)).collect();
        let ids: Vec<String> = analyzable.iter().map(|n| n.id.clone()).collect();
        Analyzed { ids, vectors }
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.ids.iter().position(|x| x == id)
    }
}

/// Top overlapping terms between two vectors, ranked by combined weight.
fn shared_terms(a: &HashMap<String, f64>, b: &HashMap<String, f64>, limit: usize) -> Vec<String> {
    let mut shared: Vec<(String, f64)> = a
        .iter()
        .filter_map(|(t, av)| b.get(t).map(|bv| (t.clone(), av + bv)))
        .collect();
    shared.sort_by(|x, y| {
        y.1.partial_cmp(&x.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.0.cmp(&y.0))
    });
    shared.into_iter().take(limit).map(|(t, _)| t).collect()
}

/// Find the note most similar to `target_id`, if any clears `min_score`.
///
/// Returns `None` when the target is not analyzable, there is nothing to compare
/// against, or the best match falls below the threshold. Used by Smart Duplicate
/// Detection at a note's commit moment.
pub fn most_similar(target_id: &str, notes: &[Note], min_score: f64) -> Option<Match> {
    let analyzed = Analyzed::build(notes);
    let ti = analyzed.index_of(target_id)?;
    let target = &analyzed.vectors[ti];

    let mut best: Option<Match> = None;
    for (i, vec) in analyzed.vectors.iter().enumerate() {
        if i == ti {
            continue;
        }
        let score = cosine(target, vec);
        if score < min_score {
            continue;
        }
        if best.as_ref().map(|m| score > m.score).unwrap_or(true) {
            best = Some(Match {
                id: analyzed.ids[i].clone(),
                score,
                shared_terms: shared_terms(target, vec, 5),
            });
        }
    }
    best
}

/// Cluster analyzable notes into groups whose members are transitively similar
/// at or above `threshold`. Singletons are omitted. Used by Smart Organization.
pub fn cluster(notes: &[Note], threshold: f64) -> Vec<Cluster> {
    let analyzed = Analyzed::build(notes);
    let n = analyzed.ids.len();
    let mut parent: Vec<usize> = (0..n).collect();

    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]]; // path halving
            x = parent[x];
        }
        x
    }

    for i in 0..n {
        for j in (i + 1)..n {
            if cosine(&analyzed.vectors[i], &analyzed.vectors[j]) >= threshold {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }

    // Collect members per root, preserving input order for stable output.
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }

    let mut clusters: Vec<Cluster> = groups
        .into_values()
        .filter(|members| members.len() >= 2)
        .map(|members| {
            let note_ids: Vec<String> = members.iter().map(|&i| analyzed.ids[i].clone()).collect();
            let label = cluster_label(&members, &analyzed.vectors);
            Cluster { note_ids, label }
        })
        .collect();

    // Largest clusters first; ties broken by first note id for determinism.
    clusters.sort_by(|a, b| {
        b.note_ids
            .len()
            .cmp(&a.note_ids.len())
            .then_with(|| a.note_ids[0].cmp(&b.note_ids[0]))
    });
    clusters
}

/// Derive a short label from the terms most common across a cluster's members.
fn cluster_label(members: &[usize], vectors: &[HashMap<String, f64>]) -> String {
    let mut weight: HashMap<String, f64> = HashMap::new();
    for &i in members {
        for (t, w) in &vectors[i] {
            *weight.entry(t.clone()).or_insert(0.0) += w;
        }
    }
    let mut terms: Vec<(String, f64)> = weight.into_iter().collect();
    terms.sort_by(|x, y| {
        y.1.partial_cmp(&x.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| x.0.cmp(&y.0))
    });
    let top: Vec<String> = terms.into_iter().take(3).map(|(t, _)| t).collect();
    if top.is_empty() {
        "Related notes".to_string()
    } else {
        top.join(", ")
    }
}

/// Compose a "Surprise Me" message from the note library (Decision 4: ignores
/// empty and protected notes, mild recency bias). Deterministic given its inputs
/// so the shell supplies the hour-of-day and a rotating `pick` seed, and it stays
/// unit-testable. Fully local — no model, no network.
pub fn surprise_message(notes: &[Note], hour: u32, pick: u64) -> String {
    let greeting = match hour {
        5..=11 => "Good morning",
        12..=17 => "Good afternoon",
        18..=22 => "Good evening",
        _ => "Still up",
    };

    let quotes = [
        "Small steps still move you forward.",
        "Done is better than perfect.",
        "You don't have to see the whole staircase — just take the first step.",
        "A little progress each day adds up.",
        "The best way out is always through.",
        "Start where you are. Use what you have.",
    ];
    let quote = quotes[(pick as usize) % quotes.len()];

    let analyzable: Vec<&Note> = notes.iter().filter(|n| is_analyzable(n)).collect();
    let nudge = if analyzable.is_empty() {
        "Your notes are a blank canvas today — jot down the first thing on your mind.".to_string()
    } else {
        // Mild recency bias: the least-recently-touched note is a good revisit.
        let oldest = analyzable
            .iter()
            .min_by_key(|n| n.updated_at)
            .expect("non-empty");
        let snippet = snippet(&oldest.content, 40);
        match pick % 3 {
            0 => format!(
                "You have {} notes going. Maybe revisit \u{201c}{}\u{201d} — it's been waiting a while.",
                analyzable.len(),
                snippet
            ),
            1 => format!(
                "{} notes and counting. Pick one small thing from them to finish today.",
                analyzable.len()
            ),
            _ => format!(
                "That older note \u{201c}{}\u{201d} could use a fresh look.",
                snippet
            ),
        }
    };

    format!("{greeting}! {quote}\n\n{nudge}")
}

/// First `max` characters of the first non-empty line of `content`, trimmed.
fn snippet(content: &str, max: usize) -> String {
    let line = content
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut s: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        s.push('\u{2026}');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, content: &str) -> Note {
        Note {
            id: id.to_string(),
            content: content.to_string(),
            color: crate::DEFAULT_COLOR.to_string(),
            x: 0.0,
            y: 0.0,
            width: crate::DEFAULT_WIDTH,
            height: crate::DEFAULT_HEIGHT,
            created_at: 0,
            updated_at: 0,
            opacity: crate::DEFAULT_OPACITY,
            group_id: None,
            protected: false,
            attachments: Vec::new(),
            enc: None,
        }
    }

    #[test]
    fn tokenize_drops_stopwords_and_short_words() {
        let toks = tokenize("The quick brown fox, it is a FOX!");
        assert!(toks.contains(&"quick".to_string()));
        assert!(toks.contains(&"brown".to_string()));
        assert_eq!(toks.iter().filter(|t| *t == "fox").count(), 2);
        assert!(!toks.contains(&"the".to_string()));
        assert!(!toks.contains(&"is".to_string()));
        assert!(!toks.contains(&"it".to_string()));
    }

    #[test]
    fn most_similar_finds_the_related_note() {
        let notes = vec![
            note("a", "Buy milk, eggs and bread from the grocery store"),
            note("b", "Grocery shopping list: milk, bread, cheese"),
            note("c", "Fix the login bug in the auth service"),
        ];
        let m = most_similar("a", &notes, 0.05).expect("a match");
        assert_eq!(m.id, "b");
        assert!(m.score > 0.0);
        assert!(m.shared_terms.iter().any(|t| t == "milk" || t == "bread" || t == "grocery"));
    }

    #[test]
    fn most_similar_respects_threshold() {
        let notes = vec![
            note("a", "quantum chromodynamics lattice gauge theory"),
            note("b", "my cat likes tuna in the morning"),
        ];
        assert!(most_similar("a", &notes, 0.2).is_none());
    }

    #[test]
    fn analysis_excludes_protected_and_empty_notes() {
        let mut protected = note("secret", "milk bread grocery store shopping");
        protected.protected = true;
        let notes = vec![
            note("a", "milk bread grocery store shopping list"),
            protected,
            note("blank", "   "),
        ];
        // The only analyzable peer for "a" is protected/empty, so no match.
        assert!(most_similar("a", &notes, 0.05).is_none());
        // A protected note is never itself a valid target.
        assert!(most_similar("secret", &notes, 0.0).is_none());
    }

    #[test]
    fn cluster_groups_similar_and_omits_singletons() {
        let notes = vec![
            note("g1", "vacation trip to japan tokyo kyoto itinerary"),
            note("g2", "japan travel plan tokyo hotels and trains"),
            note("s1", "quarterly tax return accountant paperwork"),
            note("lonely", "a completely unrelated single thought about gardening"),
        ];
        let clusters = cluster(&notes, 0.1);
        // The two japan notes cluster; the tax + gardening notes stay singletons.
        assert_eq!(clusters.len(), 1);
        let ids = &clusters[0].note_ids;
        assert!(ids.contains(&"g1".to_string()));
        assert!(ids.contains(&"g2".to_string()));
        assert!(!clusters[0].label.is_empty());
    }

    #[test]
    fn surprise_message_handles_empty_library() {
        let msg = surprise_message(&[], 9, 0);
        assert!(msg.starts_with("Good morning"));
        assert!(msg.contains("blank canvas"));
    }

    #[test]
    fn surprise_message_greets_by_hour_and_uses_library() {
        let notes = vec![note("a", "finish the quarterly report for finance")];
        let msg = surprise_message(&notes, 20, 1);
        assert!(msg.starts_with("Good evening"));
        // Deterministic given inputs.
        assert_eq!(msg, surprise_message(&notes, 20, 1));
    }
}
