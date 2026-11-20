//! The 256 colours a program can name by number.

use amacs_proto::Color;

/// The sixteen colours that themes give, and the formula for the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    /// Black, red, green, yellow, blue, magenta, cyan, white, and then
    /// their bright variants.
    pub base: [Color; 16],
}

impl Default for Palette {
    /// The colours of xterm.
    fn default() -> Self {
        let rgb = Color::rgb;
        Self {
            base: [
                rgb(0, 0, 0),
                rgb(205, 0, 0),
                rgb(0, 205, 0),
                rgb(205, 205, 0),
                rgb(0, 0, 238),
                rgb(205, 0, 205),
                rgb(0, 205, 205),
                rgb(229, 229, 229),
                rgb(127, 127, 127),
                rgb(255, 0, 0),
                rgb(0, 255, 0),
                rgb(255, 255, 0),
                rgb(92, 92, 255),
                rgb(255, 0, 255),
                rgb(0, 255, 255),
                rgb(255, 255, 255),
            ],
        }
    }
}

impl Palette {
    /// The colour with a number from 0 to 255: the base colours, then the
    /// six by six by six cube, then the greys.
    pub fn indexed(&self, index: usize) -> Option<Color> {
        match index {
            0..=15 => Some(self.base[index]),
            16..=231 => {
                let index = index - 16;
                let level = |part: usize| if part == 0 { 0 } else { 55 + 40 * part as u8 };
                Some(Color::rgb(
                    level(index / 36),
                    level(index / 6 % 6),
                    level(index % 6),
                ))
            }
            232..=255 => {
                let grey = 8 + 10 * (index as u8 - 232);
                Some(Color::rgb(grey, grey, grey))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cube_and_the_greys_follow_xterm() {
        let palette = Palette::default();
        assert_eq!(palette.indexed(16), Some(Color::rgb(0, 0, 0)));
        assert_eq!(palette.indexed(196), Some(Color::rgb(255, 0, 0)));
        assert_eq!(palette.indexed(21), Some(Color::rgb(0, 0, 255)));
        assert_eq!(palette.indexed(232), Some(Color::rgb(8, 8, 8)));
        assert_eq!(palette.indexed(255), Some(Color::rgb(238, 238, 238)));
        assert_eq!(palette.indexed(256), None);
    }
}
