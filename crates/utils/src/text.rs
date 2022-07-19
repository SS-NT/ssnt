use std::borrow::Cow;

pub fn truncate(s: &str, max_chars: usize) -> Cow<str> {
    match s.char_indices().nth(max_chars) {
        None => s.into(),
        Some((idx, _)) => [&s[..idx - 3], "..."].concat().into(),
    }
}
