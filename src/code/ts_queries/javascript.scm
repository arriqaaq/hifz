; Symbol extraction queries for JavaScript.

(function_declaration name: (identifier) @name) @def.function
(class_declaration    name: (identifier) @name) @def.class
(method_definition    name: (property_identifier) @name) @def.method
(generator_function_declaration name: (identifier) @name) @def.function
