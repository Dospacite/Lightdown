; Lightdown queries for the pinned ruby grammar.
((method name: (_) @name) @item (#set! kind "function"))
((singleton_method name: (_) @name) @item (#set! kind "function"))
((class name: (_) @name) @item (#set! kind "type"))
((module name: (_) @name) @item (#set! kind "type"))
