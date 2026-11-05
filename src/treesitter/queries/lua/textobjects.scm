; Lightdown queries for the pinned lua grammar.
(function_definition) @function.around
(function_definition body: (_) @function.inside)
(function_declaration) @function.around
(function_declaration body: (_) @function.inside)
(comment) @comment.around
(comment) @comment.inside
(parameters name: (identifier) @parameter.inside)
(parameters name: (identifier) @parameter.around)
