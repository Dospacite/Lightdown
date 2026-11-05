; Lightdown queries for the pinned zig grammar.
(function_declaration) @function.around
(function_declaration body: (_) @function.inside)
(enum_declaration) @class.around
(parameter) @parameter.around
(parameter) @parameter.inside
(comment) @comment.around
(comment) @comment.inside
