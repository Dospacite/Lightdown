; Lightdown queries for the pinned cpp grammar.
(function_definition) @local.scope
((parameter_declaration declarator: (_) @local.definition) (#set! local.role "parameter"))
((variadic_parameter_declaration declarator: (_) @local.definition) (#set! local.role "parameter"))
(identifier) @local.reference
