//! The built-in face table. Themes replace this in a later milestone.

use amacs_proto::{Color, FaceAttrs, FaceId};

pub const DEFAULT: FaceId = FaceId(0);
pub const REGION: FaceId = FaceId(1);
pub const MODE_LINE: FaceId = FaceId(2);
pub const MODE_LINE_INACTIVE: FaceId = FaceId(3);
pub const MINIBUFFER_PROMPT: FaceId = FaceId(4);
pub const VERTICAL_BORDER: FaceId = FaceId(5);
/// The selected candidate in the minibuffer.
pub const CURRENT_CANDIDATE: FaceId = FaceId(6);
/// Text of less importance, such as annotations.
pub const SHADOW: FaceId = FaceId(7);

/// Faces indexed by [`FaceId`]. The default face keeps the terminal's own
/// colours, so every other face sets both colours or neither.
pub fn table() -> Vec<FaceAttrs> {
    let plain = FaceAttrs::default();
    let black = Some(Color::rgb(0x00, 0x00, 0x00));
    vec![
        plain,
        FaceAttrs {
            fg: black,
            bg: Some(Color::rgb(0xc2, 0xbc, 0xb5)),
            ..plain
        },
        FaceAttrs {
            fg: black,
            bg: Some(Color::rgb(0xca, 0xb9, 0xb2)),
            ..plain
        },
        FaceAttrs {
            fg: Some(Color::rgb(0x58, 0x58, 0x58)),
            bg: Some(Color::rgb(0xdf, 0xd9, 0xcf)),
            ..plain
        },
        FaceAttrs {
            bold: true,
            ..plain
        },
        plain,
        FaceAttrs {
            fg: black,
            bg: Some(Color::rgb(0xf0, 0xc1, 0xcf)),
            bold: true,
            ..plain
        },
        FaceAttrs {
            italic: true,
            ..plain
        },
    ]
}
