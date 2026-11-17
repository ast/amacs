//! Faces and themes.
//!
//! A face is a named appearance for text. Its colours are names in the
//! palette of the current theme, so loading another theme recolours every
//! face. The four Modus themes are built in; more can be defined from
//! Scheme.

use std::collections::HashMap;

use amacs_proto::{Color, FaceAttrs, FaceId};
use amacs_treesit::Highlight;

use crate::error::{CmdError, CmdResult};

pub const DEFAULT: FaceId = FaceId(0);
pub const REGION: FaceId = FaceId(1);
pub const MODE_LINE: FaceId = FaceId(2);
pub const MODE_LINE_INACTIVE: FaceId = FaceId(3);
pub const MINIBUFFER_PROMPT: FaceId = FaceId(4);
pub const VERTICAL_BORDER: FaceId = FaceId(5);
/// The selected candidate in the minibuffer.
pub const CURRENT_CANDIDATE: FaceId = FaceId(6);
/// Text of less importance.
pub const SHADOW: FaceId = FaceId(7);
/// What is said about a candidate, next to it.
pub const ANNOTATION: FaceId = FaceId(8);
pub const LINE_NUMBER: FaceId = FaceId(9);
pub const LINE_NUMBER_CURRENT: FaceId = FaceId(10);
/// The list of completions that pops up at point.
pub const POPUP: FaceId = FaceId(11);
pub const POPUP_SELECTED: FaceId = FaceId(12);
pub const POPUP_ANNOTATION: FaceId = FaceId(13);
/// How text with an error, a warning or a note is marked. These are laid
/// over the face the text has anyway; see [`Faces::diagnostic`].
pub const DIAGNOSTIC_ERROR: FaceId = FaceId(14);
const FIRST_SYNTAX: u32 = 17;

/// How serious a diagnostic is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Note,
}

/// The face that draws a kind of syntax.
pub fn syntax(highlight: Highlight) -> FaceId {
    FaceId(FIRST_SYNTAX + highlight as u32)
}

pub const DEFAULT_THEME: &str = "modus-operandi-tinted";

const BUILT_IN_THEMES: &[(&str, &str)] = &[
    (
        "modus-operandi",
        include_str!("themes/modus-operandi.palette"),
    ),
    (
        "modus-operandi-tinted",
        include_str!("themes/modus-operandi-tinted.palette"),
    ),
    (
        "modus-vivendi",
        include_str!("themes/modus-vivendi.palette"),
    ),
    (
        "modus-vivendi-tinted",
        include_str!("themes/modus-vivendi-tinted.palette"),
    ),
];

/// How a face looks. Colours are palette names or `#rrggbb`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FaceSpec {
    pub fg: Option<String>,
    pub bg: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// Colour of the underline, if not that of the text.
    pub underline_color: Option<String>,
    /// Colour of a thin line along the top and bottom of the row.
    pub box_color: Option<String>,
}

impl FaceSpec {
    fn new(fg: &str, bg: &str) -> Self {
        let color = |name: &str| (!name.is_empty()).then(|| name.to_owned());
        Self {
            fg: color(fg),
            bg: color(bg),
            ..Self::default()
        }
    }

    fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    fn boxed_in(mut self, color: &str) -> Self {
        self.box_color = Some(color.to_owned());
        self
    }

    fn underlined_in(mut self, color: &str) -> Self {
        self.underline = true;
        self.underline_color = Some(color.to_owned());
        self
    }
}

