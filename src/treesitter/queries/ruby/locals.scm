; Lightdown queries for the pinned ruby grammar.
(method) @local.scope
(singleton_method) @local.scope
(assignment left: (_) @local.definition)
((lambda_parameters (identifier) @local.definition) (#set! local.role "parameter"))
((method_parameters (identifier) @local.definition) (#set! local.role "parameter"))
((optional_parameter name: (_) @local.definition) (#set! local.role "parameter"))
(identifier) @local.reference
