; Lightdown queries for the pinned php grammar.
((function_definition name: (_) @name) @item (#set! kind "function"))
((method_declaration name: (_) @name) @item (#set! kind "function"))
((class_declaration name: (_) @name) @item (#set! kind "type"))
((interface_declaration name: (_) @name) @item (#set! kind "type"))
((enum_declaration name: (_) @name) @item (#set! kind "type"))
((trait_declaration name: (_) @name) @item (#set! kind "type"))
((namespace_definition name: (_) @name) @item (#set! kind "type"))
