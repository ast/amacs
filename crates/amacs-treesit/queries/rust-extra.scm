; Additions to the Rust grammar's own query, which later patterns override.

; Calls are drawn differently from definitions.
(call_expression function: (identifier) @function.call)
(call_expression function: (field_expression field: (field_identifier) @function.call))
(call_expression function: (scoped_identifier name: (identifier) @function.call))
(generic_function function: (identifier) @function.call)
(generic_function function: (field_expression field: (field_identifier) @function.call))
(generic_function function: (scoped_identifier name: (identifier) @function.call))

((identifier) @constructor
 (#match? @constructor "^[A-Z]"))
((identifier) @constant
 (#match? @constant "^[A-Z][A-Z\\d_]+$"))

(line_comment (doc_comment)) @comment.documentation
(block_comment (doc_comment)) @comment.documentation
