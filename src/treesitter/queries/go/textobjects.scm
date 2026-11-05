; Lightdown queries for the pinned go grammar.
(function_declaration) @function.around
(function_declaration body: (_) @function.inside)
(method_declaration) @function.around
(method_declaration body: (_) @function.inside)
(parameter_declaration) @parameter.around
(parameter_declaration) @parameter.inside
(variadic_parameter_declaration) @parameter.around
(variadic_parameter_declaration) @parameter.inside
(comment) @comment.around
(comment) @comment.inside
(type_spec type: (struct_type (field_declaration_list) @class.inside)) @class.around
(type_spec type: (interface_type) @class.inside) @class.around
