//! The languages the editor knows, with their grammars and conventions.

use std::sync::OnceLock;

use tree_sitter::Query;

use crate::highlight::Capture;

/// How lines of a language are indented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentStyle {
    /// Not at all: a new line starts where the previous one did.
    None,
    /// By the brackets and constructs that enclose the line.
    Brackets(&'static BracketRules),
    /// As Lisp code, by the form that encloses the line.
    Lisp,
    /// By the previous line; a line ending in a colon indents the next.
    Offside,
}

/// What, besides brackets, changes the indentation in a bracket language.
#[derive(Debug, PartialEq, Eq)]
pub struct BracketRules {
    /// Kinds of node whose lines after the first are indented one level.
    pub indent: &'static [&'static str],
    /// Pairs of a parent and a child kind: lines in the child are indented.
    pub indent_within: &'static [(&'static str, &'static str)],
    /// Tokens that, first on a line, end the indentation of the construct
    /// they belong to, as `fi` does in a shell script.
    pub outdent_tokens: &'static [&'static str],
    /// Kinds of node that start one level out, as `case` does in C.
    pub outdent_start: &'static [&'static str],
    /// Kinds of node whose brackets do not indent what they enclose, as
    /// those of a C++ namespace.
    pub flat: &'static [&'static str],
}

const PLAIN: BracketRules = BracketRules {
    indent: &[],
    indent_within: &[],
    outdent_tokens: &[],
    outdent_start: &[],
    flat: &[],
};

const RUST: BracketRules = BracketRules {
    indent: &[
        "let_declaration",
        "assignment_expression",
        "compound_assignment_expr",
        "binary_expression",
        "where_clause",
        "match_arm",
    ],
    ..PLAIN
};

const C: BracketRules = BracketRules {
    indent: &[
        "assignment_expression",
        "binary_expression",
        "init_declarator",
    ],
    outdent_start: &["case_statement", "labeled_statement", "access_specifier"],
    ..PLAIN
};

const CPP: BracketRules = BracketRules {
    flat: &["namespace_definition"],
    ..C
};

const GO: BracketRules = BracketRules {
    outdent_start: &[
        "expression_case",
        "default_case",
        "type_case",
        "communication_case",
    ],
    ..PLAIN
};

const JAVASCRIPT: BracketRules = BracketRules {
    indent: &[
        "switch_case",
        "switch_default",
        "variable_declarator",
        "assignment_expression",
        "binary_expression",
        "ternary_expression",
        "jsx_element",
    ],
    outdent_start: &["jsx_closing_element"],
    ..PLAIN
};

const NIX: BracketRules = BracketRules {
    indent_within: &[("let_expression", "binding_set")],
    ..PLAIN
};

const BASH: BracketRules = BracketRules {
    indent: &["if_statement", "do_group", "case_statement", "case_item"],
    outdent_tokens: &["fi", "else", "elif", "then", "done", "esac", "do"],
    ..PLAIN
};

const CMAKE: BracketRules = BracketRules {
    indent: &[
        "if_condition",
        "foreach_loop",
        "while_loop",
        "function_def",
        "macro_def",
        "block_def",
    ],
    outdent_start: &[
        "endif_command",
        "else_command",
        "elseif_command",
        "endforeach_command",
        "endwhile_command",
        "endfunction_command",
        "endmacro_command",
        "endblock_command",
    ],
    ..PLAIN
};

/// Where a language holds text in other languages.
#[derive(Debug, PartialEq, Eq)]
pub struct Injections {
    /// Kinds of node whose text is in a language: the kind and the language.
    pub kinds: &'static [(&'static str, &'static str)],
    /// Kinds of node in which one node names the language of another, as a
    /// fenced block of code does: the kind, the kind of the node with the
    /// name, and the kind of the node with the text.
    pub named: &'static [(&'static str, &'static str, &'static str)],
}

