; Highlighting for HCL and Terraform.

(comment) @comment

(string_lit) @string
(quoted_template) @string
(heredoc_template) @string
[(template_interpolation_start) (template_interpolation_end)] @punctuation.special
(numeric_lit) @number
(bool_lit) @constant.builtin
(null_lit) @constant.builtin

(block (identifier) @keyword)
(block (string_lit) @type)
(attribute (identifier) @property)
(object_elem key: (expression (variable_expr (identifier) @property)))
(function_call (identifier) @function.call)
(get_attr (identifier) @property)

["if" "else" "endif" "for" "endfor" "in"] @keyword
["(" ")" "[" "]" "{" "}"] @punctuation.bracket
[(block_start) (block_end) (object_start) (object_end) (tuple_start) (tuple_end)] @punctuation.bracket
["." "," ".*" "[*]"] @punctuation.delimiter
