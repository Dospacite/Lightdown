; Lightdown queries for the pinned python grammar.
(function_definition) @function.around
(function_definition body: (_) @function.inside)
(class_definition) @class.around
(class_definition body: (_) @class.inside)
(parameter) @parameter.around
(parameter) @parameter.inside
(comment) @comment.around
(comment) @comment.inside
(parameters (identifier) @parameter.inside)
(parameters (identifier) @parameter.around)
(default_parameter) @parameter.inside
(default_parameter) @parameter.around
(typed_parameter) @parameter.inside
(typed_parameter) @parameter.around
(typed_default_parameter) @parameter.inside
(typed_default_parameter) @parameter.around