/// The faces every editor starts with, in [`FaceId`] order.
fn built_in_faces() -> Vec<(String, FaceSpec)> {
    let face = FaceSpec::new;
    let mut faces = vec![
        ("default", face("fg-main", "bg-main")),
        ("region", face("fg-region", "bg-region")),
        (
            "mode-line",
            face("fg-mode-line-active", "bg-mode-line-active").boxed_in("border-mode-line-active"),
        ),
        (
            "mode-line-inactive",
            face("fg-mode-line-inactive", "bg-mode-line-inactive")
                .boxed_in("border-mode-line-inactive"),
        ),
        ("minibuffer-prompt", face("fg-prompt", "bg-prompt")),
        ("vertical-border", face("border", "")),
        ("current-candidate", face("", "bg-completion").bold()),
        ("shadow", face("fg-dim", "")),
        ("annotation", face("docstring", "").italic()),
        (
            "line-number",
            face("fg-line-number-inactive", "bg-line-number-inactive"),
        ),
        (
            "line-number-current",
            face("fg-line-number-active", "bg-line-number-active").bold(),
        ),
        ("popup", face("", "bg-popup")),
        ("popup-selected", face("", "bg-completion").bold()),
        ("popup-annotation", face("fg-dim", "bg-popup")),
        (
            "diagnostic-error",
            face("", "").underlined_in("underline-err"),
        ),
        (
            "diagnostic-warning",
            face("", "").underlined_in("underline-warning"),
        ),
        (
            "diagnostic-note",
            face("", "").underlined_in("underline-note"),
        ),
    ];
    for highlight in Highlight::ALL {
        let spec = match highlight {
            Highlight::Comment => face("comment", "").italic(),
            Highlight::Doc => face("docstring", "").italic(),
            Highlight::String => face("string", ""),
            Highlight::Escape => face("rx-backslash", "").bold(),
            Highlight::Regexp => face("rx-construct", ""),
            Highlight::Constant => face("constant", ""),
            Highlight::Number => face("number", ""),
            Highlight::Keyword => face("keyword", "").bold(),
            Highlight::Operator => face("operator", ""),
            Highlight::Bracket => face("bracket", ""),
            Highlight::Delimiter => face("delimiter", ""),
            Highlight::FunctionName => face("fnname", ""),
            Highlight::FunctionCall => face("fnname-call", ""),
            Highlight::Builtin => face("builtin", "").bold(),
            Highlight::Preprocessor => face("preprocessor", ""),
            Highlight::Type => face("type", "").bold(),
            Highlight::VariableName => face("variable", ""),
            Highlight::Property => face("property", ""),
            Highlight::Heading => face("fg-heading-1", "").bold(),
            Highlight::Emphasis => face("", "").italic(),
            Highlight::Strong => face("", "").bold(),
            Highlight::Code => face("fg-prose-code", "bg-prose-code"),
            Highlight::Link => face("fg-link", "").underline(),
        };
        faces.push((highlight.name(), spec));
    }
    faces
        .into_iter()
        .map(|(name, spec)| (name.to_owned(), spec))
        .collect()
}

fn parse_hex(text: &str) -> Option<Color> {
    let hex = text.strip_prefix('#').filter(|hex| hex.len() == 6)?;
    let channel = |index: usize| u8::from_str_radix(hex.get(index..index + 2)?, 16).ok();
    Some(Color::rgb(channel(0)?, channel(2)?, channel(4)?))
}

/// Reads a palette: lines of a name and a value, `#` starting a comment.
fn parse_palette(source: &str) -> HashMap<String, String> {
    source
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once(' '))
        .map(|(name, value)| (name.to_owned(), value.trim().to_owned()))
        .collect()
}

struct Theme {
    name: String,
    /// Names to colours or to other names.
    palette: HashMap<String, String>,
}

/// All faces and themes.
pub struct Faces {
    faces: Vec<(String, FaceSpec)>,
    themes: Vec<Theme>,
    current: usize,
    /// Palette entries that replace those of whichever theme is loaded.
    overrides: HashMap<String, String>,
    /// Changes whenever the face table does.
    version: u64,
}

impl Default for Faces {
    fn default() -> Self {
        let themes: Vec<Theme> = BUILT_IN_THEMES
            .iter()
            .map(|(name, palette)| Theme {
                name: (*name).to_owned(),
                palette: parse_palette(palette),
            })
            .collect();
        let current = themes
            .iter()
            .position(|theme| theme.name == DEFAULT_THEME)
            .expect("the default theme is built in");
        Self {
            faces: built_in_faces(),
            themes,
            current,
            overrides: HashMap::new(),
            version: 1,
        }
    }
}

impl Faces {
    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn theme(&self) -> &str {
        &self.themes[self.current].name
    }

