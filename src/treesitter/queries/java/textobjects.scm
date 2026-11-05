; Lightdown queries for the pinned java grammar.
(method_declaration) @function.around
(method_declaration body: (_) @function.inside)
(constructor_declaration) @function.around
(constructor_declaration body: (_) @function.inside)
(class_declaration) @class.around
(class_declaration body: (_) @class.inside)
(interface_declaration) @class.around
(interface_declaration body: (_) @class.inside)
(enum_declaration) @class.around
(enum_declaration body: (_) @class.inside)
(record_declaration) @class.around
(record_declaration body: (_) @class.inside)
(formal_parameter) @parameter.around
(formal_parameter) @parameter.inside
(block_comment) @comment.around
(block_comment) @comment.inside
(line_comment) @comment.around
(line_comment) @comment.inside
