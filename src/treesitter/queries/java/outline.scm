; Lightdown queries for the pinned java grammar.
((method_declaration name: (_) @name) @item (#set! kind "function"))
((constructor_declaration name: (_) @name) @item (#set! kind "function"))
((class_declaration name: (_) @name) @item (#set! kind "type"))
((interface_declaration name: (_) @name) @item (#set! kind "type"))
((enum_declaration name: (_) @name) @item (#set! kind "type"))
((record_declaration name: (_) @name) @item (#set! kind "type"))
