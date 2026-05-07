use std::hash::{Hash, Hasher};

use ahash::AHasher;

/// Cheap identity for the exact pane buffer bytes used to produce a diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BufferStamp {
    pub revision: u64,
    pub byte_len: usize,
    pub text_hash: u64,
}

impl BufferStamp {
    pub fn from_text(revision: u64, text: &str) -> Self {
        Self::from_text_hash(revision, text.len(), hash_text(text))
    }

    pub fn from_text_hash(revision: u64, byte_len: usize, text_hash: u64) -> Self {
        Self {
            revision,
            byte_len,
            text_hash,
        }
    }
}

impl Default for BufferStamp {
    fn default() -> Self {
        Self::from_text(0, "")
    }
}

/// Deterministic buffer hash shared by buffer stamps and the layout cache.
pub fn hash_text(s: &str) -> u64 {
    let mut h = AHasher::default();
    s.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_tracks_revision_length_and_hash() {
        let stamp = BufferStamp::from_text(7, "abc");
        assert_eq!(stamp.revision, 7);
        assert_eq!(stamp.byte_len, 3);
        assert_eq!(stamp.text_hash, hash_text("abc"));
    }

    #[test]
    fn default_stamp_matches_initial_empty_buffer() {
        assert_eq!(BufferStamp::default(), BufferStamp::from_text(0, ""));
    }
}
