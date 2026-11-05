; Lightdown queries for the pinned java grammar.
(method_declaration) @local.scope
(constructor_declaration) @local.scope
((formal_parameter name: (_) @local.definition) (#set! local.role "parameter"))
(variable_declarator name: (_) @local.definition)
(identifier) @local.reference
