; Symbol extraction queries for Python.

(function_definition name: (identifier) @name) @def.function
(class_definition  name: (identifier) @name) @def.class
(decorated_definition (function_definition name: (identifier) @name)) @def.function
(decorated_definition (class_definition    name: (identifier) @name)) @def.class
