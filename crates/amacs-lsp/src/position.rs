//! Columns as servers count them.
//!
//! A position in the protocol is a line and an offset into it, counted in
//! units of an encoding that client and server agree on: UTF-16 code units
//! unless something else was negotiated. The editor counts in chars.

/// The unit that offsets within a line are counted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

impl Encoding {
    /// The encoding with the protocol's name for it, such as `utf-16`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "utf-8" => Some(Encoding::Utf8),
            "utf-16" => Some(Encoding::Utf16),
            "utf-32" => Some(Encoding::Utf32),
            _ => None,
        }
    }

    fn units(self, c: char) -> usize {
        match self {
            Encoding::Utf8 => c.len_utf8(),
            Encoding::Utf16 => c.len_utf16(),
            Encoding::Utf32 => 1,
        }
    }
}

/// The offset in units of the char at index `chars` of `line`.
pub fn to_units(line: &str, chars: usize, encoding: Encoding) -> usize {
    line.chars().take(chars).map(|c| encoding.units(c)).sum()
}

/// The index of the char at offset `units` of `line`. An offset past the
/// end of the line, or in the middle of a char, gives the nearest char
/// boundary before it, as the protocol asks.
pub fn from_units(line: &str, units: usize, encoding: Encoding) -> usize {
    let mut seen = 0;
    for (index, c) in line.chars().enumerate() {
        seen += encoding.units(c);
        if seen > units {
            return index;
        }
    }
    line.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_depend_on_the_encoding() {
        // One, two and four bytes in UTF-8; one, one and two units in UTF-16.
        let line = "aö😀b";
        assert_eq!(to_units(line, 3, Encoding::Utf8), 7);
        assert_eq!(to_units(line, 3, Encoding::Utf16), 4);
        assert_eq!(to_units(line, 3, Encoding::Utf32), 3);
        assert_eq!(to_units(line, 99, Encoding::Utf16), 5);

        assert_eq!(from_units(line, 7, Encoding::Utf8), 3);
        assert_eq!(from_units(line, 4, Encoding::Utf16), 3);
        assert_eq!(from_units(line, 3, Encoding::Utf16), 2, "inside the emoji");
        assert_eq!(from_units(line, 99, Encoding::Utf16), 4);
        assert_eq!(from_units("", 3, Encoding::Utf16), 0);
    }
}
