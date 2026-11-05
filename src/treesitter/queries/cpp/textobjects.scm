; Lightdown queries for the pinned cpp grammar.
(function_definition) @function.around
(function_definition body: (_) @function.inside)
(class_specifier) @class.around
(class_specifier body: (_) @class.inside)
(struct_specifier) @class.around
(struct_specifier body: (_) @class.inside)
(union_specifier) @class.around
(union_specifier body: (_) @class.inside)
(enum_specifier) @class.around
(enum_specifier body: (_) @class.inside)
(namespace_definition) @class.around
(namespace_definition body: (_) @class.inside)
(parameter_declaration) @parameter.around
(parameter_declaration) @parameter.inside
(variadic_parameter_declaration) @parameter.around
(variadic_parameter_declaration) @parameter.inside
(comment) @comment.around
(comment) @comment.inside
