; Lightdown queries for the pinned typescript grammar.
(function_declaration) @local.scope
(generator_function_declaration) @local.scope
(method_definition) @local.scope
((optional_parameter pattern: (_) @local.definition) (#set! local.role "parameter"))
((required_parameter pattern: (_) @local.definition) (#set! local.role "parameter"))
(variable_declarator name: (_) @local.definition)
(identifier) @local.reference
(arrow_function) @local.scope
(function_expression) @local.scope
