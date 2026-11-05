; Lightdown queries for the pinned ruby grammar.
(method) @function.around
(method body: (_) @function.inside)
(singleton_method) @function.around
(singleton_method body: (_) @function.inside)
(class) @class.around
(class body: (_) @class.inside)
(module) @class.around
(module body: (_) @class.inside)
(lambda_parameters (identifier) @parameter.around)
(lambda_parameters (identifier) @parameter.inside)
(method_parameters (identifier) @parameter.around)
(method_parameters (identifier) @parameter.inside)
(optional_parameter) @parameter.around
(optional_parameter) @parameter.inside
(comment) @comment.around
(comment) @comment.inside
