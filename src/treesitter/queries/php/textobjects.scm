; Lightdown queries for the pinned php grammar.
(function_definition) @function.around
(function_definition body: (_) @function.inside)
(method_declaration) @function.around
(method_declaration body: (_) @function.inside)
(class_declaration) @class.around
(class_declaration body: (_) @class.inside)
(interface_declaration) @class.around
(interface_declaration body: (_) @class.inside)
(enum_declaration) @class.around
(enum_declaration body: (_) @class.inside)
(trait_declaration) @class.around
(trait_declaration body: (_) @class.inside)
(namespace_definition) @class.around
(namespace_definition body: (_) @class.inside)
(simple_parameter) @parameter.around
(simple_parameter) @parameter.inside
(comment) @comment.around
(comment) @comment.inside
