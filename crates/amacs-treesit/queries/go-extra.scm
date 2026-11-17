; Additions to the Go grammar's own query, placed before it since there the
; first pattern to match a node decides. Calls are drawn differently from
; definitions.
((call_expression function: (identifier) @function.builtin)
 (#match? @function.builtin "^(append|cap|clear|close|complex|copy|delete|imag|len|make|max|min|new|panic|print|println|real|recover)$"))
(call_expression function: (identifier) @function.call)
(call_expression function: (selector_expression field: (field_identifier) @function.call))
