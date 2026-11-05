; Lightdown queries for the pinned toml grammar.
((table (bare_key) @name) @item (#set! kind "type"))
((table_array_element (bare_key) @name) @item (#set! kind "type"))
((table (dotted_key) @name) @item (#set! kind "type"))
((table (quoted_key) @name) @item (#set! kind "type"))
((table_array_element (dotted_key) @name) @item (#set! kind "type"))
((table_array_element (quoted_key) @name) @item (#set! kind "type"))
