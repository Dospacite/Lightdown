; Lightdown queries for the pinned typescript grammar.
(function_declaration) @function.around
(function_declaration body: (_) @function.inside)
(generator_function_declaration) @function.around
(generator_function_declaration body: (_) @function.inside)
(method_definition) @function.around
(method_definition body: (_) @function.inside)
(class_declaration) @class.around
(class_declaration body: (_) @class.inside)
(class) @class.around
(class body: (_) @class.inside)
(module) @class.around
(module body: (_) @class.inside)
(interface_declaration) @class.around
(interface_declaration body: (_) @class.inside)
(enum_declaration) @class.around
(enum_declaration body: (_) @class.inside)
(internal_module) @class.around
(internal_module body: (_) @class.inside)
(optional_parameter) @parameter.around
(optional_parameter) @parameter.inside
(required_parameter) @parameter.around
(required_parameter) @parameter.inside
(comment) @comment.around
(comment) @comment.inside
(html_comment) @comment.around
(html_comment) @comment.inside
(arrow_function) @function.around
(arrow_function body: (_) @function.inside)
(function_expression) @function.around
(function_expression body: (_) @function.inside)