    pub fn theme_names(&self) -> impl Iterator<Item = &str> {
        self.themes.iter().map(|theme| theme.name.as_str())
    }

    pub fn load_theme(&mut self, name: &str) -> CmdResult {
        self.current = self
            .themes
            .iter()
            .position(|theme| theme.name == name)
            .ok_or_else(|| CmdError::msg(format!("Unknown theme: {name}")))?;
        self.version += 1;
        Ok(())
    }

    /// Defines a theme as a copy of `base` with some palette entries
    /// replaced, or redefines it if it exists.
    pub fn define_theme(
        &mut self,
        name: &str,
        base: &str,
        entries: Vec<(String, String)>,
    ) -> CmdResult {
        let mut palette = self
            .themes
            .iter()
            .find(|theme| theme.name == base)
            .ok_or_else(|| CmdError::msg(format!("Unknown theme: {base}")))?
            .palette
            .clone();
        palette.extend(entries);
        match self.themes.iter_mut().find(|theme| theme.name == name) {
            Some(theme) => theme.palette = palette,
            None => self.themes.push(Theme {
                name: name.to_owned(),
                palette,
            }),
        }
        self.version += 1;
        Ok(())
    }

    /// Replaces the palette entries that apply on top of every theme.
    pub fn set_overrides(&mut self, entries: Vec<(String, String)>) {
        self.overrides = entries.into_iter().collect();
        self.version += 1;
    }

    pub fn face_id(&self, name: &str) -> Option<FaceId> {
        let index = self.faces.iter().position(|(face, _)| face == name)?;
        Some(FaceId(index as u32))
    }

    pub fn spec(&self, name: &str) -> Option<&FaceSpec> {
        let (_, spec) = self.faces.iter().find(|(face, _)| face == name)?;
        Some(spec)
    }

    /// Defines a face or changes an existing one.
    pub fn set_face(&mut self, name: &str, spec: FaceSpec) -> FaceId {
        self.version += 1;
        match self.face_id(name) {
            Some(id) => {
                self.faces[id.0 as usize].1 = spec;
                id
            }
            None => {
                self.faces.push((name.to_owned(), spec));
                FaceId(self.faces.len() as u32 - 1)
            }
        }
    }

    /// The colour that a palette name or `#rrggbb` stands for now.
    pub fn color(&self, name: &str) -> Option<Color> {
        let palette = &self.themes[self.current].palette;
        let mut name = name;
        // The limit guards against entries that name each other.
        for _ in 0..16 {
            if let Some(color) = parse_hex(name) {
                return Some(color);
            }
            name = self.overrides.get(name).or_else(|| palette.get(name))?;
        }
        None
    }

    /// The face for text in face `base` that has a diagnostic on it: the
    /// same, with the diagnostic's underline.
    pub fn diagnostic(&self, base: FaceId, severity: Severity) -> FaceId {
        let block = severity as u32 + 1;
        FaceId(block * self.faces.len() as u32 + base.0)
    }

