; What is within the paragraphs, headings and table cells of Markdown.

(emphasis) @text.emphasis
(strong_emphasis) @text.strong
(strikethrough) @comment

[
  (code_span)
  (link_title)
] @text.literal

[
  (emphasis_delimiter)
  (code_span_delimiter)
] @punctuation.delimiter

[
  (link_text)
  (link_label)
  (image_description)
  (uri_autolink)
  (email_autolink)
] @text.reference

(link_destination) @string

[
  (backslash_escape)
  (hard_line_break)
  (entity_reference)
  (numeric_character_reference)
] @string.escape

(html_tag) @tag

(image ["!" "[" "]" "(" ")"] @punctuation.delimiter)
(inline_link ["[" "]" "(" ")"] @punctuation.delimiter)
(full_reference_link ["[" "]"] @punctuation.delimiter)
(collapsed_reference_link ["[" "]"] @punctuation.delimiter)
(shortcut_link ["[" "]"] @punctuation.delimiter)
