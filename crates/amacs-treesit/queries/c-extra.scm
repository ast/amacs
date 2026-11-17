; Additions to the C grammar's own query: calls are drawn differently from
; definitions.
(call_expression function: (identifier) @function.call)
(call_expression function: (field_expression field: (field_identifier) @function.call))
