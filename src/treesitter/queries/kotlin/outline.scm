; Lightdown queries for the pinned kotlin grammar.
((function_declaration (simple_identifier) @name) @item (#set! kind "function"))
((class_declaration (type_identifier) @name) @item (#set! kind "type"))
((object_declaration (type_identifier) @name) @item (#set! kind "type"))
