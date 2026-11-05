; Lightdown queries for the pinned python grammar.
(function_definition) @local.scope
(assignment left: (_) @local.definition)
(identifier) @local.reference
((parameters (identifier) @local.definition) (#set! local.role "parameter"))
((typed_parameter (identifier) @local.definition) (#set! local.role "parameter"))
((default_parameter name: (identifier) @local.definition) (#set! local.role "parameter"))
((typed_default_parameter name: (identifier) @local.definition) (#set! local.role "parameter"))
(class_definition) @local.scope
(lambda) @local.scope
((lambda_parameters (identifier) @local.definition) (#set! local.role "parameter"))
