; Lightdown queries for the pinned rust grammar.
(function_item) @local.scope
(function_signature_item) @local.scope
(macro_definition) @local.scope
(let_declaration pattern: (_) @local.definition)
((parameter pattern: (_) @local.definition) (#set! local.role "parameter"))
(identifier) @local.reference
(block) @local.scope
(closure_expression) @local.scope
