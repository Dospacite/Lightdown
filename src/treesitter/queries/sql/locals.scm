; Lightdown queries for the pinned sql grammar.
(function_declaration) @local.scope
(assignment left: (_) @local.definition)
(identifier) @local.reference
(create_function) @local.scope
((function_argument (identifier) @local.definition) (#set! local.role "parameter"))
