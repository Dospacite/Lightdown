; Lightdown queries for the pinned html grammar.
((element (start_tag) @name) @item (#set! kind "type"))
((script_element (start_tag) @name) @item (#set! kind "type"))
((style_element (start_tag) @name) @item (#set! kind "type"))
