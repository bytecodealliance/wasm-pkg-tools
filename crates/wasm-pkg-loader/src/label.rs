/// A Component Model kebab-case label.
#[derive(Clone, PartialEq, Eq, Hash)]
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

impl TryFrom<String> for Label {
    type Error = InvalidLabel;

    fn try_from(label: String) -> Result<Self, Self::Error> {
        if label.is_empty() {
            return Err(InvalidLabel::Empty);
        }
        for word in label.split('-') {
            let mut chars = word.chars();
            let Some(first_char) = chars.next() else {
                return Err(InvalidLabel::EmptyWord);
            };
            if !first_char.is_ascii_alphabetic() {
                return Err(InvalidLabel::InvalidWordFirstChar(first_char));
            }
            let is_upper = first_char.is_ascii_uppercase();
            for char in chars {
                if !char.is_ascii_alphanumeric() {
                    return Err(InvalidLabel::InvalidChar(char));
                }
                if !char.is_ascii_digit() && char.is_ascii_uppercase() != is_upper {
                    return Err(InvalidLabel::MixedCaseWord(word.into()));
                }
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
    #[error("dash-separated words may contain only alphanumeric ASCII characters; got {0:?}")]
    InvalidChar(char),
    #[error("dash-separated words may not begin with a digit; got {0:?}")]
    InvalidWordFirstChar(char),
    #[error("dash-separated words must be all lowercase or all uppercase; got {0:?}")]
    MixedCaseWord(String),
}
