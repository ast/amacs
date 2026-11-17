; Additions to the JavaScript grammar's own query, which later patterns
; override. Calls are drawn differently from definitions.
(call_expression function: (identifier) @function.call)
(call_expression function: (member_expression property: (property_identifier) @function.call))
((identifier) @function.builtin
 (#eq? @function.builtin "require"))
