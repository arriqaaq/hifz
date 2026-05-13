; Symbol extraction queries for TypeScript / TSX.

(function_declaration  name: (identifier) @name) @def.function
(class_declaration     name: (type_identifier) @name) @def.class
(interface_declaration name: (type_identifier) @name) @def.interface
(type_alias_declaration name: (type_identifier) @name) @def.type
(enum_declaration      name: (identifier) @name) @def.enum
(method_definition     name: (property_identifier) @name) @def.method
(method_signature      name: (property_identifier) @name) @def.method
