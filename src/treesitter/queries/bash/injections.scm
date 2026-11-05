; Heredoc delimiters can name an installed language, e.g. SQL or python.
(heredoc_redirect
 (heredoc_start) @injection.language
 (heredoc_body) @injection.content)
