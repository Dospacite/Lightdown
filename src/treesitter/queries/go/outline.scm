; Lightdown queries for the pinned go grammar.
((function_declaration name: (_) @name) @item (#set! kind "function"))
((method_declaration name: (_) @name) @item (#set! kind "function"))
((type_spec name: (_) @name) @item (#set! kind "type"))
