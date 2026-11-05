; Lightdown queries for the pinned go grammar.
(function_declaration) @local.scope
(method_declaration) @local.scope
((parameter_declaration name: (_) @local.definition) (#set! local.role "parameter"))
(short_var_declaration left: (_) @local.definition)
((variadic_parameter_declaration name: (_) @local.definition) (#set! local.role "parameter"))
(identifier) @local.reference
