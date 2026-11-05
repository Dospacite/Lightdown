; Lightdown queries for the pinned c grammar.
(function_definition) @function.around
(function_definition body: (_) @function.inside)
(struct_specifier) @class.around
(struct_specifier body: (_) @class.inside)
(union_specifier) @class.around
(union_specifier body: (_) @class.inside)
(enum_specifier) @class.around
(enum_specifier body: (_) @class.inside)
(parameter_declaration) @parameter.around
(parameter_declaration) @parameter.inside
(comment) @comment.around
(comment) @comment.inside
