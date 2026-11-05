; Lightdown queries for the pinned javascript grammar.
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
(comment) @comment.around
(comment) @comment.inside
(html_comment) @comment.around
(html_comment) @comment.inside
(arrow_function) @function.around
(arrow_function body: (_) @function.inside)
(function_expression) @function.around
(function_expression body: (_) @function.inside)
(formal_parameters (identifier) @parameter.inside)
(formal_parameters (identifier) @parameter.around)
(arrow_function parameter: (identifier) @parameter.inside)
(arrow_function parameter: (identifier) @parameter.around)
