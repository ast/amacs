; The blocks of Markdown. What is within paragraphs, headings and table
; cells is highlighted by markdown-inline.scm.

(atx_heading (inline) @text.title)
(setext_heading (paragraph) @text.title)

[
  (atx_h1_marker)
  (atx_h2_marker)
  (atx_h3_marker)
  (atx_h4_marker)
  (atx_h5_marker)
  (atx_h6_marker)
  (setext_h1_underline)
  (setext_h2_underline)
] @punctuation.special

[
  (indented_code_block)
  (fenced_code_block)
] @text.literal

(fenced_code_block_delimiter) @punctuation.delimiter
(info_string) @type

(link_reference_definition (link_label) @text.reference)
(link_reference_definition (link_destination) @string)
(link_reference_definition (link_title) @text.literal)

[
  (list_marker_plus)
  (list_marker_minus)
  (list_marker_star)
  (list_marker_dot)
  (list_marker_parenthesis)
  (thematic_break)
  (block_quote_marker)
  (block_continuation)
] @punctuation.special

(task_list_marker_unchecked) @keyword
(task_list_marker_checked) @string

; Tables: the bars and the row of dashes step back, the header stands out.
(pipe_table_header (pipe_table_cell) @text.strong)
(pipe_table_delimiter_row) @punctuation.delimiter
(pipe_table_header "|" @punctuation.delimiter)
(pipe_table_row "|" @punctuation.delimiter)

[
  (minus_metadata)
  (plus_metadata)
] @comment

(backslash_escape) @string.escape
