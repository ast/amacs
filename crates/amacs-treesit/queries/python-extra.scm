; Additions to the Python grammar's own query, which later patterns override.
; Calls are drawn differently from definitions.
(call function: (identifier) @function.call)
(call function: (attribute attribute: (identifier) @function.call))
((call function: (identifier) @function.builtin)
 (#match? @function.builtin "^(abs|all|any|bool|bytes|callable|chr|dict|dir|enumerate|filter|float|format|getattr|hasattr|hash|id|input|int|isinstance|issubclass|iter|len|list|map|max|min|next|object|open|ord|print|range|repr|reversed|round|set|setattr|sorted|str|sum|super|tuple|type|zip)$"))
((call function: (identifier) @constructor)
 (#match? @constructor "^[A-Z]"))

; A string that is the first thing in a module, class or function.
(module . (expression_statement (string) @comment.documentation))
(class_definition body: (block . (expression_statement (string) @comment.documentation)))
(function_definition body: (block . (expression_statement (string) @comment.documentation)))
