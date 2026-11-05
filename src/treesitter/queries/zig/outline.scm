; Lightdown queries for the pinned zig grammar.
((function_declaration name: (_) @name) @item (#set! kind "function"))
((variable_declaration (identifier) @name) @item (#set! kind "type"))
((test_declaration (string) @name) @item (#set! kind "type"))
