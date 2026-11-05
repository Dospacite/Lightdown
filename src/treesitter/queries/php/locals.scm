; Lightdown queries for the pinned php grammar.
(function_definition) @local.scope
(method_declaration) @local.scope
((simple_parameter name: (_) @local.definition) (#set! local.role "parameter"))
(variable_name) @local.reference
