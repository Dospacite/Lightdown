; Lightdown queries for the pinned cpp grammar.
((function_definition declarator: (function_declarator declarator: (_) @name)) @item (#set! kind "function"))
((function_definition declarator: (pointer_declarator declarator: (function_declarator declarator: (_) @name))) @item (#set! kind "function"))
((function_definition declarator: (pointer_declarator declarator: (pointer_declarator declarator: (function_declarator declarator: (_) @name)))) @item (#set! kind "function"))
((class_specifier name: (_) @name body: (_)) @item (#set! kind "type"))
((struct_specifier name: (_) @name body: (_)) @item (#set! kind "type"))
((union_specifier name: (_) @name body: (_)) @item (#set! kind "type"))
((enum_specifier name: (_) @name body: (_)) @item (#set! kind "type"))
((namespace_definition name: (_) @name) @item (#set! kind "type"))
