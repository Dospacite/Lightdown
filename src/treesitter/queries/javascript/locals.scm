; Lightdown queries for the pinned javascript grammar.
(function_declaration) @local.scope
(generator_function_declaration) @local.scope
(method_definition) @local.scope
(variable_declarator name: (_) @local.definition)
(identifier) @local.reference
(arrow_function) @local.scope
(function_expression) @local.scope
((formal_parameters (identifier) @local.definition) (#set! local.role "parameter"))
((arrow_function parameter: (identifier) @local.definition) (#set! local.role "parameter"))
(statement_block) @local.scope
