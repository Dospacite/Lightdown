; Lightdown queries for the pinned kotlin grammar.
(function_declaration) @local.scope
((class_parameter (simple_identifier) @local.definition) (#set! local.role "parameter"))
((parameter (simple_identifier) @local.definition) (#set! local.role "parameter"))
(identifier) @local.reference
