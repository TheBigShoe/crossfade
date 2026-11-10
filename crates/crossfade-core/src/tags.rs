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
        let all: Vec<_> = t.iter().map(|x| (x.key.as_str(), x.value.as_str())).collect();
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
        assert_eq!(*NO_TAGS, Tags::default());
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
            ["TITLE", "ARTIST", "ALBUM", "DATE", "TRACKNUMBER", "GENRE", "COMMENT", "COPYRIGHT"]
        );
    }
}
