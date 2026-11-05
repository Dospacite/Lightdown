; Lightdown queries for the pinned bash grammar.
(function_definition) @function.around
(function_definition body: (_) @function.inside)
(comment) @comment.around
(comment) @comment.inside