const MARKDOWN: Injections = Injections {
    kinds: &[
        ("inline", "markdown-inline"),
        ("pipe_table_cell", "markdown-inline"),
        ("html_block", "html"),
        ("minus_metadata", "yaml"),
        ("plus_metadata", "toml"),
    ],
    named: &[("fenced_code_block", "language", "code_fence_content")],
};

/// A compiled grammar with its highlighting query.
pub(crate) struct Grammar {
    pub language: tree_sitter::Language,
    pub query: Query,
    /// What each capture of the query means, by capture index.
    pub captures: Vec<Capture>,
}

/// A language the editor knows.
pub struct Language {
    pub name: &'static str,
    /// What starts a comment that runs to the end of the line.
    pub line_comment: Option<&'static str>,
    pub indent: IndentStyle,
    /// Whether `'` delimits strings or chars, so that typing one inserts two.
    pub pairs_single_quote: bool,
    /// Whether a backtick delimits something.
    pub pairs_backtick: bool,
    grammar: Option<fn() -> tree_sitter::Language>,
    /// Highlighting queries, concatenated in this order.
    highlights: &'static [&'static str],
    /// Whether, of two patterns matching one node, the earlier decides its
    /// highlight. Queries written for tree-sitter before 0.25 expect that.
    pub(crate) first_pattern_wins: bool,
    pub(crate) injections: Option<&'static Injections>,
    /// Whether the language is read line by line, by code written for it,
    /// where others have a grammar.
    pub(crate) by_lines: bool,
    compiled: OnceLock<Option<Grammar>>,
}

impl std::fmt::Debug for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Language({})", self.name)
    }
}

