; Lightdown queries for the pinned sql grammar.
((create_table (object_reference) @name) @item (#set! kind "type"))
((create_function (object_reference) @name) @item (#set! kind "type"))
((create_view (object_reference) @name) @item (#set! kind "type"))
