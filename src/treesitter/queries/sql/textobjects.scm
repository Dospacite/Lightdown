; Lightdown queries for the pinned sql grammar.
(function_declaration) @function.around
(comment) @comment.around
(comment) @comment.inside
(parameter) @parameter.around
(parameter) @parameter.inside
(create_function) @function.around
(create_function (function_body) @function.inside)
(function_argument) @parameter.inside
(function_argument) @parameter.around
