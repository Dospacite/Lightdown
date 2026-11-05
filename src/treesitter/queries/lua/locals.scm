; Lightdown queries for the pinned lua grammar.
(function_definition) @local.scope
(function_declaration) @local.scope
(identifier) @local.reference
((parameters name: (identifier) @local.definition) (#set! local.role "parameter"))
