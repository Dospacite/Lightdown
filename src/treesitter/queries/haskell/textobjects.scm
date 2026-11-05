; Lightdown queries for the pinned haskell grammar.
(function) @function.around
(function match: (_) @function.inside)
(class) @class.around
(class declarations: (_) @class.inside)
(data_type) @class.around
(data_type constructors: (_) @class.inside)
(newtype) @class.around
(newtype constructors: (_) @class.inside)
(comment) @comment.around
(comment) @comment.inside
(function patterns: (patterns (variable) @parameter.inside))
(function patterns: (patterns (variable) @parameter.around))