impl Language {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            line_comment: None,
            indent: IndentStyle::None,
            pairs_single_quote: false,
            pairs_backtick: false,
            grammar: None,
            highlights: &[],
            first_pattern_wins: false,
            injections: None,
            by_lines: false,
            compiled: OnceLock::new(),
        }
    }

    const fn grammar(
        mut self,
        grammar: fn() -> tree_sitter::Language,
        highlights: &'static [&'static str],
    ) -> Self {
        self.grammar = Some(grammar);
        self.highlights = highlights;
        self
    }

    const fn comment(mut self, prefix: &'static str) -> Self {
        self.line_comment = Some(prefix);
        self
    }

    const fn indent(mut self, style: IndentStyle) -> Self {
        self.indent = style;
        self
    }

    const fn brackets(self, rules: &'static BracketRules) -> Self {
        self.indent(IndentStyle::Brackets(rules))
    }

    const fn single_quotes(mut self) -> Self {
        self.pairs_single_quote = true;
        self
    }

    const fn backticks(mut self) -> Self {
        self.pairs_backtick = true;
        self
    }

    const fn first_pattern_wins(mut self) -> Self {
        self.first_pattern_wins = true;
        self
    }

    const fn injections(mut self, injections: &'static Injections) -> Self {
        self.injections = Some(injections);
        self
    }

    const fn by_lines(mut self) -> Self {
        self.by_lines = true;
        self
    }

    pub fn has_grammar(&self) -> bool {
        self.grammar.is_some()
    }

    /// The compiled grammar and query, or `None` if the language has no
    /// grammar. Compiling happens once, on first use.
    pub(crate) fn compiled(&self) -> Option<&Grammar> {
        self.compiled
            .get_or_init(|| {
                let language = (self.grammar?)();
                let source = self.highlights.join("\n");
                // A query that does not fit its grammar is a bug that the
                // tests catch; at run time the language just goes unhighlighted.
                let query = Query::new(&language, &source).ok()?;
                let captures = query
                    .capture_names()
                    .iter()
                    .map(|name| Capture::for_name(name))
                    .collect();
                Some(Grammar {
                    language,
                    query,
                    captures,
                })
            })
            .as_ref()
    }

    /// Compiles the highlighting query and reports what is wrong with it.
    pub fn check_query(&self) -> Result<(), String> {
        let Some(grammar) = self.grammar else {
            return Ok(());
        };
        Query::new(&grammar(), &self.highlights.join("\n"))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

static LANGUAGES: [Language; 25] = [
    Language::new("bash")
        .grammar(
            || tree_sitter_bash::LANGUAGE.into(),
            &[tree_sitter_bash::HIGHLIGHT_QUERY],
        )
        .comment("#")
        .brackets(&BASH)
        .single_quotes()
        .backticks(),
    Language::new("c")
        .grammar(
            || tree_sitter_c::LANGUAGE.into(),
            &[
                tree_sitter_c::HIGHLIGHT_QUERY,
                include_str!("../queries/c-extra.scm"),
            ],
        )
        .comment("//")
        .brackets(&C)
        .single_quotes(),
    Language::new("cmake")
        .grammar(
            || tree_sitter_cmake::LANGUAGE.into(),
            &[tree_sitter_cmake::HIGHLIGHTS_QUERY],
        )
        .comment("#")
        .brackets(&CMAKE),
    Language::new("cpp")
        .grammar(
            || tree_sitter_cpp::LANGUAGE.into(),
            &[
                tree_sitter_c::HIGHLIGHT_QUERY,
                tree_sitter_cpp::HIGHLIGHT_QUERY,
                include_str!("../queries/c-extra.scm"),
                include_str!("../queries/cpp-extra.scm"),
            ],
        )
        .comment("//")
        .brackets(&CPP)
        .single_quotes(),
    Language::new("css")
        .grammar(
            || tree_sitter_css::LANGUAGE.into(),
            &[tree_sitter_css::HIGHLIGHTS_QUERY],
        )
        .brackets(&PLAIN)
        .single_quotes(),
    Language::new("devicetree")
        .grammar(
            || tree_sitter_devicetree::LANGUAGE.into(),
            &[tree_sitter_devicetree::HIGHLIGHTS_QUERY],
        )
        .comment("//")
        .brackets(&PLAIN),
    Language::new("dockerfile").comment("#"),
    Language::new("elisp")
        .grammar(
            || tree_sitter_elisp::LANGUAGE.into(),
            &[tree_sitter_elisp::HIGHLIGHTS_QUERY],
        )
        .comment(";")
        .indent(IndentStyle::Lisp),
    Language::new("go")
        .grammar(
            || tree_sitter_go::LANGUAGE.into(),
            &[
                include_str!("../queries/go-extra.scm"),
                tree_sitter_go::HIGHLIGHTS_QUERY,
            ],
        )
        .comment("//")
        .brackets(&GO)
        .single_quotes()
        .backticks()
        .first_pattern_wins(),
    Language::new("hcl")
        .grammar(
            || tree_sitter_hcl::LANGUAGE.into(),
            &[include_str!("../queries/hcl.scm")],
        )
        .comment("#")
        .brackets(&PLAIN),
    Language::new("html")
        .grammar(
            || tree_sitter_html::LANGUAGE.into(),
            &[tree_sitter_html::HIGHLIGHTS_QUERY],
        )
        .single_quotes(),
    Language::new("javascript")
        .grammar(
            || tree_sitter_javascript::LANGUAGE.into(),
            &[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                include_str!("../queries/javascript-extra.scm"),
            ],
        )
        .comment("//")
        .brackets(&JAVASCRIPT)
        .single_quotes()
        .backticks(),
    Language::new("json")
        .grammar(
            || tree_sitter_json::LANGUAGE.into(),
            &[tree_sitter_json::HIGHLIGHTS_QUERY],
        )
        .brackets(&PLAIN),
    Language::new("make")
        .grammar(
            || tree_sitter_make::LANGUAGE.into(),
            &[tree_sitter_make::HIGHLIGHTS_QUERY],
        )
        .comment("#"),
    Language::new("markdown")
        .grammar(
            || tree_sitter_md::LANGUAGE.into(),
            &[include_str!("../queries/markdown.scm")],
        )
        .injections(&MARKDOWN)
        .backticks(),
    // What is within the paragraphs, headings and table cells of Markdown.
    Language::new("markdown-inline").grammar(
        || tree_sitter_md::INLINE_LANGUAGE.into(),
        &[include_str!("../queries/markdown-inline.scm")],
    ),
    Language::new("nix")
        .grammar(
            || tree_sitter_nix::LANGUAGE.into(),
            &[tree_sitter_nix::HIGHLIGHTS_QUERY],
        )
        .comment("#")
        .brackets(&NIX)
        .first_pattern_wins(),
    // Org has no grammar: src/org.rs reads its lines.
    Language::new("org").by_lines(),
    Language::new("python")
        .grammar(
            || tree_sitter_python::LANGUAGE.into(),
            &[
                tree_sitter_python::HIGHLIGHTS_QUERY,
                include_str!("../queries/python-extra.scm"),
            ],
        )
        .comment("#")
        .indent(IndentStyle::Offside)
        .single_quotes(),
    Language::new("rust")
        .grammar(
            || tree_sitter_rust::LANGUAGE.into(),
            &[
                tree_sitter_rust::HIGHLIGHTS_QUERY,
                include_str!("../queries/rust-extra.scm"),
            ],
        )
        .comment("//")
        .brackets(&RUST),
    Language::new("scheme")
        .grammar(
            || tree_sitter_scheme::LANGUAGE.into(),
            &[include_str!("../queries/scheme.scm")],
        )
        .comment(";")
        .indent(IndentStyle::Lisp),
    Language::new("toml")
        .grammar(
            || tree_sitter_toml_ng::LANGUAGE.into(),
            &[tree_sitter_toml_ng::HIGHLIGHTS_QUERY],
        )
        .comment("#")
        .brackets(&PLAIN)
        .single_quotes(),
    Language::new("tsx")
        .grammar(
            || tree_sitter_typescript::LANGUAGE_TSX.into(),
            &[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                include_str!("../queries/javascript-extra.scm"),
            ],
        )
        .comment("//")
        .brackets(&JAVASCRIPT)
        .single_quotes()
        .backticks(),
    Language::new("typescript")
        .grammar(
            || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            &[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
                include_str!("../queries/javascript-extra.scm"),
            ],
        )
        .comment("//")
        .brackets(&JAVASCRIPT)
        .single_quotes()
        .backticks(),
    Language::new("yaml")
        .grammar(
            || tree_sitter_yaml::LANGUAGE.into(),
            &[tree_sitter_yaml::HIGHLIGHTS_QUERY],
        )
        .comment("#")
        .indent(IndentStyle::Offside)
        .single_quotes(),
];

/// The language with the given name.
pub fn language(name: &str) -> Option<&'static Language> {
    LANGUAGES.iter().find(|language| language.name == name)
}

/// The language that a name stands for where people name languages, as
/// after the fence of a block of code in Markdown: `rs`, `sh`, `C++`.
pub fn language_called(name: &str) -> Option<&'static Language> {
    const ALIASES: &[(&str, &str)] = &[
        ("c++", "cpp"),
        ("cc", "cpp"),
        ("console", "bash"),
        ("docker", "dockerfile"),
        ("dts", "devicetree"),
        ("el", "elisp"),
        ("emacs-lisp", "elisp"),
        ("golang", "go"),
        ("js", "javascript"),
        ("jsx", "tsx"),
        ("lisp", "elisp"),
        ("makefile", "make"),
        ("md", "markdown"),
        ("py", "python"),
        ("rs", "rust"),
        ("scm", "scheme"),
        ("sh", "bash"),
        ("shell", "bash"),
        ("terraform", "hcl"),
        ("tf", "hcl"),
        ("ts", "typescript"),
        ("yml", "yaml"),
        ("zsh", "bash"),
    ];
    let name = name.trim().to_lowercase();
    let name = ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name.as_str(), |(_, language)| language);
    language(name)
}

/// All languages, in alphabetical order.
pub fn languages() -> impl Iterator<Item = &'static Language> {
    LANGUAGES.iter()
}
