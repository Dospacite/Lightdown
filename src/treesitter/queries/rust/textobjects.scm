; Lightdown queries for the pinned rust grammar.
(function_item) @function.around
(function_item body: (_) @function.inside)
(function_signature_item) @function.around
(macro_definition) @function.around
(struct_item) @class.around
(struct_item body: (_) @class.inside)
(enum_item) @class.around
(enum_item body: (_) @class.inside)
(union_item) @class.around
(union_item body: (_) @class.inside)
(trait_item) @class.around
(trait_item body: (_) @class.inside)
(mod_item) @class.around
(mod_item body: (_) @class.inside)
(block_comment) @comment.around
(block_comment) @comment.inside
(line_comment) @comment.around
(line_comment) @comment.inside
(parameter) @parameter.around
(parameter) @parameter.inside
(self_parameter) @parameter.around
(self_parameter) @parameter.inside
