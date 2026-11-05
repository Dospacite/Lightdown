; Lightdown queries for the pinned swift grammar.
(function_declaration) @function.around
(function_declaration body: (_) @function.inside)
(macro_definition) @function.around
(macro_definition body: (_) @function.inside)
(class_declaration) @class.around
(class_declaration body: (_) @class.inside)
(protocol_declaration) @class.around
(protocol_declaration body: (_) @class.inside)
(parameter) @parameter.around
(parameter) @parameter.inside
(comment) @comment.around
(comment) @comment.inside
(multiline_comment) @comment.around
(multiline_comment) @comment.inside
