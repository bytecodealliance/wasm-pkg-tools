use serde::{Deserialize, Serialize};

/// A Component Model kebab-case label.
#[derive(Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Label(String);

impl AsRef<str> for Label {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Debug for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

impl From<Label> for String {
    fn from(value: Label) -> Self {
        value.0
    }
}

impl TryFrom<String> for Label {
    type Error = InvalidLabel;

    fn try_from(label: String) -> Result<Self, Self::Error> {
        if label.is_empty() {
            return Err(InvalidLabel::Empty);
        }
        for word in label.split('-') {
            let mut chars = word.chars();
            match chars.next() {
                None => return Err(InvalidLabel::EmptyWord),
                Some(ch) if !ch.is_ascii_lowercase() => {
                    return Err(InvalidLabel::InvalidWordFirstChar)
                }
                _ => (),
            }
            if !chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit()) {
                return Err(InvalidLabel::InvalidChar);
            }
        }
        Ok(Self(label))
    }
}

impl std::str::FromStr for Label {
    type Err = InvalidLabel;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.to_owned().try_into()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidLabel {
    #[error("labels may not be empty")]
    Empty,
    #[error("dash-separated words may not be empty")]
    EmptyWord,
    #[error("dash-separated words may contain only lowercase alphanumeric ASCII characters")]
    InvalidChar,
    #[error("dash-separated words must begin with an ASCII lowercase letter")]
    InvalidWordFirstChar,
}