    /// The face table for a frontend, indexed by [`FaceId`]. A face without
    /// a colour of its own has that of the default face. After the faces
    /// themselves come three copies of them, one for each severity of
    /// diagnostic, with that diagnostic's face laid over each.
    pub fn table(&self) -> Vec<FaceAttrs> {
        let resolve = |name: &Option<String>| name.as_deref().and_then(|name| self.color(name));
        let default = &self.faces[0].1;
        let (fg, bg) = (resolve(&default.fg), resolve(&default.bg));
        let mut table: Vec<FaceAttrs> = self
            .faces
            .iter()
            .map(|(_, spec)| FaceAttrs {
                fg: resolve(&spec.fg).or(fg),
                bg: resolve(&spec.bg).or(bg),
                bold: spec.bold,
                italic: spec.italic,
                underline: spec.underline,
                inverse: false,
                underline_color: resolve(&spec.underline_color),
                box_color: resolve(&spec.box_color),
            })
            .collect();
        let count = table.len();
        for severity in 0..3 {
            let over = &self.faces[DIAGNOSTIC_ERROR.0 as usize + severity].1;
            for index in 0..count {
                let base = table[index];
                table.push(FaceAttrs {
                    fg: resolve(&over.fg).or(base.fg),
                    bg: resolve(&over.bg).or(base.bg),
                    bold: base.bold || over.bold,
                    italic: base.italic || over.italic,
                    underline: base.underline || over.underline,
                    inverse: false,
                    underline_color: resolve(&over.underline_color).or(base.underline_color),
                    box_color: base.box_color,
                });
            }
        }
        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_faces_are_where_their_ids_say() {
        let faces = Faces::default();
        assert_eq!(faces.face_id("default"), Some(DEFAULT));
        assert_eq!(faces.face_id("shadow"), Some(SHADOW));
        assert_eq!(faces.face_id("popup-annotation"), Some(POPUP_ANNOTATION));
        assert_eq!(faces.face_id("diagnostic-error"), Some(DIAGNOSTIC_ERROR));
        assert_eq!(
            faces.face_id("line-number-current"),
            Some(LINE_NUMBER_CURRENT)
        );
        for highlight in Highlight::ALL {
            assert_eq!(faces.face_id(highlight.name()), Some(syntax(highlight)));
        }
    }

    #[test]
    fn every_built_in_colour_resolves_in_every_theme() {
        let mut faces = Faces::default();
        let names: Vec<String> = faces.theme_names().map(str::to_owned).collect();
        for theme in names {
            faces.load_theme(&theme).unwrap();
            for (name, spec) in &faces.faces {
                for color in [&spec.fg, &spec.bg, &spec.underline_color, &spec.box_color]
                    .into_iter()
                    .flatten()
                {
                    let unset = matches!(color.as_str(), "bg-prompt" | "bg-prose-code");
                    assert!(
                        faces.color(color).is_some() || unset,
                        "{color} of face {name} in {theme}"
                    );
                }
            }
        }
    }

    #[test]
    fn themes_and_overrides_change_colours() {
        let mut faces = Faces::default();
        let light = faces.table();
        assert_eq!(light[0].bg, Some(Color::rgb(0xfb, 0xf7, 0xf0)));
        // A face without a background has the default one.
        assert_eq!(light[SHADOW.0 as usize].bg, light[0].bg);

        faces.load_theme("modus-vivendi-tinted").unwrap();
        let dark = faces.table();
        assert_eq!(dark[0].bg, Some(Color::rgb(0x0d, 0x0e, 0x1c)));
        assert!(faces.load_theme("no-such-theme").is_err());

        let comment = syntax(Highlight::Comment).0 as usize;
        let before = dark[comment].fg;
        faces.set_overrides(vec![("comment".to_owned(), "yellow-faint".to_owned())]);
        let after = faces.table()[comment].fg;
        assert_ne!(before, after);
        assert_eq!(after, faces.color("yellow-faint"));
    }

    #[test]
    fn diagnostics_are_laid_over_any_face() {
        let faces = Faces::default();
        let table = faces.table();
        let keyword = syntax(Highlight::Keyword);
        let marked = faces.diagnostic(keyword, Severity::Warning);
        let (plain, marked) = (table[keyword.0 as usize], table[marked.0 as usize]);
        assert!(marked.underline && !plain.underline);
        assert_eq!(marked.underline_color, faces.color("underline-warning"));
        assert_eq!((marked.fg, marked.bold), (plain.fg, plain.bold));
        let error = table[faces.diagnostic(DEFAULT, Severity::Error).0 as usize];
        assert_eq!(error.underline_color, faces.color("underline-err"));
    }

    #[test]
    fn a_theme_can_be_derived_from_another() {
        let mut faces = Faces::default();
        let entries = vec![("bg-main".to_owned(), "#102030".to_owned())];
        faces
            .define_theme("mine", "modus-vivendi", entries)
            .unwrap();
        faces.load_theme("mine").unwrap();
        assert_eq!(faces.table()[0].bg, Some(Color::rgb(0x10, 0x20, 0x30)));
        assert_eq!(faces.table()[0].fg, Some(Color::rgb(0xff, 0xff, 0xff)));
    }
}
