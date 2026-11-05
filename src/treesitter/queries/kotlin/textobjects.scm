; Lightdown queries for the pinned kotlin grammar.
(function_declaration) @function.around
(function_declaration (function_body) @function.inside)
(class_declaration) @class.around
(class_declaration (class_body) @class.inside)
(object_declaration) @class.around
(object_declaration (class_body) @class.inside)
(class_parameter) @parameter.around
(class_parameter) @parameter.inside
(parameter) @parameter.around
(parameter) @parameter.inside
(line_comment) @comment.around
(line_comment) @comment.inside
(multiline_comment) @comment.around
(multiline_comment) @comment.inside
