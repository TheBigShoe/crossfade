/// One metadata entry. Keys follow Vorbis-comment naming (see [`tag_keys`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    pub key: String,
    pub value: String,
}

/// Ordered entries; duplicate keys are legal (several artists).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Tags {
    entries: Vec<Tag>,
}

/// What a container without tags returns.
pub static NO_TAGS: Tags = Tags::new();

/// Vorbis-comment names are the canonical keys; other containers map onto them.
pub mod tag_keys {
    pub const TITLE: &str = "TITLE";
    pub const ARTIST: &str = "ARTIST";
    pub const ALBUM: &str = "ALBUM";
    pub const DATE: &str = "DATE";
    pub const TRACKNUMBER: &str = "TRACKNUMBER";
    pub const GENRE: &str = "GENRE";
    pub const COMMENT: &str = "COMMENT";
    pub const COPYRIGHT: &str = "COPYRIGHT";
}

impl Tags {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn push(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.entries.push(Tag {
            key: key.into(),
            value: value.into(),
        });
    }

    /// First match, ASCII case-insensitive.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|t| t.key.eq_ignore_ascii_case(key))
            .map(|t| t.value.as_str())
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Tag> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_order_and_duplicates() {
        let mut t = Tags::new();
        assert!(t.is_empty());
        t.push("ARTIST", "A");
        t.push(String::from("ARTIST"), String::from("B"));
        t.push("TITLE", "T");
        assert_eq!(t.len(), 3);
        let all: Vec<_> = t
            .iter()
            .map(|x| (x.key.as_str(), x.value.as_str()))
            .collect();
        assert_eq!(all, [("ARTIST", "A"), ("ARTIST", "B"), ("TITLE", "T")]);
    }

    #[test]
    fn get_is_first_match_and_ascii_case_insensitive() {
        let mut t = Tags::new();
        t.push("Artist", "first");
        t.push("ARTIST", "second");
        assert_eq!(t.get("artist"), Some("first"));
        assert_eq!(t.get(tag_keys::ARTIST), Some("first"));
        assert_eq!(t.get("album"), None);
        // Only ASCII folds: a non-ASCII key must match exactly.
        t.push("ÄLBUM", "x");
        assert_eq!(t.get("äLBUM"), None);
        assert_eq!(t.get("ÄLBUM"), Some("x"));
    }

    #[test]
    fn no_tags_is_empty_and_default() {
        assert!(NO_TAGS.is_empty());
        assert_eq!(NO_TAGS, Tags::default());
        assert_eq!(Tags::default().clone(), Tags::new());
    }

    #[test]
    fn canonical_keys_are_vorbis_comment_names() {
        let keys = [
            tag_keys::TITLE,
            tag_keys::ARTIST,
            tag_keys::ALBUM,
            tag_keys::DATE,
            tag_keys::TRACKNUMBER,
            tag_keys::GENRE,
            tag_keys::COMMENT,
            tag_keys::COPYRIGHT,
        ];
        assert_eq!(
            keys,
            [
                "TITLE",
                "ARTIST",
                "ALBUM",
                "DATE",
                "TRACKNUMBER",
                "GENRE",
                "COMMENT",
                "COPYRIGHT"
            ]
        );
    }
}
